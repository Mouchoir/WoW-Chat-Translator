-- The real user interface of the future translator: a [T] link at the end of each readable
-- player message. Clicking it writes under that line the glossary meanings found in it
-- (instant), then, in test mode, the timing of a simulated model pass.
--
-- Design notes (checked against the Forever 1.60.1 UI source):
-- * The message event filter is only a notification. It never returns replacement arguments,
--   so Blizzard's chat handler keeps its own untainted values (sender, /r target...).
-- * Lines are found again through eventArgs[11], the lineID, which is never secret. This is
--   how Blizzard itself rewrites censored lines. Copies that Blizzard makes when a chat is
--   moved to a new window have no eventArgs; those are found by the exact button text.
-- * TransformMessages runs our predicate and transform in protected calls, but a transform
--   that errors leaves a nil hole in the history buffer. Transforms here only concatenate and
--   cut plain strings, and predicates refuse secret or non-string messages first.
local addonName, ns = ...
local L = ns.L

local ChatButton = {}
ns.ChatButton = ChatButton

local PLAYER_CHAT_EVENTS = {
    "CHAT_MSG_SAY", "CHAT_MSG_YELL", "CHAT_MSG_EMOTE",
    "CHAT_MSG_WHISPER", "CHAT_MSG_BN_WHISPER",
    "CHAT_MSG_PARTY", "CHAT_MSG_PARTY_LEADER",
    "CHAT_MSG_RAID", "CHAT_MSG_RAID_LEADER", "CHAT_MSG_RAID_WARNING",
    "CHAT_MSG_INSTANCE_CHAT", "CHAT_MSG_INSTANCE_CHAT_LEADER",
    "CHAT_MSG_GUILD", "CHAT_MSG_OFFICER",
    "CHAT_MSG_CHANNEL", "CHAT_MSG_COMMUNITIES_CHANNEL",
}
ChatButton.EVENTS = PLAYER_CHAT_EVENTS

local LINK_PREFIX = "addon:" .. addonName .. ":"
local BUTTON_MARK = "|H" .. LINK_PREFIX .. "t:"
local SUFFIX_START = "|n|cff9fd4ff    |H" .. LINK_PREFIX .. "x:"
local MAX_TRACKED = 1000
local MAX_ERROR_CHARS = 120

local tracked = {}       -- lineID -> text of a decorated readable message (memory only)
local trackedOrder = {}  -- FIFO of tracked lineIDs, to bound memory
local senders = {}       -- lineID -> sender name of a tracked line (memory only)
local pendingText = {}   -- lineID -> text, between the filter and the next-frame decoration
local pendingSender = {} -- lineID -> sender name, same lifetime as pendingText
local decorated = {}     -- lineIDs that received a button in the current decoration pass
local results = {}       -- lineID -> suffix lines of the last finished click
local running = {}       -- lineID -> true while its job is queued or running
local decorateQueued = false
local playerGUID

local function ButtonLink(lineID)
    return " |cff66bbff" .. BUTTON_MARK .. ns.session .. ":" .. lineID .. "|h[T]|h|r"
end

-- Chat links end at the next "|": keep our generated text free of it.
local function Safe(text)
    return (tostring(text):gsub("|", "/"))
end

-- One link per line under the message; clicking any of them hides them all.
local function SuffixLinks(lineID, lines)
    local out = {}
    for i, text in ipairs(lines) do
        out[i] = SUFFIX_START .. ns.session .. ":" .. lineID .. "|h" .. text .. "|h|r"
    end
    return table.concat(out)
end

local function LineIDOf(eventArgs)
    if type(eventArgs) ~= "table" then
        return nil
    end
    local lineID = eventArgs[11]
    if type(lineID) == "number" and not issecretvalue(lineID) then
        return lineID
    end
end

local function IsPlainString(message)
    return not issecretvalue(message) and type(message) == "string"
end

local function CountWords(text)
    local words = 0
    for _ in text:gmatch("%S+") do
        words = words + 1
    end
    return words
end

