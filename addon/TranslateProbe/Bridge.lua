-- Pixel bridge, addon side (protocol v2; the normative spec is docs\BRIDGE.md).
--
-- The addon cannot write files or sockets, so it sends requests to Translate Companion (the
-- Helper) by drawing a strip of coloured 2x2 physical-pixel blocks at the top-left corner of the
-- screen, which the helper captures and decodes. Nothing ever comes back through the strip: each
-- message stays on screen long enough to be captured several times, and the helper drops repeats.
-- The strip is only shown while a message is queued (about 0.5 s per click) and hidden otherwise.
--
-- Strip: block k at x = 2 * floor(k / 2), y = 2 * (k % 2) physical px from the client's top-left,
-- 4 px tall. Blocks 0-7 are calibration greys (level index 0,3,3,0,1,2,2,1 of 0/104/146/255),
-- then the frame bytes, most significant bit first: dense 6 bits per block, robust 3 bits,
-- mono 2 bits (grey only, for the colourblind filter). The last symbol is zero padded.
-- Frame: magic, version 2, seq, flags 0, session u16, body length u16, records (STATUS, then
-- CHUNKs), CRC-32 of everything before it.
--
-- The return path is pixel-anchored: each reserved chat row starts with a tag (8 inline colour
-- blocks: a magenta/green sync pair, then 6 base-6 digits of slot, row, session bit and CRC-5),
-- followed by a dark plate the helper paints the translation on.
--
-- Cost: nothing runs while the strip is hidden. While shown, a frame is composed at most every
-- HOLD_MS and only blocks whose colour changed get a SetColorTexture call.
local _, ns = ...
ns = ns or {}

local floor, min, max = math.floor, math.min, math.max
local byte, char, find, format = string.byte, string.char, string.find, string.format
local concat, sort = table.concat, table.sort
local band, bxor, bnot, rshift = bit.band, bit.bxor, bit.bnot, bit.rshift

local Bridge = {}
ns.Bridge = Bridge

---------------------------------------------------------------------------- constants (normative)
local VERSION = 2
local LEVELS = { 0, 104, 146, 255 }            -- 8-bit value of level index 0..3 (array index + 1)
local MODE_DENSE, MODE_ROBUST, MODE_MONO = 0, 1, 2
local MAGIC = { [0] = 0xD5, [1] = 0x2A, [2] = 0x5A }
local BITS = { [0] = 6, [1] = 3, [2] = 2 }
local MODE_NAMES = { [0] = "dense", [1] = "robust", [2] = "mono" }
local MODE_BY_NAME = { dense = MODE_DENSE, robust = MODE_ROBUST, mono = MODE_MONO }
local CALIB = { 0, 3, 3, 0, 1, 2, 2, 1 }       -- level index of calibration blocks 0..7
local NCAL = 8
local HDR_LEN, CRC_LEN, REC_HDR = 8, 4, 3
local REC_STATUS, REC_CHUNK = 0x01, 0x03       -- 0x02 (the v1 LAYOUT) is gone
local STATUS_DATA = 6                          -- physW u16, physH u16, addonBuild u16
local STATUS_LEN = REC_HDR + STATUS_DATA
local CHUNK_FIXED = 10                         -- msgId u16, idx u8, count u8, total u16, msgCrc u32
local MSG_TRANSLATE, MSG_CANCEL, MSG_CLEAR, MSG_TEST, MSG_VIEW = 0x10, 0x11, 0x12, 0x13, 0x14
local MSG_COLOURS = 0x15                       -- the link colours of the next TRANSLATE (addon build 5)
local MAX_COLOURS = 255                        -- COLOURS entries (span index u8, strictly increasing)
local MIN_COLS = { [0] = 192, [1] = 384, [2] = 576 }   -- floor for the maxCols option: 248-252 B of chunk data
local MIN_SCREEN_COLS = 200                    -- floor for a degenerate screen size: 64 B of chunk data in mono
local MAX_TEXT, MAX_TARGET, MAX_PAIRS, MAX_TERM, MAX_MEANING, MAX_PAIR_BYTES = 1024, 8, 8, 48, 120, 512
local MAX_VIEWS = 4
-- TRANSLATE flags: bit0 noCache, bit1 plate, bit2 reply (the conversation window's reply card:
-- row 0 is the Helper's status, rows 1.. the text), bit3 copy (the Helper may put the final
-- translation in the clipboard; only with reply). Bits 4-7 are 0.
local FLAG_NOCACHE, FLAG_PLATE, FLAG_REPLY, FLAG_COPY = 1, 2, 4, 8
local FLAGS_MASK = 15

