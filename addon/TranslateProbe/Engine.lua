-- Real neural translation with the Mozilla "tiny" models converted by tools/convert_model.py.
-- The math mirrors tools/marian_ref.py, which follows Marian's int8 inference: activations are
-- rounded to int8 with each matrix's alpha, products are integer sums (exact in Lua numbers),
-- then y = sum / (alpha * qB) + bias. Same formulas in the same order give the same tokens as
-- the Python reference. Every function here runs inside a Scheduler job (it calls Check).
local _, ns = ...

local Engine = {}
ns.Engine = Engine

local byte, sub = string.byte, string.sub
local floor, sqrt, exp, log, sin, cos = math.floor, math.sqrt, math.exp, math.log, math.sin, math.cos
local Check = ns.Scheduler.Check
local Kernels = ns.Kernels

local MAX_SOURCE_PIECES = 128   -- Firefox splits longer input; chat lines stay below this
local MAX_LENGTH_FACTOR = 2.0   -- output stops at 2x the source length

local models = {}               -- pair -> ready model
local packAddOn = function(pair)
    return "TranslateProbe_" .. pair:gsub("-", "")
end

local function DecodeBase64(s)
    if C_EncodingUtil and C_EncodingUtil.DecodeBase64 then
        return C_EncodingUtil.DecodeBase64(s)
    end
    return ns.Base64Decode(s)
end

function Engine.IsReady(pair)
    return models[pair] ~= nil
end

function Engine.HasPack(pair)
    if TranslateProbeModels and TranslateProbeModels[pair] then
        return true
    end
    if C_AddOns and C_AddOns.GetAddOnInfo then
        local name = C_AddOns.GetAddOnInfo(packAddOn(pair))
        return name ~= nil
    end
    return false
end

function Engine.Free(pair)
    models[pair] = nil
end

-- Loads a pack (LoadOnDemand addon) and decodes its data, one blob per Check. Job code only.
function Engine.Load(pair)
    if models[pair] then
        return models[pair]
    end
    local pack = TranslateProbeModels and TranslateProbeModels[pair]
    if not pack then
        local loaded, reason = C_AddOns.LoadAddOn(packAddOn(pair))
        pack = TranslateProbeModels and TranslateProbeModels[pair]
        if not pack then
            error("translation pack " .. packAddOn(pair) .. " not loaded: " .. tostring(reason or loaded))
        end
    end
    local cfg, blobs, floats = pack.config, pack.blobs, pack.floats
    local m = { pair = pair, cfg = cfg, mats = {}, E = {}, floats = floats }
    for _, info in ipairs(pack.matrices) do
        local s = blobs[info.name] and DecodeBase64(blobs[info.name])
        if not s or #s ~= info.rows * info.cols then
            error("bad matrix " .. info.name .. " in " .. pair)
        end
        blobs[info.name] = nil
        local biasName = info.name:gsub("_W(%w*)$", "_b%1")
        m.mats[info.name] = {
            s = s, rows = info.rows, cols = info.cols, alpha = info.alpha, qB = info.qB,
            scale = 1.0 / (info.alpha * info.qB), b = floats[biasName],
        }
        Check()
    end
    for k = 1, cfg.embBlocks do
        local s = DecodeBase64(blobs["Wemb#" .. k])
        blobs["Wemb#" .. k] = nil
        m.E[k] = s
        Check()
    end
    m.lexOffsets = DecodeBase64(blobs.lex_offsets)
    m.lexIds = DecodeBase64(blobs.lex_ids)
    blobs.lex_offsets, blobs.lex_ids = nil, nil
    m.tok = ns.Tokenizer.Prepare(pack, Check)
    m.outB = floats.decoder_ff_logit_out_b
    m.embInv = 1.0 / cfg.wembQB
    m.outScale = 1.0 / (cfg.outAlpha * cfg.wembQB)
    m.sqrtD = sqrt(cfg.D)
    m.pe = {}
    local maxCols = math.max(cfg.D, cfg.FF)
    m.qa = { {}, {}, {}, {} }
    for t = 1, 4 do
        for k = 1, maxCols do
            m.qa[t][k] = 0
        end
    end
    models[pair] = m
    return m
end

-- qa = clamp(round half to even(x * alpha), -127, 127), as Marian's int8 activations.
local function Quantize(x, n, alpha, qa)
    local sum = 0
    for k = 1, n do
        local v = x[k] * alpha
        local r = floor(v)
        local frac = v - r
        if frac > 0.5 or (frac == 0.5 and r % 2 == 1) then
            r = r + 1
        end
        if r > 127 then
            r = 127
        elseif r < -127 then
            r = -127
        end
        qa[k] = r
        sum = sum + r
    end
    return sum
