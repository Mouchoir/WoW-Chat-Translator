-- Conversation window checks (work package A): [T] routing, tabs, follow, slots, rows, VIEW 200,
-- metrics, the language detector and the inline glossary row colour. Run by tests\harness.lua with
-- the shared context H (see the top of that file); every check goes through H.check. Leaves no
-- window, conversation, focus or setting change behind.
local H = ...
local M, ns, L, db, check, section = H.M, H.ns, H.L, H.db, H.check, H.section
local Conv, ConvFrame, Reserve, Request, Bubble, ChatButton = ns.Conv, ns.ConvFrame, ns.Reserve, ns.Request, ns.Bubble,
    ns.ChatButton

local saved = { display = db.display, follow = db.follow, glossShow = db.glossShow, convOriginal = db.convOriginal,
    convText = db.convText, convScale = db.convScale, convFocus = db.convFocus, convPos = db.convPos,
    convOpenWhisper = db.convOpenWhisper, convCombat = db.convCombat, target = db.target }
db.display, db.follow, db.convFocus, db.convPos = "window", "smart", false, nil

local TRADE = { channelString = "2. Trade - City", channelIndex = 2, channelBase = "Trade - City", zoneChannelID = 2 }
local guidSeq = 100
local function NewGuid()
    guidSeq = guidSeq + 1
    return "Player-1-0" .. guidSeq
end
-- A readable line from a sender (decorated and tracked after one frame).
local function Line(event, text, sender, guid, opts)
    local id = H.Deliver(event, text, sender, guid, false, opts)
    M.Tick(1)
    return id
end
local function Click(lineID, shift, ctrl)
    M.Click(H.ButtonOf(lineID), ChatFrame1, shift, ctrl)
end
local function Find(list, pred)
    for i, v in ipairs(list) do
        if pred(v) then
            return v, i
        end
    end
end
local function Win()
    return ConvFrame.Frame()
end
local function Wd()
    return (ConvFrame.Widgets())
end
local function Tabs()
    local _, tabs = ConvFrame.Widgets()
    return tabs
