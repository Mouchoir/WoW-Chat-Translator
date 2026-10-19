-- What a [T] click does in the inline mode: reserve rows under the line and send a TRANSLATE
-- request over the pixel bridge. The helper does the rest (queue, cache, model, drawing);
-- nothing ever comes back to Lua, so a request lives here until its rows are removed. The
-- conversation window's bubbles send their TRANSLATE through StartWindow and DropWindow below.
-- State is memory only.
--
-- Hint lane (user request U1): the rows end with one more row, the lane (Reserve.LaneRow), which
-- the Helper paints over with the chat background whenever it paints the card. The addon cannot
-- know whether the Helper runs, so the lane says "No translation above? Start Translate
-- Companion. [Download]" and only Translate Companion hides it. It holds spaces at the click and
-- gets its text Reserve.LANE_DELAY (1.5 s) later, in one more rewrite of the line (a C_Timer, no
-- OnUpdate), so the hint never flashes before the Helper's first paint. It goes with the rows.
local _, ns = ...
local L = ns.L

local Request = {}
ns.Request = Request

local DEBOUNCE = 1.0
local FLAG_NO_CACHE, FLAG_PLATE = 1, 2

local active = {}       -- lineID -> { reqId, slot, K, target, flags, instant, lane }
local byReq = {}        -- reqId -> lineID
local lastClick = {}    -- lineID -> GetTime() of the last accepted click
local lastClickCount = 0
local deferredRow = {}  -- lineID -> true while its TR_DEFERRED row is shown
local nextReq = 0
local count = 0
local counters = {
    clicks = 0, debounced = 0, sent = 0, resent = 0, restarted = 0, deferred = 0, tooOld = 0,
    removed = 0, expired = 0, dropped = 0, cleared = 0, windowSent = 0, windowDropped = 0, lanes = 0,
}

-- 1..65535, wraps to 1. Shared by chat lines, window bubbles and the window's reply.
local function NextReqId()
    nextReq = nextReq % 65535 + 1
    return nextReq
end
Request.NextReqId = NextReqId

local function Flags(shift)
    return (shift and FLAG_NO_CACHE or 0) + (ns.Reserve.PLATE and FLAG_PLATE or 0)
end

-- Every TRANSLATE goes out with a VIEW just before it, changed or not: a helper started (or
-- restarted) after the last VIEW knows the chat rectangles at the first click, instead of
-- staying in its discovery scan (about 15 more bytes on the strip).
local function SendTranslate(a, marked, glossPairs)
    ns.Reserve.SyncView(true)
    ns.Bridge.Send(ns.Bridge.TranslateMessage(a.reqId, a.slot, a.K, a.flags, a.target, marked, glossPairs))
end

local function Forget(lineID)
    local a = active[lineID]
    if a then
        active[lineID] = nil
        byReq[a.reqId] = nil
        count = count - 1
        if count == 0 then
            ns.Reserve.MaybeStopWatch()
        end
    end
    return a
end

local function ClearClick(lineID)
    if lastClick[lineID] ~= nil then
        lastClick[lineID] = nil
        lastClickCount = lastClickCount - 1
    end
end

local function NoteClick(lineID, now)
    if lastClick[lineID] == nil then
        lastClickCount = lastClickCount + 1
    end
    lastClick[lineID] = now
    if lastClickCount > 256 then
        for id, t in pairs(lastClick) do
            if now - t >= DEBOUNCE then
                lastClick[id] = nil
                lastClickCount = lastClickCount - 1
            end
        end
    end
end

