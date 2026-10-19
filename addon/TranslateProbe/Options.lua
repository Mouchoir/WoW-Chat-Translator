-- Options panel in the game settings (Esc > Options > AddOns > TranslateProbe), built with the
-- 12.x Settings API (Blizzard_Settings_Shared\Blizzard_Settings.lua):
-- * the main page (vertical layout): target language, glossary row (off by default, user
--   decision D11), romanized names and the bridge mode; then the "Conversation window" section:
--   what [T] opens, following new messages, the clipboard consent, originals, text size,
--   window scale, focus, whispers, combat, and a button that resets the window position;
-- * a "Glossary" subpage (canvas layout): every abbreviation with its meaning for the current
--   target, editable; user entries can be deleted, built-in ones disabled; new entries added.
-- Every setting is a proxy on TranslateProbeDB, so the /tpr commands and the panel always agree
-- (the commands call the same setters). When the Settings API is missing, nothing is
-- registered and /tpr prints the command list instead.
-- The new section uses no native dropdown: SkillUp records a Forever crash in it
-- (skillup-forever UI\Settings.lua). Settings with a few values are discrete stock sliders with
-- labels, as SkillUp does (G17 checks the round-3 dropdowns).
-- luacheck: read globals Settings MinimalSliderWithSteppersMixin CreateSettingsButtonInitializer
-- luacheck: read globals CreateSettingsListSectionHeaderInitializer geterrorhandler
local addonName, ns = ...
local L = ns.L

local Options = {}
ns.Options = Options

local MODES = { "auto", "dense", "robust", "mono" }
local MODE_LABELS = { auto = "OPT_MODE_AUTO", dense = "OPT_MODE_DENSE", robust = "OPT_MODE_ROBUST", mono = "OPT_MODE_MONO" }
local ROWS = 12            -- glossary rows shown at once (mouse wheel and buttons scroll)
local ROW_HEIGHT = 26

