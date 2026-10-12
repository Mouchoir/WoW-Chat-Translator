-- A translation model shaped like Mozilla's Firefox Translations "tiny" student, filled with
-- random int8 weights. Its output is meaningless: it exists to time, inside the real client,
-- exactly the work a local neural translator would do. Every function here must run inside a
-- Scheduler job, because it calls Scheduler.Check to spread the work over frames.
local _, ns = ...

local Model = {}
ns.Model = Model

local byte, char, sub = string.byte, string.char, string.sub
local exp, sqrt, floor, random = math.exp, math.sqrt, math.floor, math.random
local concat = table.concat
local Check = ns.Scheduler.Check

-- Tiny architecture: 6-layer encoder, 2-layer SSRU decoder, width 256, 8 heads, feed-forward
-- 1536, 32k shared vocabulary, output projection limited to a lexical shortlist.
local cfg = { D = 256, H = 8, FF = 1536, V = 32000, SL = 500, ENC = 6, DEC = 2, POOL_KB = 1024 }

local weights        -- built model, nil until Build runs
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

function Model.Free()
    weights = nil
end

-- Bytes of int8 weights the configuration needs: tied embedding plus every layer.
function Model.ExpectedBytes()
    local D, FF = cfg.D, cfg.FF
    local encLayer = 4 * D * D + 2 * D * FF
    local decLayer = 6 * D * D + 2 * D * FF
    return cfg.V * D + cfg.ENC * encLayer + cfg.DEC * decLayer
end

function Model.Bytes()
    return weights and weights.bytes or 0
end

-- Random bytes centred on 128 (roughly normal), reused to fill every matrix.
local function BuildPool()
    local parts, chunk = {}, {}
    for k = 1, cfg.POOL_KB do
        for i = 1, 1024 do
            local v = floor(128 + (random() + random() + random() + random() - 2) * 43 + 0.5)
            if v < 0 then
                v = 0
            elseif v > 255 then
                v = 255
            end
            chunk[i] = v
        end
        parts[k] = char(unpack(chunk, 1, 1024))
        Check()
    end
    return concat(parts)
end

