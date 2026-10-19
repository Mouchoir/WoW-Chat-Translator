-- Checks of the TranslateProbe addon on the mock client (tests\mock.lua). Run by tests\run.js,
-- with the real Bridge.lua or with tests\stubs\Bridge.lua (--stub-bridge). Every message the
-- addon sends over the bridge is recorded by wrapping ns.Bridge.Send and parsed here, so the
-- same checks run with both.
--
-- Every tests\harness_*.lua (found by tests\run.js, run in name order) runs before the final
-- "privacy and errors" section, as a chunk called with one argument H, the shared context:
--   H.check(cond, label), H.section(name), H.out(line), H.M (= Mock), H.ns, H.L, H.db, H.fr (frFR
--   run), H.stubBridge, H.sent (every bridge message, live), H.Parse(bytes) -> message table,
--   H.Since(index, kind) -> parsed messages after index, H.AllOk(list), H.Deliver(event, text,
--   sender, guid, hidden, opts) -> lineID (records text, sender and GUID for the privacy walk),
--   H.Private(text) (another string that must never reach the saved variables: a draft, a sent
--   reply), H.slash(cmd), H.ButtonOf(lineID) -> the [T] link, H.EntryFor(cf, lineID),
--   H.EntryWith(cf, fragment), H.Printed(fragment), H.CountPrinted(fragment), H.Snapshot(),
--   H.Lines(message), H.Base(message), H.LinksIn(message), H.SUFFIX_START.
-- A Lua error in such a file counts as one failure; the other files still run. The files share
-- the one client session: each sets the settings it needs and leaves no window or focus behind.
local M = Mock
local out = M.rawprint

TEST_FAILURES, TEST_COUNT = 0, 0
local function check(cond, label)
    TEST_COUNT = TEST_COUNT + 1
    if cond then
        out("ok   " .. label)
    else
        TEST_FAILURES = TEST_FAILURES + 1
        out("FAIL " .. label)
    end
end
local function section(name)
    out("== " .. name)
end

local stubBridge = ADDON_FROM["Bridge.lua"] == "stub"
local fr = LOCALE_UNDER_TEST == "frFR"
local SUFFIX_START = "|n|cff9fd4ff    |Haddon:TranslateProbe:x:"

local function Printed(fragment)
    local printed = M.Printed()
    for i = #printed, 1, -1 do
        if printed[i]:find(fragment, 1, true) then
            return true
        end
    end
    return false
end
local function CountPrinted(fragment)
    local n = 0
    for _, line in ipairs(M.Printed()) do
        if line:find(fragment, 1, true) then
            n = n + 1
        end
    end
    return n
end
local function Count(s, fragment)
    local n, i = 0, 1
    while true do
        local a, b = s:find(fragment, i, true)
        if not a then
            return n
        end
        n, i = n + 1, b + 1
    end
end

-- Approximate saved-variables size in bytes.
local function Size(v)
    local t = type(v)
    if t == "table" then
        local n = 2
        for k, x in pairs(v) do
            n = n + Size(k) + Size(x) + 4
        end
        return n
    elseif t == "string" then
        return #v + 2
    end
    return #tostring(v)
end

---------------------------------------------------------------------------------------------
-- Before load: 70 KB of old telemetry in the saved variables, and rows of an older session
-- left in the chat history (chat survives a /reload).
---------------------------------------------------------------------------------------------
local fixture = {
    budgetMs = 16, speed = "fast", logging = true, simulate = true, sliceMigrated = true,
    log = {}, stats = { none = { readable = 812, secret = 0, own = 33 }, ["raid+boss"] = { readable = 2, secret = 77 } },
    transitions = {}, catchup = {},
    results = { models = {}, clicks = {}, throttles = {}, bench = { int8Rate = 1.5, floatRate = 3.2, maxSliceMs = 8 },
        client = { build = "1.60.1.70338", limits = { luaScriptBucketThrottleMaxMsPerSecondNormal = 800 } } },
    target = "xx", names = "yes", mode = "turbo", helpShown = "x", glossShow = 1, futureKey = { 1, 2, 3 },
    display = "tab",   -- the chat tab display mode of an unreleased build, dropped since
    glossary = { hot = "Hall of Thanes", zz = false, ["été"] = "summer", ["bad key"] = "x", num = 5,
        long = string.rep("a", 121), [7] = "x",
        -- per-target meanings: invalid targets and meanings dropped, the rest kept
        wbx = { fr = "boss du monde", de = "Weltboss", ["*"] = "world boss", xx = "bad target", en = string.rep("e", 121),
            ru = 5 },
        nomeaning = {},
        -- "" hides the every-target meaning for one target; without "*" it means nothing
        hidden = { ["*"] = "all", fr = "" }, lonely = { fr = "", de = "Sinn" } },
}
for i = 1, 400 do
    fixture.log[i] = { t = 1760000000 + i, e = "CHANNEL", s = i % 2, l = 0, r = "", c = "none" }
end
for i = 1, 200 do
    fixture.transitions[i] = { t = 1760000000 + i, type = "Chat", state = "Inactive", c = "raid", zone = "Stormwind" }
end
local i = 0
while Size(fixture) < 72000 do
    i = i + 1
    fixture.results.clicks[i] = { when = "2026-10-10 12:00", source = "de", target = "fr", chain = "de-en>en-fr",
        words = 7, macs = 123456 + i, busyMs = 12.5, wallMs = 800.25, queuedMs = 0, pausedMs = 0, frames = 48,
        maxSliceMs = 8.1, minBudgetMs = 4, budgetMs = 8, every = 1, fps = 60, context = "none", restrictions = "" }
end
local fixtureSize = Size(fixture)
TranslateProbeDB = fixture

local OLD_ROWS = "[Old]: hi |cff66bbff|Haddon:TranslateProbe:t:0:42|h[T]|h|r"
local OLD_ENTRY = OLD_ROWS .. SUFFIX_START .. "0:42|hold row|h|r" .. SUFFIX_START .. "0:42|hsecond|h|r"
ChatFrame1:AddMessage(OLD_ENTRY, 1, 1, 1, 1, 0, 0, "CHAT_MSG_SAY", { [11] = 42 }, nil)
ChatFrame1:AddMessage(M.Secret(), 1, 1, 1, 1, 0, 0, "CHAT_MSG_RAID", { [11] = 43 }, nil)
ChatFrame3:AddMessage(OLD_ENTRY, 1, 1, 1)   -- a copy without eventArgs