end

-- y = W x + b with int8 weights (bytes hold q + 128) and int8 activations.
local function Linear(model, mat, x, y)
    local qa = model.qa[1]
    local sumq = Quantize(x, mat.cols, mat.alpha, qa)
    Kernels.Local16(mat.s, mat.cols, qa, y, 1, mat.rows, Check)
    local off, scale, b = 128 * sumq, mat.scale, mat.b
    if b then
        for j = 1, mat.rows do
            y[j] = (y[j] - off) * scale + b[j]
        end
    else
        for j = 1, mat.rows do
            y[j] = (y[j] - off) * scale
        end
    end
    model.macs = model.macs + mat.rows * mat.cols
    return y
end

-- The same for four inputs at once: each extracted weight byte serves the four.
local function Linear4(model, mat, xa, xb, xc, xd, ya, yb, yc, yd)
    local qa = model.qa
    local sa = Quantize(xa, mat.cols, mat.alpha, qa[1])
    local sb = Quantize(xb, mat.cols, mat.alpha, qa[2])
    local sc = Quantize(xc, mat.cols, mat.alpha, qa[3])
    local sd = Quantize(xd, mat.cols, mat.alpha, qa[4])
    Kernels.Batch4(mat.s, mat.cols, qa[1], qa[2], qa[3], qa[4], ya, yb, yc, yd, 1, mat.rows, Check)
    local scale, b = mat.scale, mat.b
    local oa, ob, oc, od = 128 * sa, 128 * sb, 128 * sc, 128 * sd
    for j = 1, mat.rows do
        local bj = b and b[j]
        if bj then
            ya[j] = (ya[j] - oa) * scale + bj
            yb[j] = (yb[j] - ob) * scale + bj
            yc[j] = (yc[j] - oc) * scale + bj
            yd[j] = (yd[j] - od) * scale + bj
        else
            ya[j] = (ya[j] - oa) * scale
            yb[j] = (yb[j] - ob) * scale
            yc[j] = (yc[j] - oc) * scale
            yd[j] = (yd[j] - od) * scale
        end
    end
    model.macs = model.macs + 4 * mat.rows * mat.cols
end

local function LinearMany(model, mat, X, Y, n)
    local t = 1
    while t + 3 <= n do
        Linear4(model, mat, X[t], X[t + 1], X[t + 2], X[t + 3], Y[t], Y[t + 1], Y[t + 2], Y[t + 3])
        t = t + 4
    end
    while t <= n do
        Linear(model, mat, X[t], Y[t])
        t = t + 1
    end
end

local function Vector(n)
    local v = table.create and table.create(n) or {}
    for i = 1, n do
        v[i] = 0
    end
    return v
end

local function Vectors(count, n)
    local list = {}
    for t = 1, count do
        list[t] = Vector(n)
    end
    return list
end

-- Sinusoidal position embedding: sin half then cos half, as Marian.
local function PositionEmbedding(model, p)
    local pe = model.pe[p]
    if pe then
        return pe
    end
    local D = model.cfg.D
    local n = D / 2
    local inc = log(10000.0) / (n - 1)
    pe = {}
    for i = 0, n - 1 do
        local a = p * exp(-i * inc)
        pe[i + 1] = sin(a)
        pe[n + i + 1] = cos(a)
    end
    model.pe[p] = pe
    return pe
end

-- x = sqrt(D) * E[token] + pe(position); token nil means the all-zero first decoder input.
local function Embed(model, tok, p, x)
    local cfg = model.cfg
    local D, inv, sq = cfg.D, model.embInv, model.sqrtD
    local pe = PositionEmbedding(model, p)
    if tok == nil then
        for d = 1, D do
            x[d] = 0.0 * sq + pe[d]
        end
        return x
    end
    local rpb = cfg.rowsPerBlock
    local block = model.E[floor(tok / rpb) + 1]
    local base = (tok % rpb) * D
    for d = 1, D do
        x[d] = ((byte(block, base + d) - 128) * inv) * sq + pe[d]
    end
    return x
end

local function LayerNorm(x, g, b, D)
    local m = 0
    for i = 1, D do
        m = m + x[i]
    end
    m = m / D
    local var = 0
    for i = 1, D do
        var = var + (x[i] - m) ^ 2
    end
    var = var / D
    local sd = sqrt(var + 1e-6)
    for i = 1, D do
        x[i] = g[i] * ((x[i] - m) / sd) + b[i]
    end
end

local function AddInto(x, a, D)
    for i = 1, D do
        x[i] = a[i] + x[i]
    end
end

local function Relu(v, n)
    for i = 1, n do
        if not (v[i] > 0) then
            v[i] = 0.0
        end
    end
