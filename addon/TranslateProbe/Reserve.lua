-- Space reserved under a clicked line for the helper to draw the translation in.
--
-- Each reserved row is a chat row of its own: a tag (8 inline colour blocks that carry the
-- slot, the row number, a session bit and a CRC) followed by a dark plate. WoW's chat renderer
-- lays the rows out, scrolls, clips, fades and hides them; the helper finds the tags in its
-- capture and paints only on plate pixels it sees. Lua geometry never positions a line: the
-- VIEW message (chat clip rectangles and font size) only bounds the helper's scan.
--
-- Slots (0..126) name a request's rows. A slot is freed when its rows are removed, or by a
-- read-only scan of the chat history once its line has left every chat frame. Rows are never
-- rewritten on a timer, so lines fade like any other chat line.
local addonName, ns = ...

local Reserve = {}
ns.Reserve = Reserve

local floor, ceil, max, min = math.floor, math.ceil, math.max, math.min

Reserve.PLATE = true            -- false: no plate (full-row fill mode, TRANSLATE flags bit1 = 0)

local NUM_SLOTS = 127           -- 0..126; 127 is Bridge.TEST_SLOT
local MAX_VIEWS = 4
local WATCH_SECONDS = 0.5
local SCAN_SECONDS = 15        -- read-only slot scan from the watcher, at most this often
local INDENT = "    "           -- the 4 spaces before each row (ChatButton's SUFFIX_START)
local FALLBACK_ROWS = 2
local MAX_ROWS = 6              -- TRANSLATE rows 1..6 (4 until 2026-10-18: German into French was cut)
Reserve.MAX_ROWS = MAX_ROWS
local TEX_PROBE = "|TInterface\\Buttons\\WHITE8X8:10:100|t"   -- 100 texture units wide

-- Source script > target script -> how much longer the translation usually is.
local FACTOR = {
    ["latin>latin"] = 1.35, ["cyrillic>latin"] = 1.2, ["latin>cyrillic"] = 1.2,
    ["han>latin"] = 2.2, ["kana>latin"] = 2.2, ["hangul>latin"] = 1.8,
    ["han>cyrillic"] = 2.2, ["hangul>cyrillic"] = 1.8,
    ["latin>han"] = 0.8, ["cyrillic>han"] = 0.8, ["latin>hangul"] = 1.0,
}
local DEFAULT_FACTOR = 1.3

local free = {}                 -- FIFO of free slots: the most recently freed is reused last
local owner = {}                -- slot -> lineID
local inUse = 0
local ticker, lastViewKey
local lastScan = -math.huge      -- GetTime() of the last read-only scan
local strippedOnce = false
local counters = { scans = 0, scanFreed = 0, viewsSent = 0, stripped = 0 }

for slot = 0, NUM_SLOTS - 1 do
    free[#free + 1] = slot
end

local function IsNumber(v)
    return not issecretvalue(v) and type(v) == "number" and v == v
end

local function IsPositive(v)
    return IsNumber(v) and v > 0
end

local function Round(v)
    return floor(v + 0.5)
end

local function EscapePattern(s)
    return (s:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"))
end

---------------------------------------------------------------------------- measuring
local measureFrame, measureFS

local function MeasureString()
    if not measureFS then
        measureFrame = CreateFrame("Frame", nil, UIParent)
        measureFrame:Hide()
        measureFS = measureFrame:CreateFontString(nil, "ARTWORK")
    end
    return measureFS
end

-- Width, height and line count of text in the chat frame's font, in chat frame units. width
-- (chat units, optional) wraps the text. Errors when anything is missing.
function Reserve.Measure(chatFrame, text, width)
    local path, fontH, flags = chatFrame:GetFont()
    local eff = chatFrame:GetEffectiveScale()
    assert(type(path) == "string" and not issecretvalue(path) and IsPositive(fontH) and IsPositive(eff))
    local fs = MeasureString()
    local k = eff / fs:GetEffectiveScale()    -- chat units -> measuring units
    fs:SetFont(path, fontH * k, flags or "")
    fs:SetWidth(width and width * k or 0)
    fs:SetText(text)
    return fs:GetUnboundedStringWidth() / k, fs:GetStringHeight() / k, fs:GetNumLines(), fs
end

local function ComputeMetrics(chatFrame)
    local _, physH = GetPhysicalScreenSize()
    local eff = chatFrame:GetEffectiveScale()
    local path, fontH = chatFrame:GetFont()
    local container = chatFrame.FontStringContainer or chatFrame
    local cw = container:GetWidth()
    if not (IsPositive(physH) and IsPositive(eff) and IsPositive(fontH) and IsPositive(cw)) then
        return nil
    end
    if type(path) ~= "string" or issecretvalue(path) then
        return nil
    end
    local m = { physH = physH, eff = eff, fontH = fontH, cw = cw }
    m.ppu = eff * physH / 768                         -- physical px per chat unit
    -- Inline texture sizes are not always in chat units: on WoW Forever a texture unit came out
    -- 1.47 chat units wide with a scaled chat frame (measured 2026-10-18). Measure the ratio.
    local okTex, texW = pcall(Reserve.Measure, chatFrame, TEX_PROBE)
    m.tex = (okTex and IsPositive(texW)) and min(4, max(0.25, texW / 100)) or 1
    local texPx = m.ppu * m.tex                       -- physical px per texture unit
    m.W = max(2, ceil(5 / texPx))                     -- tag block width: at least 5 physical px
    m.H = max(6, floor(fontH / m.tex + 0.5))          -- tag and plate height: one text line
    m.indent = Reserve.Measure(chatFrame, INDENT) + 8
    m.plateW = floor((cw - m.indent - 8 * m.W * m.tex - 2) * 0.88 / m.tex)
    m.ok = m.plateW >= 40
    return m
end

-- Row geometry of a chat frame in its own units, or nil when anything is missing or secret.
function Reserve.Metrics(chatFrame)
    local ok, m = pcall(ComputeMetrics, chatFrame)
    if ok then
        return m
    end
end

-- Number of reserved rows (1..MAX_ROWS) for a line: its width in plates, times how much longer the
-- target script usually is.
function Reserve.EstimateRows(chatFrame, visibleText, target)
    local m = Reserve.Metrics(chatFrame)
    if not (m and m.ok) then
        return FALLBACK_ROWS
    end
    local ok, width = pcall(Reserve.Measure, chatFrame, visibleText)
    if not ok then
        return FALLBACK_ROWS
    end
    -- both sides in scale-1 units: width (chat units) * eff against plateW * eff. The ratio is
    -- not rounded up before the factor, so a short line ("gg") reserves one row, not two.
    local rowsSrc = width * m.eff / (m.plateW * m.tex * m.eff)
    local script = ns.Lang.Script(visibleText)
    local f = script and FACTOR[script .. ">" .. ns.Lang.TargetScript(target)] or DEFAULT_FACTOR
    return min(MAX_ROWS, max(1, ceil(rowsSrc * f - 0.15)))
end

-- The K reserved rows of a slot: tag, then the plate. The tag's row field has 2 bits, so rows 4
-- and 5 carry 0 and 1 again (Bridge.TagDigits takes row % 4); the helper numbers a card's rows on
-- from its first visible tag (docs\BRIDGE.md).
function Reserve.Rows(m, slot, K)
    local rows, sessBit = {}, ns.sessionId % 2
    for r = 0, K - 1 do
        rows[r + 1] = ns.Bridge.TagMarkup(slot, r, sessBit, m.W, m.H)
            .. (Reserve.PLATE and ns.Bridge.PlateMarkup(m.plateW, m.H) or "")
    end
    return rows
end

---------------------------------------------------------------------------- slots
function Reserve.FreeSlot(slot)
    if slot ~= nil and owner[slot] ~= nil then
        owner[slot] = nil
        inUse = inUse - 1
        free[#free + 1] = slot
    end
end

function Reserve.SlotOwner(slot)
    return owner[slot]
end

function Reserve.SlotsInUse()
    return inUse
end

-- Read-only scan of every chat frame: the predicate records the lineID of each of our rows of
-- this session and always returns false, so no entry is ever repackaged (no timestamp change,
-- no un-fade). Slots whose line was not seen are freed. Returns the number freed.
function Reserve.Scan()
    counters.scans = counters.scans + 1
    lastScan = GetTime()
    local seen = {}
    local pattern = "|H" .. EscapePattern("addon:" .. addonName .. ":x:" .. ns.session .. ":") .. "(%d+)|h"
    local function Look(message)
        if not issecretvalue(message) and type(message) == "string" then
            for id in message:gmatch(pattern) do
                seen[tonumber(id)] = true
            end
        end
        return false
    end
    local function Keep(...)
        return ...
    end
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(Look, Keep)
    end)
    local gone = {}
    for slot = 0, NUM_SLOTS - 1 do
        local lineID = owner[slot]
        if lineID ~= nil and not seen[lineID] then
            gone[#gone + 1] = lineID
            Reserve.FreeSlot(slot)
        end
    end
    counters.scanFreed = counters.scanFreed + #gone
    for _, lineID in ipairs(gone) do
        ns.Request.Drop(lineID)
    end
    return #gone
end

-- A free slot for a line: when none is left, the read-only scan reclaims slots of lines gone
-- from the chat; only when all 127 are still on screen does the oldest request expire.
function Reserve.AllocSlot(lineID)
    if #free == 0 then
        Reserve.Scan()
    end
    if #free == 0 then
        ns.Request.ExpireOldest()
    end
    local slot = table.remove(free, 1)
    if slot ~= nil then
        owner[slot] = lineID
        inUse = inUse + 1
    end
    return slot
end

---------------------------------------------------------------------------- view
local function ViewOf(frame, k, physH)
    local container = frame.FontStringContainer or frame
    local l, b, w, h = container:GetScaledRect()
    if not (IsNumber(l) and IsNumber(b) and IsPositive(w) and IsPositive(h)) then
        return nil
    end
    local _, fontH = frame:GetFont()
    local eff = frame:GetEffectiveScale()
    if not (IsPositive(fontH) and IsPositive(eff)) then
        return nil
    end
    local spacing = frame.GetSpacing and frame:GetSpacing() or 0
    if not IsNumber(spacing) then
        spacing = 0
    end
    local id = frame:GetID()
    return {
        id = (IsNumber(id) and id or 0) % 256,
        x = Round(l * k), y = Round(physH - (b + h) * k), w = Round(w * k), h = Round(h * k),
        fontPx = Round(fontH * eff * k), rowPx = Round((fontH + spacing) * eff * k),
    }
end

-- Clip rectangles of up to 4 shown chat frames, in physical px from the client's top-left. A
-- frame whose rectangle is missing or secret is left out.
function Reserve.View()
    local views = {}
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    if not (okSize and IsPositive(physH)) then
        return views
    end
    local k = physH / 768
    ChatFrameUtil.ForEachChatFrame(function(frame)
        if #views >= MAX_VIEWS then
            return
        end
        local okVisible, visible = pcall(frame.IsVisible, frame)
        if not okVisible or issecretvalue(visible) or not visible then
            return
        end
        local ok, view = pcall(ViewOf, frame, k, physH)
        if ok and view then
            views[#views + 1] = view
        end
    end)
    return views
end

function Reserve.ViewKey(views)
    local parts = {}
    for i, v in ipairs(views) do
        parts[i] = table.concat({ v.id, v.x, v.y, v.w, v.h, v.fontPx, v.rowPx }, ",")
    end
    return table.concat(parts, ";")
end

-- Sends VIEW when the chat rectangles changed since the last one (or always with force).
-- An empty view is never sent: the helper keeps the last one. Returns true when sent.
function Reserve.SyncView(force)
    local views = Reserve.View()
    if #views == 0 then
        return false
    end
    local key = Reserve.ViewKey(views)
    if not force and key == lastViewKey then
        return false
    end
    lastViewKey = key
    counters.viewsSent = counters.viewsSent + 1
    ns.Bridge.Send(ns.Bridge.ViewMessage(views))
    return true
end

-- 2 Hz geometry watch, only while requests are active (geometry reads only). Every SCAN_SECONDS
-- it also runs the read-only scan, so the slots of lines gone from the chat are freed a few at a
-- time (one CANCEL each) and the watch stops once no request is left (Request.Drop).
function Reserve.StartWatch()
    if not ticker then
        ticker = C_Timer.NewTicker(WATCH_SECONDS, function()
            Reserve.SyncView(false)
            if inUse > 0 and GetTime() - lastScan >= SCAN_SECONDS then
                Reserve.Scan()
            end
        end)
    end
end

function Reserve.StopWatch()
    if ticker then
        ticker:Cancel()
        ticker = nil
    end
end

function Reserve.IsWatching()
    return ticker ~= nil
end

---------------------------------------------------------------------------- session hygiene
-- Chat history survives a /reload: rows of an older session would carry tags the helper can
-- no longer match, so they are cut once, at the first PLAYER_ENTERING_WORLD.
function Reserve.StripForeignSessions()
    local n = ns.ChatButton.CutRows(function(session)
        return session ~= ns.session
    end)
    counters.stripped = counters.stripped + n
    return n
end

function Reserve.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    c.inUse, c.free = inUse, #free
    return c
end

local events = CreateFrame("Frame")
events:RegisterEvent("PLAYER_ENTERING_WORLD")
events:SetScript("OnEvent", function()
    if not strippedOnce then
        strippedOnce = true
        events:UnregisterEvent("PLAYER_ENTERING_WORLD")
        Reserve.StripForeignSessions()
    end
end)
