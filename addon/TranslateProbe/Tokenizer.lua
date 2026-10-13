-- SentencePiece unigram tokenizer for the model packs, without the library. It mirrors
-- tools/marian_ref.py (itself following SentencePiece's normalizer and EncodeOptimized):
-- normalize, Viterbi over piece scores, merge or byte-split unknown runs, decode back to text.
-- Normalization uses the pack's single-character map; longer keys and CJK compatibility forms
-- go through the client's NFKC (C_Intl.Normalize) first when it exists.
local _, ns = ...

local Tokenizer = {}
ns.Tokenizer = Tokenizer

local byte, sub, char = string.byte, string.sub, string.char
local concat = table.concat

local NORMAL, UNKNOWN, CONTROL, USER_DEFINED, UNUSED, BYTE = 1, 2, 3, 4, 5, 6
local SPACE_MARK = "\226\150\129"        -- U+2581, SentencePiece's visible space
local REPLACEMENT = "\239\191\189"       -- U+FFFD
local UNK_SURFACE = " \226\129\135 "     -- " U+2047 ", SentencePiece's default unknown surface
local ONE_CHAR_LEN = { [0] = 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 3, 4 }

-- Length of the valid UTF-8 character at s[i], or 0 when malformed.
local function Utf8Len(s, i)
    local c = byte(s, i)
    if c < 0x80 then
        return 1
    end
    local n
    if c >= 0xC2 and c <= 0xDF then
        n = 2
    elseif c >= 0xE0 and c <= 0xEF then
        n = 3
    elseif c >= 0xF0 and c <= 0xF4 then
        n = 4
    else
        return 0
    end
    if i + n - 1 > #s then
        return 0
    end
    local cp = c % (2 ^ (7 - n))
    for k = 1, n - 1 do
        local cc = byte(s, i + k)
        if cc < 0x80 or cc > 0xBF then
            return 0
        end
        cp = cp * 64 + (cc - 0x80)
    end
    if n == 3 and (cp < 0x800 or (cp >= 0xD800 and cp <= 0xDFFF)) then
        return 0
    end
    if n == 4 and (cp < 0x10000 or cp > 0x10FFFF) then
        return 0
    end
    return n
end
Tokenizer.Utf8Len = Utf8Len

-- Builds the lookup tables of a pack once: piece -> id (types NORMAL, USER_DEFINED, UNUSED),
-- byte -> id for BYTE pieces, longest piece length.
function Tokenizer.Prepare(pack, check)
    local vocab, byteIds, maxPiece = {}, {}, 1
    local pieces, types = pack.pieces, pack.types
    for i = 1, #pieces do
        local p, t = pieces[i], types[i]
        if t == NORMAL or t == USER_DEFINED or t == UNUSED then
            vocab[p] = i - 1
            if #p > maxPiece then
                maxPiece = #p
            end
        elseif t == BYTE then
            byteIds[tonumber(sub(p, 4, 5), 16)] = i - 1
        end
        if check and i % 2000 == 0 then
            check()
        end
    end
    return { vocab = vocab, byteIds = byteIds, maxPiece = maxPiece, pack = pack }
end

local function NormPrefix(map, s, i)
    local n = Utf8Len(s, i)
    if n == 0 then
        return REPLACEMENT, 1
    end
    local ch = sub(s, i, i + n - 1)
    return map[ch] or ch, n
end

-- SentencePiece normalization: charmap, whitespace rules, U+2581 for spaces, dummy prefix.
function Tokenizer.Normalize(tok, text)
    local cfg, map = tok.pack.config, tok.pack.charmap
    if C_Intl and C_Intl.Normalize and Enum and Enum.NormalizationForm then
        local ok, out = pcall(C_Intl.Normalize, text, Enum.NormalizationForm.Nfkc)
        if ok and type(out) == "string" then
            text = out
        end
    end
    local size, i = #text, 1
    local rew = cfg.removeExtraWhitespaces
    local sp = cfg.escapeWhitespaces and SPACE_MARK or " "
    if rew then
        while i <= size do
            local r, n = NormPrefix(map, text, i)
            if r ~= " " then
                break
            end
            i = i + n
        end
    end
    if i > size then
        return ""
    end
    local out = {}
    if cfg.addDummyPrefix then
        out[1] = sp
    end
    local prevSpace = rew
    while i <= size do
        local r, n = NormPrefix(map, text, i)
        if prevSpace then
            r = r:gsub("^ +", "")
        end
        if r ~= "" then
            out[#out + 1] = (r:gsub(" ", sp))
            prevSpace = sub(r, -1) == " "
        end
        i = i + n
        if not rew then
            prevSpace = false
        end
    end
    local s = concat(out)
    if rew then
        while #s >= #sp and sub(s, -#sp) == sp do
            s = sub(s, 1, -#sp - 1)
        end
    end
    return s
end

-- Piece ids of text (without the end-of-sentence id).
function Tokenizer.Encode(tok, text)
    local pack, vocab = tok.pack, tok.vocab
    local cfg, scores, types = pack.config, pack.scores, pack.types
    local s = Tokenizer.Normalize(tok, text)
    local size = #s
    local unkScore = cfg.minScore - 10.0
    local userScoreBase = cfg.maxScore
    local bestId, bestScore, bestStart = {}, {}, {}
    bestScore[0], bestStart[0] = 0.0, 0
    local st = 0
    while st < size do
        local base = bestScore[st]
        local mblen = math.min(ONE_CHAR_LEN[math.floor(byte(s, st + 1) / 16)], size - st)
        local single = false
        for e = st + 1, math.min(size, st + tok.maxPiece) do
            local p = sub(s, st + 1, e)
            local pid = vocab[p]
            if pid then
                local ty = types[pid + 1]
                if ty ~= UNUSED then
                    local sc = (ty == USER_DEFINED) and (#p * userScoreBase - 0.1) or scores[pid + 1]
                    if bestStart[e] == nil or sc + base > bestScore[e] then
                        bestId[e], bestScore[e], bestStart[e] = pid, sc + base, st
                    end
                    if #p == mblen then
                        single = true
                    end
                end
            end
        end
        if not single then
            local e = st + mblen
            if bestStart[e] == nil or unkScore + base > bestScore[e] then
                bestId[e], bestScore[e], bestStart[e] = cfg.unk, unkScore + base, st
            end
        end
        st = st + mblen
    end
    local path, e = {}, size
    while e > 0 do
        path[#path + 1] = e
        e = bestStart[e]
    end
    local ids, prevUnk = {}, false
    for k = #path, 1, -1 do
        local stop = path[k]
        local start = bestStart[stop]
        local pid = bestId[stop]
        local isUnk = pid == cfg.unk
        if isUnk and cfg.byteFallback then
            for b = start + 1, stop do
                ids[#ids + 1] = tok.byteIds[byte(s, b)]
            end
        elseif not (isUnk and prevUnk) then
            ids[#ids + 1] = pid
        end
        prevUnk = isUnk
    end
    return ids
end

local function FlushBytes(out, pending)
    if #pending == 0 then
        return
    end
    local s = char(unpack(pending))
    local i = 1
    while i <= #s do
        local n = Utf8Len(s, i)
        out[#out + 1] = n > 0 and sub(s, i, i + n - 1) or REPLACEMENT
        i = i + (n > 0 and n or 1)
    end
    for k = #pending, 1, -1 do
        pending[k] = nil
    end
end

-- Text of piece ids (control pieces skipped, byte pieces joined as UTF-8).
function Tokenizer.Decode(tok, ids)
    local pack = tok.pack
    local pieces, types, cfg = pack.pieces, pack.types, pack.config
    local out, pending, first = {}, {}, true
    for _, id in ipairs(ids) do
        local p, ty = pieces[id + 1], types[id + 1]
        if ty == BYTE then
            pending[#pending + 1] = tonumber(sub(p, 4, 5), 16)
            first = false
        elseif ty ~= CONTROL then
            FlushBytes(out, pending)
            if ty == UNKNOWN then
                out[#out + 1] = UNK_SURFACE
            else
                if first and cfg.addDummyPrefix and sub(p, 1, #SPACE_MARK) == SPACE_MARK then
                    p = sub(p, #SPACE_MARK + 1)
                end
                out[#out + 1] = (p:gsub(SPACE_MARK, " "))
            end
            first = false
        end
    end
    FlushBytes(out, pending)
    return concat(out)
end
