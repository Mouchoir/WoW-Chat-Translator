-- Test stub of addon\TranslateProbe\Bridge.lua (bridge protocol v2, docs\BRIDGE.md).
-- Same Lua API. The message builders, row tags and plate markup are the real ones (copied from
-- Bridge.lua); the pixel strip is replaced by a recorder: Bridge.Send appends each message to
-- Bridge._sent and nothing is drawn. tests\run.js loads it with --stub-bridge, or when the
-- real file is missing.
local _, ns = ...
ns = ns or {}

local floor, min = math.floor, math.min
local byte, char, find, format = string.byte, string.char, string.find, string.format
local concat = table.concat

local Bridge = {}
ns.Bridge = Bridge

local MSG_TRANSLATE, MSG_CANCEL, MSG_CLEAR, MSG_TEST, MSG_VIEW = 0x10, 0x11, 0x12, 0x13, 0x14
local MAX_TEXT, MAX_TARGET, MAX_PAIRS, MAX_TERM, MAX_MEANING, MAX_PAIR_BYTES = 1024, 8, 8, 48, 120, 512
local MAX_VIEWS = 4
local FLAG_NOCACHE, FLAG_PLATE, FLAG_REPLY, FLAG_COPY = 1, 2, 4, 8
local FLAGS_MASK = 15
local MODE_NAMES = { [0] = "dense", [1] = "robust", [2] = "mono" }
local MODE_BY_NAME = { dense = 0, robust = 1, mono = 2 }
local TEX_TAG = "|TInterface\\Buttons\\WHITE8X8:%d:%d:0:0:8:8:0:8:0:8:%d:%d:%d|t"
local TAG_SYNC = { { 255, 0, 255 }, { 0, 255, 0 } }
local TAG_DIGIT = { [0] = { 0, 0, 0 }, { 255, 0, 0 }, { 0, 0, 255 }, { 255, 255, 0 }, { 0, 255, 255 },
    { 255, 255, 255 } }
local PLATE_RGB = { 32, 40, 64 }
local BUBBLE_RGB = { 36, 30, 24 }

Bridge.VERSION = 2
Bridge.TEST_SLOT = 127
Bridge.PLATE_RGB = PLATE_RGB
Bridge.BUBBLE_RGB = BUBBLE_RGB
Bridge.FLAG_NOCACHE, Bridge.FLAG_PLATE, Bridge.FLAG_REPLY, Bridge.FLAG_COPY = FLAG_NOCACHE, FLAG_PLATE, FLAG_REPLY,
    FLAG_COPY
Bridge.FLAGS_MASK = FLAGS_MASK
Bridge.LEVELS = { 0, 104, 146, 255 }
Bridge.MODES = MODE_NAMES
Bridge.IS_STUB = true
Bridge._sent = {}

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

-- Bits 0-3 kept; copy dropped unless reply (as the real Bridge.lua, without the bit library).
local function TranslateFlags(flags)
    flags = Clamp(flags, 0, 255) % (FLAGS_MASK + 1)
    if flags >= FLAG_COPY and floor(flags / FLAG_REPLY) % 2 == 0 then
        flags = flags - FLAG_COPY
    end
    return flags
end

function Bridge.TranslateMessage(reqId, slot, rows, flags, target, text, pairs)
    text = CutMarked(type(text) == "string" and text or "", MAX_TEXT)
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

function Bridge.ViewMessage(views)
    local n = min(type(views) == "table" and #views or 0, MAX_VIEWS)
    local parts = { char(MSG_VIEW), char(n) }
    for i = 1, n do
        local v = views[i]
        parts[i + 2] = U8(v.id) .. U16(v.x) .. U16(v.y) .. U16(v.w) .. U16(v.h) .. U8(v.fontPx) .. U8(v.rowPx)
    end
    return concat(parts)
end

function Bridge.Crc5(payload10)
    local p = floor(tonumber(payload10) or 0) % 1024
    local c = 0x1F
    for i = 9, 0, -1 do
        local b = floor(p / 2 ^ i) % 2
        local top = floor(c / 16) % 2
        c = (c * 2) % 32
        if b ~= top then
            -- c xor 0x05 without the bit library (bits 0 and 2)
            local b0, b2 = c % 2, floor(c / 4) % 2
            c = c + (b0 == 1 and -1 or 1) + (b2 == 1 and -4 or 4)
        end
    end
    return c
end

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

function Bridge.PlateMarkup(w, h)
    return format(TEX_TAG, Clamp(h, 1, 4096), Clamp(w, 1, 4096), PLATE_RGB[1], PLATE_RGB[2], PLATE_RGB[3])
end

function Bridge.ApplyTag(textures, slot, row, sessBit)
    local colors = Bridge.TagColors(slot, row, sessBit)
    for i = 1, 8 do
        local c = colors[i]
        textures[i]:SetColorTexture(c[1] / 255, c[2] / 255, c[3] / 255, 1)
    end
end

---------------------------------------------------------------------------- stub transport
local Encoder = {}
Encoder.__index = Encoder

function Bridge.NewEncoder(opts)
    local e = setmetatable({}, Encoder)
    e.session = Clamp(opts.session, 1, 65535)
    e.mode = MODE_NAMES[opts.mode] and opts.mode or 0
    e.addonBuild = Clamp(opts.addonBuild, 0, 65535)
    e.physW, e.physH = opts.physW, opts.physH
    e.nextMsg = 1
    return e
end

function Encoder:SetScreen(w, h)
    self.physW, self.physH = w, h
end

function Encoder:SetMode(m)
    if MODE_NAMES[m] then
        self.mode = m
    end
end

function Encoder:Queue()
    local id = self.nextMsg
    self.nextMsg = id % 65535 + 1
    return id
end

function Encoder:Update()
    return false
end

function Encoder:IsIdle()
    return true
end

function Encoder:BlockCount()
    return 0
end

function Encoder:BlockKey()
    return 0
end

function Encoder:OnShow() end

function Bridge.CreateStrip()
    local f = CreateFrame("Frame", nil, UIParent)
    f:Hide()
    f.held, f.loading, f.on = false, false, false
    function f.Refresh() end
    function f.Draw() end
    return f
end

local state = { enc = nil, requested = "auto", held = false }

local function AutoMode()
    local ok, v = pcall(GetCVar, "colorblindSimulator")
    if ok and v ~= nil and v ~= "" and v ~= "0" then
        return 2
    end
    return 0
end

local function ResolveMode(name)
    return MODE_BY_NAME[name] or AutoMode()
end

function Bridge.Init()
    if state.enc then
        return state.enc
    end
    local mode = ns.db and ns.db.mode
    state.requested = (mode == "auto" or MODE_BY_NAME[mode]) and mode or "auto"
    state.enc = Bridge.NewEncoder({
        session = ns.sessionId or 1, physW = 1920, physH = 1080, mode = ResolveMode(state.requested),
        addonBuild = ns.ADDON_BUILD or 0,
    })
    if CreateFrame then
        -- as the real Bridge: mode "auto" follows the colourblind filter
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

function Bridge.Send(msg)
    local enc = state.enc or Bridge.Init()
    Bridge._sent[#Bridge._sent + 1] = msg
    return enc:Queue(msg)
end

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

function Bridge.ModeName()
    return MODE_NAMES[state.enc and state.enc.mode or ResolveMode(state.requested)]
end

function Bridge.Hold(on)
    state.held = on and true or false
end

function Bridge.IsBusy()
    return false
end

if ns.OnLoad then
    ns.OnLoad(Bridge.Init)
end

return Bridge
