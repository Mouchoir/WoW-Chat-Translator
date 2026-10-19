-- Links of a message (user requests L1-L3, 2026-10-19): every colour escape of the 12.x client
-- ("|cAARRGGBB", "|cn<name>:", "|r") stripped wherever the addon reads or measures text (L1); the
-- links bar under a bubble and the Original text's hyperlinks, clickable as in the chat (L2); the
-- colour of each link name (L3, the addon's half: PromptText writes 0x02 0x04 RRGGBB name 0x03, the
-- builders send the TRANSLATE without the colours and a COLOURS message right before it, addon
-- build 5); my own bubbles' links (L2). Run by tests\harness.lua with the shared context H (see the top of that
-- file). Every chat line here is synthetic (invented item names). Leaves no window, conversation,
-- setting or combat state behind.
local H = ...
local M, ns, L, db, check, section = H.M, H.ns, H.L, H.db, H.check, H.section
local Conv, ConvFrame, Bubble, G, Links, Bridge = ns.Conv, ns.ConvFrame, ns.Bubble, ns.Glossary, ns.Links, ns.Bridge

local saved = { display = db.display, follow = db.follow, glossShow = db.glossShow, convOriginal = db.convOriginal,
    convFocus = db.convFocus, convPos = db.convPos }
db.display, db.follow, db.glossShow, db.convOriginal, db.convFocus, db.convPos = "window", "smart", false, false, false, nil

-- Synthetic links as the 12.x client writes them in chat (item links with the named quality colour)
local ITEM = "|cnIQ3:|Hitem:190001::::::::80:::::::::|h[Lattice Leggings]|h|r"
local ITEM_DATA = "item:190001::::::::80:::::::::"
local EPIC = "|cnIQ4:|Hitem:190002::::::::80:::::::::|h[Sunlit Mantle]|h|r"
local SPELL = "|cff71d5ff|Hspell:133:0|h[Fireball]|h|r"
local SPELL_DATA = "spell:133:0"
local TRADE = "|cffffd000|Htrade:Player-1-0002:2259:171|h[Alchemy]|h|r"
local PIN = "|cffffff00|Hworldmap:1429:4200:6500|h[|A:Waypoint-MapPin-ChatIcon:13:13:0:0|a Map Pin Location]|h|r"
local PIN_DATA = "worldmap:1429:4200:6500"
local ACH = "|cffffff00|Hachievement:6:Player-1-0002:0:0:0:-1:0:0:0:0|h[Level 10]|h|r"
local PLAYER = "|cff00ff00|Hplayer:Mockname|h[Mockname]|h|r"
local QUALITY_HEX = { [0] = "9D9D9D", "FFFFFF", "1EFF00", "0070DD", "A335EE", "FF8000", "E6CC80", "00CCFF", "00CCFF" }

local guidSeq = 900
local function NewGuid()
    guidSeq = guidSeq + 1
    return "Player-1-0" .. guidSeq
end
local function Line(event, text, sender, guid, opts)
    local id = H.Deliver(event, text, sender, guid, false, opts)
    M.Tick(1)
    return id
end
local function Win()
    return ConvFrame.Frame()
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
local function Mark(name, rgb)
    return "\2" .. (rgb and ("\4" .. rgb) or "") .. name .. "\3"
end
local function Reset()
    Conv.Clear()
    if Win() then
        Win():Hide()
    end
    ns.Popup.Close()
    GameTooltip:Hide()
    M.SetChatEditActive(false)
    M.ClearLinkCalls()
    M.Tick(130)