-- The suffix of a request's line, per chat frame: K tagged rows, the hint lane (spaces until
-- revealed), then the instant rows (glossary, name).
local function Suffix(slot, K, instant, revealed)
    return function(frame)
        local m = ns.Reserve.Metrics(frame)
        if not (m and m.ok) then
            return nil
        end
        local rows = ns.Reserve.Rows(m, slot, K)
        rows[#rows + 1] = ns.Reserve.LaneRow(frame, m, revealed)
        for _, line in ipairs(instant) do
            rows[#rows + 1] = ns.ChatButton.Safe(line)
        end
        return rows
    end
end

-- LANE_DELAY after the click: the lane gets its hint, unless the rows went meanwhile (removed,
-- the line gone, /tpr clear) or were made again for a newer click.
local function RevealLane(lineID, a)
    if active[lineID] ~= a or a.lane ~= "pending" then
        return
    end
    a.lane = "shown"
    counters.lanes = counters.lanes + 1
    ns.ChatButton.SetSuffix(lineID, Suffix(a.slot, a.K, a.instant, true))
end

function Request.Start(lineID, chatFrame, shift)
    local now = GetTime()
    if now - (lastClick[lineID] or -9) < DEBOUNCE then
        counters.debounced = counters.debounced + 1
        return
    end
    NoteClick(lineID, now)
    counters.clicks = counters.clicks + 1
    chatFrame = chatFrame or DEFAULT_CHAT_FRAME

    local text = ns.ChatButton.SourceText(lineID)
    if not text then
        if C_ChatInfo.InChatMessagingLockdown() then
            counters.deferred = counters.deferred + 1
            ns.ChatButton.SetSuffix(lineID, { L.TR_DEFERRED })
            deferredRow[lineID] = true
            ns.Restriction.Defer(lineID, function()
                ClearClick(lineID)
                Request.Start(lineID, chatFrame, false)
            end)
        else
            counters.tooOld = counters.tooOld + 1
            if deferredRow[lineID] then
                deferredRow[lineID] = nil
                ns.ChatButton.SetSuffix(lineID, nil)
            end
            ns.Print(L.SUFFIX_TOO_OLD)
        end
        return
    end

    local target = ns.db.target
    local marked = ns.Glossary.PromptText(text)
    local glossPairs = ns.Glossary.Extract(text, target)

    local a = active[lineID]
    if a then
        if shift or a.target ~= target then
            -- Shift-click or a new target: same rows, new request (the card restarts)
            counters.restarted = counters.restarted + 1
            byReq[a.reqId] = nil
            a.reqId, a.target, a.flags = NextReqId(), target, Flags(shift)
            byReq[a.reqId] = lineID
        else
            -- plain re-click: the same request again (served from the helper's cache, and
            -- repairs a request the helper missed)
            counters.resent = counters.resent + 1
        end
        SendTranslate(a, marked, glossPairs)
        return
    end

    local slot = ns.Reserve.AllocSlot(lineID)
    if slot == nil then
        ns.Print(L.SUFFIX_TOO_OLD)
        return
    end
    local K = ns.Reserve.EstimateRows(chatFrame, ns.Glossary.VisibleText(text), target)
    local instant = ns.ChatButton.InstantLines(lineID, text)
    local n = ns.ChatButton.SetSuffix(lineID, Suffix(slot, K, instant, false))
    if n == 0 then
        counters.tooOld = counters.tooOld + 1
        ns.Reserve.FreeSlot(slot)
        ns.Print(L.SUFFIX_TOO_OLD)
        return
    end
    deferredRow[lineID] = nil

    a = { reqId = NextReqId(), slot = slot, K = K, target = target, flags = Flags(shift), instant = instant,
        lane = "pending" }
    active[lineID] = a
    byReq[a.reqId] = lineID
    count = count + 1
    counters.sent = counters.sent + 1
    SendTranslate(a, marked, glossPairs)
    ns.Reserve.StartWatch()
    C_Timer.After(ns.Reserve.LANE_DELAY, function()
        RevealLane(lineID, a)
    end)
    if not ns.db.helpShown then
        ns.db.helpShown = true
        ns.Print(L.FIRST_USE)
    end
end

-- Rows removed by the user (a click on a row): the helper drops the card.
function Request.Remove(lineID)
    ns.ChatButton.SetSuffix(lineID, nil)
    ns.Restriction.Defer(lineID, nil)
    deferredRow[lineID] = nil
    local a = Forget(lineID)
    if a then
        counters.removed = counters.removed + 1
        ns.Bridge.Send(ns.Bridge.CancelMessage(a.reqId, a.slot))
        ns.Reserve.FreeSlot(a.slot)
    end
end

-- Only when all 127 slots are held by lines still in the chat: the oldest request goes.
function Request.ExpireOldest()
    local oldestLine, oldestReq
    for lineID, a in pairs(active) do
        if not oldestReq or a.reqId < oldestReq then
            oldestLine, oldestReq = lineID, a.reqId
        end
    end
    if oldestLine ~= nil then
        counters.expired = counters.expired + 1
        Request.Remove(oldestLine)
    end
end

-- The line left every chat frame (its slot was freed by the read-only scan).
function Request.Drop(lineID)
    local a = Forget(lineID)
    if a then
        counters.dropped = counters.dropped + 1
        ns.Bridge.Send(ns.Bridge.CancelMessage(a.reqId, a.slot))
    end
end

-- /tpr clear: every row of this session is removed and the helper forgets every card, the
-- conversation window's included (its conversations are cleared too).
function Request.ClearAll()
    counters.cleared = counters.cleared + 1
    ns.ChatButton.CutRows(function(session)
        return session == ns.session
    end)
    ns.Restriction.ClearDeferred()
    wipe(deferredRow)
    ns.Bridge.Send(ns.Bridge.ClearMessage())
    for lineID, a in pairs(active) do
        active[lineID] = nil
        ns.Reserve.FreeSlot(a.slot)
    end
    wipe(byReq)
    count = 0
    if ns.Conv and ns.Conv.Clear then
        ns.Conv.Clear(true)
    end
    ns.Reserve.MaybeStopWatch()
end

---------------------------------------------------------------------------- window bubbles
-- TRANSLATE for a bubble of the conversation window: msg = { text, K, req? } (Conversations.lua).
-- Takes a window slot when the bubble has none (nil when none is free: the window frees one and
-- tries again after the quarantine), then sends the request with the plate, the glossary pairs
-- of the line and the saved target, a VIEW first. A bubble that already has a request sends it
-- again with a new reqId (new rows, Shift-click, or a slot given back). Returns true when sent.
function Request.StartWindow(conv, msg, noCache)
    if type(msg) ~= "table" or type(msg.text) ~= "string" then
        return false
    end
    local req = msg.req
    if not req then
        local slot = ns.Reserve.AllocWindowSlot({ kind = "win", conv = conv, msg = msg })
        if slot == nil then
            return false
        end
        req = { slot = slot }
        msg.req = req
    end
    local target = ns.db.target
    req.reqId = NextReqId()
    req.K = math.max(1, math.min(ns.Reserve.MAX_ROWS, math.floor(tonumber(msg.K) or 1)))
    req.target = target
    req.flags = (noCache and FLAG_NO_CACHE or 0) + FLAG_PLATE
    req.at = GetTime()
    counters.windowSent = counters.windowSent + 1
    SendTranslate(req, ns.Glossary.PromptText(msg.text), ns.Glossary.Extract(msg.text, target))
    return true
end

-- The bubble gives its slot back: CANCEL, and the slot waits in the quarantine.
function Request.DropWindow(msg)
    local req = type(msg) == "table" and msg.req
    if not req then
        return
    end
    msg.req = nil
    counters.windowDropped = counters.windowDropped + 1
    if req.reqId then
        ns.Bridge.Send(ns.Bridge.CancelMessage(req.reqId, req.slot))
    end
    ns.Reserve.FreeWindowSlot(req.slot)
end

function Request.Count()
    return count
end

-- reqId and slot of a line's request, for tests and /tpr debug.
function Request.Get(lineID)
    local a = active[lineID]
    if a then
        return a.reqId, a.slot, a.K, a.flags, a.target
    end
end

-- "pending" (spaces) or "shown" (the hint) for a line's hint lane, nil without a request (tests).
function Request.LaneState(lineID)
    local a = active[lineID]
    return a and a.lane or nil
end

function Request.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    c.active = count
    return c
end