-- Timing (ms)
local HOLD_MS, ROT_HOLD_MS, EXPOSE_MS, CREDIT_CAP_MS = 50, 70, 400, 100

-- Row tags and plate
local TEX_TAG = "|TInterface\\Buttons\\WHITE8X8:%d:%d:0:0:8:8:0:8:0:8:%d:%d:%d|t"   -- height, width, r, g, b
local TAG_SYNC = { { 255, 0, 255 }, { 0, 255, 0 } }                                 -- art-ok: protocol colour
local TAG_DIGIT = { [0] = { 0, 0, 0 }, { 255, 0, 0 }, { 0, 0, 255 }, { 255, 255, 0 }, { 0, 255, 255 },
    { 255, 255, 255 } }                        -- art-ok: protocol colour (black, red, blue, yellow, cyan, white)
local PLATE_RGB = { 32, 40, 64 }               -- art-ok: protocol colour
-- Opaque interior of the conversation window's bubbles and reply card (docs\BRIDGE.md section
-- 8c): it fails the plate prediction and the hue rule, so the Helper never takes it for a plate.
local BUBBLE_RGB = { 36, 30, 24 }              -- art-ok: protocol colour

Bridge.VERSION = VERSION
Bridge.TEST_SLOT = 127
Bridge.PLATE_RGB = PLATE_RGB
Bridge.BUBBLE_RGB = BUBBLE_RGB
Bridge.LEVELS = LEVELS
Bridge.MODES = MODE_NAMES
Bridge.FLAG_NOCACHE, Bridge.FLAG_PLATE, Bridge.FLAG_REPLY, Bridge.FLAG_COPY = FLAG_NOCACHE, FLAG_PLATE, FLAG_REPLY,
    FLAG_COPY
Bridge.FLAGS_MASK = FLAGS_MASK

---------------------------------------------------------------------------- bytes
local CRC_T = {}
for i = 0, 255 do
    local c = i
    for _ = 1, 8 do
        if band(c, 1) == 1 then
            c = bxor(rshift(c, 1), 0xEDB88320)
        else
            c = rshift(c, 1)
        end
    end
    CRC_T[i] = c
end

-- CRC-32 (zlib) of t[i0..i1] (bytes); returned as a signed 32-bit number (bit library convention).
local function Crc32(t, i0, i1)
    local c = -1
    for i = i0, i1 do
        c = bxor(rshift(c, 8), CRC_T[band(bxor(c, t[i]), 255)])
    end
    return bnot(c)
end
Bridge.Crc32 = Crc32

local function CrcString(s)
    local c = -1
    for i = 1, #s do
        c = bxor(rshift(c, 8), CRC_T[band(bxor(c, byte(s, i)), 255)])
    end
    return bnot(c)
end

local function Clamp(v, lo, hi)
    v = floor(tonumber(v) or 0)
    if v < lo then
        return lo
    elseif v > hi then
        return hi
    end
    return v
end

local function U8(v)
    return char(Clamp(v, 0, 255))
end

local function U16(v)
    v = Clamp(v, 0, 65535)
    return char(floor(v / 256), v % 256)
end

local function U32(c)
    return char(band(rshift(c, 24), 255), band(rshift(c, 16), 255), band(rshift(c, 8), 255), band(c, 255))
end

-- Cuts s to at most n bytes without splitting a UTF-8 sequence.
local function Utf8Cut(s, n)
    if #s <= n then
        return s
    end
    local i = n
    while i > 0 do
        local b = byte(s, i + 1)
        if b < 128 or b >= 192 then
            break
        end
        i = i - 1
    end
    return s:sub(1, i)
end
Bridge.Utf8Cut = Utf8Cut

local function LastIndex(s, ch)
    local last, i = nil, 0
    while true do
        i = find(s, ch, i + 1, true)
        if not i then
            return last
        end
        last = i
    end
end

-- Utf8Cut that never leaves a 0x02 link mark open: when the cut happens and the last 0x02 is
-- after the last 0x03, the text is cut before that 0x02.
local function CutMarked(s, n)
    local cut = Utf8Cut(s, n)
    if #cut < #s then
        local open, close = LastIndex(cut, "\2"), LastIndex(cut, "\3")
        if open and (not close or open > close) then
            cut = cut:sub(1, open - 1)
        end
    end
    return cut
end

