-- Space reserved under a clicked line for the helper to draw the translation in.
--
-- Each reserved row is a chat row of its own: a tag (8 inline colour blocks that carry the
-- slot, the row number, a session bit and a CRC) followed by a dark plate. WoW's chat renderer
-- lays the rows out, scrolls, clips, fades and hides them; the helper finds the tags in its
-- capture and paints only on plate pixels it sees. Lua geometry never positions a line: the
-- VIEW message (chat clip rectangles and font size) only bounds the helper's scan.
--
-- Slots name a request's rows: 0..99 chat lines, 100..124 the conversation window's bubbles,
-- 125 the status card of the Translate Companion popup (Bridge.PRESENCE_SLOT, no request), 126
-- the window's reply card, 127 the TEST message (docs\BRIDGE.md sections 8c and 8d). A chat slot is
-- freed when its rows are removed, or by a read-only scan of the chat history once its line has
-- left every chat frame. Rows are never rewritten on a timer, so lines fade like any other chat
-- line. A window slot belongs to { kind = "win", conv, msg } (never scanned, never expired) and
-- waits 2 s in a quarantine after it is freed, so the Helper never paints an old translation on
-- the bubble that inherits it during the strip delay.
local addonName, ns = ...

local Reserve = {}
ns.Reserve = Reserve

local floor, ceil, max, min = math.floor, math.ceil, math.max, math.min

Reserve.PLATE = true            -- false: no plate (full-row fill mode, TRANSLATE flags bit1 = 0)

local NUM_SLOTS = 100           -- chat slots 0..99 (0..126 until 2026-10-19, then the window took 100..126)
local WIN_FIRST, WIN_LAST = 100, 124    -- the conversation window's bubbles (125 is the presence card since build 6)
local QUARANTINE = 2.0          -- seconds a freed window slot waits before it is used again
Reserve.REPLY_SLOT = 126        -- the window's reply card (one reply at a time); 125 is Bridge.PRESENCE_SLOT,
                                -- 127 Bridge.TEST_SLOT
Reserve.CHAT_SLOTS = NUM_SLOTS
Reserve.WIN_FIRST, Reserve.WIN_LAST = WIN_FIRST, WIN_LAST
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
-- Bubbles of the conversation window: the same factors leaned up by 10% (a cut translation costs
-- a click, an empty row only space), except Latin to Latin, whose bubbles came out too tall (user
-- request of 2026-10-19: 3 rows for a 1-row translation). 1.25 is measured: E1 on CPU, the
-- shipped engine on the Latin-to-Latin lines of tests\eval\chat_samples.tsv, output length over
-- input length outside the protected spans (tests\eval\out\e1d_lengths.json, counts only): the 22
-- lines of 30 characters or more (the ones that can need a second row) median 1.06, P75 1.30, P90
-- 1.39, max 1.42; with the Companion's shrink down to 80% a row holds 1.25 times more, so
-- 1.25 x 1.25 = 1.56 covers them all. Shorter lines run higher (all 37: P90 1.78, max 2.5: an
-- abbreviation spelled out) but stay at 35 characters out at most, one row ("..." stays possible;
-- a click asks again with 6 rows).
local WINDOW_LEAN = 1.1
local WINDOW_FACTOR = { ["latin>latin"] = 1.25 }

local free = {}                 -- FIFO of free chat slots: the most recently freed is reused last
local owner = {}                -- chat slot -> lineID
local inUse = 0
local winFree = {}              -- FIFO of free window slots
local winQuarantine = {}        -- FIFO of { slot, at } freed window slots
local winOwner = {}             -- window slot -> { kind = "win", conv, msg }
local winInUse = 0
local ticker, lastViewKey
local lastScan = -math.huge      -- GetTime() of the last read-only scan
local strippedOnce = false
local counters = { scans = 0, scanFreed = 0, viewsSent = 0, stripped = 0, winAlloc = 0, winFreed = 0, winRefused = 0 }

