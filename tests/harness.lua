-- Mock WoW Forever client for TranslateProbe. Runs the real addon files in fengari (Lua 5.3).
-- The mock follows the Forever 1.60.1 UI source where it matters: filters are skipped when
-- arg1 is secret, history entries carry (event, eventArgs) with lineID at eventArgs[11], and a
-- transform that errors leaves a hole in the history buffer.
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
    return {
        luaScriptBucketThrottleMaxMsPerSecondNormal = 500, luaScriptBucketThrottleMaxMsBurstNormal = 1000,
        luaScriptBucketThrottleMaxMsPerSecondRestricted = 500, luaScriptBucketThrottleMaxMsBurstRestricted = 1000,
    }
end
Enum = {
    AddOnRestrictionType = { Combat = 0, Encounter = 1, ChallengeMode = 2, PvPMatch = 3, Map = 4, Chat = 5 },
    AddOnRestrictionState = { Inactive = 0, Activating = 1, Active = 2 },
    AddOnProfilerMetric = { RecentAverageTime = 1, PeakTime = 4, CountTimeOver5Ms = 6 },
}
C_RestrictedActions = {
    IsAddOnRestrictionActive = function(t)
        return t == 5 and lockdown
    end,
}
C_AddOnProfiler = {
    IsEnabled = function()
        return true
    end,
    GetAddOnMetric = function()
        return 0
    end,
    -- fengari has no collectgarbage("count"): report a fixed allocation, the test only
    -- checks that the bench reads the fields.
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

-- Delivers one chat event the way ChatFrameMixin:MessageEventHandler does, then to addon frames.
local function Deliver(event, text, sender, guid)
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
        if not discard then
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

ns.Model.Configure({ D = 32, H = 4, FF = 64, V = 500, SL = 20, ENC = 2, DEC = 1, POOL_KB = 8 })
ns.Bench.targetMs = 20

rawprint("== load (" .. LOCALE_UNDER_TEST .. ")")
check(Printed(L.LOADED), "prints the loaded line")
check(TranslateProbeDB and TranslateProbeDB.budgetMs == 4 and TranslateProbeDB.logging == true, "saved variables get defaults")
check(LOCALE_UNDER_TEST ~= "frFR" or L.LOADED:find("Tape", 1, true), "French strings on frFR")
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

rawprint("== chat lockdown")
lockdown = true
instanceType = "raid"
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 2)
local locked = Deliver("CHAT_MSG_RAID", "pull in 5", "Raidlead")
Tick(1)
local lockedEntry = EntryFor(ChatFrame1, locked)
check(lockedEntry and issecretvalue(lockedEntry.message), "secret line displayed untouched")
check(TranslateProbeDB.stats["raid"] and TranslateProbeDB.stats["raid"].secret == 1, "secret line counted in context raid")
check(TranslateProbeDB.log[#TranslateProbeDB.log].s == 1 and TranslateProbeDB.log[#TranslateProbeDB.log].r == "t",
    "log row records secret + chat restriction")
check(Printed("Chat"), "restriction change printed")

lockdown = false
FireEvent("ADDON_RESTRICTION_STATE_CHANGED", 5, 0)
Tick(70)
local catch = TranslateProbeDB.catchup[1]
check(catch and catch.tag == "1s" and catch.tried == 1 and catch.readable == 1, "catch-up re-reads the secret line after lockdown")
Tick(60 * 30)
check(TranslateProbeDB.catchup[2] and TranslateProbeDB.catchup[2].tag == "30s", "second catch-up 30 s later")

rawprint("== click: simulated translation under the line")
local button = LinksIn(EntryFor(ChatFrame1, readable).message)[1]
Click(button)
local running = EntryFor(ChatFrame1, readable).message
check(running:find("|n", 1, true) and running:find(L.SUFFIX_RUNNING:sub(1, 12), 1, true), "placeholder shown under the line")
local done = TickUntil(function()
    local e = EntryFor(ChatFrame1, readable)
    return e and e.message:find(L.SUFFIX_RESULT:sub(1, 12), 1, true) and not ns.Scheduler.IsBusy()
end, 5000)
check(done, "result replaces the placeholder")
local finished = EntryFor(ChatFrame1, readable).message
check(select(2, finished:gsub("|n", "")) == 2, "two lines under the message: glossary + timing")
check(finished:find(L.SUFFIX_NO_GLOSS, 1, true), "glossary line says nothing known for a German sentence")
check(EntryFor(ChatFrame3, readable).message == finished, "same result in every chat frame")
check(#TranslateProbeDB.results.clicks == 1 and TranslateProbeDB.results.clicks[1].macs > 0, "click timing saved")
check(ChatFrame1.holes == 0 and ChatFrame3.holes == 0, "no history holes from transforms")
check(issecretvalue(EntryFor(ChatFrame1, locked).message), "secret entries untouched by transforms")

local suffixLink = LinksIn(finished)[2]
check(suffixLink and suffixLink:find(":x:", 1, true), "result line is a link")
Click(suffixLink)
check(not EntryFor(ChatFrame1, readable).message:find("|n", 1, true), "clicking the result hides it")
Click(button)
check(EntryFor(ChatFrame1, readable).message == finished and not ns.Scheduler.IsBusy(), "clicking [T] again shows the cached result")

Click("addon:TranslateProbe:t:1:" .. readable)
check(Printed(L.OLD_SESSION), "links from an older session are refused")

rawprint("== glossary")
local G = ns.Glossary
local fr = LOCALE_UNDER_TEST == "frFR"
local g = G.Describe("LFM dm, need heal. Bgs later? lf2m Stock")
check(g and g:find("LFM: ", 1, true) and g:find("DM: ", 1, true) and g:find("BGS: ", 1, true)
    and g:find("LF2M: ", 1, true) and g:find("STOCK: ", 1, true), "abbreviations found whatever the case, plural and number")
check(g and g:find(fr and "Les Mortemines (The Deadmines)" or "DM: The Deadmines", 1, true), "meaning shown in the client language")
check(G.Describe("Lfg") and G.Describe("Lfg"):find("LFG: ", 1, true), "mixed case matches")
check(G.Describe("|cffffffff|Hitem:1|h[Deadmines Key]|h|r please") == nil, "item links and colors are not scanned")
check(G.Describe("bodmin") == nil and G.Describe("dmg") == nil, "whole words only")
local hot = G.Describe("HoT anyone?")
check(hot and select(2, hot:gsub(" / ", "")) == 2, "HoT shows its three meanings")
slash("gloss add hot Hall of Thanes")
check(G.Describe("hot") == "HOT: Hall of Thanes", "custom entry replaces a built-in")
slash("gloss del hot")
check(G.Describe("hot") == nil and TranslateProbeDB.glossary.hot == false, "deleting a built-in disables it")
slash("gloss add hot Hall of Thanes / chaud")
check(G.Describe("HOT") == "HOT: Hall of Thanes / chaud", "several custom meanings with /")
slash("gloss add zz top|secret")
check(G.Describe("zz") == "ZZ: top/secret", "pipes in custom text are neutralized")
slash("gloss del zz")
check(TranslateProbeDB.glossary.zz == nil, "deleting a custom entry removes it")
slash("gloss list")
check(Printed("HOT = Hall of Thanes / chaud"), "gloss list shows custom entries")
slash("gloss test lfm wsg")
check(Printed("WSG: "), "gloss test prints the meanings")

slash("simulate off")
check(TranslateProbeDB.simulate == false, "test mode off")
local lfm = Deliver("CHAT_MSG_CHANNEL", "LFM DM need tank", "Bob")
Tick(1)
Click(LinksIn(EntryFor(ChatFrame1, lfm).message)[1])
local glossOnly = EntryFor(ChatFrame1, lfm).message
check(not ns.Scheduler.IsBusy() and select(2, glossOnly:gsub("|n", "")) == 1 and glossOnly:find("DM: ", 1, true),
    "without test mode a click shows only the glossary, instantly")
slash("simulate on")

rawprint("== slash commands")
slash("model 4")
local model = TickUntil(function() return #TranslateProbeDB.results.models == 1 and not ns.Scheduler.IsBusy() end, 5000)
check(model, "model command finishes")
local row = TranslateProbeDB.results.models[1]
check(row and row.macs == ns.Model.EstimateMacs(row.src, row.tgt), "forward pass counts exactly the estimated multiply-adds")
check(row and row.maxSliceMs < row.budgetMs + 25, "slices stay near the budget")

inCombat = true
slash("model 3")
Tick(10)
check(#TranslateProbeDB.results.models == 1, "jobs pause in combat")
inCombat = false
check(TickUntil(function() return #TranslateProbeDB.results.models == 2 end, 5000), "jobs resume after combat")

slash("bench")
check(TickUntil(function() return TranslateProbeDB.results.bench ~= nil end, 20000), "bench finishes")
local bench = TranslateProbeDB.results.bench
check(bench and bench.int8Rate > 0 and bench.floatRate > 0 and bench.byteRate > 0, "bench rates measured")
check(bench and bench.memory and bench.memory.stringByte > 0, "bench memory measured")

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
slash("log on")
slash("report")
check(Printed(L.REPORT_HEADER), "report prints the header")
slash("free")
check(not ns.Model.IsBuilt(), "free releases the model")
slash("reset")
check(#TranslateProbeDB.log == 0 and TranslateProbeDB.results.bench == nil, "reset clears logs and results")
slash("")
check(Printed(L.HELP_CLICK), "help lists the click hint")

check(#errors == 0, "no Lua errors reported (" .. #errors .. ")")
