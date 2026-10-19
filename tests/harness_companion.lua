-- Translate Companion presence and the compact bubbles (user requests of 2026-10-19): the status
-- file and the presence it gives (running, stale, stopped, malformed, missing), every action that
-- asks for a translation gated by Presence.Ensure (no window, no rows, no request, no chat line
-- without a confirmed Translate Companion: only the centred popup), the popup (texts per state,
-- Download, Continue, Cancel, Escape, no keyboard focus, combat), its slot-125 card and VIEW 201,
-- no hint lane anywhere, the bubbles' row estimate and heights, and the strings in both locales.
-- Run by tests\harness.lua with the shared context H (see the top of that file). Starts and leaves
-- the session confirmed (the status file of tests\harness.lua said running), with no window,
-- popup, row, focus or setting change behind.
local H = ...
local M, ns, L, db, check, section = H.M, H.ns, H.L, H.db, H.check, H.section
local Conv, ConvFrame, Reserve, Request, Bubble, ChatButton, Bridge = ns.Conv, ns.ConvFrame, ns.Reserve, ns.Request,
    ns.Bubble, ns.ChatButton, ns.Bridge
local Presence, Dialog, Composer = ns.Presence, ns.CompanionDialog, ns.Composer

local URL = "https://github.com/Mouchoir/WoW-Chat-Translator/releases"
local saved = { display = db.display, follow = db.follow, glossShow = db.glossShow, names = db.names,
    convFocus = db.convFocus, convPos = db.convPos, replyClipboard = db.replyClipboard }
db.glossShow, db.names, db.convFocus, db.convPos = false, false, false, nil

local guidSeq = 800
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
local function WinShown()
    return Win() ~= nil and Win():IsShown()
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
-- A status file as Translate Companion writes it, then a /reload's reading of it.
local function Status(stateName, age)
    return { v = 1, version = "0.6.0", build = 6, state = stateName, heartbeat = time() - (age or 0) }
end
local function Reload(status)
    _G.TranslateProbe_CompanionStatus = status
    return Presence.Load()
end
local function Reset()
    Dialog.Close()
    Conv.Clear()
    if Win() then
        Win():Hide()
    end
    ns.Popup.Close()
    if M.Focused() then
        M.Focused():ClearFocus()
    end
    M.Tick(130)
end
-- Messages sent since n of a kind; the VIEWs whose first entry is the popup's (id 201).
local function Kinds(n)
    local kinds = {}
    for _, m in ipairs(H.Since(n)) do
        kinds[m.kind] = (kinds[m.kind] or 0) + 1
    end
    return kinds
