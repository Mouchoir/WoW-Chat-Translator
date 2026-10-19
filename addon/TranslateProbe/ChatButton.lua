-- The [T] link at the end of each readable player message, the rows written under a line,
-- link clicks and their tooltips. A [T] click opens the line in the conversation window
-- (display = "window", the default) or reserves rows under it (display = "inline"); Ctrl-click
-- uses the other mode, Shift-click translates again without the cache in both.
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
-- The glossary row has a colour of its own (user request, 2026-10-19): the pale gold of the
-- Blizzard item palette (artifact quality), apart from the light blue of the rows and of [T],
-- and readable on the chat background. It never starts a row block (the tagged rows come first).
local GLOSS_COLOR = "e6cc80"
local GLOSS_START = "|n|cff" .. GLOSS_COLOR .. "    |H" .. LINK_PREFIX .. "x:"
-- The hint lane right after the tagged rows (user request U1, Reserve.LaneRow): its text in the
-- gold of NORMAL_FONT_COLOR, readable on the chat background and apart from the light blue rows,
-- [T] and the pale gold glossary row; then a [Download] link of our own kind "d" in the light blue
-- of our links ([T], [Show them]), which opens the Translate Companion dialog. The hint itself is
-- an "x" link like every row (a click hides the rows). The lane never starts a row block.
local LANE_COLOR = "ffd100"
local LANE_START = "|n|cff" .. LANE_COLOR .. "    |H" .. LINK_PREFIX .. "x:"
local LINK_COLOR = "66bbff"
local DOWNLOAD_MARK = "|H" .. LINK_PREFIX .. "d:"
local MAX_TRACKED = 1000
ChatButton.LINK_PREFIX = LINK_PREFIX
ChatButton.SUFFIX_START = SUFFIX_START
ChatButton.GLOSS_START = GLOSS_START
ChatButton.GLOSS_COLOR = GLOSS_COLOR
ChatButton.LANE_START = LANE_START
ChatButton.LANE_COLOR = LANE_COLOR

local tracked = {}       -- lineID -> text of a decorated readable message (memory only)
local trackedOrder = {}  -- FIFO of tracked lineIDs, to bound memory
local senders = {}       -- lineID -> sender name of a tracked line (memory only)
local lineInfo = {}      -- lineID -> { event, sender, zoneChannelID, channelIndex, channelBase, guid,
                         -- bnID } of a tracked line, for the conversation window (memory only)
local pendingText = {}   -- lineID -> text, between the filter and the next-frame decoration
local pendingSender = {} -- lineID -> sender name, same lifetime as pendingText
local pendingInfo = {}   -- lineID -> line details, same lifetime as pendingText
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

-- The hint lane (a { lane = true, hint, download } entry of Reserve.LaneRow): the indent and the
-- hint in one "x" link (a space alone before the reveal, so the row keeps its height), then the
-- [Download] link.
local function LaneLinks(id, row)
    local out = LANE_START .. id .. (row.hint or " ") .. "|h|r"
    if row.download then
        out = out .. "|cff" .. LINK_COLOR .. DOWNLOAD_MARK .. id .. "[" .. row.download .. "]|h|r"
    end
    return out
end