---------------------------------------------------------------------------------------------
-- Load
---------------------------------------------------------------------------------------------
local ns = M.Load(ADDON_FILES, function(file)
    return ADDON_SOURCES[file]
end)
local L = ns.L
local slashFn = SlashCmdList.TRANSLATEPROBE
local function slash(cmd)
    local ok, err = pcall(slashFn, cmd)
    if not ok then
        M.Errors()[#M.Errors() + 1] = "slash " .. cmd .. ": " .. tostring(err)
        out("SLASH ERROR: " .. tostring(err))
    end
end
local G, Reserve, Request, ChatButton, Bridge = ns.Glossary, ns.Reserve, ns.Request, ns.ChatButton, ns.Bridge
local db = TranslateProbeDB

-- Every bridge message, in order.
local sent = {}
local realSend = Bridge.Send
Bridge.Send = function(msg)
    sent[#sent + 1] = msg
    return realSend(msg)
end

local function U16(s, i)
    return s:byte(i) * 256 + s:byte(i + 1)
end
-- Strict parser of the v2 messages (docs\BRIDGE.md); ok = the length matches exactly.
local function Parse(b)
    local t = b:byte(1)
    if t == 0x10 then
        local m = { kind = "translate", reqId = U16(b, 2), slot = b:byte(4), rows = b:byte(5), flags = b:byte(6), pairs = {} }
        local i = 7
        local n = b:byte(i)
        m.target = b:sub(i + 1, i + n)
        i = i + 1 + n
        local tl = U16(b, i)
        m.text = b:sub(i + 2, i + 1 + tl)
        i = i + 2 + tl
        local np = b:byte(i)
        i = i + 1
        for k = 1, np do
            local a = b:byte(i)
            local term = b:sub(i + 1, i + a)
            i = i + 1 + a
            local c = b:byte(i)
            local meaning = b:sub(i + 1, i + c)
            i = i + 1 + c
            m.pairs[k] = { term, meaning }
        end
        m.ok = i == #b + 1
        return m
    elseif t == 0x11 then
        return { kind = "cancel", reqId = U16(b, 2), slot = b:byte(4), ok = #b == 4 }
    elseif t == 0x12 then
        return { kind = "clear", ok = #b == 1 }
    elseif t == 0x13 then
        return { kind = "test", nonce = U16(b, 2), ok = #b == 3 }
    elseif t == 0x14 then
        local n = b:byte(2)
        local m = { kind = "view", views = {}, ok = #b == 2 + 11 * n }
        for k = 1, n do
            local o = 3 + (k - 1) * 11
            m.views[k] = { id = b:byte(o), x = U16(b, o + 1), y = U16(b, o + 3), w = U16(b, o + 5), h = U16(b, o + 7),
                fontPx = b:byte(o + 9), rowPx = b:byte(o + 10) }
        end
        return m
    end
    return { kind = "unknown", ok = false }
end
-- Parsed messages sent after index from (exclusive), optionally of one kind.
local function Since(from, kind)
    local list = {}
    for k = from + 1, #sent do
        local m = Parse(sent[k])
        if not kind or m.kind == kind then
            list[#list + 1] = m
        end
    end
    return list
end
local function AllOk(list)
    for _, m in ipairs(list) do
        if not m.ok then
            return false
        end
    end
    return true
end

-- Delivered texts, senders and GUIDs, for the privacy walk at the end.
local deliveredText, deliveredSender, deliveredGuid = {}, {}, {}
local function Deliver(event, text, sender, guid, hidden, opts)
    deliveredText[#deliveredText + 1] = text
    deliveredSender[#deliveredSender + 1] = sender
    if type(guid) == "string" then
        deliveredGuid[#deliveredGuid + 1] = guid
    end
    return M.Deliver(event, text, sender, guid, hidden, opts)
end

local function EntryFor(cf, lineID)
    for _, e in ipairs(M.History(cf)) do
        if e and e.extra[5] and e.extra[5][11] == lineID then
            return e
        end
    end
end
local function EntryWith(cf, fragment)
    local history = M.History(cf)
    for k = #history, 1, -1 do
        local e = history[k]
        if e and not issecretvalue(e.message) and e.message:find(fragment, 1, true) then
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
local function ButtonOf(lineID)
    return "addon:TranslateProbe:t:" .. ns.session .. ":" .. lineID
end
local function Lines(message)
    return select(2, message:gsub("|n", ""))
end
local function Base(message)
    local cut = message:find(SUFFIX_START, 1, true)
    return cut and message:sub(1, cut - 1) or message
end
local function Snapshot()
    local snap = {}
    for _, cf in ipairs(M.ChatFrames()) do
        for k, e in ipairs(M.History(cf)) do
            snap[#snap + 1] = cf:GetName() .. k .. "=" .. (e and (issecretvalue(e.message) and "<secret>" or e.message)
                .. "@" .. e.timestamp or "hole")
        end
    end
    return table.concat(snap, "\n")
end
local function Utf8Len(s)
    local n = 0
    for _ in s:gmatch("[^\128-\191]") do
        n = n + 1
    end
    return n
end
-- Expected K with the mock font metrics (0.5 x font height per character), at most 6 rows.
local function ExpectedK(text, plateW, fontH, factor)
    local width = 0.5 * fontH * Utf8Len(G.VisibleText(text))
    local rowsSrc = width / plateW     -- not rounded up before the factor
    return math.min(6, math.max(1, math.ceil(rowsSrc * factor - 0.15)))
end
local function RowsFor(cf, lineID, slot, K, instant)
    local m = Reserve.Metrics(cf)
    local rows = {}
    for r = 0, K - 1 do
        rows[#rows + 1] = Bridge.TagMarkup(slot, r, ns.sessionId % 2, m.W, m.H) .. Bridge.PlateMarkup(m.plateW, m.H)
    end
    for _, line in ipairs(instant or {}) do
        rows[#rows + 1] = ChatButton.Safe(line)
    end
    return ChatButton.SuffixLinks(lineID, rows)
end

section("load (" .. LOCALE_UNDER_TEST .. ", Bridge.lua from " .. ADDON_FROM["Bridge.lua"] .. ")")
check(Printed(L.LOADED), "prints the loaded line")
check(not fr or L.LOADED:find("Tape", 1, true), "French strings on frFR")
check(SLASH_TRANSLATEPROBE1 == "/tprobe" and SLASH_TRANSLATEPROBE2 == "/tpr" and slashFn ~= nil, "slash command registered")
check(type(ns.sessionId) == "number" and ns.sessionId >= 1 and ns.sessionId <= 65535 and ns.session == tostring(ns.sessionId),
    "session id is a u16 and its string form is used in links")
check(ns.ADDON_BUILD == 3 and Bridge.VERSION == 2 and Bridge.TEST_SLOT == 127, "addon build 3, bridge v2")
check(CountPrinted((L.INIT_FAILED:gsub("%%s", ""))) == 0, "no init failure printed")

section("bridge builders")
local function Digits(d)
    return table.concat(d, ",")
end
check(Bridge.Crc5(0) == 25 and Bridge.Crc5(44) == 17 and Bridge.Crc5(1016) == 23, "CRC-5 check values")
check(Digits(Bridge.TagDigits(0, 0, 0)) == "0,0,0,0,4,1" and Digits(Bridge.TagDigits(5, 2, 0)) == "0,1,0,3,3,3"
    and Digits(Bridge.TagDigits(127, 0, 0)) == "4,1,0,3,4,3", "tag digit check values")
if stubBridge and REAL_BRIDGE_SOURCE then
    -- The stub must build exactly what the real Bridge.lua builds (loaded apart, no frames).
    local realNs = {}
    assert(load(REAL_BRIDGE_SOURCE, "@Bridge.lua"))("TranslateProbe", realNs)
    local R = realNs.Bridge
    local same = true
    for slot = 0, 127 do
        for row = 0, 3 do
            for bitValue = 0, 1 do
                if Bridge.TagMarkup(slot, row, bitValue, 4, 14) ~= R.TagMarkup(slot, row, bitValue, 4, 14) then
                    same = false
                end
            end
        end
    end
    local samplePairs = { { "lf2m", "looking for 2 more" }, { "cheap", "pas cher" }, { string.rep("t", 60), string.rep("m", 200) } }
    local views = { { id = 1, x = 28, y = 686, w = 563, h = 253, fontPx = 20, rowPx = 20 } }
    local messages = {
        { Bridge.TranslateMessage(7, 3, 2, 3, "fr", "x \2[Linen Cloth]\3 y", samplePairs),
            R.TranslateMessage(7, 3, 2, 3, "fr", "x \2[Linen Cloth]\3 y", samplePairs) },
        { Bridge.TranslateMessage(65535, 126, 4, 2, "zh-TW", string.rep("é", 700), {}),
            R.TranslateMessage(65535, 126, 4, 2, "zh-TW", string.rep("é", 700), {}) },
        { Bridge.CancelMessage(9, 5), R.CancelMessage(9, 5) }, { Bridge.ClearMessage(), R.ClearMessage() },
        { Bridge.TestMessage(4242), R.TestMessage(4242) }, { Bridge.ViewMessage(views), R.ViewMessage(views) },
        { Bridge.PlateMarkup(290, 14), R.PlateMarkup(290, 14) }, { Bridge.Utf8Cut("aé", 2), R.Utf8Cut("aé", 2) },
    }
    for flags = 0, 255 do
        messages[#messages + 1] = { Bridge.TranslateMessage(258, 126, 3, flags, "de", "x", {}),
            R.TranslateMessage(258, 126, 3, flags, "de", "x", {}) }
    end
    for _, pair in ipairs(messages) do
        if pair[1] ~= pair[2] then
            same = false
        end
    end
    local function Textures()
        local t = {}
        for k = 1, 8 do
            t[k] = { SetColorTexture = function(self, r, g, b, a)
                self.rgba = string.format("%.6f %.6f %.6f %.6f", r, g, b, a)
            end }
        end
        return t
    end
    for slot = 0, 127, 7 do
        for row = 0, 5 do
            local a, b = Textures(), Textures()
            Bridge.ApplyTag(a, slot, row, slot % 2)
            R.ApplyTag(b, slot, row, slot % 2)
            for k = 1, 8 do
                if a[k].rgba ~= b[k].rgba then
                    same = false
                end
            end
        end
    end
    check(same, "the stub builds the same tags, plates, messages and window tags as the real Bridge.lua")
    check(R.FLAG_REPLY == Bridge.FLAG_REPLY and R.FLAG_COPY == Bridge.FLAG_COPY and R.FLAGS_MASK == Bridge.FLAGS_MASK
        and R.BUBBLE_RGB[1] == Bridge.BUBBLE_RGB[1] and R.BUBBLE_RGB[2] == Bridge.BUBBLE_RGB[2]
        and R.BUBBLE_RGB[3] == Bridge.BUBBLE_RGB[3], "the stub has the real flag and colour constants")
end

section("bridge: TRANSLATE flags and window tags")
check(Bridge.FLAG_NOCACHE == 1 and Bridge.FLAG_PLATE == 2 and Bridge.FLAG_REPLY == 4 and Bridge.FLAG_COPY == 8
    and Bridge.FLAGS_MASK == 15, "flag constants: noCache 1, plate 2, reply 4, copy 8, mask 15")
check(Bridge.BUBBLE_RGB[1] == 36 and Bridge.BUBBLE_RGB[2] == 30 and Bridge.BUBBLE_RGB[3] == 24
    and Bridge.PLATE_RGB[1] == 32, "bubble interior (36, 30, 24) beside the plate colour")
do
    local function Run()
        local function FlagsByte(flags)
            return Parse(Bridge.TranslateMessage(258, 126, 3, flags, "de", "on part dans 5 min ?", {})).flags
        end
        local expect = { [0] = 0, [2] = 2, [4] = 4, [6] = 6, [8] = 0, [10] = 2, [12] = 12, [14] = 14, [15] = 15,
            [16] = 0, [255] = 15, [-3] = 0, [300] = 15 }
        local okFlags = true
        for flags, want in pairs(expect) do
            if FlagsByte(flags) ~= want then
                okFlags = false
                out("  flags " .. flags .. " -> " .. tostring(FlagsByte(flags)) .. ", expected " .. want)
            end
        end
        check(okFlags, "builder: flags masked to bits 0-3, copy dropped unless reply, nothing else remapped")
        local m = Bridge.TranslateMessage(258, 126, 3, 14, "de", "on part dans 5 min ?", {})
        local hex = m:gsub(".", function(c) return string.format("%02x", c:byte()) end)
        check(hex == "1001027e030e02646500146f6e20706172742064616e732035206d696e203f00",
            "the reply example of docs\\BRIDGE.md, byte for byte")
        local textures = {}
        for k = 1, 8 do
            textures[k] = { SetColorTexture = function(self, r, g, b, a)
                self.r, self.g, self.b, self.a = r, g, b, a
            end }
        end
        local same = true
        for _, case in ipairs({ { 0, 0, 0 }, { 126, 5, 1 }, { 100, 4, 0 }, { 127, 3, 1 } }) do
            Bridge.ApplyTag(textures, case[1], case[2], case[3])
            local colors = Bridge.TagColors(case[1], case[2] % 4, case[3])
            for k = 1, 8 do
                local t, c = textures[k], colors[k]
                if math.abs(t.r * 255 - c[1]) > 1e-9 or math.abs(t.g * 255 - c[2]) > 1e-9
                    or math.abs(t.b * 255 - c[3]) > 1e-9 or t.a ~= 1 then
                    same = false
                end
            end
        end
        check(same, "ApplyTag colours 8 textures like TagColors (row modulo 4), opaque")
    end
    Run()
end

section("saved variables: whitelist purge")
check(fixtureSize >= 70000, "the telemetry fixture is about 70 KB (" .. fixtureSize .. " bytes)")
local KEEP = { schema = true, target = true, names = true, glossShow = true, mode = true, glossary = true, helpShown = true,
    display = true, follow = true, replyClipboard = true, convOriginal = true, convText = true, convScale = true,
    convCombat = true, convFocus = true, convOpenWhisper = true, convPos = true, convTipShown = true }
local extra = {}
for key in pairs(db) do
    if not KEEP[key] then
        extra[#extra + 1] = tostring(key)
    end
end
check(#extra == 0, "only whitelisted keys are left (" .. table.concat(extra, ", ") .. ")")
check(db.schema == 4 and db.mode == "auto" and db.names == false and db.glossShow == false and db.helpShown == nil,
    "schema 4, invalid settings back to their defaults (glossary row off by default)")
check(db.follow == "smart" and db.replyClipboard == "ask" and db.convText == 14 and db.convScale == 100
    and db.convCombat == "keep" and db.convOriginal == false and db.convFocus == false and db.convOpenWhisper == false
    and db.convPos == nil and db.convTipShown == nil, "conversation window settings get their defaults")
check(db.target == (fr and "fr" or "en"), "an unknown target falls back to the client locale")
check(db.glossary.hot == "Hall of Thanes" and db.glossary.zz == false and db.glossary["été"] == "summer",
    "valid glossary entries kept")
check(db.glossary["bad key"] == nil and db.glossary.num == nil and db.glossary.long == nil and db.glossary[7] == nil,
    "invalid glossary entries dropped")
check(db.display == "window", "an old display = \"tab\" (the dropped chat tab mode) is purged to the window default")
local wbx = db.glossary.wbx
local wbxKeys = {}
for k in pairs(type(wbx) == "table" and wbx or {}) do
    wbxKeys[#wbxKeys + 1] = k
end
table.sort(wbxKeys)
check(table.concat(wbxKeys, ",") == "*,de,fr" and wbx.fr == "boss du monde" and type(db.glossary.nomeaning) == "table",
    "per-target glossary meanings kept, invalid targets and meanings dropped")
check(Size(db) < 2048, "saved variables now under 2 KB (" .. Size(db) .. " bytes)")
check(type(db.glossary.hidden) == "table" and db.glossary.hidden.fr == "" and db.glossary.hidden["*"] == "all"
    and type(db.glossary.lonely) == "table" and db.glossary.lonely.fr == nil and db.glossary.lonely.de == "Sinn",
    "an empty marker is kept next to an every-target meaning, dropped without one")
db.glossary.hot, db.glossary.zz, db.glossary.wbx, db.glossary.nomeaning = nil, nil, nil, nil
db.glossary.hidden, db.glossary.lonely = nil, nil
-- The chat checks below were written for the glossary row on and [T] drawing rows under the
-- line. Since schema 4 the row is off by default and [T] opens the conversation window, so they
-- run with those two settings set; harness_conv.lua and harness_reply.lua cover the defaults.
db.glossShow, db.display = true, "inline"

section("session hygiene")
local old1 = M.History(ChatFrame1)[1]
local old3 = M.History(ChatFrame3)[1]
check(old1 and old1.message == OLD_ROWS and old3 and old3.message == OLD_ROWS,
    "rows of an older session are cut at PLAYER_ENTERING_WORLD, the line and its old [T] stay")
check(issecretvalue(M.History(ChatFrame1)[2].message), "secret entries untouched")
check(Reserve.Counters().stripped == 2 and ChatFrame1.holes == 0 and ChatFrame3.holes == 0, "two entries cut, no hole")
M.Click("addon:TranslateProbe:t:0:42")
check(Printed(L.OLD_SESSION) and #sent == 0, "a [T] of an older session is refused, nothing sent")

---------------------------------------------------------------------------------------------
section("button on readable lines")
local readable = Deliver("CHAT_MSG_SAY", "hallo zusammen, wer will mit in die Todesminen", "Hans")
M.Tick(1)
local e1, e3 = EntryFor(ChatFrame1, readable), EntryFor(ChatFrame3, readable)
check(e1 and e1.message:find("|Haddon:TranslateProbe:t:" .. ns.session .. ":" .. readable .. "|h[T]|h", 1, true),
    "button appended in ChatFrame1")
check(e3 and #LinksIn(e3.message) == 1, "button appended once in ChatFrame3")
check(e1 and e1.message:find("^%[Hans%]: hallo zusammen"), "original text kept at the start")
M.Tick(3)
check(#LinksIn(EntryFor(ChatFrame1, readable).message) == 1, "no second button on later frames")
local own = Deliver("CHAT_MSG_SAY", "my own line", "Me", "Player-1-0001")
M.Tick(1)
check(#LinksIn(EntryFor(ChatFrame1, own).message) == 0, "no button on my own messages")
local hidden = Deliver("CHAT_MSG_CHANNEL", "a line no tab shows", "Tom", nil, true)
M.Tick(1)
check(not ChatButton.IsTracked(hidden) and ChatButton.IsTracked(readable), "lines no frame shows are not tracked")
local questOnly = Deliver("CHAT_MSG_CHANNEL", "|cffffff00|Hquest:123:31|h[[31+] Understanding Our Present]|h|r", "Quester")
M.Tick(1)
check(#LinksIn(EntryFor(ChatFrame1, questOnly).message) == 0, "no button on a line made only of a shared quest")
check(#sent == 0, "nothing is sent while idle")

---------------------------------------------------------------------------------------------
section("click: reserved rows and the TRANSLATE request")
slash("names on")
M.SetFont(ChatFrame3, "Fonts\\ARIALN.TTF", 18)
local ITEM = "|cffffffff|Hitem:2589::::::::::::::|h[Linen Cloth]|h|r"
local clickText = "WTS " .. ITEM .. " x20 cheap, lf2m for dm {skull} [x]"
local clicked = Deliver("CHAT_MSG_CHANNEL", clickText, "하늘노래 바람검")
M.Tick(1)
local m1, m3 = Reserve.Metrics(ChatFrame1), Reserve.Metrics(ChatFrame3)
check(m1 and m1.ok and m1.W == 4 and m1.H == 14 and m1.plateW == 290 and math.abs(m1.ppu - 1.40625) < 1e-9,
    "ChatFrame1 metrics: 5 px blocks, font-high rows, plate width from the chat width")
check(m3 and m3.ok and m3.W == 4 and m3.H == 18 and m3.plateW == 283, "ChatFrame3 metrics follow its own font")
local base1, base3 = EntryFor(ChatFrame1, clicked).message, EntryFor(ChatFrame3, clicked).message
local n0 = #sent
M.Click(ButtonOf(clicked))
local msgs = Since(n0)
check(#msgs == 2 and msgs[1].kind == "view" and msgs[2].kind == "translate" and AllOk(msgs),
    "first click: VIEW, then TRANSLATE, both well formed")
local view = msgs[1].views and msgs[1].views[1]
check(msgs[1].views and #msgs[1].views == 1 and view.id == 1 and view.x == 28 and view.y == 686 and view.w == 563
    and view.h == 253 and view.fontPx == 20 and view.rowPx == 20, "VIEW: the shown chat frame in physical px")
local tr = msgs[2]
local K = ExpectedK(clickText, 290, 14, 1.35)
check(tr.slot == 0 and tr.rows == K and tr.reqId == 1 and tr.flags == 2, "TRANSLATE: slot 0, K rows, plate flag, no noCache")
check(tr.target == db.target, "TRANSLATE: target is the saved target")
check(tr.text:find("\2[Linen Cloth]\3", 1, true) and tr.text:find("\2{skull}\3", 1, true), "real item link and {skull} are marked")
check(tr.text:find(" [x]", 1, true) and not tr.text:find("\2[x]", 1, true), "a typed [x] stays plain")
check(not tr.text:find("|", 1, true), "no escape code left in the text")
local instant = ChatButton.InstantLines(clicked, clickText)
check(#instant == 2 and instant[1] == L.SUFFIX_GLOSS .. G.Describe(clickText, db.target)
    and instant[2] == L.NAME_LINE:format("ro ro"), "instant rows: glossary, then the romanized name")
local e1c, e3c = EntryFor(ChatFrame1, clicked).message, EntryFor(ChatFrame3, clicked).message
check(e1c == base1 .. RowsFor(ChatFrame1, clicked, 0, K, instant), "ChatFrame1: K tagged rows, then glossary, then name")
check(e3c == base3 .. RowsFor(ChatFrame3, clicked, 0, K, instant), "ChatFrame3: rows sized with its own metrics")
local xLink = "|Haddon:TranslateProbe:x:" .. ns.session .. ":" .. clicked .. "|h"
local firstRow = SUFFIX_START .. ns.session .. ":" .. clicked .. "|h"
    .. Bridge.TagMarkup(0, 0, ns.sessionId % 2, 4, 14):gsub("|t|T", "|t|h" .. xLink:gsub("%%", "%%%%") .. "|T")
    .. "|h" .. xLink .. Bridge.PlateMarkup(290, 14) .. "|h|r"
check(e1c:find(firstRow, 1, true), "first row: the 8 tag blocks then the plate, each texture in its own link")
local oneTexturePerLink = true
for body in e1c:gmatch("|Haddon:TranslateProbe:x:[^|]*|h(.-)|h") do
    local _, n = body:gsub("|T", "")
    if n > 1 then
        oneTexturePerLink = false
    end
end
check(oneTexturePerLink, "no link holds more than one texture (the client draws only the first)")
check(Lines(e1c) == K + 2 and Count(e1c, "|TInterface\\Buttons\\WHITE8X8:") == 9 * K, "K rows of 8 tag blocks and a plate")
check(Printed(L.FIRST_USE) and db.helpShown == true, "first click ever explains the helper")
check(Request.Count() == 1 and Reserve.SlotsInUse() == 1 and Reserve.SlotOwner(0) == clicked, "one active request on slot 0")
local pairsByTerm = {}
for _, p in ipairs(tr.pairs) do
    pairsByTerm[p[1]] = p[2]
end
if fr then
    check(pairsByTerm.cheap == "pas cher" and pairsByTerm.lf2m == "cherche 2 membre(s)"
        and pairsByTerm.dm == "Les Mortemines / Hache-Tripes / message privé" and pairsByTerm.WTS == "vend",
        "fr pairs: French meanings and the cheap false friend")
else
    check(pairsByTerm.cheap == nil and pairsByTerm.lf2m == "looking for 2 more"
        and pairsByTerm.dm == "The Deadmines / Dire Maul / direct message" and pairsByTerm.WTS == "want to sell",
        "en pairs: English meanings, no false friend")
end
check(tr.pairs[1][1] == "WTS", "pairs in order of appearance, terms as typed")

-- The strip (real Bridge only): shown after a click, hidden once the queue drains.
local function StripBlocks()
    local shown = {}
    for _, t in ipairs(M.Textures()) do
        if t.strata == "TOOLTIP" and t.shown then
            shown[#shown + 1] = t
        end
    end
    return shown
end
if stubBridge then
    out("skip strip shown and drained (stub bridge)")
else
    M.Tick(1)
    local blocks = StripBlocks()
    local function At(x, y)
        for _, t in ipairs(blocks) do
            if math.abs(t.x - x) < 1e-6 and math.abs(t.y - y) < 1e-6 and math.abs(t.w - 2) < 1e-6 and math.abs(t.h - 2) < 1e-6 then
                return t
            end
        end
    end
    local b0, b1, b2, b4 = At(0, 0), At(0, 2), At(2, 0), At(4, 0)
    check(#blocks >= 8 and Bridge.IsBusy(), "strip shown after the click")
    check(b0 and b1 and b2 and b0.r == 0 and b1.r == 1 and b2.r == 1 and b4 and math.abs(b4.r - 104 / 255) < 1e-6,
        "strip blocks: 2x2 physical px at the client's top-left, calibration greys")
    local drained = false
    for _ = 1, 600 do
        M.Tick(1)
        if #StripBlocks() == 0 and not Bridge.IsBusy() then
            drained = true
            break
        end
    end
    check(drained, "strip hidden again once the queue drained")
end

section("re-clicks")
local tr1 = tr
n0 = #sent
M.Click(ButtonOf(clicked))
check(#sent == n0, "a second click within 1 s is ignored")
M.Tick(61)
M.Click(ButtonOf(clicked))
msgs = Since(n0, "translate")
check(#msgs == 1 and msgs[1].reqId == tr1.reqId and msgs[1].slot == tr1.slot and msgs[1].flags == 2,
    "a plain re-click sends the same request again")
local resent = Since(n0)
check(#resent == 2 and resent[1].kind == "view" and resent[2].kind == "translate",
    "every TRANSLATE follows a VIEW, unchanged or not (a restarted helper leaves discovery at once)")
check(EntryFor(ChatFrame1, clicked).message == e1c, "rows unchanged by a re-click")
M.Tick(61)
n0 = #sent
M.Click(ButtonOf(clicked), ChatFrame1, true)
msgs = Since(n0, "translate")
check(#msgs == 1 and msgs[1].reqId > tr1.reqId and msgs[1].slot == tr1.slot and msgs[1].flags == 3,
    "Shift-click: new reqId with noCache, same slot")
check(EntryFor(ChatFrame1, clicked).message == e1c, "rows unchanged by a Shift-click")
local shiftReq = msgs[1].reqId
M.Tick(61)
slash("target de")
n0 = #sent
M.Click(ButtonOf(clicked))
msgs = Since(n0, "translate")
local dePairs = {}
for _, p in ipairs(msgs[1] and msgs[1].pairs or {}) do
    dePairs[p[1]] = p[2]
end
check(#msgs == 1 and msgs[1].reqId > shiftReq and msgs[1].target == "de" and msgs[1].flags == 2,
    "a new target restarts the request")
check(dePairs.lf2m == "looking for 2 more" and dePairs.cheap == nil, "de pairs: English meanings as the pivot")
slash("target " .. (fr and "fr" or "en"))
-- the glossary row starts with ChatButton.GLOSS_START (its own colour, D11), the others with SUFFIX_START
check(Count(EntryFor(ChatFrame1, clicked).message, SUFFIX_START) + Count(EntryFor(ChatFrame1, clicked).message,
    ChatButton.GLOSS_START) == K + 2, "rows never duplicated (the rewrite cuts first)")

section("tooltips")
EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkEnter", ChatFrame1, ButtonOf(clicked), "[T]")
check(GameTooltip:IsShown() and GameTooltip.lines[1] == L.CONV_TIP_T_INLINE,
    "hovering [T] names the helper (inline mode: Ctrl-click opens the conversation window)")
EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkLeave", ChatFrame1)
check(not GameTooltip:IsShown(), "leaving hides the tooltip")
EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkEnter", ChatFrame1, "addon:TranslateProbe:x:" .. ns.session .. ":" .. clicked)
check(GameTooltip:IsShown() and GameTooltip.lines[1] == L.TIP_ROWS, "hovering a row explains it")
EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkLeave", ChatFrame1)
EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkEnter", ChatFrame1, "item:2589")
check(not GameTooltip:IsShown(), "other links get no tooltip from us")

section("no timed rewrites, VIEW watcher")
local before = Snapshot()
n0 = #sent
M.Tick(60 * 30)
check(Snapshot() == before, "30 s of ticks leave every history entry and timestamp unchanged")
check(#Since(n0) == 0, "nothing sent while nothing changes")
M.SetChatRect(ChatFrame1, 30, 120, 420, 200)
M.Tick(31)
msgs = Since(n0, "view")
check(#msgs == 1 and msgs[1].views[1].x == 42 and msgs[1].views[1].y == 630 and msgs[1].views[1].w == 591
    and msgs[1].views[1].h == 281, "a moved chat frame sends a new VIEW within 0.5 s")
M.Tick(60)
check(#Since(n0, "view") == 1, "no VIEW while the chat stays put")
M.SetChatRect(ChatFrame1, 20, 100, 400, 180)

section("removal")
n0 = #sent
local reqNow = Request.Get(clicked)
M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. clicked)
msgs = Since(n0)
check(EntryFor(ChatFrame1, clicked).message == base1 and EntryFor(ChatFrame3, clicked).message == base3,
    "a click on a row removes the rows in every frame")
check(#msgs == 1 and msgs[1].kind == "cancel" and msgs[1].reqId == reqNow and msgs[1].slot == 0, "CANCEL sent with reqId and slot")
check(Request.Count() == 0 and Reserve.SlotsInUse() == 0 and not Reserve.IsWatching(), "slot freed, watcher stopped")
n0 = #sent
M.SetChatRect(ChatFrame1, 25, 100, 400, 180)
M.Tick(60)
check(#Since(n0) == 0, "no VIEW without active requests")
M.SetChatRect(ChatFrame1, 20, 100, 400, 180)

local gone = Deliver("CHAT_MSG_SAY", "lfm dm need tank", "Bob")
M.Tick(1)
for _, cf in ipairs(M.ChatFrames()) do
    local history = M.History(cf)
    for k = #history, 1, -1 do
        if history[k] and history[k].extra[5] and history[k].extra[5][11] == gone then
            table.remove(history, k)
        end
    end
end
n0 = #sent
M.Click(ButtonOf(gone))
check(#sent == n0 and Printed(L.SUFFIX_TOO_OLD) and Reserve.SlotsInUse() == 0,
    "a tracked line gone from every frame: nothing sent, too old, no slot kept")
M.Click(ButtonOf(99999))
check(#sent == n0, "a line the client forgot: nothing sent")

-- Blizzard copies lines into a new window without eventArgs and removes the originals.
local moved = Deliver("CHAT_MSG_WHISPER", "LFM DM", "Bob")
M.Tick(1)
local original = EntryFor(ChatFrame1, moved)
local copy = { message = original.message, r = 1, g = 1, b = 1, extra = table.pack(1, 0, 0), timestamp = 0 }
for k, e in ipairs(M.History(ChatFrame1)) do
    if e == original then
        table.remove(M.History(ChatFrame1), k)
        break
    end
end
for k, e in ipairs(M.History(ChatFrame3)) do
    if e == EntryFor(ChatFrame3, moved) then
        M.History(ChatFrame3)[k] = copy
        break
    end
end
M.Click(ButtonOf(moved))
local copied
for _, e in ipairs(M.History(ChatFrame3)) do
    if e and e.extra.n == 3 and not issecretvalue(e.message) and e.message:find(ButtonOf(moved), 1, true) then
        copied = e
    end
end
check(copied and Lines(copied.message) >= 2 and #Since(n0, "translate") == 1, "a moved line (no eventArgs) still gets its rows")

-- 200 transforms over secret entries and copies without eventArgs leave no hole.
local transformsBefore = ChatFrame1.transforms + ChatFrame3.transforms
for _ = 1, 100 do
    ChatButton.SetSuffix(moved, function(cf)
        local m = Reserve.Metrics(cf)
        return m and Reserve.Rows(m, 5, 2) or nil
    end)
    ChatButton.SetSuffix(moved, nil)
end
check(ChatFrame1.transforms + ChatFrame3.transforms - transformsBefore >= 200 and ChatFrame1.holes == 0
    and ChatFrame3.holes == 0, "no history hole after 200 transforms")
check(issecretvalue(M.History(ChatFrame1)[2].message), "secret entries never rewritten")
M.Tick(61)
M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. moved)

-- A clicked line that scrolls out of every frame: the watcher's periodic read-only scan frees its
-- slot (one CANCEL) and the watch stops, so the idle state has no ticker again.
local leaving = Deliver("CHAT_MSG_SAY", "anyone selling linen near the bank", "Ann")
M.Tick(1)
M.Click(ButtonOf(leaving))
local leavingReq, leavingSlot = Request.Get(leaving)
check(Request.Count() == 1 and Reserve.IsWatching(), "a request starts the watcher")
local beforeLeave = Snapshot()
n0 = #sent
M.Tick(60 * 20)
check(Request.Count() == 1 and Snapshot() == beforeLeave and #Since(n0, "cancel") == 0,
    "periodic scans keep a line still in the chat and change no entry")
for _, cf in ipairs(M.ChatFrames()) do
    local history = M.History(cf)
    for k = #history, 1, -1 do
        if history[k] and history[k].extra[5] and history[k].extra[5][11] == leaving then
            table.remove(history, k)
        end
    end
end
n0 = #sent
M.Tick(60 * 16)
local leftCancels = Since(n0, "cancel")
check(#leftCancels == 1 and leftCancels[1].reqId == leavingReq and leftCancels[1].slot == leavingSlot
    and Request.Count() == 0 and Reserve.SlotsInUse() == 0 and not Reserve.IsWatching(),
    "a line gone from the chat is freed by the next periodic scan (CANCEL), then the watch stops")

---------------------------------------------------------------------------------------------
section("prompt text")
check(G.PromptText("WTS " .. ITEM .. " x20 {skull} [x] a||b") == "WTS \2[Linen Cloth]\3 x20 \2{skull}\3 [x] a|b",
    "links and {...} marked, codes removed, typed brackets plain, || unescaped")
check(G.PromptText("a\2b\3c\1d") == "abcd", "control bytes typed in a message never pass for marks")
check(G.PromptText("{} {" .. string.rep("z", 33) .. "} {rt8}") == "{} {" .. string.rep("z", 33) .. "} \2{rt8}\3",
    "only {...} of 1-32 characters are marked")
local hangul12 = "{" .. string.rep("한", 12) .. "}"
check(G.PromptText(hangul12 .. " {" .. string.rep("é", 33) .. "}") == "\2" .. hangul12 .. "\3 {" .. string.rep("é", 33) .. "}",
    "{...} length counted in UTF-8 characters, not bytes")
check(G.PromptText("|cff00ff00|Hplayer:Bob|h[Bob]|h|r: |TInterface\\Icons\\x:0|t |A:atlas:12:12|a hi |Kq1|k")
    == "\2[Bob]\3:   hi ", "textures, atlases and protected names removed")
local long = string.rep("a", 1010) .. " " .. ITEM .. " tail"
local cutLong = G.PromptText(long)
check(cutLong == string.rep("a", 1010) .. " ", "1024 B cut never inside a mark")
local utf = G.PromptText(string.rep("é", 600))
check(#utf == 1024 and utf == string.rep("é", 512), "1024 B cut on a UTF-8 boundary")
local longLine = Deliver("CHAT_MSG_CHANNEL", long, "Spammer")
M.Tick(1)
n0 = #sent
M.Click(ButtonOf(longLine))
msgs = Since(n0, "translate")
check(#msgs == 1 and msgs[1].ok and #msgs[1].text <= 1024 and Count(msgs[1].text, "\2") == Count(msgs[1].text, "\3")
    and msgs[1].rows == 6, "a long line: text cut on its own, 6 rows reserved")
local longEntry = EntryFor(ChatFrame1, longLine).message
local longSlot = msgs[1].slot
check(Lines(longEntry) == 6 and Count(longEntry, "|TInterface\\Buttons\\WHITE8X8:") == 9 * 6
    and longEntry:find(RowsFor(ChatFrame1, longLine, longSlot, 6), 1, true),
    "6 reserved rows under the line, each a tag and a plate")
check(Bridge.TagMarkup(longSlot, 4, 1, 4, 14) == Bridge.TagMarkup(longSlot, 0, 1, 4, 14)
    and Bridge.TagMarkup(longSlot, 5, 1, 4, 14) == Bridge.TagMarkup(longSlot, 1, 1, 4, 14)
    and Bridge.TagMarkup(longSlot, 3, 1, 4, 14) ~= Bridge.TagMarkup(longSlot, 0, 1, 4, 14),
    "rows 4 and 5 carry tag rows 0 and 1 (the tag's row field has 2 bits)")
M.Tick(61)
M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. longLine)

section("rows estimate")
check(ExpectedK("gg", 290, 14, 1.35) == 1 and Reserve.EstimateRows(ChatFrame1, "gg", "fr") == 1,
    "a short Latin line reserves one row (the width ratio is not rounded up first)")
-- mock widths: 7 units per character against a 290-unit plate
check(Reserve.EstimateRows(ChatFrame1, string.rep("w", 50), "fr") == 2
    and Reserve.EstimateRows(ChatFrame1, string.rep("word ", 18), "en") == 3,
    "Latin into Latin: width in plates x1.35, rounded up")
local german = string.rep("wort ", 26)
check(ExpectedK(german, 290, 14, 1.25) == 4 and Reserve.EstimateRows(ChatFrame1, german, "fr") == 5,
    "a line that x1.25 sized to 4 rows (German into French, cut in game) now gets 5")
check(Reserve.EstimateRows(ChatFrame1, "gg", "ko") == 1 and Reserve.EstimateRows(ChatFrame1, "gg", "zh") == 1,
    "Latin into Korean or Chinese: 1 row")
check(Reserve.EstimateRows(ChatFrame1, string.rep("你", 50), "en") == 3, "Chinese into English: x2.2")
check(Reserve.MAX_ROWS == 6 and Reserve.EstimateRows(ChatFrame1, string.rep("word ", 200), "en") == 6,
    "never more than 6 rows")
check(Reserve.EstimateRows(ChatFrame1, string.rep("1", 70), "en") == 3, "no letters: the default factor (x1.3)")
check(Reserve.EstimateRows(ChatFrame1, "", "en") == 1, "an empty text reserves one row")
do
    -- WoW Forever drew inline textures 1.47 chat units per texture unit: the row must still fit.
    local realMeasure = Reserve.Measure
    Reserve.Measure = function(cf, text, width)
        local w, h, n, fs = realMeasure(cf, text, width)
        if text:find("^|T") then
            w = w * 1.47
        end
        return w, h, n, fs
    end
    local m = Reserve.Metrics(ChatFrame1)
    Reserve.Measure = realMeasure
    check(m and math.abs(m.tex - 1.47) < 1e-9 and m.W * m.tex * m.ppu >= 5
        and m.indent + (8 * m.W + m.plateW) * m.tex <= m.cw and m.H == math.floor(m.fontH / 1.47 + 0.5),
        "inline textures larger than chat units: blocks, plate and height are measured in texture units")
end

---------------------------------------------------------------------------------------------
section("chat lockdown")
local untracked = Deliver("CHAT_MSG_GUILD", "anyone for strat later tonight", "Lea")
M.Tick(1)
for k = 1, 1000 do
    ChatButton.Track(5000000 + k, "x", nil)   -- pushes older lines out of the tracked memory
end
check(not ChatButton.IsTracked(untracked), "a line can fall out of the tracked memory")
local tracked = Deliver("CHAT_MSG_PARTY", "pull after the next pat, cc the skull", "Tank")
M.Tick(1)
M.Restriction(5, 1)
M.SetLockdown(true)
check(ns.Restriction.Active(), "restriction active after Activating")
n0 = #sent
M.Click(ButtonOf(tracked))
msgs = Since(n0, "translate")
check(#msgs == 1 and Lines(EntryFor(ChatFrame1, tracked).message) >= 1, "a tracked line is translated during the restriction")
n0 = #sent
M.Click(ButtonOf(untracked))
check(#sent == n0 and EntryFor(ChatFrame1, untracked).message:find(L.TR_DEFERRED, 1, true),
    "an untracked line in lockdown waits (TR_DEFERRED row), nothing sent")
local s1 = Deliver("CHAT_MSG_RAID", "pull in 5", "Raidlead")
Deliver("CHAT_MSG_RAID", "heal the tank pls", "Raidlead")
Deliver("CHAT_MSG_RAID", "lust now", "Shaman")
Deliver("CHAT_MSG_RAID", "my own secret line", "Me", "Player-1-0001")
local hiddenSecret = Deliver("CHAT_MSG_CHANNEL", "hidden channel secret words", "Hidden2", nil, true)
M.Tick(1)
check(issecretvalue(EntryFor(ChatFrame1, s1).message), "secret lines displayed untouched")
M.SetLockdown(false)
-- plain text with a secret sender: the client skips the filters (no [T]), so it is offered too
deliveredText[#deliveredText + 1] = "half secret line from a stranger"
local halfSecret = M.Deliver("CHAT_MSG_SAY", "half secret line from a stranger", M.Secret())
M.Tick(1)
check(not ChatButton.IsTracked(halfSecret) and not EntryWith(ChatFrame1, ButtonOf(halfSecret)),
    "a line with a secret sender gets no [T] (filters skipped)")
M.Restriction(5, 0)
M.Tick(70)
msgs = Since(n0, "translate")
check(not ns.Restriction.Active() and #msgs == 1 and not EntryFor(ChatFrame1, untracked).message:find(L.TR_DEFERRED, 1, true)
    and Lines(EntryFor(ChatFrame1, untracked).message) >= 1, "the deferred click runs after the restriction")
local summary = L.LOCKDOWN_SUMMARY:format(4)
check(CountPrinted(summary) == 1, "one summary for the lines that arrived secret (own and hidden lines left out)")
local rLink
for _, line in ipairs(M.Printed()) do
    rLink = rLink or line:match("|H(addon:TranslateProbe:r:%d+:%d+)|h")
end
check(rLink ~= nil and Printed("[" .. L.LOCKDOWN_SHOW .. "]"), "the summary carries a [Show them] link")
M.Tick(60 * 30)
check(CountPrinted(summary) == 1 and #Since(n0, "translate") == 1, "the +30 s attempt does nothing more")
M.Click(rLink)
local reprinted = EntryWith(ChatFrame1, L.REPRINT_LINE:format("Raidlead", "pull in 5"))
check(reprinted and reprinted.message:find(ButtonOf(s1), 1, true), "[Show them] reprints the lines with a [T]")
check(EntryWith(ChatFrame1, L.REPRINT_LINE:format("Shaman", "lust now")) and not EntryWith(ChatFrame1, "my own secret line"),
    "every readable line reprinted, my own left out")
check(not EntryWith(ChatFrame1, "hidden channel secret words") and not ChatButton.IsTracked(hiddenSecret),
    "a secret line no chat frame shows is never reprinted")
local halfRow = EntryWith(ChatFrame1, L.REPRINT_LINE:format("?", "half secret line from a stranger"))
check(halfRow and halfRow.message:find(ButtonOf(halfSecret), 1, true), "the secret-sender line is reprinted with a [T]")
n0 = #sent
M.Click(ButtonOf(s1))
msgs = Since(n0, "translate")
reprinted = EntryWith(ChatFrame1, L.REPRINT_LINE:format("Raidlead", "pull in 5"))
check(#msgs == 1 and msgs[1].text == "pull in 5" and Lines(reprinted.message) >= 1, "a reprinted line translates")
local function CountEntries(fragment)
    local n = 0
    for _, e in ipairs(M.History(ChatFrame1)) do
        if e and not issecretvalue(e.message) and e.message:find(fragment, 1, true) then
            n = n + 1
        end
    end
    return n
end
local reprintCount = CountEntries(L.REPRINT_LINE:format("Raidlead", "pull in 5"))
M.Click(rLink)
check(reprintCount == 1 and CountEntries(L.REPRINT_LINE:format("Raidlead", "pull in 5")) == 1,
    "the batch is gone after one reprint")
slash("clear")

---------------------------------------------------------------------------------------------
section("glossary")
local sample = "LFM dm, need heal. Bgs later? lf2m Stock"
local gEn, gFr = G.Describe(sample, "en"), G.Describe(sample, "fr")
check(gEn and gEn:find("LFM", 1, true) and gEn:find("DM", 1, true) and gEn:find("BGS", 1, true)
    and gEn:find("LF2M", 1, true) and gEn:find("STOCK", 1, true), "abbreviations found whatever the case, plural and number")
check(gFr and gFr:find("Les Mortemines (The Deadmines)", 1, true) and gFr:find("Hache-Tripes", 1, true)
    and gEn:find("The Deadmines", 1, true) and not gEn:find("Mortemines", 1, true), "meanings follow the target")
check(G.Describe(sample) == G.Describe(sample, db.target), "without a target, the saved target")
check(G.Describe("Lfg", "en") and G.Describe("Lfg", "en"):find("LFG", 1, true), "mixed case matches")
check(G.Describe("|cffffffff|Hitem:1|h[Deadmines Key]|h|r please", "en") == nil, "item links and colors are not scanned")
check(G.Describe("bodmin", "en") == nil and G.Describe("dmg", "en") == nil, "whole words only")
check(G.Describe("son stéréo est abîmé, quelle stratégie, libérez-le, ein Stück", "en") == nil,
    "accented words are never cut into abbreviations")
check(G.Describe("I need abs", "en") == nil and G.Describe("dms", "en") == nil, "plural only where allowed")
check(G.Describe("strats", "en") ~= nil and G.Describe("hots", "en") ~= nil, "allowed plurals work")
local hot = G.Describe("HoT anyone?", "en")
check(hot and select(2, hot:gsub(" / ", "")) == 2, "HoT shows its three meanings")
check(not fr or (G.Describe("lfm", "fr") or ""):find("LFM : ", 1, true), "French separator on frFR")
local cheapFr, cheapEn = G.Extract("so cheap", "fr"), G.Extract("so cheap", "en")
check(#cheapFr == 1 and cheapFr[1][1] == "cheap" and cheapFr[1][2] == "pas cher" and #cheapEn == 0,
    "cheap = pas cher only in the fr prompt pairs")
check(G.Describe("so cheap", "fr") == nil and G.Describe("so cheap", "en") == nil, "false friends never shown")
check(G.Extract("lfm", "de")[1][2] == "looking for more" and G.Extract("lfm", "zh-TW")[1][2] == "looking for more",
    "other targets get English meanings")
check(G.Extract("dm", "fr")[1][2] == "Les Mortemines / Hache-Tripes / message privé", "several meanings joined with /")
slash("gloss add hot Hall of Thanes")
check(G.Describe("hot") == L.GLOSS_PAIR:format("HOT", "Hall of Thanes"), "custom entry replaces a built-in")
local otherTarget = fr and "en" or "fr"
check(type(db.glossary.hot) == "table" and db.glossary.hot[db.target] == "Hall of Thanes"
    and G.Extract("hot", otherTarget)[1][2] ~= "Hall of Thanes",
    "gloss add stores the meaning for the current target only (other targets keep the built-in)")
local ordered = G.Extract("lfm and hot", db.target)
check(ordered[1][1] == "hot" and ordered[1][2] == "Hall of Thanes" and ordered[2][1] == "lfm", "user entries first in the pairs")
slash("gloss del hot")
check(G.Describe("hot") == nil and db.glossary.hot == false and #G.Extract("hot", "fr") == 0,
    "deleting a built-in disables it, in the prompt too")
check(G.Describe("hots") == nil, "a disabled built-in also disables its plural")
slash("gloss add hot Hall of Thanes / chaud")
check(G.Describe("HOT") == L.GLOSS_PAIR:format("HOT", "Hall of Thanes / chaud"), "several custom meanings with /")
slash("gloss add zz top|secret")
check(G.Describe("zz") == L.GLOSS_PAIR:format("ZZ", "top/secret"), "pipes in custom text are neutralized")
slash("gloss del zz")
check(db.glossary.zz == nil, "deleting a custom entry removes it")
slash("gloss add été summer")
check(G.Describe("en été") == L.GLOSS_PAIR:format("été", "summer"), "accented custom keys work")
slash("gloss add qq QQ")
check(#G.Extract("qq", "en") == 0, "a pair whose meaning is the word itself is skipped")
slash("gloss del qq")
slash("gloss list")
check(Printed("HOT = Hall of Thanes / chaud"), "gloss list shows custom entries")
slash("gloss test lfm wsg")
check(Printed("WSG"), "gloss test prints the meanings")
check(#G.Extract("lfm lfg wts wtb wtt ah mats boe bop ml", "en") == 8, "at most 8 pairs")
local big = string.rep("m", 150)
for k = 1, 5 do
    slash("gloss add uu" .. k .. " " .. big)
end
check(#G.UserMeaning(db.glossary.uu1, db.target) == 120, "custom meanings are kept to 120 bytes")
local capped = G.Extract("uu1 uu2 uu3 uu4 uu5", db.target)
local used = 0
for _, p in ipairs(capped) do
    used = used + 2 + #p[1] + #p[2]
end
check(#capped == 4 and used <= 512, "pairs stop before 512 bytes")
local longKey = string.rep("k", 50)
slash("gloss add " .. longKey .. " long key")
check(#G.Extract(longKey, db.target)[1][1] == 48, "terms cut to 48 bytes")
for k = 1, 5 do
    slash("gloss del uu" .. k)
end
slash("gloss del " .. longKey)

---------------------------------------------------------------------------------------------
section("options panel")
local S = M.Settings()
local Options = ns.Options
local main = Options.Category()
check(main and #S.addOn == 1 and S.addOn[1] == main and main.kind == "vertical" and main.name == "TranslateProbe",
    "one add-on category registered without errors (vertical layout, named after the addon)")
local function Setting(key)
    return S.settings["TranslateProbe_" .. key]
end
check(Setting("target") and Setting("glossShow") and Setting("names") and Setting("mode") and Setting("display"),
    "settings: target, glossary row, names, bridge mode, then the conversation window (display...)")
local kinds, targetInit = {}, nil
for _, init in ipairs(S.initializers) do
    kinds[#kinds + 1] = init.kind .. ":" .. (init.setting and init.setting.variable:gsub("^TranslateProbe_", "")
        or init.name)
    if init.setting == Setting("target") then
        targetInit = init
    end
end
check(table.concat(kinds, ",", 1, math.min(4, #kinds)) == "dropdown:target,checkbox:glossShow,checkbox:names,dropdown:mode",
    "controls in order: target dropdown, two checkboxes, bridge mode dropdown (the conversation section follows)")
local values, labels = {}, {}
for i, o in ipairs(targetInit and targetInit.options() or {}) do
    values[i], labels[i] = o.value, o.label
end
check(table.concat(values, ",") == table.concat(ns.Lang.TARGETS, ",") and table.concat(labels, ",")
    == "English,Français,Deutsch,Español,Italiano,Português,Русский,한국어,简体中文,繁體中文",
    "target dropdown: every target, each named in its own language")
local home = db.target
Setting("target"):SetValue("de")
check(db.target == "de" and Setting("target"):GetValue() == "de", "the target dropdown sets the saved target")
Setting("target"):SetValue("xx")
check(db.target == "de", "an unknown target value is ignored")
Setting("glossShow"):SetValue(false)
check(db.glossShow == false and Setting("glossShow"):GetValue() == false, "glossary row checkbox")
Setting("glossShow"):SetValue(true)
Setting("names"):SetValue(true)
check(db.names == true and Setting("names"):GetValue() == true, "romanized names checkbox")
Setting("names"):SetValue(false)
Setting("mode"):SetValue("robust")
check(db.mode == "robust" and Bridge.ModeName() == "robust", "bridge mode dropdown")
Setting("mode"):SetValue("auto")
check(db.mode == "auto" and Bridge.ModeName() == "dense", "bridge mode back to auto")

local openedBefore = #S.opened
slash("")
slash("options")
check(#S.opened == openedBefore + 2 and S.opened[#S.opened] == main:GetID(),
    "/tpr and /tpr options open the panel on the addon's page")
local realOpen = Settings.OpenToCategory
Settings.OpenToCategory = nil
local helpBefore = CountPrinted(L.HELP_CLICK)
slash("")
check(Printed(L.OPTIONS_NA) and CountPrinted(L.HELP_CLICK) == helpBefore + 1,
    "without the settings panel, /tpr prints the command list")
Settings.OpenToCategory = realOpen

local gcat
for _, c in ipairs(S.categories) do
    if c.parent == main then
        gcat = c
    end
end
check(gcat and gcat.kind == "canvas" and gcat.name == L.GLOSS_PAGE, "glossary subpage registered (canvas layout)")
check(not (Options.Page() and Options.Page().built), "the glossary page is built only when first shown")
M.ShowCanvas(gcat)
local page = Options.Page()
check(page and page.built and page.title:GetText() == L.GLOSS_PAGE_TITLE:format("Deutsch"),
    "glossary page shown: its title names the target in its own language")
local function RowFor(key)
    Options.RefreshGlossary()
    for i, entry in ipairs(page.entries) do
        if entry.key == key then
            page.offset = i - 1
            Options.RefreshGlossary()
            for _, row in ipairs(page.rows) do
                if row.entry and row.entry.key == key and row:IsShown() then
                    return row
                end
            end
        end
    end
end
local function KeyCount(t)
    local n = 0
    for _ in pairs(t) do
        n = n + 1
    end
    return n
end
local lfmRow = RowFor("lfm")
check(lfmRow and lfmRow.edit:GetText() == "looking for more" and lfmRow.mark:GetText() == L.GLOSS_MARK_PIVOT
    and lfmRow.button:GetText() == L.GLOSS_BTN_DISABLE,
    "de: a built-in shows the English meaning it sends, marked as English, with Disable")
page.addKey:SetText("wb")
page.addMeaning:SetText("Weltboss")
M.Press(page.addMeaning, "OnEnterPressed")
check(type(db.glossary.wb) == "table" and db.glossary.wb.de == "Weltboss" and KeyCount(db.glossary.wb) == 1,
    "Add stores the new abbreviation with its meaning for the current target")
check(page.addKey:GetText() == "" and page.addMeaning:GetText() == "", "the add row is emptied")
local wbRow
for _, row in ipairs(page.rows) do
    if row.entry and row.entry.key == "wb" and row:IsShown() then
        wbRow = row
    end
end
check(wbRow and wbRow.edit:GetText() == "Weltboss" and wbRow.mark:GetText() == L.GLOSS_MARK_USER
    and wbRow.button:GetText() == L.GLOSS_BTN_DELETE, "the new entry is in view, marked as the user's, with Delete")
check(G.Extract("need wb", "de")[1][2] == "Weltboss" and #G.Extract("wb", "fr") == 0 and G.Describe("wb", "en") == nil,
    "the meaning serves de only")

Setting("target"):SetValue("fr")
wbRow = RowFor("wb")
check(page.title:GetText() == L.GLOSS_PAGE_TITLE:format("Français") and wbRow and wbRow.edit:GetText() == "",
    "switching the target refreshes the page: the abbreviation stays, its meaning is empty in fr")
wbRow.edit:SetText("boss du monde")
M.Press(wbRow.edit, "OnEnterPressed")
check(db.glossary.wb.de == "Weltboss" and db.glossary.wb.fr == "boss du monde" and G.Extract("wb", "fr")[1][2] == "boss du monde"
    and G.Extract("wb", "de")[1][2] == "Weltboss", "a meaning typed for fr is stored next to the de one")
lfmRow = RowFor("lfm")
check(lfmRow.edit:GetText() == "cherche des membres" and lfmRow.mark:GetText() == "", "fr: a built-in shows its French meaning")
lfmRow.edit:SetText("cherche du monde")
M.Press(lfmRow.edit, "OnEnterPressed")
lfmRow = RowFor("lfm")
check(type(db.glossary.lfm) == "table" and db.glossary.lfm.fr == "cherche du monde" and KeyCount(db.glossary.lfm) == 1
    and lfmRow.mark:GetText() == L.GLOSS_MARK_EDITED, "editing a built-in stores a meaning for this target, marked edited")
check(G.Extract("lfm", "fr")[1][2] == "cherche du monde" and G.Extract("lfm", "de")[1][2] == "looking for more"
    and G.Describe("lfm", "fr") == L.GLOSS_PAIR:format("LFM", "cherche du monde"), "other targets keep the built-in meaning")
lfmRow.edit:SetText("")
M.Press(lfmRow.edit, "OnEnterPressed")
check(db.glossary.lfm == nil and G.Extract("lfm", "fr")[1][2] == "cherche des membres", "an empty meaning gives the built-in one back")
lfmRow = RowFor("lfm")
lfmRow.edit:SetText("oops")
M.Press(lfmRow.edit, "OnEscapePressed")
check(lfmRow.edit:GetText() == "cherche des membres" and db.glossary.lfm == nil, "Escape restores the meaning, nothing stored")
lfmRow.button:Click()
lfmRow = RowFor("lfm")
check(db.glossary.lfm == false and #G.Extract("lfm", "fr") == 0 and not lfmRow.edit:IsEnabled()
    and lfmRow.button:GetText() == L.GLOSS_BTN_ENABLE and lfmRow.mark:GetText() == L.GLOSS_MARK_DISABLED,
    "Disable turns a built-in off, in the prompt too")
lfmRow.button:Click()
check(db.glossary.lfm == nil and G.Extract("lfm", "fr")[1][2] == "cherche des membres", "Enable turns it back on")

db.glossary.oldk = "old meaning"   -- the format before per-target meanings
local oldRow = RowFor("oldk")
check(oldRow and oldRow.edit:GetText() == "old meaning" and oldRow.mark:GetText() == L.GLOSS_MARK_USER .. ", " .. L.GLOSS_MARK_SHARED
    and G.Extract("oldk", "de")[1][2] == "old meaning" and G.Extract("oldk", "ko")[1][2] == "old meaning"
    and G.Describe("oldk", "en") == L.GLOSS_PAIR:format("OLDK", "old meaning"),
    "an old one-string entry still serves every target, marked as such")
oldRow.edit:SetText("nouveau sens")
M.Press(oldRow.edit, "OnEnterPressed")
check(type(db.glossary.oldk) == "table" and db.glossary.oldk["*"] == "old meaning" and db.glossary.oldk.fr == "nouveau sens"
    and G.Extract("oldk", "fr")[1][2] == "nouveau sens" and G.Extract("oldk", "de")[1][2] == "old meaning",
    "editing it for fr keeps the old meaning for the other targets")
oldRow = RowFor("oldk")
oldRow.edit:SetText("")
M.Press(oldRow.edit, "OnEnterPressed")
check(db.glossary.oldk == "old meaning", "clearing the fr meaning gives the old string back, unchanged")
oldRow = RowFor("oldk")
oldRow.edit:SetText("")
M.Press(oldRow.edit, "OnEnterPressed")
oldRow = RowFor("oldk")
check(type(db.glossary.oldk) == "table" and db.glossary.oldk["*"] == "old meaning" and db.glossary.oldk.fr == ""
    and oldRow.edit:GetText() == "" and #G.Extract("oldk", "fr") == 0 and G.Describe("oldk", "fr") == nil
    and G.Extract("oldk", "de")[1][2] == "old meaning" and G.Extract("oldk", "ko")[1][2] == "old meaning",
    "clearing an all-languages meaning hides it for this target only, the others keep it")
oldRow.edit:SetText("sens fr")
M.Press(oldRow.edit, "OnEnterPressed")
check(db.glossary.oldk.fr == "sens fr" and db.glossary.oldk["*"] == "old meaning", "a meaning typed over the hidden one replaces it")
oldRow = RowFor("oldk")
oldRow.edit:SetText("")
M.Press(oldRow.edit, "OnEnterPressed")
check(db.glossary.oldk == "old meaning", "clearing it again gives the old string back")
slash("target de")
slash("gloss add lfm cherche du monde")
slash("target fr")
local listed = L.GLOSS_ENTRY:format("LFM", "cherche des membres")
local listedBefore = CountPrinted(listed)
slash("gloss list")
check(CountPrinted(listed) == listedBefore + 1,
    "gloss list: a built-in edited for another target shows the built-in meaning this target sends")
slash("gloss del lfm")
G.Enable("lfm")

-- typing in one row, then clicking a button: the typed text is saved first, to its own entry
lfmRow = RowFor("lfm")
local libRow
for i, row in ipairs(page.rows) do
    if row == lfmRow then
        libRow = page.rows[i + 1]
    end
end
check(libRow and libRow.entry and libRow.entry.key == "lib", "the row below LFM is LIB")
lfmRow.edit:SetFocus()
lfmRow.edit:SetText("cherche du monde")
libRow.button:Click()
check(type(db.glossary.lfm) == "table" and db.glossary.lfm.fr == "cherche du monde" and db.glossary.lib == false
    and not lfmRow.edit:HasFocus(), "a click on another row's button saves the text being typed first")
RowFor("lib").button:Click()
lfmRow = RowFor("lfm")
lfmRow.edit:SetFocus()
lfmRow.edit:SetText("cherche encore")
page.addKey:SetText("zzq")
page.addMeaning:SetText("essai")
page.addButton:Click()
check(db.glossary.lfm.fr == "cherche encore" and type(db.glossary.zzq) == "table" and db.glossary.zzq.fr == "essai"
    and db.glossary.lib == nil, "Add also saves the text being typed in a row first")
slash("gloss del zzq")
lfmRow = RowFor("lfm")
lfmRow.edit:SetText("")
M.Press(lfmRow.edit, "OnEnterPressed")
check(db.glossary.lfm == nil and db.glossary.zzq == nil, "test entries removed")

wbRow = RowFor("wb")
wbRow.edit:SetFocus()
wbRow.edit:SetText("boss mondial")
M.HideCanvas(gcat)
check(db.glossary.wb.fr == "boss mondial", "leaving the page saves the meaning being typed")
M.ShowCanvas(gcat)
page.addKey:SetText("two words")
page.addMeaning:SetText("x")
page.addButton:Click()
check(page.status:GetText() == L.GLOSS_BAD_KEY and db.glossary["two words"] == nil and db.glossary.two == nil,
    "an abbreviation must be one word")
page.addKey:SetText("capk")
page.addMeaning:SetText("   ")
page.addButton:Click()
check(page.status:GetText() == L.GLOSS_NEED_MEANING and db.glossary.capk == nil, "a meaning is required")
page.addMeaning:SetText(string.rep("é", 100) .. "|x")
page.addButton:Click()
check(type(db.glossary.capk) == "table" and db.glossary.capk.fr == string.rep("é", 60) and page.status:GetText() == "",
    "panel meanings are kept to 120 bytes, cut on a UTF-8 boundary")
page.offset = 0
Options.RefreshGlossary()
local total = #page.entries
check(total > 12 and page.range:GetText() == L.GLOSS_RANGE:format(1, 12, total) and not page.prev:IsEnabled()
    and page.next:IsEnabled(), "12 rows at a time, Previous off at the top")
page.next:Click()
check(page.offset == 12 and page.prev:IsEnabled() and page.rows[1].entry == page.entries[13], "Next shows the following rows")
page.frame.scripts.OnMouseWheel(page.frame, 1)
check(page.offset == 9 and page.rows[1].entry == page.entries[10], "the mouse wheel scrolls 3 rows")

local notifiedBefore = #S.notified
slash("show off")
slash("show on")
slash("target de")
local notified = {}
for k = notifiedBefore + 1, #S.notified do
    notified[S.notified[k]] = true
end
check(notified.TranslateProbe_glossShow and notified.TranslateProbe_target
    and page.title:GetText() == L.GLOSS_PAGE_TITLE:format("Deutsch"), "commands update the panel")
slash("gloss del wb")
slash("gloss del capk")
slash("gloss del oldk")
check(db.glossary.wb == nil and db.glossary.capk == nil and db.glossary.oldk == nil, "user entries deleted")
Setting("target"):SetValue(home)
M.HideCanvas(gcat)

---------------------------------------------------------------------------------------------
section("commands")
slash("target xx")
check(db.target == (fr and "fr" or "en") and Printed(L.TARGET_IS:format(fr and "FR" or "EN",
    "en, fr, de, es, it, pt, ru, ko, zh, zh-TW")), "unknown target ignored, targets listed")
slash("target zh-tw")
check(db.target == "zh-TW", "target codes are case-insensitive")
slash("target " .. (fr and "fr" or "en"))
slash("mode robust")
check(db.mode == "robust" and Bridge.ModeName() == "robust" and Printed(L.MODE_SET:format("robust", "robust")), "mode robust")
M.SetCVar("colorblindSimulator", "2")
slash("mode auto")
check(db.mode == "auto" and Bridge.ModeName() == "mono", "mode auto picks mono with the colourblind filter")
M.SetCVar("colorblindSimulator", "0")
slash("mode auto")
check(Bridge.ModeName() == "dense", "mode auto is dense without it")
M.SetCVar("colorblindSimulator", "1")
check(db.mode == "auto" and Bridge.ModeName() == "mono", "mode auto follows the filter turned on mid-session")
M.SetCVar("colorblindSimulator", "0")
check(Bridge.ModeName() == "dense", "and back to dense when it is turned off")
slash("mode robust")
M.SetCVar("colorblindSimulator", "1")
check(Bridge.ModeName() == "robust", "an explicit mode ignores the filter")
M.SetCVar("colorblindSimulator", "0")
slash("mode auto")
slash("mode turbo")
check(db.mode == "auto" and Printed(L.HELP_MODE), "unknown mode refused")
slash("show off")
check(db.glossShow == false and Printed(L.SHOW_OFF), "show off")
slash("names off")
local plain = Deliver("CHAT_MSG_SAY", "lfm dm", "Bob")
M.Tick(1)
M.Click(ButtonOf(plain))
local plainK = Request.Get(plain) and select(3, Request.Get(plain))
check(plainK and Lines(EntryFor(ChatFrame1, plain).message) == plainK, "without the glossary row: only the K reserved rows")
slash("show on")
check(db.glossShow == true and Printed(L.SHOW_ON), "show on")

n0 = #sent
slash("test")
msgs = Since(n0)
local testTr = Since(n0, "translate")[1]
check(msgs[1] and msgs[1].kind == "test" and Printed(L.TEST_SENT:format(msgs[1].nonce)), "test: TEST message first")
local sampleItem = db.target == "en" and "\2[Étoffe de lin]\3" or "\2[Linen Cloth]\3"
check(testTr and testTr.flags == 3 and testTr.text:find(sampleItem, 1, true) and testTr.text:find("lf2m", 1, true),
    "test: a sample request with noCache, a real item link and lf2m")
local sampleLine = EntryWith(ChatFrame1, ButtonOf(900000001))
check(sampleLine and Lines(sampleLine.message) >= testTr.rows, "test: the sample line gets its rows")
local trCount = #Since(0, "translate")
slash("test rows")
check(#Since(0, "translate") == trCount, "test rows: no TRANSLATE")
local rowsLine = EntryWith(ChatFrame1, ButtonOf(900000002))
local function Row127(r)
    return ChatButton.SuffixLinks(900000002, { Bridge.TagMarkup(127, r, ns.sessionId % 2, 4, 14) }):sub(1, -3)
end
check(rowsLine and rowsLine.message:find(Row127(0), 1, true) and Lines(rowsLine.message) == 3
    and rowsLine.message:find(Row127(2), 1, true),
    "test rows: 3 rows on slot 127")
check(Printed(L.TEST_ROWS_SHOWN:format(1)) and Printed(L.TEST_ROWS_METRICS:format(1.40625, 14, 4, 14, 290, 400)),
    "test rows: metrics printed")
check(CountPrinted(L.TEST_ROWS_HEIGHT:format("a", 14, 1)) == 1 and CountPrinted(L.TEST_ROWS_HEIGHT:format("a|n ", 28, 2)) == 1,
    "test rows: string heights printed")
n0 = #sent
slash("test lockdown")
check(Printed(L.TEST_LOCKDOWN_STATE:format(L.NO, L.NO)) and Printed(L.TEST_LOCKDOWN_RECT:format(L.NO))
    and Printed(L.TEST_LOCKDOWN_LINE:format(L.NO)) and Since(n0, "test")[1] ~= nil, "test lockdown: readable state, TEST sent")
M.SetRectSecret(ChatFrame1, true)
M.SetLockdown(true)
slash("test lockdown")
check(Printed(L.TEST_LOCKDOWN_STATE:format(L.YES, L.NO)) and Printed(L.TEST_LOCKDOWN_RECT:format(L.YES))
    and Printed(L.TEST_LOCKDOWN_LINE:format(L.YES)), "test lockdown: secret rectangles reported")
check(#Reserve.View() == 0, "a secret chat rectangle is left out of VIEW")
M.SetLockdown(false)
M.SetRectSecret(ChatFrame1, false)
slash("info")
check(Printed("1.60.1") and Printed(L.INFO_VERSION:format("0.4.0", ns.ADDON_BUILD, ns.sessionId))
    and Printed(L.INFO_FRAME:format("ChatFrame1", L.YES, L.YES)), "info prints version, client and chat frames")
check(Printed(L.INFO_REQUESTS:format(Request.Count(), Reserve.SlotsInUse())), "info prints requests and slots")
slash("debug")
check(Printed((L.DEBUG_REQUESTS:match("^[^%%]*"))) and Printed((L.DEBUG_SLOTS:match("^[^%%]*"))), "debug prints the counters")
slash("app")
check(Printed(L.APP_1) and Printed(L.APP_4), "app explains the helper")
slash("help")
check(Printed(L.HELP_CLICK) and Printed(L.HELP_TEST_ROWS) and Printed(L.HELP_OPTIONS) and Printed(L.HELP_HELP),
    "help lists the commands (options included) and the click hint")
slash("names on")
check(db.names == true, "names option on")
slash("names off")
slash("translit привет")
check(Printed("ro"), "translit command spells text")
slash("translit hello")
check(Printed(L.TRANSLIT_NA), "translit says when there is nothing to spell")
n0 = #sent
slash("clear")
local anyRow = false
for _, cf in ipairs(M.ChatFrames()) do
    for _, e in ipairs(M.History(cf)) do
        if e and not issecretvalue(e.message) and e.message:find(SUFFIX_START .. ns.session .. ":", 1, true) then
            anyRow = true
        end
    end
end
check(Since(n0, "clear")[1] ~= nil and not anyRow and Request.Count() == 0 and Reserve.SlotsInUse() == 0
    and Printed(L.CLEARED), "clear sends CLEAR and removes every row of this session")

---------------------------------------------------------------------------------------------
section("no chat tab display")
local tabInToc = false
for _, file in ipairs(ADDON_FILES) do
    if file == "Tab.lua" then
        tabInToc = true
    end
end
check(not tabInToc and ns.Tab == nil, "Tab.lua is gone from the TOC and from the namespace")
check(L.TAB_NAME == nil and L.DISPLAY_TAB == nil and L.HELP_SHOW_TAB == nil and L.OPT_DISPLAY == nil,
    "no chat tab string left")
local helpMentionsTab = false
for key, value in pairs(L) do
    if type(value) == "string" and key:find("^HELP_") and value:find("tab|inline", 1, true) then
        helpMentionsTab = true
    end
end
check(not helpMentionsTab, "the help no longer offers /tpr show tab")
local showBefore = db.glossShow
slash("show tab")
check(db.display == "inline" and db.glossShow == showBefore, "/tpr show tab changes nothing (show takes on|off)")
local trackedLine = Deliver("CHAT_MSG_SAY", "tab check line for a click", "Tabby")
M.Tick(1)
M.Click(ButtonOf(trackedLine))
check(select("#", Request.Get(trackedLine)) == 5 and EntryFor(ChatFrame1, trackedLine).message:find(SUFFIX_START, 1, true),
    "a click puts the rows under the line")
M.Tick(61)
M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. trackedLine)

---------------------------------------------------------------------------------------------
section("slots")
-- chat lines own slots 0..99 (Reserve.CHAT_SLOTS); 100..125 are the conversation window's, 126 its
-- reply, 127 the TEST message
check(Reserve.CHAT_SLOTS == 100 and Reserve.WIN_FIRST == 100 and Reserve.WIN_LAST == 125 and Reserve.REPLY_SLOT == 126,
    "slot ranges: chat 0..99, window 100..125, reply 126")
local lines = {}
for k = 1, Reserve.CHAT_SLOTS do
    lines[k] = Deliver("CHAT_MSG_CHANNEL", "selling stuff number " .. k, "Seller" .. k)
end
M.Tick(1)
local slotsSeen, reqOf = {}, {}
n0 = #sent
for k = 1, Reserve.CHAT_SLOTS do
    M.Click(ButtonOf(lines[k]))
    local reqId, slot = Request.Get(lines[k])
    reqOf[k] = reqId
    if slot then
        slotsSeen[slot] = true
    end
end
local distinct = 0
for s = 0, 127 do
    distinct = distinct + (slotsSeen[s] and 1 or 0)
end
check(Request.Count() == 100 and Reserve.SlotsInUse() == 100 and distinct == 100 and not slotsSeen[100],
    "100 requests hold the 100 chat slots (0..99)")
before = Snapshot()
check(Reserve.Scan() == 0 and Snapshot() == before, "the read-only scan frees nothing in use and changes no entry or timestamp")
local expiredBefore = Request.Counters().expired
local extraLine = Deliver("CHAT_MSG_CHANNEL", "one more line", "Late")
M.Tick(1)
local expiredSlot = select(2, Request.Get(lines[1]))
n0 = #sent
M.Click(ButtonOf(extraLine))
local cancels = Since(n0, "cancel")
check(Request.Counters().expired == expiredBefore + 1 and #cancels == 1 and cancels[1].reqId == reqOf[1]
    and select(2, Request.Get(extraLine)) == expiredSlot, "all 100 on screen: the oldest request expires for the new one")
for k = 1, 40 do
    Deliver("CHAT_MSG_CHANNEL", "pushing the old lines out " .. k, "Pusher")
end
local last = Deliver("CHAT_MSG_CHANNEL", "a new line after the scroll", "Newbie")
M.Tick(1)
local goneActive = 0
for k = 2, Reserve.CHAT_SLOTS do
    if not EntryFor(ChatFrame1, lines[k]) and not EntryFor(ChatFrame3, lines[k]) then
        goneActive = goneActive + 1
    end
end
check(goneActive > 0 and Reserve.SlotsInUse() == 100, "lines that left the chat keep their slot until a scan")
n0 = #sent
M.Click(ButtonOf(last))
cancels = Since(n0, "cancel")
check(Request.Counters().expired == expiredBefore + 1 and #cancels == goneActive
    and Reserve.SlotsInUse() == 100 - goneActive + 1 and Request.Count() == 100 - goneActive + 1,
    "the scan frees the slots of lines gone from the chat (CANCEL each), nothing expires")
check(AllOk(Since(0)), "every message sent this session is well formed")
slash("clear")

---------------------------------------------------------------------------------------------
section("mock: conversation window scaffolding")
-- The mock pieces the window and the reply composer rely on, checked once here so a broken mock
-- shows up as a mock failure, not as a window failure. (Each block of checks added below runs in
-- a function of its own: the main chunk is close to Lua's limit of 200 local variables.)
do
    local function Run()
        local fails = {}
        local function expect(cond, what)
            if not cond then
                fails[#fails + 1] = what
            end
        end
        local win = CreateFrame("Frame", "TPMockWin", UIParent, "PortraitFrameTemplate")
        expect(win.CloseButton and win.TitleContainer.TitleText and win:GetPortrait() and win.NineSlice, "portrait frame parts")
        win:SetTitle("Title")
        expect(win:GetTitleText():GetText() == "Title", "SetTitle")
        win:SetPortraitToClassIcon("MAGE")
        expect(win:GetPortrait().texCoord ~= nil, "class portrait")
        expect(not pcall(win.SetPortraitToClassIcon, win, "NOTACLASS"), "unknown class errors as on the client")
        win:SetSize(440, 540)
        win:SetPoint("CENTER", UIParent, "CENTER", -300, 60)
        local inset = CreateFrame("Frame", nil, win, "InsetFrameTemplate")
        inset:SetPoint("TOPLEFT", win, "TOPLEFT", 8, -76)
        inset:SetPoint("BOTTOMRIGHT", win, "BOTTOMRIGHT", -8, 94)
        expect(math.abs(inset:GetWidth() - 424) < 1e-6 and math.abs(inset:GetHeight() - (540 - 76 - 94)) < 1e-6,
            "two-point anchors set the size")
        local tabs = {}
        for k = 1, 3 do
            tabs[k] = CreateFrame("Button", nil, win, "PanelTopTabButtonTemplate")
            tabs[k]:SetText("Tab " .. k)
            tabs[k]:SetID(k)
        end
        expect(win.Tabs and #win.Tabs == 3 and win.Tabs[2] == tabs[2], "tabs join their parent's Tabs")
        PanelTemplates_SetNumTabs(win, 3)
        PanelTemplates_SetTab(win, 2)
        expect(not tabs[2]:IsEnabled() and tabs[1]:IsEnabled(), "the selected tab is disabled (no OnClick on it)")
        local clicked, upped = {}, {}
        for k = 1, 3 do
            tabs[k]:RegisterForClicks("LeftButtonUp", "RightButtonUp", "MiddleButtonUp")
            tabs[k]:SetScript("OnClick", function(_, button) clicked[#clicked + 1] = k .. button end)
            tabs[k]:SetScript("OnMouseUp", function(_, button) upped[#upped + 1] = k .. button end)
        end
        M.ClickButton(tabs[1], "MiddleButton")
        M.ClickButton(tabs[2], "MiddleButton")
        expect(clicked[1] == "1MiddleButton" and #clicked == 1 and upped[2] == "2MiddleButton",
            "middle-click: OnClick on an enabled tab, only OnMouseUp on the selected one")
        tabs[3]:RegisterForClicks("LeftButtonUp")
        M.ClickButton(tabs[3], "MiddleButton")
        expect(#clicked == 1, "a button not registered for a mouse button gets no OnClick")
        PanelTemplates_TabResize(tabs[1], 0, nil, 72, 130)
        expect(tabs[1]:GetWidth() >= 72 and tabs[1]:GetWidth() <= 130, "PanelTemplates_TabResize within bounds")
        M.SetTemplateMissing("TabSystemTopButtonTemplate", true)
        expect(C_XMLUtil.GetTemplateInfo("TabSystemTopButtonTemplate") == nil
            and not pcall(CreateFrame, "Button", nil, win, "TabSystemTopButtonTemplate"), "a missing template")
        M.SetTemplateMissing("TabSystemTopButtonTemplate", false)
        expect(C_XMLUtil.GetTemplateInfo("PanelTopTabButtonTemplate").height == 32, "template info")
        -- font objects
        local font = CreateFont("TPMockConvFont")
        font:CopyFontObject(ChatFontNormal)
        local path, size = font:GetFont()
        expect(path == "Fonts\\ARIALN.TTF" and size == 14, "CreateFont copies ChatFontNormal")
        local fs = win:CreateFontString(nil, "ARTWORK", "TPMockConvFont")
        fs:SetText("abcd")
        font:SetFont(path, 18, "")
        expect(fs:GetUnboundedStringWidth() == 0.5 * 18 * 4, "a font object change reaches its font strings")
        -- the reply box: focus, typing, pasting, keys
        local box = CreateFrame("EditBox", nil, win, "InputBoxInstructionsTemplate")
        box:SetAutoFocus(false)
        box:SetMaxBytes(1024)
        local changes = {}
        box:SetScript("OnTextChanged", function(self, userInput)
            InputBoxInstructions_OnTextChanged(self)
            changes[#changes + 1] = { userInput, IsControlKeyDown(), self:GetText() }
        end)
        local chars = 0
        box:SetScript("OnChar", function() chars = chars + 1 end)
        expect(box.Instructions:IsShown() and not M.Paste(box, "x"), "no paste without focus")
        box:SetFocus()
        expect(M.Focused() == box and M.FocusLog()[#M.FocusLog()].box == box, "focus recorded")
        M.Type(box, "hé")
        expect(box:GetText() == "hé" and chars == 2 and #changes == 2 and changes[2][1] == true and changes[2][2] == false
            and not box.Instructions:IsShown(), "typing: OnTextChanged(true) per character, no Ctrl, OnChar")
        box:SetText("")
        expect(changes[3][1] == false, "SetText fires OnTextChanged(false)")
        M.Paste(box, "Brechen wir auf?")
        local last = changes[#changes]
        expect(last[1] == true and last[2] == true and last[3] == "Brechen wir auf?" and chars == 2 and not IsControlKeyDown(),
            "paste: one OnTextChanged(true) with Ctrl down, no OnChar, Ctrl released")
        local before = #changes
        M.Type(box, "abc", { burst = true })
        expect(#changes == before + 1 and chars == 5, "an IME burst: one OnTextChanged(true), OnChar per character")
        local pressed = {}
        box:SetScript("OnEnterPressed", function() pressed[#pressed + 1] = "enter" end)
        box:SetScript("OnArrowPressed", function(_, key) pressed[#pressed + 1] = key end)
        box:SetScript("OnTabPressed", function() pressed[#pressed + 1] = IsShiftKeyDown() and "shift-tab" or "tab" end)
        M.Key(box, "ENTER")
        M.Key(box, "UP")
        M.Key(box, "TAB", { shift = true })
        expect(table.concat(pressed, ",") == "enter,UP,shift-tab", "keys: Enter, arrows, Shift+Tab")
        win:Hide()
        expect(M.Focused() == nil, "hiding the window takes the focus away from its box")
        win:Show()
        -- sending, echoes, blocked sends
        M.SetChannels({ [2] = "Trade - City" })
        expect(GetChannelName("Trade - City") == 2 and GetChannelName("Trade") == 2 and GetChannelName(5) == 0,
            "channels by name, base name and number")
        local send = CreateFrame("Button", nil, win, "UIPanelButtonTemplate")
        send:SetText(SAY)
        send:SetScript("OnClick", function() C_ChatInfo.SendChatMessage("hi", "CHANNEL", nil, GetChannelName("Trade")) end)
        local nSent = #M.SentChat()
        send:Click()
        local rec = M.SentChat()[nSent + 1]
        expect(rec and rec.blocked and not rec.hardware, "a channel send outside a hardware event is blocked")
        M.ClickButton(send)
        rec = M.SentChat()[nSent + 2]
        expect(rec and not rec.blocked and rec.hardware and rec.target == 2, "a channel send from a click goes out")
        local seen = {}
        local echoFrame = CreateFrame("Frame")
        echoFrame:RegisterEvent("CHAT_MSG_CHANNEL")
        echoFrame:RegisterEvent("CHAT_MSG_WHISPER_INFORM")
        echoFrame:RegisterEvent("CHAT_MSG_SYSTEM")
        echoFrame:SetScript("OnEvent", function(_, event, text, _, _, _, _, _, _, index, _, _, _, guid)
            seen[#seen + 1] = { event = event, text = text, index = index, guid = guid }
        end)
        M.EchoSent(rec)
        expect(seen[1] and seen[1].event == "CHAT_MSG_CHANNEL" and seen[1].guid == UnitGUID("player") and seen[1].index == 2,
            "the echo comes back from the player on the channel")
        C_ChatInfo.SendChatMessage("hello", "WHISPER", nil, "Neco-OtherRealm")
        M.EchoSent()
        M.PlayerNotFound("Neco-OtherRealm")
        expect(seen[2].event == "CHAT_MSG_WHISPER_INFORM" and seen[3].event == "CHAT_MSG_SYSTEM"
            and seen[3].text == ERR_CHAT_PLAYER_NOT_FOUND_S:format("Neco-OtherRealm"), "whisper echo, player not found")
        echoFrame:UnregisterAllEvents()
        local bnLine = M.Deliver("CHAT_MSG_BN_WHISPER", "bn line", "|Kq1|k", nil, true, { bnID = 7 })
        expect(C_ChatInfo.GetChatLineSenderGUID(bnLine) == nil, "a Battle.net line has no player GUID")
        local chanLine = M.Deliver("CHAT_MSG_CHANNEL", "chan line", "Brunhilde-Realm", "Player-1-0042", true,
            { channelString = "2. Trade - City", channelIndex = 2, channelBase = "Trade - City", class = "PRIEST" })
        expect(select(2, GetPlayerInfoByGUID("Player-1-0042")) == "PRIEST" and chanLine > bnLine, "sender class by GUID")
        M.SetLockdown(true)
        C_BattleNet.SendWhisper(7, "x")
        M.SetLockdown(false)
        expect(M.SentChat()[#M.SentChat()].blocked, "sends during the chat lockdown are blocked")
        -- combat, groups, sounds, tooltips, pixels, atlases, nine-slice
        local regen = {}
        echoFrame:RegisterEvent("PLAYER_REGEN_DISABLED")
        echoFrame:SetScript("OnEvent", function(_, event) regen[#regen + 1] = event end)
        M.SetCombat(true)
        expect(InCombatLockdown() and regen[1] == "PLAYER_REGEN_DISABLED", "combat start")
        M.SetCombat(false)
        echoFrame:UnregisterAllEvents()
        M.SetGroup({ party = true, guild = true })
        expect(IsInGroup() and not IsInRaid() and IsInGuild() and not C_GuildInfo.IsGuildOfficer(), "group state")
        M.SetGroup({ party = false, guild = false })
        PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
        expect(M.Sounds()[#M.Sounds()] == SOUNDKIT.IG_CHARACTER_INFO_TAB, "sounds recorded")
        send:SetMotionScriptsWhileDisabled(true)
        send:Disable()
        send.tooltipText = "why"
        M.Hover(send)
        expect(GameTooltip:IsOwned(send) and GameTooltip.lines[1] == "why", "a disabled button with motion scripts shows its tooltip")
        M.Leave()
        expect(not GameTooltip:IsShown(), "leaving hides it")
        local _, physH = GetPhysicalScreenSize()
        expect(math.abs(PixelUtil.GetNearestPixelSize(1, 1) - Round(physH / 768) * 768 / physH) < 1e-9,
            "PixelUtil rounds to physical pixels")
        expect(C_Texture.GetAtlasInfo("ChatBubble-Tail") and C_Texture.GetAtlasInfo("no-such-atlas") == nil, "atlas info")
        local bubble = CreateFrame("Frame", nil, win)
        NineSliceUtil.ApplyLayoutByName(bubble, "ChatBubble")
        expect(bubble.TopLeftCorner and bubble.Center and bubble.TopLeftCorner:GetAtlas() == "ChatBubble-NineSlice-CornerTopLeft",
            "the ChatBubble nine-slice layout")
        M.SetLayoutMissing("ChatBubble", true)
        expect(NineSliceUtil.GetLayout("ChatBubble") == nil and not pcall(NineSliceUtil.ApplyLayoutByName, bubble, "ChatBubble"),
            "a missing layout")
        M.SetLayoutMissing("ChatBubble", false)
        -- scrolling, dragging, resizing
        local scroll = CreateFrame("ScrollFrame", nil, inset)
        scroll:SetPoint("TOPLEFT", inset, "TOPLEFT", 6, -6)
        scroll:SetPoint("BOTTOMRIGHT", inset, "BOTTOMRIGHT", -22, 6)
        local child = CreateFrame("Frame", nil, scroll)
        child:SetSize(396, 1000)
        scroll:SetScrollChild(child)
        local row = child:CreateTexture(nil, "ARTWORK")
        row:SetSize(10, 16)
        row:SetPoint("TOPLEFT", child, "TOPLEFT", 0, -100)
        local _, top0 = row:GetRect()
        scroll:SetVerticalScroll(40)
        local _, top1 = row:GetRect()
        expect(math.abs((top1 - top0) - 40) < 1e-6 and scroll:GetVerticalScrollRange() > 0, "scrolling moves the child up")
        win:SetMovable(true)
        win:RegisterForDrag("LeftButton")
        win:SetScript("OnDragStart", win.StartMoving)
        win:SetScript("OnDragStop", win.StopMovingOrSizing)
        local x0 = select(4, win:GetPoint())
        M.Drag(win, 25, -10)
        expect(select(4, win:GetPoint()) == x0 + 25, "dragging moves the window")
        win:SetResizable(true)
        win:SetResizeBounds(380, 440, 760, 900)
        local grip = CreateFrame("Button", nil, win, "PanelResizeButtonTemplate")
        grip:Init(win, 380, 440, 760, 900)
        M.MouseDown(grip)
        M.Resize(win, 2000, 100)
        M.MouseUp(grip)
        expect(win:GetWidth() == 760 and win:GetHeight() == 440 and not win.sizing, "resizing is clamped to the bounds")
        win:Hide()
        M.SetChannels({})
        expect(type(UISpecialFrames) == "table" and Constants.ChatFrameConstants.MaxChatChannels == 20
            and (fr and SAY == "Dire" or not fr and SAY == "Say"), "globals: UISpecialFrames, constants, localized SAY")
        check(#fails == 0, "conversation window mock pieces (" .. table.concat(fails, "; ") .. ")")
    end
    Run()
end

---------------------------------------------------------------------------------------------
-- tests\harness_*.lua: the window and reply checks of their own work packages
local privateStrings = {}
do
    local H = {
        check = check, section = section, out = out, M = M, ns = ns, L = L, db = db, fr = fr, stubBridge = stubBridge,
        sent = sent, Parse = Parse, Since = Since, AllOk = AllOk, Deliver = Deliver, slash = slash, ButtonOf = ButtonOf,
        EntryFor = EntryFor, EntryWith = EntryWith, Printed = Printed, CountPrinted = CountPrinted, Snapshot = Snapshot,
        Lines = Lines, Base = Base, LinksIn = LinksIn, SUFFIX_START = SUFFIX_START,
        Private = function(text)
            privateStrings[#privateStrings + 1] = text
        end,
    }
    for _, file in ipairs(HARNESS_FILES or {}) do
        section("file " .. file)
        local chunk, err = load(HARNESS_SOURCES[file], "@tests/" .. file)
        local ok = chunk ~= nil
        if ok then
            ok, err = pcall(chunk, H)
        end
        check(ok, file .. " ran to the end" .. (ok and "" or (": " .. tostring(err))))
    end
end

---------------------------------------------------------------------------------------------
section("privacy and errors")
-- Nothing a player typed, no sender, no GUID and no glossary hit may reach the saved variables.
local hits = { "lf2m", "cheap", "skull", "strat", "Linen Cloth", "pull in 5" }
local leaks = {}
local function Walk(v, pathName, inGlossary)
    if type(v) == "table" then
        for k, x in pairs(v) do
            Walk(k, pathName .. ".<key>", inGlossary)
            Walk(x, pathName .. "." .. tostring(k), inGlossary or (v == db and k == "glossary"))
        end
    elseif type(v) == "string" then
        for _, t in ipairs(deliveredText) do
            if #t >= 4 and v:find(t, 1, true) then
                leaks[#leaks + 1] = pathName
            end
        end
        for _, s in ipairs(deliveredSender) do
            if v:find(s, 1, true) then
                leaks[#leaks + 1] = pathName
            end
        end
        for _, g in ipairs(deliveredGuid) do
            if v:find(g, 1, true) then
                leaks[#leaks + 1] = pathName
            end
        end
        for _, t in ipairs(privateStrings) do
            if #t >= 4 and v:find(t, 1, true) then
                leaks[#leaks + 1] = pathName
            end
        end
        for _, rec in ipairs(M.SentChat()) do
            if type(rec.text) == "string" and #rec.text >= 4 and v:find(rec.text, 1, true) then
                leaks[#leaks + 1] = pathName
            end
        end
        if not inGlossary then
            for _, h in ipairs(hits) do
                if v:lower():find(h:lower(), 1, true) then
                    leaks[#leaks + 1] = pathName
                end
            end
        end
    end
end
Walk(db, "TranslateProbeDB", false)
check(#leaks == 0, "no text, sender, GUID, sent text or glossary hit in the saved variables ("
    .. table.concat(leaks, ", ") .. ")")
local glossKeys = {}
for k in pairs(db.glossary) do
    glossKeys[#glossKeys + 1] = k
end
table.sort(glossKeys)
check(table.concat(glossKeys, ",") == "hot,été", "the glossary holds only the user's own entries")
if stubBridge then
    check(#Bridge._sent == #sent, "the stub recorded every message")
end
check(ChatFrame1.holes == 0 and ChatFrame3.holes == 0, "no history hole in the whole session")
local errors = M.Errors()
check(#errors == 0, "errorCount == 0 (" .. table.concat(errors, " | ") .. ")")
