-- Records, for every incoming player chat line, whether its text is readable by an addon and
-- in which context (instance type, boss fight, PvP match phase, active restrictions). It never
-- stores message text or sender names. After a lockdown it tries to read the secret lines
-- again by lineID, to learn whether a delayed translation would be possible.
local _, ns = ...
local L = ns.L

local LockdownLog = {}
ns.LockdownLog = LockdownLog

local MAX_ROWS, MAX_TRANSITIONS, MAX_PENDING, MAX_CATCHUP = 400, 200, 200, 50
local CATCHUP_DELAYS = { { tag = "1s", delay = 1 }, { tag = "30s", delay = 30 } }

local SHORT_EVENT = {}
for _, event in ipairs(ns.ChatButton.EVENTS) do
    SHORT_EVENT[event] = (event:gsub("^CHAT_MSG_", ""))
end

-- Letters for the active restriction types, in Enum.AddOnRestrictionType order.
local RESTRICTION_LETTERS = { [0] = "c", [1] = "e", [2] = "k", [3] = "p", [4] = "m", [5] = "t" }
local VERBOSE_TYPES = { Encounter = true, PvPMatch = true, Map = true, Chat = true }

local pending, pendingIDs, dropped = {}, {}, 0  -- secret lines waiting for a catch-up, oldest first
local generation = 0                          -- bumped by Reset so old timers do nothing
local announced = {}                          -- restriction type -> last printed on/off
local playerGUID
local frame = CreateFrame("Frame")

local function EnumName(enum, value)
    for name, v in pairs(enum or {}) do
        if v == value then
            return name
        end
    end
    return tostring(value)
end

