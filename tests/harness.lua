-- Mock WoW Forever client for TranslateProbe. Runs the real addon files in fengari (Lua 5.3).
-- The mock follows the Forever 1.60.1 UI source where it matters: filters are skipped when
-- arg1 is secret, history entries carry (event, eventArgs) with lineID at eventArgs[11], a
-- transform that errors leaves a hole in the history buffer, lines moved to a new window lose
-- their eventArgs, and ADDON_RESTRICTION_STATE_CHANGED arrives as Activating / Inactive.
local rawprint = print
unpack = table.unpack

TEST_FAILURES = 0
local function check(cond, label)
    if cond then
        rawprint("ok   " .. label)
    else
        TEST_FAILURES = TEST_FAILURES + 1
        rawprint("FAIL " .. label)
    end
end

---------------------------------------------------------------------------------------------
-- Lua helpers the client adds
---------------------------------------------------------------------------------------------
local printed = {}
print = function(...)
    local parts = {}
    for i = 1, select("#", ...) do
        parts[i] = tostring(select(i, ...))
    end
    local line = table.concat(parts, " ")
    printed[#printed + 1] = line
    rawprint("  | " .. line)
end
local function Printed(fragment)
    for i = #printed, 1, -1 do
        if printed[i]:find(fragment, 1, true) then
            return true
        end
    end
    return false
end
local function CountPrinted(fragment)
    local n = 0
    for _, line in ipairs(printed) do
        if line:find(fragment, 1, true) then
            n = n + 1
        end
    end
    return n
end

wipe = function(t)
    for k in pairs(t) do
        t[k] = nil
    end
    return t
end
table.create = function()
    return {}
end
time = os.time
date = os.date
debugprofilestop = function()
    return os.clock() * 1000
end
debugstack = function()
    return "stack"
end
local errors = {}
geterrorhandler = function()
    return function(e)
        errors[#errors + 1] = tostring(e)
        rawprint("ERRORHANDLER: " .. tostring(e))
    end
end

-- Secret values: any use other than issecretvalue/concatenation raises, like on the client.
local SecretMT = {}
local function Secret()
    return setmetatable({}, SecretMT)
end
SecretMT.__concat = function()
    return Secret()
end
SecretMT.__len = function()
    error("attempt to get length of a secret value")
end
SecretMT.__index = function()
    error("attempt to index a secret value")
end
SecretMT.__lt = function()
    error("attempt to compare a secret value")
end
SecretMT.__le = SecretMT.__lt
issecretvalue = function(v)
    return getmetatable(v) == SecretMT
end
canaccessvalue = function(v)
    return not issecretvalue(v)
end

---------------------------------------------------------------------------------------------
-- Client state
---------------------------------------------------------------------------------------------
local gameTime = 0
local lockdown = false
local inCombat = false
local instanceType = "none"
local encounter = false
local matchState = 0
local bucket = {
    luaScriptBucketThrottleMaxMsPerSecondNormal = 500, luaScriptBucketThrottleMaxMsBurstNormal = 1000,
    luaScriptBucketThrottleMaxMsPerSecondRestricted = 500, luaScriptBucketThrottleMaxMsBurstRestricted = 1000,
}

GetLocale = function()
    return LOCALE_UNDER_TEST
end
GetFramerate = function()
    return 60
end
InCombatLockdown = function()
    return inCombat
end
IsShiftKeyDown = function()
    return false
end
UnitGUID = function()
    return "Player-1-0001"
end
GetInstanceInfo = function()
    return "Somewhere", instanceType
end
GetRealZoneText = function()
    return "Stormwind"
end
GetBuildInfo = function()
    return "1.60.1", "70338", "Oct 10 2026", 16001
end
WOW_PROJECT_ID = 18
GetScriptBucketThrottleLimits = function()
    return bucket
end
Enum = {
    AddOnRestrictionType = { Combat = 0, Encounter = 1, ChallengeMode = 2, PvPMatch = 3, Map = 4, Chat = 5 },
    AddOnRestrictionState = { Inactive = 0, Activating = 1, Active = 2 },
    AddOnProfilerMetric = { RecentAverageTime = 1, PeakTime = 4, CountTimeOver5Ms = 6 },
    PvPMatchState = { Inactive = 0, Waiting = 1, StartUp = 2, Engaged = 3, PostRound = 4, Complete = 5 },
}
C_RestrictedActions = {
    IsAddOnRestrictionActive = function(t)
        return t == 5 and lockdown
    end,
}
C_InstanceEncounter = {
    IsEncounterInProgress = function()
        return encounter
    end,
}
C_PvP = {
    GetActiveMatchState = function()
        return matchState
    end,
}
-- fengari has no collectgarbage("count"): report a fixed allocation, the test only checks
-- that the bench reads the fields.
C_AddOnProfiler = {
    IsEnabled = function()
        return true
    end,
    GetAddOnMetric = function()
        return 0
    end,
    MeasureCall = function(fn, ...)
        local t0 = os.clock()
        local returns = table.pack(fn(...))
        return { elapsedMilliseconds = (os.clock() - t0) * 1000, allocatedBytes = 1048576, deallocatedBytes = 0 },
            table.unpack(returns, 1, returns.n)
    end,
}
UpdateAddOnMemoryUsage = function() end
GetAddOnMemoryUsage = function()
    return 20480
end
SlashCmdList = {}

---------------------------------------------------------------------------------------------
-- Frames, events, timers
---------------------------------------------------------------------------------------------
local frames = {}
local function NewFrame()
    local f = { scripts = {}, events = {}, shown = true }
    function f:RegisterEvent(e) self.events[e] = true end
    function f:UnregisterEvent(e) self.events[e] = nil end
    function f:UnregisterAllEvents() self.events = {} end
    function f:SetScript(k, fn) self.scripts[k] = fn end
    function f:Show() self.shown = true end
    function f:Hide() self.shown = false end
    frames[#frames + 1] = f
    return f
end
CreateFrame = function()
    return NewFrame()
end
local function FireEvent(event, ...)
    for _, f in ipairs(frames) do
        if f.events[event] and f.scripts.OnEvent then
            f.scripts.OnEvent(f, event, ...)
        end
    end
end

local timers = {}
C_Timer = {
    After = function(delay, fn)
        timers[#timers + 1] = { at = gameTime + delay, fn = fn }
    end,
}
local function Tick(n)
    for _ = 1, n or 1 do
        gameTime = gameTime + 1 / 60
        local due, keep = {}, {}
        for _, t in ipairs(timers) do
            if t.at <= gameTime then due[#due + 1] = t else keep[#keep + 1] = t end
        end
        timers = keep
        for _, t in ipairs(due) do
            t.fn()
        end
        for _, f in ipairs(frames) do
            if f.shown and f.scripts.OnUpdate then
                f.scripts.OnUpdate(f, 1 / 60)
            end
        end
    end
end
local function TickUntil(cond, max)
    for i = 1, max do
        Tick(1)
        if cond() then
            return true, i
        end
    end
    return false, max
end

local callbacks = {}
EventRegistry = {
    RegisterCallback = function(_, event, fn, owner)
        callbacks[event] = callbacks[event] or {}
        callbacks[event][owner or fn] = fn
    end,
    TriggerEvent = function(_, event, ...)
        for owner, fn in pairs(callbacks[event] or {}) do
            fn(owner, ...)
        end
    end,
}

---------------------------------------------------------------------------------------------
-- Chat frames and the chat pipeline
---------------------------------------------------------------------------------------------
CHAT_FRAMES = { "ChatFrame1", "ChatFrame3" }
local function NewChatFrame(name)
    local cf = NewFrame()
    cf.name, cf.history, cf.holes = name, {}, 0
    function cf:AddMessage(message, r, g, b, ...)
        self.history[#self.history + 1] = { message = message, r = r, g = g, b = b, extra = table.pack(...) }
    end
    local function Unpack(e)
        return e.message, e.r, e.g, e.b, table.unpack(e.extra, 1, e.extra.n)
    end
    function cf:TransformMessages(predicate, transform)
        for i = 1, #self.history do
            local e = self.history[i]
            if e then
                local okP, match = pcall(predicate, Unpack(e))
                if okP and match then
                    local res = table.pack(pcall(transform, Unpack(e)))
                    if res[1] then
                        self.history[i] = { message = res[2], r = res[3], g = res[4], b = res[5],
                            extra = table.pack(table.unpack(res, 6, res.n)) }
                    else
                        self.history[i] = false
                        self.holes = self.holes + 1
                        rawprint("TRANSFORM ERROR: " .. tostring(res[2]))
                    end
                elseif not okP then
                    rawprint("PREDICATE ERROR: " .. tostring(match))
                end
            end
        end
    end
    _G[name] = cf
    return cf
end
NewChatFrame("ChatFrame1")
NewChatFrame("ChatFrame3")

local filters = {}
ChatFrameUtil = {
    AddMessageEventFilter = function(event, fn)
        filters[event] = filters[event] or {}
        table.insert(filters[event], fn)
    end,
    ForEachChatFrame = function(fn)
        for _, name in ipairs(CHAT_FRAMES) do
            fn(_G[name])
        end
    end,
}

local nextLineID, lineStore = 100, {}
C_ChatInfo = {
    InChatMessagingLockdown = function()
        return lockdown
    end,
    GetChatLineText = function(id)
        if lockdown then
            return Secret()
        end
        return lineStore[id]
    end,
    IsValidChatLine = function(id)
        return lineStore[id] ~= nil
    end,
    IsChatLineCensored = function()
        return false
    end,
}

-- Delivers one chat event the way ChatFrameMixin:MessageEventHandler does, then to addon
-- frames. hidden = true runs the filters but no frame shows the line (a channel no tab shows).
local function Deliver(event, text, sender, guid, hidden)
    nextLineID = nextLineID + 1
    local lineID = nextLineID
    lineStore[lineID] = text
    local secret = lockdown
    local args = table.pack(secret and Secret() or text, secret and Secret() or sender, "", "", "", "", 0, 0, "", 0,
        lineID, secret and Secret() or (guid or "Player-1-0002"), 0, false)
    for _, name in ipairs(CHAT_FRAMES) do
        local cf = _G[name]
        local a, discard = table.pack(table.unpack(args, 1, args.n)), false
        for _, fn in ipairs(filters[event] or {}) do
            if canaccessvalue(a[1]) then
                local res = table.pack(pcall(fn, cf, event, table.unpack(a, 1, a.n)))
                if not res[1] then
                    errors[#errors + 1] = tostring(res[2])
                    rawprint("FILTER ERROR: " .. tostring(res[2]))
                elseif res[2] then
                    discard = true
                    break
                elseif res[3] then
                    a = table.pack(table.unpack(res, 3, res.n))
                end
            end
        end
        if not discard and not hidden then
            local msg = secret and Secret() or ("[" .. sender .. "]: " .. a[1])
            cf:AddMessage(msg, 1, 1, 1, 1, 0, 0, event, args, function(t) return t end)
        end
    end
    FireEvent(event, table.unpack(args, 1, args.n))
    return lineID
end

local function EntryFor(cf, lineID)
    for _, e in ipairs(cf.history) do
        if e and e.extra[5] and e.extra[5][11] == lineID then
            return e
        end
    end
end
local function LinksIn(message)
    local links = {}
    for link in message:gmatch("|H(addon:[^|]+)|h") do
        links[#links + 1] = link
    end
    return links
end
local function Click(link)
    EventRegistry:TriggerEvent("SetItemRef", link, "[T]", "LeftButton", ChatFrame1)
end
local function Lines(message)
    return select(2, message:gsub("|n", ""))
end

---------------------------------------------------------------------------------------------
-- Load the addon
---------------------------------------------------------------------------------------------
local ns = {}
for file in ADDON_FILES:gmatch("[^,]+") do
    local chunk = assert(load(_G["SRC_" .. file], "@" .. file))
    chunk("TranslateProbe", ns)
end
FireEvent("ADDON_LOADED", "TranslateProbe")
local L = ns.L
local slash = SlashCmdList.TRANSLATEPROBE
local fr = LOCALE_UNDER_TEST == "frFR"

ns.Model.Configure({ D = 32, H = 4, FF = 64, V = 500, SL = 20, ENC = 2, DEC = 1, POOL_KB = 8, ROWS_PER_BLOCK = 64 })
ns.Bench.targetMs = 20
local builds = 0
local realBuild = ns.Model.Build
ns.Model.Build = function(...)
    builds = builds + 1
    return realBuild(...)
end

rawprint("== load (" .. LOCALE_UNDER_TEST .. ")")
check(Printed(L.LOADED), "prints the loaded line")
check(TranslateProbeDB and TranslateProbeDB.budgetMs == 4 and TranslateProbeDB.logging == true
    and TranslateProbeDB.simulate == true, "saved variables get defaults")
check(not fr or L.LOADED:find("Tape", 1, true), "French strings on frFR")
check(SLASH_TRANSLATEPROBE1 == "/tprobe" and slash ~= nil, "slash command registered")

rawprint("== button on readable lines")
local readable = Deliver("CHAT_MSG_SAY", "hallo zusammen, wer will mit in die Todesminen", "Hans")
Tick(1)
local e1, e3 = EntryFor(ChatFrame1, readable), EntryFor(ChatFrame3, readable)
check(e1 and e1.message:find("|Haddon:TranslateProbe:t:" .. ns.session .. ":" .. readable .. "|h[T]|h", 1, true),
    "button appended in ChatFrame1")
check(e3 and #LinksIn(e3.message) == 1, "button appended once in ChatFrame3")
check(e1 and e1.message:find("^%[Hans%]: hallo zusammen"), "original text kept at the start")
Tick(3)
check(#LinksIn(EntryFor(ChatFrame1, readable).message) == 1, "no second button on later frames")

local own = Deliver("CHAT_MSG_SAY", "my own line", "Me", "Player-1-0001")
Tick(1)
check(#LinksIn(EntryFor(ChatFrame1, own).message) == 0, "no button on my own messages")
check(TranslateProbeDB.stats.none.own == 1 and TranslateProbeDB.stats.none.readable == 1, "own lines logged apart")

local hidden = Deliver("CHAT_MSG_CHANNEL", "a line no tab shows", "Tom", nil, true)
Tick(1)
check(not ns.ChatButton.IsTracked(hidden) and ns.ChatButton.IsTracked(readable), "lines no frame shows are not tracked")

rawprint("== chat lockdown")
instanceType = "raid"
encounter = true
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 1)
check(Printed(L.RESTRICTION_ON:format("Chat", "raid+boss")), "activation is announced with its context")
lockdown = true
local locked = Deliver("CHAT_MSG_RAID", "pull in 5", "Raidlead")
Tick(1)
local lockedEntry = EntryFor(ChatFrame1, locked)
check(lockedEntry and issecretvalue(lockedEntry.message), "secret line displayed untouched")
check(TranslateProbeDB.stats["raid+boss"] and TranslateProbeDB.stats["raid+boss"].secret == 1, "secret line counted in raid+boss")
local lastRow = TranslateProbeDB.log[#TranslateProbeDB.log]
check(lastRow.s == 1 and lastRow.r == "t" and lastRow.c == "raid+boss", "log row records secret, chat restriction, context")

-- The encounter ends while chat stays locked (map still restricted): the 1 s attempt requeues.
encounter = false
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 1, 0)
Tick(70)
check(#TranslateProbeDB.catchup == 0, "no catch-up row while chat is still locked")

lockdown = false
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 0)
check(Printed(L.RESTRICTION_OFF:format("Chat", "raid")), "deactivation is announced")
Tick(70)
local catch = TranslateProbeDB.catchup[1]
check(catch and catch.tag == "1s" and catch.tried == 1 and catch.readable == 1, "catch-up re-reads the requeued line")
check(catch and catch.byContext["raid+boss"] and catch.byContext["raid+boss"].readable == 1, "catch-up keeps the original context")

-- Chat locks again before the 30 s attempt: it notes the lock instead of requeueing.
lockdown = true
Tick(60 * 30)
local second = TranslateProbeDB.catchup[2]
check(second and second.tag == "30s" and second.locked, "30 s attempt during a new lock is noted, not requeued")
lockdown = false
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 0)
Tick(60 * 31)
check(#TranslateProbeDB.catchup == 2, "measured lines are never counted twice")

-- A reset cancels pending catch-ups.
lockdown = true
Deliver("CHAT_MSG_RAID", "another secret", "Raidlead")
lockdown = false
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 0)
slash("reset")
Tick(60 * 31)
check(#TranslateProbeDB.catchup == 0, "reset cancels scheduled catch-ups")
instanceType = "none"

rawprint("== click: glossary and simulated model under the line")
local button = LinksIn(EntryFor(ChatFrame1, readable).message)[1]
Click(button)
local building = EntryFor(ChatFrame1, readable).message
check(Lines(building) == 2 and building:find(L.SUFFIX_BUILDING:sub(1, 12), 1, true), "first click says the model is being built")
local done = TickUntil(function()
    local e = EntryFor(ChatFrame1, readable)
    return e and e.message:find(L.SUFFIX_RESULT:sub(1, 12), 1, true) and not ns.Scheduler.IsBusy()
end, 5000)
check(done, "result replaces the placeholder")
local finished = EntryFor(ChatFrame1, readable).message
check(Lines(finished) == 2, "two lines under the message: glossary + timing")
check(finished:find(L.SUFFIX_NO_GLOSS, 1, true), "glossary line says nothing known for a German sentence")
check(EntryFor(ChatFrame3, readable).message == finished, "same result in every chat frame")
local click = TranslateProbeDB.results.clicks[1]
check(click and click.macs == ns.Model.EstimateMacs(click.src, click.tgt) and click.context == "none",
    "click row times only the forward pass, with its context")
check(builds == 1, "model built once")
check(ChatFrame1.holes == 0 and ChatFrame3.holes == 0, "no history holes from transforms")
check(issecretvalue(EntryFor(ChatFrame1, locked).message), "secret entries untouched by transforms")

local suffixLink = LinksIn(finished)[2]
check(suffixLink and suffixLink:find(":x:", 1, true), "result line is a link")
Click(suffixLink)
check(Lines(EntryFor(ChatFrame1, readable).message) == 0, "clicking the result hides it")
Click(button)
check(EntryFor(ChatFrame1, readable).message == finished and not ns.Scheduler.IsBusy(), "clicking [T] again shows the cached result")

Click("addon:TranslateProbe:t:1:" .. readable)
check(Printed(L.OLD_SESSION), "links from an older session are refused")
Click("addon:TranslateProbe:t:" .. ns.session .. ":99999")
check(Printed(L.SUFFIX_TOO_OLD) and not ns.Scheduler.IsBusy(), "a line the client forgot gets a clear message, no job")

-- Blizzard copies lines into a new window without eventArgs and removes the originals.
local moved = Deliver("CHAT_MSG_WHISPER", "LFM DM", "Bob")
Tick(1)
local original = EntryFor(ChatFrame1, moved)
local copy = { message = original.message, r = 1, g = 1, b = 1, extra = table.pack(1, 0, 0) }
for i, e in ipairs(ChatFrame1.history) do
    if e == original then
        table.remove(ChatFrame1.history, i)
        break
    end
end
local e3moved = EntryFor(ChatFrame3, moved)
for i, e in ipairs(ChatFrame3.history) do
    if e == e3moved then
        ChatFrame3.history[i] = copy
        break
    end
end
Click(LinksIn(copy.message)[1])
TickUntil(function() return not ns.Scheduler.IsBusy() end, 5000)
local copied = ChatFrame3.history[#ChatFrame3.history]
for _, e in ipairs(ChatFrame3.history) do
    if e and e.extra.n == 3 and e.message:find("LFM DM", 1, true) then
        copied = e
    end
end
check(Lines(copied.message) == 2 and copied.message:find("DM", 1, true), "a moved line still gets its result")

rawprint("== glossary")
local G = ns.Glossary
local g = G.Describe("LFM dm, need heal. Bgs later? lf2m Stock")
check(g and g:find("LFM", 1, true) and g:find("DM", 1, true) and g:find("BGS", 1, true)
    and g:find("LF2M", 1, true) and g:find("STOCK", 1, true), "abbreviations found whatever the case, plural and number")
check(g and g:find(fr and "Les Mortemines (The Deadmines)" or "The Deadmines", 1, true)
    and g:find(fr and "Hache-Tripes" or "Dire Maul", 1, true), "DM lists Deadmines and Dire Maul in the client language")
check(G.Describe("Lfg") and G.Describe("Lfg"):find("LFG", 1, true), "mixed case matches")
check(G.Describe("|cffffffff|Hitem:1|h[Deadmines Key]|h|r please") == nil, "item links and colors are not scanned")
check(G.Describe("bodmin") == nil and G.Describe("dmg") == nil, "whole words only")
check(G.Describe("son stéréo est abîmé, quelle stratégie, libérez-le, ein Stück") == nil, "accented words are never cut into abbreviations")
check(G.Describe("I need abs") == nil and G.Describe("dms") == nil, "plural only where allowed")
check(G.Describe("strats") ~= nil and G.Describe("hots") ~= nil, "allowed plurals work")
local hot = G.Describe("HoT anyone?")
check(hot and select(2, hot:gsub(" / ", "")) == 2, "HoT shows its three meanings")
check(fr == false or (G.Describe("lfm") or ""):find("LFM : ", 1, true), "French separator on frFR")
slash("gloss add hot Hall of Thanes")
check(G.Describe("hot") == L.GLOSS_PAIR:format("HOT", "Hall of Thanes"), "custom entry replaces a built-in")
slash("gloss del hot")
check(G.Describe("hot") == nil and TranslateProbeDB.glossary.hot == false, "deleting a built-in disables it")
check(G.Describe("hots") == nil, "a disabled built-in also disables its plural")
slash("gloss add hot Hall of Thanes / chaud")
check(G.Describe("HOT") == L.GLOSS_PAIR:format("HOT", "Hall of Thanes / chaud"), "several custom meanings with /")
slash("gloss add zz top|secret")
check(G.Describe("zz") == L.GLOSS_PAIR:format("ZZ", "top/secret"), "pipes in custom text are neutralized")
slash("gloss del zz")
check(TranslateProbeDB.glossary.zz == nil, "deleting a custom entry removes it")
slash("gloss add été summer")
check(G.Describe("en été") == L.GLOSS_PAIR:format("été", "summer"), "accented custom keys work")
slash("gloss list")
check(Printed("HOT = Hall of Thanes / chaud"), "gloss list shows custom entries")
slash("gloss test lfm wsg")
check(Printed("WSG"), "gloss test prints the meanings")

slash("simulate off")
check(TranslateProbeDB.simulate == false, "test mode off")
local lfm = Deliver("CHAT_MSG_CHANNEL", "LFM DM need tank", "Bob")
Tick(1)
Click(LinksIn(EntryFor(ChatFrame1, lfm).message)[1])
local glossOnly = EntryFor(ChatFrame1, lfm).message
check(not ns.Scheduler.IsBusy() and Lines(glossOnly) == 1 and glossOnly:find("DM", 1, true),
    "without test mode a click shows only the glossary, instantly")
slash("simulate on")

rawprint("== scheduler")
bucket.luaScriptBucketThrottleMaxMsPerSecondRestricted = 20
local slice, every = ns.Scheduler.Plan()
check(slice == 0.5 and every == 3, "low per-second allowance skips frames instead of raising the slice")
bucket.luaScriptBucketThrottleMaxMsPerSecondRestricted = 500
slice, every = ns.Scheduler.Plan()
check(math.abs(slice - 4) < 1e-9 and every == 1, "normal allowance keeps the setting")

rawprint("== slash commands")
slash("model 4")
local model = TickUntil(function() return #TranslateProbeDB.results.models == 1 and not ns.Scheduler.IsBusy() end, 5000)
check(model, "model command finishes")
local row = TranslateProbeDB.results.models[1]
check(row and row.macs == ns.Model.EstimateMacs(row.src, row.tgt) and row.tgt == ns.Model.TargetFor(4),
    "forward pass counts exactly the estimated multiply-adds")
check(row and row.maxSliceMs < row.budgetMs + 25, "slices stay near the budget")
check(builds == 1 and not Printed(L.MODEL_BUILD:format(ns.Model.ExpectedBytes() / 1048576)), "no rebuild when the model exists")

inCombat = true
slash("model 3")
Tick(30)
check(#TranslateProbeDB.results.models == 1, "jobs pause in combat")
inCombat = false
check(TickUntil(function() return #TranslateProbeDB.results.models == 2 end, 5000), "jobs resume after combat")
check(TranslateProbeDB.results.models[2].pausedMs > 0, "combat pause recorded apart from the waiting time")

slash("free")
slash("model 2")
slash("model 2")
TickUntil(function() return #TranslateProbeDB.results.models == 4 and not ns.Scheduler.IsBusy() end, 5000)
check(builds == 2 and CountPrinted(L.MODEL_BUILD:format(ns.Model.ExpectedBytes() / 1048576)) == 1,
    "two quick model commands build once")

local failForward = ns.Model.Forward
ns.Model.Forward = function()
    error("boom")
end
local failing = Deliver("CHAT_MSG_SAY", "anything", "Hans")
Tick(1)
Click(LinksIn(EntryFor(ChatFrame1, failing).message)[1])
TickUntil(function() return not ns.Scheduler.IsBusy() end, 100)
check(EntryFor(ChatFrame1, failing).message:find("boom", 1, true), "a failed run shows its error under the line")
ns.Model.Forward = failForward
local errorCount = #errors
wipe(errors)

slash("bench")
check(TickUntil(function() return TranslateProbeDB.results.bench ~= nil end, 20000), "bench finishes")
local bench = TranslateProbeDB.results.bench
check(bench and bench.int8Rate > 0 and bench.floatRate > 0 and bench.byteRate > 0, "bench rates measured")
check(bench and bench.memory and bench.memory.slot > 0 and bench.memory.stringByte > 0, "bench memory measured")
check(bench and bench.maxSliceMs ~= nil, "bench records its longest slice")

slash("info")
check(Printed("1.60.1"), "info prints the client build")
slash("budget 2")
check(TranslateProbeDB.budgetMs == 2, "budget command stores the setting")
slash("budget 0")
check(TranslateProbeDB.budgetMs == 2 and Printed(L.BUDGET_USAGE), "budget command rejects 0")
slash("log off")
check(TranslateProbeDB.logging == false, "log off")
local before = #TranslateProbeDB.log
Deliver("CHAT_MSG_SAY", "not logged", "Hans")
check(#TranslateProbeDB.log == before, "nothing logged while off")
encounter = true
check(ns.LockdownLog.Context() == "none+boss", "context read from the client even with logging off")
encounter = false
slash("log on")
matchState = 3
instanceType = "pvp"
check(ns.LockdownLog.Context() == "pvp+engaged", "battleground match phase in the context")
matchState, instanceType = 0, "none"
slash("report")
check(Printed(L.REPORT_HEADER), "report prints the header")
slash("free")
check(not ns.Model.IsBuilt(), "free releases the model")
slash("reset")
check(#TranslateProbeDB.log == 0 and TranslateProbeDB.results.bench == nil, "reset clears logs and results")
slash("")
check(Printed(L.HELP_CLICK), "help lists the click hint")

check(errorCount == 1 and #errors == 0, "only the forced failure reached the error handler (" .. errorCount .. ", " .. #errors .. ")")
