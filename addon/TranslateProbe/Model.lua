-- A translation model shaped like Mozilla's Firefox Translations "tiny" student, filled with
-- random int8 weights. Its output is meaningless: it exists to time, inside the real client,
-- exactly the work a local neural translator would do. Everything here runs inside Scheduler
-- jobs and calls Scheduler.Check between small units of work, so no step blocks a frame.
local _, ns = ...

local Model = {}
ns.Model = Model

local byte, char, sub = string.byte, string.char, string.sub
local exp, sqrt, floor, random = math.exp, math.sqrt, math.floor, math.random
local concat = table.concat
local Check = ns.Scheduler.Check

-- Tiny architecture: 6-layer encoder, 2-layer SSRU decoder, width 256, 8 heads, feed-forward
-- 1536, 32k shared vocabulary, output projection limited to a lexical shortlist of SL rows.
-- The embedding is stored in blocks of ROWS_PER_BLOCK rows (64 KB) so no multi-MB string is
-- ever built in one slice.
local cfg = { D = 256, H = 8, FF = 1536, V = 32000, SL = 500, ENC = 6, DEC = 2, POOL_KB = 256, ROWS_PER_BLOCK = 256 }
local PIECE = 65536          -- largest string copied between two Check() calls
local GROUP = 64             -- 1 KB chunks joined per Check() when building random bytes
local MAX_SRC_PIECES = 80    -- a 255-character chat line is roughly 50 words

local weights        -- built model, nil until Build runs
local buildQueued = false
local macs = 0       -- multiply-adds counted by the current forward pass

-- Tests shrink the model so it runs fast in a JS-hosted Lua.
function Model.Configure(overrides)
    for key, value in pairs(overrides) do
        cfg[key] = value
    end
    weights = nil
end

function Model.IsBuilt()
    return weights ~= nil
end

function Model.IsBuildQueued()
    return buildQueued
end

function Model.Free()
    weights = nil
end

function Model.ShortlistRows()
    return cfg.SL
end

-- Bytes of int8 weights the configuration needs: tied embedding plus every layer. This is the
-- weights alone; a real translator also needs its vocabulary and shortlist tables.
function Model.ExpectedBytes()
    local D, FF = cfg.D, cfg.FF
    local encLayer = 4 * D * D + 2 * D * FF
    local decLayer = 6 * D * D + 2 * D * FF
    return cfg.V * D + cfg.ENC * encLayer + cfg.DEC * decLayer
end

function Model.Bytes()
    return weights and weights.bytes or 0
end

