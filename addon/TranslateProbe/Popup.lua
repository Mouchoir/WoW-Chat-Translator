-- A small popup menu of our own for the conversation window (tab options, the overflow list,
-- the language list, a bubble's options, the reply composer's other channels). Blizzard menus are
-- never used: rows of UIPanelButtonTemplate buttons, 24 high, on a solid dark background.
--
-- Popup.Open(owner, items, anchor): items are { text, color (a colour object or {r, g, b}),
-- disabled, tooltip, onClick(item), title (a gold header row, not clickable), separator,
-- checked (a check mark before the text) }; anchor is { point, relativeTo, relativePoint, x, y }
-- (default: below the owner). It closes on a click anywhere outside it (GLOBAL_MOUSE_DOWN), on
-- Escape, when its owner hides, and after a row is clicked. Popup.Close(); Popup.IsOpen(owner).
--
-- Escape: the popup is in UISpecialFrames; while it is shown the conversation window leaves that
-- list (and comes back a frame after the popup hides), so the first Escape closes the popup only.
-- No keyboard capture: EnableKeyboard is protected in combat. Other frames of ours that sit over
-- the window (the Translate Companion popup) hold the window out of the list the same way, with
-- Popup.HoldWindowEscape(key) on show and Popup.ReleaseWindowEscape(key) on hide.
local _, ns = ...

local Popup = {}
ns.Popup = Popup

local ROW_H = 24
local TITLE_H = 18
local SEP_H = 9
local PAD = 6
local MIN_W, MAX_W = 140, 320
local WINDOW_NAME = "TranslateProbeConvFrame"
local POPUP_NAME = "TranslateProbePopup"

local frame
local rows, titles, seps = {}, {}, {}
local currentOwner
local holders = {}          -- key -> true: frames shown that keep the window out of UISpecialFrames
local suspended = false     -- the window was taken out of UISpecialFrames by a holder
local readdPending = false  -- a re-add is scheduled for the next frame
local generation = 0        -- bumped on every hold: a re-add scheduled before it is dropped

local function AddSpecial(name)
    if not tContains(UISpecialFrames, name) then
        tinsert(UISpecialFrames, name)
    end
end

local function RemoveSpecial(name)
    for i = #UISpecialFrames, 1, -1 do
        if UISpecialFrames[i] == name then
            table.remove(UISpecialFrames, i)
            return true
        end
    end
    return false
end

-- A frame over the window shows (key: any value): the window leaves the Escape list while any
-- holder is shown, so the first Escape closes the holders only.
function Popup.HoldWindowEscape(key)
    holders[key] = true
    generation = generation + 1
    if readdPending then
        -- held again before the re-add of the previous holder ran: the window is still out of
        -- the list, keep it out (the pending re-add sees the new generation and does nothing)
        readdPending = false
        suspended = true
    elseif not suspended then
        suspended = RemoveSpecial(WINDOW_NAME)
    end
end

