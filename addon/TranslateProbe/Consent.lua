-- R1: the in-game consent to copy translated replies to the Windows clipboard (never a Windows
-- dialog). Consent.Ask(onAllow, onDeny, anchor) calls onAllow (copy bit set) or onDeny (preview
-- only) at once when the saved choice is "allow" or "never", else shows the dialog:
--   title, the explanation (the clipboard is cleared when Translate is clicked, then replaced by
--   the translation; incoming messages are never copied; nothing leaves the PC), "Remember my
--   choice" and [Allow] [Deny].
-- There is no default button: Enter is left to the game and Escape means Deny, this time only. With
-- "Remember my choice" ticked, Allow saves replyClipboard = "allow" and Deny "never" (Options
-- and /tpr clip change it back). Hidden any other way (its window closed), nothing is called.
local _, ns = ...
local L = ns.L

local Consent = {}
ns.Consent = Consent

local WIDTH, HEIGHT = 380, 196
local dialog
local onAllow, onDeny

function Consent.Mode()
    local mode = ns.db and ns.db.replyClipboard
    if mode == "allow" or mode == "never" then
        return mode
    end
    return "ask"
end

local function HasTemplate(name)
    if not (C_XMLUtil and type(C_XMLUtil.GetTemplateInfo) == "function") then
        return false
    end
    local ok, info = pcall(C_XMLUtil.GetTemplateInfo, name)
    return ok and info ~= nil
end

local function Remember(mode)
    if dialog.remember:GetChecked() then
        ns.db.replyClipboard = mode
        if ns.Options and ns.Options.Changed then
            ns.Options.Changed("replyClipboard")
        end
    end
end

-- Closes the dialog and runs the chosen callback once. thisTimeOnly (Escape): the choice is not
-- remembered, even with the box ticked.
local function Decide(allow, thisTimeOnly)
    local fnAllow, fnDeny = onAllow, onDeny
    onAllow, onDeny = nil, nil
    if not thisTimeOnly then
        Remember(allow and "allow" or "never")
    end
    dialog.deciding = true
    dialog:Hide()
    dialog.deciding = false
    local fn = allow and fnAllow or fnDeny
    if fn then
        fn()
    end
end

local function Button(text, onClick)
    local b = CreateFrame("Button", nil, dialog, "UIPanelButtonTemplate")
    b:SetSize(110, 22)
    b:SetText(text)
    b:SetScript("OnClick", onClick)
    return b
end

local function Build()
    dialog = CreateFrame("Frame", "TranslateProbeConsent", UIParent)
    dialog:SetSize(WIDTH, HEIGHT)
    dialog:SetFrameStrata("DIALOG")
    dialog:SetToplevel(true)
    dialog:SetClampedToScreen(true)
    dialog:EnableMouse(true)
    dialog:Hide()
    -- art: the dark dialog border (G11 checks the template on Forever), else a tooltip border,
    -- else a flat dark fill
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
    title:SetWidth(WIDTH - 40)
    title:SetText(L.RPL_CONSENT_TITLE)
    dialog.title = title

    local body = dialog:CreateFontString(nil, "ARTWORK", "GameFontHighlightSmall")
    body:SetPoint("TOPLEFT", dialog, "TOPLEFT", 20, -44)
    body:SetWidth(WIDTH - 40)
    body:SetJustifyH("LEFT")
    body:SetText(L.RPL_CONSENT_BODY)
    dialog.body = body
    -- 380 x 196 at least; taller when the explanation needs more lines (frFR)
    dialog:SetHeight(math.max(HEIGHT, math.ceil(44 + body:GetStringHeight() + 8 + 84)))

    local remember = CreateFrame("CheckButton", nil, dialog, "UICheckButtonTemplate")
    remember:SetSize(24, 24)
    remember:SetPoint("BOTTOMLEFT", dialog, "BOTTOMLEFT", 18, 60)
    remember.Text:SetFontObject(GameFontNormal)
    remember.Text:SetText(L.RPL_CONSENT_REMEMBER)
    remember:SetScript("OnClick", function(self)
        PlaySound(self:GetChecked() and SOUNDKIT.IG_MAINMENU_OPTION_CHECKBOX_ON
            or SOUNDKIT.IG_MAINMENU_OPTION_CHECKBOX_OFF)
    end)
    dialog.remember = remember

    local hint = dialog:CreateFontString(nil, "ARTWORK", "GameFontDisableSmall")
    hint:SetPoint("TOPLEFT", remember, "BOTTOMLEFT", 26, 2)
    hint:SetText(L.RPL_CONSENT_HINT)
    dialog.hint = hint

    dialog.allow = Button(L.RPL_CONSENT_ALLOW, function()
        Decide(true)
    end)
    dialog.allow:SetPoint("BOTTOMRIGHT", dialog, "BOTTOM", -6, 16)
    dialog.deny = Button(L.RPL_CONSENT_DENY, function()
        Decide(false)
    end)
    dialog.deny:SetPoint("BOTTOMLEFT", dialog, "BOTTOM", 6, 16)

    -- Escape = Deny (this time). Every other key, Enter included, reaches the game (movement,
    -- abilities, the chat box): the dialog has no default button, so Enter decides nothing.
    -- Propagation is turned off only for that Escape, which hides the dialog in the same call,
    -- and never in combat (SetPropagateKeyboardInput is restricted there). So a shown dialog never
    -- keeps propagation off: a key swallowed before combat would otherwise eat every key in it.
    dialog:SetScript("OnKeyDown", function(self, key)
        if key ~= "ESCAPE" then
            return
        end
        if not InCombatLockdown() then
            self:SetPropagateKeyboardInput(false)
        end
        Decide(false, true)
    end)
    dialog:SetScript("OnShow", function(self)
        PlaySound(SOUNDKIT.IG_MAINMENU_OPEN)
        if not InCombatLockdown() then
            self:EnableKeyboard(true)
            self:SetPropagateKeyboardInput(true)
        end
    end)
    dialog:SetScript("OnHide", function(self)
        if not InCombatLockdown() then
            self:EnableKeyboard(false)
        end
        if not self.deciding then
            -- closed another way (its window was hidden): nothing is decided
            onAllow, onDeny = nil, nil
        end
    end)
end

-- onAllow / onDeny run once, at once or after the user's click. anchor (optional): the frame
-- the dialog is centred on (the conversation window), else the screen.
function Consent.Ask(allowFn, denyFn, anchor)
    local mode = Consent.Mode()
    if mode == "allow" then
        if allowFn then
            allowFn()
        end
        return
    elseif mode == "never" then
        if denyFn then
            denyFn()
        end
        return
    end
    if not dialog then
        Build()
    end
    onAllow, onDeny = allowFn, denyFn
    dialog.remember:SetChecked(false)
    dialog:ClearAllPoints()
    local relative = (anchor and anchor:IsShown()) and anchor or UIParent
    dialog:SetPoint("CENTER", relative, "CENTER", 0, 0)
    dialog:Show()
    dialog:Raise()
end

-- True while the dialog waits for a click.
function Consent.IsShown()
    return dialog ~= nil and dialog:IsShown()
end

-- The dialog frame, for tests (nil before the first Ask that needed it).
function Consent.Frame()
    return dialog
end

-- Closes the dialog without a decision (the conversation window was hidden).
function Consent.Cancel()
    if dialog and dialog:IsShown() then
        dialog:Hide()
    end
end
