-- The Translate Companion dialog (user request U1): opened by [Download] in a hint lane (under a
-- chat line, under a conversation bubble or under the reply card). The addon cannot know whether
-- Translate Companion runs, and WoW cannot open a web browser, so the dialog explains both cases
-- ("not running: start it", "not installed: download it") and shows the download page in a
-- read-only edit box, highlighted for Ctrl+C, with a Close button.
--
-- Same art as the consent dialog (Consent.lua): DialogBorderDarkTemplate, else a tooltip border,
-- else a flat dark fill; strata DIALOG, centred on the conversation window when it is shown.
-- Escape closes it: in the box (OnEscapePressed), or through UISpecialFrames once the box has lost
-- the focus; while it shows, the conversation window leaves that list (Popup.HoldWindowEscape), so
-- the first Escape closes only the dialog. The box takes the keyboard focus only out of combat,
-- and combat start takes it back, so movement and ability keys keep working.
local _, ns = ...
local L = ns.L

local CompanionDialog = {}
ns.CompanionDialog = CompanionDialog

local FRAME_NAME = "TranslateProbeCompanion"
local WIDTH, MIN_HEIGHT, PAD = 400, 190, 20
local BOX_H, BUTTON_W, BUTTON_H = 22, 110, 22
local dialog

local function HasTemplate(name)
    if not (C_XMLUtil and type(C_XMLUtil.GetTemplateInfo) == "function") then
        return false
    end
    local ok, info = pcall(C_XMLUtil.GetTemplateInfo, name)
    return ok and info ~= nil
end

-- The download page, whole and highlighted (the box is read-only: any edit puts it back).
local function SelectUrl()
    local box = dialog.Box
    box:SetText(ns.COMPANION_URL)
    box:SetCursorPosition(0)
    box:HighlightText()
end

local function Close()
    if dialog and dialog:IsShown() then
        dialog:Hide()
    end
end

local function Build()
    dialog = CreateFrame("Frame", FRAME_NAME, UIParent)
    dialog:SetSize(WIDTH, MIN_HEIGHT)
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
    body:SetText(L.COMPANION_NOT_RUNNING)
    dialog.Body = body

    local download = dialog:CreateFontString(nil, "ARTWORK", "GameFontHighlightSmall")
    download:SetPoint("TOPLEFT", body, "BOTTOMLEFT", 0, -10)
    download:SetWidth(WIDTH - 2 * PAD)
    download:SetJustifyH("LEFT")
    download:SetText(L.COMPANION_DOWNLOAD)
    dialog.Download = download

    -- InputBoxTemplate draws its left border 5 units out of the box: the box starts 6 units in
    local box = CreateFrame("EditBox", nil, dialog, "InputBoxTemplate")
    box:SetAutoFocus(false)
    box:SetSize(WIDTH - 2 * PAD - 6, BOX_H)
    box:SetPoint("TOPLEFT", download, "BOTTOMLEFT", 6, -6)
    box:SetScript("OnTextChanged", function(_, userInput)
        if userInput then
            SelectUrl()
        end
    end)
    box:SetScript("OnEscapePressed", Close)
    box:SetScript("OnEnterPressed", Close)
    dialog.Box = box

    local hint = dialog:CreateFontString(nil, "ARTWORK", "GameFontDisableSmall")
    hint:SetPoint("TOPLEFT", box, "BOTTOMLEFT", -6, -6)
    hint:SetWidth(WIDTH - 2 * PAD)
    hint:SetJustifyH("LEFT")
    hint:SetText(L.COMPANION_COPY_HINT)
    dialog.Hint = hint

    local close = CreateFrame("Button", nil, dialog, "UIPanelButtonTemplate")
    close:SetSize(BUTTON_W, BUTTON_H)
    close:SetPoint("BOTTOM", dialog, "BOTTOM", 0, 16)
    close:SetText(CLOSE or L.COMPANION_CLOSE)
    close:SetScript("OnClick", Close)
    dialog.CloseButton = close

    -- 400 x 190 at least; taller when the French text needs more lines
    local textH = body:GetStringHeight() + 10 + download:GetStringHeight() + 6 + BOX_H + 6 + hint:GetStringHeight()
    dialog:SetHeight(math.max(MIN_HEIGHT, math.ceil(44 + textH + 16 + BUTTON_H + 16)))

    if not tContains(UISpecialFrames, FRAME_NAME) then
        tinsert(UISpecialFrames, FRAME_NAME)
    end
    dialog:SetScript("OnShow", function(self)
        PlaySound(SOUNDKIT.IG_MAINMENU_OPEN)
        self:RegisterEvent("PLAYER_REGEN_DISABLED")
        ns.Popup.HoldWindowEscape(FRAME_NAME)
    end)
    dialog:SetScript("OnHide", function(self)
        PlaySound(SOUNDKIT.IG_MAINMENU_CLOSE)
        self:UnregisterEvent("PLAYER_REGEN_DISABLED")
        self.Box:ClearFocus()
        ns.Popup.ReleaseWindowEscape(FRAME_NAME)
    end)
    dialog:SetScript("OnEvent", function(self)
        -- combat: the keys go back to the game (Escape still closes the dialog)
        self.Box:ClearFocus()
    end)
end

-- Shows the dialog, centred on anchor (the conversation window) when it is shown, else on the
-- screen; again while shown, it only comes back to the front.
function CompanionDialog.Open(anchor)
    if not dialog then
        Build()
    end
    dialog:ClearAllPoints()
    local relative = (anchor and anchor.IsShown and anchor:IsShown()) and anchor or UIParent
    dialog:SetPoint("CENTER", relative, "CENTER", 0, 0)
    dialog:Show()
    dialog:Raise()
    if not InCombatLockdown() then
        dialog.Box:SetFocus()
    end
    SelectUrl()
end

function CompanionDialog.Close()
    Close()
end

function CompanionDialog.IsShown()
    return dialog ~= nil and dialog:IsShown()
end

-- The dialog frame, for tests (nil before the first Open).
function CompanionDialog.Frame()
    return dialog
end