local function Int8String(pool, cursor, n)
    local pieces, got, size = {}, 0, #pool
    while got < n do
        local start = (cursor.pos % (size - 1)) + 1
        local take = math.min(n - got, size - start + 1)
        pieces[#pieces + 1] = sub(pool, start, start + take - 1)
        got = got + take
        cursor.pos = cursor.pos + take + 4099
    end
    Check()
    return concat(pieces)
end

local function NewMatrix(pool, cursor, rows, cols)
    return { s = Int8String(pool, cursor, rows * cols), rows = rows, cols = cols, scale = 1 / (25 * sqrt(cols)) }
end

function Model.Build()
    local D, FF = cfg.D, cfg.FF
    local pool = BuildPool()
    local cursor = { pos = 0 }
    local w = { enc = {}, dec = {}, shortlist = {} }
    w.E = Int8String(pool, cursor, cfg.V * D)
    for l = 1, cfg.ENC do
        w.enc[l] = {
            q = NewMatrix(pool, cursor, D, D), k = NewMatrix(pool, cursor, D, D),
            v = NewMatrix(pool, cursor, D, D), o = NewMatrix(pool, cursor, D, D),
            f1 = NewMatrix(pool, cursor, FF, D), f2 = NewMatrix(pool, cursor, D, FF),
        }
    end
    for l = 1, cfg.DEC do
        w.dec[l] = {
            rw = NewMatrix(pool, cursor, D, D), rf = NewMatrix(pool, cursor, D, D),
            cq = NewMatrix(pool, cursor, D, D), ck = NewMatrix(pool, cursor, D, D),
            cv = NewMatrix(pool, cursor, D, D), co = NewMatrix(pool, cursor, D, D),
            f1 = NewMatrix(pool, cursor, FF, D), f2 = NewMatrix(pool, cursor, D, FF),
        }
    end
    for r = 1, cfg.SL do
        w.shortlist[r] = floor(random() * cfg.V) + 1
    end
    w.bytes = Model.ExpectedBytes()
    weights = w
end

-- y = W x for int8 weights stored row by row in one string, 16 bytes per string.byte call.
-- Bytes hold q + 128; the offset is removed once per row through sum(x).
local function MatVec(M, x, y)
    local S, cols, rows, scale = M.s, M.cols, M.rows, M.scale
    local sumx = 0
    for i = 1, cols do
        sumx = sumx + x[i]
    end
    local offset = 128 * sumx
    for j = 1, rows do
        local acc, base = 0, (j - 1) * cols
        for i = 1, cols, 16 do
            local p = base + i
            local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16 = byte(S, p, p + 15)
            acc = acc + a1 * x[i] + a2 * x[i + 1] + a3 * x[i + 2] + a4 * x[i + 3]
                + a5 * x[i + 4] + a6 * x[i + 5] + a7 * x[i + 6] + a8 * x[i + 7]
                + a9 * x[i + 8] + a10 * x[i + 9] + a11 * x[i + 10] + a12 * x[i + 11]
                + a13 * x[i + 12] + a14 * x[i + 13] + a15 * x[i + 14] + a16 * x[i + 15]
        end
        y[j] = scale * (acc - offset)
        Check()
    end
    macs = macs + rows * cols
    return y
end
Model.MatVec = MatVec

local function NewVector(n)
    local v = table.create and table.create(n) or {}
    for i = 1, n do
        v[i] = 0
    end
    return v
end

local function Embed(tok)
    local D, E = cfg.D, weights.E
    local x, base = NewVector(D), (tok - 1) * D
    for i = 1, D do
        x[i] = (byte(E, base + i) - 128) * 0.01
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

local function Encode(src)
    local D, FF = cfg.D, cfg.FF
    local n = #src
    local X = {}
    for t = 1, n do
        X[t] = Embed(src[t])
    end
    local ctx, scores, hidden, tmp = NewVector(D), NewVector(n), NewVector(FF), NewVector(D)
    for l = 1, cfg.ENC do
        local layer = weights.enc[l]
        local Q, K, V = {}, {}, {}
        for t = 1, n do
            Q[t] = MatVec(layer.q, X[t], NewVector(D))
            K[t] = MatVec(layer.k, X[t], NewVector(D))
            V[t] = MatVec(layer.v, X[t], NewVector(D))
        end
        for t = 1, n do
            Attend(Q[t], K, V, n, ctx, scores)
            local x = LayerNorm(AddInto(X[t], MatVec(layer.o, ctx, tmp)))
            Relu(MatVec(layer.f1, x, hidden), FF)
            X[t] = LayerNorm(AddInto(x, MatVec(layer.f2, hidden, tmp)))
        end
    end
    return X
end

local function Decode(enc, steps)
    local D, FF, E = cfg.D, cfg.FF, weights.E
    local n = #enc
    local crossK, crossV, cells = {}, {}, {}
    for l = 1, cfg.DEC do
        crossK[l], crossV[l] = {}, {}
        for u = 1, n do
            crossK[l][u] = MatVec(weights.dec[l].ck, enc[u], NewVector(D))
            crossV[l][u] = MatVec(weights.dec[l].cv, enc[u], NewVector(D))
        end
        cells[l] = NewVector(D)
    end
    local ctx, scores, hidden, tmp = NewVector(D), NewVector(n), NewVector(FF), NewVector(D)
    local fx, wx, q = NewVector(D), NewVector(D), NewVector(D)
    local shortlist, tok = weights.shortlist, 1
    for _ = 1, steps do
        local x = Embed(tok)
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
            x = LayerNorm(AddInto(x, tmp))
            Attend(MatVec(layer.cq, x, q), crossK[l], crossV[l], n, ctx, scores)
            x = LayerNorm(AddInto(x, MatVec(layer.co, ctx, tmp)))
            Relu(MatVec(layer.f1, x, hidden), FF)
            x = LayerNorm(AddInto(x, MatVec(layer.f2, hidden, tmp)))
        end
        -- Output projection over the shortlist rows of the tied embedding, greedy pick.
        local best, bestRow = -1e30, 1
        for r = 1, #shortlist do
            local base, acc = (shortlist[r] - 1) * D, 0
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
    end
end

-- Runs one full translation pass: srcPieces in, tgtPieces out. Returns the multiply-adds done.
function Model.Forward(srcPieces, tgtPieces)
    if not weights then
        Model.Build()
    end
    macs = 0
    local src = {}
    for t = 1, srcPieces do
        src[t] = floor(random() * cfg.V) + 1
    end
    Decode(Encode(src), tgtPieces)
    return macs
end

-- Multiply-adds of one pass, from the shapes alone (matches what Forward counts).
function Model.EstimateMacs(srcPieces, tgtPieces)
    local D, FF, n = cfg.D, cfg.FF, srcPieces
    local encoder = cfg.ENC * n * (4 * D * D + 2 * D * FF + 2 * n * D)
    local crossKV = cfg.DEC * n * 2 * D * D
    local perStep = cfg.DEC * (4 * D * D + 2 * D * FF + 2 * n * D) + cfg.SL * D
    return encoder + crossKV + tgtPieces * perStep
end

-- Rough SentencePiece count for chat text: about 1.5 pieces per word, plus one for the end.
function Model.EstimatePieces(wordCount)
    local src = math.min(48, math.max(2, floor(wordCount * 1.5 + 0.5)))
    return src, floor(src * 1.1 + 0.5) + 1
end