end

-- Multi-head attention of one query over n keys/values (heads are contiguous slices).
local function Attend(model, q, K, V, n, out, z)
    local D, H = model.cfg.D, model.cfg.H
    local dk = D / H
    local scale = 1.0 / sqrt(dk)
    for h = 0, H - 1 do
        local s0 = h * dk
        local best = -math.huge
        for j = 1, n do
            local k, s = K[j], 0
            for d = s0 + 1, s0 + dk do
                s = s + q[d] * k[d]
            end
            s = scale * s
            z[j] = s
            if s > best then
                best = s
            end
        end
        local total = 0
        for j = 1, n do
            local e = exp(z[j] - best)
            z[j] = e
            total = total + e
        end
        for j = 1, n do
            z[j] = z[j] / total
        end
        for d = s0 + 1, s0 + dk do
            local acc = 0
            for j = 1, n do
                acc = acc + z[j] * V[j][d]
            end
            out[d] = acc
        end
    end
    Check()
end

local function Encode(model, ids)
    local cfg, F = model.cfg, model.floats
    local D, FF, n = cfg.D, cfg.FF, #ids
    local X = Vectors(n, D)
    for i = 1, n do
        Embed(model, ids[i], i - 1, X[i])
    end
    local Q, K, V, A, O, Hd = Vectors(n, D), Vectors(n, D), Vectors(n, D), Vectors(n, D), Vectors(n, D), Vectors(n, FF)
    local z = Vector(n)
    for l = 1, cfg.enc do
        local p = "encoder_l" .. l
        local mats = model.mats
        LinearMany(model, mats[p .. "_self_Wq"], X, Q, n)
        LinearMany(model, mats[p .. "_self_Wk"], X, K, n)
        LinearMany(model, mats[p .. "_self_Wv"], X, V, n)
        for i = 1, n do
            Attend(model, Q[i], K, V, n, A[i], z)
        end
        LinearMany(model, mats[p .. "_self_Wo"], A, O, n)
        local g, b = F[p .. "_self_Wo_ln_scale"], F[p .. "_self_Wo_ln_bias"]
        for i = 1, n do
            AddInto(X[i], O[i], D)
            LayerNorm(X[i], g, b, D)
        end
        LinearMany(model, mats[p .. "_ffn_W1"], X, Hd, n)
        for i = 1, n do
            Relu(Hd[i], FF)
        end
        LinearMany(model, mats[p .. "_ffn_W2"], Hd, O, n)
        g, b = F[p .. "_ffn_ffn_ln_scale"], F[p .. "_ffn_ffn_ln_bias"]
        for i = 1, n do
            AddInto(X[i], O[i], D)
            LayerNorm(X[i], g, b, D)
        end
    end
    return X
end

