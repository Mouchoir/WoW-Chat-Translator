-- The Translate Companion popup (user request of 2026-10-19): what a [T] click opens, alone, while
-- the session has no confirmed Translate Companion (Presence.lua). Centred on the screen, it says
-- Translate Companion is not running (or not installed) and offers:
--   [Continue]  the user started it: the session is confirmed and the click runs
--   [Download]  the download page in a read-only box, highlighted for Ctrl+C (WoW cannot open a
--               browser); not shown when the status file says Translate Companion is installed
--   [Cancel]    the click is dropped (Escape too)
-- Live confirmation without /reload: the popup holds a small status card, one row of slot 125
-- (Bridge.PRESENCE_SLOT: a tag and a plate over the opaque BUBBLE_RGB fill, like a bubble's rows,
-- docs\BRIDGE.md section 8d), and while it shows the card is VIEW 201 (sent when it opens, then
-- every 2 s, so a Translate Companion started after the popup finds it within about 2 s). A
-- running Translate Companion paints "Translate Companion detected" in green over the card; the
-- addon's own grey caption under the card (never on it) says what the card is for.
--
-- Same art as the consent dialog (Consent.lua): DialogBorderDarkTemplate, else a tooltip border,
-- else a flat dark fill; strata DIALOG. It never takes the keyboard focus when it opens: only the
-- URL box does, after a click on [Download], out of combat (combat start takes it back). Escape
-- closes it: in the box (OnEscapePressed), or through UISpecialFrames; while it shows, the
-- conversation window leaves that list (Popup.HoldWindowEscape), so Escape closes only the popup.
local _, ns = ...
local L = ns.L

local CompanionDialog = {}
ns.CompanionDialog = CompanionDialog

local FRAME_NAME = "TranslateProbeCompanion"
local WIDTH, PAD = 400, 20
local BOX_H, BUTTON_W, BUTTON_H, BUTTON_GAP = 22, 112, 22, 8
local CARD_TEXT = 12            -- the card's text size: VIEW 201's fontPx, rows of CARD_TEXT + 2 units
local VIEW_ID = 201             -- the popup's VIEW id (200 is the conversation window)
local VIEW_SECONDS = 2          -- the VIEW goes out again this often while the popup shows
CompanionDialog.VIEW_ID = VIEW_ID
CompanionDialog.VIEW_SECONDS = VIEW_SECONDS
CompanionDialog.FRAME_NAME = FRAME_NAME

local floor, ceil, max = math.floor, math.ceil, math.max
local dialog
local state = "absent"          -- the presence the popup was opened for
local continuing = false        -- Continue is closing the popup: the waiting action must not drop
local ticker
local counters = { opened = 0, views = 0 }

local function HasTemplate(name)
    if not (C_XMLUtil and type(C_XMLUtil.GetTemplateInfo) == "function") then
        return false
    end
    local ok, info = pcall(C_XMLUtil.GetTemplateInfo, name)
    return ok and info ~= nil
end

local function IsNumber(v)
    return not issecretvalue(v) and type(v) == "number" and v == v
end

local function Rgb255(t)
    return t[1] / 255, t[2] / 255, t[3] / 255
end

local function NoSnap(t)
    t:SetSnapToPixelGrid(false)
    t:SetTexelSnappingBias(0)
end

local function Snap(v)
    return PixelUtil.GetNearestPixelSize(v, dialog:GetEffectiveScale())
end

