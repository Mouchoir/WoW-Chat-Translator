-- Records, for every incoming player chat line, whether its text is readable by an addon and
-- in which context (instance type, boss fight, PvP match, active restrictions). It never
-- stores message text or sender names. After a lockdown it also tries to read the secret lines
-- again by lineID, to learn whether a delayed translation would be possible.
local _, ns = ...
local L = ns.L

local LockdownLog = {}
ns.LockdownLog = LockdownLog

local MAX_ROWS, MAX_TRANSITIONS, MAX_PENDING = 400, 200, 200
local CATCHUP_DELAYS = { { tag = "1s", delay = 1 }, { tag = "30s", delay = 30 } }

local SHORT_EVENT = {}
for _, event in ipairs(ns.ChatButton.EVENTS) do
    SHORT_EVENT[event] = event:gsub("^CHAT_MSG_", "")
end

-- Letters for the active restriction types, in Enum.AddOnRestrictionType order.
local RESTRICTION_LETTERS = { [0] = "c", [1] = "e", [2] = "k", [3] = "p", [4] = "m", [5] = "t" }

local encounterActive, matchActive = false, false
local pendingSecret = {}   -- lineIDs received while secret, waiting for a catch-up attempt
local frame = CreateFrame("Frame")

local function RestrictionName(value)
    for name, v in pairs(Enum.AddOnRestrictionType) do
        if v == value then
            return name
        end
    end
    return tostring(value)
end
LockdownLog.RestrictionName = RestrictionName

local function StateName(value)
    for name, v in pairs(Enum.AddOnRestrictionState) do
        if v == value then
            return name
        end
    end
    return tostring(value)
end