-- Candidate output ids: the first firstNum ids, the source ids, their shortlist entries,
-- padded to a multiple of 8 with the next unused ids, sorted.
local function Shortlist(model, ids)
    local cfg = model.cfg
    local V, offs, lst = cfg.V, model.lexOffsets, model.lexIds
    local sel, count = {}, 0
    local function Add(id)
        if not sel[id] then
            sel[id] = true
            count = count + 1
        end
    end
    for i = 0, math.min(cfg.firstNum, V) - 1 do
        Add(i)
    end
    local seen = {}
    for _, s in ipairs(ids) do
        if not seen[s] then
            seen[s] = true
            Add(s)
            local p = s * 4
            local a1, a2, a3, a4, b1, b2, b3, b4 = byte(offs, p + 1, p + 8)
            local from = a1 + a2 * 256 + a3 * 65536 + a4 * 16777216
            local to = b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
            for k = from, to - 1 do
                local lo, hi = byte(lst, 2 * k + 1, 2 * k + 2)
                Add(lo + hi * 256)
            end
        end
    end
    local i = cfg.firstNum
    while count % 8 ~= 0 and i < V do
        Add(i)
        i = i + 1
    end
    local list = {}
    for id in pairs(sel) do
        list[#list + 1] = id
    end
    table.sort(list)
    return list
end

-- Greedy decoding with the SSRU decoder. Returns output ids (without the end mark).
local function Decode(model, enc, nSrc, shortlist)
    local cfg, F, mats = model.cfg, model.floats, model.mats
    local D, FF, n = cfg.D, cfg.FF, #enc
    local KV, cells = {}, {}
    for l = 1, cfg.dec do
        local p = "decoder_l" .. l .. "_context"
        local Kc, Vc = Vectors(n, D), Vectors(n, D)
        LinearMany(model, mats[p .. "_Wk"], enc, Kc, n)
        LinearMany(model, mats[p .. "_Wv"], enc, Vc, n)
        KV[l] = { Kc, Vc }
        cells[l] = Vector(D)
    end
    local q, xw, f, h, a, o, hid = Vector(D), Vector(D), Vector(D), Vector(D), Vector(D), Vector(D), Vector(FF)
    local z, qa = Vector(n), model.qa[1]
    local outB, outScale, outAlpha = model.outB, model.outScale, cfg.outAlpha
    local rpb = cfg.rowsPerBlock
    local out, prev = {}, nil
    for t = 0, MAX_LENGTH_FACTOR * nSrc + 1 do
        Embed(model, prev, t, q)
        for l = 1, cfg.dec do
            local p = "decoder_l" .. l
            Linear(model, mats[p .. "_rnn_W"], q, xw)
            Linear(model, mats[p .. "_rnn_Wf"], q, f)
            local c = cells[l]
            for i = 1, D do
                local s = 1.0 / (1.0 + exp(-f[i]))
                c[i] = s * c[i] + (1.0 - s) * xw[i]
                h[i] = c[i] > 0 and c[i] or 0.0
            end
            AddInto(q, h, D)
            LayerNorm(q, F[p .. "_rnn_ffn_ln_scale"], F[p .. "_rnn_ffn_ln_bias"], D)
            Linear(model, mats[p .. "_context_Wq"], q, a)
            Attend(model, a, KV[l][1], KV[l][2], n, h, z)
            Linear(model, mats[p .. "_context_Wo"], h, o)
            AddInto(q, o, D)
            LayerNorm(q, F[p .. "_context_Wo_ln_scale"], F[p .. "_context_Wo_ln_bias"], D)
            Linear(model, mats[p .. "_ffn_W1"], q, hid)
            Relu(hid, FF)
            Linear(model, mats[p .. "_ffn_W2"], hid, o)
            AddInto(q, o, D)
            LayerNorm(q, F[p .. "_ffn_ffn_ln_scale"], F[p .. "_ffn_ffn_ln_bias"], D)
        end
        -- Output layer over the shortlist rows of the tied embedding, greedy pick, <unk> skipped.
        local sumq = Quantize(q, D, outAlpha, qa)
        local off = 128 * sumq
        local best, bestId = -math.huge, nil
        for r = 1, #shortlist do
            local id = shortlist[r]
            if id ~= cfg.unk then
                local block, base = model.E[floor(id / rpb) + 1], (id % rpb) * D
                local acc = 0
                for i = 1, D, 16 do
                    local pos = base + i
                    local w1, w2, w3, w4, w5, w6, w7, w8, w9, w10, w11, w12, w13, w14, w15, w16 = byte(block, pos, pos + 15)
                    acc = acc + w1 * qa[i] + w2 * qa[i + 1] + w3 * qa[i + 2] + w4 * qa[i + 3]
                        + w5 * qa[i + 4] + w6 * qa[i + 5] + w7 * qa[i + 6] + w8 * qa[i + 7]
                        + w9 * qa[i + 8] + w10 * qa[i + 9] + w11 * qa[i + 10] + w12 * qa[i + 11]
                        + w13 * qa[i + 12] + w14 * qa[i + 13] + w15 * qa[i + 14] + w16 * qa[i + 15]
                end
                local s = (acc - off) * outScale + outB[id + 1]
                if s > best then
                    best, bestId = s, id
                end
            end
            if r % 64 == 0 then
                Check()
            end
        end
        model.macs = model.macs + #shortlist * D
        if bestId == cfg.eos then
            break
        end
        out[#out + 1] = bestId
        prev = bestId
        if #out >= MAX_LENGTH_FACTOR * nSrc then
            break
        end
    end
    return out
end

-- Translates text with one ready model. Returns translation, source ids, output ids, shortlist size.
function Engine.TranslateWith(model, text)
    model.macs = 0
    local tok = model.tok
    local ids = ns.Tokenizer.Encode(tok, text)
    while #ids >= MAX_SOURCE_PIECES do
        ids[#ids] = nil
    end
    ids[#ids + 1] = model.cfg.eos
    local shortlist = Shortlist(model, ids)
    Check()
    local enc = Encode(model, ids)
    local outIds = Decode(model, enc, #ids, shortlist)
    return ns.Tokenizer.Decode(tok, outIds), ids, outIds, #shortlist
end

-- Translates through a chain of pairs (for example de-en then en-fr). Job code only.
function Engine.Translate(pairs, text)
    local macs = 0
    for _, pair in ipairs(pairs) do
        local model = Engine.Load(pair)
        text = Engine.TranslateWith(model, text)
        macs = macs + model.macs
    end
    return text, macs
end