-- A holder hides: once none is left, the window comes back to the list one frame later (not
-- during CloseSpecialWindows' traversal of the list).
function Popup.ReleaseWindowEscape(key)
    holders[key] = nil
    if next(holders) ~= nil or not suspended then
        return
    end
    suspended = false
    readdPending = true
    local gen = generation
    C_Timer.After(0, function()
        if gen ~= generation or next(holders) ~= nil then
            return
        end
        readdPending = false
        if _G[WINDOW_NAME] then
            AddSpecial(WINDOW_NAME)
        end
    end)
end

local function Rgb(color)
    if type(color) == "table" then
        if color.GetRGB then
            return color:GetRGB()
        end
        return color[1], color[2], color[3]
    end
end

local function Build()
    frame = CreateFrame("Frame", POPUP_NAME, UIParent)
    frame:SetFrameStrata("DIALOG")
    frame:SetToplevel(true)
    frame:SetClampedToScreen(true)
    frame:EnableMouse(true)
    frame:Hide()
    local bg = frame:CreateTexture(nil, "BACKGROUND")
    bg:SetColorTexture(0.05, 0.04, 0.03, 0.97)
    bg:SetAllPoints(frame)
    frame.Bg = bg
    local border = CreateFrame("Frame", nil, frame)
    border:SetAllPoints(frame)
    if NineSliceUtil.GetLayout("TooltipDefaultDarkLayout") then
        NineSliceUtil.ApplyLayoutByName(border, "TooltipDefaultDarkLayout")
    end
    frame.Border = border
    frame:SetScript("OnEvent", function(self, event)
        if event == "GLOBAL_MOUSE_DOWN" and self:IsShown() and not self:IsMouseOver() then
            if not (currentOwner and currentOwner.IsMouseOver and currentOwner:IsMouseOver()) then
                Popup.Close()
            end
        end
    end)
    frame:SetScript("OnShow", function(self)
        self:RegisterEvent("GLOBAL_MOUSE_DOWN")
        Popup.HoldWindowEscape("popup")
    end)
    frame:SetScript("OnHide", function(self)
        self:UnregisterEvent("GLOBAL_MOUSE_DOWN")
        currentOwner = nil
        Popup.ReleaseWindowEscape("popup")
        if GameTooltip:GetOwner() and GameTooltip:GetOwner():GetParent() == self then
            GameTooltip:Hide()
        end
    end)
    AddSpecial(POPUP_NAME)
end

local function Row(i)
    local b = rows[i]
    if not b then
        b = CreateFrame("Button", nil, frame, "UIPanelButtonTemplate")
        b:SetHeight(ROW_H)
        b:SetMotionScriptsWhileDisabled(true)
        local check = b:CreateTexture(nil, "OVERLAY")
        check:SetAtlas("checkmark-minimal", true)
        check:SetPoint("LEFT", b, "LEFT", 6, 0)
        b.Check = check
        b:SetScript("OnClick", function(self)
            local item = self.item
            Popup.Close()
            PlaySound(SOUNDKIT.IG_MAINMENU_OPTION)
            if item and item.onClick then
                item.onClick(item)
            end
        end)
        b:SetScript("OnEnter", function(self)
            local item = self.item
            if item and item.tooltip then
                GameTooltip:SetOwner(self, "ANCHOR_RIGHT")
                GameTooltip_SetTitle(GameTooltip, item.text)
                GameTooltip_AddNormalLine(GameTooltip, item.tooltip)
                GameTooltip:Show()
            end
        end)
        b:SetScript("OnLeave", function()
            GameTooltip:Hide()
        end)
        rows[i] = b
    end
    return b
end

local function Title(i)
    local fs = titles[i]
    if not fs then
        fs = frame:CreateFontString(nil, "ARTWORK", "GameFontNormalSmall")
        fs:SetJustifyH("LEFT")
        titles[i] = fs
    end
    return fs
end

local function Sep(i)
    local t = seps[i]
    if not t then
        t = frame:CreateTexture(nil, "ARTWORK")
        t:SetColorTexture(0.6, 0.5, 0.3, 0.5)
        t:SetHeight(1)
        seps[i] = t
    end
    return t
end

local function Measure(text)
    local fs = Title(0)
    fs:SetFontObject(GameFontNormal)
    fs:SetWidth(0)
    fs:SetText(text or "")
    local w = fs:GetUnboundedStringWidth()
    fs:SetText("")
    fs:SetFontObject(GameFontNormalSmall)
    return w
end

-- Opens the popup for owner with items (see the top of this file). Opening it again for the same
-- owner closes it (a second click on the button that opened it).
function Popup.Open(owner, items, anchor)
    if not frame then
        Build()
    end
    if frame:IsShown() and currentOwner == owner then
        Popup.Close()
        return
    end
    Popup.Close()
    currentOwner = owner
    local width = MIN_W
    for _, item in ipairs(items) do
        if item.text and not item.separator then
            width = math.max(width, Measure(item.text) + (item.checked ~= nil and 44 or 28))
        end
    end
    width = math.min(MAX_W, width)
    local y, nRow, nTitle, nSep = -PAD, 0, 0, 0
    for _, b in ipairs(rows) do
        b:Hide()
    end
    for i, fs in ipairs(titles) do
        if i > 0 then
            fs:Hide()
        end
    end
    for _, t in ipairs(seps) do
        t:Hide()
    end
    for _, item in ipairs(items) do
        if item.separator then
            nSep = nSep + 1
            local t = Sep(nSep)
            t:ClearAllPoints()
            t:SetPoint("TOPLEFT", frame, "TOPLEFT", PAD + 4, y - 4)
            t:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -PAD - 4, y - 4)
            t:Show()
            y = y - SEP_H
        elseif item.title then
            nTitle = nTitle + 1
            local fs = Title(nTitle)
            fs:ClearAllPoints()
            fs:SetPoint("TOPLEFT", frame, "TOPLEFT", PAD + 4, y - 3)
            fs:SetWidth(width - 2 * PAD - 8)
            fs:SetText(item.text)
            fs:Show()
            y = y - TITLE_H
        else
            nRow = nRow + 1
            local b = Row(nRow)
            b.item = item
            b:ClearAllPoints()
            b:SetPoint("TOPLEFT", frame, "TOPLEFT", PAD, y)
            b:SetWidth(width - 2 * PAD)
            b:SetText(item.text or "")
            local fs = b:GetFontString()
            if fs then
                fs:ClearAllPoints()
                fs:SetPoint("LEFT", b, "LEFT", item.checked ~= nil and 26 or 10, 0)
                fs:SetPoint("RIGHT", b, "RIGHT", -8, 0)
                fs:SetJustifyH("LEFT")
                local r, g, bl = Rgb(item.color)
                if r then
                    fs:SetTextColor(r, g, bl)
                elseif item.disabled then
                    fs:SetTextColor(DISABLED_FONT_COLOR:GetRGB())
                else
                    fs:SetTextColor(NORMAL_FONT_COLOR:GetRGB())
                end
            end
            b.Check:SetShown(item.checked == true)
            b:SetEnabled(not item.disabled)
            b:Show()
            y = y - ROW_H
        end
    end
    frame:SetSize(width, -y + PAD)
    frame:ClearAllPoints()
    if anchor then
        frame:SetPoint(anchor[1] or "TOPLEFT", anchor[2] or owner, anchor[3] or "BOTTOMLEFT", anchor[4] or 0, anchor[5] or 0)
    else
        frame:SetPoint("TOPLEFT", owner, "BOTTOMLEFT", 0, -2)
    end
    frame:Show()
    frame:Raise()
    return frame
end

function Popup.Close()
    if frame and frame:IsShown() then
        frame:Hide()
    end
end

-- True when the popup is shown (for owner, when given).
function Popup.IsOpen(owner)
    return frame ~= nil and frame:IsShown() and (owner == nil or currentOwner == owner)
end

-- The shown rows, for tests: { { text, enabled, checked, button } } in order.
function Popup.Rows()
    local out = {}
    if not (frame and frame:IsShown()) then
        return out
    end
    for _, b in ipairs(rows) do
        if b:IsShown() then
            out[#out + 1] = { text = b.item and b.item.text, enabled = b:IsEnabled(), checked = b.Check:IsShown(),
                button = b }
        end
    end
    return out
end

-- Closes the popup when its owner (or an ancestor) hides: the owner calls this from OnHide.
function Popup.OwnerHidden(owner)
    if frame and frame:IsShown() and currentOwner then
        local r = currentOwner
        while r do
            if r == owner then
                Popup.Close()
                return
            end
            r = r.GetParent and r:GetParent()
        end
    end
end

function Popup.Frame()
    return frame
end