local function Str8(s, n)
    s = Utf8Cut(type(s) == "string" and s or "", n)
    return char(#s) .. s
end

-- One marked span of the addon's text (docs\BRIDGE.md, "Marked spans"): 0x02, then 0x04 and the
-- colour as 6 uppercase hex digits when rgb is 6 hex digits (addon build 5: the colour the client
-- draws a link name in), the name without bytes 0x01-0x04, then 0x03. "" for an empty name. The
-- message builders take the colours out (SplitColours): the TRANSLATE gets 0x02 name 0x03, the
-- COLOURS message the colour.
function Bridge.MarkSpan(name, rgb)
    if type(name) ~= "string" then
        return ""
    end
    name = name:gsub("[\1-\4]", "")
    if name == "" then
        return ""
    end
    local color = ""
    if type(rgb) == "string" and find(rgb, "^%x%x%x%x%x%x$") then
        color = "\4" .. rgb:upper()
    end
    return "\2" .. color .. name .. "\3"
end

---------------------------------------------------------------------------- messages
-- pairs: array of { term, meaning } (the glossary pairs matched in this line). Pairs are cut to
-- 48 and 120 bytes; at most 8 are sent and they stop before 512 bytes (2 + #term + #meaning each).
-- Integers are written as given (clamped to their field width): the helper refuses values out of
-- range rather than the addon remapping them. Flags are masked to bits 0-3 and the copy bit is
-- dropped unless the reply bit is set (the Helper never copies an incoming message); a reply
-- without the plate bit or with fewer than 2 rows is written as given and refused by the Helper.
local function TranslateFlags(flags)
    flags = band(Clamp(flags, 0, 255), FLAGS_MASK)
    if band(flags, FLAG_COPY) ~= 0 and band(flags, FLAG_REPLY) == 0 then
        flags = flags - FLAG_COPY
    end
    return flags
end

-- The wire form of the addon's marked text and its colours (docs\BRIDGE.md section 3b). Each
-- colour prefix MarkSpan writes (0x04 and 6 hex digits right after a 0x02) is taken out and kept as
-- { span index from 0, "RRGGBB" }, counting every 0x02; a 0x04 not followed by 6 hex digits is
-- dropped alone and any other 0x04 is deleted. Then the 1024 B cut (CutMarked), and only the
-- colours of the spans it kept, index <= 255, at most 255. A TRANSLATE never carries a 0x04: a
-- Translate Companion of build 4 would paint the digits as text, while it skips COLOURS.
local function SplitColours(text)
    text = type(text) == "string" and text or ""
    local colours = {}
    if find(text, "\4", 1, true) then
        local span = -1
        text = text:gsub("\2(\4?)(%x?%x?%x?%x?%x?%x?)", function(prefix, hex)
            span = span + 1
            if prefix ~= "" and #hex == 6 then
                colours[#colours + 1] = { span, hex:upper() }
                return "\2"
            end
            return "\2" .. hex
        end):gsub("\4", "")
    end
    text = CutMarked(text, MAX_TEXT)
    if #colours > 0 then
        local kept = select(2, text:gsub("\2", "\2"))
        local out = {}
        for i = 1, #colours do
            local c = colours[i]
            if c[1] < kept and c[1] <= 255 and #out < MAX_COLOURS then
                out[#out + 1] = c
            end
        end
        colours = out
    end
    return text, colours
end

-- COLOURS for a TRANSLATE text (the same text as given to TranslateMessage), sent right before
-- that TRANSLATE: reqId u16, n u8, n x { span u8, r u8, g u8, b u8 }. nil when no span has a colour.
function Bridge.ColoursMessage(reqId, text)
    local _, colours = SplitColours(text)
    local n = #colours
    if n == 0 then
        return nil
    end
    local parts = { char(MSG_COLOURS), U16(reqId), char(n) }
    for i = 1, n do
        local v = tonumber(colours[i][2], 16)
        parts[i + 3] = char(colours[i][1], floor(v / 65536), floor(v / 256) % 256, v % 256)
    end
    return concat(parts)
end

function Bridge.TranslateMessage(reqId, slot, rows, flags, target, text, pairs)
    text = SplitColours(text)
    local parts = { char(MSG_TRANSLATE), U16(reqId), U8(slot), U8(rows), char(TranslateFlags(flags)),
        Str8(target, MAX_TARGET), U16(#text), text }
    local out, n, used = {}, 0, 0
    if type(pairs) == "table" then
        for i = 1, #pairs do
            if n >= MAX_PAIRS then
                break
            end
            local p = pairs[i]
            if type(p) == "table" and type(p[1]) == "string" and type(p[2]) == "string" then
                local term, meaning = Utf8Cut(p[1], MAX_TERM), Utf8Cut(p[2], MAX_MEANING)
                local cost = 2 + #term + #meaning
                if used + cost > MAX_PAIR_BYTES then
                    break
                end
                n = n + 1
                out[n] = char(#term) .. term .. char(#meaning) .. meaning
                used = used + cost
            end
        end
    end
    parts[#parts + 1] = char(n)
    parts[#parts + 1] = concat(out)
    return concat(parts)
end

function Bridge.CancelMessage(reqId, slot)
    return char(MSG_CANCEL) .. U16(reqId) .. U8(slot)
end

function Bridge.ClearMessage()
    return char(MSG_CLEAR)
end

function Bridge.TestMessage(nonce)
    return char(MSG_TEST) .. U16(nonce)
end

-- views: array of { id, x, y, w, h, fontPx, rowPx } in physical px from the client origin
-- (Reserve.View); at most 4 are sent.
function Bridge.ViewMessage(views)
    local n = min(type(views) == "table" and #views or 0, MAX_VIEWS)
    local parts = { char(MSG_VIEW), char(n) }
    for i = 1, n do
        local v = views[i]
        parts[i + 2] = U8(v.id) .. U16(v.x) .. U16(v.y) .. U16(v.w) .. U16(v.h) .. U8(v.fontPx) .. U8(v.rowPx)
    end
    return concat(parts)
end

---------------------------------------------------------------------------- row tags
-- CRC-5 of the 10-bit tag payload: polynomial 0x05, init 0x1F, most significant bit first.
function Bridge.Crc5(payload10)
    local p = floor(tonumber(payload10) or 0) % 1024
    local c = 0x1F
    for i = 9, 0, -1 do
        local b = floor(p / 2 ^ i) % 2
        local top = floor(c / 16) % 2
        c = (c * 2) % 32
        if b ~= top then
            c = bxor(c, 0x05)
        end
    end
    return c
end

-- The 6 base-6 data digits (most significant first) of v = payload10 << 5 | crc5, with
-- payload10 = slot << 3 | row << 1 | sessBit (slot 0..127, row 0..3, sessBit 0..1).
function Bridge.TagDigits(slot, row, sessBit)
    local payload = floor(slot) % 128 * 8 + floor(row) % 4 * 2 + floor(sessBit) % 2
    local v = payload * 32 + Bridge.Crc5(payload)
    local d = {}
    for i = 6, 1, -1 do
        d[i] = v % 6
        v = floor(v / 6)
    end
    return d
end

-- The 8 block colours of a tag: sync pair, then the digits. Each entry is a new { r, g, b }.
function Bridge.TagColors(slot, row, sessBit)
    local d = Bridge.TagDigits(slot, row, sessBit)
    local c = {}
    for i = 1, 2 do
        local s = TAG_SYNC[i]
        c[i] = { s[1], s[2], s[3] }
    end
    for i = 1, 6 do
        local s = TAG_DIGIT[d[i]]
        c[i + 2] = { s[1], s[2], s[3] }
    end
    return c
end

-- Inline markup of a tag: 8 solid textures of w x h chat units, explicit vertex colours.
function Bridge.TagMarkup(slot, row, sessBit, w, h)
    local colors = Bridge.TagColors(slot, row, sessBit)
    w, h = Clamp(w, 1, 4096), Clamp(h, 1, 4096)
    local parts = {}
    for i = 1, 8 do
        local c = colors[i]
        parts[i] = format(TEX_TAG, h, w, c[1], c[2], c[3])
    end
    return concat(parts)
end

-- Inline markup of the plate drawn right after a tag.
function Bridge.PlateMarkup(w, h)
    return format(TEX_TAG, Clamp(h, 1, 4096), Clamp(w, 1, 4096), PLATE_RGB[1], PLATE_RGB[2], PLATE_RGB[3])
end

-- Colours the 8 Textures of a tag drawn by a frame (the conversation window draws its tags with
-- textures, not inline markup): textures[1..8] get the sync pair then the digits of
-- (slot, row % 4, sessBit), opaque. Same colours as TagColors, without allocating.
function Bridge.ApplyTag(textures, slot, row, sessBit)
    local payload = floor(slot) % 128 * 8 + floor(row) % 4 * 2 + floor(sessBit) % 2
    local v = payload * 32 + Bridge.Crc5(payload)
    for i = 8, 3, -1 do
        local c = TAG_DIGIT[v % 6]
        textures[i]:SetColorTexture(c[1] / 255, c[2] / 255, c[3] / 255, 1)
        v = floor(v / 6)
    end
    for i = 1, 2 do
        local c = TAG_SYNC[i]
        textures[i]:SetColorTexture(c[1] / 255, c[2] / 255, c[3] / 255, 1)
    end
end

---------------------------------------------------------------------------- capacity
local function FrameCapacity(ncols, mode)
    return floor((2 * ncols - NCAL) * BITS[mode] / 8)
end

local function MaxChunkData(ncols, mode)
    return FrameCapacity(ncols, mode) - HDR_LEN - CRC_LEN - STATUS_LEN - REC_HDR - CHUNK_FIXED
end
Bridge.FrameCapacity = FrameCapacity
Bridge.MaxChunkData = MaxChunkData

---------------------------------------------------------------------------- encoder
local Encoder = {}
Encoder.__index = Encoder

-- Splits a message into chunk records (byte arrays) of at most size data bytes.
local function BuildChunks(id, msg, size)
    local count = floor((#msg + size - 1) / size)
    if count < 1 then
        count = 1
    end
    local mcrc = U32(CrcString(msg))
    local chunks = {}
    for i = 0, count - 1 do
        local data = msg:sub(i * size + 1, (i + 1) * size)
        local rec = char(REC_CHUNK) .. U16(CHUNK_FIXED + #data) .. U16(id) .. char(i, count) .. U16(#msg)
            .. mcrc .. data
        local bytes = { byte(rec, 1, REC_HDR + CHUNK_FIXED) }
        for k = REC_HDR + CHUNK_FIXED + 1, #rec do
            bytes[k] = byte(rec, k)
        end
        chunks[i + 1] = { rec = bytes, n = #rec, exp = 0, msg = id, order = 0 }
    end
    return chunks
end

-- opts: session (1..65535), physW, physH, mode (0 dense, 1 robust, 2 mono), maxCols, addonBuild,
-- and for tests holdMs, rotHoldMs, exposeMs. Times passed to Update are ms (GetTime() * 1000).
function Bridge.NewEncoder(opts)
    local e = setmetatable({}, Encoder)
    e.session = Clamp(opts.session, 1, 65535)
    e.mode = MAGIC[opts.mode] and opts.mode or MODE_DENSE
    e.addonBuild = Clamp(opts.addonBuild, 0, 65535)
    e.hold = opts.holdMs or HOLD_MS
    e.rotHold = opts.rotHoldMs or ROT_HOLD_MS
    e.expose = opts.exposeMs or EXPOSE_MS
    e.maxColsOpt = opts.maxCols
    e.msgs = {}         -- { id, msg, chunks = { { rec = bytes array, n, exp, msg = id, order } } }
    e.fresh = false     -- a message was queued since the last frame
    e.seq = 0
    e.nextMsg = 1
    e.dirty = true
    e.rotating = false
    e.lastTick = nil
    e.frameTicks = 0
    e.frameStart = nil
    e.cur, e.ncur = {}, 0
    e.flat = {}
    e.fb, e.nfb = {}, 0
    e.sym, e.nsym = {}, 0
    e:SetScreen(opts.physW, opts.physH)
    return e
end

-- Strip width for the screen and mode; pending messages are cut again when it changes.
function Encoder:Fit(force)
    -- never wider than the screen: blocks past the client's right edge are never captured
    local cols = floor(self.physW / 2)
    if self.maxColsOpt then
        cols = min(max(self.maxColsOpt, MIN_COLS[self.mode]), cols)
    end
    cols = max(cols, MIN_SCREEN_COLS)
    if force or cols ~= self.maxCols then
        local size = MaxChunkData(cols, self.mode)
        for i = 1, #self.msgs do
            local m = self.msgs[i]
            m.chunks = BuildChunks(m.id, m.msg, size)
        end
    end
    self.maxCols = cols
    self.dirty = true
end

function Encoder:SetScreen(physW, physH)
    self.physW, self.physH = Clamp(physW, 0, 65535), Clamp(physH, 0, 65535)
    self:Fit(false)
end

-- mode: 0 dense, 1 robust, 2 mono. Pending messages are cut again for the new capacity.
function Encoder:SetMode(mode)
    if not MAGIC[mode] or mode == self.mode then
        return
    end
    self.mode = mode
    self:Fit(true)
end

-- Queues a message string; returns its message id (1..65535).
function Encoder:Queue(msg)
    local id = self.nextMsg
    self.nextMsg = id % 65535 + 1
    local chunks = BuildChunks(id, msg, MaxChunkData(self.maxCols, self.mode))
    self.msgs[#self.msgs + 1] = { id = id, msg = msg, chunks = chunks }
    self.dirty = true
    self.fresh = true
    return id
end

-- True when nothing is queued: the strip is hidden.
function Encoder:IsIdle()
    return #self.msgs == 0
end

local function MessageDone(m, expose)
    local chunks = m.chunks
    for i = 1, #chunks do
        if chunks[i].exp < expose then
            return false
        end
    end
    return true
end

local function ByExposure(a, b)
    if a.exp ~= b.exp then
        return a.exp < b.exp
    end
    return a.order < b.order
end

local function ByOrder(a, b)
    return a.order < b.order
end

local function Put16(t, n, v)
    t[n + 1], t[n + 2] = floor(v / 256) % 256, v % 256
end

function Encoder:Compose()
    self.seq = (self.seq + 1) % 256
    local fb = self.fb
    local n = HDR_LEN
    fb[n + 1], fb[n + 2], fb[n + 3] = REC_STATUS, 0, STATUS_DATA
    Put16(fb, n + 3, self.physW)
    Put16(fb, n + 5, self.physH)
    Put16(fb, n + 7, self.addonBuild)
    n = n + STATUS_LEN
    local room = FrameCapacity(self.maxCols, self.mode) - CRC_LEN - n
    -- flat list of every active chunk, in message then chunk order
    local flat, nflat, total = self.flat, 0, 0
    for i = 1, #self.msgs do
        local chunks = self.msgs[i].chunks
        for j = 1, #chunks do
            nflat = nflat + 1
            flat[nflat] = chunks[j]
            chunks[j].order = nflat
            total = total + chunks[j].n
        end
    end
    for i = nflat + 1, #flat do
        flat[i] = nil
    end
    local cur, ncur = self.cur, 0
    if total <= room then
        self.rotating = false
        for i = 1, nflat do
            cur[i] = flat[i]
        end
        ncur = nflat
    else
        -- least exposed first, first fit: fresh messages go out at once, the rest rotates
        self.rotating = true
        sort(flat, ByExposure)
        local used = 0
        for i = 1, nflat do
            local c = flat[i]
            if used + c.n <= room then
                ncur = ncur + 1
                cur[ncur] = c
                used = used + c.n
            end
        end
    end
    for i = ncur + 1, #cur do
        cur[i] = nil
    end
    if self.rotating then
        sort(cur, ByOrder)    -- the frame keeps message order
    end
    self.ncur = ncur
    for i = 1, ncur do
        local rec, len = cur[i].rec, cur[i].n
        for k = 1, len do
            fb[n + k] = rec[k]
        end
        n = n + len
    end
    local bodyLen = n - HDR_LEN
    fb[1], fb[2], fb[3], fb[4] = MAGIC[self.mode], VERSION, self.seq, 0   -- flags: always 0
    Put16(fb, 4, self.session)
    Put16(fb, 6, bodyLen)
    local c = Crc32(fb, 1, n)
    fb[n + 1], fb[n + 2] = band(rshift(c, 24), 255), band(rshift(c, 16), 255)
    fb[n + 3], fb[n + 4] = band(rshift(c, 8), 255), band(c, 255)
    n = n + 4
    for i = n + 1, self.nfb do
        fb[i] = nil
    end
    self.nfb = n
    self:Pack()
end

-- Frame bytes -> symbols (most significant bit first, last symbol zero padded).
function Encoder:Pack()
    local fb, n, sym = self.fb, self.nfb, self.sym
    local nsy = 0
    if self.mode == MODE_DENSE then
        for i = 1, n, 3 do
            local b1, b2, b3 = fb[i], fb[i + 1] or 0, fb[i + 2] or 0
            sym[nsy + 1] = floor(b1 / 4)
            sym[nsy + 2] = (b1 % 4) * 16 + floor(b2 / 16)
            sym[nsy + 3] = (b2 % 16) * 4 + floor(b3 / 64)
            sym[nsy + 4] = b3 % 64
            nsy = nsy + 4
        end
        nsy = floor((n * 8 + 5) / 6)
    elseif self.mode == MODE_ROBUST then
        for i = 1, n, 3 do
            local v = (fb[i] * 256 + (fb[i + 1] or 0)) * 256 + (fb[i + 2] or 0)
            for k = 7, 0, -1 do
                nsy = nsy + 1
                sym[nsy] = floor(v / 8 ^ k) % 8
            end
        end
        nsy = floor((n * 8 + 2) / 3)
    else
        for i = 1, n do
            local b = fb[i]
            sym[nsy + 1] = floor(b / 64)
            sym[nsy + 2] = floor(b / 16) % 4
            sym[nsy + 3] = floor(b / 4) % 4
            sym[nsy + 4] = b % 4
            nsy = nsy + 4
        end
    end
    for i = nsy + 1, #sym do
        sym[i] = nil
    end
    self.nsym = nsy
end

-- Called when the strip is shown again (after a click while idle, Alt+Z or a loading screen).
function Encoder:OnShow()
    self.lastTick = nil
    self.frameTicks = 0
    if self.frameStart then
        self.frameStart = -1e18
    end
    self.dirty = true
end

-- Per game frame while shown. now: ms. Returns true when a new frame was composed (the caller
-- redraws). Screen time is credited from the second game frame after a change, at most
-- CREDIT_CAP_MS per game frame; a message leaves once each chunk has EXPOSE_MS. When the queue
-- drains nothing is composed (IsIdle() turns true and the strip hides), and the next queued
-- message is composed on the next call.
function Encoder:Update(now)
    local cur = self.cur
    if self.lastTick and self.frameTicks >= 1 then
        local credit = min(now - self.lastTick, CREDIT_CAP_MS)
        for i = 1, self.ncur do
            cur[i].exp = cur[i].exp + credit
        end
    end
    self.frameTicks = self.frameTicks + 1
    self.lastTick = now
    if self.frameStart then
        local held = now - self.frameStart
        -- a rotating strip advances every rotHold (> one capture period); a new message or a
        -- non-rotating change only waits for hold
        local wait = self.hold
        if self.rotating and not self.fresh then
            wait = self.rotHold
        end
        if held < wait then
            return false
        end
        local msgs, expose, done = self.msgs, self.expose, false
        for i = 1, #msgs do
            if MessageDone(msgs[i], expose) then
                done = true
                break
            end
        end
        if not (self.dirty or self.rotating or done) then
            return false
        end
        if done then
            local keep = 0
            for i = 1, #msgs do
                local m = msgs[i]
                msgs[i] = nil
                if not MessageDone(m, expose) then
                    keep = keep + 1
                    msgs[keep] = m
                end
            end
        end
    end
    if #self.msgs == 0 then
        self.frameStart = nil
        self.rotating = false
        self.ncur = 0
        return false
    end
    self:Compose()
    self.frameStart = now
    self.frameTicks = 0
    self.dirty = false
    self.fresh = false
    return true
end

-- Number of blocks to draw (calibration + data).
function Encoder:BlockCount()
    return NCAL + self.nsym
end

-- Colour key of block k (0-based): level indices packed as r * 16 + g * 4 + b.
function Encoder:BlockKey(k)
    if k < NCAL then
        return CALIB[k + 1] * 21
    end
    local s = self.sym[k - NCAL + 1]
    if self.mode == MODE_DENSE then
        return s
    elseif self.mode == MODE_ROBUST then
        return 3 * (floor(s / 4) % 2) * 16 + 3 * (floor(s / 2) % 2) * 4 + 3 * (s % 2)
    end
    return s * 21
end

---------------------------------------------------------------------------- WoW strip
-- Key (0..63) -> r, g, b in 0..1 for SetColorTexture.
local KEY_R, KEY_G, KEY_B = {}, {}, {}
for key = 0, 63 do
    KEY_R[key] = LEVELS[floor(key / 16) % 4 + 1] / 255
    KEY_G[key] = LEVELS[floor(key / 4) % 4 + 1] / 255
    KEY_B[key] = LEVELS[key % 4 + 1] / 255
end
Bridge.KEY_R, Bridge.KEY_G, Bridge.KEY_B = KEY_R, KEY_G, KEY_B

local function ScreenSize()
    local w, h
    if GetPhysicalScreenSize then
        w, h = GetPhysicalScreenSize()
    end
    w, h = tonumber(w), tonumber(h)
    if not (w and h and w > 0 and h > 0) then
        return 1920, 1080
    end
    return w, h
end

-- Creates the strip frame for an encoder, hidden while the encoder is idle. Parented to UIParent
-- so Alt+Z hides it; scaled so one unit is one physical pixel. frame:Refresh() shows it when a
-- message is queued and it is neither held (frame.held) nor behind a loading screen.
function Bridge.CreateStrip(enc)
    local f = CreateFrame("Frame", nil, UIParent)
    f:Hide()
    f:SetFrameStrata("TOOLTIP")
    f:SetFrameLevel(9000)
    f:SetIgnoreParentScale(true)
    f:SetPoint("TOPLEFT", UIParent, "TOPLEFT", 0, 0)
    f.held, f.loading, f.on = false, false, false
    local tex, shown, nshown = {}, {}, 0

    local function Rescale()
        local w, h = ScreenSize()
        f:SetScale(768 / h)
        f:SetSize(w, 4)
        enc:SetScreen(w, h)
    end

    local function Draw()
        local n = enc:BlockCount()
        for k = 0, n - 1 do
            local t = tex[k]
            if not t then
                t = f:CreateTexture(nil, "OVERLAY")
                t:SetSize(2, 2)
                t:SetPoint("TOPLEFT", f, "TOPLEFT", 2 * floor(k / 2), -2 * (k % 2))
                t:SetSnapToPixelGrid(false)
                t:SetTexelSnappingBias(0)
                tex[k] = t
            end
            local key = enc:BlockKey(k)
            if shown[k] ~= key then
                t:SetColorTexture(KEY_R[key], KEY_G[key], KEY_B[key], 1)
                shown[k] = key
            end
            if k >= nshown then
                t:Show()
            end
        end
        for k = n, nshown - 1 do
            tex[k]:Hide()
        end
        nshown = n
    end

    local function SetOn(on)
        if on ~= f.on then
            f.on = on
            if on then
                f:Show()
            else
                f:Hide()
            end
        end
    end

    function f.Refresh()
        SetOn(not (enc:IsIdle() or f.held or f.loading))
    end

    f:SetScript("OnUpdate", function()
        if enc:Update(GetTime() * 1000) then
            Draw()
        end
        if enc:IsIdle() then
            SetOn(false)
        end
    end)
    f:SetScript("OnShow", function()
        enc:OnShow()
    end)
    f:SetScript("OnEvent", function(_, event)
        if event == "LOADING_SCREEN_ENABLED" then
            f.loading = true
            f.Refresh()
        elseif event == "LOADING_SCREEN_DISABLED" then
            f.loading = false
            f.Refresh()
        else
            Rescale()
        end
    end)
    f:RegisterEvent("DISPLAY_SIZE_CHANGED")
    f:RegisterEvent("UI_SCALE_CHANGED")
    f:RegisterEvent("LOADING_SCREEN_ENABLED")
    f:RegisterEvent("LOADING_SCREEN_DISABLED")
    Rescale()
    f.Draw = Draw
    return f
end

---------------------------------------------------------------------------- addon wiring
local state = { enc = nil, strip = nil, requested = "auto", held = false }

-- Mode "auto" is mono when the colourblind filter is on, else dense.
local function AutoMode()
    local ok, v = pcall(GetCVar, "colorblindSimulator")
    if ok and v ~= nil and v ~= "" and v ~= "0" then
        return MODE_MONO
    end
    return MODE_DENSE
end

local function ResolveMode(name)
    return MODE_BY_NAME[name] or AutoMode()
end

-- Creates the encoder and the strip from ns.sessionId, ns.db.mode and ns.ADDON_BUILD.
-- Runs once (registered with ns.OnLoad); later calls return the existing encoder.
function Bridge.Init()
    if state.enc then
        return state.enc
    end
    local mode = ns.db and ns.db.mode
    state.requested = (mode == "auto" or MODE_BY_NAME[mode]) and mode or "auto"
    local w, h = ScreenSize()
    state.enc = Bridge.NewEncoder({
        session = ns.sessionId or math.random(1, 65535),
        physW = w,
        physH = h,
        mode = ResolveMode(state.requested),
        addonBuild = ns.ADDON_BUILD or 0,
    })
    if CreateFrame and UIParent then
        state.strip = Bridge.CreateStrip(state.enc)
        state.strip.held = state.held
        -- mode "auto" follows the colourblind filter when it is switched in the game settings
        local watch = CreateFrame("Frame")
        watch:RegisterEvent("CVAR_UPDATE")
        watch:SetScript("OnEvent", function(_, _, name)
            if state.requested == "auto" and type(name) == "string" and not issecretvalue(name)
                and name:lower() == "colorblindsimulator" then
                Bridge.SetMode("auto")
            end
        end)
    end
    return state.enc
end

-- Queues a message (bytes from the builders above) and shows the strip; returns its msgId.
function Bridge.Send(msg)
    local enc = state.enc or Bridge.Init()
    local id = enc:Queue(msg)
    if state.strip then
        state.strip.Refresh()
    end
    return id
end

-- name: "auto", "dense", "robust" or "mono" (anything else is "auto"). Returns the effective
-- mode name. The caller stores the choice in ns.db.mode.
function Bridge.SetMode(name)
    if name ~= "auto" and not MODE_BY_NAME[name] then
        name = "auto"
    end
    state.requested = name
    local mode = ResolveMode(name)
    if state.enc then
        state.enc:SetMode(mode)
    end
    return MODE_NAMES[mode]
end

-- Effective mode name: "dense", "robust" or "mono".
function Bridge.ModeName()
    return MODE_NAMES[state.enc and state.enc.mode or ResolveMode(state.requested)]
end

-- on = true keeps the strip hidden with its queue intact (lockdown fallback); false resumes.
function Bridge.Hold(on)
    state.held = on and true or false
    if state.strip then
        state.strip.held = state.held
        state.strip.Refresh()
    end
end

-- True while a message is queued (the strip is shown or held).
function Bridge.IsBusy()
    return state.enc ~= nil and not state.enc:IsIdle()
end

if ns.OnLoad then
    ns.OnLoad(Bridge.Init)
end

return Bridge
