-- The real user interface of the future translator: a [T] link at the end of each readable
-- player message. Clicking it writes under that line the glossary meanings found in it
-- (instant), then, in test mode, the timing of a simulated model pass.
--
-- Design notes (checked against the Forever 1.60.1 UI source):
-- * The message event filter is only a notification. It never returns replacement arguments,
--   so Blizzard's chat handler keeps its own untainted values (sender, /r target...).
-- * Lines are found again through eventArgs[11], the lineID, which is never secret. This is
--   how Blizzard itself rewrites censored lines.
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

local tracked = {}       -- lineID -> text of a readable message we decorated (memory only)
local trackedOrder = {}  -- FIFO of tracked lineIDs, to bound memory in busy channels
local results = {}       -- lineID -> suffix lines of the last finished click
local running = {}       -- lineID -> true while its job is queued or running
local toDecorate = {}    -- lineIDs waiting for their button on the next frame
local decorateQueued = false
local playerGUID

local function ButtonLink(lineID)
    return " |cff66bbff" .. BUTTON_MARK .. ns.session .. ":" .. lineID .. "|h[T]|h|r"
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

local function Track(lineID, text)
    tracked[lineID] = text
    trackedOrder[#trackedOrder + 1] = lineID
    while #trackedOrder > MAX_TRACKED do
        local old = table.remove(trackedOrder, 1)
        tracked[old], results[old] = nil, nil
    end
end

-- Predicate and transform for adding the button. Signatures follow the chat history entry:
-- message, r, g, b, infoID, accessID, typeID, event, eventArgs, MessageFormatter.
local function NeedsButton(message, _, _, _, _, _, _, _, eventArgs)
    local lineID = LineIDOf(eventArgs)
    return lineID ~= nil and toDecorate[lineID] == true and IsPlainString(message)
        and not message:find(BUTTON_MARK, 1, true)
end

local function AddButton(message, r, g, b, infoID, accessID, typeID, event, eventArgs, ...)
    return message .. ButtonLink(eventArgs[11]), r, g, b, infoID, accessID, typeID, event, eventArgs, ...
end

local function DecoratePending()
    decorateQueued = false
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(NeedsButton, AddButton)
    end)
    wipe(toDecorate)
end

-- Replaces whatever we wrote under the line with these lines, or removes it when nil.
local function SetSuffix(lineID, lines)
    local function Matches(message, _, _, _, _, _, _, _, eventArgs)
        return LineIDOf(eventArgs) == lineID and IsPlainString(message)
    end
    local function Rewrite(message, ...)
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
end
ChatButton.SetSuffix = SetSuffix

-- Message event filter, used only to learn that a readable player line arrived. The client
-- skips addon filters while the text is secret, and we return nothing so nothing changes.
local function Filter(_, _, message, _, _, _, _, _, _, _, _, _, lineID, guid)
    if not IsPlainString(message) or message == "" then
        return
    end
    if type(lineID) ~= "number" or issecretvalue(lineID) or lineID == 0 or tracked[lineID] then
        return
    end
    if guid and not issecretvalue(guid) and guid == playerGUID then
        return
    end
    if C_ChatInfo.IsChatLineCensored and C_ChatInfo.IsChatLineCensored(lineID) then
        return
    end
    Track(lineID, message)
    toDecorate[lineID] = true
    if not decorateQueued then
        decorateQueued = true
        C_Timer.After(0, DecoratePending)
    end
end

local function GlossLine(text)
    local gloss = text and ns.Glossary.Describe(text)
    return gloss and (L.SUFFIX_GLOSS .. gloss) or L.SUFFIX_NO_GLOSS
end

local function StartRun(lineID)
    local text = tracked[lineID]
    local gloss = GlossLine(text)
    if not ns.db.simulate then
        results[lineID] = { gloss }
        SetSuffix(lineID, results[lineID])
        return
    end
    local srcPieces, tgtPieces = ns.Model.EstimatePieces(text and CountWords(text) or 1)
    running[lineID] = true
    local queued = ns.Scheduler.IsBusy()
    SetSuffix(lineID, { gloss, (queued and L.SUFFIX_QUEUED or L.SUFFIX_RUNNING):format(srcPieces) })
    ns.Scheduler.Enqueue({
        run = function(job)
            job.macs = ns.Model.Forward(srcPieces, tgtPieces)
        end,
        done = function(job, ok)
            running[lineID] = nil
            if not ok then
                SetSuffix(lineID, { gloss, L.SUFFIX_FAILED })
                return
            end
            local s = job.stats
            results[lineID] = { gloss, L.SUFFIX_RESULT:format(srcPieces, s.busyMs / 1000, s.wallMs / 1000, s.frames) }
            SetSuffix(lineID, results[lineID])
            ns.PushCapped(ns.db.results.clicks, {
                when = date("%Y-%m-%d %H:%M"), src = srcPieces, tgt = tgtPieces, macs = job.macs,
                busyMs = s.busyMs, wallMs = s.wallMs, frames = s.frames,
                maxSliceMs = s.maxSliceMs, budgetMs = s.maxBudgetMs, fps = GetFramerate(),
            }, 50)
        end,
    })
end

-- EventRegistry "SetItemRef" receives clicks on every addon link; ours start with LINK_PREFIX.
local function OnLinkClicked(_, link)
    if type(link) ~= "string" or issecretvalue(link) then
        return
    end
    local kind, session, id = link:match("^addon:" .. addonName .. ":(%a):(%d+):(%d+)$")
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
        SetSuffix(lineID, results[lineID])
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