end
local function PopupViews(n)
    local out = {}
    for _, m in ipairs(H.Since(n, "view")) do
        if m.views[1] and m.views[1].id == 201 then
            out[#out + 1] = m.views[1]
        end
    end
    return out
end
local function Only(kinds, allowed)
    for kind in pairs(kinds) do
        if not allowed[kind] then
            return false
        end
    end
    return true
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
        COMPANION_TITLE = "Translate Companion",
        COMPANION_TEXT_ABSENT = "Translate Companion n'est pas installé ou n'est pas lancé. Lancez-le sur votre ordinateur, puis cliquez sur Continuer. Pas installé ? Téléchargez-le.",
        COMPANION_TEXT_INSTALLED = "Translate Companion est installé mais n'est pas lancé. Lancez-le sur votre ordinateur, puis cliquez sur Continuer.",
        COMPANION_CARD_CAPTION = "Dès que Translate Companion tourne, il l'indique sur la barre ci-dessus.",
        COMPANION_BTN_CONTINUE = "Continuer",
        COMPANION_BTN_DOWNLOAD = "Télécharger",
        COMPANION_BTN_CANCEL = "Annuler",
        COMPANION_COPY_HINT = "Appuyez sur Ctrl+C pour copier le lien, puis collez-le dans votre navigateur.",
        CONV_HELPER_TIP = "Rien d'affiché ? Translate Companion est peut-être fermé.",
        APP_DOWNLOAD = "Pas encore installé ? Télécharge-le ici : %s",
        HELP_APP = "/tpr app - à propos de Translate Companion et où le télécharger",
    } or {
        COMPANION_TITLE = "Translate Companion",
        COMPANION_TEXT_ABSENT = "Translate Companion is not installed or not running. Start it on your computer, then click Continue. Not installed? Download it.",
        COMPANION_TEXT_INSTALLED = "Translate Companion is installed but not running. Start it on your computer, then click Continue.",
        COMPANION_CARD_CAPTION = "Once Translate Companion runs, it says so on the bar above.",
        COMPANION_BTN_CONTINUE = "Continue",
        COMPANION_BTN_DOWNLOAD = "Download",
        COMPANION_BTN_CANCEL = "Cancel",
        COMPANION_COPY_HINT = "Press Ctrl+C to copy the link, then paste it in your web browser.",
        CONV_HELPER_TIP = "Nothing painted? Translate Companion may be closed.",
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
    check(#wrong == 0, "popup, tooltip and /tpr app strings in this locale (" .. table.concat(wrong, ", ") .. ")")
    local gone = {}
    for _, key in ipairs({ "LANE_HINT", "LANE_HINT_SHORT", "LANE_DOWNLOAD", "CONV_LANE_HINT", "CONV_LANE_IN",
        "CONV_LANE_SHORT", "CONV_DOWNLOAD", "RPL_LANE_HINT", "FIRST_USE", "COMPANION_NOT_RUNNING", "COMPANION_DOWNLOAD",
        "COMPANION_CLOSE" }) do
        if L[key] ~= nil then
            gone[#gone + 1] = key
        end
    end
    check(#gone == 0, "no hint lane, first-use or old dialog string is left (" .. table.concat(gone, ", ") .. ")")
    -- frFR complete: every popup and status key of Locales.lua has its French line
    local src = ADDON_SOURCES and ADDON_SOURCES["Locales.lua"] or ""
    local enPart, frPart = src:match("local L = (%b{})"), src:match("local frFR = (%b{})")
    local function Keys(part)
        local keys = {}
        for key in (part or ""):gmatch("\n%s*([%u%d_]+)%s*=") do
            if key:find("^COMPANION_") or key:find("^APP_") then
                keys[key] = true
            end
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
    table.sort(missing)
    check(count >= 16 and #missing == 0, "every popup and /tpr app string has its frFR line (" .. count .. " keys; missing: "
        .. table.concat(missing, ", ") .. ")")
    local piped, dashed = {}, {}
    for key in pairs(en) do
        if type(L[key]) == "string" and L[key]:find("|", 1, true) then
            piped[#piped + 1] = key
        end
    end
    for _, file in ipairs({ "Locales.lua", "Presence.lua", "CompanionDialog.lua", "CompanionStatus.lua" }) do
        local s = ADDON_SOURCES and ADDON_SOURCES[file] or ""
        if s:find("\226\128\148", 1, true) or s:find("\226\128\166", 1, true) then
            dashed[#dashed + 1] = file
        end
    end
    check(#piped == 0 and #dashed == 0, "no pipe in the popup strings, no em dash or one-character ellipsis ("
        .. table.concat(piped, ", ") .. table.concat(dashed, ", ") .. ")")
    check(ns.COMPANION_URL == URL, "one download URL on the addon side: " .. URL)
    check(ns.VERSION == "0.7.0" and ns.ADDON_BUILD == 6, "addon version 0.7.0, protocol build 6")
    local printed = #M.Printed()
    H.slash("app")
    local lines = {}
    for i = printed + 1, #M.Printed() do
        lines[#lines + 1] = M.Printed()[i]
    end
    check(#lines == 7 and lines[1]:find(L.APP_1, 1, true) and lines[5]:find(L.APP_DOWNLOAD:format(URL), 1, true)
        and lines[6]:find(L.APP_STATUS_RUNNING:format(" 0.6.0"), 1, true) and lines[7]:find(L.APP_STATUS_CONFIRMED, 1, true),
        "/tpr app: the four lines, the download page, what the status file said, the session confirmed")
end

---------------------------------------------------------------------------------------------
section("companion: the status file")
do
    local now = time()
    local function Is(status, want, label)
        local got = Presence.Classify(status, now)
        check(got == want, label .. ": " .. want .. " (" .. tostring(got) .. ")")
    end
    Is(nil, "absent", "no status table (the file as the addon ships it)")
    Is({ v = 1, state = "running", heartbeat = now }, "running", "running, heartbeat now")
    Is({ v = 1, state = "running", heartbeat = now - 150 }, "running", "running, heartbeat 150 s old")
    Is({ v = 1, state = "running", heartbeat = now + 150 }, "running", "running, heartbeat 150 s ahead (clock skew)")
    Is({ v = 1, state = "running", heartbeat = now - 151 }, "installed", "running but stale (151 s)")
    Is({ v = 1, state = "running", heartbeat = now + 151 }, "installed", "running, 151 s ahead")
    Is({ v = 1, state = "stopped", heartbeat = now }, "installed", "stopped (a clean exit)")
    Is({ v = 1, state = "RUNNING", heartbeat = now }, "installed", "malformed: another word")
    Is({ v = 1, state = "running", heartbeat = tostring(now) }, "installed", "malformed: heartbeat as a string")
    Is({ v = 1, state = "running" }, "installed", "malformed: no heartbeat")
    Is({ v = 1, state = "running", heartbeat = 0 / 0 }, "installed", "malformed: NaN heartbeat")
    Is({ v = 1, state = "running", heartbeat = math.huge }, "installed", "malformed: infinite heartbeat")
    Is({}, "installed", "an empty table")
    Is("running", "installed", "not a table")
    Is(setmetatable({}, { __index = function()
        error("no")
    end }), "installed", "a table whose metamethods fail is read raw")
    local _, info = Presence.Classify({ v = 1, version = "0.6.0", build = 6, state = "running", heartbeat = now - 12 }, now)
    check(info.version == "0.6.0" and info.build == 6 and info.age == 12, "version, build and heartbeat age are read")
    local _, odd = Presence.Classify({ version = "0.6.0; os.exit()", build = "6", heartbeat = now }, now)
    check(odd.version == nil and odd.build == nil, "a version with other characters, a build as text: ignored")

    -- what a /reload reads (the global the deployed CompanionStatus.lua sets)
    check(Reload(Status("running", 30)) == "running" and Presence.IsConfirmed(), "running at load: the session starts confirmed")
    check(Reload(Status("running", 600)) == "installed" and not Presence.IsConfirmed(), "stale at load: installed, not confirmed")
    check(Reload(Status("stopped")) == "installed" and not Presence.IsConfirmed(), "stopped at load: installed, not confirmed")
    check(Reload(nil) == "absent" and not Presence.IsConfirmed(), "no status at load: absent, not confirmed")
    -- the shipped file: a comment and the global set to nil, nothing else
    local shipped = ADDON_SOURCES and ADDON_SOURCES["CompanionStatus.lua"] or ""
    local code = shipped:gsub("%-%-[^\n]*", ""):gsub("%s+", " ")
    check(code == " TranslateProbe_CompanionStatus = nil " or code == "TranslateProbe_CompanionStatus = nil "
        or code == " TranslateProbe_CompanionStatus = nil", "the shipped CompanionStatus.lua sets the global to nil only")
    local toc = {}
    for i, file in ipairs(ADDON_FILES) do
        toc[file] = i
    end
    check(toc["CompanionStatus.lua"] and toc["CompanionStatus.lua"] < toc["Core.lua"] and toc["Presence.lua"],
        "listed in the TOC before Core.lua")
end

---------------------------------------------------------------------------------------------
section("companion: [T] without Translate Companion opens only the popup")
do
    Reload(nil)     -- absent
    db.display = "window"
    local guid = NewGuid()
    local id = Line("CHAT_MSG_WHISPER", "Hallo, hast du Lust auf die Todesminen heute Abend?", "Gate-Pewpewp", guid)
    local base1 = H.EntryFor(ChatFrame1, id).message
    local function Untouched(label)
        check(not WinShown() and Conv.Get(guid) == nil and #Conv.List() == 0, label .. ": no conversation window, no conversation")
        check(H.EntryFor(ChatFrame1, id).message == base1 and Request.Count() == 0, label .. ": no rows under the line")
    end
    local printed, n = #M.Printed(), #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1)
    local d = Dialog.Frame()
    check(Dialog.IsShown() and d and d:GetFrameStrata() == "DIALOG" and d:IsToplevel() and d:IsClampedToScreen(),
        "a [T] click: the Translate Companion popup (strata DIALOG)")
    Untouched("[T]")
    local kinds = Kinds(n)
    check(Only(kinds, { view = true }) and (kinds.view or 0) >= 1, "nothing sent but the popup's VIEW (no TRANSLATE, no COLOURS)")
    check(#M.Printed() == printed, "nothing printed in the chat")
    check(M.Focused() == nil, "the popup never takes the keyboard focus")
    check(Presence.HasPending(), "the click waits for Continue")
    -- centred on the screen
    local dl, dbm, dw, dh = d:GetScaledRect()
    local sw, sh = UIParent:GetScaledRect()
    sw, sh = select(3, UIParent:GetScaledRect())
    check(math.abs((dl + dw / 2) - sw / 2) < 1 and math.abs((dbm + dh / 2) - sh / 2) < 1, "centred on the screen")
    -- the texts and buttons for "absent"
    check(d.Title:GetText() == L.COMPANION_TITLE and d.Body:GetText() == L.COMPANION_TEXT_ABSENT
        and d.Caption:GetText() == L.COMPANION_CARD_CAPTION, "title, the absent text, the card's caption")
    check(d.ContinueButton:IsShown() and d.DownloadButton:IsShown() and d.CancelButton:IsShown()
        and d.ContinueButton:GetText() == L.COMPANION_BTN_CONTINUE and d.DownloadButton:GetText() == L.COMPANION_BTN_DOWNLOAD
        and d.CancelButton:GetText() == L.COMPANION_BTN_CANCEL, "[Continue] [Download] [Cancel]")
    local _, cb = d.ContinueButton:GetScaledRect()
    local _, db2 = d.DownloadButton:GetScaledRect()
    local cl = d.ContinueButton:GetScaledRect()
    local dll = d.DownloadButton:GetScaledRect()
    local kl = d.CancelButton:GetScaledRect()
    check(cb == db2 and cl < dll and dll < kl, "in one row, Continue first")
    check(not d.Box:IsShown() and not d.CopyHint:IsShown(), "the download page waits for [Download]")
    check(tContains(UISpecialFrames, "TranslateProbeCompanion"), "Escape closes it (UISpecialFrames)")

    -- Ctrl-click (inline) and Shift-click: the same popup, still nothing else
    M.Tick(61)
    n = #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1, false, true)
    Untouched("Ctrl-click")
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1, true)
    Untouched("Shift-click")
    check(Only(Kinds(n), { view = true }) and Dialog.IsShown() and #M.Printed() == printed,
        "Ctrl and Shift: the popup again, nothing sent but VIEWs, nothing printed")

    -- Cancel drops the click; Escape too
    M.ClickButton(d.CancelButton)
    M.Tick(5)
    check(not Dialog.IsShown() and not Presence.HasPending() and not Presence.IsConfirmed(), "Cancel: closed, the click dropped")
    Untouched("after Cancel")
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1)
    PressEscape()
    M.Tick(5)
    check(not Dialog.IsShown() and not Presence.HasPending(), "Escape: closed, the click dropped")
    Untouched("after Escape")
    -- the key bindings and /tpr conv: the window needs Translate Companion too
    TranslateProbe_ConvToggle()
    check(Dialog.IsShown() and not WinShown(), "the show/hide binding: only the popup")
    Dialog.Close()
    H.slash("conv")
    check(Dialog.IsShown() and not WinShown(), "/tpr conv: only the popup")
    Dialog.Close()
    TranslateProbe_ConvReply()
    check(Dialog.IsShown() and not WinShown() and M.Focused() == nil, "the reply binding: only the popup, no focus taken")
    Dialog.Close()
    check(#M.Printed() == printed and not Presence.HasPending(), "nothing printed, nothing waiting")

    -- Continue runs the latest click once
    local guid2 = NewGuid()
    local id2 = Line("CHAT_MSG_WHISPER", "Bist du noch da? Wir warten am Eingang.", "Gate-Second", guid2)
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1)
    M.Click(H.ButtonOf(id2), ChatFrame1)
    n = #H.sent
    M.ClickButton(d.ContinueButton)
    local tr = H.Since(n, "translate")
    check(not Dialog.IsShown() and Presence.IsConfirmed() and not Presence.HasPending(), "Continue: closed, the session confirmed")
    check(WinShown() and Conv.Get(guid2) ~= nil and Conv.Get(guid) == nil and #tr == 1 and tr[1].slot >= 100
        and tr[1].slot <= 124, "the latest click runs, once: its conversation opens and its bubble is requested")
    local extra = {}
    for i = printed + 1, #M.Printed() do
        if not M.Printed()[i]:find(L.CONV_FIRST_TIP, 1, true) then
            extra[#extra + 1] = M.Printed()[i]
        end
    end
    check(#extra == 0, "nothing printed in the chat but the window's one-time tip")
    M.ClickButton(d.ContinueButton)
    check(#H.Since(n, "translate") == 1, "a second Continue runs nothing")
    -- confirmed: [T] acts at once, no popup
    M.Tick(61)
    n = #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1)
    check(not Dialog.IsShown() and Conv.Get(guid) ~= nil and #H.Since(n, "translate") == 1, "confirmed: [T] opens the window at once")
    M.Tick(61)
    db.display = "inline"
    local id3 = Line("CHAT_MSG_SAY", "wer kommt mit Gnomeregan, brauchen Tank", "Gate-Third", NewGuid())
    n = #H.sent
    M.Click(H.ButtonOf(id3), ChatFrame1)
    check(not Dialog.IsShown() and Request.Get(id3) ~= nil and #H.Since(n, "translate") == 1, "confirmed: inline rows at once")
    Request.Remove(id3)
    db.display = "window"
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: inline mode, Continue writes the rows")
do
    Reload(Status("stopped"))
    db.display = "inline"
    local id = Line("CHAT_MSG_SAY", "Hallo zusammen, wer kommt mit in die Todesminen? lfm", "Gate-Inline", NewGuid())
    local base = H.EntryFor(ChatFrame1, id).message
    local n = #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1, true)    -- Shift: without the cache
    local d = Dialog.Frame()
    check(Dialog.IsShown() and d.Body:GetText() == L.COMPANION_TEXT_INSTALLED and not d.DownloadButton:IsShown()
        and d.ContinueButton:IsShown() and d.CancelButton:IsShown(), "installed: its own text, no [Download]")
    local cl = d.ContinueButton:GetScaledRect()
    local kl = d.CancelButton:GetScaledRect()
    local _, _, cw = d.ContinueButton:GetScaledRect()
    local dl, _, dw = d:GetScaledRect()
    check(math.abs((cl + (kl + cw)) / 2 - (dl + dw / 2)) < 1, "[Continue] [Cancel] centred")
    check(H.EntryFor(ChatFrame1, id).message == base and #H.Since(n, "translate") == 0, "no rows, no TRANSLATE before Continue")
    M.ClickButton(d.ContinueButton)
    local tr = H.Since(n, "translate")
    check(#tr == 1 and tr[1].slot < 100 and tr[1].flags == 3 and H.EntryFor(ChatFrame1, id).message ~= base,
        "Continue: the rows and the TRANSLATE of the click, Shift kept (noCache)")
    check(H.Lines(H.EntryFor(ChatFrame1, id).message) == tr[1].rows, "only the K tagged rows under the line (no lane)")
    local snap = H.Snapshot()
    M.Tick(60 * 5)
    check(H.Snapshot() == snap, "no delayed rewrite afterwards")
    Request.Remove(id)
    db.display = "window"
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: the UI hidden while the popup shows; an old [T]")
do
    -- Alt+Z (or a cinematic) hides UIParent: the client fires the popup's OnHide while its own
    -- IsShown() stays true, then OnShow when the UI comes back (the mock does not propagate, so the
    -- scripts are called as the client calls them)
    Reload(nil)
    db.display = "window"
    local guid = NewGuid()
    local id = Line("CHAT_MSG_WHISPER", "Kommst du heute noch mit zum Kloster?", "Hide-Pewpewp", guid)
    M.Click(H.ButtonOf(id), ChatFrame1)
    local d = Dialog.Frame()
    local printed = #M.Printed()
    local n = #H.sent
    UIParent:Hide()
    d:GetScript("OnHide")(d)
    check(d:IsShown() and Presence.HasPending() and Dialog.ViewEntry() == nil, "UI hidden: the popup and its waiting click stay, its card is off screen")
    M.Tick(60 * 5)
    check(#PopupViews(n) == 0, "no VIEW of the card while the UI is hidden")
    n = #H.sent
    UIParent:Show()
    d:GetScript("OnShow")(d)
    check(Dialog.IsShown() and #PopupViews(n) == 1 and Presence.HasPending(), "UI back: the card's VIEW at once, the click still waits")
    M.Tick(60 * Dialog.VIEW_SECONDS + 2)
    check(#PopupViews(n) >= 2, "and again every 2 s")
    PressEscape()
    check(not Dialog.IsShown() and not Presence.HasPending() and not WinShown(), "Escape still closes it and drops the click")
    -- Continue after a hide and show runs the click
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1)
    UIParent:Hide()
    d:GetScript("OnHide")(d)
    UIParent:Show()
    d:GetScript("OnShow")(d)
    n = #H.sent
    M.ClickButton(d.ContinueButton)
    check(Presence.IsConfirmed() and WinShown() and Conv.Get(guid) ~= nil and #H.Since(n, "translate") == 1,
        "Continue after the UI came back: the waiting click runs")
    Reset()
    -- closed while the UI is hidden (no OnHide fires then): the click is dropped all the same
    Reload(nil)
    M.Click(H.ButtonOf(id), ChatFrame1)
    UIParent:Hide()
    d:GetScript("OnHide")(d)
    Dialog.Close()
    UIParent:Show()
    check(not Dialog.IsShown() and not Presence.HasPending(), "closed while the UI is hidden: closed, the click dropped")
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1)
    check(Dialog.IsShown() and Presence.HasPending(), "the next [T] opens it again")
    Dialog.Close()
    -- a [T] from before the last /reload: the popup first, nothing in the chat
    printed = #M.Printed()
    n = #H.sent
    M.Click("addon:TranslateProbe:t:0:42")
    check(Dialog.IsShown() and #M.Printed() == printed and Only(Kinds(n), { view = true }), "an old [T]: only the popup, nothing printed")
    M.ClickButton(d.ContinueButton)
    check(#M.Printed() == printed + 1 and M.Printed()[#M.Printed()]:find(L.OLD_SESSION, 1, true) ~= nil,
        "Continue: then the old-line notice")
    db.display = "window"
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: the popup's card and VIEW 201")
do
    Reload(nil)
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local id = Line("CHAT_MSG_SAY", "anyone selling linen near the bank", "Card-Pewpewp", NewGuid())
    local n = #H.sent
    M.Click(H.ButtonOf(id), ChatFrame1)
    local d = Dialog.Frame()
    local card = d.Card
    local m = Dialog.CardMetrics()
    local views = PopupViews(n)
    local v = views[1]
    local e = Dialog.ViewEntry()
    check(#views == 1 and v and e and v.id == 201 and v.x == e.x and v.y == e.y and v.w == e.w and v.h == e.h
        and v.fontPx == m.fontPx and v.rowPx == m.rowPx, "a VIEW goes out when it opens, the card first (id 201)")
    local l, b, w, h = card:GetScaledRect()
    check(math.abs(v.x - l * k) <= 0.5 and math.abs(v.y - (physH - (b + h) * k)) <= 0.5 and math.abs(v.w - w * k) <= 0.5
        and math.abs(v.h - h * k) <= 0.5, "VIEW 201: the card's fill in physical px")
    local others = 0
    for _, x in ipairs(H.Since(n, "view")[1].views) do
        others = others + ((x.id >= 1 and x.id < 200) and 1 or 0)
    end
    check(others >= 1 and #H.Since(n, "view")[1].views <= 4, "the shown chat frames follow it, 4 entries at most")
    -- one row of slot 125: 8 tag blocks of the tag colours, then the plate, over the bubble fill
    local sessBit = ns.sessionId % 2
    local colours = Bridge.TagColors(Bridge.PRESENCE_SLOT, 0, sessBit)
    local tagsOk = true
    local x0
    for i, t in ipairs(card.Tags) do
        local tl, tb, tw, th = t:GetScaledRect()
        x0 = x0 or tl
        if math.abs(t.r * 255 - colours[i][1]) > 1e-6 or math.abs(t.g * 255 - colours[i][2]) > 1e-6
            or math.abs(t.b * 255 - colours[i][3]) > 1e-6 or t.a ~= 1 or not t:IsVisible()
            or math.abs(tl - (x0 + (i - 1) * m.W * card:GetEffectiveScale())) > 1e-6 or math.abs(th * k - m.rowPx) > 1e-6
            or tw * k < 5 - 1e-6 or tb ~= select(2, card.Tags[1]:GetScaledRect()) then
            tagsOk = false
        end
    end
    check(tagsOk, "8 tag blocks of slot 125, row 0, this session, opaque, 5 px wide at least, rowPx high")
    local pl, pb, pw, ph = card.Plate:GetScaledRect()
    local pr, pg, pbl = card.Plate.r * 255, card.Plate.g * 255, card.Plate.b * 255
    check(math.abs(pr - 32) < 1e-6 and math.abs(pg - 40) < 1e-6 and math.abs(pbl - 64) < 1e-6 and card.Plate:IsVisible()
        and math.abs(pl - (x0 + 8 * m.W * card:GetEffectiveScale())) < 1e-6 and math.abs(ph * k - m.rowPx) < 1e-6,
        "then the plate: the dark strip (PLATE_RGB), one row")
    local fr, fg, fb = card.Fill.r * 255, card.Fill.g * 255, card.Fill.b * 255
    check(math.abs(fr - 36) < 1e-6 and math.abs(fg - 30) < 1e-6 and math.abs(fb - 24) < 1e-6 and card.Fill.a == 1,
        "over the opaque bubble fill (BUBBLE_RGB)")
    local tb = select(2, card.Tags[1]:GetScaledRect())
    check((x0 - l) * k >= 3 and (b + h - (tb + m.rowH * card:GetEffectiveScale())) * k >= 3 and (tb - b) * k >= 3
        and (l + w - (pl + pw)) * k >= 3, "3 px of fill at least above, under, left and right of the row")
    check((x0 - l) * k >= Dialog.DISCOVERY_LEFT * m.rowPx + 3 - 1e-6 and math.abs(pl + pw - (l + w - m.pad * card:GetEffectiveScale())) < 1e-6,
        "left of the tag, the fill a discovery scan samples (0.75 of the row height) and 3 px more; the plate ends pad before the right edge")
    local _, capB, _, capH = d.Caption:GetScaledRect()
    check(capB + capH <= b + 1e-6 and d.Caption:GetText() ~= "", "the caption is under the card, never on it")
    -- again every 2 s while it shows
    n = #H.sent
    M.Tick(60 * Dialog.VIEW_SECONDS + 2)
    check(#PopupViews(n) >= 1, "the VIEW goes out again every 2 s (a Companion started later finds the card)")
    -- [Download]: the page under the card; the card does not move
    n = #H.sent
    local beforeH = d:GetHeight()
    M.ClickButton(d.DownloadButton)
    local box = d.Box
    check(d.Box:IsShown() and d.CopyHint:IsShown() and d.CopyHint:GetText() == L.COMPANION_COPY_HINT and d:GetHeight() > beforeH,
        "[Download]: the page and the Ctrl+C hint, the popup grows")
    check(box:GetText() == URL and box.highlight and box.highlight[1] == 0 and box.highlight[2] == #URL and M.Focused() == box,
        "the URL, exactly, highlighted and focused for Ctrl+C after the click: " .. URL)
    local l2, b2 = card:GetScaledRect()
    check(math.abs(l2 - l) < 1e-6 and math.abs(b2 - b) < 1e-6 and #PopupViews(n) >= 1
        and PopupViews(n)[1].y == v.y, "the card stays where it was (the popup grows down)")
    M.Type(box, "x")
    check(box:GetText() == URL and box.highlight and box.highlight[2] == #URL, "read-only: typing puts the URL back, highlighted")
    M.Key(box, "ENTER")
    check(Dialog.IsShown() and M.Focused() == nil, "Enter in the box gives the keyboard back, the popup stays")
    -- closing: the card's VIEW goes
    n = #H.sent
    box:SetFocus()
    M.Key(box, "ESCAPE")
    check(not Dialog.IsShown() and M.Focused() == nil and not Presence.HasPending(), "Escape in the box closes it, the click dropped")
    local after = H.Since(n, "view")
    local has201 = false
    for _, x in ipairs(after[#after] and after[#after].views or {}) do
        has201 = has201 or x.id == 201
    end
    check(#after >= 1 and not has201 and Dialog.ViewEntry() == nil, "closed: a VIEW without the card")
    n = #H.sent
    M.Tick(60 * 5)
    check(#PopupViews(n) == 0, "no more VIEW for it once closed")
    -- combat: [Download] never takes the keys; combat start gives them back
    M.Tick(61)
    M.Click(H.ButtonOf(id), ChatFrame1)
    M.ClickButton(d.DownloadButton)
    M.SetCombat(true)
    check(Dialog.IsShown() and M.Focused() == nil, "combat start: the box loses the focus, the popup stays")
    Dialog.Close()
    M.Click(H.ButtonOf(id), ChatFrame1)
    M.ClickButton(d.DownloadButton)
    check(Dialog.IsShown() and M.Focused() == nil and d.Box:GetText() == URL, "[Download] in combat: the page, no keyboard focus")
    Dialog.Close()
    M.SetCombat(false)
    -- the UI scale changes while it shows: laid out and its VIEW sent again
    M.Click(H.ButtonOf(id), ChatFrame1)
    n = #H.sent
    M.SetScreen(1920, 1080, 0.9)
    local v2 = PopupViews(n)[1]
    local m2 = Dialog.CardMetrics()
    check(v2 and v2.rowPx == m2.rowPx and m2.W * m2.ppu >= 5 and math.abs(m2.ppu - 0.9 * 1080 / 768) < 1e-9,
        "a new UI scale: the card's metrics and VIEW follow")
    M.SetScreen(1920, 1080, 1)
    Dialog.Close()
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: the window's own translate actions are gated too")
do
    -- a conversation made while confirmed, then a session without Translate Companion
    Reload(Status("running"))
    db.display, db.follow = "window", "add"
    local guid = NewGuid()
    local id = Line("CHAT_MSG_WHISPER", "Hallo, kommst du mit in die Todesminen?", "Gate-Window", guid)
    M.Click(H.ButtonOf(id), ChatFrame1)
    local conv = Conv.Get(guid)
    Line("CHAT_MSG_WHISPER", "Wir brauchen noch einen Heiler, bitte.", "Gate-Window", guid)
    local plainMsg = conv.msgs[#conv.msgs]
    db.follow = saved.follow
    Reload(nil)
    ConvFrame.Relayout()
    local pb = ConvFrame.BubbleOf(plainMsg)
    check(WinShown() and plainMsg.state ~= "translate" and pb and pb.translateBtn:IsShown(), "an untranslated bubble with its Translate button")
    local n, printed = #H.sent, #M.Printed()
    M.ClickButton(pb.translateBtn)
    check(Dialog.IsShown() and plainMsg.state ~= "translate" and #H.Since(n, "translate") == 0 and #M.Printed() == printed,
        "its Translate: only the popup (no TRANSLATE, no chat line)")
    check(not tContains(UISpecialFrames, "TranslateProbeConvFrame"), "while it shows, Escape closes the popup only")
    PressEscape()
    M.Tick(2)
    check(not Dialog.IsShown() and WinShown() and tContains(UISpecialFrames, "TranslateProbeConvFrame"),
        "Escape: the popup goes, the window stays and is back in the Escape list")
    -- a click on a translated bubble (translate again)
    local first = conv.msgs[1]
    local fb = ConvFrame.BubbleOf(first)
    n = #H.sent
    M.ClickButton(fb.box, "LeftButton")
    check(Dialog.IsShown() and #H.Since(n, "translate") == 0 and not first.userK, "a bubble click: only the popup")
    Dialog.Close()
    -- the reply's Translate
    db.replyClipboard = "never"
    if not Conv.ReplyLang(conv) then
        Conv.SetReplyLang(conv, "de")
    end
    local box = Composer.Widgets().box
    box:SetFocus()
    M.Type(box, "oui j'arrive")
    n = #H.sent
    M.ClickButton(Composer.Widgets().translate)
    check(Dialog.IsShown() and #H.Since(n, "translate") == 0, "the reply's Translate: only the popup")
    -- Continue: the reply's TRANSLATE goes out
    M.ClickButton(Dialog.Frame().ContinueButton)
    local tr = H.Since(n, "translate")
    check(#tr == 1 and tr[1].slot == Reserve.REPLY_SLOT and tr[1].flags == 6, "Continue: the reply's TRANSLATE (slot 126)")
    -- confirmed now: the bubble's Translate acts at once
    n = #H.sent
    pb = ConvFrame.BubbleOf(plainMsg)
    M.ClickButton(pb.translateBtn)
    check(not Dialog.IsShown() and plainMsg.state == "translate" and #H.Since(n, "translate") == 1,
        "confirmed: the bubble's Translate acts at once")
    -- the bubble's tooltip says what to check when nothing is painted
    M.Hover(ConvFrame.BubbleOf(first).box)
    local tipped = false
    for _, line in ipairs(GameTooltip.lines or {}) do
        tipped = tipped or line == L.CONV_HELPER_TIP
    end
    check(GameTooltip:IsShown() and tipped, "the bubble's tooltip: \"" .. L.CONV_HELPER_TIP .. "\"")
    M.Leave()
    Composer.Reset()
    db.replyClipboard = saved.replyClipboard
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: no hint lane anywhere")
do
    Reload(Status("running"))
    check(Reserve.LaneRow == nil and Reserve.LANE_DELAY == nil and Request.LaneState == nil and ChatButton.LANE_START == nil
        and Bubble.LaneEdge == nil and Bubble.SetMoving == nil and Bubble.RefreshLanes == nil, "no lane code is left")
    -- inline: the rows and nothing gold under them, then or later
    db.display, db.glossShow = "inline", true
    local id = Line("CHAT_MSG_SAY", "Hallo zusammen, wer kommt mit in die Todesminen? lfm", "NoLane-Inline", NewGuid())
    M.Click(H.ButtonOf(id), ChatFrame1)
    M.Tick(60 * 3)
    local entry = H.EntryFor(ChatFrame1, id).message
    local K = select(3, Request.Get(id))
    check(not entry:find("|cffffd100", 1, true) and not entry:find(":d:", 1, true) and H.Lines(entry) == K + 1,
        "inline: K rows and the glossary row, no gold hint, no [Download], 3 s later too")
    -- an old [Download] link of this session does nothing
    local printed = #M.Printed()
    M.Click("addon:TranslateProbe:d:" .. ns.session .. ":" .. id, ChatFrame1)
    check(not Dialog.IsShown() and #M.Printed() == printed, "a [Download] link of an older build does nothing")
    Request.Remove(id)
    -- window: a bubble and the reply card have no lane
    db.display = "window"
    local guid = NewGuid()
    M.Click(H.ButtonOf(Line("CHAT_MSG_WHISPER", "Hallo, hast du heute Zeit?", "NoLane-Window", guid)), ChatFrame1)
    local msg = Conv.Get(guid).msgs[1]
    local b = ConvFrame.BubbleOf(msg)
    M.Tick(60 * 3)
    local m = ConvFrame.Metrics()
    local _, bb, _, bh = b.box:GetScaledRect()
    check(b.lane == nil and b.laneButton == nil and b.laneTop == nil
        and math.abs(bh / b.box:GetEffectiveScale() - (2 * m.inset + msg.K * m.rowH)) < 1,
        "a bubble: inset, K rows, inset; nothing under the rows")
    local reply = Bubble.AcquireReply(ConvFrame.composerHost)
    reply:SetDraft("on y va ?")
    reply:SetRows(Reserve.REPLY_SLOT, 2, ns.sessionId % 2)
    local rh = reply:Layout(m.bubbleWidth, m)
    check(reply.lane == nil and reply.SetLaneHint == nil and math.abs(rh - (8 + 14 + 4 + 2 * m.rowH + m.inset)) < 1,
        "the reply card: the draft line, its rows and the inset under them (no lane)")
    Bubble.Release(reply)
    check(bb ~= nil and not reply:IsShown(), "released")
    db.glossShow = false
end
Reset()

---------------------------------------------------------------------------------------------
section("companion: compact bubbles (row estimate and heights)")
do
    Reload(Status("running"))
    db.display = "window"
    -- the estimate: link names counted as they are, the other text times the bubble factor
    check(Reserve.WindowFactor("hello there", "fr") == 1.25 and Reserve.WindowFactor("hello there", "en") == 1.25,
        "Latin to Latin in a bubble: x1.25 (E1 output lengths)")
    check(math.abs(Reserve.WindowFactor("你好", "en") - 2.2 * 1.1) < 1e-9 and math.abs(Reserve.WindowFactor("привет", "fr") - 1.2 * 1.1) < 1e-9
        and math.abs(Reserve.WindowFactor("hello", "zh") - 0.8 * 1.1) < 1e-9, "other scripts: the inline factors leaned up by 10%, as before")
    check(Reserve.EstimateWindowRows(100, 296, "gg", "fr") == 1, "a short line: one row")
    check(Reserve.EstimateWindowRows(294, 296, "[Encantamiento: Encantar arma 2M: impacto]", "fr", 294) == 1,
        "a line that is one link name: as wide as it is (the Companion copies it)")
    check(Reserve.EstimateWindowRows(2000, 296, "long text", "fr", 0) == 6 and Reserve.EstimateWindowRows(0, 296, "", "fr") == 1,
        "6 rows at most, one for nothing")
    check(Reserve.EstimateWindowRows(350, 296, "x", "fr", 9999) == Reserve.EstimateWindowRows(350, 296, "x", "fr", 350),
        "link width capped at the text width")
    local function BubbleFor(text, sender)
        local guid = NewGuid()
        M.Click(H.ButtonOf(Line("CHAT_MSG_CHANNEL", text, sender, guid,
            { channelString = "2. Trade - City", channelIndex = 2, channelBase = "Trade - City", zoneChannelID = 2 })), ChatFrame1)
        local msg = Conv.Get(guid).msgs[1]
        return msg, ConvFrame.BubbleOf(msg)
    end
    local m = ConvFrame.Metrics()
    local ENCHANT = "|cffffd000|Henchant:7788|h[Encantamiento: Encantar arma 2M: impacto]|h|r"
    local msg, b = BubbleFor("Cherche " .. ENCHANT, "Compact-One")
    local _, _, _, bh = b.box:GetScaledRect()
    local eff = b.box:GetEffectiveScale()
    check(msg.K <= 2 and msg.K >= 1, "\"Cherche [Encantamiento: Encantar arma 2M: impacto]\" at the default window: "
        .. msg.K .. " row(s), not 3")
    local gap = Bubble.BarGap(m)
    check(math.abs(bh / eff - (2 * m.inset + msg.K * m.rowH + gap + b.barH)) < 1 and b.barH == m.rowH,
        "its bubble: inset, its rows, the gap, one line of links, inset")
    local _, pkb = b.rowsList[msg.K].plate:GetScaledRect()
    local _, btnB, _, btnH = b.linkButtons[1]:GetScaledRect()
    check(math.abs((pkb - (btnB + btnH)) / eff - gap) < 1e-6, "the links bar right under the last row (the small gap)")
    local msg2, b2 = BubbleFor("lfg", "Compact-Two")
    local _, _, _, bh2 = b2.box:GetScaledRect()
    check(msg2.K == 1 and math.abs(bh2 / eff - (2 * m.inset + m.rowH)) < 1, "a short line without links: one row, nothing else")
    -- a click asks for 6 rows: the bubble grows with them, still nothing under the last row
    M.ClickButton(b2.box, "LeftButton")
    b2 = ConvFrame.BubbleOf(msg2)
    local _, _, _, bh5 = b2.box:GetScaledRect()
    check(msg2.K == 6 and math.abs(bh5 / eff - (2 * m.inset + 6 * m.rowH)) < 1, "a bubble click: 6 rows, inset under the last")
    local long = "Wir suchen noch einen Heiler und zwei Schadensverursacher für die Schwarzfelsspitze, Treffpunkt in "
        .. "zehn Minuten am Eingang, bitte mit Erfahrung und Geduld"
    local msg3, b3 = BubbleFor(long, "Compact-Three")
    local _, _, _, bh3 = b3.box:GetScaledRect()
    local visible = ns.Glossary.VisibleText(long)
    local expected = math.min(6, math.max(1, math.ceil(Bubble.TextWidth(visible) / m.plateWidth * 1.25 - 0.15)))
    check(msg3.K == expected and math.abs(bh3 / eff - (2 * m.inset + msg3.K * m.rowH)) < 1,
        "a long line without links: width in plates x1.25 (" .. msg3.K .. " rows), no extra row")
    local medium = "Wir suchen noch einen Heiler für die Schwarzfelsspitze, Treffpunkt am Eingang"
    local msg4, b4 = BubbleFor(medium .. " " .. ENCHANT, "Compact-Four")
    local _, _, _, bh4 = b4.box:GetScaledRect()
    local nameW = Bubble.TextWidth("[Encantamiento: Encantar arma 2M: impacto]")
    local withName = math.min(6, math.max(1, math.ceil((Bubble.TextWidth(medium .. " ") * 1.25 + nameW) / m.plateWidth - 0.15)))
    local before = math.min(6, math.max(1, math.ceil(Bubble.TextWidth(ns.Glossary.VisibleText(medium .. " " .. ENCHANT))
        / m.plateWidth * 1.35 * 1.1 - 0.15)))
    check(msg4.K == withName and withName < before and math.abs(bh4 / eff - (2 * m.inset + msg4.K * m.rowH + gap + b4.barH)) < 1,
        "a longer line with a link: the name counted as it is, " .. msg4.K .. " rows (" .. before
        .. " with the factor on the whole line), then the bar")
end
Reset()

-- the session as the other files expect it: Translate Companion running, confirmed
Reload(Status("running"))
check(Presence.IsConfirmed() and not Dialog.IsShown() and not WinShown() and M.Focused() == nil,
    "companion: the session is confirmed again, no window, popup or focus left")
db.display, db.follow, db.glossShow, db.names, db.convFocus, db.convPos, db.replyClipboard = saved.display, saved.follow,
    saved.glossShow, saved.names, saved.convFocus, saved.convPos, saved.replyClipboard
