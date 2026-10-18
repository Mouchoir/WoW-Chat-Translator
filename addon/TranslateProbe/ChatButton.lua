-- The [T] link at the end of each readable player message, the rows written under a line,
-- link clicks and their tooltips.
--
-- Design notes (checked against the Forever 1.60.1 UI source):
-- * The message event filter is only a notification. It never returns replacement arguments,
--   so Blizzard's chat handler keeps its own untainted values (sender, /r target...).
-- * Lines are found again through eventArgs[11], the lineID, which is never secret. This is
--   how Blizzard itself rewrites censored lines. Copies that Blizzard makes when a chat is
--   moved to a new window, and lines the addon prints itself, have no eventArgs; those are
--   found by the exact button text.
-- * TransformMessages runs our predicate and transform in protected calls, but a transform
--   that errors leaves a nil hole in the history buffer. Transforms here only concatenate and
--   cut plain strings, and predicates refuse secret or non-string messages first. Anything
--   that could fail (measuring a chat frame) runs before TransformMessages.
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
ChatButton.LINK_PREFIX = LINK_PREFIX
ChatButton.SUFFIX_START = SUFFIX_START

local tracked = {}       -- lineID -> text of a decorated readable message (memory only)
local trackedOrder = {}  -- FIFO of tracked lineIDs, to bound memory
local senders = {}       -- lineID -> sender name of a tracked line (memory only)
local pendingText = {}   -- lineID -> text, between the filter and the next-frame decoration
local pendingSender = {} -- lineID -> sender name, same lifetime as pendingText
local decorated = {}     -- lineIDs that received a button in the current decoration pass
local decorateQueued = false
local tipShown = false
local playerGUID

function ChatButton.ButtonLink(lineID)
    return " |cff66bbff" .. BUTTON_MARK .. ns.session .. ":" .. lineID .. "|h[T]|h|r"
end
local ButtonLink = ChatButton.ButtonLink

-- Chat links end at the next "|": keep our generated text free of it.
function ChatButton.Safe(text)
    return (tostring(text):gsub("|", "/"))
end

