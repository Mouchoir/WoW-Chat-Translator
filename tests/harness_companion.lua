-- Hint lanes and the Translate Companion dialog (user requests U1-U3, 2026-10-19): the lane under
-- the inline rows and under the conversation window's bubbles (shown 1.5 s after the card's rows,
-- never before; gone with the card), its [Download] link or button, the dialog (exact URL, Ctrl+C
-- ready, Close, Escape, combat), the reply card's lane, and the "Translate Companion" strings in
-- both locales. Run by tests\harness.lua with the shared context H (see the top of that file).
-- Leaves no window, dialog, row, focus or setting change behind.
local H = ...
local M, ns, L, db, check, section = H.M, H.ns, H.L, H.db, H.check, H.section
local Conv, ConvFrame, Reserve, Request, Bubble, ChatButton = ns.Conv, ns.ConvFrame, ns.Reserve, ns.Request, ns.Bubble,
    ns.ChatButton
local Dialog = ns.CompanionDialog

local URL = "https://github.com/Mouchoir/WoW-Chat-Translator/releases"
local DELAY_FRAMES = 90         -- 1.5 s at the mock's 60 frames per second
local saved = { display = db.display, follow = db.follow, glossShow = db.glossShow, names = db.names,
    convFocus = db.convFocus, convPos = db.convPos }
db.glossShow, db.names, db.convFocus, db.convPos = true, false, false, nil

local guidSeq = 700
local function NewGuid()
    guidSeq = guidSeq + 1
    return "Player-1-0" .. guidSeq