local function ActiveRestrictions()
    local letters = {}
    for value = 0, 5 do
        if C_RestrictedActions.IsAddOnRestrictionActive(value) then
            letters[#letters + 1] = RESTRICTION_LETTERS[value]
        end
    end
    return table.concat(letters)
end
LockdownLog.ActiveRestrictions = ActiveRestrictions

local function Context()
    local _, instanceType = GetInstanceInfo()
    local key = instanceType or "none"
    if encounterActive then key = key .. "+boss" end
    if matchActive then key = key .. "+match" end
    return key
end
LockdownLog.Context = Context

local function OnChat(event, text, ...)
    local secret = issecretvalue(text)
    local lineID = select(10, ...)
    local context = Context()
    local lock = C_ChatInfo.InChatMessagingLockdown()

    local stats = ns.db.stats[context] or { readable = 0, secret = 0 }
    ns.db.stats[context] = stats
    if secret then
        stats.secret = stats.secret + 1
    else
        stats.readable = stats.readable + 1
    end

    ns.PushCapped(ns.db.log, {
        t = time(), e = SHORT_EVENT[event] or event, s = secret and 1 or 0, l = lock and 1 or 0,
        r = ActiveRestrictions(), c = context,
    }, MAX_ROWS)

    if secret and type(lineID) == "number" and not issecretvalue(lineID) and #pendingSecret < MAX_PENDING then
        pendingSecret[#pendingSecret + 1] = lineID
    end
end

-- Tries to read back lines that arrived secret. Stores counts only, never the text.
-- If chat is still locked (another restriction ended first), the batch waits for the next end.
local function TryCatchup(tag, batch)
    local lineIDs = batch.lineIDs
    if batch.requeued then
        return
    end
    if C_ChatInfo.InChatMessagingLockdown() then
        batch.requeued = true
        for _, lineID in ipairs(lineIDs) do
            if #pendingSecret < MAX_PENDING then
                pendingSecret[#pendingSecret + 1] = lineID
            end
        end
        return
    end
    local row = { tag = tag, t = time(), tried = #lineIDs, readable = 0, secret = 0, gone = 0, c = Context() }
    for _, lineID in ipairs(lineIDs) do
        local valid = C_ChatInfo.IsValidChatLine(lineID)
        local text = C_ChatInfo.GetChatLineText(lineID)
        if issecretvalue(text) then
            row.secret = row.secret + 1
        elseif type(text) == "string" and (issecretvalue(valid) or valid) then
            row.readable = row.readable + 1
        else
            row.gone = row.gone + 1
        end
    end
    ns.PushCapped(ns.db.catchup, row, 50)
    ns.Printf(L.CATCHUP_LINE, tag, row.tried, row.readable, row.secret, row.gone)
end

local function ScheduleCatchup()
    if #pendingSecret == 0 then
        return
    end
    local batch = { lineIDs = pendingSecret }
    pendingSecret = {}
    for _, step in ipairs(CATCHUP_DELAYS) do
        C_Timer.After(step.delay, function()
            TryCatchup(step.tag, batch)
        end)
    end
end

local VERBOSE_TYPES = { Encounter = true, PvPMatch = true, Map = true, Chat = true }

local function OnRestrictionChanged(restrictionType, state)
    local typeName, stateName = RestrictionName(restrictionType), StateName(state)
    ns.PushCapped(ns.db.transitions, {
        t = time(), type = typeName, state = stateName, c = Context(), zone = GetRealZoneText(),
    }, MAX_TRANSITIONS)
    if VERBOSE_TYPES[typeName] and state ~= Enum.AddOnRestrictionState.Activating then
        ns.Printf(L.RESTRICTION_CHANGE, typeName, stateName, Context())
    end
    if state == Enum.AddOnRestrictionState.Inactive then
        ScheduleCatchup()
    end
end

frame:SetScript("OnEvent", function(_, event, ...)
    if SHORT_EVENT[event] then
        OnChat(event, ...)
    elseif event == "ADDON_RESTRICTION_STATE_CHANGED" then
        OnRestrictionChanged(...)
    elseif event == "ENCOUNTER_START" then
        encounterActive = true
    elseif event == "ENCOUNTER_END" then
        encounterActive = false
    elseif event == "PVP_MATCH_ACTIVE" then
        matchActive = true
    elseif event == "PVP_MATCH_COMPLETE" or event == "PLAYER_ENTERING_WORLD" then
        matchActive = false
        if event == "PLAYER_ENTERING_WORLD" then
            encounterActive = false
        end
    end
end)

local CONTEXT_EVENTS = {
    "ADDON_RESTRICTION_STATE_CHANGED", "ENCOUNTER_START", "ENCOUNTER_END",
    "PVP_MATCH_ACTIVE", "PVP_MATCH_COMPLETE", "PLAYER_ENTERING_WORLD",
}

function LockdownLog.SetEnabled(enabled)
    ns.db.logging = enabled
    if enabled then
        for _, event in ipairs(ns.ChatButton.EVENTS) do
            frame:RegisterEvent(event)
        end
        for _, event in ipairs(CONTEXT_EVENTS) do
            frame:RegisterEvent(event)
        end
    else
        frame:UnregisterAllEvents()
    end
end

function LockdownLog.Report()
    local contexts = {}
    for context in pairs(ns.db.stats) do
        contexts[#contexts + 1] = context
    end
    table.sort(contexts)
    if #contexts == 0 then
        ns.Print(L.REPORT_EMPTY)
    else
        ns.Print(L.REPORT_HEADER)
        for _, context in ipairs(contexts) do
            local s = ns.db.stats[context]
            ns.Printf(L.REPORT_CONTEXT, context, s.readable, s.secret)
        end
    end
    ns.Printf(L.REPORT_TRANSITIONS, #ns.db.transitions)
    for _, row in ipairs(ns.db.catchup) do
        ns.Printf(L.CATCHUP_LINE, row.tag, row.tried, row.readable, row.secret, row.gone)
    end
end

function LockdownLog.Reset()
    wipe(ns.db.log)
    wipe(ns.db.stats)
    wipe(ns.db.transitions)
    wipe(ns.db.catchup)
    wipe(pendingSecret)
end

ns.OnLoad(function()
    LockdownLog.SetEnabled(ns.db.logging)
end)
