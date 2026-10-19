-- Where a reply from the conversation window goes, and sending it (docs\ADDON.md, "Reply and
-- send"). Everything here is memory only: names, Battle.net ids, channel numbers and sent texts
-- live in Lua tables until /reload.
--
-- * Targets(conv): the channel buttons under the reply box. The default is where the sender
--   wrote last (a whisper, a numbered channel, say, party...); then Whisper, then Say or Yell,
--   replaced by the channel I last used in this conversation when different. Battle.net
--   senders get Whisper only. Others(conv) lists every other choice for the [Other] popup.
-- * Wire(text): the exact bytes sent. Control bytes 0x01-0x03 removed, line breaks become
--   spaces, trimmed, and canonical pipe escaping (every "||" kept, every other "|" doubled), which
--   gives the same result whether the edit box returns raw or doubled pipes. The 255-byte limit
--   is checked on that result.
-- * Send(conv, id, wire): refused before any API call while chat is locked; one call per click,
--   from the button's OnClick (a hardware event, needed for say, yell and channels outdoors).
--   It adds my bubble ("Sending..."), then follows it: the server's echo within 5 s gives
--   "sent", none gives "unconfirmed", ERR_CHAT_PLAYER_NOT_FOUND_S gives "offline" and
--   ADDON_ACTION_BLOCKED naming this addon gives "blocked" (Conv.SetOutState).
local addonName, ns = ...
local L = ns.L

local Channels = {}
ns.Channels = Channels

Channels.MAX_BYTES = 255
local ECHO_SECONDS = 5          -- an echo later than this does not count
local RECENT_SECONDS = 10       -- sends remembered for IsOwnEcho after their echo
local REPEAT_SECONDS = 1        -- a second send within this time is ignored

-- Incoming chat type (event without CHAT_MSG_) -> the default target id.
local DEFAULT_OF = {
    WHISPER = "WHISPER", WHISPER_INFORM = "WHISPER", BN_WHISPER = "BN_WHISPER", BN_WHISPER_INFORM = "BN_WHISPER",
    CHANNEL = "CHANNEL", SAY = "SAY", EMOTE = "SAY", TEXT_EMOTE = "SAY", YELL = "YELL",
    PARTY = "PARTY", PARTY_LEADER = "PARTY", RAID = "RAID", RAID_LEADER = "RAID", RAID_WARNING = "RAID",
    INSTANCE_CHAT = "INSTANCE_CHAT", INSTANCE_CHAT_LEADER = "INSTANCE_CHAT", GUILD = "GUILD", OFFICER = "OFFICER",
}
-- Echo event (without CHAT_MSG_) -> the chat type it confirms.
local ECHO_OF = {
    SAY = "SAY", YELL = "YELL", EMOTE = "SAY", PARTY = "PARTY", PARTY_LEADER = "PARTY", RAID = "RAID",
    RAID_LEADER = "RAID", RAID_WARNING = "RAID", INSTANCE_CHAT = "INSTANCE_CHAT",
    INSTANCE_CHAT_LEADER = "INSTANCE_CHAT", GUILD = "GUILD", OFFICER = "OFFICER", CHANNEL = "CHANNEL",
    WHISPER_INFORM = "WHISPER", BN_WHISPER_INFORM = "BN_WHISPER",
}
-- Blizzard's localized names (frFR: Dire, Crier, Chuchoter...), with our own fallback.
local LABEL_GLOBAL = {
    SAY = "SAY", YELL = "YELL", WHISPER = "WHISPER", BN_WHISPER = "WHISPER", PARTY = "PARTY", RAID = "RAID",
    INSTANCE_CHAT = "INSTANCE_CHAT", GUILD = "GUILD", OFFICER = "OFFICER",
}
local OTHER_ORDER = { "SAY", "YELL", "PARTY", "RAID", "INSTANCE_CHAT", "GUILD", "OFFICER" }

local pending = {}      -- sends waiting for their echo: { msg, chatType, target, text, at, conv }
local recent = {}       -- confirmed sends, kept RECENT_SECONDS for IsOwnEcho
local lastSendAt = -math.huge
local counters = { sent = 0, confirmed = 0, unconfirmed = 0, offline = 0, blocked = 0, refused = 0 }

---------------------------------------------------------------------------- helpers
local function Plain(v)
    return type(v) == "string" and not issecretvalue(v)
end

local function Short(name)
    if not Plain(name) then
        return "?"
    end
    return (name:gsub("%-.*$", ""))
end

local function ChatKind(event)
    if not Plain(event) then
        return nil
    end
    return (event:gsub("^CHAT_MSG_", ""))
end

local function Call(fn, ...)
    if type(fn) ~= "function" then
        return false
    end
    local ok, a, b, c = pcall(fn, ...)
    if ok then
        return true, a, b, c
    end
    return false
end

local function Conv(name, ...)
    local C = ns.Conv
    if C and type(C[name]) == "function" then
        return C[name](...)
    end
end

local function Label(chatType)
    local global = LABEL_GLOBAL[chatType]
    local v = global and _G[global]
    if Plain(v) and v ~= "" then
        return v
    end
    return L["RPL_CH_" .. chatType] or chatType
end

local function Color(chatType, index)
    local info = ChatTypeInfo and ((index and ChatTypeInfo["CHANNEL" .. index]) or ChatTypeInfo[chatType])
    if type(info) == "table" and type(info.r) == "number" then
        return { info.r, info.g, info.b }
    end
    return { 1, 1, 1 }
end

-- The current channel number of the channel named base, or nil when I am not in it (any more).
-- The old number (index) is only a second way to find the same channel: it is accepted when the
-- channel now at that number has exactly that name. A zone channel ("General - Elwynn Forest")
-- left for another zone, or a channel left and replaced by another one at the same number, gives
-- nil, so the button is greyed ("You left this channel") instead of sending somewhere else.
local function ChannelIndex(base, index)
    if not (Plain(base) and base ~= "") then
        return nil
    end
    local ok, id = Call(GetChannelName, base)
    if ok and type(id) == "number" and id > 0 then
        return id
    end
    if type(index) == "number" and index > 0 then
        local okIndex, idIndex, name = Call(GetChannelName, index)
        if okIndex and type(idIndex) == "number" and idIndex > 0 and Plain(name) and name:lower() == base:lower() then
            return idIndex
        end
    end
    return nil
end

local function ChannelName(index)
    local ok, id, name = Call(GetChannelName, index)
    if ok and type(id) == "number" and id > 0 and Plain(name) then
        return name
    end
end

-- "2. Trade" from channel 2 named "Trade - City". A channel I left keeps its own name with its
-- old number (never the name of the channel that took that number since).
local function ChannelLabel(index, base, oldIndex)
    local name = (index and ChannelName(index)) or base or "?"
    return ("%d. %s"):format(index or oldIndex or 0, name:match("^(.-) %- ") or name)
end

local function IsBN(conv)
    return conv and type(conv.bnID) == "number" and conv.bnID > 0
end

-- The name a whisper goes to ("Name" or "Name-Realm"); never read for Battle.net.
local function WhisperName(conv)
    if IsBN(conv) then
        return nil
    end
    local last = conv.last or {}
    local name = Plain(last.sender) and last.sender ~= "" and last.sender or conv.name
    if Plain(name) and name ~= "" then
        return name
    end
end

local function Officer()
    if C_GuildInfo and type(C_GuildInfo.IsGuildOfficer) == "function" then
        local ok, yes = Call(C_GuildInfo.IsGuildOfficer)
        return ok and yes == true
    end
    local ok, yes = Call(CanEditOfficerNote)
    return ok and yes == true
end

-- nil when the chat type can be used now, else the L key of the reason.
local function Unavailable(chatType)
    if chatType == "PARTY" then
        return not IsInGroup(LE_PARTY_CATEGORY_HOME) and "RPL_WHY_NO_GROUP" or nil
    elseif chatType == "RAID" then
        return not IsInRaid() and "RPL_WHY_NO_RAID" or nil
    elseif chatType == "INSTANCE_CHAT" then
        return not IsInGroup(LE_PARTY_CATEGORY_INSTANCE) and "RPL_WHY_NO_INSTANCE" or nil
    elseif chatType == "GUILD" then
        return not IsInGuild() and "RPL_WHY_NO_GUILD" or nil
    elseif chatType == "OFFICER" then
        if not IsInGuild() then
            return "RPL_WHY_NO_GUILD"
        end
        return not Officer() and "RPL_WHY_NOT_OFFICER" or nil
    end
    return nil
end

---------------------------------------------------------------------------- targets
-- A target by id, built from the conversation now: { id, label, chatType, target, enabled,
-- reason (L key or nil), color = { r, g, b }, isDefault }. Ids: "WHISPER", "BN_WHISPER",
-- "SAY", "YELL", "PARTY", "RAID", "INSTANCE_CHAT", "GUILD", "OFFICER", "CHANNEL:<name>".
local function Build(conv, id)
    if type(id) ~= "string" then
        return nil
    end
    local t = { id = id, enabled = true }
    local base = id:match("^CHANNEL:(.+)$")
    if base then
        local last = conv.last or {}
        local fallback = (last.channelBase == base) and last.channelIndex or nil
        local index = ChannelIndex(base, fallback)
        t.chatType, t.target = "CHANNEL", index
        t.label = ChannelLabel(index, base, fallback)
        t.color = Color("CHANNEL", index)
        if not index then
            t.enabled, t.reason = false, "RPL_WHY_LEFT"
        end
        return t
    end
    if id == "BN_WHISPER" then
        t.chatType, t.target = "BN_WHISPER", IsBN(conv) and conv.bnID or nil
        t.label, t.color = Label("BN_WHISPER"), Color("BN_WHISPER")
        if not t.target then
            t.enabled, t.reason = false, "RPL_WHY_NO_NAME"
        end
        return t
    end
    if id == "WHISPER" then
        t.chatType, t.target = "WHISPER", WhisperName(conv)
        t.label, t.color = Label("WHISPER"), Color("WHISPER")
        if not t.target then
            t.enabled, t.reason = false, "RPL_WHY_NO_NAME"
        end
        return t
    end
    if not LABEL_GLOBAL[id] then
        return nil
    end
    t.chatType, t.label, t.color = id, Label(id), Color(id)
    local why = Unavailable(id)
    if why then
        t.enabled, t.reason = false, why
    end
    return t
end

-- Id of the conversation's default target: where the sender wrote last.
function Channels.Default(conv)
    if not conv then
        return nil
    end
    if IsBN(conv) then
        return "BN_WHISPER"
    end
    local last = conv.last or {}
    local def = DEFAULT_OF[ChatKind(last.event) or ""]
    if def == "BN_WHISPER" then
        def = "WHISPER"
    end
    if def == "CHANNEL" then
        -- Only the channel's own name identifies it: the channel now at the old number may be
        -- another one, so a line without its channel name never defaults to that number.
        local base = Plain(last.channelBase) and last.channelBase ~= "" and last.channelBase
        return base and ("CHANNEL:" .. base) or (WhisperName(conv) and "WHISPER" or "SAY")
    end
    return def or (WhisperName(conv) and "WHISPER" or "SAY")
end

-- The channel buttons: { default, Whisper (unless default), Say or Yell or my last channel }.
-- Battle.net: Whisper only.
function Channels.Targets(conv)
    local list = {}
    if not conv then
        return list
    end
    local def = Channels.Default(conv)
    local seen = {}
    local function Add(id, isDefault)
        if id and not seen[id] then
            local t = Build(conv, id)
            if t then
                t.isDefault = isDefault or false
                list[#list + 1] = t
                seen[id] = true
            end
        end
    end
    Add(def, true)
    if IsBN(conv) then
        return list
    end
    Add("WHISPER")
    local choice = conv.channelChoice
    if type(choice) == "string" and not seen[choice] and choice ~= "BN_WHISPER" and Build(conv, choice) then
        Add(choice)
    else
        Add(seen.SAY and "YELL" or "SAY")
    end
    return list
end

-- The [Other] popup: say, yell, party, raid, instance, guild, officer, then every channel I am
-- in, then "Whisper <name>". Entries have the same fields as Targets, plus separator = true
-- before the channels.
function Channels.Others(conv)
    local list = {}
    if not conv or IsBN(conv) then
        return list
    end
    for _, id in ipairs(OTHER_ORDER) do
        list[#list + 1] = Build(conv, id)
    end
    local first = true
    local max = Constants and Constants.ChatFrameConstants and Constants.ChatFrameConstants.MaxChatChannels or 20
    for i = 1, max do
        local name = ChannelName(i)
        if name then
            local t = Build(conv, "CHANNEL:" .. name)
            if t then
                t.separator = first
                first = false
                list[#list + 1] = t
            end
        end
    end
    local whisper = Build(conv, "WHISPER")
    if whisper then
        whisper.label = L.RPL_WHISPER_NAME:format(Short(whisper.target or conv.name))
        whisper.separator = first
        list[#list + 1] = whisper
    end
    return list
end

-- The target with this id, as it is now (nil for an unknown id).
function Channels.Find(conv, id)
    if not conv then
        return nil
    end
    return Build(conv, id)
end

---------------------------------------------------------------------------- wire text
-- The exact text sent and its length in bytes, or nil, 0 when nothing is left to send.
function Channels.Wire(text)
    if not Plain(text) then
        return nil, 0
    end
    local s = text:gsub("[\1\2\3]", ""):gsub("[\r\n]+", " ")
    s = strtrim(s)
    if s == "" then
        return nil, 0
    end
    -- canonical escaping: "||" stays, any other "|" is doubled (idempotent)
    s = s:gsub("||", "\1"):gsub("|", "||"):gsub("\1", "||")
    return s, #s
end

-- True while the client refuses addon chat messages (an encounter, a challenge mode...).
function Channels.Locked()
    local ok, locked = Call(C_ChatInfo and C_ChatInfo.InChatMessagingLockdown)
    if ok and locked == true then
        return true
    end
    if C_RestrictedActions and Enum and Enum.AddOnRestrictionType and Enum.AddOnRestrictionType.Chat then
        local okR, active = Call(C_RestrictedActions.IsAddOnRestrictionActive, Enum.AddOnRestrictionType.Chat)
        if okR and active == true then
            return true
        end
    end
    return false
end

---------------------------------------------------------------------------- sending
local function SetState(p, state)
    if p.msg then
        Conv("SetOutState", p.msg, state)
    end
end

local function Remove(list, p)
    for i, x in ipairs(list) do
        if x == p then
            table.remove(list, i)
            return true
        end
    end
    return false
end

local function Prune(now)
    for i = #recent, 1, -1 do
        if now - recent[i].at > RECENT_SECONDS then
            table.remove(recent, i)
        end
    end
end

local function Confirm(p)
    if Remove(pending, p) then
        counters.confirmed = counters.confirmed + 1
        Prune(GetTime())
        recent[#recent + 1] = p
        SetState(p, "sent")
    end
end

-- Sends wire to the target id of conv. Returns true, nil, msg (my bubble) when the message
-- went to the client, else false and the L key of the reason. opts.draft: the text I wrote
-- before it was translated (shown as "You wrote: ..." under my bubble).
function Channels.Send(conv, id, wire, opts)
    opts = opts or {}
    if Channels.Locked() then
        counters.refused = counters.refused + 1
        return false, "RPL_WHY_LOCKED"
    end
    local t = Channels.Find(conv, id)
    if not t then
        return false, "RPL_WHY_UNKNOWN"
    end
    if not t.enabled then
        return false, t.reason or "RPL_WHY_UNKNOWN"
    end
    if not Plain(wire) or wire == "" then
        return false, "RPL_WHY_EMPTY"
    end
    if #wire > Channels.MAX_BYTES then
        return false, "RPL_WHY_TOO_LONG"
    end
    local now = GetTime()
    if now - lastSendAt < REPEAT_SECONDS then
        return false, "RPL_WHY_REPEAT"
    end
    lastSendAt = now
    counters.sent = counters.sent + 1

    local msg = Conv("AddOut", conv, { text = wire, draft = opts.draft, label = t.label, chatType = t.chatType,
        target = t.target })
    local p = { msg = msg, chatType = t.chatType, target = t.target, text = wire, at = now, conv = conv }
    pending[#pending + 1] = p
    SetState(p, "sending")
    -- in the list before the call: a refusal fires ADDON_ACTION_BLOCKED during it
    local ok
    if t.chatType == "BN_WHISPER" then
        ok = Call(C_BattleNet and C_BattleNet.SendWhisper, t.target, wire)
    elseif t.chatType == "WHISPER" or t.chatType == "CHANNEL" then
        ok = Call(C_ChatInfo.SendChatMessage, wire, t.chatType, nil, t.target)
    else
        ok = Call(C_ChatInfo.SendChatMessage, wire, t.chatType)
    end
    if not ok then
        if Remove(pending, p) then
            counters.blocked = counters.blocked + 1
            SetState(p, "blocked")
        end
        return false, "RPL_WHY_FAILED", msg
    end
    C_Timer.After(ECHO_SECONDS, function()
        if Remove(pending, p) then
            counters.unconfirmed = counters.unconfirmed + 1
            SetState(p, "unconfirmed")
        end
    end)
    return true, nil, msg
end

-- Pipes compare as the reader sees them.
local function Same(a, b)
    if not (Plain(a) and Plain(b)) then
        return false
    end
    return strtrim((a:gsub("||", "|"))) == strtrim((b:gsub("||", "|")))
end

-- Same player: the same name ignoring case, or the same short name when one side has no realm.
local function SameName(a, b)
    if not (Plain(a) and Plain(b)) then
        return false
    end
    if a:lower() == b:lower() then
        return true
    end
    local bothRealms = a:find("-", 1, true) and b:find("-", 1, true)
    return not bothRealms and Short(a):lower() == Short(b):lower()
end

-- True when a line of mine (an echo or an *_INFORM line) is the echo of a send from the
-- window, pending or confirmed in the last 10 s: the window shows that bubble already.
function Channels.IsOwnEcho(event, text)
    local kind = ECHO_OF[ChatKind(event) or ""]
    if not kind or not Plain(text) then
        return false
    end
    Prune(GetTime())
    for _, list in ipairs({ pending, recent }) do
        for _, p in ipairs(list) do
            if p.chatType == kind and Same(p.text, text) then
                return true
            end
        end
    end
    return false
end

local function OnEcho(kind, text, target, channelIndex, guid, bnID)
    for _, p in ipairs(pending) do
        if p.chatType == kind then
            local match
            if kind == "WHISPER" then
                match = SameName(p.target, target) or Same(p.text, text)
            elseif kind == "BN_WHISPER" then
                match = (type(bnID) == "number" and bnID == p.target) or Same(p.text, text)
            elseif guid == UnitGUID("player") and Same(p.text, text) then
                match = kind ~= "CHANNEL" or channelIndex == p.target or channelIndex == nil
            end
            if match then
                Confirm(p)
                return
            end
        end
    end
end

local function OnSystem(text)
    if not Plain(text) or not Plain(ERR_CHAT_PLAYER_NOT_FOUND_S) then
        return
    end
    for _, p in ipairs(pending) do
        if p.chatType == "WHISPER" and Plain(p.target) then
            if text == ERR_CHAT_PLAYER_NOT_FOUND_S:format(p.target)
                or text == ERR_CHAT_PLAYER_NOT_FOUND_S:format(Short(p.target)) then
                Remove(pending, p)
                counters.offline = counters.offline + 1
                SetState(p, "offline")
                return
            end
        end
    end
end

local function OnBlocked(name)
    if name ~= addonName then
        return
    end
    local p = pending[#pending]
    if p and GetTime() - p.at <= 2 then
        Remove(pending, p)
        counters.blocked = counters.blocked + 1
        SetState(p, "blocked")
    end
end

local events = CreateFrame("Frame")
events:SetScript("OnEvent", function(_, event, ...)
    if event == "ADDON_ACTION_BLOCKED" then
        OnBlocked((...))
        return
    end
    if #pending == 0 then
        return
    end
    if event == "CHAT_MSG_SYSTEM" then
        OnSystem((...))
        return
    end
    local text, sender, _, _, _, _, _, channelIndex, _, _, _, guid, bnID = ...
    if not canaccessvalue(text, sender, channelIndex, guid, bnID) then
        return
    end
    local kind = ECHO_OF[ChatKind(event) or ""]
    if kind then
        OnEcho(kind, text, sender, channelIndex, guid, bnID)
    end
end)
for kind in pairs(ECHO_OF) do
    events:RegisterEvent("CHAT_MSG_" .. kind)
end
events:RegisterEvent("CHAT_MSG_SYSTEM")
events:RegisterEvent("ADDON_ACTION_BLOCKED")

-- Counts for /tpr debug (no text, no name).
function Channels.Counters()
    local c = {}
    for k, v in pairs(counters) do
        c[k] = v
    end
    c.pending = #pending
    return c
end

-- Forgets every pending send (/tpr conv clear): their bubbles are gone.
function Channels.Clear()
    wipe(pending)
    wipe(recent)
end