-- Labels of the choice sliders, in slider order (Core.lua's ns.CHOICES holds the valid values).
local FOLLOW_ORDER = { "off", "add", "smart", "translate" }
local FOLLOW_LABELS = { off = "RPL_OPT_FOLLOW_OFF", add = "RPL_OPT_FOLLOW_ADD", smart = "RPL_OPT_FOLLOW_SMART",
    translate = "RPL_OPT_FOLLOW_TRANSLATE" }
local CLIP_ORDER = { "ask", "allow", "never" }
local CLIP_LABELS = { ask = "RPL_OPT_CLIP_ASK", allow = "RPL_OPT_CLIP_ALLOW", never = "RPL_OPT_CLIP_NEVER" }
-- Settings the conversation window redraws for.
local WINDOW_KEYS = { convText = true, convScale = true, convOriginal = true }

local category              -- the main settings category once registered
local page                  -- glossary page: { frame, built, rows, offset, entries, ... }
local listeners = {}        -- functions called with the key of a changed setting

local function Var(key)
    return addonName .. "_" .. key
end

-- fn(key) runs after any setting changed, from the panel or a command (for the window).
function Options.OnChanged(fn)
    listeners[#listeners + 1] = fn
end

-- A setting changed: the composer and the conversation window follow it.
local function Notify(key)
    for _, fn in ipairs(listeners) do
        local ok, err = pcall(fn, key)
        if not ok then
            geterrorhandler()(err)
        end
    end
    if WINDOW_KEYS[key] then
        if ns.Composer and ns.Composer.ApplySettings then
            ns.Composer.ApplySettings()
        end
        local F = ns.ConvFrame
        if F and type(F.ApplySettings) == "function" then
            pcall(F.ApplySettings, key)
        elseif F and type(F.Relayout) == "function" then
            pcall(F.Relayout)
            if type(F.SyncView) == "function" then
                pcall(F.SyncView, true)
            end
        end
    elseif key == "replyClipboard" or key == "target" then
        if ns.Composer and ns.Composer.Refresh then
            ns.Composer.Refresh()
        end
    end
end

---------------------------------------------------------------------------- setters
-- Shared by the panel and the commands. Invalid values are ignored.
function Options.SetTarget(code)
    if ns.Lang.IsTarget(code) and code ~= ns.db.target then
        ns.db.target = code
        Options.RefreshGlossary()
    end
end

function Options.SetMode(value)
    for _, mode in ipairs(MODES) do
        if mode == value then
            ns.db.mode = value
            ns.Bridge.SetMode(value)
        end
    end
end

-- A choice setting (display, follow, replyClipboard, convCombat). Invalid values are ignored;
-- returns true when the value is valid. Like SetTarget, it does not notify: the panel's own
-- setters notify the window, commands call Options.Changed.
function Options.SetChoice(key, value)
    if not ns.IsChoice(key, value) then
        return false
    end
    ns.db[key] = value
    return true
end

-- A numeric setting (convText, convScale), rounded and clamped. True when value is a number.
function Options.SetNumber(key, value)
    local v = ns.ClampSetting(key, value)
    if not v then
        return false
    end
    ns.db[key] = v
    return true
end

-- Forgets the saved window position; an open window goes back to the default place and size.
function Options.ResetConvPosition()
    ns.db.convPos = nil
    local F = ns.ConvFrame
    if F and type(F.ResetPosition) == "function" then
        pcall(F.ResetPosition)
    end
    Notify("convPos")
end

-- A command changed a value: the panel shows it at once.
function Options.Changed(key)
    if key == "target" or key == "glossary" then
        Options.RefreshGlossary()
    end
    if key ~= "glossary" and type(Settings) == "table" and type(Settings.NotifyUpdate) == "function" then
        pcall(Settings.NotifyUpdate, Var(key))
    end
    Notify(key)
end

---------------------------------------------------------------------------- glossary page
local function Label(key)
    return key:find("[\128-\255]") and key or key:upper()
end

local MARKS = {
    { field = "disabled", key = "GLOSS_MARK_DISABLED", r = 0.5, g = 0.5, b = 0.5 },
    { field = "user", key = "GLOSS_MARK_USER", r = 0.4, g = 0.8, b = 1 },
    { field = "edited", key = "GLOSS_MARK_EDITED", r = 1, g = 0.82, b = 0 },
    { field = "pivot", key = "GLOSS_MARK_PIVOT", r = 0.6, g = 0.6, b = 0.6 },
}

local function Mark(entry)
    for _, mark in ipairs(MARKS) do
        if entry[mark.field] then
            local text = L[mark.key]
            if entry.shared and not entry.disabled then
                text = text .. ", " .. L.GLOSS_MARK_SHARED
            end
            return text, mark.r, mark.g, mark.b
        end
    end
    return "", 1, 1, 1
end

local function SetStatus(text)
    if page and page.status then
        page.status:SetText(text or "")
    end
end

-- A row's meaning was edited: store it for the current target (an empty text removes the
-- meaning shown here; a built-in key goes back to its built-in meaning).
local function Commit(row)
    local entry = row.entry
    if not entry or entry.disabled then
        return
    end
    local text = row.edit:GetText() or ""
    if text == entry.meaning then
        return
    end
    ns.Glossary.SetMeaning(entry.key, ns.db.target, text)
    Options.RefreshGlossary()
end

-- Saves the meaning being typed in a row, if any: losing the focus commits it. Called before
-- anything rebinds the rows (a click on a button does not take the keyboard focus, so the box
-- would otherwise keep it while showing another entry, and lose the typed text).
local function CommitFocused()
    for _, row in ipairs(page.rows) do
        if row.edit:HasFocus() then
            row.edit:ClearFocus()
        end
    end
end

local function RowButton(row)
    local entry = row.entry
    if not entry then
        return
    end
    local key, disabled = entry.key, entry.disabled
    CommitFocused()
    if disabled then
        ns.Glossary.Enable(key)
    else
        ns.Glossary.Remove(key)
    end
    Options.RefreshGlossary()
end

local function AddEntry()
    local typedKey, typedMeaning = page.addKey:GetText(), page.addMeaning:GetText()
    CommitFocused()
    local key = ns.Glossary.NormalizeKey(typedKey)
    if not key then
        SetStatus(L.GLOSS_BAD_KEY)
        return
    end
    local meaning = ns.Glossary.CleanMeaning(typedMeaning)
    if meaning == "" then
        SetStatus(L.GLOSS_NEED_MEANING)
        return
    end
    ns.Glossary.SetMeaning(key, ns.db.target, meaning)
    page.addKey:SetText("")
    page.addMeaning:SetText("")
    page.addKey:ClearFocus()
    page.addMeaning:ClearFocus()
    SetStatus("")
    -- scroll so the entry is in view
    for i, entry in ipairs(ns.Glossary.Entries(ns.db.target)) do
        if entry.key == key then
            page.offset = math.max(0, i - math.ceil(ROWS / 2))
        end
    end
    Options.RefreshGlossary()
end

local function Scroll(delta)
    -- an edit in progress is saved first, then the rows move
    CommitFocused()
    page.offset = page.offset + delta
    Options.RefreshGlossary()
end

local function NewEditBox(parent, width)
    local edit = CreateFrame("EditBox", nil, parent, "InputBoxTemplate")
    edit:SetAutoFocus(false)
    edit:SetSize(width, 20)
    return edit
end

local function NewButton(parent, width, text, onClick)
    local button = CreateFrame("Button", nil, parent, "UIPanelButtonTemplate")
    button:SetSize(width, 22)
    button:SetText(text)
    button:SetScript("OnClick", onClick)
    return button
end

local function NewText(parent, font)
    local text = parent:CreateFontString(nil, "ARTWORK", font)
    text:SetJustifyH("LEFT")
    return text
end

local function Build(frame)
    page.built = true
    page.offset = 0
    page.title = NewText(frame, "GameFontNormalLarge")
    page.title:SetPoint("TOPLEFT", 16, -16)
    page.hint = NewText(frame, "GameFontHighlightSmall")
    page.hint:SetPoint("TOPLEFT", 16, -42)
    page.hint:SetWidth(600)
    page.hint:SetText(L.GLOSS_PAGE_HINT)
    local colKey = NewText(frame, "GameFontNormalSmall")
    colKey:SetPoint("TOPLEFT", 16, -86)
    colKey:SetText(L.GLOSS_COL_ABBR)
    local colMeaning = NewText(frame, "GameFontNormalSmall")
    colMeaning:SetPoint("TOPLEFT", 226, -86)
    colMeaning:SetText(L.GLOSS_COL_MEANING)

    page.rows = {}
    for i = 1, ROWS do
        local row = CreateFrame("Frame", nil, frame)
        row:SetSize(620, ROW_HEIGHT)
        row:SetPoint("TOPLEFT", 16, -100 - (i - 1) * ROW_HEIGHT)
        row.key = NewText(row, "GameFontHighlight")
        row.key:SetPoint("LEFT", 0, 0)
        row.key:SetWidth(110)
        row.mark = NewText(row, "GameFontHighlightSmall")
        row.mark:SetPoint("LEFT", 112, 0)
        row.mark:SetWidth(96)
        row.edit = NewEditBox(row, 290)
        row.edit:SetPoint("LEFT", 216, 0)
        row.edit:SetScript("OnEnterPressed", function(self)
            Commit(row)
            self:ClearFocus()
        end)
        row.edit:SetScript("OnEscapePressed", function(self)
            self:SetText(row.entry and row.entry.meaning or "")
            self:ClearFocus()
        end)
        -- hooked, not set: InputBoxTemplate's own OnEditFocusLost clears the text highlight
        row.edit:HookScript("OnEditFocusLost", function()
            Commit(row)
        end)
        row.button = NewButton(row, 100, L.GLOSS_BTN_DELETE, function()
            RowButton(row)
        end)
        row.button:SetPoint("LEFT", 516, 0)
        page.rows[i] = row
    end

    local bottom = -100 - ROWS * ROW_HEIGHT - 6
    page.prev = NewButton(frame, 100, L.GLOSS_PREV, function()
        Scroll(-ROWS)
    end)
    page.prev:SetPoint("TOPLEFT", 16, bottom)
    page.range = NewText(frame, "GameFontHighlightSmall")
    page.range:SetPoint("TOPLEFT", 126, bottom - 5)
    page.next = NewButton(frame, 100, L.GLOSS_NEXT, function()
        Scroll(ROWS)
    end)
    page.next:SetPoint("TOPLEFT", 232, bottom)
    frame:EnableMouseWheel(true)
    frame:SetScript("OnMouseWheel", function(_, delta)
        Scroll(-3 * delta)
    end)

    local addLabel = NewText(frame, "GameFontNormal")
    addLabel:SetPoint("TOPLEFT", 16, bottom - 36)
    addLabel:SetText(L.GLOSS_ADD_LABEL)
    page.addKey = NewEditBox(frame, 120)
    page.addKey:SetPoint("TOPLEFT", 22, bottom - 56)
    page.addMeaning = NewEditBox(frame, 290)
    page.addMeaning:SetPoint("TOPLEFT", 222, bottom - 56)
    page.addKey:SetScript("OnEnterPressed", function()
        page.addMeaning:SetFocus()
    end)
    page.addMeaning:SetScript("OnEnterPressed", AddEntry)
    for _, edit in ipairs({ page.addKey, page.addMeaning }) do
        edit:SetScript("OnEscapePressed", function(self)
            self:ClearFocus()
        end)
    end
    page.addButton = NewButton(frame, 100, L.GLOSS_BTN_ADD, AddEntry)
    page.addButton:SetPoint("TOPLEFT", 532, bottom - 55)
    page.status = NewText(frame, "GameFontHighlightSmall")
    page.status:SetPoint("TOPLEFT", 16, bottom - 82)
    page.status:SetTextColor(1, 0.4, 0.4)
end

-- Rebuilds the glossary rows for the current target (the page is built the first time the
-- settings panel shows it).
function Options.RefreshGlossary()
    if not (page and page.built) then
        return
    end
    local target = ns.db.target
    page.entries = ns.Glossary.Entries(target)
    local total = #page.entries
    page.offset = math.max(0, math.min(page.offset, total - ROWS))
    page.title:SetText(L.GLOSS_PAGE_TITLE:format(ns.Lang.NATIVE[target] or target))
    for i, row in ipairs(page.rows) do
        local entry = page.entries[page.offset + i]
        row.entry = entry
        if entry then
            row.key:SetText(Label(entry.key))
            local mark, r, g, b = Mark(entry)
            row.mark:SetText(mark)
            row.mark:SetTextColor(r, g, b)
            row.edit:SetText(entry.meaning)
            if entry.disabled then
                row.edit:Disable()
                row.button:SetText(L.GLOSS_BTN_ENABLE)
            else
                row.edit:Enable()
                row.button:SetText(entry.builtin and L.GLOSS_BTN_DISABLE or L.GLOSS_BTN_DELETE)
            end
            row:Show()
        else
            row:Hide()
        end
    end
    local first, last = math.min(total, page.offset + 1), math.min(total, page.offset + ROWS)
    page.range:SetText(L.GLOSS_RANGE:format(first, last, total))
    if page.offset > 0 then
        page.prev:Enable()
    else
        page.prev:Disable()
    end
    if page.offset + ROWS < total then
        page.next:Enable()
    else
        page.next:Disable()
    end
end

-- Widgets of the glossary page, for tests (nil before the panel first showed it).
function Options.Page()
    return page
end

---------------------------------------------------------------------------- conversation window section
local function HasSliders()
    return type(Settings.CreateSlider) == "function" and type(Settings.CreateSliderOptions) == "function"
        and type(MinimalSliderWithSteppersMixin) == "table" and type(MinimalSliderWithSteppersMixin.Label) == "table"
end

local function AddInitializer(cat, init)
    if type(Settings.RegisterInitializer) == "function" and init then
        Settings.RegisterInitializer(cat, init)
    end
end

local function IndexOf(list, value)
    for i, v in ipairs(list) do
        if v == value then
            return i
        end
    end
end

-- The "Conversation window" section of the main page. Checkboxes are proxy settings on
-- TranslateProbeDB; settings with a few values are discrete sliders with a label on the right
-- (no native dropdown); the reset button and the section header are added when the client has
-- their initializers (SkillUp and Target In Combat use them on Forever).
function Options.RegisterConversation(cat, types)
    local BOOLEAN, NUMBER = types.BOOLEAN, types.NUMBER
    if type(CreateSettingsListSectionHeaderInitializer) == "function" then
        AddInitializer(cat, CreateSettingsListSectionHeaderInitializer(L.RPL_OPT_SECTION))
    end

    local function Checkbox(key, name, tooltip, default, get, set)
        local setting = Settings.RegisterProxySetting(cat, Var(key), BOOLEAN, name, default, get, function(value)
            set(value == true)
            Notify(key)
        end)
        Settings.CreateCheckbox(cat, setting, tooltip)
    end
    local function Flag(key, name, tooltip)
        Checkbox(key, name, tooltip, false, function()
            return ns.db[key] == true
        end, function(value)
            ns.db[key] = value
        end)
    end
    -- order: values in slider order; labels: value -> L key
    local function Choice(key, name, tooltip, order, labels)
        if not HasSliders() then
            return
        end
        local default = IndexOf(order, ns.CHOICES[key][1])
        local setting = Settings.RegisterProxySetting(cat, Var(key), NUMBER, name, default, function()
            return IndexOf(order, ns.db[key]) or default
        end, function(value)
            local v = order[value]
            if v and Options.SetChoice(key, v) then
                Notify(key)
            end
        end)
        local options = Settings.CreateSliderOptions(1, #order, 1)
        options:SetLabelFormatter(MinimalSliderWithSteppersMixin.Label.Right, function(value)
            local v = order[value]
            return v and L[labels[v]] or ""
        end)
        Settings.CreateSlider(cat, setting, options, tooltip)
    end
    local function Number(key, name, tooltip, step, format)
        if not HasSliders() then
            return
        end
        local r = ns.RANGES[key]
        local setting = Settings.RegisterProxySetting(cat, Var(key), NUMBER, name, r[3], function()
            return ns.db[key]
        end, function(value)
            if Options.SetNumber(key, value) then
                Notify(key)
            end
        end)
        local options = Settings.CreateSliderOptions(r[1], r[2], step)
        options:SetLabelFormatter(MinimalSliderWithSteppersMixin.Label.Right, function(value)
            return format and format:format(value) or tostring(value)
        end)
        Settings.CreateSlider(cat, setting, options, tooltip)
    end

    Checkbox("display", L.RPL_OPT_DISPLAY, L.RPL_OPT_DISPLAY_TIP, true, function()
        return ns.db.display == "window"
    end, function(value)
        Options.SetChoice("display", value and "window" or "inline")
    end)
    Choice("follow", L.RPL_OPT_FOLLOW, L.RPL_OPT_FOLLOW_TIP, FOLLOW_ORDER, FOLLOW_LABELS)
    Choice("replyClipboard", L.RPL_OPT_CLIP, L.RPL_OPT_CLIP_TIP, CLIP_ORDER, CLIP_LABELS)
    Flag("convOriginal", L.RPL_OPT_ORIGINAL, L.RPL_OPT_ORIGINAL_TIP)
    Number("convText", L.RPL_OPT_TEXT, L.RPL_OPT_TEXT_TIP, 1)
    Number("convScale", L.RPL_OPT_SCALE, L.RPL_OPT_SCALE_TIP, 5, L.RPL_PERCENT)
    Flag("convFocus", L.RPL_OPT_FOCUS, L.RPL_OPT_FOCUS_TIP)
    Flag("convOpenWhisper", L.RPL_OPT_WHISPER, L.RPL_OPT_WHISPER_TIP)
    Checkbox("convCombat", L.RPL_OPT_COMBAT, L.RPL_OPT_COMBAT_TIP, false, function()
        return ns.db.convCombat == "hide"
    end, function(value)
        Options.SetChoice("convCombat", value and "hide" or "keep")
    end)
    if type(CreateSettingsButtonInitializer) == "function" then
        local reset = type(HUD_EDIT_MODE_RESET_POSITION) == "string" and HUD_EDIT_MODE_RESET_POSITION
            or L.RPL_OPT_RESET
        AddInitializer(cat, CreateSettingsButtonInitializer(L.RPL_OPT_RESET, reset, Options.ResetConvPosition,
            L.RPL_OPT_RESET_TIP, true))
    end
end

---------------------------------------------------------------------------- registration
local function HasSettingsAPI()
    if type(Settings) ~= "table" then
        return false
    end
    for _, name in ipairs({ "RegisterVerticalLayoutCategory", "RegisterAddOnCategory", "RegisterProxySetting",
        "CreateDropdown", "CreateCheckbox", "CreateControlTextContainer" }) do
        if type(Settings[name]) ~= "function" then
            return false
        end
    end
    return true
end

local function Register()
    if not HasSettingsAPI() then
        return
    end
    local cat = Settings.RegisterVerticalLayoutCategory(addonName)
    local varType = type(Settings.VarType) == "table" and Settings.VarType or {}
    local STRING, BOOLEAN = varType.String or "string", varType.Boolean or "boolean"
    local NUMBER = varType.Number or "number"

    local function Dropdown(key, name, tooltip, default, fill, set)
        local setting = Settings.RegisterProxySetting(cat, Var(key), STRING, name, default, function()
            return ns.db[key]
        end, set)
        Settings.CreateDropdown(cat, setting, function()
            local container = Settings.CreateControlTextContainer()
            fill(container)
            return container:GetData()
        end, tooltip)
    end
    local function Checkbox(key, name, tooltip, default)
        local setting = Settings.RegisterProxySetting(cat, Var(key), BOOLEAN, name, default, function()
            return ns.db[key] == true
        end, function(value)
            ns.db[key] = value == true
        end)
        Settings.CreateCheckbox(cat, setting, tooltip)
    end

    Dropdown("target", L.OPT_TARGET, L.OPT_TARGET_TIP, ns.Lang.DefaultTarget(), function(container)
        for _, code in ipairs(ns.Lang.TARGETS) do
            container:Add(code, ns.Lang.NATIVE[code] or code)
        end
    end, Options.SetTarget)
    Checkbox("glossShow", L.OPT_GLOSS_ROW, L.OPT_GLOSS_ROW_TIP, false)
    Checkbox("names", L.OPT_NAMES, L.OPT_NAMES_TIP, false)
    Dropdown("mode", L.OPT_MODE, L.OPT_MODE_TIP, "auto", function(container)
        for _, mode in ipairs(MODES) do
            container:Add(mode, L[MODE_LABELS[mode]])
        end
    end, Options.SetMode)
    Options.RegisterConversation(cat, { BOOLEAN = BOOLEAN, NUMBER = NUMBER })

    if type(Settings.RegisterCanvasLayoutSubcategory) == "function" then
        local frame = CreateFrame("Frame")
        frame:Hide()
        page = { frame = frame }
        -- called by the settings panel each time it shows the page (and when the panel opens)
        function frame:OnRefresh()
            if not page.built then
                Build(self)
            end
            Options.RefreshGlossary()
        end
        page.category = Settings.RegisterCanvasLayoutSubcategory(cat, frame, L.GLOSS_PAGE)
    end
    Settings.RegisterAddOnCategory(cat)
    category = cat
end

-- Opens the panel on the addon's page. False when there is no panel.
function Options.Open()
    if not category or type(Settings) ~= "table" or type(Settings.OpenToCategory) ~= "function" then
        return false
    end
    local id = type(category.GetID) == "function" and category:GetID() or category.ID
    return (pcall(Settings.OpenToCategory, id))
end

function Options.Category()
    return category
end

ns.OnLoad(Register)