-- n random bytes, built 1 KB at a time and joined 64 KB at a time between Check() calls.
-- normal = true centres the bytes on 128 like quantized weights; otherwise uniform.
function Model.RandomBytes(n, normal)
    local groups, parts, chunk = {}, {}, {}
    local left = n
    while left > 0 do
        local size = math.min(1024, left)
        for i = 1, size do
            local v
            if normal then
                v = floor(128 + (random() + random() + random() + random() - 2) * 43 + 0.5)
                if v < 0 then
                    v = 0
                elseif v > 255 then
                    v = 255
                end
            else
                v = random(0, 255)
            end
            chunk[i] = v
        end
        parts[#parts + 1] = char(unpack(chunk, 1, size))
        left = left - size
        Check()
        if #parts == GROUP or left == 0 then
            groups[#groups + 1] = concat(parts)
            parts = {}
            Check()
        end
    end
    return concat(groups)
end

-- n bytes copied from random places of the pool, at most PIECE bytes per step.
local function Int8String(pool, n)
    local size = #pool
    local step = math.min(PIECE, size)
    local pieces, got = {}, 0
    while got < n do
        local take = math.min(n - got, step)
        local start = random(1, size - take + 1)
        pieces[#pieces + 1] = sub(pool, start, start + take - 1)
        got = got + take
        Check()
    end
    if #pieces == 1 then
        return pieces[1]
    end
    local joined = concat(pieces)
    Check()
    return joined
end

local function NewMatrix(pool, rows, cols)
    return { s = Int8String(pool, rows * cols), rows = rows, cols = cols, scale = 1 / (25 * sqrt(cols)) }
end

-- Builds the weights. Run it only through Model.QueueBuild, never inside a timed job.
function Model.Build()
    if weights then
        return
    end
    local D, FF, RPB = cfg.D, cfg.FF, cfg.ROWS_PER_BLOCK
    local pool = Model.RandomBytes(cfg.POOL_KB * 1024, true)
    local w = { enc = {}, dec = {}, shortlist = {}, E = {} }
    local rowsLeft = cfg.V
    while rowsLeft > 0 do
        local rows = math.min(RPB, rowsLeft)
        w.E[#w.E + 1] = Int8String(pool, rows * D)
        rowsLeft = rowsLeft - rows
    end
    for l = 1, cfg.ENC do
        w.enc[l] = {
            q = NewMatrix(pool, D, D), k = NewMatrix(pool, D, D),
            v = NewMatrix(pool, D, D), o = NewMatrix(pool, D, D),
            f1 = NewMatrix(pool, FF, D), f2 = NewMatrix(pool, D, FF),
        }
    end
    for l = 1, cfg.DEC do
        w.dec[l] = {
            rw = NewMatrix(pool, D, D), rf = NewMatrix(pool, D, D),
            cq = NewMatrix(pool, D, D), ck = NewMatrix(pool, D, D),
            cv = NewMatrix(pool, D, D), co = NewMatrix(pool, D, D),
            f1 = NewMatrix(pool, FF, D), f2 = NewMatrix(pool, D, FF),
        }
    end
    for r = 1, cfg.SL do
        w.shortlist[r] = floor(random() * cfg.V) + 1
    end
    w.bytes = Model.ExpectedBytes()
    weights = w
end

-- Queues one build job unless the model exists or a build is already queued. onDone(job, ok)
-- runs when that build finishes. Returns true when a build was queued by this call.
function Model.QueueBuild(onDone)
    if weights or buildQueued then
        return false
    end
    buildQueued = true
    ns.Scheduler.Enqueue({
        run = Model.Build,
        done = function(job, ok, err)
            buildQueued = false
            if onDone then
                onDone(job, ok, err)
            end
        end,
    })
    return true
end

local Kernels = ns.Kernels
local reference = false   -- true: the original one-row kernel, used by tests as the ground truth

-- Tests compare the fast kernels with the reference: both must give bit-identical results.
function Model.SetReference(on)
    reference = on and true or false
end

-- y = W x. Bytes hold q + 128; the offset is removed once per row through sum(x).
local function MatVec(M, x, y)
    local S, cols, rows, scale = M.s, M.cols, M.rows, M.scale
    local sumx = 0
    for i = 1, cols do
        sumx = sumx + x[i]
    end
    if reference then
        Kernels.Row16(S, cols, x, y, 1, rows, Check)
    else
        Kernels.Local16(S, cols, x, y, 1, rows, Check)
    end
    local offset = 128 * sumx
    for j = 1, rows do
        y[j] = scale * (y[j] - offset)
    end
    macs = macs + rows * cols
    return y
end

-- Four tokens through the same matrix, each extracted byte serving all four.
local function MatVec4(M, xa, xb, xc, xd, ya, yb, yc, yd)
    local S, cols, rows, scale = M.s, M.cols, M.rows, M.scale
    local sa, sb, sc, sd = 0, 0, 0, 0
    for i = 1, cols do
        sa, sb, sc, sd = sa + xa[i], sb + xb[i], sc + xc[i], sd + xd[i]
    end
    Kernels.Batch4(S, cols, xa, xb, xc, xd, ya, yb, yc, yd, 1, rows, Check)
    local oa, ob, oc, od = 128 * sa, 128 * sb, 128 * sc, 128 * sd
    for j = 1, rows do
        ya[j], yb[j] = scale * (ya[j] - oa), scale * (yb[j] - ob)
        yc[j], yd[j] = scale * (yc[j] - oc), scale * (yd[j] - od)
    end
    macs = macs + 4 * rows * cols
end

-- Y[t] = W X[t] for t = 1..n, four tokens at a time when possible.
local function MatVecMany(M, X, Y, n)
    local t = 1
    if not reference then
        while t + 3 <= n do
            MatVec4(M, X[t], X[t + 1], X[t + 2], X[t + 3], Y[t], Y[t + 1], Y[t + 2], Y[t + 3])
            t = t + 4
        end
    end
    while t <= n do
        MatVec(M, X[t], Y[t])
        t = t + 1
    end
end

local function NewVector(n)
    local v = table.create and table.create(n) or {}
    for i = 1, n do
        v[i] = 0
    end
    return v
end

local function NewVectors(count, n)
    local list = {}
    for t = 1, count do
        list[t] = NewVector(n)
    end
    return list
end

-- Embedding row of a token: block string and byte offset of that row inside it.
local function EmbeddingRow(tok)
    local RPB, D = cfg.ROWS_PER_BLOCK, cfg.D
    return weights.E[floor((tok - 1) / RPB) + 1], ((tok - 1) % RPB) * D
end

local function Embed(tok, x)
    local D = cfg.D
    local block, base = EmbeddingRow(tok)
    x = x or NewVector(D)
    for i = 1, D do
        x[i] = (byte(block, base + i) - 128) * 0.01
    end
    return x
end

local function LayerNorm(x)
    local D = cfg.D
    local mean = 0
    for i = 1, D do
        mean = mean + x[i]
    end
    mean = mean / D
    local var = 0
    for i = 1, D do
        local d = x[i] - mean
        var = var + d * d
    end
    local inv = 1 / sqrt(var / D + 1e-6)
    for i = 1, D do
        x[i] = (x[i] - mean) * inv
    end
    return x
end

local function AddInto(x, a)
    for i = 1, cfg.D do
        x[i] = x[i] + a[i]
    end
    return x
end

local function Relu(v, n)
    for i = 1, n do
        if v[i] < 0 then
            v[i] = 0
        end
    end
    return v
end

-- Multi-head attention of one query over n key/value vectors, written into ctx.
local function Attend(q, keys, values, n, ctx, scores)
    local D, H = cfg.D, cfg.H
    local DH = D / H
    local inv = 1 / sqrt(DH)
    for h = 0, H - 1 do
        local o = h * DH
        local best = -1e30
        for u = 1, n do
            local k, s = keys[u], 0
            for d = o + 1, o + DH do
                s = s + q[d] * k[d]
            end
            s = s * inv
            scores[u] = s
            if s > best then
                best = s
            end
        end
        local sum = 0
        for u = 1, n do
            local e = exp(scores[u] - best)
            scores[u] = e
            sum = sum + e
        end
        for d = o + 1, o + DH do
            ctx[d] = 0
        end
        for u = 1, n do
            local p, v = scores[u] / sum, values[u]
            for d = o + 1, o + DH do
                ctx[d] = ctx[d] + p * v[d]
            end
        end
    end
    macs = macs + 2 * n * D
    Check()
    return ctx
end

-- Every source token goes through the same matrices, so each layer runs them batched. All
-- buffers are allocated once per pass and reused by every layer.
local function Encode(src)
    local D, FF = cfg.D, cfg.FF
    local n = #src
    local X = {}
    for t = 1, n do
        X[t] = Embed(src[t])
    end
    local Q, K, V = NewVectors(n, D), NewVectors(n, D), NewVectors(n, D)
    local C, O, Hd = NewVectors(n, D), NewVectors(n, D), NewVectors(n, FF)
    local scores = NewVector(n)
    for l = 1, cfg.ENC do
        local layer = weights.enc[l]
        MatVecMany(layer.q, X, Q, n)
        MatVecMany(layer.k, X, K, n)
        MatVecMany(layer.v, X, V, n)
        for t = 1, n do
            Attend(Q[t], K, V, n, C[t], scores)
        end
        MatVecMany(layer.o, C, O, n)
        for t = 1, n do
            LayerNorm(AddInto(X[t], O[t]))
        end
        MatVecMany(layer.f1, X, Hd, n)
        for t = 1, n do
            Relu(Hd[t], FF)
        end
        MatVecMany(layer.f2, Hd, O, n)
        for t = 1, n do
            LayerNorm(AddInto(X[t], O[t]))
        end
    end
    return X
end

-- The decoder makes one token at a time, so only the one-token kernel applies there.
local function Decode(enc, steps)
    local D, FF = cfg.D, cfg.FF
    local n = #enc
    local crossK, crossV, cells = {}, {}, {}
    for l = 1, cfg.DEC do
        crossK[l], crossV[l] = NewVectors(n, D), NewVectors(n, D)
        MatVecMany(weights.dec[l].ck, enc, crossK[l], n)
        MatVecMany(weights.dec[l].cv, enc, crossV[l], n)
        cells[l] = NewVector(D)
    end
    local ctx, scores, hidden, tmp = NewVector(D), NewVector(n), NewVector(FF), NewVector(D)
    local fx, wx, q, x = NewVector(D), NewVector(D), NewVector(D), NewVector(D)
    local shortlist, tok, out = weights.shortlist, 1, {}
    for step = 1, steps do
        Embed(tok, x)
        for l = 1, cfg.DEC do
            local layer, c = weights.dec[l], cells[l]
            -- SSRU: f = sigmoid(Wf x), c = f*c + (1-f)*(W x), out = relu(c)
            MatVec(layer.rf, x, fx)
            MatVec(layer.rw, x, wx)
            for i = 1, D do
                local f = 1 / (1 + exp(-fx[i]))
                local ci = f * c[i] + (1 - f) * wx[i]
                c[i] = ci
                tmp[i] = ci > 0 and ci or 0
            end
            LayerNorm(AddInto(x, tmp))
            Attend(MatVec(layer.cq, x, q), crossK[l], crossV[l], n, ctx, scores)
            LayerNorm(AddInto(x, MatVec(layer.co, ctx, tmp)))
            Relu(MatVec(layer.f1, x, hidden), FF)
            LayerNorm(AddInto(x, MatVec(layer.f2, hidden, tmp)))
        end
        -- Output projection over the shortlist rows of the tied embedding, greedy pick.
        local best, bestRow = -1e30, 1
        for r = 1, #shortlist do
            local E, base = EmbeddingRow(shortlist[r])
            local acc = 0
            for i = 1, D, 16 do
                local p = base + i
                local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16 = byte(E, p, p + 15)
                acc = acc + a1 * x[i] + a2 * x[i + 1] + a3 * x[i + 2] + a4 * x[i + 3]
                    + a5 * x[i + 4] + a6 * x[i + 5] + a7 * x[i + 6] + a8 * x[i + 7]
                    + a9 * x[i + 8] + a10 * x[i + 9] + a11 * x[i + 10] + a12 * x[i + 11]
                    + a13 * x[i + 12] + a14 * x[i + 13] + a15 * x[i + 14] + a16 * x[i + 15]
            end
            if acc > best then
                best, bestRow = acc, r
            end
            if r % 16 == 0 then
                Check()
            end
        end
        macs = macs + #shortlist * D
        tok = shortlist[bestRow]
        out[step] = tok
    end
    return out
end

-- One full translation pass: srcPieces in, tgtPieces out. Returns the multiply-adds done.
-- The model must already be built (Model.QueueBuild), so a timed job never includes the build.
function Model.Forward(srcPieces, tgtPieces, src)
    if not weights then
        error("simulated model is not built")
    end
    macs = 0
    if not src then
        src = {}
        for t = 1, srcPieces do
            src[t] = floor(random() * cfg.V) + 1
        end
    end
    local out = Decode(Encode(src), tgtPieces)
    return macs, out
end

-- Multiply-adds of the batchable part (encoder and cross-attention keys/values) and of the
-- one-token decoder steps, for projections with per-kernel speeds.
function Model.EstimateMacsSplit(srcPieces, tgtPieces)
    local D, FF, n = cfg.D, cfg.FF, srcPieces
    local batched = cfg.ENC * n * (4 * D * D + 2 * D * FF + 2 * n * D) + cfg.DEC * n * 2 * D * D
    return batched, Model.EstimateMacs(srcPieces, tgtPieces) - batched
end

-- Multiply-adds of one pass, from the shapes alone (matches what Forward counts).
function Model.EstimateMacs(srcPieces, tgtPieces)
    local D, FF, n = cfg.D, cfg.FF, srcPieces
    local encoder = cfg.ENC * n * (4 * D * D + 2 * D * FF + 2 * n * D)
    local crossKV = cfg.DEC * n * 2 * D * D
    local perStep = cfg.DEC * (4 * D * D + 2 * D * FF + 2 * n * D) + cfg.SL * D
    return encoder + crossKV + tgtPieces * perStep
end

-- Output length used everywhere for a given input: about 10 percent longer, plus the end mark.
function Model.TargetFor(srcPieces)
    return floor(srcPieces * 1.1 + 0.5) + 1
end

-- Rough SentencePiece count for chat text: about 1.5 pieces per word. Returns src, tgt and
-- whether the line was longer than the cap.
function Model.EstimatePieces(wordCount)
    local wanted = floor(wordCount * 1.5 + 0.5)
    local src = math.min(MAX_SRC_PIECES, math.max(2, wanted))
    return src, Model.TargetFor(src), wanted > MAX_SRC_PIECES
end
