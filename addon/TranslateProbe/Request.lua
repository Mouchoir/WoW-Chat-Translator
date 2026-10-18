-- What a [T] click does: reserve rows under the line and send a TRANSLATE request over the
-- pixel bridge. The helper does the rest (queue, cache, model, drawing); nothing ever comes
-- back to Lua, so a request lives here until its rows are removed. State is memory only.
local _, ns = ...
local L = ns.L

local Request = {}
ns.Request = Request

local DEBOUNCE = 1.0
local FLAG_NO_CACHE, FLAG_PLATE = 1, 2

local active = {}       -- lineID -> { reqId, slot, K, target, flags }
local byReq = {}        -- reqId -> lineID
local lastClick = {}    -- lineID -> GetTime() of the last accepted click
local lastClickCount = 0
local deferredRow = {}  -- lineID -> true while its TR_DEFERRED row is shown
local nextReq = 0
local count = 0
local counters = {
    clicks = 0, debounced = 0, sent = 0, resent = 0, restarted = 0, deferred = 0, tooOld = 0,
    removed = 0, expired = 0, dropped = 0, cleared = 0,
}

-- 1..65535, wraps to 1.
local function NextReqId()
    nextReq = nextReq % 65535 + 1
    return nextReq
end

local function Flags(shift)
    return (shift and FLAG_NO_CACHE or 0) + (ns.Reserve.PLATE and FLAG_PLATE or 0)
end

local function SendTranslate(a, marked, glossPairs)
    ns.Bridge.Send(ns.Bridge.TranslateMessage(a.reqId, a.slot, a.K, a.flags, a.target, marked, glossPairs))
end

local function Forget(lineID)
    local a = active[lineID]
    if a then
        active[lineID] = nil
        byReq[a.reqId] = nil
        count = count - 1
        if count == 0 then
            ns.Reserve.StopWatch()
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
    local n = ns.ChatButton.SetSuffix(lineID, function(frame)
        local m = ns.Reserve.Metrics(frame)
        if not (m and m.ok) then
            return nil
        end
        local rows = ns.Reserve.Rows(m, slot, K)
        for _, line in ipairs(instant) do
            rows[#rows + 1] = ns.ChatButton.Safe(line)
        end
        return rows
    end)
    if n == 0 then
        counters.tooOld = counters.tooOld + 1
        ns.Reserve.FreeSlot(slot)
        ns.Print(L.SUFFIX_TOO_OLD)
        return
    end
    deferredRow[lineID] = nil

    ns.Reserve.SyncView(false)    -- first send of the session, or the chat moved
    a = { reqId = NextReqId(), slot = slot, K = K, target = target, flags = Flags(shift) }
    active[lineID] = a
    byReq[a.reqId] = lineID
    count = count + 1
    counters.sent = counters.sent + 1
    SendTranslate(a, marked, glossPairs)
    ns.Reserve.StartWatch()
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

-- /tpr clear: every row of this session is removed and the helper forgets every card.
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
    ns.Reserve.StopWatch()
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

function Request.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    c.active = count
    return c
end