local function Track(lineID, text, sender)
    tracked[lineID], senders[lineID] = text, sender
    trackedOrder[#trackedOrder + 1] = lineID
    while #trackedOrder > MAX_TRACKED do
        local old = table.remove(trackedOrder, 1)
        tracked[old], senders[old], results[old] = nil, nil, nil
    end
end

-- Predicate and transform for adding the button. Signatures follow the chat history entry:
-- message, r, g, b, infoID, accessID, typeID, event, eventArgs, MessageFormatter.
local function NeedsButton(message, _, _, _, _, _, _, _, eventArgs)
    local lineID = LineIDOf(eventArgs)
    return lineID ~= nil and pendingText[lineID] ~= nil and IsPlainString(message)
        and not message:find(BUTTON_MARK, 1, true)
end

local function AddButton(message, r, g, b, infoID, accessID, typeID, event, eventArgs, ...)
    local lineID = eventArgs[11]
    decorated[lineID] = true
    return message .. ButtonLink(lineID), r, g, b, infoID, accessID, typeID, event, eventArgs, ...
end

-- Adds the buttons, then keeps only the lines some chat frame actually shows: the filter also
-- runs for channel lines that no frame displays.
local function DecoratePending()
    decorateQueued = false
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(NeedsButton, AddButton)
    end)
    for lineID, text in pairs(pendingText) do
        if decorated[lineID] then
            Track(lineID, text, pendingSender[lineID])
        end
    end
    wipe(pendingText)
    wipe(pendingSender)
    wipe(decorated)
end

-- Replaces whatever we wrote under the line with these lines, or removes it when nil.
-- Returns how many history entries were rewritten.
local function SetSuffix(lineID, lines)
    local exactMark = BUTTON_MARK .. ns.session .. ":" .. lineID .. "|h"
    local count = 0
    local function Matches(message, _, _, _, _, _, _, _, eventArgs)
        if not IsPlainString(message) then
            return false
        end
        if eventArgs == nil then
            return message:find(exactMark, 1, true) ~= nil
        end
        return LineIDOf(eventArgs) == lineID
    end
    local function Rewrite(message, ...)
        count = count + 1
        local cut = message:find(SUFFIX_START, 1, true)
        if cut then
            message = message:sub(1, cut - 1)
        end
        if lines then
            message = message .. SuffixLinks(lineID, lines)
        end
        return message, ...
    end
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(Matches, Rewrite)
    end)
    return count
end
ChatButton.SetSuffix = SetSuffix

function ChatButton.IsTracked(lineID)
    return tracked[lineID] ~= nil
end

-- Shows lines under the message; a final result whose line is gone is printed instead.
local function Show(lineID, lines, final)
    if SetSuffix(lineID, lines) == 0 and final and lines then
        ns.Print(table.concat(lines, " - "))
    end
end

-- Message event filter, used only to learn that a readable player line arrived. The client
-- skips addon filters while the text is secret, and we return nothing so nothing changes.
local function Filter(_, event, message, sender, _, _, _, _, _, _, _, _, lineID, guid)
    if not IsPlainString(message) or message == "" or not ns.Glossary.HasWords(message) then
        return
    end
    if type(lineID) ~= "number" or issecretvalue(lineID) or lineID == 0 then
        return
    end
    if tracked[lineID] or pendingText[lineID] then
        return
    end
    if not issecretvalue(guid) and guid == playerGUID then
        return
    end
    if C_ChatInfo.IsChatLineCensored and C_ChatInfo.IsChatLineCensored(lineID) then
        return
    end
    pendingText[lineID] = message
    -- Battle.net names are protected strings: never touch them.
    if event ~= "CHAT_MSG_BN_WHISPER" and IsPlainString(sender) then
        pendingSender[lineID] = sender
    end
    if not decorateQueued then
        decorateQueued = true
        C_Timer.After(0, DecoratePending)
    end
end

-- Text of a line we decorated; re-read from the client if it fell out of our memory.
local function SourceText(lineID)
    if tracked[lineID] then
        return tracked[lineID]
    end
    if C_ChatInfo.InChatMessagingLockdown() then
        return nil
    end
    local okValid, valid = pcall(C_ChatInfo.IsValidChatLine, lineID)
    if not okValid or issecretvalue(valid) or not valid then
        return nil
    end
    local okText, text = pcall(C_ChatInfo.GetChatLineText, lineID)
    if okText and not issecretvalue(text) and type(text) == "string" and text ~= "" then
        return text
    end
end