function LockdownLog.ActiveRestrictions()
    local letters = {}
    for value = 0, 5 do
        if C_RestrictedActions.IsAddOnRestrictionActive(value) then
            letters[#letters + 1] = RESTRICTION_LETTERS[value]
        end
    end
    return table.concat(letters)
end

local function EncounterInProgress()
    if not (C_InstanceEncounter and C_InstanceEncounter.IsEncounterInProgress) then
        return false
    end
    local ok, active = pcall(C_InstanceEncounter.IsEncounterInProgress)
    return ok and not issecretvalue(active) and active == true
end

-- "waiting", "engaged", "complete"... while a PvP match exists, nil otherwise.
local function MatchPhase()
    if not (C_PvP and C_PvP.GetActiveMatchState and Enum.PvPMatchState) then
        return nil
    end
    local ok, state = pcall(C_PvP.GetActiveMatchState)
    if not ok or issecretvalue(state) or state == nil or state == Enum.PvPMatchState.Inactive then
        return nil
    end
    return EnumName(Enum.PvPMatchState, state):lower()
end

-- Context key such as "none", "party", "raid+boss" or "pvp+engaged", read from the client
-- each time so it stays right after a /reload or with logging off.
function LockdownLog.Context()
    local _, instanceType = GetInstanceInfo()
    local key = (not issecretvalue(instanceType) and instanceType) or "none"
    if EncounterInProgress() then
        key = key .. "+boss"
    end
    local phase = MatchPhase()
    if phase then
        key = key .. "+" .. phase
    end
    return key
end
local Context = LockdownLog.Context

local function Pend(item)
    if pendingIDs[item.id] then
        return
    end
    pending[#pending + 1] = item
    pendingIDs[item.id] = true
    if #pending > MAX_PENDING then
        local oldest = table.remove(pending, 1)
        pendingIDs[oldest.id] = nil
        dropped = dropped + 1
    end
end

local function OnChat(event, text, ...)
    local lineID, guid = select(10, ...), select(11, ...)
    local secret = issecretvalue(text)
    local context = Context()
    local stats = ns.db.stats[context] or { readable = 0, secret = 0, own = 0 }
    stats.own = stats.own or 0
    ns.db.stats[context] = stats

    -- Our own lines are not what a translator handles (only identifiable while readable).
    if not secret and not issecretvalue(guid) and guid == playerGUID then
        stats.own = stats.own + 1
        return
    end
    if secret then
        stats.secret = stats.secret + 1
    else
        stats.readable = stats.readable + 1
    end

    ns.PushCapped(ns.db.log, {
        t = time(), e = SHORT_EVENT[event] or event, s = secret and 1 or 0,
        l = C_ChatInfo.InChatMessagingLockdown() and 1 or 0, r = LockdownLog.ActiveRestrictions(), c = context,
    }, MAX_ROWS)

    if secret and type(lineID) == "number" and not issecretvalue(lineID) and lineID ~= 0 then
        Pend({ id = lineID, c = context, t = time() })
    end
end

-- "readable", "secret" or "gone" for one line, never raising.
local function ReadBack(lineID)
    local okValid, valid = pcall(C_ChatInfo.IsValidChatLine, lineID)
    if okValid and not issecretvalue(valid) and not valid then
        return "gone"
    end
    local okText, text = pcall(C_ChatInfo.GetChatLineText, lineID)
    if not okText then
        return "gone"
    end
    if issecretvalue(text) then
        return "secret"
    end
    if type(text) == "string" and text ~= "" then
        return "readable"
    end
    return "gone"
end

local function PrintCatchup(row)
    if row.locked then
        ns.Printf(L.CATCHUP_LOCKED, row.tag, row.tried)
        return
    end
    ns.Printf(L.CATCHUP_LINE, row.tag, row.tried, row.dropped or 0, row.readable, row.secret, row.gone,
        row.minAge or 0, row.maxAge or 0)
    for context, c in pairs(row.byContext or {}) do
        ns.Printf(L.CATCHUP_CONTEXT, context, c.readable, c.secret, c.gone)
    end
end

-- Re-reads one batch of lines that arrived secret. Counts only, never the text. A batch that
-- finds chat locked again before its first read goes back to the queue; once read, later
-- attempts only note that chat was locked.
local function TryCatchup(tag, batch)
    if batch.gen ~= generation or batch.requeued then
        return
    end
    if C_ChatInfo.InChatMessagingLockdown() then
        if batch.measured then
            local row = { tag = tag, t = time(), locked = true, tried = #batch.items }
            ns.PushCapped(ns.db.catchup, row, MAX_CATCHUP)
            PrintCatchup(row)
        else
            batch.requeued = true
            for _, item in ipairs(batch.items) do
                Pend(item)
            end
        end
        return
    end
    batch.measured = true
    local now = time()
    local row = { tag = tag, t = now, tried = #batch.items, dropped = batch.dropped,
        readable = 0, secret = 0, gone = 0, byContext = {} }
    for _, item in ipairs(batch.items) do
        local outcome = ReadBack(item.id)
        row[outcome] = row[outcome] + 1
        local c = row.byContext[item.c] or { readable = 0, secret = 0, gone = 0 }
        row.byContext[item.c] = c
        c[outcome] = c[outcome] + 1
        local age = now - item.t
        row.minAge = math.min(row.minAge or age, age)
        row.maxAge = math.max(row.maxAge or age, age)
    end
    ns.PushCapped(ns.db.catchup, row, MAX_CATCHUP)
    PrintCatchup(row)
end

local function ScheduleCatchup()
    if #pending == 0 then
        return
    end
    local batch = { items = pending, dropped = dropped, gen = generation }
    pending, dropped = {}, 0
    wipe(pendingIDs)
    for _, step in ipairs(CATCHUP_DELAYS) do
        C_Timer.After(step.delay, function()
            TryCatchup(step.tag, batch)
        end)
    end
end

-- The client fires this with state Activating just before a restriction starts and with
-- Inactive just after it ends (RestrictedActionsDocumentation).
local function OnRestrictionChanged(restrictionType, state)
    local typeName = EnumName(Enum.AddOnRestrictionType, restrictionType)
    local stateName = EnumName(Enum.AddOnRestrictionState, state)
    local context = Context()
    ns.PushCapped(ns.db.transitions, {
        t = time(), type = typeName, state = stateName, c = context, zone = GetRealZoneText(),
    }, MAX_TRANSITIONS)
    local on = state ~= Enum.AddOnRestrictionState.Inactive
    if VERBOSE_TYPES[typeName] and announced[typeName] ~= on then
        announced[typeName] = on
        ns.Printf(on and L.RESTRICTION_ON or L.RESTRICTION_OFF, typeName, context)
    end
    if not on then
        ScheduleCatchup()
    end
end

frame:SetScript("OnEvent", function(_, event, ...)
    if SHORT_EVENT[event] then
        OnChat(event, ...)
    elseif event == "ADDON_RESTRICTION_STATE_CHANGED" then
        OnRestrictionChanged(...)
    end
end)

function LockdownLog.SetEnabled(enabled)
    ns.db.logging = enabled
    if enabled then
        for _, event in ipairs(ns.ChatButton.EVENTS) do
            frame:RegisterEvent(event)
        end
        frame:RegisterEvent("ADDON_RESTRICTION_STATE_CHANGED")
    else
        frame:UnregisterAllEvents()
    end
end

-- Manual catch-up, for when no restriction change is announced (a forced CVar test).
function LockdownLog.CatchupNow()
    if #pending == 0 then
        ns.Print(L.CATCHUP_NONE)
        return
    end
    ns.Printf(L.CATCHUP_SCHEDULED, #pending)
    ScheduleCatchup()
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
            ns.Printf(L.REPORT_CONTEXT, context, s.readable, s.secret, s.own or 0)
        end
    end
    ns.Printf(L.REPORT_TRANSITIONS, #ns.db.transitions)
    for _, row in ipairs(ns.db.catchup) do
        PrintCatchup(row)
    end
end

function LockdownLog.Reset()
    generation = generation + 1
    wipe(ns.db.log)
    wipe(ns.db.stats)
    wipe(ns.db.transitions)
    wipe(ns.db.catchup)
    wipe(pending)
    wipe(pendingIDs)
    dropped = 0
end

ns.OnLoad(function()
    playerGUID = UnitGUID("player")
    LockdownLog.SetEnabled(ns.db.logging)
end)