---------------------------------------------------------------------------- the status card
-- Geometry of the card in the popup's units, like a bubble's (Reserve.WindowMetrics): ppu physical
-- px per unit; tag blocks W units (5 px at least); one row rowPx physical px high (rowH units);
-- pad: fill above, under and right of the row, 6 units and 4 px at least (the Helper needs 3 px of
-- fill around a card); left: the fill left of the tag, wider, because a Translate Companion that
-- finds the card by its discovery scan (started after the popup's last VIEW) samples the background
-- up to 0.75 of a row height left of the tag (helper\app\src\backdrop.rs, DISCOVERY_LEFT): with
-- less fill it sampled the popup's own background and painted a darker strip over the card until
-- the next VIEW 201 (integration review); the plate from the tag's end to pad before the right edge.
CompanionDialog.DISCOVERY_LEFT = 0.75
function CompanionDialog.CardMetrics()
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    local eff = dialog and dialog:GetEffectiveScale() or UIParent:GetEffectiveScale()
    if not (okSize and IsNumber(physH) and physH > 0 and IsNumber(eff) and eff > 0) then
        physH, eff = 768, 1
    end
    local m = { ppu = eff * physH / 768, textSize = CARD_TEXT }
    m.W = max(2, ceil(5 / m.ppu))
    m.rowPx = max(1, floor((CARD_TEXT + 2) * m.ppu + 0.5))
    m.rowH = m.rowPx / m.ppu
    m.fontPx = floor(CARD_TEXT * m.ppu + 0.5)
    m.pad = max(6, ceil(4 / m.ppu))
    m.left = max(m.pad, ceil((CompanionDialog.DISCOVERY_LEFT * m.rowPx + 3) / m.ppu))
    m.width = WIDTH - 2 * PAD
    m.plateWidth = m.width - m.left - m.pad - 8 * m.W
    m.height = 2 * m.pad + m.rowH
    return m
end

local function BuildCard()
    local card = CreateFrame("Frame", nil, dialog)
    local fill = card:CreateTexture(nil, "BACKGROUND")
    fill:SetColorTexture(Rgb255(ns.Bridge.BUBBLE_RGB))   -- art-ok: protocol colour
    fill:SetAllPoints(card)
    NoSnap(fill)
    card.Fill = fill
    card.Tags = {}
    for i = 1, 8 do
        local t = card:CreateTexture(nil, "ARTWORK")
        NoSnap(t)
        card.Tags[i] = t
    end
    local plate = card:CreateTexture(nil, "ARTWORK")
    plate:SetColorTexture(Rgb255(ns.Bridge.PLATE_RGB))   -- art-ok: protocol colour
    NoSnap(plate)
    card.Plate = plate
    return card
end

-- The card's row: the tag of slot 125, row 0, this session, then the plate.
local function LayoutCard(m)
    local card = dialog.Card
    card:SetSize(m.width, m.height)
    ns.Bridge.ApplyTag(card.Tags, ns.Bridge.PRESENCE_SLOT, 0, ns.sessionId % 2)
    for i, t in ipairs(card.Tags) do
        t:ClearAllPoints()
        t:SetPoint("TOPLEFT", card, "TOPLEFT", m.left + (i - 1) * m.W, -m.pad)
        t:SetSize(m.W, m.rowH)
    end
    card.Plate:ClearAllPoints()
    card.Plate:SetPoint("TOPLEFT", card, "TOPLEFT", m.left + 8 * m.W, -m.pad)
    card.Plate:SetSize(m.plateWidth, m.rowH)
end

-- VIEW entry 201 while the popup shows: the card's fill, physical px from the client's top-left.
function CompanionDialog.ViewEntry()
    if not (dialog and dialog:IsVisible() and dialog.metrics) then
        return nil
    end
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    local l, b, w, h = dialog.Card:GetScaledRect()
    if not (okSize and IsNumber(physH) and IsNumber(l) and IsNumber(b) and IsNumber(w) and IsNumber(h)) then
        return nil
    end
    local k = physH / 768
    local function Round(v)
        return floor(v + 0.5)
    end
    return { id = VIEW_ID, x = Round(l * k), y = Round(physH - (b + h) * k), w = Round(w * k), h = Round(h * k),
        fontPx = dialog.metrics.fontPx, rowPx = dialog.metrics.rowPx }
end

local function SendView()
    counters.views = counters.views + 1
    ns.Reserve.SyncView(true)
end

---------------------------------------------------------------------------- the popup
-- The download page, whole and highlighted (the box is read-only: any edit puts it back).
local function SelectUrl()
    local box = dialog.Box
    box:SetText(ns.COMPANION_URL)
    box:SetCursorPosition(0)
    box:HighlightText()
end

-- Lays the popup out from its top: title, text, the card and its caption, the download page when
-- asked for, the buttons. The top stays where it is, so the card never moves when the popup grows.
local function Layout()
    local m = CompanionDialog.CardMetrics()
    dialog.metrics = m
    local y = 44
    dialog.Body:SetText(state == "installed" and L.COMPANION_TEXT_INSTALLED or L.COMPANION_TEXT_ABSENT)
    y = y + dialog.Body:GetStringHeight() + 12
    dialog.Card:ClearAllPoints()
    dialog.Card:SetPoint("TOPLEFT", dialog, "TOPLEFT", PAD, -Snap(y))
    LayoutCard(m)
    y = y + m.height + 4
    dialog.Caption:ClearAllPoints()
    dialog.Caption:SetPoint("TOPLEFT", dialog, "TOPLEFT", PAD, -y)
    y = y + dialog.Caption:GetStringHeight() + 12
    local downloading = dialog.downloading and state ~= "installed"
    dialog.Box:SetShown(downloading)
    dialog.CopyHint:SetShown(downloading)
    if downloading then
        dialog.Box:ClearAllPoints()
        dialog.Box:SetPoint("TOPLEFT", dialog, "TOPLEFT", PAD + 6, -y)
        y = y + BOX_H + 6
        dialog.CopyHint:ClearAllPoints()
        dialog.CopyHint:SetPoint("TOPLEFT", dialog, "TOPLEFT", PAD, -y)
        y = y + dialog.CopyHint:GetStringHeight() + 12
    end
    -- the buttons, centred: [Continue] [Download] [Cancel], without [Download] when installed
    local buttons = { dialog.ContinueButton }
    dialog.DownloadButton:SetShown(state ~= "installed")
    if state ~= "installed" then
        buttons[#buttons + 1] = dialog.DownloadButton
    end
    buttons[#buttons + 1] = dialog.CancelButton
    local total = #buttons * BUTTON_W + (#buttons - 1) * BUTTON_GAP
    for i, b in ipairs(buttons) do
        b:ClearAllPoints()
        b:SetPoint("TOPLEFT", dialog, "TOP", -total / 2 + (i - 1) * (BUTTON_W + BUTTON_GAP), -y)
    end
    y = y + BUTTON_H + 16
    dialog:SetHeight(ceil(y))
end

-- The popup itself is closed: what its own hide undoes (also called by Close when it is closed while
-- hidden with its parent, as no OnHide fires then).
local function Finish(self)
    self.parentHidden = false
    PlaySound(SOUNDKIT.IG_MAINMENU_CLOSE)
    self:UnregisterEvent("PLAYER_REGEN_DISABLED")
    self:UnregisterEvent("UI_SCALE_CHANGED")
    self:UnregisterEvent("DISPLAY_SIZE_CHANGED")
    self.downloading = false
    ns.Popup.ReleaseWindowEscape(FRAME_NAME)
    if not continuing then
        ns.Presence.Drop()      -- Cancel, Escape: the click is dropped
    end
    ns.Reserve.SyncView(true)   -- the card's VIEW goes
end

local function Close()
    if dialog and dialog:IsShown() then
        dialog:Hide()
        if dialog.parentHidden then
            Finish(dialog)      -- closed while hidden with its parent: no OnHide fired
        end
    end
end

local function Continue()
    continuing = true
    Close()
    continuing = false
    ns.Presence.Continue()
end

local function ShowDownload()
    if state == "installed" then
        return
    end
    dialog.downloading = true
    Layout()
    SelectUrl()
    if not InCombatLockdown() then
        dialog.Box:SetFocus()
    end
    SendView()      -- the popup grew under the card: same card, a fresh VIEW anyway
end
CompanionDialog.ShowDownload = function()
    if dialog and dialog:IsShown() then
        ShowDownload()
    end
end

local function Button(text, onClick)
    local b = CreateFrame("Button", nil, dialog, "UIPanelButtonTemplate")
    b:SetSize(BUTTON_W, BUTTON_H)
    b:SetText(text)
    b:SetScript("OnClick", onClick)
    return b
end

local function Build()
    dialog = CreateFrame("Frame", FRAME_NAME, UIParent)
    dialog:SetSize(WIDTH, 200)
    dialog:SetFrameStrata("DIALOG")
    dialog:SetToplevel(true)
    dialog:SetClampedToScreen(true)
    dialog:EnableMouse(true)
    dialog:Hide()
    if HasTemplate("DialogBorderDarkTemplate") then
        dialog.Border = CreateFrame("Frame", nil, dialog, "DialogBorderDarkTemplate")
        dialog.Border:SetAllPoints(dialog)
    elseif HasTemplate("TooltipBackdropTemplate") then
        dialog.Border = CreateFrame("Frame", nil, dialog, "TooltipBackdropTemplate")
        dialog.Border:SetAllPoints(dialog)
    else
        local bg = dialog:CreateTexture(nil, "BACKGROUND")
        bg:SetColorTexture(0.05, 0.05, 0.05, 0.95)
        bg:SetAllPoints(dialog)
    end

    local title = dialog:CreateFontString(nil, "ARTWORK", "GameFontNormalLarge")
    title:SetPoint("TOP", dialog, "TOP", 0, -18)
    title:SetWidth(WIDTH - 2 * PAD)
    title:SetText(L.COMPANION_TITLE)
    dialog.Title = title

    local body = dialog:CreateFontString(nil, "ARTWORK", "GameFontHighlightSmall")
    body:SetPoint("TOPLEFT", dialog, "TOPLEFT", PAD, -44)
    body:SetWidth(WIDTH - 2 * PAD)
    body:SetJustifyH("LEFT")
    dialog.Body = body

    dialog.Card = BuildCard()

    local caption = dialog:CreateFontString(nil, "ARTWORK", "GameFontDisableSmall")
    caption:SetWidth(WIDTH - 2 * PAD)
    caption:SetJustifyH("LEFT")
    caption:SetText(L.COMPANION_CARD_CAPTION)
    dialog.Caption = caption

    -- InputBoxTemplate draws its left border 5 units out of the box: the box starts 6 units in
    local box = CreateFrame("EditBox", nil, dialog, "InputBoxTemplate")
    box:SetAutoFocus(false)
    box:SetSize(WIDTH - 2 * PAD - 6, BOX_H)
    box:SetScript("OnTextChanged", function(_, userInput)
        if userInput then
            SelectUrl()
        end
    end)
    box:SetScript("OnEscapePressed", Close)
    box:SetScript("OnEnterPressed", function(self)
        self:ClearFocus()
    end)
    box:Hide()
    dialog.Box = box

    local hint = dialog:CreateFontString(nil, "ARTWORK", "GameFontDisableSmall")
    hint:SetWidth(WIDTH - 2 * PAD)
    hint:SetJustifyH("LEFT")
    hint:SetText(L.COMPANION_COPY_HINT)
    hint:Hide()
    dialog.CopyHint = hint

    dialog.ContinueButton = Button(L.COMPANION_BTN_CONTINUE, Continue)
    dialog.DownloadButton = Button(L.COMPANION_BTN_DOWNLOAD, ShowDownload)
    dialog.CancelButton = Button(L.COMPANION_BTN_CANCEL, Close)

    if not tContains(UISpecialFrames, FRAME_NAME) then
        tinsert(UISpecialFrames, FRAME_NAME)
    end
    -- OnShow and OnHide also fire when an ancestor shows or hides (Alt+Z, a cinematic) while the
    -- popup itself stays shown (IsShown() true in OnHide): then only the card's VIEW and the ticker
    -- follow; the waiting click, the Escape hold and the events stay, and no sound plays
    -- (integration review: the click was dropped and Continue ran nothing once the UI came back).
    dialog:SetScript("OnShow", function(self)
        local back = self.parentHidden
        self.parentHidden = false
        if not back then
            PlaySound(SOUNDKIT.IG_MAINMENU_OPEN)
            self:RegisterEvent("PLAYER_REGEN_DISABLED")
            self:RegisterEvent("UI_SCALE_CHANGED")
            self:RegisterEvent("DISPLAY_SIZE_CHANGED")
            ns.Popup.HoldWindowEscape(FRAME_NAME)
        end
        if not ticker then
            ticker = C_Timer.NewTicker(VIEW_SECONDS, SendView)
        end
        if back then
            SendView()              -- back with the UI: the card's VIEW at once (Open sends its own)
        end
    end)
    dialog:SetScript("OnHide", function(self)
        self.Box:ClearFocus()
        if ticker then
            ticker:Cancel()
            ticker = nil
        end
        if self:IsShown() then
            self.parentHidden = true
            ns.Reserve.SyncView(true)   -- the card is not on screen: its VIEW goes until it comes back
            return
        end
        Finish(self)
    end)
    dialog:SetScript("OnEvent", function(self, event)
        if event == "PLAYER_REGEN_DISABLED" then
            -- combat: the keys go back to the game (Escape still closes the popup)
            self.Box:ClearFocus()
        else
            Layout()
            SendView()
        end
    end)
end

-- Shows the popup for a presence ("absent" or "installed"), centred on the screen, without the
-- keyboard focus, and sends the card's VIEW. Opened again while shown, it only comes to the front.
function CompanionDialog.Open(presence)
    if not dialog then
        Build()
    end
    counters.opened = counters.opened + 1
    if dialog:IsShown() then
        dialog:Raise()
        return
    end
    state = presence == "installed" and "installed" or "absent"
    dialog.downloading = false
    Layout()
    -- centred on the screen; anchored by its top, so a longer popup (the download page) grows down
    dialog:ClearAllPoints()
    dialog:SetPoint("TOP", UIParent, "CENTER", 0, Snap(floor(dialog:GetHeight() / 2 + 0.5)))
    dialog:Show()
    dialog:Raise()
    SendView()
end

function CompanionDialog.Close()
    Close()
end

function CompanionDialog.IsShown()
    return dialog ~= nil and dialog:IsShown()
end

-- The popup frame, for tests (nil before the first Open).
function CompanionDialog.Frame()
    return dialog
end

function CompanionDialog.Counters()
    return { opened = counters.opened, views = counters.views }
end