-- The rows under the message. Each row starts with SUFFIX_START (the glossary row, which begins
-- with L.SUFFIX_GLOSS, with GLOSS_START: same link, its own colour; the hint lane, a table entry,
-- with LANE_START); every piece of it is a link, so clicking anywhere on the rows hides them all,
-- except the lane's [Download], which opens the Translate Companion dialog.
function ChatButton.SuffixLinks(lineID, rows)
    local id = ns.session .. ":" .. lineID .. "|h"
    local more = "|H" .. LINK_PREFIX .. "x:" .. id
    local glossPrefix = L.SUFFIX_GLOSS
    local out = {}
    for _, text in ipairs(rows) do
        if type(text) == "table" then
            out[#out + 1] = LaneLinks(id, text)
        else
            local bodies = LinkBodies(text)
            if #bodies == 0 then
                bodies[1] = ""
            end
            local isGloss = glossPrefix ~= nil and text:sub(1, #glossPrefix) == glossPrefix
            local first = (isGloss and GLOSS_START or SUFFIX_START) .. id
            for k, body in ipairs(bodies) do
                out[#out + 1] = (k == 1 and first or more) .. body .. "|h"
            end
            out[#out + 1] = "|r"
        end
    end
    return table.concat(out)
end
local SuffixLinks = ChatButton.SuffixLinks

-- Where our rows start in a chat message (the first row block marker), or nil.
local function RowsAt(message)
    local a = message:find(SUFFIX_START, 1, true)
    local b = message:find(GLOSS_START, 1, true)
    if a and b then
        return math.min(a, b)
    end
    return a or b
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

function ChatButton.Track(lineID, text, sender, info)
    if tracked[lineID] == nil then
        trackedOrder[#trackedOrder + 1] = lineID
    end
    tracked[lineID], senders[lineID] = text, sender
    if info then
        lineInfo[lineID] = info
    end
    while #trackedOrder > MAX_TRACKED do
        local old = table.remove(trackedOrder, 1)
        tracked[old], senders[old], lineInfo[old] = nil, nil, nil
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
-- The conversation window then sees each new tracked line (Conv.OnLineTracked: a sender who has
-- a conversation is followed), in arrival order.
local function DecoratePending()
    decorateQueued = false
    ChatFrameUtil.ForEachChatFrame(function(chatFrame)
        chatFrame:TransformMessages(NeedsButton, AddButton)
    end)
    local fresh = {}
    for lineID, text in pairs(pendingText) do
        if decorated[lineID] then
            Track(lineID, text, pendingSender[lineID], pendingInfo[lineID])
            fresh[#fresh + 1] = lineID
        end
    end
    wipe(pendingText)
    wipe(pendingSender)
    wipe(pendingInfo)
    wipe(decorated)
    table.sort(fresh)
    local conv = ns.Conv
    if conv and conv.OnLineTracked then
        for _, lineID in ipairs(fresh) do
            conv.OnLineTracked(lineID)
        end
    end
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
        local cut = RowsAt(message)
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
        return message:sub(1, RowsAt(message) - 1), ...
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
-- skips addon filters while the text is secret, and we return nothing so nothing changes. The
-- line's details (event, sender, channel, GUID, Battle.net account) are kept in memory for the
-- conversation window; a Battle.net sender name (arg2) is never kept.
local function Filter(_, event, message, sender, _, _, _, _, zoneChannelID, channelIndex, channelBaseName, _, lineID,
                      guid, bnSenderID)
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
    local info = { event = event }
    -- Battle.net names are protected strings: never touch them.
    if event ~= "CHAT_MSG_BN_WHISPER" and IsPlainString(sender) then
        pendingSender[lineID] = sender
        info.sender = sender
    end
    if not issecretvalue(guid) and type(guid) == "string" and guid ~= "" then
        info.guid = guid
    end
    if not issecretvalue(bnSenderID) and type(bnSenderID) == "number" and bnSenderID ~= 0 then
        info.bnID = bnSenderID
    end
    if not issecretvalue(zoneChannelID) and type(zoneChannelID) == "number" then
        info.zoneChannelID = zoneChannelID
    end
    if not issecretvalue(channelIndex) and type(channelIndex) == "number" then
        info.channelIndex = channelIndex
    end
    if IsPlainString(channelBaseName) and channelBaseName ~= "" then
        info.channelBase = channelBaseName
    end
    pendingInfo[lineID] = info
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

-- Details of a tracked line for the conversation window: { event, sender, zoneChannelID,
-- channelIndex, channelBase, guid, bnID } (a copy; memory only), or nil. A line that was not seen
-- by the filter (a line reprinted after a chat restriction) is read back from the client when
-- chat is not locked: sender name and GUID, no event.
function ChatButton.LineInfo(lineID)
    local info = lineInfo[lineID]
    if info then
        local copy = {}
        for k, v in pairs(info) do
            copy[k] = v
        end
        return copy
    end
    if not tracked[lineID] or C_ChatInfo.InChatMessagingLockdown() then
        return nil
    end
    local back = {}
    if C_ChatInfo.GetChatLineSenderGUID then
        local ok, guid = pcall(C_ChatInfo.GetChatLineSenderGUID, lineID)
        if ok and IsPlainString(guid) and guid ~= "" then
            back.guid = guid
        end
    end
    local sender = senders[lineID]
    if IsPlainString(sender) and sender ~= "" then
        back.sender = sender
    end
    if back.guid or back.sender then
        return back
    end
end

-- Rows shown at once under a clicked line: the glossary (when that option is on and the line
-- has a known abbreviation; drawn in GLOSS_COLOR by SuffixLinks), then the sender name in Latin
-- letters (when names is on).
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

-- True when a [T] click goes to the conversation window: display "window" (the default; any
-- other value than "inline" counts as it), the other mode with Ctrl held.
function ChatButton.WindowMode(ctrl)
    local window = not (ns.db and ns.db.display == "inline")
    if ctrl then
        window = not window
    end
    return window and ns.ConvFrame ~= nil and ns.ConvFrame.OpenFromLine ~= nil
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
        local shift = IsShiftKeyDown() and true or false
        if ChatButton.WindowMode(IsControlKeyDown()) then
            ns.ConvFrame.OpenFromLine(id, shift, chatFrame or DEFAULT_CHAT_FRAME)
        else
            ns.Request.Start(id, chatFrame or DEFAULT_CHAT_FRAME, shift)
        end
    elseif kind == "x" then
        ns.Request.Remove(id)
    elseif kind == "d" then
        -- [Download] of a hint lane: the Translate Companion dialog (the rows stay)
        ns.CompanionDialog.Open()
    elseif kind == "r" then
        ns.Restriction.Reprint(id)
    end
end

local function OnLinkEnter(_, frame, link)
    local kind = ParseLink(link)
    local tipT = ChatButton.WindowMode(false) and L.CONV_TIP_T_WINDOW or L.CONV_TIP_T_INLINE
    -- No tooltip on the reserved rows: it would sit on the translation, and the helper never
    -- paints under a game frame (measured in game on 2026-10-19).
    local tip = kind == "t" and (tipT or L.TIP_T) or nil
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