for slot = 0, NUM_SLOTS - 1 do
    free[#free + 1] = slot
end
for slot = WIN_FIRST, WIN_LAST do
    winFree[#winFree + 1] = slot
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

-- How much longer a translation into target usually is than visibleText (by writing script).
function Reserve.ScriptFactor(visibleText, target)
    local script = ns.Lang.Script(visibleText)
    return script and FACTOR[script .. ">" .. ns.Lang.TargetScript(target)] or DEFAULT_FACTOR
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
    local f = Reserve.ScriptFactor(visibleText, target)
    return min(MAX_ROWS, max(1, ceil(rowsSrc * f - 0.15)))
end

-- How much longer a translation into target usually is than visibleText, in a bubble.
function Reserve.WindowFactor(visibleText, target)
    local script = ns.Lang.Script(visibleText)
    local own = script and WINDOW_FACTOR[script .. ">" .. ns.Lang.TargetScript(target)]
    return own or Reserve.ScriptFactor(visibleText, target) * WINDOW_LEAN
end

-- Rows of a window bubble (1..MAX_ROWS) from the bubble's own text width (the window font; the
-- plate width of WindowMetrics): the text outside link names times the bubble factor, plus the
-- link names as they are (linkWidth, the width of every link name of the line: Translate Companion
-- copies them as written), against the plate width. Like the inline estimate, the ratio is not
-- rounded up first, so a short line reserves one row.
function Reserve.EstimateWindowRows(textWidth, plateWidth, visibleText, target, linkWidth)
    if not (IsPositive(textWidth) and IsPositive(plateWidth)) then
        return 1
    end
    linkWidth = IsNumber(linkWidth) and min(textWidth, max(0, linkWidth)) or 0
    local f = Reserve.WindowFactor(visibleText, target)
    local est = ((textWidth - linkWidth) * f + linkWidth) / plateWidth
    return min(MAX_ROWS, max(1, ceil(est - 0.15)))
end

-- Geometry of the conversation window's bubbles, in the window's units (docs\ADDON.md, the
-- conversation window). frame: the window (for its effective scale); contentWidth: the width of
-- the thread's scroll child; contentLeft: the scroll child's left edge from the window's left.
-- Returns m = { ppu, W, H, rowH, rowPx, fontPx, pad, border, inset, bubbleWidth, plateWidth, x0,
-- margin, fillX0, fillWidth, textSize }, or nil when the scale or the screen size is unreadable.
--   W       tag block width: at least 5 physical px
--   H       nominal row height (text size + 2); rowH = rowPx / ppu is the exact row pitch
--   pad     fill around the rows: at least 6 units and 4 physical px
--   border  the part of a bubble its ChatBubble border art may shade; the fill is clean inside
--   inset   border + pad: from a bubble's edge to its first row and to its tag
--   x0      the bubble column's left edge from the window's left; fillX0 its clean fill
local BUBBLE_MARGIN, BUBBLE_SPARE, BUBBLE_BORDER = 6, 36, 10
function Reserve.WindowMetrics(frame, contentWidth, contentLeft)
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    local okEff, eff = pcall(frame.GetEffectiveScale, frame)
    if not (okSize and okEff and IsPositive(physH) and IsPositive(eff)) then
        return nil
    end
    local textSize = (ns.db and tonumber(ns.db.convText)) or 14
    textSize = min(20, max(12, floor(textSize + 0.5)))
    local m = { textSize = textSize, border = BUBBLE_BORDER, margin = BUBBLE_MARGIN }
    m.ppu = eff * physH / 768
    m.W = max(2, ceil(5 / m.ppu))
    m.H = textSize + 2
    m.rowPx = max(1, Round(m.H * m.ppu))
    m.rowH = m.rowPx / m.ppu
    m.fontPx = Round(textSize * m.ppu)
    m.pad = max(6, ceil(4 / m.ppu))
    m.inset = m.border + m.pad
    contentWidth = IsPositive(contentWidth) and contentWidth or 396
    m.bubbleWidth = max(120, contentWidth - BUBBLE_SPARE)
    m.plateWidth = max(20, m.bubbleWidth - 2 * m.border - 2 * m.pad - 8 * m.W)
    m.x0 = (contentLeft or 14) + BUBBLE_MARGIN
    m.fillX0 = m.x0 + m.border
    m.fillWidth = m.bubbleWidth - 2 * m.border
    return m
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

-- The lineID of a chat slot, or the { kind = "win", conv, msg } owner of a window slot.
function Reserve.SlotOwner(slot)
    return owner[slot] or winOwner[slot]
end

-- Chat slots in use (window slots: Reserve.WindowSlotsInUse).
function Reserve.SlotsInUse()
    return inUse
end

-- Quarantined window slots whose 2 s are over go back to the free FIFO, oldest first.
local function ReleaseQuarantine()
    local now = GetTime()
    while winQuarantine[1] and now - winQuarantine[1].at >= QUARANTINE do
        winFree[#winFree + 1] = table.remove(winQuarantine, 1).slot
    end
end

-- A window slot (100..124) for slotOwner = { kind = "win", conv, msg }, or nil when every slot
-- is in use or still in quarantine (the caller frees the least recently visible one and tries
-- again once the quarantine is over).
function Reserve.AllocWindowSlot(slotOwner)
    ReleaseQuarantine()
    local slot = table.remove(winFree, 1)
    if slot == nil then
        counters.winRefused = counters.winRefused + 1
        return nil
    end
    winOwner[slot] = slotOwner
    winInUse = winInUse + 1
    counters.winAlloc = counters.winAlloc + 1
    Reserve.StartWatch()
    return slot
end

-- Puts a window slot into the quarantine (the caller sent its CANCEL).
function Reserve.FreeWindowSlot(slot)
    if slot ~= nil and winOwner[slot] ~= nil then
        winOwner[slot] = nil
        winInUse = winInUse - 1
        counters.winFreed = counters.winFreed + 1
        winQuarantine[#winQuarantine + 1] = { slot = slot, at = GetTime() }
    end
end

function Reserve.WindowSlotsInUse()
    return winInUse
end

-- Seconds until the oldest quarantined window slot is free again, or nil when none waits.
function Reserve.QuarantineLeft()
    local q = winQuarantine[1]
    if q then
        return max(0, QUARANTINE - (GetTime() - q.at))
    end
end

function Reserve.IsWindowSlot(slot)
    return type(slot) == "number" and slot >= WIN_FIRST and slot <= WIN_LAST
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

-- The Translate Companion popup's VIEW entry (id 201, its status card) while it shows, else nil.
local function PopupView()
    local dialog = ns.CompanionDialog
    if not (dialog and dialog.ViewEntry) then
        return nil
    end
    local ok, view = pcall(dialog.ViewEntry)
    if ok and type(view) == "table" then
        return view
    end
end

-- The conversation window's VIEW entry (id 200) while it is shown, else nil.
local function WindowView()
    local cf = ns.ConvFrame
    if not (cf and cf.ViewEntry) then
        return nil
    end
    local ok, view = pcall(cf.ViewEntry)
    if ok and type(view) == "table" then
        return view
    end
end

-- True when the conversation window overlaps this chat frame while no inline request is active:
-- nothing of ours can be drawn there, so the frame is left out of the VIEW.
local function CoveredChat(frame)
    local cf = ns.ConvFrame
    if not (cf and cf.Covers) or ns.Request.Count() > 0 then
        return false
    end
    local ok, covered = pcall(cf.Covers, frame)
    return ok and covered == true
end

-- Rectangles of the Translate Companion popup's card (first, id 201, while it shows), of the
-- conversation window (id 200, while shown) and of the shown chat frames, 4 in all at most, in
-- physical px from the client's top-left. A frame whose rectangle is missing or secret is left
-- out, and so is a chat frame the window overlaps while no inline request is active.
function Reserve.View()
    local views = {}
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    if not (okSize and IsPositive(physH)) then
        return views
    end
    local k = physH / 768
    local popup = PopupView()
    if popup then
        views[#views + 1] = popup
    end
    local win = WindowView()
    if win then
        views[#views + 1] = win
    end
    ChatFrameUtil.ForEachChatFrame(function(frame)
        if #views >= MAX_VIEWS then
            return
        end
        local okVisible, visible = pcall(frame.IsVisible, frame)
        if not okVisible or issecretvalue(visible) or not visible then
            return
        end
        if win and CoveredChat(frame) then
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

-- 2 Hz geometry watch, only while chat requests are active or the conversation window uses
-- slots or is shown (geometry reads only). Every SCAN_SECONDS it also runs the read-only scan,
-- so the slots of lines gone from the chat are freed a few at a time (one CANCEL each), and the
-- watch stops once nothing needs it (Request.Drop, Reserve.MaybeStopWatch).
function Reserve.StartWatch()
    if not ticker then
        ticker = C_Timer.NewTicker(WATCH_SECONDS, function()
            Reserve.SyncView(false)
            if inUse > 0 and GetTime() - lastScan >= SCAN_SECONDS then
                Reserve.Scan()
            end
            Reserve.MaybeStopWatch()
        end)
    end
end

function Reserve.StopWatch()
    if ticker then
        ticker:Cancel()
        ticker = nil
    end
end

-- Stops the watch once no chat request is active, no window slot is in use and the window is
-- hidden.
function Reserve.MaybeStopWatch()
    local cf = ns.ConvFrame
    local shown = cf and cf.IsShown and cf.IsShown()
    if ns.Request.Count() == 0 and winInUse == 0 and not shown then
        Reserve.StopWatch()
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
    c.winInUse, c.winFree, c.winQuarantined = winInUse, #winFree, #winQuarantine
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