-- Splits a row into link bodies: each inline texture alone, plain text runs kept whole.
-- WoW Forever (12.x) draws only the first texture of a link's text and drops the others
-- (measured in game on 2026-10-18), so a texture never shares a link.
local function LinkBodies(text)
    local bodies, at = {}, 1
    while at <= #text do
        local s = text:find("|T", at, true)
        local e = s and text:find("|t", s + 2, true)
        if not e then
            bodies[#bodies + 1] = text:sub(at)
            break
        end
        if s > at then
            bodies[#bodies + 1] = text:sub(at, s - 1)
        end
        bodies[#bodies + 1] = text:sub(s, e + 1)
        at = e + 2
    end
    return bodies
end

-- The rows under the message. Each row starts with SUFFIX_START; every piece of it is a link,
-- so clicking anywhere on the rows hides them all.
function ChatButton.SuffixLinks(lineID, rows)
    local id = ns.session .. ":" .. lineID .. "|h"
    local first, more = SUFFIX_START .. id, "|H" .. LINK_PREFIX .. "x:" .. id
    local out = {}
    for _, text in ipairs(rows) do
        local bodies = LinkBodies(text)
        if #bodies == 0 then
            bodies[1] = ""
        end
        for k, body in ipairs(bodies) do
            out[#out + 1] = (k == 1 and first or more) .. body .. "|h"
        end
        out[#out + 1] = "|r"
    end
    return table.concat(out)
end
local SuffixLinks = ChatButton.SuffixLinks

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

function ChatButton.Track(lineID, text, sender)
    if tracked[lineID] == nil then
        trackedOrder[#trackedOrder + 1] = lineID
    end
    tracked[lineID], senders[lineID] = text, sender
    while #trackedOrder > MAX_TRACKED do
        local old = table.remove(trackedOrder, 1)
        tracked[old], senders[old] = nil, nil
    end
end
local Track = ChatButton.Track

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

-- Replaces whatever we wrote under the line, in every chat frame. rows: nil (remove), an array
-- of row strings, or function(chatFrame) -> array|nil, called per frame before its transform
-- (under pcall; nil or an error leaves that frame untouched). Returns how many history entries
-- were rewritten.
function ChatButton.SetSuffix(lineID, rows)
    local exactMark = BUTTON_MARK .. ns.session .. ":" .. lineID .. "|h"
    local count = 0
    local suffix = ""
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
        return message .. suffix, ...
    end
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        local list = rows
        if type(rows) == "function" then
            local ok, built = pcall(rows, chatFrame)
            if not ok or type(built) ~= "table" then
                return
            end
            list = built
        end
        suffix = list and SuffixLinks(lineID, list) or ""
        chatFrame:TransformMessages(Matches, Rewrite)
    end)
    return count
end

-- Cuts every row block whose session passes shouldCut(sessionString). Returns the number of
-- entries rewritten. Entries without rows are never touched.
function ChatButton.CutRows(shouldCut)
    local count = 0
    local function Matches(message)
        if not IsPlainString(message) then
            return false
        end
        local at = message:find(SUFFIX_START, 1, true)
        if not at then
            return false
        end
        local session = message:match("^(%d+):", at + #SUFFIX_START)
        return session ~= nil and shouldCut(session) == true
    end
    local function Cut(message, ...)
        count = count + 1
        return message:sub(1, message:find(SUFFIX_START, 1, true) - 1), ...
    end
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(Matches, Cut)
    end)
    return count
end

-- Set of the lineIDs some chat frame's history holds right now. Read-only: the predicate records
-- eventArgs[11] (never secret) and always returns false, so no entry is ever repackaged.
function ChatButton.ShownLines()
    local shown = {}
    local function Look(_, _, _, _, _, _, _, _, eventArgs)
        local lineID = LineIDOf(eventArgs)
        if lineID ~= nil then
            shown[lineID] = true
        end
        return false
    end
    local function Keep(...)
        return ...
    end
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(Look, Keep)
    end)
    return shown
end

function ChatButton.IsTracked(lineID)
    return tracked[lineID] ~= nil
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
function ChatButton.SourceText(lineID)
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

-- Rows shown at once under a clicked line: the glossary (when that option is on and the line
-- has a known abbreviation), then the sender name in Latin letters (when names is on).
function ChatButton.InstantLines(lineID, text)
    local lines = {}
    if ns.db.glossShow then
        local gloss = ns.Glossary.Describe(text, ns.db.target)
        if gloss then
            lines[#lines + 1] = L.SUFFIX_GLOSS .. gloss
        end
    end
    local roman = ns.db.names and ns.Glossary.Romanize(senders[lineID])
    if roman then
        lines[#lines + 1] = L.NAME_LINE:format(roman)
    end
    return lines
end

-- kind, lineID (number) of one of our links of this session; kind "old" for an older session.
local function ParseLink(link)
    if issecretvalue(link) or type(link) ~= "string" or link:sub(1, #LINK_PREFIX) ~= LINK_PREFIX then
        return nil
    end
    local kind, session, id = link:sub(#LINK_PREFIX + 1):match("^(%a):(%d+):(%d+)$")
    if not kind then
        return nil
    end
    if session ~= ns.session then
        return "old"
    end
    return kind, tonumber(id)
end

-- EventRegistry "SetItemRef" receives clicks on every addon link; ours start with LINK_PREFIX.
local function OnLinkClicked(_, link, _, _, chatFrame)
    local kind, id = ParseLink(link)
    if kind == "old" then
        ns.Print(L.OLD_SESSION)
    elseif kind == "t" then
        ns.Request.Start(id, chatFrame or DEFAULT_CHAT_FRAME, IsShiftKeyDown() and true or false)
    elseif kind == "x" then
        ns.Request.Remove(id)
    elseif kind == "r" then
        ns.Restriction.Reprint(id)
    end
end

local function OnLinkEnter(_, frame, link)
    local kind = ParseLink(link)
    local tip = (kind == "t" and L.TIP_T) or (kind == "x" and L.TIP_ROWS) or nil
    if tip and GameTooltip then
        GameTooltip:SetOwner(frame or UIParent, "ANCHOR_CURSOR")
        GameTooltip:SetText(tip, 1, 1, 1, 1, true)
        GameTooltip:Show()
        tipShown = true
    end
end

local function OnLinkLeave()
    if tipShown and GameTooltip then
        tipShown = false
        GameTooltip:Hide()
    end
end

ns.OnLoad(function()
    playerGUID = UnitGUID("player")
    for _, event in ipairs(PLAYER_CHAT_EVENTS) do
        ChatFrameUtil.AddMessageEventFilter(event, Filter)
    end
    EventRegistry:RegisterCallback("SetItemRef", OnLinkClicked, ns)
    EventRegistry:RegisterCallback("ChatFrame.OnHyperlinkEnter", OnLinkEnter, ns)
    EventRegistry:RegisterCallback("ChatFrame.OnHyperlinkLeave", OnLinkLeave, ns)
end)
