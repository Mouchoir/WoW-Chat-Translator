-- Reply, send and settings checks (work package R): Channels.lua, Consent.lua, Composer.lua,
-- Core.lua (settings), Options.lua, Commands.lua, ReplyLocales.lua. Run by tests\harness.lua
-- with the shared context H (see its header).
--
-- The composer is tested on a window of its own, with stand-ins for the window's modules
-- (Conversations, Bubble, Popup, LangDetect, ConvFrame) so these checks do not depend on how the
-- window is built; the real modules are put back at the end, and the composer goes back to the
-- window's host. Everything typed here is synthetic and registered with H.Private, so the final
-- privacy walk checks it never reaches the saved variables.
local H = ...
local M, ns, L, db, check, section = H.M, H.ns, H.L, H.db, H.check, H.section
local Composer, Channels, Consent, Options = ns.Composer, ns.Channels, ns.Consent, ns.Options
local fr = H.fr
local MY = db.target                    -- my language (en, or fr on a French client)

local function Contains(s, fragment)
    return type(s) == "string" and s:find(fragment, 1, true) ~= nil
end

---------------------------------------------------------------------------- stand-ins
local real = { Conv = ns.Conv, Bubble = ns.Bubble, Popup = ns.Popup, LangDetect = ns.LangDetect,
    ConvFrame = ns.ConvFrame, host = Composer.Host(), selected = nil }
if real.Conv and real.Conv.Selected then
    real.selected = real.Conv.Selected()
end
local saved = { display = db.display, glossShow = db.glossShow, follow = db.follow, replyClipboard = db.replyClipboard,
    convText = db.convText, convScale = db.convScale, convCombat = db.convCombat }

