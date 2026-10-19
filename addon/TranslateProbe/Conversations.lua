-- Conversations of the conversation window (ns.Conv): one per sender, memory only.
--
-- A conversation is keyed by the sender's GUID (arg12), else "bn:" .. the Battle.net account id
-- (arg13; a Battle.net name, arg2, is never read), else the full name. A [T] click (FromLine)
-- creates or reopens it; later readable lines of that sender are followed (OnLineTracked, called
-- by ChatButton for every new tracked line) by the follow setting:
--   off        only [T] clicks add messages
--   add        added untranslated (the original in the bubble and a Translate button)
--   smart      (default) translated when the conversation is a whisper, once I replied in it, or
--              when its "Translate new messages" box is ticked; a public first contact as add
--   translate  always translated
-- The window asks the Helper only for bubbles it shows (ConvFrame's slot manager).
--
-- Closing a tab keeps the conversation (user decision D10, 2026-10-19): a later message of that
-- sender that is translated (a [T] click, or a followed line in translate mode) reopens the tab
-- with the whole history; a followed line in add mode joins the history and the tab stays
-- closed. Bounds: 30 conversations (open and closed), 100 messages each (the oldest goes). The
-- 31st evicts the least recently active closed conversation first, then the least recently
-- active open one that has no unread message, no pending reply and is not selected; when none
-- can go, the new one is refused. Everything here is lost on /reload and logout: nothing is
-- written to the saved variables.
--
-- conv = { key, name, guid, bnID, classFile, last = { event, sender, zoneChannelID, channelIndex,
--   channelBase, bnID, lineID }, replied, follow (nil, or true/false: the tab's own choice),
--   unread, draft, channelChoice (the reply composer's), msgs, open, whisper, pendingReply (set by
--   the reply composer while its reply belongs to this conversation) }
-- msg (incoming) = { kind = "in", lineID, text, event, channelIndex, channelBase, zoneChannelID,
--   at (GetTime), time ("14:02"), state = "plain" | "translate", K, req (Request.StartWindow),
--   noCache, showOriginal }
-- msg (mine) = { kind = "out", text, draft, label, chatType, target, state, at, time }; state is
--   "sending", "sent", "unconfirmed", "offline", "blocked" (Channels) or "typed" (a whisper I
--   typed in the normal chat to this sender).
local _, ns = ...

local Conv = {}
ns.Conv = Conv

local MAX_CONVS = 30
local MAX_MSGS = 100
local LANG_LINES = 5            -- the source language is decided on the sender's last 5 lines
local ECHO_SECONDS = 5          -- a whisper echo this soon after a window send is that send
Conv.MAX_CONVS, Conv.MAX_MSGS = MAX_CONVS, MAX_MSGS

local convs = {}                -- key -> conv
local order = {}                -- keys of the open conversations, in tab order
local selectedKey
local activity = 0              -- a counter: each conversation keeps the value of its last activity
local listeners = { select = {}, change = {}, close = {}, incoming = {} }
local counters = { created = 0, evicted = 0, refused = 0, reopened = 0, followed = 0, dropped = 0, typed = 0 }

local WHISPERS = { CHAT_MSG_WHISPER = true, CHAT_MSG_BN_WHISPER = true }

local function IsPlainString(v)
    return not issecretvalue(v) and type(v) == "string"
end

---------------------------------------------------------------------------- events
-- Conv.On(event, fn): "select" (conv or nil), "change" (conv or nil, what: "msg", "state",
-- "lang", "follow", "order", "meta", "evict", "clear"), "close" (conv), "incoming" (conv, msg).
function Conv.On(event, fn)
    local list = listeners[event]
    assert(list, "unknown conversation event " .. tostring(event))
    list[#list + 1] = fn
end

local function Fire(event, ...)
    for _, fn in ipairs(listeners[event]) do
        fn(...)
    end
end

local function Touch(conv)
    activity = activity + 1
    conv.active = activity
end

---------------------------------------------------------------------------- lookup
-- Key of a sender from line details (ChatButton.LineInfo), or nil.
function Conv.KeyOf(info)
    if type(info) ~= "table" then
        return nil
    end
    if IsPlainString(info.guid) and info.guid ~= "" then
        return info.guid
    elseif type(info.bnID) == "number" and info.bnID ~= 0 then
        return "bn:" .. info.bnID
    elseif IsPlainString(info.sender) and info.sender ~= "" then
        return "name:" .. info.sender
    end
end

function Conv.Get(key)
    return key and convs[key]
end

local function ByName(name)
    for _, c in pairs(convs) do
        if c.name and (c.name == name or Ambiguate(c.name, "none") == Ambiguate(name, "none")) then
            return c
        end
    end
end

local function IndexOf(key)
    for i, k in ipairs(order) do
        if k == key then
            return i
        end
    end
end

function Conv.Selected()
    return selectedKey and convs[selectedKey]
end

-- The open conversations, in tab order.
function Conv.List()
    local list = {}
    for i, key in ipairs(order) do
        list[i] = convs[key]
    end
    return list
end

-- Every conversation kept (open and closed), most recently active first.
function Conv.All()
    local list = {}
    for _, c in pairs(convs) do
        list[#list + 1] = c
    end
    table.sort(list, function(a, b)
        return (a.active or 0) > (b.active or 0)
    end)
    return list
end

function Conv.Count()
    local n = 0
    for _ in pairs(convs) do
        n = n + 1
    end
    return n, #order
end

---------------------------------------------------------------------------- sender details
local function ClassOf(guid)
    if not (IsPlainString(guid) and guid:find("^Player%-")) then
        return nil
    end
    local ok, _, classFile = pcall(GetPlayerInfoByGUID, guid)
    if ok and IsPlainString(classFile) and classFile ~= "" then
        return classFile
    end
end

local function UpdateSender(conv, info, lineID)
    conv.last = { event = info.event or (conv.last and conv.last.event), sender = info.sender,
        zoneChannelID = info.zoneChannelID, channelIndex = info.channelIndex, channelBase = info.channelBase,
        bnID = info.bnID, lineID = lineID }
    if IsPlainString(info.sender) and info.sender ~= "" then
        conv.name = info.sender
    end
    if IsPlainString(info.guid) and info.guid ~= "" then
        conv.guid = info.guid
    end
    if type(info.bnID) == "number" and info.bnID ~= 0 then
        conv.bnID = info.bnID
    end
    conv.classFile = ClassOf(conv.guid) or conv.classFile
    if info.event and WHISPERS[info.event] then
        conv.whisper = true
    end
end

---------------------------------------------------------------------------- language
local function Redetect(conv)
    local acc
    for _, scores in ipairs(conv.langLines) do
        acc = ns.LangDetect.Merge(acc, scores)
    end
    conv.detected = acc and ns.LangDetect.Decide(acc) or nil
end

-- The sender's language: the conversation's own choice (guessed = false), else the language
-- detected on the sender's last lines (guessed = true), or nil when unknown.
function Conv.SourceLang(conv)
    if not conv then
        return nil
    end
    if conv.srcOverride then
        return conv.srcOverride, false
    end
    return conv.detected, true
end

function Conv.SetSourceLang(conv, code)
    conv.srcOverride = code or nil
    Fire("change", conv, "lang")
end

-- The language of my replies in this conversation: my choice for it, else the sender's language
-- when the translator writes it (Lang.TARGETS), else nil.
function Conv.ReplyLang(conv)
    if not conv then
        return nil
    end
    if conv.replyLang then
        return conv.replyLang
    end
    local src = Conv.SourceLang(conv)
    if src and ns.Lang.IsTarget(src) then
        return src
    end
end

function Conv.SetReplyLang(conv, code)
    conv.replyLang = code or nil
    Fire("change", conv, "lang")
end

---------------------------------------------------------------------------- follow
-- The follow setting (off, add, smart or translate; anything else reads as smart).
function Conv.FollowMode()
    local mode = ns.db and ns.db.follow
    if mode == "off" or mode == "add" or mode == "translate" then
        return mode
    end
    return "smart"
end

-- What a followed line of this conversation becomes: "off" (not added), "add" (untranslated) or
-- "translate".
function Conv.EffectiveFollow(conv)
    local mode = Conv.FollowMode()
    if conv.follow == true then
        return "translate"
    elseif conv.follow == false then
        return mode == "off" and "off" or "add"
    elseif mode == "smart" then
        return (conv.whisper or conv.replied) and "translate" or "add"
    end
    return mode
end

-- The tab's own "Translate new messages" choice (true or false), or nil to follow the setting.
function Conv.SetFollow(conv, on)
    conv.follow = on
    Fire("change", conv, "follow")
end

function Conv.MarkReplied(conv)
    if conv and not conv.replied then
        conv.replied = true
        Fire("change", conv, "follow")
    end
end

---------------------------------------------------------------------------- messages
local function DropRequest(msg, silent)
    if not msg.req then
        return
    end
    if silent then
        ns.Reserve.FreeWindowSlot(msg.req.slot)
        msg.req = nil
    else
        ns.Request.DropWindow(msg)
    end
end

local function Trim(conv)
    while #conv.msgs > MAX_MSGS do
        local old = table.remove(conv.msgs, 1)
        if old.lineID then
            conv.lines[old.lineID] = nil
        end
        DropRequest(old)
        counters.dropped = counters.dropped + 1
    end
end

local function NewMsg(conv, msg)
    msg.at = GetTime()
    msg.time = date("%H:%M")
    msg.convKey = conv.key
    conv.msgs[#conv.msgs + 1] = msg
    Trim(conv)
    return msg
end

local function AddIn(conv, lineID, text, info, state)
    local msg = NewMsg(conv, { kind = "in", lineID = lineID, text = text, event = info.event,
        channelIndex = info.channelIndex, channelBase = info.channelBase, zoneChannelID = info.zoneChannelID,
        state = state })
    conv.lines[lineID] = msg
    local scores = ns.LangDetect.Scores(text)
    conv.langLines[#conv.langLines + 1] = scores
    while #conv.langLines > LANG_LINES do
        table.remove(conv.langLines, 1)
    end
    Redetect(conv)
    return msg
end

function Conv.FindLine(conv, lineID)
    return conv and conv.lines[lineID]
end

function Conv.ConvOf(msg)
    return msg and msg.convKey and convs[msg.convKey]
end

-- My message in a conversation: o = { text, draft, label, chatType, target, state }. Returns msg.
function Conv.AddOut(conv, o)
    local msg = NewMsg(conv, { kind = "out", text = o.text, draft = o.draft, label = o.label,
        chatType = o.chatType, target = o.target, state = o.state or "sending" })
    Touch(conv)
    Fire("change", conv, "msg")
    return msg
end

function Conv.SetOutState(msg, state)
    if not msg then
        return
    end
    msg.state = state
    local conv = Conv.ConvOf(msg)
    if conv then
        Fire("change", conv, "state")
    end
end

-- Removes one message (the bubble's "Remove"): its request is cancelled.
function Conv.RemoveMsg(conv, msg)
    for i, m in ipairs(conv.msgs) do
        if m == msg then
            table.remove(conv.msgs, i)
            if msg.lineID then
                conv.lines[msg.lineID] = nil
            end
            DropRequest(msg)
            Fire("change", conv, "msg")
            return true
        end
    end
    return false
end

---------------------------------------------------------------------------- open, close, evict
local function PendingReply(conv)
    if conv.pendingReply then
        return true
    end
    local composer = ns.Composer
    if composer and composer.PendingConv then
        local ok, pending = pcall(composer.PendingConv)
        return ok and pending == conv
    end
    return false
end

local function Remove(conv)
    for _, msg in ipairs(conv.msgs) do
        DropRequest(msg)
    end
    local i = IndexOf(conv.key)
    if i then
        table.remove(order, i)
    end
    convs[conv.key] = nil
    counters.evicted = counters.evicted + 1
    if i then
        Fire("close", conv)
    end
    Fire("change", conv, "evict")
end

-- Makes room for one more conversation; false when none can go.
local function EvictOne()
    for pass = 1, 2 do
        local victim
        for _, c in pairs(convs) do
            local ok = c.key ~= selectedKey
            if c.open then
                ok = ok and pass == 2 and (c.unread or 0) == 0 and not PendingReply(c)
            end
            if ok and (not victim or (c.active or 0) < (victim.active or 0)) then
                victim = c
            end
        end
        if victim then
            Remove(victim)
            return true
        end
    end
    return false
end

local function Create(key)
    if Conv.Count() >= MAX_CONVS and not EvictOne() then
        counters.refused = counters.refused + 1
        return nil
    end
    local conv = { key = key, msgs = {}, lines = {}, langLines = {}, unread = 0, open = false }
    convs[key] = conv
    counters.created = counters.created + 1
    return conv
end

-- Puts a closed conversation back on the tab strip (at the right end).
local function Open(conv)
    if not conv.open then
        conv.open = true
        order[#order + 1] = conv.key
        if conv.msgs[1] then
            counters.reopened = counters.reopened + 1
        end
        Fire("change", conv, "order")
    end
end

function Conv.Select(key)
    local conv = key and convs[key]
    if conv and not conv.open then
        Open(conv)
    end
    selectedKey = conv and key or nil
    Fire("select", conv)
end

-- Closes a tab: the conversation and its messages stay in memory; its bubbles give their slots
-- back. The selected tab hands the selection to its right neighbour, else its left one.
function Conv.Close(key)
    local conv = convs[key]
    if not (conv and conv.open) then
        return
    end
    local i = IndexOf(key)
    table.remove(order, i)
    conv.open = false
    conv.unread = 0
    for _, msg in ipairs(conv.msgs) do
        DropRequest(msg)
    end
    -- a closed conversation is never the selected one, not even while the events run
    local wasSelected = selectedKey == key
    if wasSelected then
        selectedKey = nil
    end
    Fire("close", conv)
    Fire("change", conv, "order")
    if wasSelected then
        Conv.Select(order[i] or order[i - 1])
    end
end

function Conv.CloseOthers(key)
    local keep = {}
    for _, k in ipairs(order) do
        if k ~= key then
            keep[#keep + 1] = k
        end
    end
    for _, k in ipairs(keep) do
        Conv.Close(k)
    end
end

-- Swaps two open conversations on the tab strip (the overflow list puts the one picked in place of
-- the rightmost shown tab).
function Conv.Swap(i, j)
    if order[i] and order[j] and i ~= j then
        order[i], order[j] = order[j], order[i]
        Fire("change", nil, "order")
    end
end

function Conv.IndexOf(conv)
    return conv and IndexOf(conv.key)
end

function Conv.MarkRead(conv)
    if conv and (conv.unread or 0) > 0 then
        conv.unread = 0
        Fire("change", conv, "meta")
    end
end

local function WindowShows(conv)
    local cf = ns.ConvFrame
    return cf and cf.IsShown and cf.IsShown() and selectedKey == conv.key
end

---------------------------------------------------------------------------- lines
-- A [T] click on lineID: the sender's conversation (created, or reopened with its history) and
-- the line's message, which is added unless the line is already there (dup = true), and set to be
-- translated. Returns conv, msg, dup; or nil and a reason ("old": the line's text or details are
-- gone; "full": no conversation can make room).
function Conv.FromLine(lineID)
    local info = ns.ChatButton.LineInfo(lineID)
    local text = ns.ChatButton.SourceText(lineID)
    local key = Conv.KeyOf(info)
    if not (key and text) then
        return nil, "old"
    end
    local conv = convs[key] or Create(key)
    if not conv then
        return nil, "full"
    end
    UpdateSender(conv, info, lineID)
    local msg = conv.lines[lineID]
    local dup = msg ~= nil
    if msg then
        msg.state = "translate"
    else
        msg = AddIn(conv, lineID, text, info, "translate")
    end
    Open(conv)
    Touch(conv)
    Fire("change", conv, "msg")
    return conv, msg, dup
end

-- A new readable line (ChatButton): when its sender has a conversation, it is followed.
function Conv.OnLineTracked(lineID)
    if next(convs) == nil then
        return      -- no conversation: nothing to follow (the usual case, no allocation)
    end
    local info = ns.ChatButton.LineInfo(lineID)
    local key = Conv.KeyOf(info)
    local conv = key and convs[key]
    if not conv or conv.lines[lineID] then
        return
    end
    if info.event and WHISPERS[info.event] then
        conv.whisper = true
    end
    local mode = Conv.EffectiveFollow(conv)
    if mode == "off" then
        return
    end
    local text = ns.ChatButton.SourceText(lineID)
    if not text then
        return
    end
    counters.followed = counters.followed + 1
    UpdateSender(conv, info, lineID)
    local msg = AddIn(conv, lineID, text, info, mode == "translate" and "translate" or "plain")
    msg.followed = true
    if not conv.open and mode ~= "translate" then
        -- kept in the history; the tab comes back with the next translated message
        Fire("change", conv, "msg")
        return
    end
    Open(conv)
    if not WindowShows(conv) then
        conv.unread = (conv.unread or 0) + 1
    end
    Touch(conv)
    Fire("incoming", conv, msg)
    Fire("change", conv, "msg")
end

---------------------------------------------------------------------------- my whispers typed in the chat
local function Unescape(text)
    return (text:gsub("||", "|"))
end

local function RecentOut(conv, text)
    local now = GetTime()
    for i = #conv.msgs, 1, -1 do
        local m = conv.msgs[i]
        if now - (m.at or 0) > ECHO_SECONDS then
            break
        end
        if m.kind == "out" and m.state ~= "typed" and type(m.text) == "string"
            and (m.text == text or Unescape(m.text) == text or m.text == Unescape(text)) then
            return true
        end
    end
    return false
end

-- CHAT_MSG_WHISPER_INFORM / CHAT_MSG_BN_WHISPER_INFORM: a whisper I sent. When it is to a sender
-- who has a conversation and is not the echo of a reply sent from the window, it becomes my
-- bubble (the text as sent, nothing to translate).
local function OnInform(event, text, target, ...)
    if not IsPlainString(text) or text == "" then
        return
    end
    local guid, bnID = select(10, ...)
    local info = {}
    if event == "CHAT_MSG_BN_WHISPER_INFORM" then
        if issecretvalue(bnID) or type(bnID) ~= "number" or bnID == 0 then
            return
        end
        info.bnID = bnID
    else
        if IsPlainString(guid) and guid ~= "" then
            info.guid = guid
        end
        if IsPlainString(target) and target ~= "" then
            info.sender = target
        end
    end
    local key = Conv.KeyOf(info)
    local conv = key and convs[key] or (info.sender and ByName(info.sender))
    if not conv then
        return
    end
    -- one frame later: a send from the window adds its bubble before its echo can arrive, and
    -- the reply composer marks it sent on this same event
    C_Timer.After(0, function()
        if convs[conv.key] ~= conv or RecentOut(conv, text) then
            return
        end
        counters.typed = counters.typed + 1
        Conv.AddOut(conv, { text = text, state = "typed",
            chatType = event == "CHAT_MSG_BN_WHISPER_INFORM" and "BN_WHISPER" or "WHISPER" })
    end)
end

local events = CreateFrame("Frame")
events:RegisterEvent("CHAT_MSG_WHISPER_INFORM")
events:RegisterEvent("CHAT_MSG_BN_WHISPER_INFORM")
events:SetScript("OnEvent", function(_, event, ...)
    OnInform(event, ...)
end)

---------------------------------------------------------------------------- clear, counters
-- /tpr conv clear, /tpr clear: every conversation goes. silent = true when the Helper already
-- forgot every card (a CLEAR was sent): the slots are freed without a CANCEL each.
function Conv.Clear(silent)
    for _, c in pairs(convs) do
        for _, msg in ipairs(c.msgs) do
            DropRequest(msg, silent)
        end
    end
    local had = next(convs) ~= nil
    wipe(convs)
    wipe(order)
    selectedKey = nil
    if had then
        Fire("select", nil)
        Fire("change", nil, "clear")
    end
end

-- Counts only (for /tpr debug): never a name or a text.
function Conv.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    local msgs, slots = 0, 0
    for _, conv in pairs(convs) do
        msgs = msgs + #conv.msgs
        for _, m in ipairs(conv.msgs) do
            slots = slots + (m.req and 1 or 0)
        end
    end
    c.kept, c.open = Conv.Count()
    c.messages, c.slots = msgs, slots
    return c
end