end
local function Shown(list)
    local out = {}
    for _, b in ipairs(list or {}) do
        if b:IsShown() then
            out[#out + 1] = b
        end
    end
    return out
end

---------------------------------------------------------------------------------------------
section("links: every colour escape of the 12.x client is stripped (L1)")
do
    local line = "WTS " .. ITEM .. " cheap, " .. "|cnNORMAL_FONT_COLOR:gold words|r and |cnIQ4:epic text|r"
    local plain = G.PlainText(line)
    check(not plain:find("|", 1, true) and not plain:find("cnIQ", 1, true) and not plain:find("NORMAL_FONT", 1, true)
        and plain:find("gold words", 1, true) and plain:find("epic text", 1, true),
        "PlainText: |cn<name>: and |r gone, the words kept, the link gone")
    check(G.VisibleText(line) == "WTS [Lattice Leggings] cheap, gold words and epic text",
        "VisibleText: the link's name as drawn, every colour code gone")
    check(not G.HasWords(ITEM) and not G.HasWords(" " .. EPIC .. " " .. SPELL .. " "), "a line of links alone has no words")
    local allQ = true
    for q = 0, 8 do
        local s = "|cnIQ" .. q .. ":word|r"
        if G.VisibleText(s) ~= "word" or G.PlainText(s):find("IQ", 1, true) or G.PromptText(s) ~= "word" then
            allQ = false
        end
    end
    check(allQ, "|cnIQ0: to |cnIQ8: stripped by VisibleText, PlainText and PromptText")
    check(G.VisibleText("|cff00ff00green |cnIQ4:inner|r still|r") == "green inner still", "nested colours")
    check(G.VisibleText("a||cnIQ3:b") == "a|cnIQ3:b" and G.PlainText("a||b") == "a b",
        "a typed pipe (||) is not an escape: shown as |, not a word")
    local prompt = G.PromptText(line)
    check(not prompt:find("|", 1, true) and not prompt:find("cnIQ", 1, true) and not prompt:find("NORMAL_FONT", 1, true),
        "PromptText: no escape left")
    -- the glossary and the language detector read the words only
    local described = G.Describe("WTS " .. ITEM .. " lf2m", db.target) or ""
    local extracted = G.Extract("WTS " .. ITEM .. " lf2m", db.target)
    local leak = described:lower():find("cniq", 1, true) ~= nil
    for _, p in ipairs(extracted) do
        if p[1]:lower():find("iq", 1, true) then
            leak = true
        end
    end
    check(not leak and described:find("WTS", 1, true), "the glossary never sees cnIQ3 as a word")
    -- the window's measure (the mock font string counts no code, like the client)
    check(Bubble.TextWidth("|cnIQ3:abc|r") == Bubble.TextWidth("abc")
        and Bubble.TextWidth(ITEM) == Bubble.TextWidth("[Lattice Leggings]"), "a |cn code takes no width")
    check(Links.StripColors("|cffaabbcca|cnIQ3:b|cnFOO_BAR:c|r|r", "") == "abc", "Links.StripColors: all three forms")
end

---------------------------------------------------------------------------------------------
section("links: the colour of each link name in the marks (L3, addon build 5)")
do
    check(ns.ADDON_BUILD == 5, "addon build 5 (an older Translate Companion says it is older than the addon)")
    check(Bridge.MarkSpan("[x]", "0070dd") == "\2\0040070DD[x]\3" and Bridge.MarkSpan("{skull}") == "\2{skull}\3"
        and Bridge.MarkSpan("[x]", "zzzzzz") == "\2[x]\3" and Bridge.MarkSpan("[x]", "0070D") == "\2[x]\3"
        and Bridge.MarkSpan("a\2b\3c\4d", "FFD100") == "\2\4FFD100abcd\3" and Bridge.MarkSpan("") == ""
        and Bridge.MarkSpan(nil) == "", "Bridge.MarkSpan: 0x02, 0x04 RRGGBB (uppercase) when the colour is 6 hex digits, name, 0x03")
    local qok = true
    for q = 0, 8 do
        local text = G.PromptText("|cnIQ" .. q .. ":|Hitem:190001|h[Thing]|h|r")
        if text ~= Mark("[Thing]", QUALITY_HEX[q]) then
            qok = false
        end
    end
    check(qok, "an item link of quality 0..8: its quality colour from the client")
    check(G.PromptText("WTS " .. ITEM .. " / " .. SPELL) == "WTS " .. Mark("[Lattice Leggings]", "0070DD") .. " / "
        .. Mark("[Fireball]", "71D5FF"), "a rare item (|cnIQ3:) and a spell (|cff71d5ff)")
    check(G.PromptText(TRADE .. PIN) == Mark("[Alchemy]", "FFD000") .. Mark("[ Map Pin Location]", "FFFF00"),
        "a profession and a map pin (its atlas left out of the name)")
    check(G.PromptText("|cnGREEN_FONT_COLOR:|Hquest:1:10|h[Named Green]|h|r") == Mark("[Named Green]", "1AFF1A")
        and G.PromptText("|cnNORMAL_FONT_COLOR:|Hquest:1:10|h[Named Gold]|h|r") == Mark("[Named Gold]", "FFD100"),
        "a named global colour (|cnGREEN_FONT_COLOR:, |cnNORMAL_FONT_COLOR:) through the client's colour")
    check(G.PromptText("|cnNOT_A_COLOR_HERE:|Hquest:1|h[Unknown]|h|r |Hquest:2|h[Bare]|h") == Mark("[Unknown]", "FFD100")
        .. " " .. Mark("[Bare]", "FFD100"), "an unknown named colour, or none: gold (FFD100)")
    check(G.PromptText("|cff00ff00around |cnIQ4:|Hitem:2|h[Inner]|h|r out|r") == "around " .. Mark("[Inner]", "A335EE") .. " out",
        "the innermost colour at the link")
    check(G.PromptText("|Hitem:3|h|cff1eff00[Own Colour]|r|h") == Mark("[Own Colour]", "1EFF00"),
        "a colour at the start of the name wins (that is what the client draws)")
    check(G.PromptText("{skull} {rt8}") == "\2{skull}\3 \2{rt8}\3", "{...} groups carry no colour")
    check(G.PromptText("a\4FFD100b \2c\3") == "aFFD100b c", "a typed 0x04 never passes for a colour prefix")
    -- the user's colour override of a quality is the client's: the API, not a fixed table
    M.SetQualityColor(3, 1, 0, 0)
    check(G.PromptText(ITEM) == Mark("[Lattice Leggings]", "FF0000"), "a user override of the rare colour is followed")
    M.SetQualityColor(3, nil)
    local cm, co, ci = ColorManager, C_ColorOverrides, C_Item.GetItemQualityColor
    ColorManager = nil
    local viaOverrides = Links.QualityRGB(4)
    C_ColorOverrides = nil
    local viaItem = Links.QualityRGB(4)
    C_Item.GetItemQualityColor = nil
    local viaTable = Links.QualityRGB(4)
    ColorManager, C_ColorOverrides, C_Item.GetItemQualityColor = cm, co, ci
    check(viaOverrides == "A335EE" and viaItem == "A335EE" and viaTable == "A335EE",
        "quality colour fallbacks: C_ColorOverrides, C_Item.GetItemQualityColor, ITEM_QUALITY_COLORS")
    check(Links.QualityRGB(99) == nil and Links.NamedRGB("IQ99") == nil and Links.EscapeRGB("|cnIQ99:") == nil,
        "an unknown quality has no colour (the mark gets gold)")
    check(Links.Hex({ r = 0, g = 0.439, b = 0.871, hex = "|cff0070dd" }) == "0070DD" and Links.Hex(0, 0.439, 0.871) == "0070DE"
        and Links.Hex(0, 0, 0, "ffa335ee") == "A335EE" and Links.Hex(CreateColor(1, 0.82, 0)) == "FFD100",
        "the client's own colour code wins over its floats (no rounding difference)")
    -- the 1024 B cut never splits a coloured mark
    local long = string.rep("a", 1000) .. " " .. ITEM .. " tail"
    local cut = G.PromptText(long)
    check(cut == string.rep("a", 1000) .. " ", "1024 B cut before a coloured mark that does not fit")
    local fits = G.PromptText(string.rep("a", 990) .. " " .. ITEM)
    check(#fits == 991 + #Mark("[Lattice Leggings]", "0070DD") and Count(fits, "\2") == 1 and Count(fits, "\3") == 1
        and Count(fits, "\4") == 1, "a coloured mark that fits is kept whole")
end

---------------------------------------------------------------------------------------------
section("links: the colours go in COLOURS, right before a TRANSLATE without any 0x04 (window and inline)")
do
    -- the builders (docs\BRIDGE.md 3b): a Translate Companion of build 4 skips COLOURS as an
    -- unknown message, and would have painted an in-text colour code
    local marked = "a " .. Mark("[x]", "0070DD") .. " " .. Mark("{skull}") .. " " .. Mark("[y]", "71D5FF")
    local wire = H.Parse(Bridge.TranslateMessage(5, 1, 1, 2, "fr", marked, {}))
    local co = H.Parse(Bridge.ColoursMessage(5, marked))
    check(wire.ok and wire.text == "a " .. Mark("[x]") .. " " .. Mark("{skull}") .. " " .. Mark("[y]")
        and co.ok and co.reqId == 5 and co.list == "0:0070DD,2:71D5FF", "TranslateMessage drops the colours, ColoursMessage carries them by span index")
    check(Bridge.ColoursMessage(5, "a " .. Mark("{skull}")) == nil and Bridge.ColoursMessage(5, nil) == nil,
        "no coloured span: no COLOURS")
    local cutText = string.rep("a", 1020) .. Mark("[x]", "0070DD")
    check(#H.Parse(Bridge.TranslateMessage(6, 1, 1, 2, "fr", cutText, {})).text == 1020
        and Bridge.ColoursMessage(6, cutText) == nil, "a span the 1024 B cut drops has no colour sent")
    local guid = NewGuid()
    local text = "WTS " .. ITEM .. " and " .. SPELL .. " {skull}"
    local n0 = #H.sent
    M.Click(H.ButtonOf(Line("CHAT_MSG_CHANNEL", text, "Links-Trader", guid)), ChatFrame1)
    local all = H.Since(n0)
    local tr = H.Since(n0, "translate")
    local colours = H.Since(n0, "colours")
    check(#tr == 1 and H.AllOk(tr) and tr[1].text == "WTS " .. Mark("[Lattice Leggings]") .. " and "
        .. Mark("[Fireball]") .. " " .. Mark("{skull}"), "a window bubble's TRANSLATE: link marks, no colour code")
    local before
    for i, m in ipairs(all) do
        if m.kind == "translate" then
            before = all[i - 1]
        end
    end
    check(#colours == 1 and colours[1].ok and colours[1].reqId == tr[1].reqId and colours[1].list == "0:0070DD,1:71D5FF"
        and before and before.kind == "colours" and before.reqId == tr[1].reqId, "its COLOURS right before it: the rare item and the spell, not the {skull} group ("
        .. tostring(colours[1] and colours[1].list) .. ")")
    db.display = "inline"
    n0 = #H.sent
    local inline = Line("CHAT_MSG_CHANNEL", "Selling " .. EPIC, "Links-Trader", guid)
    M.Click(H.ButtonOf(inline), ChatFrame1)
    tr = H.Since(n0, "translate")
    colours = H.Since(n0, "colours")
    check(#tr == 1 and H.AllOk(tr) and tr[1].text == "Selling " .. Mark("[Sunlit Mantle]") and #colours == 1
        and colours[1].reqId == tr[1].reqId and colours[1].list == "0:A335EE", "an inline TRANSLATE: the same, epic purple")
    ns.Request.Remove(inline)
    db.display = "window"
end
Reset()

---------------------------------------------------------------------------------------------
section("links: the links bar under a bubble (L2)")
do
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local guid = NewGuid()
    local text = "WTS " .. ITEM .. " + " .. SPELL .. " " .. TRADE .. ", meet at " .. PIN .. " " .. ITEM .. " {skull} " .. ACH
    local id = Line("CHAT_MSG_CHANNEL", text, "Links-Bar", guid)
    M.Click(H.ButtonOf(id), ChatFrame1)
    local conv = Conv.Get(guid)
    local msg = conv and conv.msgs[1]
    local b = msg and ConvFrame.BubbleOf(msg)
    local m = ConvFrame.Metrics()
    local buttons = Shown(b and b.linkButtons)
    check(b and msg.req and #buttons == 5, "one button per link, the repeated item once (" .. #buttons .. ")")
    local kinds = {}
    for i, btn in ipairs(buttons) do
        kinds[i] = btn.entry and btn.entry.kind
    end
    check(table.concat(kinds, ",") == "item,spell,trade,worldmap,achievement", "in the message's order (" .. table.concat(kinds, ",") .. ")")
    local labels = {}
    for i, btn in ipairs(buttons) do
        labels[i] = btn:GetFontString():GetText()
    end
    check(labels[1] == "|cnIQ3:[Lattice Leggings]|r" and labels[2] == "|cff71d5ff[Fireball]|r"
        and labels[3] == "|cffffd000[Alchemy]|r"
        and labels[4] == "|cffffff00[|A:Waypoint-MapPin-ChatIcon:13:13:0:0|a Map Pin Location]|r"
        and labels[5] == "|cffffff00[Level 10]|r", "each name in its own colour escape, as the chat draws it")
    check(buttons[1].entry.text == ITEM and buttons[1].entry.link == ITEM_DATA and buttons[4].entry.link == PIN_DATA,
        "each button holds the whole link (colour, |H...|h...|h, |r) and its data")
    check(buttons[1]:GetFontString().fontObject == Bubble.Font(), "the window font")
    -- geometry: the rows, the tags, the plates and the lane where they are without a bar; the bar
    -- under the lane, from the tag's x0 to the plate's end, inside the fill
    local K = msg.K
    local eff = b.box:GetEffectiveScale()
    local bl, bb, _, bh = b.box:GetScaledRect()
    local boxTop = bb + bh
    local _, p1b, _, p1h = b.rowsList[1].plate:GetScaledRect()
    local tl = b.rowsList[1].tag[1]:GetScaledRect()
    local rowTop0 = b.rowTop0
    check(math.abs(rowTop0 - m.inset) < 1 and math.abs((boxTop - (p1b + p1h)) / eff - rowTop0) < 1e-6
        and math.abs((tl - bl) / eff - m.inset) < 1e-6, "row 0 and its tag at the inset, as without links")
    check(math.abs(b.laneTop - (rowTop0 + K * m.rowH)) < 1e-6, "the lane right under row K")
    local gap = Bubble.BarGap(m)
    check(gap == math.max(4, 4 / m.ppu) and gap * m.ppu >= 4 - 1e-9, "the bar's gap: 4 units, 4 px at least")
    local laneBottomPx = (boxTop - (b.laneTop + m.rowH) * eff) * k
    local barTopPx = -math.huge
    local left, right, bottom = math.huge, -math.huge, math.huge
    for _, btn in ipairs(buttons) do
        local l, bt, w, h = btn:GetScaledRect()
        barTopPx = math.max(barTopPx, (bt + h) * k)
        left, right, bottom = math.min(left, l), math.max(right, l + w), math.min(bottom, bt)
    end
    check(laneBottomPx - barTopPx >= 4 - 1e-6 and math.abs((laneBottomPx - barTopPx) / k / eff - gap) < 1e-6,
        "the bar starts its gap under the lane's bottom (the Helper's lane search, +-2 px, never reaches it)")
    local plateEnd = bl + (m.inset + 8 * m.W + m.plateWidth) * eff
    check(math.abs(left - (bl + m.inset * eff)) < 1e-6 and right <= plateEnd + 1e-6,
        "the bar runs from the tag's x0 to the plate's end")
    check(b.barH == 2 * m.rowH, "two lines: the map pin did not fit on the first (" .. tostring(b.barH and b.barH / m.rowH) .. ")")
    local _, b4, _, h4 = buttons[4]:GetScaledRect()
    local _, b1, _, h1 = buttons[1]:GetScaledRect()
    check(math.abs(((b1 + h1) - (b4 + h4)) / eff - m.rowH) < 1e-6, "lines rowH apart")
    check(math.abs(bh / eff - (2 * m.inset + (K + 1) * m.rowH + gap + b.barH)) < 1,
        "the bubble: inset, K rows, the lane, the gap, the bar, inset")
    check(bottom - bb >= (m.inset - 1) * eff, "the bar inside the fill, clear of the border art")
    -- the same bubble without its bar: every tag, plate and the lane's hint at the same place
    local function Places(x)
        local _, xb, _, xh = x.box:GetScaledRect()
        local top, out = xb + xh, {}
        for r = 1, x.K do
            local row = x.rowsList[r]
            for i = 1, 8 do
                local tl0, tb0 = row.tag[i]:GetScaledRect()
                out[#out + 1] = string.format("%.6f,%.6f", tl0, top - tb0)
            end
            local pl0, pb0, pw0 = row.plate:GetScaledRect()
            out[#out + 1] = string.format("%.6f,%.6f,%.6f", pl0, top - pb0, pw0)
        end
        local hl0, hb0 = x.lane:GetScaledRect()
        out[#out + 1] = string.format("%.6f,%.6f", hl0, top - hb0)
        return table.concat(out, ";")
    end
    local withBar = Places(b)
    msg.links = {}
    ConvFrame.Relayout()
    local bare = ConvFrame.BubbleOf(msg)
    local withoutBar = Places(bare)
    local _, _, _, bareH = bare.box:GetScaledRect()
    check(withBar == withoutBar and #Shown(bare.linkButtons) == 0,
        "tags, plates and the lane: the same places with and without the bar")
    check(math.abs((bh - bareH) / eff - (gap + 2 * m.rowH)) < 1, "the bar adds its gap and its lines, nothing else")
    msg.links = nil
    ConvFrame.Relayout()
    b = ConvFrame.BubbleOf(msg)
    -- a message without links: no bar, the bubble one row taller than its rows (as before)
    local id2 = Line("CHAT_MSG_CHANNEL", "Nothing to link in this line at all", "Links-Bar", guid)
    M.Click(H.ButtonOf(id2), ChatFrame1)
    local msg2 = conv.msgs[2]
    local b2 = ConvFrame.BubbleOf(msg2)
    local _, _, _, bh2 = b2.box:GetScaledRect()
    check(#Shown(b2.linkButtons) == 0 and b2.barH == 0 and math.abs(bh2 / b2.box:GetEffectiveScale()
        - (2 * m.inset + (msg2.K + 1) * m.rowH)) < 1, "no links: no bar, the bubble as before")
    -- the lane still shows 1.5 s after the rows, with the bar under it
    M.Tick(91)
    b = ConvFrame.BubbleOf(msg)
    check(b.lane:IsShown() and b.laneButton:IsShown() and #Shown(b.linkButtons) == 5, "the lane's hint shows, the bar stays")
    -- clicks: the chat's handling (SetItemRef), Shift inserts the link, Ctrl dresses up, nothing for the bubble
    buttons = Shown(b.linkButtons)
    M.ClearLinkCalls()
    local n0 = #H.sent
    M.ClickButton(buttons[1])
    local calls = M.LinkCalls()
    local ref = calls.refs[1]
    check(#calls.refs == 1 and ref.link == ITEM_DATA and ref.text == ITEM and ref.button == "LeftButton"
        and ref.frame == DEFAULT_CHAT_FRAME and ref.result == "itemref" and #calls.inserts == 0,
        "left click: SetItemRef(link, text, button, the chat frame), as the chat (the item's tooltip)")
    M.ClickButton(buttons[4])
    check(#calls.refs == 2 and calls.refs[2].link == PIN_DATA and calls.refs[2].result == "map",
        "a map pin: the map opens with the waypoint (SetItemRef)")
    M.ClickButton(buttons[1], "RightButton")
    check(#calls.refs == 3 and calls.refs[3].button == "RightButton", "right click: to SetItemRef too, as in the chat")
    M.SetChatEditActive(true)
    M.ClickButton(buttons[1], "LeftButton", { shift = true })
    check(#calls.inserts == 1 and calls.inserts[1].text == ITEM and calls.inserts[1].inserted and #calls.refs == 3,
        "Shift-click with the chat edit box open: the link goes into it (ChatFrameUtil.InsertLink)")
    M.ClickButton(buttons[4], "LeftButton", { shift = true })
    check(#calls.inserts == 2 and calls.inserts[2].text == buttons[4].entry.text and #calls.refs == 3,
        "a map pin too: every link type can be put in the message")
    M.SetChatEditActive(false)
    M.ClickButton(buttons[2], "LeftButton", { shift = true })
    check(#calls.refs == 4 and calls.refs[4].shift and calls.refs[4].link == SPELL_DATA and calls.refs[4].result == "none",
        "Shift-click without an edit box: on to SetItemRef, as the chat does")
    M.ClickButton(buttons[1], "LeftButton", { ctrl = true })
    check(#calls.refs == 5 and calls.refs[5].ctrl and calls.refs[5].result == "dressup" and calls.dressUps[1] == ITEM,
        "Ctrl-click on an item: the dressing room (SetItemRef's modified click)")
    check(#H.Since(n0, "translate") == 0 and not ns.Popup.IsOpen(), "link clicks never retranslate the bubble or open its menu")
    -- hover: the client's tooltip for the link, else its name and what a click does
    M.Hover(buttons[1])
    check(GameTooltip:IsShown() and GameTooltip:GetOwner() == buttons[1] and calls.tooltips[#calls.tooltips] == ITEM_DATA
        and GameTooltip.lines[1] == "hyperlink " .. ITEM_DATA, "hover an item: GameTooltip:SetHyperlink")
    M.Leave()
    check(not GameTooltip:IsShown(), "it goes with the cursor")
    local before = #calls.tooltips
    M.Hover(buttons[4])
    check(GameTooltip:IsShown() and GameTooltip:GetOwner() == buttons[4] and #calls.tooltips == before + 1
        and GameTooltip.lines[1] == buttons[4].entry.label and GameTooltip.lines[2] == L.CONV_LINK_TIP_CLICK
        and GameTooltip.lines[3] == L.CONV_LINK_TIP_SHIFT, "a map pin (no tooltip from the client): its name and the clicks")
    M.Leave()
    -- a long name is cut to the bar's width
    local long = "|cnIQ4:|Hitem:190003|h[" .. string.rep("Endless ", 12) .. "Cloak]|h|r"
    local id3 = Line("CHAT_MSG_CHANNEL", "WTS " .. long, "Links-Bar", guid)
    M.Click(H.ButtonOf(id3), ChatFrame1)
    local b3 = ConvFrame.BubbleOf(conv.msgs[3])
    local only = Shown(b3.linkButtons)
    check(#only == 1 and math.abs(only[1]:GetWidth() - (8 * m.W + m.plateWidth)) < 1e-6 and b3.barH == m.rowH,
        "a name wider than the bar: cut to its width, one line")
    -- a pooled bubble reused for a line without links shows no old button
    Conv.Clear()
    M.Tick(1)
    local guid2 = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", "No links here either, friend", "Links-Reuse", guid2)), ChatFrame1)
    local rb = ConvFrame.BubbleOf(Conv.Get(guid2).msgs[1])
    check(rb and #Shown(rb.linkButtons) == 0 and (rb.barH or 0) == 0, "a reused bubble: no stale link button")
end
Reset()

---------------------------------------------------------------------------------------------
section("links: the Original text's hyperlinks (L2)")
do
    local guid = NewGuid()
    local text = "Need " .. ITEM .. " and " .. PIN .. " " .. PLAYER
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", text, "Links-Orig", guid)), ChatFrame1)
    local msg = Conv.Get(guid).msgs[1]
    local b = ConvFrame.BubbleOf(msg)
    check(not M.ClickHyperlink(b.originalFrame, ITEM_DATA), "the original hidden: nothing to click")
    M.ClickButton(b.originalLink)
    b = ConvFrame.BubbleOf(msg)
    check(b.original:IsShown() and b.originalFrame:IsShown() and b.original:GetText() == msg.text,
        "Original shows the message with its real links")
    check(b.originalFrame:GetHyperlinksEnabled() and b.originalFrame:IsMouseEnabled(), "its hyperlinks are enabled")
    M.ClearLinkCalls()
    local calls = M.LinkCalls()
    local n0 = #H.sent
    check(M.ClickHyperlink(b.originalFrame, ITEM_DATA) and #calls.refs == 1 and calls.refs[1].link == ITEM_DATA
        and calls.refs[1].text == ITEM and calls.refs[1].frame == DEFAULT_CHAT_FRAME, "a click on its item: SetItemRef")
    check(M.ClickHyperlink(b.originalFrame, PIN_DATA) and calls.refs[2].result == "map", "its map pin opens the map")
    M.SetChatEditActive(true)
    check(M.ClickHyperlink(b.originalFrame, ITEM_DATA, "LeftButton", { shift = true }) and #calls.inserts == 1
        and calls.inserts[1].text == ITEM and #calls.refs == 2, "Shift-click: into the chat edit box")
    -- a player link's Shift-click is the client's own (the plain name, a popup or /who): never the raw link
    check(M.ClickHyperlink(b.originalFrame, "player:Mockname", "LeftButton", { shift = true }) and #calls.inserts == 2
        and calls.inserts[2].text == "Mockname" and #calls.refs == 3 and calls.refs[3].link == "player:Mockname"
        and calls.refs[3].shift and calls.refs[3].result == "name",
        "Shift-click on a player link: straight to SetItemRef, which puts the plain name in, as from the chat")
    M.SetChatEditActive(false)
    check(M.ClickHyperlink(b.originalFrame, ITEM_DATA, "LeftButton", { ctrl = true }) and calls.dressUps[1] == ITEM,
        "Ctrl-click: the dressing room")
    check(#H.Since(n0, "translate") == 0, "no retranslation")
    check(M.HoverHyperlink(b.originalFrame, ITEM_DATA) and GameTooltip:IsShown()
        and GameTooltip:GetOwner() == b.originalFrame and GameTooltip.lines[1] == "hyperlink " .. ITEM_DATA,
        "hover its item: the item's tooltip")
    M.LeaveHyperlink(b.originalFrame)
    check(not GameTooltip:IsShown(), "and it goes")
    local tips = #calls.tooltips
    check(M.HoverHyperlink(b.originalFrame, "player:Mockname") and #calls.tooltips == tips
        and GameTooltip.lines[2] == L.CONV_LINK_TIP_CLICK, "a player link: no SetHyperlink, the name and the clicks")
    M.LeaveHyperlink(b.originalFrame)
    -- made during combat: enabled once it ends, never blocked
    local blocked = 0
    local watch = CreateFrame("Frame")
    watch:RegisterEvent("ADDON_ACTION_BLOCKED")
    watch:SetScript("OnEvent", function()
        blocked = blocked + 1
    end)
    M.SetCombat(true)
    local pooled = Bubble.Counts()
    local made = {}
    for i = 1, pooled + 1 do
        made[i] = Bubble.AcquireIn(UIParent)
    end
    local fresh = made[#made]
    local inCombat = fresh.originalFrame:GetHyperlinksEnabled()
    local generic = CreateFrame("Frame")
    local genericNow = Links.EnableHyperlinks(generic)
    M.SetCombat(false)
    check(not inCombat and not genericNow and blocked == 0, "during combat: not enabled yet, nothing blocked")
    check(fresh.originalFrame:GetHyperlinksEnabled() and generic:GetHyperlinksEnabled() and blocked == 0,
        "after the combat: enabled")
    for _, x in ipairs(made) do
        Bubble.Release(x)
    end
    watch:UnregisterAllEvents()
end
Reset()

---------------------------------------------------------------------------------------------
section("links: my own bubble's links (a whisper typed in the chat)")
do
    local EPIC_DATA = "item:190002::::::::80:::::::::"
    local guid = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", "do you still have it?", "Links-Mine", guid)), ChatFrame1)
    H.Deliver("CHAT_MSG_WHISPER_INFORM", "here: " .. EPIC, "Links-Mine", guid)
    M.Tick(2)
    local conv = Conv.Get(guid)
    local mine = conv and conv.msgs[#conv.msgs]
    local ob = mine and ConvFrame.BubbleOf(mine)
    check(mine and mine.kind == "out" and ob and ob.text:GetText() == mine.text and ob.textFrame:GetHyperlinksEnabled()
        and ob.textFrame:IsMouseEnabled(), "my bubble with a link: the real link, its hyperlinks enabled")
    M.ClearLinkCalls()
    local calls = M.LinkCalls()
    check(ob and M.ClickHyperlink(ob.textFrame, EPIC_DATA) and #calls.refs == 1 and calls.refs[1].text == EPIC
        and calls.refs[1].frame == DEFAULT_CHAT_FRAME, "a click on it: SetItemRef, as in the chat")
    M.SetChatEditActive(true)
    check(ob and M.ClickHyperlink(ob.textFrame, EPIC_DATA, "LeftButton", { shift = true }) and #calls.inserts == 1
        and calls.inserts[1].text == EPIC, "Shift-click: into the chat edit box")
    M.SetChatEditActive(false)
    check(ob and M.HoverHyperlink(ob.textFrame, EPIC_DATA) and GameTooltip:IsShown()
        and GameTooltip.lines[1] == "hyperlink " .. EPIC_DATA, "hover: the item's tooltip")
    M.LeaveHyperlink(ob.textFrame)
    H.Deliver("CHAT_MSG_WHISPER_INFORM", "nothing to link this time", "Links-Mine", guid)
    M.Tick(2)
    local plainOut = ConvFrame.BubbleOf(conv.msgs[#conv.msgs])
    check(plainOut and plainOut ~= ob and not plainOut.textFrame:IsMouseEnabled(),
        "without links: the text takes no mouse (the bubble's hover as before)")
end
Reset()

---------------------------------------------------------------------------------------------
section("links: an untranslated bubble's bar (follow mode add)")
do
    db.follow = "add"
    local guid = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", "hello there", "Links-Plain", guid)), ChatFrame1)
    Line("CHAT_MSG_WHISPER", "look: " .. SPELL .. " " .. EPIC, "Links-Plain", guid)
    local conv = Conv.Get(guid)
    local msg = conv.msgs[#conv.msgs]
    local b = ConvFrame.BubbleOf(msg)
    local m = ConvFrame.Metrics()
    local buttons = Shown(b and b.linkButtons)
    check(msg.state ~= "translate" and b.plain:IsShown() and #buttons == 2, "a plain bubble with links: its bar (2)")
    local eff = b.box:GetEffectiveScale()
    local _, bb, _, bh = b.box:GetScaledRect()
    local _, btb, _, bth = buttons[1]:GetScaledRect()
    local textH = b.plain:GetStringHeight()
    check(math.abs(((bb + bh) - (btb + bth)) / eff - (m.inset + textH + 4 + Bubble.META_H + 4)) < 1e-6,
        "under the Translate button (inset, the text, 4, the button), 4 units apart")
    check(math.abs(bh / eff - (2 * m.inset + textH + 4 + Bubble.META_H + 4 + b.barH)) < 1, "the bubble grows by the bar")
    M.ClearLinkCalls()
    M.ClickButton(buttons[1])
    check(#M.LinkCalls().refs == 1 and M.LinkCalls().refs[1].link == SPELL_DATA, "its links work the same")
    db.follow = "smart"
end
Reset()

---------------------------------------------------------------------------------------------
section("links: strings and privacy")
do
    local expected = H.fr and { "Clic : l'ouvrir, comme dans le chat.", "Maj-clic : l'ajouter à votre message." }
        or { "Click: open it, as in the chat.", "Shift-click: put it in your chat message." }
    check(L.CONV_LINK_TIP_CLICK == expected[1] and L.CONV_LINK_TIP_SHIFT == expected[2], "the link tooltip lines in "
        .. (H.fr and "French" or "English"))
    local onlyNumbers = true
    for key, value in pairs(Links.Counters()) do
        if type(key) ~= "string" or type(value) ~= "number" then
            onlyNumbers = false
        end
    end
    check(onlyNumbers and Links.Counters().clicks > 0, "Links.Counters: counts only")
    for _, s in ipairs({ "Lattice Leggings", "Sunlit Mantle", "Map Pin Location", "Mockname", "Links-Mine" }) do
        H.Private(s)
    end
end

for key, value in pairs(saved) do
    db[key] = value
end