local Fake = { outs = {}, replied = {}, cycled = {}, relayouts = 0, syncs = 0, toggles = 0, detect = {} }
local FakeConv = {
    ReplyLang = function(c)
        return c.replyLang or c.lang
    end,
    SetReplyLang = function(c, code)
        c.replyLang = code
    end,
    SourceLang = function(c)
        return c.lang
    end,
    AddOut = function(c, o)
        local msg = { conv = c, o = o, states = {} }
        Fake.outs[#Fake.outs + 1] = msg
        return msg
    end,
    SetOutState = function(msg, state)
        msg.state = state
        msg.states[#msg.states + 1] = state
    end,
    MarkReplied = function(c)
        c.replied = true
        Fake.replied[#Fake.replied + 1] = c
    end,
    Selected = function()
        return Composer.Conversation()
    end,
    Clear = function(silent)
        Fake.cleared = silent
    end,
}
local FakeBubble = {
    AcquireReply = function(parent)
        local card = CreateFrame("Frame", nil, parent)
        card.EditButton = CreateFrame("Button", nil, card, "UIPanelButtonTemplate")
        card.CancelButton = CreateFrame("Button", nil, card, "UIPanelButtonTemplate")
        function card:SetDraft(text)
            self.draft = text
        end
        function card:SetRows(slot, K, sessBit)
            self.slot, self.K, self.sessBit = slot, K, sessBit
        end
        function card:Layout(width, m)
            self.width, self.m = width, m
            self:SetSize(width, 82)
            return 82
        end
        function card:CardRect()
            return self:GetScaledRect()
        end
        Fake.card = card
        return card
    end,
    Release = function(card)
        card:Hide()
        Fake.released = (Fake.released or 0) + 1
    end,
}
local FakePopup = {
    Open = function(owner, items)
        Fake.popupOwner, Fake.popup = owner, items
    end,
    Close = function()
        Fake.popup = nil
    end,
}
local FakeDetect = {
    Detect = function(text)
        return Fake.detect[text]
    end,
}
local win = CreateFrame("Frame", nil, UIParent)
win:SetSize(440, 540)
win:SetPoint("CENTER", UIParent, "CENTER", -300, 60)
local host = CreateFrame("Frame", nil, win)
host:SetPoint("BOTTOMLEFT", win, "BOTTOMLEFT", 0, 0)
host:SetPoint("BOTTOMRIGHT", win, "BOTTOMRIGHT", 0, 0)
host:SetHeight(88)
local FakeFrame = {
    composerHost = host,
    Metrics = function()
        return { ppu = 1.40625, W = 4, H = 16, rowH = 16, rowPx = 22, fontPx = 20, pad = 6, inset = 16, border = 10,
            bubbleWidth = 360, plateWidth = 300, x0 = 20 }
    end,
    Relayout = function()
        Fake.relayouts = Fake.relayouts + 1
        host:SetHeight(Composer.Height())
    end,
    SyncView = function()
        Fake.syncs = Fake.syncs + 1
    end,
    CycleTab = function(dir)
        Fake.cycled[#Fake.cycled + 1] = dir
    end,
    Toggle = function()
        Fake.toggles = Fake.toggles + 1
    end,
    IsShown = function()
        return true
    end,
}
ns.Conv, ns.Bubble, ns.Popup, ns.LangDetect, ns.ConvFrame = FakeConv, FakeBubble, FakePopup, FakeDetect, FakeFrame

M.SetChannels({ [1] = "General - City", [2] = "Trade - City" })
M.SetGroup({ party = false, raid = false, instance = false, guild = false, officer = false, inInstance = false })

-- Conversations as the window keeps them (docs: conv = { key, name, guid, bnID, last, ... }).
local function NewConv(event, sender, lang, extra)
    local c = { key = sender or "?", name = sender, lang = lang, msgs = {},
        last = { event = event, sender = sender, lineID = 1 } }
    for k, v in pairs(extra or {}) do
        if k == "last" then
            for lk, lv in pairs(v) do
                c.last[lk] = lv
            end
        else
            c[k] = v
        end
    end
    return c
end
local convW = NewConv("CHAT_MSG_WHISPER", "Neco-OtherRealm", "de", { guid = "Player-2-0001" })
local convT = NewConv("CHAT_MSG_CHANNEL", "Brunhilde", "de", { last = { channelIndex = 2, channelBase = "Trade - City",
    zoneChannelID = 2 } })
local convB = NewConv("CHAT_MSG_BN_WHISPER", nil, "de", { bnID = 77, key = "bn:77", name = "Friend" })
convB.last.sender = nil
local convFR = NewConv("CHAT_MSG_SAY", "Pierre", MY)

local ui
local function Box()
    return Composer.Widgets().box
end
local function Stepper()
    return Composer.Widgets().stepper:GetText() or ""
end
local function Buttons()
    local list = {}
    for _, b in ipairs(Composer.Widgets().buttons) do
        if b:IsShown() then
            list[#list + 1] = b
        end
    end
    return list
end
local function ButtonFor(id)
    for _, b in ipairs(Buttons()) do
        if b.targetId == id then
            return b
        end
    end
end
local function SentCount()
    return #M.SentChat()
end
local function LastSent()
    local s = M.SentChat()
    return s[#s]
end
-- (a click into a box that has no focus highlights its text, as InputBoxTemplate does: typing
-- then replaces it, so the focus is taken only when missing)
local function Focus()
    if not Box():HasFocus() then
        Box():SetFocus()
    end
end
local function Type(text, burst)
    H.Private(text)
    Focus()
    M.Type(Box(), text, burst and { burst = true } or nil)
end
local function Paste(text)
    H.Private(text)
    Focus()
    return M.Paste(Box(), text)
end
local function Wait(seconds)
    M.Tick(math.ceil(seconds * 60) + 1)
end
local function Translations(from)
    return H.Since(from, "translate")
end

Composer.Attach(host)
ui = Composer.Widgets()

---------------------------------------------------------------------------- wire text
section("reply: wire text and the 255-byte limit")
do
    local W = Channels.Wire
    check(W("a|b") == "a||b" and W("a||b") == "a||b" and W("|cffff0000red|r") == "||cffff0000red||r",
        "canonical pipe escaping: every single pipe doubled, an escaped pipe kept")
    local idem = true
    for _, s in ipairs({ "a|b", "a||b", "|||", "||||", "x|cff|r||y", "plain text", "é|ß|한" }) do
        local once = W(s)
        if W(once) ~= once then
            idem = false
        end
    end
    check(idem, "escaping is idempotent (raw or already doubled pipes give the same wire)")
    check(W("  \1hi\2 \3  ") == "hi" and W("   ") == nil and W("") == nil and W(nil) == nil,
        "control bytes 0x01-0x03 removed, trimmed, empty refused")
    check(W("line one\r\nline two") == "line one line two", "line breaks become spaces")
    local _, b1 = W(string.rep("a", 254) .. "|")
    local _, b2 = W(string.rep("é", 127) .. "a")
    check(b1 == 256 and b2 == 255, "bytes are counted on the escaped result (UTF-8 and pipes)")

    Composer.SetConversation(convFR)
    local long = string.rep("a", 254) .. "|"
    Type(long, true)
    local b = Buttons()[1]
    check(b and not b:IsEnabled() and Contains(Stepper(), L.RPL_STEP_TOO_LONG:format(1))
        and ui.bytes:GetText() == L.RPL_BYTES:format(256), "over 255 bytes after escaping: buttons greyed, 'Too long by 1'")
    M.Hover(b)
    check(GameTooltip:IsShown() and Contains(table.concat(GameTooltip.lines, "\n"), L.RPL_WHY_TOO_LONG:format(1)),
        "a greyed button keeps a tooltip with the reason")
    M.Leave()
    Box():SetText("")
end

---------------------------------------------------------------------------- targets and API calls
section("reply: channel targets and the calls they make")
do
    local cases = {
        { "CHAT_MSG_WHISPER", "WHISPER" }, { "CHAT_MSG_SAY", "SAY" }, { "CHAT_MSG_EMOTE", "SAY" },
        { "CHAT_MSG_YELL", "YELL" }, { "CHAT_MSG_PARTY", "PARTY" }, { "CHAT_MSG_PARTY_LEADER", "PARTY" },
        { "CHAT_MSG_RAID", "RAID" }, { "CHAT_MSG_RAID_LEADER", "RAID" }, { "CHAT_MSG_RAID_WARNING", "RAID" },
        { "CHAT_MSG_INSTANCE_CHAT", "INSTANCE_CHAT" }, { "CHAT_MSG_INSTANCE_CHAT_LEADER", "INSTANCE_CHAT" },
        { "CHAT_MSG_GUILD", "GUILD" }, { "CHAT_MSG_OFFICER", "OFFICER" },
    }
    local wrong = {}
    for _, case in ipairs(cases) do
        local c = NewConv(case[1], "Someone", "de")
        local t = Channels.Targets(c)[1]
        if not (t and t.id == case[2] and t.isDefault) then
            wrong[#wrong + 1] = case[1]
        end
    end
    check(#wrong == 0, "default button = where the sender wrote (" .. table.concat(wrong, ", ") .. ")")
    local tT = Channels.Targets(convT)
    check(#tT == 3 and tT[1].id == "CHANNEL:Trade - City" and tT[1].label == "2. Trade" and tT[1].target == 2
        and tT[2].id == "WHISPER" and tT[3].id == "SAY", "Trade line: [2. Trade] [Whisper] [Say]")
    local tW = Channels.Targets(convW)
    check(#tW == 2 and tW[1].id == "WHISPER" and tW[1].target == "Neco-OtherRealm" and tW[2].id == "SAY",
        "whisper: [Whisper] (default, cross-realm name) [Say]")
    local tS = Channels.Targets(NewConv("CHAT_MSG_SAY", "Someone", "de"))
    check(tS[1].id == "SAY" and tS[2].id == "WHISPER" and tS[3].id == "YELL", "say: [Say] [Whisper] [Yell]")
    local tB = Channels.Targets(convB)
    check(#tB == 1 and tB[1].id == "BN_WHISPER" and tB[1].target == 77 and #Channels.Others(convB) == 0,
        "Battle.net sender: Whisper only, by account id")
    local labels = {}
    for _, t in ipairs(Channels.Others(convT)) do
        labels[#labels + 1] = t.label
    end
    check(table.concat(labels, ",") == table.concat({ SAY, YELL, PARTY, RAID, INSTANCE_CHAT, GUILD, OFFICER,
        "1. General", "2. Trade", L.RPL_WHISPER_NAME:format("Brunhilde") }, ","),
        "[Other]: say, yell, party, raid, instance, guild, officer, joined channels, Whisper <name>")
    check(SAY == (fr and "Dire" or "Say") and tS[1].label == SAY, "labels are Blizzard's localized globals")

    -- availability greying
    local function Find(id)
        return Channels.Find(convT, id)
    end
    check(not Find("PARTY").enabled and Find("PARTY").reason == "RPL_WHY_NO_GROUP" and not Find("RAID").enabled
        and not Find("INSTANCE_CHAT").enabled and Find("GUILD").reason == "RPL_WHY_NO_GUILD"
        and Find("OFFICER").reason == "RPL_WHY_NO_GUILD", "solo: party, raid, instance, guild and officer greyed")
    M.SetGroup({ party = true, guild = true })
    check(Find("PARTY").enabled and not Find("RAID").enabled and Find("GUILD").enabled
        and Find("OFFICER").reason == "RPL_WHY_NOT_OFFICER", "in a party and a guild: party and guild, not officer")
    M.SetGroup({ raid = true, instance = true, officer = true })
    check(Find("RAID").enabled and Find("INSTANCE_CHAT").enabled and Find("OFFICER").enabled, "raid, instance, officer")
    M.SetChannels({ [1] = "General - City" })
    check(not Find("CHANNEL:Trade - City").enabled and Find("CHANNEL:Trade - City").reason == "RPL_WHY_LEFT",
        "a channel I left is greyed: 'You left this channel'")
    M.SetChannels({ [1] = "General - City", [4] = "Trade - City" })
    check(Find("CHANNEL:Trade - City").target == 4 and Find("CHANNEL:Trade - City").label == "4. Trade",
        "the channel number is read again at send time (by name)")
    -- The old number never sends to whatever channel took it since (review finding R1).
    local convZ = NewConv("CHAT_MSG_CHANNEL", "Neco", "de", { last = { channelIndex = 1,
        channelBase = "General - Elwynn Forest", zoneChannelID = 1 } })
    M.SetChannels({ [1] = "General - Elwynn Forest", [2] = "Trade - City" })
    local z0 = Channels.Targets(convZ)[1]
    M.SetChannels({ [1] = "General - Westfall", [2] = "Trade - City" })
    local z1 = Channels.Targets(convZ)[1]
    check(z0.enabled and z0.target == 1 and z1.id == "CHANNEL:General - Elwynn Forest" and z1.isDefault
        and not z1.enabled and z1.target == nil and z1.reason == "RPL_WHY_LEFT" and z1.label == "1. General",
        "a zone channel left for another zone is greyed, not sent to the new zone's channel at that number")
    local convG = NewConv("CHAT_MSG_CHANNEL", "Brunhilde", "de", { last = { channelIndex = 4,
        channelBase = "LookingForGroup" } })
    M.SetChannels({ [2] = "Trade - City", [4] = "mycustom" })
    local g1 = Channels.Targets(convG)[1]
    check(g1.id == "CHANNEL:LookingForGroup" and not g1.enabled and g1.target == nil
        and g1.label == "4. LookingForGroup" and g1.reason == "RPL_WHY_LEFT",
        "a channel left and replaced by another at the same number is greyed with its own name")
    M.SetChannels({ [2] = "Trade - City", [5] = "LookingForGroup" })
    check(Channels.Targets(convG)[1].target == 5 and Channels.Targets(convG)[1].enabled,
        "the same channel under a new number is found by its name")
    local convN = NewConv("CHAT_MSG_CHANNEL", "Brunhilde", "de", { last = { channelIndex = 2 } })
    check(Channels.Default(convN) == "WHISPER", "a channel line without its channel name never defaults to its number")
    M.SetChannels({ [1] = "General - City", [2] = "Trade - City" })

    -- every target through a real button click (a hardware event)
    local expect = {
        { convW, "WHISPER", "WHISPER", "Neco-OtherRealm" }, { convT, "CHANNEL:Trade - City", "CHANNEL", 2 },
        { convB, "BN_WHISPER", "BN_WHISPER", 77 },
    }
    for _, id in ipairs({ "SAY", "YELL", "PARTY", "RAID", "INSTANCE_CHAT", "GUILD", "OFFICER" }) do
        expect[#expect + 1] = { convT, id, id, nil }
    end
    local bad = {}
    for i, e in ipairs(expect) do
        local c = e[1]
        c.channelChoice = (e[2] ~= Channels.Default(c) and e[2] ~= "WHISPER") and e[2] or nil
        Composer.SetConversation(c)
        Type("synthetic reply number " .. i, true)
        Fake.detect["synthetic reply number " .. i] = "de"
        local b = ButtonFor(e[2])
        local before = SentCount()
        local clicked = b and M.ClickButton(b)
        local rec = LastSent()
        if not (clicked and SentCount() == before + 1 and rec.chatType == e[3] and rec.target == e[4]
            and rec.text == "synthetic reply number " .. i and rec.hardware and not rec.blocked) then
            bad[#bad + 1] = e[2]
        end
        Wait(1.1)
    end
    check(#bad == 0, "each button sends once with the right call: SendChatMessage(text, type, nil, target) or "
        .. "C_BattleNet.SendWhisper(id, text), from a hardware click (" .. table.concat(bad, ", ") .. ")")
    check(convT.channelChoice == "OFFICER" and Channels.Targets(convT)[3].id == "OFFICER",
        "my last channel in a conversation replaces the third button")
    convT.channelChoice = nil
    M.SetGroup({ party = false, raid = false, instance = false, guild = false, officer = false })
end

---------------------------------------------------------------------------- Enter, one click, guard
section("reply: Enter never sends, one click sends once")
do
    Composer.SetConversation(convFR)
    local before, n = SentCount(), #H.sent
    Type("bonne idée, on y va")
    check(Composer.Mode() == "DRAFT", "typing: DRAFT")
    M.Key(Box(), "ENTER")
    check(SentCount() == before and #Translations(n) == 0 and not Box():HasFocus()
        and Contains(Stepper(), L.RPL_STEP_CLICK_CHANNEL), "same language: Enter clears the focus, sends nothing")
    check(not ui.translate:IsShown() and Contains(Stepper(), L.RPL_STEP_CLICK_CHANNEL)
        and ui.box.Instructions:GetText() == L.RPL_PH_SAME,
        "same language: no Translate button")
    local b = Buttons()[1]
    M.ClickButton(b)
    M.ClickButton(b)
    check(SentCount() == before + 1 and LastSent().text == "bonne idée, on y va", "a double click sends once")
    check(Box():GetText() == "" and Composer.Mode() == "SENT" and Contains(Stepper(), L.RPL_STEP_SENT:format(b.targetLabel)),
        "after a send: box emptied, 'Sent to ...'")
    Type("encore")
    M.ClickButton(Buttons()[1])
    check(SentCount() == before + 1, "a second send within 1 s is ignored")
    Wait(1.1)
    M.ClickButton(Buttons()[1])
    check(SentCount() == before + 2, "after 1 s the next click sends")
    Wait(4.5)
    check(Composer.Mode() == "EMPTY", "'Sent' goes back to EMPTY after a few seconds")
end

---------------------------------------------------------------------------- consent and flags
section("reply: clipboard consent and TRANSLATE flags")
local DRAFT = "on part dans 5 min ?"
H.Private(DRAFT)
local function TranslateDraft(c, text, shift)
    Composer.SetConversation(c)
    Box():SetText("")
    Type(text or DRAFT)
    local n = #H.sent
    if shift then
        M.ClickButton(ui.translate, "LeftButton", { shift = true })
    else
        M.Key(Box(), "ENTER")
    end
    return n
end
do
    db.replyClipboard = "ask"
    local n = TranslateDraft(convT)
    local dlg = Consent.Frame()
    check(Consent.IsShown() and #Translations(n) == 0 and not Box():HasFocus(),
        "ask: Enter opens the consent dialog, the box gives up its focus, nothing is sent yet")
    check(dlg.title:GetText() == L.RPL_CONSENT_TITLE and dlg.body:GetText() == L.RPL_CONSENT_BODY
        and dlg.remember.Text:GetText() == L.RPL_CONSENT_REMEMBER and dlg:GetFrameStrata() == "DIALOG"
        and dlg:GetWidth() == 380 and dlg:GetHeight() >= 196, "the dialog: title, explanation, Remember my choice")
    M.Key(dlg, "ENTER")
    check(Consent.IsShown() and #Translations(n) == 0 and dlg:GetPropagateKeyboardInput(),
        "Enter decides nothing in the dialog (no default button) and reaches the game")
    -- review finding R2: a key before combat never leaves the keyboard captured in combat
    M.SetCombat(true)
    M.Key(dlg, "W")
    M.Key(dlg, "1")
    check(Consent.IsShown() and #Translations(n) == 0
        and (dlg:GetPropagateKeyboardInput() or not dlg:IsKeyboardEnabled()),
        "Enter, then combat: movement and ability keys still reach the game")
    M.SetCombat(false)
    M.ClickButton(dlg.allow)
    local tr = Translations(n)[1]
    check(tr and tr.ok and tr.slot == 126 and tr.flags == 14 and tr.target == "de" and tr.text == DRAFT
        and #tr.pairs == 0 and tr.rows >= 2 and tr.rows <= 6, "Allow: TRANSLATE slot 126, plate+reply+copy (14), 0 pairs")
    check(db.replyClipboard == "ask" and not Consent.IsShown(), "Allow without Remember asks again next time")
    check(Composer.Mode() == "WAITING" and Box():GetText() == "" and M.Focused() == Box()
        and ui.box.Instructions:GetText() == L.RPL_PH_PASTE, "WAITING: box emptied and focused, 'Press Ctrl+V...'")
    local card = Fake.card
    check(card and card:IsShown() and card.draft == DRAFT and card.slot == 126 and card.K == tr.rows
        and card.sessBit == ns.sessionId % 2 and card.width == 360,
        "the reply card: draft, rows on slot 126, bubble column width")
    check(Composer.Height() == 88 + 82 + 6 and host:GetHeight() == 176 and Composer.ReplyCardRect() ~= nil
        and Fake.syncs > 0, "composer height 88 + card + 6; the card rectangle is offered for VIEW 200")
    check(Contains(Stepper(), L.RPL_STEP_1) and Contains(Stepper(), L.RPL_STEP_2) and Contains(Stepper(), L.RPL_STEP_3),
        "stepper: 1 Write > 2 Paste (Ctrl+V) > 3 Send")

    n = TranslateDraft(convT)
    M.ClickButton(dlg.deny)
    tr = Translations(n)[1]
    check(tr and tr.flags == 6 and db.replyClipboard == "ask" and ui.box.Instructions:GetText() == L.RPL_PH_TYPE,
        "Deny: plate+reply only (6), 'Type the translation shown above', asks again next time")

    n = TranslateDraft(convT)
    M.ClickButton(dlg.remember)
    M.Key(dlg, "ESCAPE")
    tr = Translations(n)[1]
    check(tr and tr.flags == 6 and db.replyClipboard == "ask", "Escape means Deny, this time only (Remember ignored)")

    n = TranslateDraft(convT)
    check(not dlg.remember:GetChecked(), "Remember starts unticked")
    M.ClickButton(dlg.remember)
    M.ClickButton(dlg.allow)
    check(Translations(n)[1].flags == 14 and db.replyClipboard == "allow", "Remember + Allow saves allow")
    n = TranslateDraft(convT)
    check(not Consent.IsShown() and Translations(n)[1].flags == 14, "allow: no dialog, copy bit set")

    db.replyClipboard = "ask"
    n = TranslateDraft(convT)
    M.ClickButton(dlg.remember)
    M.ClickButton(dlg.deny)
    check(Translations(n)[1].flags == 6 and db.replyClipboard == "never", "Remember + Deny saves never")
    n = TranslateDraft(convT)
    check(not Consent.IsShown() and Translations(n)[1].flags == 6, "never: no dialog, no copy bit")

    db.replyClipboard = "allow"
    n = TranslateDraft(convT, nil, true)
    check(Translations(n)[1].flags == 15, "Shift-click Translate adds noCache (15)")
    db.replyClipboard = "never"
    n = TranslateDraft(convT, nil, true)
    check(Translations(n)[1].flags == 7, "never + Shift-click: 7")
    db.replyClipboard = "allow"

    local longDraft = string.rep("wort ", 70)
    n = TranslateDraft(convT, longDraft)
    check(Translations(n)[1].rows == 6, "a long draft: rows capped at 6 (status row + 5 text rows)")
    n = TranslateDraft(convT, "ja")
    check(Translations(n)[1].rows == 2, "a short draft: 2 rows (status + 1)")
    local reqA = Composer.Reply().reqId
    n = TranslateDraft(convW)
    check(Composer.Reply().conv == convW and Composer.Reply().reqId ~= reqA and #H.Since(n, "cancel") == 0
        and convW.pendingReply == true and convT.pendingReply == nil,
        "a Translate in another conversation replaces the reply on the same slot (no CANCEL needed)")
    Composer.SetConversation(convT)
    check(not (Fake.card and Fake.card:IsShown()) and Composer.Height() == 88 and Composer.ReplyCardRect() == nil,
        "the card shows only in the conversation the reply belongs to")
end

---------------------------------------------------------------------------- paste detection
section("reply: paste detection")
local TRANSLATION = "Brechen wir in 5 Minuten auf?"
H.Private(TRANSLATION)
do
    TranslateDraft(convT)
    Wait(1.5)
    check(Paste(TRANSLATION) and Composer.Mode() == "PASTED" and Contains(Stepper(), L.RPL_STEP_2_PASTED)
        and Contains(Stepper(), L.RPL_STEP_3_CLICK), "Ctrl+V in WAITING: PASTED, '2 Pasted', '3 Click a channel'")
    M.Key(Box(), "ENTER")
    check(not Box():HasFocus() and Box():GetText() == TRANSLATION and Contains(Stepper(), L.RPL_STEP_CLICK_CHANNEL),
        "Enter in PASTED: focus cleared, nothing sent")

    TranslateDraft(convT)
    Wait(1.5)
    Type("Hallo zusammen", true)
    check(Composer.Mode() == "PASTED", "2+ characters at once into the emptied box (no Ctrl): a paste")

    TranslateDraft(convT)
    Type("H")
    check(Composer.Mode() == "DRAFT" and Fake.card:IsShown(), "one typed character in WAITING: DRAFT, the card stays")
    Type("allo", true)
    check(Composer.Mode() == "DRAFT", "an IME-like burst outside WAITING is typing, not a paste")

    local n = TranslateDraft(convT)
    M.Key(Box(), "ENTER")
    check(Composer.Mode() == "WAITING" and #Translations(n) == 1 and Contains(Stepper(), L.RPL_STEP_PASTE_FIRST),
        "Enter in WAITING sends nothing: 'Paste with Ctrl+V first'")
    Wait(1.5)
    Paste("Zeile eins\nZeile zwei")
    check(Box():GetText() == "Zeile eins" and Composer.Mode() == "PASTED" and Contains(Stepper(), L.RPL_STEP_ONE_LINE),
        "a multi-line paste keeps the first line, with a warning")
    H.Private("Zeile eins")
end

---------------------------------------------------------------------------- send checks
section("reply: send checks arm the button")
local function PastedReply(text, early)
    TranslateDraft(convT)
    if not early then
        Wait(1.5)
    end
    Paste(text)
    return ButtonFor("CHANNEL:Trade - City")
end
do
    local function Arms(text, warning, early)
        local b = PastedReply(text, early)
        local before = SentCount()
        M.ClickButton(b)
        local warned = SentCount() == before and Contains(Stepper(), warning)
        M.ClickButton(b)
        local sent = SentCount() == before + 1
        Wait(1.1)
        return warned and sent
    end
    check(Arms(DRAFT, L.RPL_WARN_OWN_TEXT), "own text pasted: the first click warns, the second sends")
    check(Arms("pw: Pa55word!x", L.RPL_WARN_SUSPICIOUS), "a password-like token: warned")
    check(Arms("go to www.example.com now", L.RPL_WARN_SUSPICIOUS), "a URL: warned")
    check(Arms("mail me at someone@example.org", L.RPL_WARN_SUSPICIOUS), "an e-mail address: warned")
    Fake.detect["C'est parti dans cinq minutes"] = "fr"
    check(Arms("C'est parti dans cinq minutes",
        L.RPL_WARN_LANG:format(L.RPL_LANG_FR, L.RPL_LANG_DE)), "another language than the reply language: warned")
    check(Arms(TRANSLATION, L.RPL_WARN_EARLY, true), "pasted under 1 s after Translate: warned")
    local plainWords = "Aujourd'hui, l'entrée: Brechen"
    local b = PastedReply(plainWords)
    local before = SentCount()
    M.ClickButton(b)
    check(SentCount() == before + 1, "a clean paste sends at the first click (apostrophes and capitals are words)")
    H.Private(plainWords)
    Wait(1.1)

    -- the arm lasts 3 s and any text change disarms
    b = PastedReply(DRAFT)
    before = SentCount()
    M.ClickButton(b)
    Wait(3.2)
    M.ClickButton(b)
    check(SentCount() == before and Contains(Stepper(), L.RPL_WARN_OWN_TEXT), "after 3 s the arm is gone: warned again")
    Type(" ")
    M.ClickButton(b)
    check(SentCount() == before, "a text change disarms: warned again")
    M.ClickButton(b)
    check(SentCount() == before + 1, "then the second click sends")
    Wait(1.1)

    -- DRAFT: still in my own language while the reply language differs
    Composer.SetConversation(convT)
    local mine = fr and "je viens tout de suite" or "I am coming right now"
    Fake.detect[mine] = MY
    Box():SetText("")
    Type(mine)
    before = SentCount()
    M.ClickButton(ButtonFor("CHANNEL:Trade - City"))
    check(SentCount() == before and Contains(Stepper(), L.RPL_WARN_NOT_TRANSLATED),
        "typed text in my own language: 'Not translated yet - click again...'")
    M.ClickButton(ButtonFor("CHANNEL:Trade - City"))
    check(SentCount() == before + 1 and LastSent().text == mine, "the second click sends it as it is")
    Wait(1.1)
end

---------------------------------------------------------------------------- after a send
section("reply: after a send")
do
    local b = PastedReply(TRANSLATION)
    local reply = Composer.Reply()
    local n, before, outs = #H.sent, SentCount(), #Fake.outs
    M.ClickButton(b)
    local cancels = H.Since(n, "cancel")
    local out = Fake.outs[outs + 1]
    check(SentCount() == before + 1 and LastSent().text == TRANSLATION and LastSent().chatType == "CHANNEL",
        "the pasted translation goes to the channel")
    check(#cancels == 1 and cancels[1].slot == 126 and cancels[1].reqId == reply.reqId and Composer.Reply() == nil,
        "CANCEL on the reply slot (the Helper empties the clipboard if it still holds its text)")
    check(out and out.conv == convT and out.o.text == TRANSLATION and out.o.draft == DRAFT and out.o.label == "2. Trade"
        and out.o.chatType == "CHANNEL" and out.o.target == 2 and out.states[1] == "sending",
        "my bubble: sent text, my draft ('You wrote'), channel label, 'Sending...'")
    check(Box():GetText() == "" and not Box():HasFocus() and convT.replied and Composer.Height() == 88
        and not Fake.card:IsShown() and convT.channelChoice == "CHANNEL:Trade - City" and convT.pendingReply == nil,
        "box emptied, focus cleared, conversation marked replied, card gone")
    M.EchoSent()
    check(out.state == "sent", "the server's echo within 5 s: Sent")
    check(Channels.IsOwnEcho("CHAT_MSG_CHANNEL", TRANSLATION) and not Channels.IsOwnEcho("CHAT_MSG_CHANNEL", "other"),
        "IsOwnEcho tells the window's own sends from other lines")
    Wait(1.1)
end

---------------------------------------------------------------------------- echo states
section("reply: echo states")
do
    local function SendFrom(c, id, text, viaClick)
        Composer.SetConversation(c)
        Box():SetText("")
        Type(text, true)
        Fake.detect[text] = "de"
        local outs = #Fake.outs
        if viaClick then
            M.ClickButton(ButtonFor(id))
        else
            Composer.SendTo(id)
        end
        Wait(1.1)
        return Fake.outs[outs + 1]
    end
    local w = SendFrom(convW, "WHISPER", "bis gleich", true)
    M.EchoSent()
    check(w and w.state == "sent", "whisper: CHAT_MSG_WHISPER_INFORM gives Sent")
    local bn = SendFrom(convB, "BN_WHISPER", "bis morgen", true)
    M.EchoSent()
    check(bn and bn.state == "sent", "Battle.net whisper: CHAT_MSG_BN_WHISPER_INFORM gives Sent")
    local s = SendFrom(convT, "SAY", "hallo alle", true)
    Wait(4)
    check(s and s.state == "unconfirmed" and s.states[1] == "sending", "no echo within 5 s: Not confirmed")
    local o = SendFrom(convW, "WHISPER", "bist du da", true)
    M.PlayerNotFound("Neco-OtherRealm")
    check(o and o.state == "offline", "ERR_CHAT_PLAYER_NOT_FOUND_S for that name: offline")
    local blocked = SendFrom(convT, "SAY", "ohne klick", false)
    check(blocked and blocked.state == "blocked" and LastSent().blocked,
        "ADDON_ACTION_BLOCKED naming this addon: Blocked (a say sent outside a click, outdoors)")
    local other = SendFrom(convT, "SAY", "noch einmal", true)
    M.ActionBlocked("SomeOtherAddon")
    check(other and other.state == "sending", "ADDON_ACTION_BLOCKED of another addon changes nothing")
    Wait(5)
end

---------------------------------------------------------------------------- lockdown
section("reply: chat lockdown")
do
    local b = PastedReply(TRANSLATION)
    local reply = Composer.Reply()
    local n, before = #H.sent, SentCount()
    M.SetLockdown(true)
    Composer.Refresh()
    check(not b:IsEnabled() and Contains(Stepper(), L.RPL_STEP_LOCKED), "locked: buttons greyed, the manual path shown")
    M.Hover(b)
    check(Contains(table.concat(GameTooltip.lines, "\n"), L.RPL_WHY_LOCKED), "the greyed button says why")
    M.Leave()
    Composer.SendTo(b.targetId)
    local ok, why = Channels.Send(convT, "SAY", "x")
    check(SentCount() == before and #H.Since(n, "cancel") == 0 and Composer.Reply() == reply and not ok
        and why == "RPL_WHY_LOCKED", "a send is refused before any API call, and the reply is not cancelled")
    check(Box():GetText() == TRANSLATION, "the pasted text stays in the box")
    Composer.SetConversation(convW)
    Box():SetText("")
    n = TranslateDraft(convW, "wir sehen uns")
    check(#Translations(n) == 1, "Translate still works during the lockdown (my own text)")
    M.SetLockdown(false)
    Composer.Refresh()
    Composer.SetConversation(convT)
    check(ButtonFor("CHANNEL:Trade - City"):IsEnabled(), "lockdown over: the buttons come back")
end

---------------------------------------------------------------------------- Edit, cancel, Up, drafts, keys
section("reply: Edit, cancel, Up arrow, drafts and keys")
do
    local n = TranslateDraft(convT)
    local reply = Composer.Reply()
    M.ClickButton(Fake.card.EditButton)
    local c = H.Since(n, "cancel")
    check(#c == 1 and c[1].reqId == reply.reqId and Box():GetText() == DRAFT and Composer.Mode() == "DRAFT"
        and not Fake.card:IsShown() and Composer.Height() == 88, "[Edit]: CANCEL, the draft back in the box")
    n = TranslateDraft(convT)
    M.ClickButton(Fake.card.CancelButton)
    check(#H.Since(n, "cancel") == 1 and Composer.Reply() == nil and Composer.Mode() == "EMPTY", "[x]: CANCEL, EMPTY")

    TranslateDraft(convT)
    M.Key(Box(), "UP")
    check(Box():GetText() == DRAFT and Composer.Mode() == "DRAFT", "Up arrow in the empty box recalls the draft")
    Composer.CancelReply()

    Box():SetText("")
    Type("brouillon pour Brunhilde")
    Composer.SetConversation(convW)
    check(Box():GetText() == "", "each conversation keeps its own draft (the other box is empty)")
    Type("brouillon pour Neco")
    Composer.SetConversation(convT)
    check(Box():GetText() == "brouillon pour Brunhilde" and Composer.Mode() == "DRAFT", "the draft comes back")
    M.Key(Box(), "TAB")
    M.Key(Box(), "TAB", { shift = true })
    check(Fake.cycled[#Fake.cycled - 1] == 1 and Fake.cycled[#Fake.cycled] == -1, "Tab / Shift+Tab switch conversations")
    Box():SetFocus()
    M.Key(Box(), "ESCAPE")
    check(not Box():HasFocus() and Box():GetText() == "brouillon pour Brunhilde", "Escape clears the focus, keeps the text")
    Box():SetText("")
    Composer.SetConversation(convW)
    Box():SetText("")
end

---------------------------------------------------------------------------- language popup
section("reply: reply language")
do
    local convJ = NewConv("CHAT_MSG_SAY", "Taro", "ja")
    Composer.SetConversation(convJ)
    check(ui.translate:GetText() == L.RPL_TRANSLATE_UNKNOWN, "a language the translator cannot write: 'Translate to ?'")
    local n = TranslateDraft(convJ, "on se voit demain")
    check(#Translations(n) == 0 and Fake.popup and Fake.popup[1].title and #Fake.popup == #ns.Lang.TARGETS + 1,
        "Translate opens the language list first")
    for _, item in ipairs(Fake.popup) do
        if item.text == ns.Lang.NATIVE.es then
            item.onClick()
        end
    end
    local tr = Translations(n)[1]
    check(convJ.replyLang == "es" and tr and tr.target == "es" and ui.translate:GetText() == L.RPL_TRANSLATE_TO:format("ES"),
        "picking Español sets the reply language, then translates")
    Composer.CancelReply()
    M.ClickButton(ui.lang)
    check(Fake.popupOwner == ui.lang and Fake.popup[#Fake.popup].text == ns.Lang.NATIVE["zh-TW"], "the arrow opens the list")
    Composer.SetConversation(convT)
    M.ClickButton(ui.other)
    local guild
    for _, item in ipairs(Fake.popup) do
        if item.text == GUILD then
            guild = item
        end
    end
    check(guild and guild.disabled and guild.tooltip == L.RPL_WHY_NO_GUILD, "[Other]: unavailable entries greyed with a reason")
    for _, item in ipairs(Fake.popup) do
        if item.text == YELL then
            item.onClick()
        end
    end
    check(convT.channelChoice == "YELL" and Buttons()[3].targetId == "YELL" and Contains(Stepper(),
        L.RPL_STEP_CHOSEN:format(YELL)), "picking from [Other] puts it on the third button; it does not send (D1)")
    convT.channelChoice = nil
    Composer.Refresh()
end

---------------------------------------------------------------------------- combat focus
section("reply: focus at combat start")
do
    Composer.SetConversation(convT)
    Box():SetFocus()
    Wait(2)
    M.SetCombat(true)
    check(not Box():HasFocus(), "combat start clears the focus when nothing was typed for 1.5 s")
    M.SetCombat(false)
    Type("x")
    M.SetCombat(true)
    check(Box():HasFocus(), "a key typed in the last 1.5 s keeps the focus (the next letters are not abilities)")
    M.SetCombat(false)
    Box():ClearFocus()
    Box():SetText("")
end

---------------------------------------------------------------------------- settings
section("settings: validation and migration (schema 4)")
do
    local function P(t)
        ns.Purge(t)
        return t
    end
    check(P({ schema = 3, glossShow = true }).glossShow == false, "schema 3, glossary row on (maybe the old default): off once")
    check(P({ schema = 3, glossShow = false }).glossShow == false, "schema 3, glossary row off: stays off")
    check(P({ schema = 4, glossShow = true }).glossShow == true, "schema 4: the user's choice is kept")
    local new = P({})
    check(new.glossShow == false and new.schema == 4 and new.display == "window" and new.follow == "smart"
        and new.replyClipboard == "ask" and new.convText == 14 and new.convScale == 100 and new.convCombat == "keep"
        and new.convOriginal == false and new.convFocus == false and new.convOpenWhisper == false,
        "a new user: every default (glossary row off, window, smart, ask, 14, 100%, keep)")
    local t = P({ display = "tab", follow = "bogus", replyClipboard = "allow", convText = 99, convScale = 81.4,
        convCombat = "hide", convFocus = "yes", convPos = { p = "TOPLEFT", x = 1, y = -2, w = 440, h = 540, name = "Neco" },
        convTipShown = true })
    check(t.display == "window" and t.follow == "smart" and t.replyClipboard == "allow" and t.convText == 20
        and t.convScale == 81 and t.convCombat == "hide" and t.convFocus == false and t.convTipShown == true,
        "invalid values reset, numbers rounded and clamped, valid values kept")
    local keys = {}
    for k in pairs(t.convPos) do
        keys[#keys + 1] = k
    end
    table.sort(keys)
    check(table.concat(keys, ",") == "h,p,w,x,y" and t.convPos.x == 1, "the window position keeps p, x, y, w, h only")
    check(P({ convPos = { p = "NOWHERE", x = 1, y = 2, w = 3, h = 4 } }).convPos == nil
        and P({ convPos = { p = "TOP", x = "1", y = 2, w = 3, h = 4 } }).convPos == nil
        and P({ convPos = { p = "TOP", x = 1, y = 2, w = 0, h = 4 } }).convPos == nil, "a bad position is dropped")
    check(P({ convText = 0 / 0 }).convText == 14 and P({ convScale = math.huge }).convScale == 100, "NaN and inf refused")
end

section("settings: options panel (conversation window section)")
do
    local S = M.Settings()
    local function Setting(key)
        return S.settings["TranslateProbe_" .. key]
    end
    local kinds, inits = {}, {}
    for _, init in ipairs(S.initializers) do
        local name = init.setting and init.setting.variable:gsub("^TranslateProbe_", "") or init.name
        kinds[#kinds + 1] = init.kind .. ":" .. name
        inits[name] = init
    end
    check(table.concat(kinds, ",", 5) == table.concat({ "header:" .. L.RPL_OPT_SECTION, "checkbox:display",
        "slider:follow", "slider:replyClipboard", "checkbox:convOriginal", "slider:convText", "slider:convScale",
        "checkbox:convFocus", "checkbox:convOpenWhisper", "checkbox:convCombat", "button:" .. L.RPL_OPT_RESET }, ","),
        "after the bridge mode: header, display, follow, clipboard, original, text, scale, focus, whisper, combat, reset")
    check(Setting("glossShow").default == false, "the glossary row checkbox defaults to off (D11)")
    local heard = {}
    Options.OnChanged(function(key)
        heard[#heard + 1] = key
    end)
    Setting("display"):SetValue(false)
    check(db.display == "inline" and Setting("display"):GetValue() == false, "display checkbox off: inline")
    Setting("display"):SetValue(true)
    check(db.display == "window" and Setting("display").default == true, "display checkbox on: window (default)")
    local right = MinimalSliderWithSteppersMixin.Label.Right
    local follow = inits.follow
    db.follow = "smart"
    check(Setting("follow"):GetValue() == 3 and follow.options.minValue == 1 and follow.options.maxValue == 4
        and follow.options.formatters[right](2) == L.RPL_OPT_FOLLOW_ADD and Setting("follow").default == 3,
        "follow: a 4-step labelled slider, smart by default")
    Setting("follow"):SetValue(1)
    check(db.follow == "off", "follow slider at 1: off")
    Setting("follow"):SetValue(3)
    db.replyClipboard = "ask"
    check(Setting("replyClipboard"):GetValue() == 1 and inits.replyClipboard.options.formatters[right](3)
        == L.RPL_OPT_CLIP_NEVER, "clipboard: a 3-step labelled slider (ask, allow, never)")
    Setting("replyClipboard"):SetValue(2)
    check(db.replyClipboard == "allow", "clipboard slider at 2: allow")
    Setting("replyClipboard"):SetValue(9)
    check(db.replyClipboard == "allow", "an out-of-range slider value is ignored")
    Setting("convText"):SetValue(18)
    check(db.convText == 18 and inits.convText.options.minValue == 12 and inits.convText.options.maxValue == 20,
        "text size slider 12-20")
    Setting("convScale"):SetValue(120)
    check(db.convScale == 120 and inits.convScale.options.formatters[right](120) == L.RPL_PERCENT:format(120),
        "window scale slider 80-150 %")
    Setting("convCombat"):SetValue(true)
    check(db.convCombat == "hide", "combat checkbox: hide")
    Setting("convCombat"):SetValue(false)
    Setting("convOriginal"):SetValue(true)
    check(db.convOriginal == true, "original checkbox")
    Setting("convOriginal"):SetValue(false)
    db.convPos = { p = "TOPLEFT", x = 10, y = -10, w = 500, h = 600 }
    inits[L.RPL_OPT_RESET].click()
    check(db.convPos == nil, "the reset button forgets the window position")
    local got = table.concat(heard, ",")
    check(Contains(got, "display") and Contains(got, "follow") and Contains(got, "convText") and Contains(got, "convScale")
        and Contains(got, "convPos"), "Options.OnChanged hears every change (for the window)")
    Setting("convText"):SetValue(14)
    Setting("convScale"):SetValue(100)
end

section("settings: commands")
do
    local S = M.Settings()
    local notified = #S.notified
    H.slash("clip allow")
    check(db.replyClipboard == "allow" and H.Printed(L.RPL_CLIP_ALLOW_IS)
        and S.notified[#S.notified] == "TranslateProbe_replyClipboard" and #S.notified > notified, "/tpr clip allow")
    H.slash("clip never")
    check(db.replyClipboard == "never" and H.Printed(L.RPL_CLIP_NEVER_IS), "/tpr clip never")
    H.slash("clip sometimes")
    check(db.replyClipboard == "never" and H.Printed(L.RPL_HELP_CLIP), "/tpr clip with a bad value: unchanged, help")
    H.slash("clip ASK")
    check(db.replyClipboard == "ask" and H.Printed(L.RPL_CLIP_ASK_IS), "/tpr clip ask (case ignored)")
    H.slash("display inline")
    check(db.display == "inline" and H.Printed(L.RPL_DISPLAY_INLINE), "/tpr display inline")
    H.slash("display window")
    check(db.display == "window" and H.Printed(L.RPL_DISPLAY_WINDOW), "/tpr display window")
    H.slash("show on")
    check(db.glossShow == true and H.Printed(L.SHOW_ON), "/tpr show on|off keeps working (D11)")
    H.slash("show off")
    check(db.glossShow == false, "/tpr show off")
    local toggles = Fake.toggles
    H.slash("conv")
    check(Fake.toggles == toggles + 1, "/tpr conv toggles the window")
    db.convPos = { p = "CENTER", x = 0, y = 0, w = 440, h = 540 }
    H.slash("conv reset")
    check(db.convPos == nil and H.Printed(L.RPL_CONV_RESET), "/tpr conv reset")
    db.replyClipboard = "allow"
    local n = TranslateDraft(convT)
    H.slash("conv clear")
    check(#H.Since(n, "cancel") == 1 and Composer.Reply() == nil and Fake.cleared == false and H.Printed(L.RPL_CONV_CLEARED)
        and convT.draft == nil, "/tpr conv clear: the reply cancelled, conversations and drafts forgotten")
    TranslateDraft(convT)
    H.slash("clear")
    check(Composer.Reply() == nil and Fake.cleared == true, "/tpr clear also clears the conversations (after its CLEAR)")
    H.slash("help")
    check(H.Printed(L.RPL_HELP_CONV) and H.Printed(L.RPL_HELP_DISPLAY) and H.Printed(L.RPL_HELP_CLIP)
        and H.Printed(L.RPL_HELP_TEST_PASTE), "the help lists the new commands")
    H.slash("test paste")
    Composer.SetConversation(convT)
    Box():SetText("")
    Type("a|b")
    local typed = H.Printed(L.RPL_PROBE:format("OnTextChanged", L.YES, L.NO, 3, L.NO, 1, 0, 0))
        and H.Printed(L.RPL_PROBE:format("OnChar", "-", L.NO, 3, L.NO, 1, 0, 1))
    Box():SetText("")
    Paste("x||y")
    local pasted = H.Printed(L.RPL_PROBE:format("OnTextChanged", L.YES, L.YES, 4, L.NO, 0, 1, 0))
    H.slash("test paste")
    local printed = #M.Printed()
    Type("z")
    check(H.Printed(L.RPL_PROBE_ON) and typed and pasted and H.Printed(L.RPL_PROBE_OFF) and #M.Printed() == printed,
        "/tpr test paste: the reply box probe prints counts only (typed, Ctrl, bytes, pipes), then stops")
    Box():SetText("")
    Box():ClearFocus()
    H.slash("debug")
    local c, ch = Composer.Counters(), Channels.Counters()
    check(H.Printed(L.RPL_DEBUG:format(L.NO, ch.sent, ch.confirmed, ch.unconfirmed, ch.offline, ch.blocked,
        ch.refused + c.refused, c.pasted, c.warned)) and ch.sent > 0, "/tpr debug prints reply counts (numbers only)")
end

---------------------------------------------------------------------------- locales
section("reply: locales")
do
    local src = ADDON_SOURCES and ADDON_SOURCES["ReplyLocales.lua"] or ""
    local enPart, frPart = src:match("local en = (%b{})"), src:match("local frFR = (%b{})")
    local function Keys(part)
        local keys = {}
        for key in (part or ""):gmatch("\n%s*(RPL_[%w_]+)%s*=") do
            keys[key] = true
        end
        return keys
    end
    local en, frk = Keys(enPart), Keys(frPart)
    local missing, count = {}, 0
    for key in pairs(en) do
        count = count + 1
        if not frk[key] then
            missing[#missing + 1] = key
        end
    end
    check(count > 100 and #missing == 0, "every reply string has its frFR translation (" .. count .. " keys; missing: "
        .. table.concat(missing, ", ") .. ")")
    local bad = {}
    for _, file in ipairs({ "ReplyLocales.lua", "Channels.lua", "Consent.lua", "Composer.lua" }) do
        local s = ADDON_SOURCES and ADDON_SOURCES[file] or ""
        if s:find("\226\128\148", 1, true) or s:find("\226\128\166", 1, true) then
            bad[#bad + 1] = file
        end
    end
    check(#bad == 0, "no em dash and no single-character ellipsis (" .. table.concat(bad, ", ") .. ")")
    local piped = {}
    for key in pairs(en) do
        if not key:find("^RPL_HELP_") and type(L[key]) == "string" and L[key]:find("|", 1, true) then
            piped[#piped + 1] = key
        end
    end
    check(#piped == 0, "no reply string holds a pipe, except the /tpr help lines (" .. table.concat(piped, ", ") .. ")")
    check(fr == (L.RPL_CONSENT_ALLOW == "Autoriser"), "the French strings on frFR")
end

---------------------------------------------------------------------------- privacy, cleanup
section("reply: nothing saved")
do
    local found = {}
    local function Walk(v, path)
        if type(v) == "table" then
            for k, x in pairs(v) do
                Walk(k, path .. ".<key>")
                Walk(x, path .. "." .. tostring(k))
            end
        elseif type(v) == "string" then
            for _, s in ipairs({ DRAFT, TRANSLATION, "Neco", "Brunhilde", "Player-2-0001", "bis gleich" }) do
                if v:find(s, 1, true) then
                    found[#found + 1] = path
                end
            end
        end
    end
    Walk(db, "TranslateProbeDB")
    check(#found == 0, "no draft, reply, sent text, name or GUID in the saved variables (" .. table.concat(found, ", ") .. ")")
end

Composer.Reset()
Consent.Cancel()
ns.Conv, ns.Bubble, ns.Popup, ns.LangDetect, ns.ConvFrame = real.Conv, real.Bubble, real.Popup, real.LangDetect,
    real.ConvFrame
if real.host then
    Composer.Attach(real.host)
end
Composer.SetConversation(real.selected)
Composer.ApplySettings()
win:Hide()
if M.Focused() then
    M.Focused():ClearFocus()
end
M.SetChannels({})
M.SetGroup({ party = false, raid = false, instance = false, guild = false, officer = false, inInstance = false })
for k, v in pairs(saved) do
    db[k] = v
end
check(ns.Conv == real.Conv and ns.ConvFrame == real.ConvFrame and not Consent.IsShown(),
    "the window's real modules are back, no dialog or focus left behind")

---------------------------------------------------------------------------- with the real window
-- The same flow through the window of work package A: a [T] click on a whisper opens it, the
-- composer sits in its host, the reply card is part of VIEW 200, and my bubble joins the real
-- conversation (once, even after the whisper's echo).
section("reply: in the real conversation window")
local CF, RC = ns.ConvFrame, ns.Conv
if not (CF and type(CF.OpenFromLine) == "function" and RC and type(RC.FromLine) == "function") then
    H.out("skip: the conversation window is not built yet")
else
    local keep = { display = db.display, replyClipboard = db.replyClipboard, follow = db.follow, convFocus = db.convFocus }
    db.display, db.replyClipboard, db.follow, db.convFocus = "window", "allow", "smart", false
    RC.Clear()
    local line = "Hallo, kommst du mit in die Todesminen? Wir brauchen noch einen Heiler."
    local id = H.Deliver("CHAT_MSG_WHISPER", line, "Gerda-Pewpewp", "Player-1-0900")
    M.Tick(1)
    M.Click(H.ButtonOf(id), ChatFrame1)
    local c = RC.Selected()
    local frame = CF.Frame and CF.Frame()
    check(frame and frame:IsShown() and c and Composer.Host() == CF.composerHost and Composer.Conversation() == c,
        "[T] on a whisper: the window opens, the composer works for its conversation")
    local first = Buttons()[1]
    check(first and first.targetId == "WHISPER" and first:IsHighlightLocked(), "the default button: Whisper")
    if c and not RC.ReplyLang(c) then
        RC.SetReplyLang(c, "de")
    end
    local lang = RC.ReplyLang(c)
    local function View200(from)
        local views = H.Since(from, "view")
        for k = #views, 1, -1 do
            for _, v in ipairs(views[k].views) do
                if v.id == 200 then
                    return v
                end
            end
        end
    end
    local v0 = View200(0)
    local draft = "oui je viens, je suis soigneur niveau 16"
    H.Private(draft)
    Focus()
    M.Type(Box(), draft)
    local n = #H.sent
    M.Key(Box(), "ENTER")
    local tr = Translations(n)[1]
    local v1 = View200(n)
    check(tr and tr.slot == 126 and tr.flags == 14 and tr.target == lang and Composer.ReplyCardRect() ~= nil
        and CF.composerHost:GetHeight() > 88, "Translate: TRANSLATE on 126, the real card shown, the host grows")
    local _, cardBottom = Composer.ReplyCardRect()
    local _, physH = GetPhysicalScreenSize()
    local cardPx = cardBottom and (physH - cardBottom * physH / 768)
    check(v0 and v1 and cardPx and math.abs(v1.y + v1.h - cardPx) <= 1 and v1.y == v0.y,
        "VIEW 200 reaches down to the reply card's fill (same top)")
    Wait(1.5)
    local translated = "ja ich komme mit, ich bin Heiler Stufe 16"
    Paste(translated)
    local before, outsBefore = SentCount(), 0
    for _, m in ipairs(c.msgs) do
        outsBefore = outsBefore + (m.kind == "out" and 1 or 0)
    end
    n = #H.sent
    M.ClickButton(ButtonFor("WHISPER"))
    local rec = LastSent()
    check(SentCount() == before + 1 and rec.chatType == "WHISPER" and rec.target == "Gerda-Pewpewp"
        and rec.text == translated and #H.Since(n, "cancel") == 1, "the whisper goes out, the reply is cancelled")
    M.EchoSent()
    M.Tick(2)
    local outs, last = 0, nil
    for _, m in ipairs(c.msgs) do
        if m.kind == "out" then
            outs, last = outs + 1, m
        end
    end
    check(outs == outsBefore + 1 and last and last.state == "sent" and last.draft == draft and c.replied,
        "one bubble of mine in the conversation, Sent after the echo, the conversation marked replied")
    -- review finding R4: the consent dialog closes with its window, nothing decided or sent
    db.replyClipboard = "ask"
    Focus()
    M.Type(Box(), draft)
    n = #H.sent
    M.Key(Box(), "ENTER")
    local askedShown = Consent.IsShown()
    CF.Hide()
    check(askedShown and not Consent.IsShown() and #Translations(n) == 0,
        "hiding the window closes its consent dialog without a decision (no TRANSLATE)")
    CF.Show()
    db.replyClipboard = "allow"
    Composer.Reset()
    RC.Clear()
    if frame then
        frame:Hide()
    end
    if M.Focused() then
        M.Focused():ClearFocus()
    end
    for k, v in pairs(keep) do
        db[k] = v
    end
end