-- Instant lines: glossary, then the sender name in Latin letters when that option is on.
local function InstantLines(lineID, text)
    local gloss = ns.Glossary.Describe(text)
    local lines = { gloss and (L.SUFFIX_GLOSS .. gloss) or L.SUFFIX_NO_GLOSS }
    local roman = ns.db.names and ns.Glossary.Romanize(senders[lineID])
    if roman then
        lines[#lines + 1] = L.NAME_LINE:format(roman)
    end
    return lines
end

local function With(lines, extra)
    local out = {}
    for i, line in ipairs(lines) do
        out[i] = line
    end
    out[#out + 1] = extra
    return out
end

local function StartRun(lineID)
    local text = SourceText(lineID)
    if not text then
        Show(lineID, { L.SUFFIX_TOO_OLD }, true)
        return
    end
    local instant = InstantLines(lineID, text)
    if not ns.db.simulate then
        results[lineID] = instant
        Show(lineID, results[lineID], true)
        return
    end

    local srcPieces, tgtPieces, truncated = ns.Model.EstimatePieces(CountWords(ns.Glossary.PlainText(text)))
    local placeholder
    if not ns.Model.IsBuilt() then
        placeholder = L.SUFFIX_BUILDING
    elseif ns.Scheduler.IsBusy() then
        placeholder = L.SUFFIX_QUEUED:format(srcPieces)
    else
        placeholder = L.SUFFIX_RUNNING:format(srcPieces)
    end
    running[lineID] = true
    Show(lineID, With(instant, placeholder))
    ns.Model.QueueBuild()

    ns.Scheduler.Enqueue({
        start = function()
            Show(lineID, With(instant, L.SUFFIX_RUNNING:format(srcPieces)))
        end,
        run = function(job)
            job.macs = ns.Model.Forward(srcPieces, tgtPieces)
        end,
        done = function(job, ok)
            running[lineID] = nil
            if not ok then
                Show(lineID, With(instant, L.SUFFIX_FAILED:format(Safe(job.err or "?"):sub(1, MAX_ERROR_CHARS))), true)
                return
            end
            local s = job.stats
            results[lineID] = With(instant, L.SUFFIX_RESULT:format(srcPieces, s.busyMs / 1000, s.wallMs / 1000, s.frames))
            Show(lineID, results[lineID], true)
            ns.PushCapped(ns.db.results.clicks, {
                when = date("%Y-%m-%d %H:%M"), src = srcPieces, tgt = tgtPieces, truncated = truncated,
                shortlist = ns.Model.ShortlistRows(), macs = job.macs,
                busyMs = s.busyMs, wallMs = s.wallMs, queuedMs = s.queuedMs, pausedMs = s.pausedMs,
                frames = s.frames, maxSliceMs = s.maxSliceMs, minBudgetMs = s.minBudgetMs,
                budgetMs = s.maxBudgetMs, every = s.maxEvery, fps = s.avgFps,
                context = ns.LockdownLog.Context(), restrictions = ns.LockdownLog.ActiveRestrictions(),
            }, 50)
        end,
    })
end

-- EventRegistry "SetItemRef" receives clicks on every addon link; ours start with LINK_PREFIX.
local function OnLinkClicked(_, link)
    if issecretvalue(link) or type(link) ~= "string" or link:sub(1, #LINK_PREFIX) ~= LINK_PREFIX then
        return
    end
    local kind, session, id = link:sub(#LINK_PREFIX + 1):match("^(%a):(%d+):(%d+)$")
    if not kind then
        return
    end
    local lineID = tonumber(id)
    if session ~= ns.session then
        ns.Print(L.OLD_SESSION)
        return
    end
    if kind == "x" then
        SetSuffix(lineID, nil)
    elseif running[lineID] then
        ns.Print(L.ALREADY_RUNNING)
    elseif results[lineID] and not IsShiftKeyDown() then
        Show(lineID, results[lineID], true)
    else
        StartRun(lineID)
    end
end

ns.OnLoad(function()
    ns.db.results.clicks = ns.db.results.clicks or {}
    playerGUID = UnitGUID("player")
    for _, event in ipairs(PLAYER_CHAT_EVENTS) do
        ChatFrameUtil.AddMessageEventFilter(event, Filter)
    end
    EventRegistry:RegisterCallback("SetItemRef", OnLinkClicked, ns)
end)