end
local function ShownTabs()
    local out = {}
    for _, tab in ipairs(Tabs() or {}) do
        if tab:IsShown() then
            out[#out + 1] = tab
        end
    end
    return out
end
local function Reset()
    Conv.Clear()
    if Win() then
        Win():Hide()
    end
    ns.Popup.Close()
    M.Tick(130)     -- the freed window slots leave their 2 s quarantine
end

---------------------------------------------------------------------------------------------
section("conversation window: [T] opens it")
do
    local n0 = #H.sent
    local guid = NewGuid()
    local id = Line("CHAT_MSG_CHANNEL", "Jemand für den Flammenschlund? Brauche noch einen Heiler, bin Krieger Stufe 15.",
        "Neco-Pewpewp", guid, TRADE)
    local focusBefore = #M.FocusLog()
    Click(id)
    local win = Win()
    check(win and win:IsShown(), "[T] shows the conversation window")
    local conv = Conv.Selected()
    check(conv and conv.key == guid and conv.name == "Neco-Pewpewp" and #conv.msgs == 1, "the sender's tab is selected, one message")
    local msg = conv and conv.msgs[1]
    local translates = H.Since(n0, "translate")
    local tr = translates[#translates]
    check(#translates == 1 and tr.slot >= 100 and tr.slot <= 125 and tr.flags == 2 and tr.rows == msg.K
        and tr.target == db.target, "one TRANSLATE on a window slot (100..125) with the plate, K rows")
    local views = H.Since(n0, "view")
    local view = views[#views]
    check(view and view.views[1].id == 200 and #view.views <= 4, "VIEW lists the window first (id 200)")
    local before = H.Since(n0)
    local vi, ti
    for i, m in ipairs(before) do
        if m.kind == "view" and not vi then
            vi = i
        elseif m.kind == "translate" then
            ti = i
        end
    end
    check(vi and ti and vi < ti, "the VIEW goes out before the TRANSLATE")
    check(#M.FocusLog() == focusBefore and M.Focused() == nil, "no keyboard focus taken")
    check(Request.Count() == 0 and not H.EntryFor(ChatFrame1, id).message:find(H.SUFFIX_START, 1, true),
        "nothing reserved under the chat line")
    check(win:GetFrameStrata() == "HIGH" and win:IsToplevel() and win:IsClampedToScreen() and win:GetDontSavePosition(),
        "strata HIGH, toplevel, clamped, position saved by the addon")
    check(tContains(UISpecialFrames, "TranslateProbeConvFrame"), "Escape closes it (UISpecialFrames)")
    local b = ConvFrame.BubbleOf(msg)
    check(b and b:ShownRows() == msg.K, "the bubble shows its K rows")
    local rgb = ns.Bridge.BUBBLE_RGB
    check(math.abs(b.fill.r * 255 - rgb[1]) < 1e-6 and math.abs(b.fill.g * 255 - rgb[2]) < 1e-6 and b.fill.a == 1,
        "the bubble's fill is the opaque BUBBLE_RGB")
    check(win:GetTitleText():GetText() == "Neco-Pewpewp", "title: the full name")

    section("conversation window: duplicate line, Shift and Ctrl")
    M.Tick(70)
    n0 = #H.sent
    Click(id)
    check(#conv.msgs == 1 and #H.Since(n0, "translate") == 0, "the same line again: not added twice, nothing sent")
    check(b.flash:IsShown(), "its meta line is highlighted")
    M.Tick(100)
    check(not b.flash:IsShown(), "the highlight goes after 1.5 s")
    n0 = #H.sent
    Click(id, true)
    local again = H.Since(n0, "translate")
    check(#again == 1 and again[1].flags == 3 and again[1].slot == tr.slot and again[1].reqId ~= tr.reqId,
        "Shift-click: noCache, same slot, new request")
    local other = Line("CHAT_MSG_SAY", "wer hat lust auf strat", "Hans-Pewpewp", NewGuid())
    n0 = #H.sent
    Click(other, false, true)
    check(Request.Count() == 1 and H.EntryFor(ChatFrame1, other).message:find(H.SUFFIX_START, 1, true)
        and Conv.Count() == 1, "Ctrl-click: the inline rows, no conversation")
    db.display = "inline"
    local third = Line("CHAT_MSG_SAY", "kann mir jemand helfen", "Gerd-Pewpewp", NewGuid())
    Click(third, false, true)
    check(Conv.Count() == 2, "display inline: Ctrl-click opens the window")
    db.display = "window"
    H.slash("clear")
    check(Request.Count() == 0 and Conv.Count() == 0 and not win:IsShown(), "/tpr clear also clears the conversations")
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: tabs")
do
    local names = { "Aelin-Pewpewp", "Brunhilde-Pewpewp", "Sylvanor-Pewpewp", "Doran-Pewpewp", "Eowa-Pewpewp" }
    local keys, ids = {}, {}
    for i, name in ipairs(names) do
        keys[i] = NewGuid()
        ids[i] = Line("CHAT_MSG_CHANNEL", "selling linen cloth number " .. i, name, keys[i], TRADE)
        Click(ids[i])
    end
    local list = Conv.List()
    -- new tabs open on the right; one selected beyond the strip takes the rightmost shown place
    check(#list == 5 and list[1].key == keys[1] and list[2].key == keys[2] and list[3].key == keys[5],
        "tabs in opening order, the selected new one in the rightmost shown place")
    local W = Wd()
    local shown = ShownTabs()
    check(#shown == 3 and W.overflow:IsShown() and W.overflow:GetText() == "+2",
        "440 wide: 3 tabs fit, the other 2 behind +2")
    check(Conv.Selected().key == keys[5] and shown[3].conv.key == keys[5], "the selected one is always shown (rightmost)")
    local before = {}
    for i, c in ipairs(Conv.List()) do
        before[i] = c.key
    end
    -- a followed message never moves a tab
    db.follow = "translate"
    Line("CHAT_MSG_CHANNEL", "still selling linen cloth", names[1], keys[1], TRADE)
    local same = true
    for i, c in ipairs(Conv.List()) do
        same = same and before[i] == c.key
    end
    check(same and Conv.Get(keys[1]).unread == 1, "a new message marks the tab unread and moves nothing")
    -- the unread label and the static highlight
    local tab1 = Find(ShownTabs(), function(t)
        return t.conv and t.conv.key == keys[1]
    end)
    check(tab1 and tab1:GetText():find("(1)", 1, true) and tab1:IsHighlightLocked(), "unread: ' (N)' and a locked highlight")
    -- overflow: picking a hidden conversation swaps it with the rightmost shown tab
    M.ClickButton(W.overflow)
    local rows = ns.Popup.Rows()
    check(#rows == 2, "+2 lists the 2 hidden conversations")
    if rows[1] then
        M.ClickButton(rows[1].button)
    end
    local sel = Conv.Selected()
    shown = ShownTabs()
    check(sel and sel.key ~= keys[5] and shown[#shown].conv == sel and not ns.Popup.IsOpen(),
        "a pick from +N is selected in place of the rightmost shown tab")
    -- wider window: more tabs fit, at most 10
    Win():SetSize(760, 600)
    ConvFrame.Relayout()
    check(#ShownTabs() == 5 and not W.overflow:IsShown(), "760 wide: all 5 tabs fit")
    Win():SetSize(440, 540)
    ConvFrame.Relayout()

    section("conversation window: closing tabs keeps the history")
    local selTab = Find(ShownTabs(), function(t)
        return t.selected
    end)
    check(selTab and selTab.Close:IsShown() and not selTab:IsEnabled(), "the selected tab shows its close cross (and is disabled)")
    local otherTab = Find(ShownTabs(), function(t)
        return not t.selected
    end)
    check(otherTab and not otherTab.Close:IsShown(), "other tabs hide it")
    M.Hover(otherTab)
    check(otherTab.Close:IsShown() and GameTooltip:IsOwned(otherTab), "hovering a tab shows its cross and its tooltip")
    check(GameTooltip.lines[#GameTooltip.lines] == L.CONV_TAB_HINT, "the tab tooltip ends with how to close it")
    M.Hover(otherTab.Close)
    M.Tick(1)
    check(otherTab.Close:IsShown(), "moving onto the cross keeps it")
    M.Leave()
    M.Tick(1)
    check(not otherTab.Close:IsShown(), "leaving hides it again")
    -- middle-click on the selected (disabled) tab
    local closedKey = Conv.Selected().key
    local closedConv = Conv.Selected()
    local histBefore = #closedConv.msgs
    local slotsBefore = Reserve.WindowSlotsInUse()
    M.ClickButton(selTab, "MiddleButton")
    check(not Conv.Get(closedKey).open and #Conv.List() == 4 and Conv.Get(closedKey) == closedConv,
        "middle-click closes the selected tab; the conversation stays in memory")
    check(Conv.Selected() and Conv.Selected().key ~= closedKey, "the selection moves to a neighbour")
    check(Reserve.WindowSlotsInUse() < slotsBefore, "the closed tab's bubbles give their slots back")
    -- the cross of another tab
    local victim = Find(ShownTabs(), function(t)
        return not t.selected
    end)
    local victimKey = victim.conv.key
    M.Hover(victim)
    M.ClickButton(victim.Close)
    M.Leave()
    check(not Conv.Get(victimKey).open and #Conv.List() == 3, "the cross closes its tab")
    -- right-click popup on the selected tab (read on mouse up: a selected tab gets no OnClick)
    selTab = Find(ShownTabs(), function(t)
        return t.selected
    end)
    M.MouseUp(selTab, "RightButton")
    rows = ns.Popup.Rows()
    check(#rows == 3 and rows[1].text == L.CONV_TAB_CLOSE and rows[2].text == L.CONV_TAB_CLOSE_OTHERS
        and rows[3].text == L.CONV_FOLLOW, "right-click: Close, Close the others, Translate new messages")
    check(not tContains(UISpecialFrames, "TranslateProbeConvFrame") and tContains(UISpecialFrames, "TranslateProbePopup"),
        "while the popup shows, Escape closes the popup only")
    M.GlobalMouseDown("LeftButton")
    M.Tick(1)
    check(not ns.Popup.IsOpen() and tContains(UISpecialFrames, "TranslateProbeConvFrame"),
        "a click elsewhere closes the popup; the window is back in the Escape list")
    -- reopen with the whole history: a new translated line from the closed sender
    db.follow = "smart"
    local back = Line("CHAT_MSG_WHISPER", "bist du noch da?", closedConv.name, closedKey)
    check(closedConv.open and #closedConv.msgs == histBefore + 1 and Conv.Selected() ~= closedConv,
        "a followed whisper reopens the closed tab with its whole history (not selected)")
    check(Conv.List()[#Conv.List()] == closedConv and closedConv.unread == 1, "it comes back on the right, unread")
    Click(back)
    check(Conv.Selected() == closedConv and #closedConv.msgs == histBefore + 1, "[T] on it selects it, nothing doubled")
    -- add mode while closed: kept in the history, the tab stays closed
    M.ClickButton(Find(ShownTabs(), function(t)
        return t.selected
    end), "MiddleButton")
    db.follow = "add"
    Line("CHAT_MSG_CHANNEL", "anyone selling linen?", closedConv.name, closedKey, TRADE)
    check(not closedConv.open and #closedConv.msgs == histBefore + 2, "a followed line in add mode joins the history, the tab stays closed")
    db.follow = "smart"
    -- Close the others, then the last tab hides the window
    local keep = Conv.Selected()
    Conv.CloseOthers(keep.key)
    check(#Conv.List() == 1 and Conv.List()[1] == keep, "Close the others")
    M.ClickButton(Find(ShownTabs(), function(t)
        return t.selected
    end), "MiddleButton")
    check(not Win():IsShown() and #Conv.List() == 0 and Conv.Count() >= 5, "closing the last tab hides the window; conversations stay")
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: memory bounds (30 conversations, 100 messages)")
do
    check(Conv.MAX_CONVS == 30 and Conv.MAX_MSGS == 100, "bounds: 30 conversations, 100 messages each")
    local keys = {}
    for i = 1, 30 do
        keys[i] = NewGuid()
        Click(Line("CHAT_MSG_SAY", "hello number " .. i, "Bulk" .. i .. "-Pewpewp", keys[i]))
    end
    check(Conv.Count() == 30, "30 conversations kept")
    -- close two; the least recently active closed one goes first
    Conv.Close(keys[5])
    Conv.Close(keys[3])
    local n31 = NewGuid()
    Click(Line("CHAT_MSG_SAY", "hello number 31", "Bulk31-Pewpewp", n31))
    check(Conv.Count() == 30 and Conv.Get(keys[3]) == nil and Conv.Get(keys[5]) ~= nil and Conv.Get(n31) ~= nil,
        "the 31st evicts the least recently active closed conversation")
    Click(Line("CHAT_MSG_SAY", "hello number 32", "Bulk32-Pewpewp", NewGuid()))
    check(Conv.Get(keys[5]) == nil, "then the next closed one")
    -- no closed one: the least recently active open one without unread message or pending reply
    Conv.Get(keys[1]).unread = 2
    Conv.Get(keys[2]).pendingReply = true
    Click(Line("CHAT_MSG_SAY", "hello number 33", "Bulk33-Pewpewp", NewGuid()))
    check(Conv.Get(keys[1]) and Conv.Get(keys[2]) and Conv.Get(keys[4]) == nil,
        "then an open one, never one with unread messages or a pending reply")
    -- none can go: refused
    for _, c in ipairs(Conv.All()) do
        c.unread = 1
    end
    local printed = H.CountPrinted(L.CONV_FULL)
    Click(Line("CHAT_MSG_SAY", "hello number 34", "Bulk34-Pewpewp", NewGuid()))
    check(H.CountPrinted(L.CONV_FULL) == printed + 1 and Conv.Count() == 30, "when none can go, the new one is refused")
    Conv.Get(keys[2]).pendingReply = nil
    -- 100 messages each: the oldest goes (and its slot)
    local conv = Conv.Get(keys[6])
    db.follow = "translate"
    conv.unread = 0
    for i = 1, 105 do
        H.Deliver("CHAT_MSG_SAY", "line " .. i .. " of a long talk", conv.name, conv.key)
    end
    M.Tick(1)
    check(#conv.msgs == 100 and conv.msgs[100].text == "line 105 of a long talk", "100 messages kept, the oldest dropped")
    db.follow = "smart"
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: following new messages")
do
    -- mode x first contact (public or whisper) x replied -> what the next public line becomes
    local expect = {
        off = { public = nil, whisper = nil, replied = nil },
        add = { public = "plain", whisper = "plain", replied = "plain" },
        smart = { public = "plain", whisper = "translate", replied = "translate" },
        translate = { public = "translate", whisper = "translate", replied = "translate" },
    }
    local bad = {}
    for _, mode in ipairs({ "off", "add", "smart", "translate" }) do
        for _, case in ipairs({ "public", "whisper", "replied" }) do
            db.follow = mode
            local guid = NewGuid()
            local name = "Follow" .. mode .. case .. "-Pewpewp"
            local first = case == "whisper" and Line("CHAT_MSG_WHISPER", "salut, tu as une minute ?", name, guid)
                or Line("CHAT_MSG_CHANNEL", "quelqu'un pour le donjon ?", name, guid, TRADE)
            Click(first)
            local conv = Conv.Get(guid)
            if case == "replied" then
                Conv.MarkReplied(conv)
            end
            local n = #conv.msgs
            Line("CHAT_MSG_CHANNEL", "il nous manque un tank", name, guid, TRADE)
            local got = #conv.msgs > n and conv.msgs[#conv.msgs].state or nil
            if got ~= expect[mode][case] then
                bad[#bad + 1] = mode .. "/" .. case .. ": " .. tostring(got)
            end
            Conv.Clear()
        end
    end
    check(#bad == 0, "follow matrix: 4 modes x public, whisper, replied (" .. table.concat(bad, "; ") .. ")")
    -- the tab's own choice overrides the mode, and the check box shows the effective state
    db.follow = "smart"
    local guid = NewGuid()
    Click(Line("CHAT_MSG_CHANNEL", "wts linen cloth", "Override-Pewpewp", guid, TRADE))
    local conv = Conv.Get(guid)
    local W = Wd()
    check(not W.followCheck:GetChecked(), "smart, public first contact: the box is unticked")
    M.ClickButton(W.followCheck)
    check(conv.follow == true and Conv.EffectiveFollow(conv) == "translate", "ticking it translates this tab's new messages")
    local n0 = #H.sent
    Line("CHAT_MSG_CHANNEL", "still selling linen", "Override-Pewpewp", guid, TRADE)
    check(conv.msgs[#conv.msgs].state == "translate" and #H.Since(n0, "translate") == 1,
        "a followed line in view is translated at once")
    -- plain (add): the original in the bubble and a Translate button; one click translates it
    M.ClickButton(W.followCheck)
    check(conv.follow == false and Conv.EffectiveFollow(conv) == "add", "unticking: added untranslated")
    Line("CHAT_MSG_CHANNEL", "last call for linen", "Override-Pewpewp", guid, TRADE)
    local plain = conv.msgs[#conv.msgs]
    local b = ConvFrame.BubbleOf(plain)
    check(plain.state == "plain" and b and b.plain:IsShown() and b.translateBtn:IsShown() and b:ShownRows() == 0,
        "a plain bubble: the original drawn by the addon, a Translate button, no rows")
    n0 = #H.sent
    M.ClickButton(b.translateBtn)
    check(plain.state == "translate" and #H.Since(n0, "translate") == 1 and ConvFrame.BubbleOf(plain):ShownRows() == plain.K,
        "Translate: TRANSLATE sent, rows shown")
    -- nothing is asked while the window is hidden
    conv.follow = true
    Win():Hide()
    n0 = #H.sent
    Line("CHAT_MSG_CHANNEL", "hidden window line", "Override-Pewpewp", guid, TRADE)
    check(#H.Since(n0, "translate") == 0 and conv.unread == 1, "window hidden: nothing sent, the tab marked unread")
    ConvFrame.Toggle()
    check(Win():IsShown() and conv.unread == 0 and #H.Since(n0, "translate") == 1, "shown again: read, translated")
    -- scrolled up: new messages wait behind a button
    for i = 1, 8 do
        H.Deliver("CHAT_MSG_CHANNEL", "filler line " .. i .. " to make the thread scroll a little", "Override-Pewpewp", guid,
            false, TRADE)
    end
    M.Tick(1)
    local scroll = Wd().scroll
    scroll:SetVerticalScroll(0)
    Line("CHAT_MSG_CHANNEL", "one more while you read", "Override-Pewpewp", guid, TRADE)
    check(Wd().newPill:IsShown() and Wd().newPill:GetText() == L.CONV_NEW_ONE and scroll:GetVerticalScroll() == 0,
        "scrolled up: the view stays, '1 new message' shows")
    M.ClickButton(Wd().newPill)
    check(not Wd().newPill:IsShown() and scroll:GetVerticalScroll() >= scroll:GetVerticalScrollRange() - 1,
        "a click scrolls to the end")
    -- convOpenWhisper: a followed whisper shows the hidden window, never in combat, never with focus
    Win():Hide()
    db.convOpenWhisper = true
    M.SetCombat(true)
    Line("CHAT_MSG_WHISPER", "bist du da?", "Override-Pewpewp", guid)
    check(not Win():IsShown(), "convOpenWhisper: not in combat")
    M.SetCombat(false)
    Line("CHAT_MSG_WHISPER", "hallo?", "Override-Pewpewp", guid)
    check(Win():IsShown() and M.Focused() == nil, "convOpenWhisper: a followed whisper shows the window, without focus")
    db.convOpenWhisper = saved.convOpenWhisper
    -- my whisper typed in the normal chat becomes my bubble
    Conv.Clear()
    local wguid = NewGuid()
    Click(Line("CHAT_MSG_WHISPER", "hast du kurz Zeit?", "Whisperer-Pewpewp", wguid))
    H.Deliver("CHAT_MSG_WHISPER_INFORM", "ja, was gibt es?", "Whisperer-Pewpewp", wguid)
    M.Tick(2)
    local wconv = Conv.Get(wguid)
    local mine = wconv.msgs[#wconv.msgs]
    check(mine.kind == "out" and mine.state == "typed" and mine.text == "ja, was gibt es?" and ConvFrame.BubbleOf(mine),
        "a whisper I typed in the chat to this sender: my bubble")
    local out = Conv.AddOut(wconv, { text = "bis gleich", draft = "à tout de suite", chatType = "WHISPER", state = "sending" })
    H.Deliver("CHAT_MSG_WHISPER_INFORM", "bis gleich", "Whisperer-Pewpewp", wguid)
    M.Tick(2)
    check(wconv.msgs[#wconv.msgs] == out, "the echo of a reply sent from the window is not added twice")
    local ob = ConvFrame.BubbleOf(out)
    check(ob and ob.metaRight:GetText() == L.CONV_OUT_SENDING and ob.wrote:IsShown()
        and ob.wrote:GetText() == L.CONV_YOU_WROTE:format("à tout de suite"), "my bubble: state and 'You wrote'")
    Conv.SetOutState(out, "offline")
    ob = ConvFrame.BubbleOf(out)
    check(ob.metaRight:GetText() == L.CONV_OUT_OFFLINE:format("Whisperer"), "states: not delivered, with the short name")
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: slots")
do
    db.follow = "translate"
    local guid = NewGuid()
    Click(Line("CHAT_MSG_SAY", "erste Nachricht", "Slotty-Pewpewp", guid))
    local conv = Conv.Get(guid)
    local slot = conv.msgs[1].req.slot
    check(Reserve.IsWindowSlot(slot) and Reserve.SlotOwner(slot).kind == "win" and Reserve.SlotOwner(slot).msg == conv.msgs[1],
        "a window slot belongs to { kind = 'win', conv, msg }")
    check(Reserve.Scan() == 0 and conv.msgs[1].req and conv.msgs[1].req.slot == slot, "the chat scan never frees a window slot")
    local expired = Request.Counters().expired
    Request.ExpireOldest()
    check(Request.Counters().expired == expired and conv.msgs[1].req ~= nil, "ExpireOldest never takes a window slot")
    -- quarantine: a freed window slot is not reused for 2 s
    local n0 = #H.sent
    Conv.RemoveMsg(conv, conv.msgs[1])
    local cancels = H.Since(n0, "cancel")
    check(#cancels == 1 and cancels[1].slot == slot, "removing a bubble sends its CANCEL")
    local id2 = Line("CHAT_MSG_SAY", "zweite Nachricht", "Slotty-Pewpewp", guid)
    local m2 = conv.lines[id2]
    check(m2.req and m2.req.slot ~= slot, "within 2 s the freed slot is not given again")
    M.Tick(130)
    local freed = {}
    for _ = 1, 30 do
        local s = Reserve.AllocWindowSlot({ kind = "win" })
        if s then
            freed[#freed + 1] = s
        end
    end
    check(tContains(freed, slot), "after the quarantine it is free again")
    for _, s in ipairs(freed) do
        Reserve.FreeWindowSlot(s)
    end
    M.Tick(130)
    -- hidden tabs keep their slots until the budget needs them; the least recently seen goes first
    local hiddenGuid = NewGuid()
    Click(Line("CHAT_MSG_SAY", "ich warte hier", "Hidden-Pewpewp", hiddenGuid))
    local hiddenMsg = Conv.Get(hiddenGuid).msgs[1]
    local hiddenSlot = hiddenMsg.req.slot
    Conv.Select(guid)
    check(hiddenMsg.req and hiddenMsg.req.slot == hiddenSlot, "a hidden tab's bubble keeps its slot")
    -- fill the selected conversation until every window slot is wanted in view
    Win():SetSize(760, 900)
    db.convText = 12
    ConvFrame.Relayout()
    n0 = #H.sent
    for i = 1, 40 do
        H.Deliver("CHAT_MSG_SAY", "kurz " .. i, "Slotty-Pewpewp", guid)
    end
    M.Tick(1)
    check(hiddenMsg.req == nil, "the budget takes the hidden tab's slot back (least recently seen)")
    local gaveBack = false
    for _, c in ipairs(H.Since(n0, "cancel")) do
        gaveBack = gaveBack or c.slot == hiddenSlot
    end
    check(gaveBack, "with a CANCEL")
    check(Reserve.WindowSlotsInUse() <= 26, "never more than the 26 window slots")
    M.Tick(130)
    Win():SetSize(440, 540)
    db.convText = saved.convText
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: per-row visibility")
do
    db.follow = "translate"
    local guid = NewGuid()
    local long = "Wir suchen noch einen Heiler und zwei Schadensverursacher für die Schwarzfelsspitze, Treffpunkt in "
        .. "zehn Minuten am Eingang, bitte mit Erfahrung und Geduld, danke schön"
    Click(Line("CHAT_MSG_SAY", long, "Clip-Pewpewp", guid))
    local conv = Conv.Get(guid)
    for i = 1, 12 do
        H.Deliver("CHAT_MSG_SAY", long .. " " .. i, "Clip-Pewpewp", guid)
    end
    M.Tick(1)
    local W = Wd()
    local scroll = W.scroll
    local _, sb, _, sh = scroll:GetScaledRect()
    local top, bottom = sb + sh, sb
    local function RowState()
        local partial, wrongShown, wrongHidden = 0, 0, 0
        for _, msg in ipairs(conv.msgs) do
            local b = ConvFrame.BubbleOf(msg)
            if b and msg.req then
                for r = 1, msg.K do
                    local row = b.rowsList[r]
                    local _, pb, _, ph = row.plate:GetScaledRect()
                    local inside = pb + ph <= top + 0.01 and pb >= bottom - 0.01
                    local crosses = pb + ph > top + 0.01 and pb < top or pb < bottom - 0.01 and pb + ph > bottom
                    if crosses then
                        partial = partial + 1
                    end
                    if row.plate:IsShown() and not inside then
                        wrongShown = wrongShown + 1
                    elseif inside and not row.plate:IsShown() then
                        wrongHidden = wrongHidden + 1
                    end
                end
            end
        end
        return partial, wrongShown, wrongHidden
    end
    -- scroll by half a row so rows straddle both edges
    local m = ConvFrame.Metrics()
    scroll:SetVerticalScroll(math.max(0, scroll:GetVerticalScroll() - m.rowH * 2.5))
    local partial, wrongShown, wrongHidden = RowState()
    check(partial > 0 and wrongShown == 0, "a row partly outside the thread has its tag and plate hidden (" .. partial .. " cut)")
    check(wrongHidden == 0, "every row entirely inside is shown")
    M.Wheel(scroll, 1)
    partial, wrongShown, wrongHidden = RowState()
    check(wrongShown == 0 and wrongHidden == 0, "same after a wheel step")
    local okTags = true
    for _, msg in ipairs(conv.msgs) do
        local b = ConvFrame.BubbleOf(msg)
        if b and msg.req then
            local row = b.rowsList[1]
            for i = 1, 8 do
                okTags = okTags and row.tag[i]:IsShown() == row.plate:IsShown()
            end
        end
    end
    check(okTags, "a row's 8 tag blocks follow its plate")
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: VIEW 200 and the chat frames")
do
    local guid = NewGuid()
    Click(Line("CHAT_MSG_SAY", "hallo, alles gut?", "Viewer-Pewpewp", guid))
    local views = Reserve.View()
    local v = views[1]
    check(v and v.id == 200 and #views <= 4, "View(): the window first, 4 entries at most")
    local m = ConvFrame.Metrics()
    local _, physH = GetPhysicalScreenSize()
    local W = Wd()
    local sl, sb, _, sh = W.scroll:GetScaledRect()
    local k = physH / 768
    local eff = Win():GetEffectiveScale()
    check(v.x == Round((sl + (m.margin + m.border) * eff) * k) and v.w == Round(m.fillWidth * eff * k)
        and v.y == Round(physH - (sb + sh) * k), "VIEW 200: the bubble column's fill, from the thread's top")
    check(v.rowPx == m.rowPx and v.fontPx == m.fontPx, "VIEW 200: the window's rowPx and fontPx")
    -- the window over ChatFrame1 with no inline request: the chat frame is left out
    Win():ClearAllPoints()
    Win():SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 10, 90)
    views = Reserve.View()
    local hasChat = false
    for _, view in ipairs(views) do
        hasChat = hasChat or view.id == 1
    end
    check(ConvFrame.Covers(ChatFrame1) and not hasChat, "a chat frame under the window is left out while no inline request is active")
    -- an inline request brings it back
    local inline = Line("CHAT_MSG_SAY", "wer hat lust auf strat", "Inline-Pewpewp", NewGuid())
    Click(inline, false, true)
    views = Reserve.View()
    hasChat = false
    for _, view in ipairs(views) do
        hasChat = hasChat or view.id == 1
    end
    check(views[1].id == 200 and hasChat, "with an inline request the chat frame is listed again, after the window")
    H.slash("clear")
    M.Tick(130)
    -- hidden window: no entry 200
    Click(Line("CHAT_MSG_SAY", "noch eine Frage", "Viewer-Pewpewp", guid))
    Win():Hide()
    views = Reserve.View()
    check(views[1] and views[1].id ~= 200, "the window hidden: no entry 200")
    ConvFrame.ResetPosition()
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: metrics at UI scales 0.64 to 1.0")
do
    local guid = NewGuid()
    db.follow = "translate"
    Click(Line("CHAT_MSG_SAY", "Wir brauchen noch einen Heiler für die Todesminen, wer kommt mit? Wir starten gleich am Eingang",
        "Metric-Pewpewp", guid))
    local conv = Conv.Get(guid)
    local bad = {}
    for _, case in ipairs({ { 1920, 1080, 0.64 }, { 1920, 1080, 0.8 }, { 2560, 1440, 0.71 }, { 1920, 1080, 1.0 },
        { 3840, 2160, 0.64 } }) do
        M.SetScreen(case[1], case[2], case[3])
        ConvFrame.Relayout()
        local m = ConvFrame.Metrics()
        local ppu = Win():GetEffectiveScale() * case[2] / 768
        if math.abs(m.ppu - ppu) > 1e-9 or m.W * m.ppu < 5 - 1e-9 or m.rowPx ~= Round(m.H * m.ppu)
            or m.pad * m.ppu < 4 - 1e-9 then
            bad[#bad + 1] = case[3] .. ": metrics"
        end
        local msg = conv.msgs[#conv.msgs]
        local b = ConvFrame.BubbleOf(msg)
        if b and msg.req and msg.K >= 2 then
            local tops = {}
            for r = 1, msg.K do
                local _, t = b.rowsList[r].plate:GetScaledRect()
                local _, pb, _, ph = b.rowsList[r].plate:GetScaledRect()
                t = pb + ph
                tops[r] = t * case[2] / 768
            end
            for r = 2, msg.K do
                if math.abs((tops[r - 1] - tops[r]) - m.rowPx) > 1e-6 then
                    bad[#bad + 1] = case[3] .. ": pitch " .. (tops[r - 1] - tops[r])
                end
            end
            local tag = b.rowsList[1].tag[1]
            local _, _, tw = tag:GetScaledRect()
            if tw * case[2] / 768 < 5 - 1e-6 then
                bad[#bad + 1] = case[3] .. ": block width"
            end
        else
            bad[#bad + 1] = case[3] .. ": no rows"
        end
        local v = ConvFrame.ViewEntry()
        if not v or v.rowPx ~= m.rowPx then
            bad[#bad + 1] = case[3] .. ": view"
        end
    end
    check(#bad == 0, "W at least 5 px, row pitch exactly rowPx, VIEW rowPx at every scale (" .. table.concat(bad, "; ") .. ")")
    M.SetScreen(1920, 1080, 1)
    db.convScale = 150
    ConvFrame.Relayout()
    local m = ConvFrame.Metrics()
    check(Win():GetScale() == 1.5 and math.abs(m.ppu - 1.5 * 1080 / 768) < 1e-9, "window scale 150%: metrics follow")
    db.convScale = saved.convScale
    db.convText = 20
    ConvFrame.Relayout()
    m = ConvFrame.Metrics()
    check(m.H == 22 and ns.Bubble.Font():GetFont() and select(2, ns.Bubble.Font():GetFont()) == 20, "text size 20: rows 22, font 20")
    db.convText = saved.convText
    ConvFrame.Relayout()
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: header, bubbles and popups")
do
    local guid = NewGuid()
    Click(Line("CHAT_MSG_CHANNEL", "Suche Gruppe für die Elite-Quest, wer hat Lust?", "Header-Pewpewp", guid, TRADE))
    local conv = Conv.Get(guid)
    local W = Wd()
    check(W.sourceBtn:GetText() == "DE v" and W.targetText:GetText() == ns.Lang.NAMES[db.target],
        "header: DE > the target")
    check(W.channelText:GetText():find("2. Trade", 1, true) ~= nil, "header: via the sender's channel")
    check(Conv.ReplyLang(conv) == "de", "reply language: the detected one")
    M.ClickButton(W.sourceBtn)
    local rows = ns.Popup.Rows()
    check(#rows == 1 + #ns.Lang.TARGETS and rows[1].checked and rows[1].text:find(ns.Lang.NATIVE.de, 1, true),
        "They write in: Automatic (Deutsch) ticked, then each language in its own name")
    M.ClickButton(rows[3].button)
    check(conv.srcOverride == ns.Lang.TARGETS[2] and W.sourceBtn:GetText():find(ns.Lang.NAMES[ns.Lang.TARGETS[2]], 1, true),
        "a pick sticks for this conversation")
    Conv.SetSourceLang(conv, nil)
    -- a Japanese sender: not a target language
    local jguid = NewGuid()
    Click(Line("CHAT_MSG_WHISPER", "こんにちは、一緒に行きませんか", "Nihon-Pewpewp", jguid))
    check(W.sourceBtn:GetText() == "JA v" and Conv.ReplyLang(Conv.Get(jguid)) == nil, "Japanese: JA, no reply language")
    Conv.Select(guid)
    -- the Original link, the tooltip, the right-click popup
    local msg = conv.msgs[1]
    local b = ConvFrame.BubbleOf(msg)
    check(b.originalLink:IsShown() and not b.original:IsShown(), "the original is hidden under a link (convOriginal off)")
    M.ClickButton(b.originalLink)
    b = ConvFrame.BubbleOf(msg)
    check(b.original:IsShown() and b.original:GetText() == msg.text, "Original shows it")
    M.Hover(b.box)
    check(GameTooltip:IsOwned(b.box) and GameTooltip.lines[2] == msg.text, "hover: the original in the tooltip")
    M.Leave()
    local n0 = #H.sent
    M.ClickButton(b.box)
    local tr = H.Since(n0, "translate")
    check(#tr == 1 and tr[1].rows == 6 and msg.K == 6, "a click on the bubble: translate again with 6 rows")
    for i = 2, 3 do
        n0 = #H.sent
        M.ClickButton(ConvFrame.BubbleOf(msg).box)
        tr = H.Since(n0, "translate")
        check(#tr == 1 and tr[1].rows == 6 and tr[1].flags == 2,
            "plain click " .. i .. " on the bubble asks again, from the cache ('Not received - click the bubble')")
    end
    n0 = #H.sent
    M.ClickButton(ConvFrame.BubbleOf(msg).box, "LeftButton", { shift = true })
    tr = H.Since(n0, "translate")
    check(#tr == 1 and tr[1].flags == 3, "Shift-click: without the cache")
    M.ClickButton(ConvFrame.BubbleOf(msg).box, "RightButton")
    rows = ns.Popup.Rows()
    check(#rows == 3 and rows[1].text == L.CONV_MENU_RETRANSLATE and rows[2].text == L.CONV_MENU_COPY
        and rows[3].text == L.CONV_MENU_REMOVE, "right-click: Translate again, Copy the original, Remove")
    M.ClickButton(rows[2].button)
    local copy = _G.TranslateProbeCopyFrame
    check(copy and copy:IsShown() and copy.Box:GetText() == msg.text and M.Focused() == copy.Box,
        "Copy the original: the text selected in a box (the user asked for it)")
    M.Key(copy.Box, "ESCAPE")
    check(not copy:IsShown() and M.Focused() == nil, "Escape closes it")
    -- a popup opened while another one is open: the window stays out of the Escape list
    M.ClickButton(W.sourceBtn)
    M.ClickButton(ConvFrame.BubbleOf(msg).box, "RightButton")
    M.Tick(6)
    check(ns.Popup.IsOpen(ConvFrame.BubbleOf(msg).box) and not tContains(UISpecialFrames, "TranslateProbeConvFrame"),
        "a second popup opened over the first: the first Escape still closes only the popup")
    ns.Popup.Close()
    M.Tick(2)
    check(not ns.Popup.IsOpen() and tContains(UISpecialFrames, "TranslateProbeConvFrame"),
        "the popup closed: the window is back in the Escape list once")
    -- the glossary line under a bubble follows glossShow
    db.glossShow = true
    Click(Line("CHAT_MSG_CHANNEL", "WTS linen cloth, lf2m for dm", "Header-Pewpewp", guid, TRADE))
    local gb = ConvFrame.BubbleOf(conv.msgs[#conv.msgs])
    check(gb.gloss:IsShown() and gb.gloss:GetText() == ns.Glossary.Describe("WTS linen cloth, lf2m for dm", db.target),
        "glossShow on: the glossary line under the bubble")
    db.glossShow = saved.glossShow
    -- names on: the romanized name in the title
    local names = db.names
    db.names = true
    Click(Line("CHAT_MSG_SAY", "안녕하세요 같이 하실분", "하늘노래", NewGuid()))
    check(Win():GetTitleText():GetText() == "하늘노래 (ro)", "names on: the romanized name in the title")
    db.names = names
    -- the binding toggles the window, the empty window says what to do
    TranslateProbe_ConvToggle()
    check(not Win():IsShown(), "the binding hides the window")
    TranslateProbe_ConvToggle()
    check(Win():IsShown(), "and shows it again")
    check(BINDING_HEADER_TRANSLATEPROBE == L.CONV_BINDING_HEADER and BINDING_NAME_TRANSLATEPROBE_CONV_TOGGLE == L.CONV_BINDING_TOGGLE
        and BINDING_NAME_TRANSLATEPROBE_CONV_REPLY == L.CONV_BINDING_REPLY, "key binding names")
    Conv.Clear()
    TranslateProbe_ConvToggle()
    check(Win():IsShown() and Wd().empty:IsShown() and Wd().empty:GetText() == L.CONV_EMPTY, "the empty window explains [T]")
    Win():Hide()
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: the reply card (Bubble.AcquireReply)")
do
    ConvFrame.Build()
    local host = ConvFrame.composerHost
    local card = Bubble.AcquireReply(host)
    local m = ConvFrame.Metrics()
    card:SetDraft("on part dans 5 min ?")
    card:SetRows(Reserve.REPLY_SLOT, 3, ns.sessionId % 2)
    card:SetLaneHint(L.CONV_LANE_HINT)
    card:ClearAllPoints()
    card:SetPoint("BOTTOMLEFT", host, "BOTTOMLEFT", m.x0, 100)
    ConvFrame.Show()
    local h = card:Layout(m.bubbleWidth, m)
    check(card.draftText:GetText() == "on part dans 5 min ?" and card.EditButton and card.CancelButton, "the card: draft, Edit, cancel")
    check(card:ShownRows() == 3 and math.abs(h - card:GetHeight()) < 1e-6, "3 rows shown, Layout returns the height")
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local tops = {}
    for r = 1, 3 do
        local _, pb, _, ph = card.rowsList[r].plate:GetScaledRect()
        tops[r] = (pb + ph) * k
    end
    check(math.abs((tops[1] - tops[2]) - m.rowPx) < 1e-6 and math.abs((tops[2] - tops[3]) - m.rowPx) < 1e-6,
        "card rows touch at exactly rowPx")
    local _, lb, _, lh = card.lane:GetScaledRect()
    local laneTop = (lb + lh) * k
    local row0Top = tops[1]
    check(math.abs((row0Top - laneTop) - (3 * m.rowPx + 2 * m.ppu)) < 1e-3,
        "the lane hint starts 2 units under row0.yTop + K * rowPx (clear of the lane's top)")
    local cl, cb, cw, ch = card:CardRect()
    local l, b, w, hh = card:GetScaledRect()
    local inset = m.border * card:GetEffectiveScale()
    check(math.abs(cl - (l + inset)) < 1e-6 and math.abs(cb - (b + inset)) < 1e-6 and math.abs(cw - (w - 2 * inset)) < 1e-6
        and math.abs(ch - (hh - 2 * inset)) < 1e-6, "CardRect: the clean fill inside the border")
    check(cb * k < (laneTop - m.rowPx * 1) , "the fill reaches below the lane")
    local tag = card.rowsList[1].tag[1]
    local tl = tag:GetScaledRect()
    check(math.abs((tl - l) / card:GetEffectiveScale() - m.inset) < 1e-6, "the tag starts at border + pad")
    Bubble.Release(card)
    check(not card:IsShown(), "released")
    Win():Hide()
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: position, combat")
do
    local guid = NewGuid()
    Click(Line("CHAT_MSG_SAY", "bis später", "Pos-Pewpewp", guid))
    local win = Win()
    M.Hover(win.TitleContainer)
    M.Drag(win, 40, -20)
    M.Leave()
    local pos = db.convPos
    local fields = 0
    for _ in pairs(type(pos) == "table" and pos or {}) do
        fields = fields + 1
    end
    check(type(pos) == "table" and pos.p == "TOPLEFT" and type(pos.x) == "number" and type(pos.y) == "number"
        and pos.w == 440 and pos.h == 540 and fields == 5, "a drag by the title bar saves { p, x, y, w, h } (numbers only)")
    local x0 = pos.x
    Win():Hide()
    ConvFrame.Show()
    check(math.abs(Win():GetLeft() - x0) < 1 and db.convPos.x == x0, "the window comes back where it was")
    local grip = Wd().grip
    M.MouseDown(grip)
    M.Resize(win, 2000, 300)
    M.MouseUp(grip)
    check(win:GetWidth() == 760 and win:GetHeight() == 440 and db.convPos.w == 760 and db.convPos.h == 440,
        "a resize is clamped (380 x 440 to 760 x 900) and saved")
    M.ClickButton(win.CloseButton)
    check(not win:IsShown(), "the window's X hides it")
    ConvFrame.Show()
    ConvFrame.ResetPosition()
    check(db.convPos == nil and win:GetWidth() == 440 and win:GetHeight() == 540, "Reset window position")
    db.convCombat = "hide"
    M.SetCombat(true)
    check(not win:IsShown(), "convCombat hide: hidden at combat start")
    M.SetCombat(false)
    check(win:IsShown(), "and shown again after it")
    db.convCombat = saved.convCombat
    M.SetCombat(true)
    local n = #Conv.Get(guid).msgs
    Click(Line("CHAT_MSG_SAY", "im Kampf", "Pos-Pewpewp", guid))
    check(win:IsShown() and #Conv.Get(guid).msgs == n + 1, "keep: [T] works in combat")
    M.SetCombat(false)
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: language detector")
do
    local LD = ns.LangDetect
    check(type(ns.LangData) == "table" and ns.LangData.trigrams.en and ns.LangData.trigrams.pt and ns.LangData.floor.de,
        "LangData: the trigram profiles restored from 8b7750e (en, de, fr, es, it, pt)")
    -- a small JSON reader for the shared corpus (arrays, objects, strings; no other values used)
    local function ParseJson(s)
        local i = 1
        local function Skip()
            i = s:find("[^ \t\r\n]", i) or #s + 1
        end
        local Value
        local function Str()
            local out = {}
            i = i + 1
            while true do
                local c = s:sub(i, i)
                if c == '"' then
                    i = i + 1
                    return table.concat(out)
                elseif c == "\\" then
                    local e = s:sub(i + 1, i + 1)
                    local map = { n = "\n", t = "\t", ['"'] = '"', ["\\"] = "\\", ["/"] = "/" }
                    out[#out + 1] = map[e] or e
                    i = i + 2
                else
                    out[#out + 1] = c
                    i = i + 1
                end
            end
        end
        function Value()
            Skip()
            local c = s:sub(i, i)
            if c == '"' then
                return Str()
            elseif c == "[" then
                local t = {}
                i = i + 1
                Skip()
                if s:sub(i, i) == "]" then
                    i = i + 1
                    return t
                end
                while true do
                    t[#t + 1] = Value()
                    Skip()
                    local d = s:sub(i, i)
                    i = i + 1
                    if d == "]" then
                        return t
                    end
                end
            elseif c == "{" then
                local t = {}
                i = i + 1
                while true do
                    Skip()
                    if s:sub(i, i) == "}" then
                        i = i + 1
                        return t
                    end
                    local key = Str()
                    Skip()
                    i = i + 1   -- ':'
                    t[key] = Value()
                    Skip()
                    if s:sub(i, i) == "," then
                        i = i + 1
                    end
                end
            end
            error("unexpected JSON at " .. i)
        end
        return Value()
    end
    local raw = READ_TEST_FILE and READ_TEST_FILE("vectors/lang_corpus.json")
    local ok, doc = pcall(ParseJson, raw or "")
    local lines = ok and type(doc) == "table" and doc.lines or {}
    check(#lines >= 100, "the shared corpus tests/vectors/lang_corpus.json (" .. #lines .. " lines)")
    local right, unknown, wrong, longRight, longN = 0, 0, {}, 0, 0
    for _, entry in ipairs(lines) do
        local want, text = entry[1], entry[2]
        local got = LD.Detect(text)
        local _, words = LD.Scores(text)
        if got == want then
            right = right + 1
        elseif got == nil then
            unknown = unknown + 1
        else
            wrong[#wrong + 1] = want .. "->" .. got .. ": " .. text
        end
        if words >= 4 then
            longN = longN + 1
            longRight = longRight + (got == want and 1 or 0)
        end
    end
    check(#wrong == 0, "no line detected as a wrong language (" .. table.concat(wrong, "; ") .. ")")
    check(right >= 0.9 * #lines, ("at least 90%% right (%d of %d, %d unknown)"):format(right, #lines, unknown))
    check(longRight >= 0.9 * longN, ("lines of 4 words or more: at least 90%% right, the rest unknown (%d of %d)"):format(
        longRight, longN))
    -- the unknown rule
    check(LD.Detect("xyzzy") == nil and LD.Detect("qwrt zxcv") == nil, "2 words or fewer, no known word: unknown")
    check(LD.Detect("gg") == "en" and LD.Detect("moin") == "de" and LD.Detect("valeu") == "pt",
        "a short line decided by its known words")
    local acc = LD.Merge(nil, (LD.Scores("vendo mineral de cobre barato, susurrad")))
    local _, lead = LD.Lead(acc)
    check(lead < LD.LEAD and LD.Decide(acc) == nil, "a best score that leads by less than 15%: unknown")
    check(LD.Detect("") == nil and LD.Detect("12345 !!!") == nil and LD.Detect(M.Secret()) == nil,
        "no letters, or a secret value: unknown, no error")
    -- votes over the sender's last lines
    acc = nil
    for _, text in ipairs({ "danke schön", "bin gleich wieder da", "ok", "wer kommt mit" }) do
        acc = LD.Merge(acc, (LD.Scores(text)))
    end
    check(LD.Decide(acc) == "de", "German lines and an 'ok': German")
    acc = nil
    for _, text in ipairs({ "같이 하실분", "파티 구해요", "gg" }) do
        acc = LD.Merge(acc, (LD.Scores(text)))
    end
    check(LD.Decide(acc) == "ko", "Korean lines and a 'gg': Korean")
    -- a conversation keeps its sender's last 5 lines
    db.follow = "translate"
    local guid = NewGuid()
    Click(Line("CHAT_MSG_WHISPER", "salut", "Lang-Pewpewp", guid))
    for _, text in ipairs({ "je cherche un groupe pour la quête", "merci pour le groupe", "on y va",
        "il nous manque un tank et un heal", "bonne nuit tout le monde" }) do
        Line("CHAT_MSG_WHISPER", text, "Lang-Pewpewp", guid)
    end
    local conv = Conv.Get(guid)
    check(#conv.langLines == 5 and Conv.SourceLang(conv) == "fr", "a conversation decides on its sender's last 5 lines")
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("inline mode: the glossary row's own colour")
do
    db.display, db.glossShow = "inline", true
    local id = Line("CHAT_MSG_CHANNEL", "WTS linen cloth x20, lf2m for dm", "Gloss-Pewpewp", NewGuid(), TRADE)
    Click(id)
    local e = H.EntryFor(ChatFrame1, id)
    check(e and e.message:find(ChatButton.GLOSS_START, 1, true) and ChatButton.GLOSS_START:find("|cffe6cc80", 1, true),
        "the glossary row is drawn in its own colour (pale gold e6cc80)")
    check(ChatButton.GLOSS_COLOR ~= "9fd4ff" and ChatButton.GLOSS_COLOR ~= "66bbff", "not the light blue of the rows and of [T]")
    local gloss = e.message:sub((e.message:find(ChatButton.GLOSS_START, 1, true)))
    check(gloss:find(L.SUFFIX_GLOSS, 1, true) and not gloss:find(H.SUFFIX_START, 1, true),
        "it is the last row block, after the tagged rows")
    M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. id)
    check(not H.EntryFor(ChatFrame1, id).message:find(ChatButton.GLOSS_START, 1, true), "a click on the rows removes it too")
    db.display, db.glossShow = "window", saved.glossShow
end
Reset()

---------------------------------------------------------------------------------------------
section("conversation window: nothing saved, nothing left behind")
do
    local keys = {}
    for key in pairs(db) do
        keys[#keys + 1] = key
    end
    local bad = {}
    for _, key in ipairs(keys) do
        local v = db[key]
        if type(v) == "table" and key ~= "glossary" and key ~= "convPos" then
            bad[#bad + 1] = key
        end
    end
    check(#bad == 0, "no conversation table in the saved variables (" .. table.concat(bad, ", ") .. ")")
    local c = Conv.Counters()
    check(c.kept == 0 and c.open == 0 and Reserve.WindowSlotsInUse() == 0 and not (Win() and Win():IsShown()),
        "no conversation, no window slot, no window left")
end
for _, key in ipairs({ "display", "follow", "glossShow", "convOriginal", "convText", "convScale", "convFocus", "convPos",
    "convOpenWhisper", "convCombat", "target" }) do
    db[key] = saved[key]    -- nil values too (pairs would skip them)
end
M.Tick(130)

return true