end
local function Line(event, text, sender, guid, opts)
    local id = H.Deliver(event, text, sender, guid, false, opts)
    M.Tick(1)
    return id
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
local function Segments(message)
    local out = {}
    for seg in (message .. "|n"):gmatch("(.-)|n") do
        out[#out + 1] = seg
    end
    return out
end
local function DLink(lineID)
    return "addon:TranslateProbe:d:" .. ns.session .. ":" .. lineID
end
local function XLink(lineID)
    return "addon:TranslateProbe:x:" .. ns.session .. ":" .. lineID
end
local function Win()
    return ConvFrame.Frame()
end
-- What CloseSpecialWindows does on Escape: every shown frame named in UISpecialFrames hides.
local function PressEscape()
    local names = {}
    for i, name in ipairs(UISpecialFrames) do
        names[i] = name
    end
    for _, name in ipairs(names) do
        local f = _G[name]
        if f and f:IsShown() then
            f:Hide()
        end
    end
end
local function Reset()
    Conv.Clear()
    if Win() then
        Win():Hide()
    end
    ns.Popup.Close()
    Dialog.Close()
    M.Tick(130)
end

---------------------------------------------------------------------------------------------
section("companion: strings (" .. (H.fr and "frFR" or "enUS") .. ")")
do
    local helperLeft = {}
    for key, value in pairs(L) do
        if type(value) == "string" and value:find("Helper", 1, true) then
            helperLeft[#helperLeft + 1] = key
        end
    end
    table.sort(helperLeft)
    check(#helperLeft == 0, "no user string names the old Helper (" .. table.concat(helperLeft, ", ") .. ")")
    local expected = H.fr and {
        LANE_HINT = "Pas de traduction au-dessus ? Lance Translate Companion.",
        LANE_HINT_SHORT = "Lance Translate Companion.",
        LANE_DOWNLOAD = "Télécharger",
        CONV_LANE_IN = "Pas de traduction au-dessus ? Lancez Translate Companion.",
        CONV_LANE_SHORT = "Lancez Translate Companion.",
        CONV_LANE_HINT = "Pas d'état au-dessus ? Lancez Translate Companion.",
        RPL_LANE_HINT = "Pas d'état au-dessus ? Lancez Translate Companion.",
        CONV_DOWNLOAD = "Télécharger",
        COMPANION_TITLE = "Translate Companion",
        COMPANION_NOT_RUNNING = "Translate Companion n'est pas lancé. Lancez-le sur votre ordinateur.",
        COMPANION_DOWNLOAD = "Pas encore installé ? Téléchargez-le ici :",
        COMPANION_COPY_HINT = "Appuyez sur Ctrl+C pour copier le lien, puis collez-le dans votre navigateur.",
        COMPANION_CLOSE = "Fermer",
        APP_DOWNLOAD = "Pas encore installé ? Télécharge-le ici : %s",
        HELP_APP = "/tpr app - à propos de Translate Companion et où le télécharger",
    } or {
        LANE_HINT = "No translation above? Start Translate Companion.",
        LANE_HINT_SHORT = "Start Translate Companion.",
        LANE_DOWNLOAD = "Download",
        CONV_LANE_IN = "No translation above? Start Translate Companion.",
        CONV_LANE_SHORT = "Start Translate Companion.",
        CONV_LANE_HINT = "No status above? Start Translate Companion.",
        RPL_LANE_HINT = "No status above? Start Translate Companion.",
        CONV_DOWNLOAD = "Download",
        COMPANION_TITLE = "Translate Companion",
        COMPANION_NOT_RUNNING = "Translate Companion is not running. Start it on your computer.",
        COMPANION_DOWNLOAD = "Not installed? Download it from:",
        COMPANION_COPY_HINT = "Press Ctrl+C to copy the link, then paste it in your web browser.",
        COMPANION_CLOSE = "Close",
        APP_DOWNLOAD = "Not installed? Download it from %s",
        HELP_APP = "/tpr app - about Translate Companion and where to download it",
    }
    local wrong = {}
    for key, value in pairs(expected) do
        if L[key] ~= value then
            wrong[#wrong + 1] = key
        end
    end
    table.sort(wrong)
    check(#wrong == 0, "lane, dialog and /tpr app strings in this locale (" .. table.concat(wrong, ", ") .. ")")
    local piped = false
    for _, key in ipairs({ "LANE_HINT", "LANE_HINT_SHORT", "LANE_DOWNLOAD" }) do
        piped = piped or L[key]:find("|", 1, true) ~= nil
    end
    check(not piped, "the inline lane's strings hold no pipe (they sit inside chat links)")
    check(ns.COMPANION_URL == URL, "one download URL on the addon side: " .. URL)
    check(ns.VERSION == "0.5.0", "addon version 0.5.0")
    local printed = #M.Printed()
    H.slash("app")
    local lines = {}
    for i = printed + 1, #M.Printed() do
        lines[#lines + 1] = M.Printed()[i]
    end
    check(#lines == 5 and lines[5]:find(L.APP_DOWNLOAD:format(URL), 1, true) ~= nil and lines[1]:find(L.APP_1, 1, true) ~= nil,
        "/tpr app: the four lines, then where to download Translate Companion (the exact URL)")
    H.slash("help")
    check(H.Printed(L.HELP_APP), "/tpr help lists /tpr app")
end

---------------------------------------------------------------------------------------------
section("companion: Reserve.LaneRow fits the plate")
do
    local m = Reserve.Metrics(ChatFrame1)
    local function Fake(plateW)
        return { W = m.W, plateW = plateW, tex = m.tex }
    end
    local function Fits(row, plateW)
        local text = (row.hint or "") .. (row.download and ("[" .. row.download .. "]") or "")
        return select(1, Reserve.Measure(ChatFrame1, text)) <= (8 * m.W + plateW) * m.tex
    end
    local pending = Reserve.LaneRow(ChatFrame1, m, false)
    check(pending.lane and pending.hint == nil and pending.download == nil, "before the reveal: the row's room only")
    local wide = Reserve.LaneRow(ChatFrame1, Fake(600), true)
    check(wide.hint == L.LANE_HINT .. " " and wide.download == L.LANE_DOWNLOAD and Fits(wide, 600),
        "a wide chat: the full hint and [Download], within the plate's end")
    local mid = Reserve.LaneRow(ChatFrame1, m, true)
    check(mid.hint == L.LANE_HINT_SHORT .. " " and mid.download == L.LANE_DOWNLOAD and Fits(mid, m.plateW)
        and not Fits({ hint = L.LANE_HINT .. " ", download = L.LANE_DOWNLOAD }, m.plateW),
        "the default chat: the short hint (the full one would pass the plate's end)")
    local narrow = Reserve.LaneRow(ChatFrame1, Fake(70), true)
    check(narrow.hint == nil and narrow.download == L.LANE_DOWNLOAD and Fits(narrow, 70), "a narrow chat: [Download] alone")
    local tiny = Reserve.LaneRow(ChatFrame1, Fake(10), true)
    check(tiny.hint == nil and tiny.download == nil, "no room: spaces only, the row never wraps")
end

---------------------------------------------------------------------------------------------
section("companion: the hint lane under inline rows")
do
    db.display = "inline"
    local text = "Hallo zusammen, wer kommt mit in die Todesminen? Wir brauchen noch einen Heiler, lfm"
    local id = Line("CHAT_MSG_SAY", text, "Lane-Pewpewp", NewGuid())
    local base = H.EntryFor(ChatFrame1, id).message
    local n0 = #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1)
    local tr = H.Since(n0, "translate")[1]
    local K = tr and tr.rows
    local pendingEntry = H.EntryFor(ChatFrame1, id).message
    local segs = Segments(pendingEntry)
    local laneSeg = segs[K + 2] or ""
    check(K and Request.LaneState(id) == "pending" and laneSeg:find("|cffffd100    |H" .. XLink(id) .. "|h |h|r", 1, true) == 1,
        "at the click: one more row right after the K tagged rows, gold, spaces only")
    check(not pendingEntry:find(L.LANE_HINT_SHORT, 1, true) and not pendingEntry:find(DLink(id), 1, true),
        "no hint and no [Download] before 1.5 s")
    check(segs[K + 1]:find("|TInterface\\Buttons\\WHITE8X8:", 1, true) and segs[K + 3]
        and segs[K + 3]:find(ChatButton.GLOSS_START:sub(3), 1, true) == 1, "the lane sits between the last tagged row and the glossary row")
    local lines = H.Lines(pendingEntry)
    M.Tick(DELAY_FRAMES - 1)
    check(H.EntryFor(ChatFrame1, id).message == pendingEntry and Request.LaneState(id) == "pending",
        "still spaces at 1.5 s minus one frame (no rewrite before)")
    M.Tick(1)
    local shown = H.EntryFor(ChatFrame1, id).message
    local expectedLane = "|n|cffffd100    |H" .. XLink(id) .. "|h" .. L.LANE_HINT_SHORT .. " |h|r|cff66bbff|H" .. DLink(id)
        .. "|h[" .. L.LANE_DOWNLOAD .. "]|h|r"
    check(Request.LaneState(id) == "shown" and shown:find(expectedLane, 1, true) ~= nil,
        "at 1.5 s: the gold hint, then [Download] in our links' blue (a link of kind d)")
    check(H.Lines(shown) == lines and Count(shown, ChatButton.LANE_START) == 1 and shown:sub(1, #base) == base,
        "the same number of rows as before the reveal (no jump), one lane")
    local f3 = H.EntryFor(ChatFrame3, id)
    check(f3 and f3.message:find(DLink(id), 1, true) ~= nil, "the lane is in every chat frame that shows the line")
    local snap = H.Snapshot()
    M.Tick(60 * 20)
    check(H.Snapshot() == snap, "no other rewrite afterwards (the line fades like any other)")
    -- hovering [Download] shows no tooltip (it would cover the translation the Helper draws)
    EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkEnter", ChatFrame1, DLink(id), "[" .. L.LANE_DOWNLOAD .. "]")
    check(not GameTooltip:IsShown(), "no tooltip on [Download]")
    EventRegistry:TriggerEvent("ChatFrame.OnHyperlinkLeave", ChatFrame1)

    section("companion: [Download] opens the dialog")
    n0 = #H.sent
    M.Click(DLink(id), ChatFrame1)
    local d = Dialog.Frame()
    check(Dialog.IsShown() and d and d:GetFrameStrata() == "DIALOG" and d:IsToplevel() and d:IsClampedToScreen(),
        "[Download] opens the Translate Companion dialog (strata DIALOG)")
    check(#H.sent == n0 and H.EntryFor(ChatFrame1, id).message == shown and Request.LaneState(id) == "shown",
        "the rows and the lane stay, nothing sent")
    check(d.Title:GetText() == L.COMPANION_TITLE and d.Body:GetText() == L.COMPANION_NOT_RUNNING
        and d.Download:GetText() == L.COMPANION_DOWNLOAD and d.Hint:GetText() == L.COMPANION_COPY_HINT,
        "title, start-it, download-it and Ctrl+C texts")
    local box = d.Box
    check(box:GetText() == URL, "the URL, exactly: " .. URL)
    check(M.Focused() == box and box.highlight and box.highlight[1] == 0 and box.highlight[2] == #URL,
        "the URL is focused and highlighted, ready for Ctrl+C")
    check(d.CloseButton:GetText() == CLOSE, "a Close button with the client's own word")
    check(tContains(UISpecialFrames, "TranslateProbeCompanion"), "Escape closes it (UISpecialFrames)")
    M.Type(box, "x")
    check(box:GetText() == URL and box.highlight and box.highlight[2] == #URL, "read-only: typing puts the URL back, highlighted")
    M.Key(box, "ESCAPE")
    check(not Dialog.IsShown() and M.Focused() == nil, "Escape in the box closes it and frees the keyboard")
    M.Click(DLink(id), ChatFrame1)
    M.ClickButton(d.CloseButton)
    check(not Dialog.IsShown() and M.Focused() == nil, "Close closes it")
    M.Click(DLink(id), ChatFrame1)
    box:ClearFocus()
    PressEscape()
    check(not Dialog.IsShown(), "Escape without the box's focus (UISpecialFrames) closes it too")
    M.Tick(1)
    -- combat: the box gives the keys back, and a dialog opened in combat never takes them
    M.Click(DLink(id), ChatFrame1)
    M.SetCombat(true)
    check(Dialog.IsShown() and M.Focused() == nil, "combat start: the box loses the focus, the dialog stays")
    Dialog.Close()
    M.Click(DLink(id), ChatFrame1)
    check(Dialog.IsShown() and M.Focused() == nil and box:GetText() == URL, "opened in combat: shown, no keyboard focus")
    Dialog.Close()
    M.SetCombat(false)
    M.Tick(1)

    section("companion: inline lane removal")
    M.Tick(61)
    M.Click(XLink(id), ChatFrame1)
    check(H.EntryFor(ChatFrame1, id).message == base and Request.LaneState(id) == nil, "a click on the rows removes the lane with them")
    -- removed before the reveal: nothing comes back
    local id2 = Line("CHAT_MSG_SAY", "wer geht mit Gnomeregan, brauchen Tank", "Lane-Two", NewGuid())
    local base2 = H.EntryFor(ChatFrame1, id2).message
    M.Click(H.ButtonOf(id2), ChatFrame1)
    M.Tick(30)
    M.Click(XLink(id2), ChatFrame1)
    M.Tick(DELAY_FRAMES)
    check(H.EntryFor(ChatFrame1, id2).message == base2, "rows removed before 1.5 s: the reveal does nothing")
    -- a re-click while the lane waits: still one lane, revealed once
    M.Tick(61)
    M.Click(H.ButtonOf(id2), ChatFrame1)
    M.Tick(65)
    M.Click(H.ButtonOf(id2), ChatFrame1)
    M.Tick(30)
    local e2 = H.EntryFor(ChatFrame1, id2).message
    check(Request.LaneState(id2) == "shown" and Count(e2, ChatButton.LANE_START) == 1 and Count(e2, DLink(id2)) == 1,
        "a re-click keeps one lane, revealed 1.5 s after the first click")
    -- /tpr clear before the reveal
    M.Tick(61)
    M.Click(XLink(id2), ChatFrame1)
    local id3 = Line("CHAT_MSG_SAY", "lf1m heal, dann gehts los", "Lane-Three", NewGuid())
    local base3 = H.EntryFor(ChatFrame1, id3).message
    M.Click(H.ButtonOf(id3), ChatFrame1)
    Request.ClearAll()
    M.Tick(DELAY_FRAMES)
    check(H.EntryFor(ChatFrame1, id3).message == base3, "/tpr clear before 1.5 s: no lane comes back")
    db.display = saved.display
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: the hint lane under a window bubble")
do
    db.display = "window"
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local guid = NewGuid()
    local id = Line("CHAT_MSG_WHISPER", "Hallo, hast du Lust auf die Todesminen heute Abend? Wir brauchen noch jemanden.",
        "Bubble-Lane", guid)
    M.Click(H.ButtonOf(id), ChatFrame1)
    local conv = Conv.Get(guid)
    local msg = conv and conv.msgs[1]
    local b = msg and ConvFrame.BubbleOf(msg)
    local m = ConvFrame.Metrics()
    check(b and msg.req and b.lane and b.laneButton and not b.lane:IsShown() and not b.laneButton:IsShown(),
        "at the click: the bubble has a lane, its hint hidden")
    -- geometry: the band [row0.yTop + K * rowPx, + rowPx], the hint from the plate's x, clear of the
    -- lane's top and bottom by 2 units and 2 physical px at least
    local K = msg.K
    local edge = Bubble.LaneEdge(m)
    check(edge == math.max(2, 2 / m.ppu) and edge * m.ppu >= 2 - 1e-9, "the lane's margin: 2 units, 2 px at least")
    local _, p1b, _, p1h = b.rowsList[1].plate:GetScaledRect()
    local row0Top = (p1b + p1h) * k
    local hl, hb, hw, hh = b.lane:GetScaledRect()
    local hintTop = (hb + hh) * k
    local eff = b.box:GetEffectiveScale()
    local bl, bb, _, bh = b.box:GetScaledRect()
    check(math.abs((row0Top - hintTop) - (K * m.rowPx + edge * m.ppu)) < 1e-3,
        "the hint starts its margin under row0.yTop + K * rowPx (the lane's top)")
    check(math.abs(hh / eff - (m.rowH - 2 * edge)) < 1e-6, "the hint is the row's height less the margin at each edge")
    check(math.abs((hl - bl) / eff - (m.inset + 8 * m.W)) < 1e-6, "the hint starts at the plate's x (the lane's tag column is plain fill)")
    local btl, btb, btw, bth = b.laneButton:GetScaledRect()
    local plateEnd = bl + (m.inset + 8 * m.W + m.plateWidth) * eff
    check(math.abs(btl - (hl + hw + 4 * eff)) < 1e-6 and btl + btw <= plateEnd + 1e-6 and math.abs((btb + bth) - (hb + hh)) < 1e-6,
        "[Download] 4 units after the hint, on the same band, before the plate's end")
    local laneBottom = row0Top - (K + 1) * m.rowPx
    check(laneBottom - bb * k >= (m.border + 3 / m.ppu) * m.ppu - 1e-6, "fill under the lane: the border and 3 px at least")
    check(math.abs(bh / eff - (2 * m.inset + (K + 1) * m.rowH)) < 1, "the bubble is one row taller: inset, K rows, the lane, inset")
    local tw = b.lane:GetUnboundedStringWidth()
    local fullW = (function()
        local t = b.lane:GetText()
        b.lane:SetText(L.CONV_LANE_IN)
        local w = b.lane:GetUnboundedStringWidth()
        b.lane:SetText(t)
        return w
    end)()
    local bw = b.laneButton:GetWidth()
    local fitsFull = fullW + 2 + 4 + bw <= m.plateWidth
    check((fitsFull and b.lane:GetText() == L.CONV_LANE_IN) or (not fitsFull and b.lane:GetText() == L.CONV_LANE_SHORT
        and tw + 2 + 4 + bw <= m.plateWidth), "the longest hint that fits beside [Download] (" .. b.lane:GetText() .. ")")
    local laneFont = _G.TranslateProbeLaneFont or GameFontNormalSmall
    local lr, lg, lbl = laneFont:GetTextColor()
    local _, laneSize = laneFont:GetFont()
    check(laneFont and b.lane.fontObject == laneFont and b.laneButton:GetFontString().fontObject == laneFont,
        "the hint and [Download] use TranslateProbeLaneFont")
    check(lr == 1 and lg == 0.82 and lbl == 0 and laneSize <= m.rowH - 2 * edge - 1 and laneSize == 10,
        "the lane font: GameFontNormalSmall's gold and size (" .. tostring(laneSize) .. "), a point under the hint's box")
    check(b.laneButton:GetFontString():GetText() == "|cff66bbff[" .. L.CONV_DOWNLOAD .. "]|r", "[Download] in our links' blue")
    -- text size 12 at 1.1 px per unit: rows of 15 px, the hint's box 9.6 units: an 8-point lane font
    local convText = db.convText
    db.convText = 12
    M.SetScreen(1920, 1080, 1.1 * 768 / 1080)
    ConvFrame.Relayout()
    local m12 = ConvFrame.Metrics()
    local e12 = Bubble.LaneEdge(m12)
    local _, size12 = laneFont:GetFont()
    check(m12.rowPx == 15 and e12 == 2 and size12 == 8 and math.abs(b.lane:GetHeight() - (m12.rowH - 2 * e12)) < 1e-6,
        "text size 12, 1.1 px per unit: the lane font shrinks to fit its box (" .. size12 .. ")")
    -- a pixel-perfect UI (UIParent at 768 / physH) and the window at 80%: 0.8 px per unit, so 2 units
    -- are 1.6 px: the margin grows to 2 px, the hint's box and font shrink with it
    local convScale = db.convScale
    db.convText, db.convScale = convText, 80
    M.SetScreen(1920, 1080, 768 / 1080)
    ConvFrame.Relayout()
    local m80 = ConvFrame.Metrics()
    local e80 = Bubble.LaneEdge(m80)
    local _, size80 = laneFont:GetFont()
    local _, p80b, _, p80h = b.rowsList[1].plate:GetScaledRect()
    local _, h80b, _, h80h = b.lane:GetScaledRect()
    local laneTop80 = (p80b + p80h) * k - msg.K * m80.rowPx
    local hintTop80, hintBottom80 = (h80b + h80h) * k, h80b * k
    check(math.abs(m80.ppu - 0.8) < 1e-6 and math.abs(e80 - 2.5) < 1e-6 and size80 <= math.floor(m80.rowH - 2 * e80) - 1,
        "window at 80% on a pixel-perfect UI: margin 2.5 units (2 px), lane font " .. size80)
    check(laneTop80 - hintTop80 >= 2 - 1e-6 and hintBottom80 - (laneTop80 - m80.rowPx) >= 2 - 1e-6,
        "the hint's box keeps clear of the lane's outer 2 px rows (the Helper reads them as plain fill)")
    db.convScale = convScale
    M.SetScreen(1920, 1080, 1)
    ConvFrame.Relayout()
    check(select(2, laneFont:GetFont()) == 10, "back to the default: 10")
    -- the delayed reveal (the layouts above moved it: it waits from the last one)
    M.Tick(DELAY_FRAMES - 1)
    check(not b.lane:IsShown() and not b.laneButton:IsShown(), "still hidden at 1.5 s minus one frame")
    M.Tick(1)
    check(b.lane:IsShown() and b.laneButton:IsShown(), "shown at 1.5 s")
    -- [Download] in a bubble: a right-click and the hover go on to the bubble
    M.Hover(b.laneButton)
    check(GameTooltip:IsShown() and GameTooltip:GetOwner() == b.box, "hovering [Download]: the bubble's tooltip")
    M.Leave()
    check(not GameTooltip:IsShown(), "and it goes with the cursor")
    M.ClickButton(b.laneButton, "RightButton")
    check(ns.Popup.IsOpen() and not Dialog.IsShown(), "a right-click on [Download]: the bubble's menu, not the dialog")
    ns.Popup.Close()
    M.Tick(1)
    -- the window dragged: every hint hides at once (the Helper's VIEW is stale), then waits again
    local wl0, wb0 = Win():GetScaledRect()
    M.Hover(Win().TitleContainer)
    M.Drag(Win(), 40, -30)
    M.Leave()
    local wl1, wb1 = Win():GetScaledRect()
    check(wl1 ~= wl0 and wb1 ~= wb0 and not b.lane:IsShown() and not b.laneButton:IsShown(), "dragged: the hint hides at its new place")
    M.Tick(DELAY_FRAMES - 1)
    check(not b.lane:IsShown(), "still hidden 1.5 s minus one frame after the drag")
    M.Tick(1)
    check(b.lane:IsShown(), "shown 1.5 s after the drag")
    Bubble.SetMoving(true)
    check(not b.lane:IsShown(), "while the window moves: hidden")
    M.Tick(DELAY_FRAMES)
    check(not b.lane:IsShown(), "and still hidden 1.5 s into the move")
    Bubble.SetMoving(false)
    M.Tick(DELAY_FRAMES)
    check(b.lane:IsShown(), "1.5 s after the move: shown")
    -- the grip pressed (a resize): hidden until 1.5 s after it ends
    M.MouseDown(ConvFrame.Widgets().grip)
    check(not b.lane:IsShown(), "the resize grip pressed: hidden")
    M.MouseUp(ConvFrame.Widgets().grip)
    M.Tick(DELAY_FRAMES)
    check(b.lane:IsShown(), "1.5 s after the resize: shown")
    -- [Download]: the dialog, centred on the window; the window leaves the Escape list meanwhile
    check(M.ClickButton(b.laneButton), "[Download] is a button")
    local d = Dialog.Frame()
    local dl, db_, dw, dh = d:GetScaledRect()
    local wl, wb, ww, wh = Win():GetScaledRect()
    check(Dialog.IsShown() and math.abs((dl + dw / 2) - (wl + ww / 2)) < 1e-6 and math.abs((db_ + dh / 2) - (wb + wh / 2)) < 1e-6,
        "the dialog opens centred on the conversation window")
    check(not tContains(UISpecialFrames, "TranslateProbeConvFrame") and tContains(UISpecialFrames, "TranslateProbeCompanion"),
        "while it shows, Escape closes the dialog only")
    d.Box:ClearFocus()
    PressEscape()
    check(not Dialog.IsShown() and Win():IsShown(), "Escape: the dialog goes, the window stays")
    M.Tick(1)
    check(tContains(UISpecialFrames, "TranslateProbeConvFrame"), "the window is back in the Escape list a frame later")
    -- a popup and the dialog together: the window stays out until both are gone
    M.ClickButton(b.box, "RightButton")
    check(ns.Popup.IsOpen() and not tContains(UISpecialFrames, "TranslateProbeConvFrame"), "a bubble popup opens")
    Dialog.Open(Win())
    ns.Popup.Close()
    M.Tick(2)
    check(Dialog.IsShown() and not tContains(UISpecialFrames, "TranslateProbeConvFrame"), "the popup closed, the dialog still holds the window out")
    Dialog.Close()
    M.Tick(1)
    check(tContains(UISpecialFrames, "TranslateProbeConvFrame"), "both closed: the window is back once")
    local inList = 0
    for _, name in ipairs(UISpecialFrames) do
        inList = inList + (name == "TranslateProbeConvFrame" and 1 or 0)
    end
    check(inList == 1, "listed once")

    section("companion: window lanes follow the request, the window and the bubble")
    -- a click asks again with 6 rows: the lane moves, hidden again for 1.5 s
    M.ClickButton(b.box, "LeftButton")
    check(msg.K == 6 and not b.lane:IsShown(), "a bubble click (6 rows, a new request): the lane moves and waits again")
    local _, nb, _, nh = b.lane:GetScaledRect()
    local _, q1b, _, q1h = b.rowsList[1].plate:GetScaledRect()
    check(math.abs(((q1b + q1h) * k - (nb + nh) * k) - (6 * m.rowPx + 2 * m.ppu)) < 1e-3, "now under row 6")
    M.Tick(DELAY_FRAMES)
    check(b.lane:IsShown(), "shown 1.5 s later")
    -- the window hides and shows: the lane waits again
    Win():Hide()
    check(not b.lane:IsShown() and not b.laneButton:IsShown(), "the window hides: the hint goes")
    ConvFrame.Show()
    check(not b.lane:IsShown(), "the window shows: the hint waits")
    M.Tick(DELAY_FRAMES)
    check(b.lane:IsShown(), "and comes back 1.5 s later")
    -- a re-layout that changes nothing keeps it
    ConvFrame.Relayout()
    check(b.lane:IsShown(), "a layout that moves nothing keeps the hint")
    -- removed: the bubble is released, its lane hidden, a pending reveal does nothing
    local guid2 = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", "bist du noch da? wir warten am Eingang", "Bubble-Two", guid2)), ChatFrame1)
    local conv2 = Conv.Get(guid2)
    local msg2 = conv2.msgs[1]
    local b2 = ConvFrame.BubbleOf(msg2)
    M.Tick(30)
    Conv.RemoveMsg(conv2, msg2)
    M.Tick(DELAY_FRAMES)
    check(not b2.lane:IsShown() and not b2.laneButton:IsShown() and not b2.inUse, "a removed bubble: its lane never shows")
    -- an untranslated bubble (follow add): no lane
    db.follow = "add"
    local guid3 = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_CHANNEL", "WTS Leinenstoff billig, flüstert mir", "Plain-Lane", guid3,
        { channelString = "2. Trade - City", channelIndex = 2, channelBase = "Trade - City", zoneChannelID = 2 })), ChatFrame1)
    local conv3 = Conv.Get(guid3)
    Line("CHAT_MSG_CHANNEL", "noch 20 Stück Leinenstoff da", "Plain-Lane", guid3,
        { channelString = "2. Trade - City", channelIndex = 2, channelBase = "Trade - City", zoneChannelID = 2 })
    local plainMsg = conv3.msgs[#conv3.msgs]
    local pb = ConvFrame.BubbleOf(plainMsg)
    M.Tick(DELAY_FRAMES)
    check(plainMsg.state ~= "translate" and pb and pb.laneTop == nil and not pb.lane:IsShown() and not pb.laneButton:IsShown(),
        "an untranslated bubble has no lane")
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: window lanes at the thread's edges")
do
    db.display, db.follow = "window", "translate"
    local guid = NewGuid()
    local long = "Wir suchen noch einen Heiler und zwei Schadensverursacher für die Schwarzfelsspitze, Treffpunkt in "
        .. "zehn Minuten am Eingang, bitte mit Erfahrung und Geduld, danke schön"
    M.Click(H.ButtonOf(Line("CHAT_MSG_SAY", long, "Edge-Lane", guid)), ChatFrame1)
    local conv = Conv.Get(guid)
    for i = 1, 12 do
        H.Deliver("CHAT_MSG_SAY", long .. " " .. i, "Edge-Lane", guid)
    end
    M.Tick(1)
    local scroll = ConvFrame.Widgets().scroll
    local m = ConvFrame.Metrics()
    local function LaneState()
        local _, sb, _, sh = scroll:GetScaledRect()
        local top, bottom = sb + sh, sb
        local wrongShown, wrongHidden, shown, cut = 0, 0, 0, 0
        for _, msg in ipairs(conv.msgs) do
            local b = ConvFrame.BubbleOf(msg)
            if b and b.lane then
                local _, lb, _, lh = b.lane:GetScaledRect()
                local eff = b.box:GetEffectiveScale()
                local laneTop, laneBottom = lb + lh + 2 * eff, lb - 2 * eff
                local _, pb, _, ph = b.rowsList[msg.K].plate:GetScaledRect()
                local inside = laneTop <= top + 0.01 and laneBottom >= bottom - 0.01 and pb + ph <= top + 0.01 and pb >= bottom - 0.01
                if not inside and (laneTop > top + 0.01 and laneBottom < top or laneBottom < bottom - 0.01 and laneTop > bottom) then
                    cut = cut + 1
                end
                local should = inside and msg.req ~= nil
                if b.lane:IsShown() then
                    shown = shown + 1
                    if not should then
                        wrongShown = wrongShown + 1
                    end
                elseif should then
                    wrongHidden = wrongHidden + 1
                end
            end
        end
        return wrongShown, wrongHidden, shown, cut
    end
    M.Tick(DELAY_FRAMES)
    local wrongShown, wrongHidden, shown = LaneState()
    check(shown > 0 and wrongShown == 0 and wrongHidden == 0, "at the end of the thread: every whole lane with a request shows (" .. shown .. ")")
    local base = scroll:GetVerticalScroll()
    local cut
    for _, rows in ipairs({ 2.5, 3.5, 1.5, 4.5, 0.5, 5.5 }) do
        scroll:SetVerticalScroll(math.max(0, base - m.rowH * rows))
        wrongShown, wrongHidden, shown, cut = LaneState()
        if cut > 0 then
            break
        end
    end
    check(cut > 0 and wrongShown == 0, "a lane cut by the thread's edge, or above a cut last row, never shows (" .. cut .. " cut)")
    -- every lane moved with the scroll: the hint moves one capture before the Helper's paint does,
    -- so each one hides at its new place and waits again
    check(shown == 0, "right after a scroll every moved lane hides (" .. shown .. " shown)")
    M.Tick(DELAY_FRAMES)
    wrongShown, wrongHidden = LaneState()
    check(wrongShown == 0 and wrongHidden == 0, "1.5 s after the scroll: every whole lane with a request shows again")
    db.follow = saved.follow
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: the reply card's lane")
do
    ConvFrame.Build()
    ConvFrame.Show()
    local host = ConvFrame.composerHost
    local card = Bubble.AcquireReply(host)
    local m = ConvFrame.Metrics()
    card:SetDraft("on y va ?")
    card:SetRows(Reserve.REPLY_SLOT, 2, ns.sessionId % 2)
    card:SetLaneHint(L.RPL_LANE_HINT)
    card:ClearAllPoints()
    card:SetPoint("BOTTOMLEFT", host, "BOTTOMLEFT", m.x0, 100)
    card:Layout(m.bubbleWidth, m)
    check(card.laneButton and not card.lane:IsShown() and not card.laneButton:IsShown(), "a new reply: the lane's hint waits")
    M.Tick(DELAY_FRAMES - 1)
    check(not card.lane:IsShown(), "still hidden at 1.5 s minus one frame")
    M.Tick(1)
    -- the full hint when it fits beside [Download] in the plate's width, else the short one
    local shownText = card.lane:GetText()
    card.lane:SetText(L.RPL_LANE_HINT)
    local fitsFull = card.lane:GetUnboundedStringWidth() + 2 + 4 + card.laneButton:GetWidth() <= m.plateWidth
    card.lane:SetText(shownText)
    local want = fitsFull and L.RPL_LANE_HINT or L.CONV_LANE_SHORT
    check(card.lane:IsShown() and card.laneButton:IsShown() and shownText == want
        and card.lane.fontObject == _G.TranslateProbeLaneFont, "then \"" .. want .. "\" in gold, with [Download]")
    M.ClickButton(card.laneButton)
    check(Dialog.IsShown(), "the reply card's [Download] opens the dialog too")
    Dialog.Close()
    M.Tick(1)
    card:Layout(m.bubbleWidth, m)
    check(not card.lane:IsShown(), "laid out again (a new reply): it waits again")
    Win():Hide()
    M.Tick(DELAY_FRAMES)
    check(not card.lane:IsShown(), "the window hidden: no hint")
    ConvFrame.Show()
    check(not card.lane:IsShown(), "the window shown again: it waits")
    M.Tick(DELAY_FRAMES)
    check(card.lane:IsShown(), "then shows")
    Bubble.Release(card)
    check(not card.lane:IsShown() and not card.laneButton:IsShown(), "released: no hint")
    Win():Hide()
end
Reset()

db.display, db.follow, db.glossShow, db.names, db.convFocus, db.convPos = saved.display, saved.follow, saved.glossShow,
    saved.names, saved.convFocus, saved.convPos
