-- Chat restrictions (encounters, challenge modes, PvP matches, restricted maps).
--
-- While chat is in messaging lockdown, a line that arrives has secret text: the client skips
-- addon filters, so it gets no [T], and nothing about it can be read. A line that arrived
-- readable is tracked in memory and stays translatable at any time.
-- * A click on a line whose text is not in memory, during lockdown, is deferred: it runs again
--   once the restriction ends (at +1 s, and +30 s if chat is still locked then).
-- * Lines that arrived secret (any secret argument) are re-read by lineID after the restriction
--   ends. When some are still shown by a chat frame and readable again, one summary line offers
--   [Show them], which reprints them with a [T].
-- Nothing here is saved: no text, sender or lineID ever reaches the saved variables.
local addonName, ns = ...
local L = ns.L

local Restriction = {}
ns.Restriction = Restriction

local MAX_SECRET = 20
local RETRY_DELAYS = { 1, 30 }
local INACTIVE = Enum and Enum.AddOnRestrictionState and Enum.AddOnRestrictionState.Inactive or 0

local isChatEvent = {}
for _, event in ipairs(ns.ChatButton.EVENTS) do
    isChatEvent[event] = true
end

local activeTypes = {}          -- restriction type -> true while not Inactive
local secretArrivals = {}       -- { id = lineID, event = event }, oldest first
local deferred = {}             -- lineID -> callback
local deferredOrder = {}        -- lineIDs in the order they were deferred
local batches = {}              -- batchId -> rows read back after a restriction
local nextBatch = 0
local generation = 0            -- bumped on each Inactive: only the latest timers act
local playerGUID
local counters = { secret = 0, dropped = 0, batches = 0, reprinted = 0, deferred = 0, ran = 0 }

local function IsPlainString(v)
    return not issecretvalue(v) and type(v) == "string"
end

-- The client skips message filters whenever any argument of the event is secret (not only the
-- text), so such a line got no [T]: it is offered here once the restriction ends.
local function OnChat(event, text, ...)
    if canaccessvalue(text, ...) then
        return
    end
    local lineID = select(10, ...)
    if issecretvalue(lineID) or type(lineID) ~= "number" or lineID == 0 then
        return
    end
    counters.secret = counters.secret + 1
    secretArrivals[#secretArrivals + 1] = { id = lineID, event = event }
    if #secretArrivals > MAX_SECRET then
        table.remove(secretArrivals, 1)
        counters.dropped = counters.dropped + 1
    end
end

-- { id, event, text, sender } for a line that arrived secret and is readable now, else nil.
local function ReadBack(item)
    local okValid, valid = pcall(C_ChatInfo.IsValidChatLine, item.id)
    if okValid and not issecretvalue(valid) and not valid then
        return nil
    end
    local okText, text = pcall(C_ChatInfo.GetChatLineText, item.id)
    if not okText or not IsPlainString(text) or text == "" or not ns.Glossary.HasWords(text) then
        return nil
    end
    if C_ChatInfo.GetChatLineSenderGUID then
        local okGuid, guid = pcall(C_ChatInfo.GetChatLineSenderGUID, item.id)
        if okGuid and IsPlainString(guid) and guid == playerGUID then
            return nil
        end
    end
    local sender
    -- Battle.net names are protected strings: never touch them.
    if item.event ~= "CHAT_MSG_BN_WHISPER" and C_ChatInfo.GetChatLineSenderName then
        local okName, name = pcall(C_ChatInfo.GetChatLineSenderName, item.id)
        if okName and IsPlainString(name) and name ~= "" then
            sender = name
        end
    end
    return { id = item.id, event = item.event, text = text, sender = sender }
end

local function Catchup(gen)
    if gen ~= generation or C_ChatInfo.InChatMessagingLockdown() then
        return
    end
    -- Each deferred click runs in its own timer callback, so one error cannot stop the others.
    local order = deferredOrder
    deferredOrder = {}
    for _, lineID in ipairs(order) do
        local fn = deferred[lineID]
        deferred[lineID] = nil
        if fn then
            counters.ran = counters.ran + 1
            C_Timer.After(0, fn)
        end
    end
    -- Only lines some chat frame still shows: a channel no tab displays (or a line that already
    -- scrolled out) is never printed into the main chat, as on the readable path.
    local shown = ns.ChatButton.ShownLines()
    local batch = {}
    for _, item in ipairs(secretArrivals) do
        local row = shown[item.id] and ReadBack(item)
        if row then
            batch[#batch + 1] = row
        end
    end
    wipe(secretArrivals)
    if #batch > 0 then
        nextBatch = nextBatch + 1
        batches[nextBatch] = batch
        counters.batches = counters.batches + 1
        local link = "|cff66bbff|Haddon:" .. addonName .. ":r:" .. ns.session .. ":" .. nextBatch .. "|h["
            .. L.LOCKDOWN_SHOW .. "]|h|r"
        ns.Print(L.LOCKDOWN_SUMMARY:format(#batch) .. " " .. link)
    end
end

-- The client fires this with state Activating just before a restriction starts and with
-- Inactive just after it ends (RestrictedActionsDocumentation).
local function OnRestrictionChanged(restrictionType, state)
    if issecretvalue(restrictionType) or issecretvalue(state) then
        return
    end
    if state ~= INACTIVE then
        activeTypes[restrictionType] = true
        return
    end
    activeTypes[restrictionType] = nil
    generation = generation + 1
    local gen = generation
    for _, delay in ipairs(RETRY_DELAYS) do
        C_Timer.After(delay, function()
            Catchup(gen)
        end)
    end
end

local frame = CreateFrame("Frame")
for _, event in ipairs(ns.ChatButton.EVENTS) do
    frame:RegisterEvent(event)
end
frame:RegisterEvent("ADDON_RESTRICTION_STATE_CHANGED")
frame:SetScript("OnEvent", function(_, event, ...)
    if isChatEvent[event] then
        OnChat(event, ...)
    elseif event == "ADDON_RESTRICTION_STATE_CHANGED" then
        OnRestrictionChanged(...)
    end
end)

function Restriction.Active()
    return next(activeTypes) ~= nil
end

-- Runs fn once the restriction ends; fn = nil cancels. One callback per line (the latest).
function Restriction.Defer(lineID, fn)
    if fn == nil then
        deferred[lineID] = nil
        return
    end
    if deferred[lineID] == nil then
        deferredOrder[#deferredOrder + 1] = lineID
    end
    deferred[lineID] = fn
    counters.deferred = counters.deferred + 1
end

function Restriction.ClearDeferred()
    wipe(deferred)
    deferredOrder = {}
end

-- [Show them]: prints the lines read back after the restriction, each with a [T].
function Restriction.Reprint(batchId)
    local batch = batches[batchId]
    if not batch then
        return
    end
    batches[batchId] = nil
    for _, row in ipairs(batch) do
        DEFAULT_CHAT_FRAME:AddMessage(L.REPRINT_LINE:format(row.sender or "?", row.text)
            .. ns.ChatButton.ButtonLink(row.id))
        ns.ChatButton.Track(row.id, row.text, row.sender)
        counters.reprinted = counters.reprinted + 1
    end
end

function Restriction.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    c.active = Restriction.Active()
    c.waiting = #secretArrivals
    local n = 0
    for _ in pairs(deferred) do
        n = n + 1
    end
    c.deferredNow = n
    return c
end

ns.OnLoad(function()
    playerGUID = UnitGUID("player")
end)
