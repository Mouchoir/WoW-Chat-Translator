-- The conversation window (docs\ADDON.md, "Conversation window"): one tab per sender, a header
-- with the sender's language, a thread of bubbles the Helper paints, and the reply composer
-- (Composer.lua) in ConvFrame.composerHost at the bottom.
--
-- * A [T] click (ChatButton) calls OpenFromLine: the sender's conversation is created or
--   reopened, the line added (or found again and highlighted), the window shown and raised, the
--   tab selected and the bubble scrolled into view. The keyboard focus is never taken unless the
--   convFocus setting is on.
-- * Tabs: opening order, new ones on the right, never moved by a message; up to 10 shown, as many
--   as fit, the rest in "+N". A small close cross on the selected tab and on hover, middle-click
--   and the right-click popup close a tab; its conversation stays in memory (Conversations.lua).
-- * Slots: a bubble gets a window slot (100..124) when it comes within one page of the view;
--   bubbles of other tabs keep theirs until the budget needs them, the least recently seen first
--   (CANCEL, then the 2 s quarantine of Reserve). Only bubbles in view are ever requested, so
--   nothing is queued while the window is hidden.
-- * Per-row visibility: a row not entirely inside the visible thread has its tag and plate hidden.
-- * Compact bubbles (user request of 2026-10-19): the rows are estimated from the bubble's own text
--   width, link names counted as they are (the Companion copies them), with the window's own script
--   factors (Reserve.EstimateWindowRows); no hint lane under them since addon build 6.
-- * Every action that asks for a translation here (a bubble's Translate button, a click on a
--   bubble, "Translate again") and every way to show the window (the key bindings, /tpr conv) go
--   through Presence.Ensure, like a [T] click: no window without a confirmed Translate Companion.
-- * VIEW 200: the bubble column's fill, from the thread's top to the reply card's fill bottom.
-- * Links (user request L2, Links.lua): a message's hyperlinks are parsed once (msg.links, memory
--   only) for the links bar its bubble shows under its last row; the bar is inside the bubble
--   column, so VIEW 200 and the per-row visibility do not change.
-- Stock templates and fonts, named colours and ChatTypeInfo; no hooks on Blizzard frames, no
-- Blizzard menus, no animation. Everything is memory only; the saved variables get the window's
-- position (convPos) and convTipShown.
local _, ns = ...
local L = ns.L

local ConvFrame = {}
ns.ConvFrame = ConvFrame

local max, min, floor = math.max, math.min, math.floor

local FRAME_NAME = "TranslateProbeConvFrame"
local DEFAULT_W, DEFAULT_H = 440, 540
local MIN_W, MIN_H, MAX_W, MAX_H = 380, 440, 760, 900
local THREAD_TOP, THREAD_SIDE = 76, 8
local SCROLL_L, SCROLL_T, SCROLL_R, SCROLL_B = 6, 6, 22, 6
local TAB_LEFT, TAB_RIGHT, TAB_TOP, TAB_H = 66, 46, -26, 24
local MIN_TAB_W, MAX_TAB_W, TAB_GAP, TAB_PADDING = 72, 130, 3, 14
local MAX_SHOWN_TABS = 10
local WHEEL_ROWS = 3
local VIEW_ID = 200
local FLASH_SECONDS = 1.5
local BANNER_H = 14
local CLOSE_SIZE = 14
ConvFrame.VIEW_ID = VIEW_ID
ConvFrame.MAX_SHOWN_TABS = MAX_SHOWN_TABS

local Conv = ns.Conv

local frame                     -- TranslateProbeConvFrame, built on first use
local W = {}                    -- the window's widgets
local bubbles = {}              -- msg -> bubble of the selected conversation
local tops = {}                 -- msg -> top of its bubble in the thread content
local tabs = {}                 -- tab buttons (TabStrip.Tabs), bound to shown conversations
local shownCount = 0
local shownConv                 -- the conversation the thread shows
local metrics                   -- Reserve.WindowMetrics of the last layout
local composerH = 0
local newBelow = 0              -- messages that arrived below the view
local laying, layoutAgain = false, false
local retryQueued = false
local seenClock = 0
local counters = { opened = 0, relayouts = 0, slotsAsked = 0, slotsGivenBack = 0, flashes = 0 }

---------------------------------------------------------------------------- helpers
local function IsNumber(v)
    return not issecretvalue(v) and type(v) == "number" and v == v
end

local function Settings()
    return ns.db or {}
end

local function ColorCode(r, g, b)
    return ("|cff%02x%02x%02x"):format(floor(r * 255 + 0.5), floor(g * 255 + 0.5), floor(b * 255 + 0.5))
end

local CHAT_TYPE = {
    CHAT_MSG_SAY = "SAY", CHAT_MSG_YELL = "YELL", CHAT_MSG_EMOTE = "EMOTE", CHAT_MSG_WHISPER = "WHISPER",
    CHAT_MSG_BN_WHISPER = "BN_WHISPER", CHAT_MSG_PARTY = "PARTY", CHAT_MSG_PARTY_LEADER = "PARTY_LEADER",
    CHAT_MSG_RAID = "RAID", CHAT_MSG_RAID_LEADER = "RAID_LEADER", CHAT_MSG_RAID_WARNING = "RAID_WARNING",
    CHAT_MSG_INSTANCE_CHAT = "INSTANCE_CHAT", CHAT_MSG_INSTANCE_CHAT_LEADER = "INSTANCE_CHAT_LEADER",
    CHAT_MSG_GUILD = "GUILD", CHAT_MSG_OFFICER = "OFFICER", CHAT_MSG_CHANNEL = "CHANNEL",
    CHAT_MSG_COMMUNITIES_CHANNEL = "COMMUNITIES_CHANNEL",
}
local TYPE_LABEL = {
    SAY = "SAY", YELL = "YELL", EMOTE = "EMOTE", WHISPER = "WHISPER", BN_WHISPER = "WHISPER", PARTY = "PARTY",
    PARTY_LEADER = "PARTY", RAID = "RAID", RAID_LEADER = "RAID", RAID_WARNING = "RAID_WARNING",
    INSTANCE_CHAT = "INSTANCE_CHAT", INSTANCE_CHAT_LEADER = "INSTANCE_CHAT", GUILD = "GUILD", OFFICER = "OFFICER",
    COMMUNITIES_CHANNEL = "CHANNEL",
}

-- A channel's short label ("2. Trade" from index 2 and "Trade - City") and its chat colour.
function ConvFrame.ChannelLabel(chatType, channelIndex, channelBase)
    local label
    if chatType == "CHANNEL" then
        local base = type(channelBase) == "string" and channelBase:match("^(.-) %- ") or channelBase
        if IsNumber(channelIndex) and channelIndex > 0 then
            label = channelIndex .. ". " .. (base or "")
        else
            label = base or _G.CHANNEL
        end
    else
        local global = TYPE_LABEL[chatType]
        label = global and _G[global] or chatType or ""
    end
    local info = ChatTypeInfo[(chatType == "CHANNEL" and IsNumber(channelIndex)) and ("CHANNEL" .. channelIndex) or ""]
        or ChatTypeInfo[chatType or ""] or ChatTypeInfo.SAY
    local r, g, b = 1, 1, 1
    if info then
        r, g, b = info.r, info.g, info.b
    end
    return label, r, g, b
end

local function MsgChannel(msg)
    return ConvFrame.ChannelLabel(CHAT_TYPE[msg.event or ""], msg.channelIndex, msg.channelBase)
end

local function LastChannel(conv)
    local last = conv.last or {}
    return ConvFrame.ChannelLabel(CHAT_TYPE[last.event or ""], last.channelIndex, last.channelBase)
end

-- Display name of a conversation: the full name-realm, or a Battle.net account name (shown, never
-- parsed), or "?".
local function FullName(conv)
    if conv.name then
        return conv.name
    end
    if conv.bnID and C_BattleNet.GetAccountInfoByID then
        local ok, info = pcall(C_BattleNet.GetAccountInfoByID, conv.bnID)
        if ok and type(info) == "table" and info.accountName ~= nil then
            return info.accountName
        end
    end
    return "?"
end

local function ShortName(conv)
    if conv.name then
        return (Ambiguate(conv.name, "short"))
    end
    return FullName(conv)
end

---------------------------------------------------------------------------- metrics and view
local function ContentWidth()
    local w = W.scroll and W.scroll:GetWidth() or 0
    if not (IsNumber(w) and w > 0) then
        w = (frame and frame:GetWidth() or DEFAULT_W) - 2 * THREAD_SIDE - SCROLL_L - SCROLL_R
    end
    return w
end

-- The bubble geometry of the window (Reserve.WindowMetrics), for the composer's reply card too.
function ConvFrame.Metrics()
    if not frame then
        return nil
    end
    return ns.Reserve.WindowMetrics(frame, ContentWidth(), THREAD_SIDE + SCROLL_L)
end

function ConvFrame.IsShown()
    return frame ~= nil and frame:IsShown()
end

-- The thread's visible rows, top and bottom, in GetScaledRect units (the banner and the new
-- messages button take their part).
local function ClipRect()
    local l, b, w, h = W.scroll:GetScaledRect()
    if not (l and IsNumber(b) and IsNumber(h)) then
        return nil
    end
    local top, bottom = b + h, b
    if W.newPill:IsShown() then
        local _, pb, _, ph = W.newPill:GetScaledRect()
        if IsNumber(pb) and IsNumber(ph) then
            bottom = max(bottom, pb + ph)
        end
    end
    return top, bottom, l, w
end

-- VIEW entry 200 while the window shows a conversation: the bubble column's fill from the
-- thread's top to the reply card's fill bottom (the thread's bottom without a card), physical px.
function ConvFrame.ViewEntry()
    if not (frame and frame:IsVisible() and shownConv) then
        return nil
    end
    local m = metrics or ConvFrame.Metrics()
    if not m then
        return nil
    end
    local okSize, _, physH = pcall(GetPhysicalScreenSize)
    local l, b, _, h = W.scroll:GetScaledRect()
    if not (okSize and IsNumber(physH) and IsNumber(l) and IsNumber(b) and IsNumber(h)) then
        return nil
    end
    local eff = frame:GetEffectiveScale()
    local x0 = l + (m.margin + m.border) * eff
    local width = m.fillWidth * eff
    local top, bottom = b + h, b
    local composer = ns.Composer
    if composer and composer.ReplyCardRect then
        local ok, _, cb = pcall(composer.ReplyCardRect)
        if ok and IsNumber(cb) and cb < bottom then
            bottom = cb
        end
    end
    local k = physH / 768
    return { id = VIEW_ID, x = Round(x0 * k), y = Round(physH - top * k), w = Round(width * k),
        h = Round((top - bottom) * k), fontPx = m.fontPx, rowPx = m.rowPx }
end

function ConvFrame.SyncView(force)
    return ns.Reserve.SyncView(force)
end

-- True when the shown window overlaps a chat frame's text area.
function ConvFrame.Covers(chatFrame)
    if not (frame and frame:IsVisible()) then
        return false
    end
    local area = chatFrame.FontStringContainer or chatFrame
    local cl, cb, cw, ch = area:GetScaledRect()
    local fl, fb, fw, fh = frame:GetScaledRect()
    for _, v in ipairs({ cl, cb, cw, ch, fl, fb, fw, fh }) do
        if not IsNumber(v) then
            return false
        end
    end
    return cl < fl + fw and fl < cl + cw and cb < fb + fh and fb < cb + ch
end

---------------------------------------------------------------------------- slots and rows
local function SessBit()
    return ns.sessionId % 2
end

local function ApplyRows(b, msg)
    if b.kind ~= "in" or msg.state ~= "translate" then
        return
    end
    local slot = msg.req and msg.req.slot or nil
    if b.slot ~= slot or b.K ~= msg.K then
        local resized = b.K ~= msg.K
        b:SetRows(slot, msg.K, SessBit())
        if resized then
            b.needsLayout = true
        else
            b:RefreshRows()
        end
    end
end

local function ClipAll()
    if not shownConv then
        return
    end
    local top, bottom = ClipRect()
    for msg, b in pairs(bubbles) do
        if b.kind == "in" and msg.state == "translate" then
            b:ClipRows(top, bottom)
        end
    end
end

-- The bubble with a slot that was seen least recently and is not in the near set.
local function LeastRecentlySeen(near)
    local victim
    for _, conv in ipairs(Conv.All()) do
        for _, msg in ipairs(conv.msgs) do
            if msg.req and not near[msg] and (not victim or (msg.seen or 0) < (victim.seen or 0)) then
                victim = msg
            end
        end
    end
    return victim
end

local UpdateSlots

local function QueueRetry()
    if retryQueued then
        return
    end
    retryQueued = true
    local wait = (ns.Reserve.QuarantineLeft() or 0) + 0.05
    C_Timer.After(wait, function()
        retryQueued = false
        UpdateSlots()
    end)
end

-- Gives a slot to every translated bubble within one page of the view (and sends its TRANSLATE,
-- a VIEW first), sends again a bubble whose rows changed, then shows the rows that have a slot.
function UpdateSlots()
    if not (frame and frame:IsShown() and shownConv) then
        return
    end
    local conv = shownConv
    local viewTop = W.scroll:GetVerticalScroll() or 0
    local viewH = W.scroll:GetHeight()
    local nearTop, nearBottom = viewTop - viewH, viewTop + 2 * viewH
    seenClock = seenClock + 1
    local near, need = {}, {}
    for _, msg in ipairs(conv.msgs) do
        local b = bubbles[msg]
        if msg.kind == "in" and msg.state == "translate" and b and tops[msg] then
            local top, h = tops[msg], b:GetHeight()
            if top + h >= nearTop and top <= nearBottom then
                near[msg] = true
                msg.seen = seenClock
                if not msg.req then
                    need[#need + 1] = msg
                elseif msg.req.K ~= msg.K or msg.noCache or msg.resend then
                    counters.slotsAsked = counters.slotsAsked + 1
                    ns.Request.StartWindow(conv, msg, msg.noCache)
                    msg.noCache, msg.resend = nil, nil
                end
            end
        end
    end
    local short = false
    for _, msg in ipairs(need) do
        counters.slotsAsked = counters.slotsAsked + 1
        if ns.Request.StartWindow(conv, msg, msg.noCache) then
            msg.noCache, msg.resend = nil, nil
        else
            short = true
            local victim = LeastRecentlySeen(near)
            if victim then
                counters.slotsGivenBack = counters.slotsGivenBack + 1
                ns.Request.DropWindow(victim)
            end
        end
    end
    if short then
        QueueRetry()
    end
    local relayout = false
    for msg, b in pairs(bubbles) do
        ApplyRows(b, msg)
        relayout = relayout or b.needsLayout
    end
    if relayout then
        ConvFrame.Relayout()
        return
    end
    ClipAll()
end

---------------------------------------------------------------------------- bubbles
local function BubbleTooltip(b)
    local msg = b.msg
    if not msg then
        return
    end
    GameTooltip:SetOwner(b.box, "ANCHOR_RIGHT")
    local label, r, g, bl = MsgChannel(msg)
    GameTooltip_SetTitle(GameTooltip, label .. "  " .. (msg.time or ""), CreateColor(r, g, bl))
    if msg.kind == "in" then
        GameTooltip_AddHighlightLine(GameTooltip, msg.text)
        if msg.state == "translate" then
            GameTooltip_AddInstructionLine(GameTooltip, L.CONV_BUBBLE_HINT_CLICK)
            GameTooltip_AddInstructionLine(GameTooltip, L.CONV_BUBBLE_HINT_SHIFT)
            GameTooltip_AddColoredLine(GameTooltip, L.CONV_HELPER_TIP, GRAY_FONT_COLOR)
        else
            GameTooltip_AddInstructionLine(GameTooltip, L.CONV_BUBBLE_HINT_PLAIN)
        end
    end
    GameTooltip:Show()
end

local copyFrame
local function CopyOriginal(text)
    if not copyFrame then
        copyFrame = CreateFrame("Frame", "TranslateProbeCopyFrame", UIParent, "TooltipBackdropTemplate")
        copyFrame:SetFrameStrata("DIALOG")
        copyFrame:SetSize(340, 64)
        copyFrame:SetClampedToScreen(true)
        copyFrame:EnableMouse(true)
        local hint = copyFrame:CreateFontString(nil, "ARTWORK")
        hint:SetFontObject(GameFontNormalSmall)
        hint:SetPoint("TOPLEFT", copyFrame, "TOPLEFT", 12, -10)
        hint:SetText(L.CONV_COPY_HINT)
        local box = CreateFrame("EditBox", nil, copyFrame, "InputBoxTemplate")
        box:SetAutoFocus(false)
        box:SetSize(312, 22)
        box:SetPoint("BOTTOMLEFT", copyFrame, "BOTTOMLEFT", 16, 10)
        box:SetScript("OnEscapePressed", function()
            copyFrame:Hide()
        end)
        box:SetScript("OnEnterPressed", function()
            copyFrame:Hide()
        end)
        box:SetScript("OnTextChanged", function(self, userInput)
            if userInput then
                self:SetText(copyFrame.text or "")
                self:HighlightText()
            end
        end)
        copyFrame.Box = box
        copyFrame:SetScript("OnHide", function()
            copyFrame.text = nil
            box:SetText("")
            box:ClearFocus()
        end)
        tinsert(UISpecialFrames, "TranslateProbeCopyFrame")
    end
    copyFrame.text = text
    copyFrame:ClearAllPoints()
    copyFrame:SetPoint("CENTER", frame or UIParent, "CENTER", 0, 0)
    copyFrame:Show()
    copyFrame:Raise()
    copyFrame.Box:SetText(text)
    copyFrame.Box:SetFocus()
    copyFrame.Box:HighlightText()
end
ConvFrame.CopyOriginal = CopyOriginal

-- Every click asks the Helper again (a new reqId, served from the cache unless Shift): it is the
-- repair for "Not received - click the bubble" and for a text cut with "...", so it must work on
-- the second click too, not only when the row count changes.
local function Retranslate(conv, msg, noCache)
    msg.state = "translate"
    msg.K = ns.Reserve.MAX_ROWS     -- more rows when the Helper had to cut it (served from the cache)
    msg.userK = true
    msg.noCache = noCache or nil
    msg.resend = true
    ConvFrame.Relayout()
end

-- Retranslate once Translate Companion is confirmed (else its popup, and this runs on Continue).
local function AskRetranslate(conv, msg, noCache)
    ns.Presence.Ensure(function()
        Retranslate(conv, msg, noCache)
    end)
end

local function BubbleMenu(b, conv, msg)
    local items = {
        { text = L.CONV_MENU_RETRANSLATE, onClick = function()
            AskRetranslate(conv, msg, false)
        end },
        { text = L.CONV_MENU_COPY, onClick = function()
            CopyOriginal(msg.text)
        end },
        { text = L.CONV_MENU_REMOVE, onClick = function()
            Conv.RemoveMsg(conv, msg)
        end },
    }
    ns.Popup.Open(b.box, items, { "TOPLEFT", b.box, "TOPRIGHT", -40, 0 })
end

local function InHandlers(conv, msg)
    return {
        click = function(b, button)
            if button == "RightButton" then
                BubbleMenu(b, conv, msg)
            elseif msg.state == "translate" then
                AskRetranslate(conv, msg, IsShiftKeyDown() and true or false)
            end
        end,
        enter = BubbleTooltip,
        leave = GameTooltip_Hide,
        original = function()
            msg.showOriginal = not msg.showOriginal
            ConvFrame.Relayout()
        end,
        translate = function()
            ns.Presence.Ensure(function()
                msg.state = "translate"
                ConvFrame.Relayout()
            end)
        end,
    }
end

local OUT_STATE = {
    sending = { "CONV_OUT_SENDING", GRAY_FONT_COLOR }, sent = { "CONV_OUT_SENT", GREEN_FONT_COLOR },
    unconfirmed = { "CONV_OUT_UNCONFIRMED", NORMAL_FONT_COLOR }, offline = { "CONV_OUT_OFFLINE", RED_FONT_COLOR },
    blocked = { "CONV_OUT_BLOCKED", RED_FONT_COLOR },
}

-- The width of every link name of a message in the window font, as a reader sees it (each
-- occurrence: the Companion copies them as written, so they do not grow with the translation).
local function LinkNamesWidth(text)
    local width = 0
    ns.Links.Scan(text, function(kind, entry)
        if kind == "link" then
            width = width + ns.Bubble.TextWidth(entry.plain)
        end
    end)
    return width
end

-- The row estimate of an incoming bubble: its text in the window font against the plate width,
-- link names counted as they are, unless the user asked for more rows (a click).
local function EstimateK(msg, m)
    if msg.userK then
        return msg.K
    end
    if msg.kPlate ~= m.plateWidth or msg.kText ~= m.textSize or msg.kTarget ~= Settings().target then
        local visible = ns.Glossary.VisibleText(msg.text)
        msg.K = ns.Reserve.EstimateWindowRows(ns.Bubble.TextWidth(visible), m.plateWidth, visible, Settings().target,
            LinkNamesWidth(msg.text))
        msg.kPlate, msg.kText, msg.kTarget = m.plateWidth, m.textSize, Settings().target
    end
    return msg.K
end

local function ConfigureIn(b, conv, msg, m)
    local label, r, g, bl = MsgChannel(msg)
    b:SetMeta(ColorCode(r, g, bl) .. label .. "|r " .. (msg.time or ""))
    b:Tint(r, g, bl)
    b:SetHandlers(InHandlers(conv, msg))
    if msg.state == "translate" then
        b:SetPlain(nil)
        local shown = msg.showOriginal
        if shown == nil then
            shown = Settings().convOriginal == true
        end
        b:SetOriginal(msg.text, shown)
        EstimateK(msg, m)
        b:SetRows(msg.req and msg.req.slot or nil, msg.K, SessBit())
    else
        b:SetPlain(msg.text)
        b:SetOriginal(nil, false)
    end
    -- the links bar: the message's hyperlinks, parsed once (memory only, with the message)
    if msg.links == nil then
        msg.links = ns.Links.Parse(msg.text)
    end
    b:SetLinks(msg.links)
    local gloss = Settings().glossShow and msg.state == "translate" and ns.Glossary.Describe(msg.text, Settings().target)
    b:SetGloss(gloss or nil)
end

local function ConfigureOut(b, conv, msg)
    local chatType = msg.chatType or "WHISPER"
    local label, r, g, bl = ConvFrame.ChannelLabel(chatType, tonumber(msg.target), nil)
    if msg.label then
        label = msg.label
    end
    b:SetMeta(L.CONV_YOU_TO:format(ColorCode(r, g, bl) .. label .. "|r") .. " " .. (msg.time or ""))
    local state = OUT_STATE[msg.state]
    if state then
        local text = L[state[1]]
        if msg.state == "offline" then
            text = text:format(ShortName(conv))
        end
        b:SetMetaRight(text, state[2]:GetRGB())
    else
        b:SetMetaRight("")
    end
    b:Tint(r, g, bl)
    b:SetOut(msg.text, msg.draft)
    b:SetHandlers({ enter = BubbleTooltip, leave = GameTooltip_Hide })
end

local function ReleaseAll()
    for msg, b in pairs(bubbles) do
        ns.Bubble.Release(b)
        bubbles[msg] = nil
    end
    wipe(tops)
end

---------------------------------------------------------------------------- tabs
local function TabLabel(conv, dupNames)
    local name = ShortName(conv)
    if conv.name and dupNames[name] then
        name = Ambiguate(conv.name, "none")
    end
    if (conv.unread or 0) > 0 then
        name = name .. " |cffffffff(" .. conv.unread .. ")|r"
    end
    return name
end

local function SelectedIndex(list)
    local sel = Conv.Selected()
    for i, c in ipairs(list) do
        if c == sel then
            return i
        end
    end
end

-- How many tabs fit: the selected one at its widest, the others at their narrowest; at most 10.
local function FitCount(total)
    local avail = (frame:GetWidth() or DEFAULT_W) - TAB_LEFT - TAB_RIGHT
    local n = 1 + floor((avail - MAX_TAB_W) / (MIN_TAB_W + TAB_GAP))
    return max(1, min(MAX_SHOWN_TABS, total, n)), avail
end

local function ShowCross(tab, on)
    tab.Close:SetShown(on and tab.conv ~= nil)
end

local function TabTooltip(tab)
    local conv = tab.conv
    if not conv then
        return
    end
    GameTooltip:SetOwner(tab, "ANCHOR_BOTTOM")
    GameTooltip_SetTitle(GameTooltip, FullName(conv))
    if conv.guid then
        local ok, className = pcall(GetPlayerInfoByGUID, conv.guid)
        if ok and type(className) == "string" and className ~= "" then
            GameTooltip_AddHighlightLine(GameTooltip, className)
        end
    end
    local label = LastChannel(conv)
    if label and label ~= "" then
        GameTooltip_AddNormalLine(GameTooltip, L.CONV_TAB_CHANNEL:format(label))
    end
    GameTooltip_AddInstructionLine(GameTooltip, L.CONV_TAB_HINT)
    GameTooltip:Show()
end

local function TabMenu(tab)
    local conv = tab.conv
    if not conv then
        return
    end
    local following = Conv.EffectiveFollow(conv) == "translate"
    ns.Popup.Open(tab, {
        { text = L.CONV_TAB_CLOSE, onClick = function()
            Conv.Close(conv.key)
        end },
        { text = L.CONV_TAB_CLOSE_OTHERS, disabled = #Conv.List() < 2, onClick = function()
            Conv.CloseOthers(conv.key)
        end },
        { separator = true },
        { text = L.CONV_FOLLOW, checked = following, onClick = function()
            Conv.SetFollow(conv, not following)
        end },
    })
end

local function NewTab(i)
    local tab = CreateFrame("Button", nil, W.tabStrip, "PanelTopTabButtonTemplate")
    tab:SetID(i)
    tab:RegisterForClicks("LeftButtonUp")
    tab.selectedTextX, tab.deselectedTextX = -6, -6
    local close = CreateFrame("Button", nil, tab, "UIPanelCloseButton")
    close:SetSize(CLOSE_SIZE, CLOSE_SIZE)
    close:SetPoint("RIGHT", tab, "RIGHT", -4, -2)
    close:SetFrameLevel(tab:GetFrameLevel() + 2)
    close:Hide()
    close:SetScript("OnClick", function()
        if tab.conv then
            PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
            Conv.Close(tab.conv.key)
        end
    end)
    close:SetScript("OnEnter", function(self)
        GameTooltip:SetOwner(self, "ANCHOR_RIGHT")
        GameTooltip_SetTitle(GameTooltip, L.CONV_TAB_CLOSE)
        GameTooltip_AddNormalLine(GameTooltip, L.CONV_CLOSE_TIP)
        GameTooltip:Show()
    end)
    close:SetScript("OnLeave", function()
        GameTooltip:Hide()
        C_Timer.After(0, function()
            ShowCross(tab, tab.selected or tab:IsMouseOver())
        end)
    end)
    tab.Close = close
    tab:SetScript("OnClick", function(self)
        if self.conv then
            PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
            Conv.Select(self.conv.key)
        end
    end)
    -- the selected tab is disabled (PanelTemplates), so it never gets OnClick: middle and right
    -- clicks are read on mouse up, which every tab gets
    tab:SetScript("OnMouseUp", function(self, button)
        if not self.conv then
            return
        end
        if button == "MiddleButton" then
            PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
            Conv.Close(self.conv.key)
        elseif button == "RightButton" then
            TabMenu(self)
        end
    end)
    tab:SetScript("OnEnter", function(self)
        ShowCross(self, true)
        TabTooltip(self)
    end)
    tab:SetScript("OnLeave", function(self)
        GameTooltip:Hide()
        C_Timer.After(0, function()
            ShowCross(self, self.selected or self:IsMouseOver())
        end)
    end)
    tabs[i] = tab
    return tab
end

local function UpdateTabs()
    if not frame then
        return
    end
    local list = Conv.List()
    local n, avail = FitCount(#list)
    if #list == 0 then
        n = 0
    end
    -- the selected conversation is always shown: picked from beyond the strip, it takes the place
    -- of the rightmost shown tab
    local sel = SelectedIndex(list)
    if sel and sel > n then
        Conv.Swap(sel, n)
        list = Conv.List()
    end
    local dup, seen = {}, {}
    for i = 1, n do
        local s = ShortName(list[i])
        if seen[s] then
            dup[s] = true
        end
        seen[s] = true
    end
    for i = 1, MAX_SHOWN_TABS do
        local tab = tabs[i] or NewTab(i)
        local conv = list[i]
        if i <= n and conv then
            tab.conv = conv
            tab:SetText(TabLabel(conv, dup))
            if (conv.unread or 0) > 0 then
                tab:LockHighlight()
            else
                tab:UnlockHighlight()
            end
            tab:Show()
            PanelTemplates_TabResize(tab, TAB_PADDING, nil, MIN_TAB_W, MAX_TAB_W)
        else
            tab.conv = nil
            tab.selected = false
            tab:Hide()
            tab.Close:Hide()
        end
    end
    shownCount = n
    if n > 0 then
        PanelTemplates_SetNumTabs(W.tabStrip, n)
        local selIndex = SelectedIndex(list) or 1
        PanelTemplates_SetTab(W.tabStrip, selIndex)
        PanelTemplates_ResizeTabsToFit(W.tabStrip, avail - (n - 1) * TAB_GAP)
        for i = 1, n do
            local tab = tabs[i]
            tab.selected = i == selIndex
            ShowCross(tab, tab.selected or tab:IsMouseOver())
        end
    end
    local hidden = #list - n
    W.overflow:SetShown(hidden > 0)
    W.overflow:SetText("+" .. hidden)
end

local function OverflowMenu()
    local list = Conv.List()
    local hidden = {}
    for i = shownCount + 1, #list do
        hidden[#hidden + 1] = list[i]
    end
    table.sort(hidden, function(a, b)
        local ua, ub = (a.unread or 0) > 0, (b.unread or 0) > 0
        if ua ~= ub then
            return ua
        end
        return (a.active or 0) > (b.active or 0)
    end)
    local items = {}
    for _, conv in ipairs(hidden) do
        local text = ShortName(conv)
        if (conv.unread or 0) > 0 then
            text = text .. " |cffffffff(" .. conv.unread .. ")|r"
        end
        items[#items + 1] = { text = text, onClick = function()
            PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
            Conv.Select(conv.key)
        end }
    end
    ns.Popup.Open(W.overflow, items, { "TOPRIGHT", W.overflow, "BOTTOMRIGHT", 0, -2 })
end

---------------------------------------------------------------------------- header
local function LangName(code)
    return ns.Lang.NATIVE[code] or ns.Lang.NAMES[code] or (code and code:upper()) or "?"
end

local function LangCode(code)
    return ns.Lang.NAMES[code] or (code and code:upper()) or "?"
end

local function SourceMenu()
    local conv = Conv.Selected()
    if not conv then
        return
    end
    local _, guessed = Conv.SourceLang(conv)
    local detected = conv.detected
    local items = { { title = true, text = L.CONV_THEY_WRITE } }
    items[#items + 1] = { text = detected and L.CONV_AUTOMATIC:format(LangName(detected)) or L.CONV_AUTOMATIC_UNKNOWN,
        checked = guessed, onClick = function()
            Conv.SetSourceLang(conv, nil)
        end }
    for _, code in ipairs(ns.Lang.TARGETS) do
        items[#items + 1] = { text = LangName(code), checked = not guessed and conv.srcOverride == code,
            onClick = function()
                Conv.SetSourceLang(conv, code)
            end }
    end
    ns.Popup.Open(W.sourceBtn, items)
end

local function UpdateHeader()
    local conv = Conv.Selected()
    if not conv then
        frame:SetTitle(L.CONV_TITLE)
        frame:SetPortraitToAsset("Interface\\Icons\\INV_Misc_QuestionMark")
        frame:SetPortraitTexCoord(0, 1, 0, 1)
        W.header:Hide()
        return
    end
    W.header:Show()
    local title = FullName(conv)
    if conv.name and Settings().names then
        local roman = ns.Glossary.Romanize(conv.name)
        if roman then
            title = title .. " (" .. roman .. ")"
        end
    end
    frame:SetTitle(title)
    if conv.classFile and CLASS_ICON_TCOORDS[conv.classFile] then
        frame:SetPortraitToClassIcon(conv.classFile)
    else
        frame:SetPortraitToAsset("Interface\\Icons\\INV_Misc_QuestionMark")
        frame:SetPortraitTexCoord(0, 1, 0, 1)
    end
    local src = Conv.SourceLang(conv)
    W.sourceBtn:SetText((src and LangCode(src) or "?") .. " v")
    W.sourceBtn:SetWidth(W.sourceBtn:GetTextWidth() + 8)
    W.targetText:SetText(LangCode(Settings().target))
    local label, r, g, b = LastChannel(conv)
    W.channelText:SetText(L.CONV_VIA:format(ColorCode(r, g, b) .. label .. "|r"))
    W.followCheck:SetChecked(Conv.EffectiveFollow(conv) == "translate")
end

local function UpdateBanner()
    local restricted = C_ChatInfo.InChatMessagingLockdown() or (ns.Restriction and ns.Restriction.Active())
    W.banner:SetShown(restricted and true or false)
    W.scroll:SetPoint("TOPLEFT", W.thread, "TOPLEFT", SCROLL_L, -SCROLL_T - (restricted and BANNER_H or 0))
end

---------------------------------------------------------------------------- thread layout
local function AtEnd()
    local range = W.scroll:GetVerticalScrollRange()
    return W.scroll:GetVerticalScroll() >= range - 1
end

local function ScrollToEnd()
    W.scroll:UpdateScrollChildRect()
    W.scroll:SetVerticalScroll(W.scroll:GetVerticalScrollRange())
end

local function ScrollToMsg(msg)
    local top = tops[msg]
    local b = bubbles[msg]
    if not (top and b) then
        return
    end
    W.scroll:UpdateScrollChildRect()
    local viewH = W.scroll:GetHeight()
    local range = W.scroll:GetVerticalScrollRange()
    local v = W.scroll:GetVerticalScroll()
    local h = b:GetHeight()
    if top < v or top + h > v + viewH then
        v = top + h - viewH
        if h > viewH or v < 0 then
            v = top
        end
        W.scroll:SetVerticalScroll(max(0, min(range, v)))
    end
end

local function UpdatePill()
    if newBelow > 0 and not AtEnd() then
        W.newPill:SetText(newBelow == 1 and L.CONV_NEW_ONE or L.CONV_NEW_MANY:format(newBelow))
        W.newPill:SetWidth(W.newPill:GetTextWidth() + 32)
        W.newPill:Show()
    else
        newBelow = 0
        W.newPill:Hide()
    end
end

local function LayoutThread(conv)
    if conv ~= shownConv then
        if shownConv then
            shownConv.scroll, shownConv.atEnd = W.scroll:GetVerticalScroll(), AtEnd()
        end
        ReleaseAll()
        shownConv = conv
        newBelow = 0
    end
    W.empty:SetShown(conv == nil)
    if not conv then
        W.content:SetHeight(1)
        return
    end
    local wasEnd = conv.atEnd ~= false and (conv.scroll == nil or AtEnd())
    local m = ConvFrame.Metrics()
    metrics = m
    local width = ContentWidth()
    W.content:SetWidth(width)
    local keep = {}
    for _, msg in ipairs(conv.msgs) do
        keep[msg] = true
    end
    for msg, b in pairs(bubbles) do
        if not keep[msg] then
            ns.Bubble.Release(b)
            bubbles[msg], tops[msg] = nil, nil
        end
    end
    local y = 4
    for _, msg in ipairs(conv.msgs) do
        local b = bubbles[msg]
        if not b or b.kind ~= msg.kind then
            if b then
                ns.Bubble.Release(b)
            end
            b = msg.kind == "out" and ns.Bubble.AcquireOut(W.content) or ns.Bubble.AcquireIn(W.content)
            bubbles[msg] = b
        end
        b.msg = msg
        if msg.kind == "out" then
            ConfigureOut(b, conv, msg)
        else
            ConfigureIn(b, conv, msg, m)
        end
        b:ClearAllPoints()
        b:SetPoint("TOPLEFT", W.content, "TOPLEFT", 0, -PixelUtil.GetNearestPixelSize(y, W.content:GetEffectiveScale()))
        tops[msg] = y
        b.needsLayout = false
        y = y + b:Layout(width, m)
    end
    W.content:SetHeight(max(1, y))
    W.scroll:UpdateScrollChildRect()
    if wasEnd then
        ScrollToEnd()
    elseif conv.scroll then
        W.scroll:SetVerticalScroll(max(0, min(W.scroll:GetVerticalScrollRange(), conv.scroll)))
    end
end

-- Lays the whole window out again: tabs, header, thread, slots, rows, VIEW. Called by the window
-- itself and by the reply composer when its height changes.
function ConvFrame.Relayout()
    if not frame then
        return
    end
    if laying then
        layoutAgain = true
        return
    end
    laying = true
    repeat
        layoutAgain = false
        counters.relayouts = counters.relayouts + 1
        local scale = tonumber(Settings().convScale) or 100
        scale = min(150, max(80, scale))
        if frame:GetScale() ~= scale / 100 then
            frame:SetScale(scale / 100)
        end
        ns.Bubble.Font()
        local composer = ns.Composer
        local h = 0
        if composer and composer.Height then
            local ok, value = pcall(composer.Height)
            h = (ok and IsNumber(value)) and value or 0
        end
        composerH = h
        W.composerHost:SetHeight(max(1, composerH))
        W.thread:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -THREAD_SIDE, composerH + 6)
        UpdateTabs()
        UpdateHeader()
        UpdateBanner()
        LayoutThread(Conv.Selected())
    until not layoutAgain
    laying = false
    UpdateSlots()
    UpdatePill()
    if frame:IsShown() then
        ns.Reserve.SyncView(false)
    end
end

---------------------------------------------------------------------------- position
-- convPos = { p, x, y, w, h } (numbers only; Core.lua validates it): the window's point p (the
-- addon writes "TOPLEFT") at (x, y) from UIParent's bottom-left corner, in the window's own units
-- (GetLeft and GetTop), and its size.
local function SavePosition()
    if not (frame and ns.db) then
        return
    end
    local left, top = frame:GetLeft(), frame:GetTop()
    local w, h = frame:GetWidth(), frame:GetHeight()
    if IsNumber(left) and IsNumber(top) and IsNumber(w) and IsNumber(h) then
        ns.db.convPos = { p = "TOPLEFT", x = floor(left + 0.5), y = floor(top + 0.5), w = floor(w + 0.5),
            h = floor(h + 0.5) }
    end
end

local function ValidPos(pos)
    return type(pos) == "table" and type(pos.p) == "string" and IsNumber(pos.x) and IsNumber(pos.y)
        and IsNumber(pos.w) and IsNumber(pos.h)
end

local function RestorePosition()
    local pos = Settings().convPos
    frame:ClearAllPoints()
    if ValidPos(pos) then
        frame:SetSize(min(MAX_W, max(MIN_W, pos.w)), min(MAX_H, max(MIN_H, pos.h)))
        frame:SetPoint(pos.p, UIParent, "BOTTOMLEFT", pos.x, pos.y)
    else
        frame:SetSize(DEFAULT_W, DEFAULT_H)
        frame:SetPoint("CENTER", UIParent, "CENTER", -300, 60)
    end
end

-- Options' "Reset window position": back to the first position and size.
function ConvFrame.ResetPosition()
    if ns.db then
        ns.db.convPos = nil
    end
    if frame then
        RestorePosition()
        ConvFrame.Relayout()
        ns.Reserve.SyncView(true)
    end
end

---------------------------------------------------------------------------- build
local function BuildHeader()
    local header = CreateFrame("Frame", nil, frame)
    header:SetPoint("TOPLEFT", frame, "TOPLEFT", 70, -52)
    header:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -12, -52)
    header:SetHeight(20)
    W.header = header
    local src = CreateFrame("Button", nil, header)
    src:SetPoint("LEFT", header, "LEFT", 0, 0)
    src:SetSize(36, 16)
    src:SetNormalFontObject(GameFontNormalSmall)
    src:SetHighlightFontObject(GameFontHighlightSmall)
    src:SetText("? v")
    local hl = src:CreateTexture(nil, "HIGHLIGHT")
    hl:SetColorTexture(1, 1, 1, 0.12)
    hl:SetAllPoints(src)
    src:SetScript("OnClick", function()
        PlaySound(SOUNDKIT.IG_MAINMENU_OPTION)
        SourceMenu()
    end)
    src:SetScript("OnEnter", function(self)
        local conv = Conv.Selected()
        if not conv then
            return
        end
        local code, guessed = Conv.SourceLang(conv)
        GameTooltip:SetOwner(self, "ANCHOR_BOTTOM")
        if not code then
            GameTooltip_SetTitle(GameTooltip, L.CONV_LANG_UNKNOWN_TIP)
        else
            GameTooltip_SetTitle(GameTooltip, LangName(code))
            GameTooltip_AddNormalLine(GameTooltip, guessed and L.CONV_LANG_TIP or L.CONV_LANG_SET_TIP)
            if not ns.Lang.IsTarget(code) then
                GameTooltip_AddErrorLine(GameTooltip, L.CONV_LANG_NOT_TARGET:format(LangCode(code)))
            end
        end
        GameTooltip:Show()
    end)
    src:SetScript("OnLeave", GameTooltip_Hide)
    W.sourceBtn = src
    local arrow = header:CreateFontString(nil, "ARTWORK")
    arrow:SetFontObject(GameFontHighlightSmall)
    arrow:SetPoint("LEFT", src, "RIGHT", 2, 0)
    arrow:SetText(L.CONV_ARROW)
    W.arrow = arrow
    local target = CreateFrame("Button", nil, header)
    target:SetPoint("LEFT", arrow, "RIGHT", 4, 0)
    target:SetSize(24, 16)
    local targetText = target:CreateFontString(nil, "ARTWORK")
    targetText:SetFontObject(GameFontHighlightSmall)
    targetText:SetPoint("LEFT", target, "LEFT", 0, 0)
    W.targetText = targetText
    target:SetScript("OnEnter", function(self)
        GameTooltip:SetOwner(self, "ANCHOR_BOTTOM")
        GameTooltip_SetTitle(GameTooltip, LangName(Settings().target))
        GameTooltip_AddNormalLine(GameTooltip, L.CONV_YOUR_LANG)
        GameTooltip:Show()
    end)
    target:SetScript("OnLeave", GameTooltip_Hide)
    W.target = target
    local channel = header:CreateFontString(nil, "ARTWORK")
    channel:SetFontObject(GameFontDisableSmall)
    channel:SetPoint("LEFT", target, "RIGHT", 12, 0)
    channel:SetJustifyH("LEFT")
    channel:SetWordWrap(false)
    W.channelText = channel
    -- "Translate new messages": the check at 75%, its label in the normal small font, right-aligned
    local label = CreateFrame("Button", nil, header)
    label:SetPoint("RIGHT", header, "RIGHT", 0, 0)
    label:SetHeight(16)
    local labelText = label:CreateFontString(nil, "ARTWORK")
    labelText:SetFontObject(GameFontNormalSmall)
    labelText:SetPoint("RIGHT", label, "RIGHT", 0, 0)
    labelText:SetText(L.CONV_FOLLOW)
    label:SetWidth(labelText:GetUnboundedStringWidth() + 2)
    W.followLabel = label
    local check = CreateFrame("CheckButton", nil, header, "UICheckButtonTemplate")
    check:SetScale(0.75)
    check:SetSize(24, 24)
    check:SetPoint("RIGHT", label, "LEFT", 0, 0)
    W.followCheck = check
    channel:SetPoint("RIGHT", check, "LEFT", -6, 0)
    local function Toggle(on)
        local conv = Conv.Selected()
        if conv then
            PlaySound(on and SOUNDKIT.IG_MAINMENU_OPTION_CHECKBOX_ON or SOUNDKIT.IG_MAINMENU_OPTION_CHECKBOX_OFF)
            Conv.SetFollow(conv, on)
        end
    end
    check:SetScript("OnClick", function(self)
        Toggle(self:GetChecked() and true or false)
    end)
    label:SetScript("OnClick", function()
        check:SetChecked(not check:GetChecked())
        Toggle(check:GetChecked() and true or false)
    end)
    local function FollowTip(self)
        GameTooltip:SetOwner(self, "ANCHOR_BOTTOM")
        GameTooltip_SetTitle(GameTooltip, L.CONV_FOLLOW)
        GameTooltip_AddNormalLine(GameTooltip, L.CONV_FOLLOW_TIP)
        GameTooltip:Show()
    end
    check:SetScript("OnEnter", FollowTip)
    check:SetScript("OnLeave", GameTooltip_Hide)
    label:SetScript("OnEnter", FollowTip)
    label:SetScript("OnLeave", GameTooltip_Hide)
end

local function BuildThread()
    local thread = CreateFrame("Frame", nil, frame, "InsetFrameTemplate")
    thread:SetPoint("TOPLEFT", frame, "TOPLEFT", THREAD_SIDE, -THREAD_TOP)
    thread:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -THREAD_SIDE, 6)
    W.thread = thread
    local scroll = CreateFrame("ScrollFrame", nil, thread)
    scroll:SetPoint("TOPLEFT", thread, "TOPLEFT", SCROLL_L, -SCROLL_T)
    scroll:SetPoint("BOTTOMRIGHT", thread, "BOTTOMRIGHT", -SCROLL_R, SCROLL_B)
    W.scroll = scroll
    local content = CreateFrame("Frame", nil, scroll)
    content:SetSize(ContentWidth(), 1)
    scroll:SetScrollChild(content)
    W.content = content
    local bar = CreateFrame("EventFrame", nil, thread, "MinimalScrollBar")
    bar:SetPoint("TOPLEFT", scroll, "TOPRIGHT", 6, 0)
    bar:SetPoint("BOTTOMLEFT", scroll, "BOTTOMRIGHT", 6, 0)
    if bar.SetHideIfUnscrollable then
        bar:SetHideIfUnscrollable(true)
    end
    W.scrollBar = bar
    ScrollUtil.InitScrollFrameWithScrollBar(scroll, bar)
    scroll:HookScript("OnVerticalScroll", function()
        if shownConv then
            shownConv.scroll, shownConv.atEnd = scroll:GetVerticalScroll(), AtEnd()
        end
        if not laying then
            UpdateSlots()
            UpdatePill()
            ClipAll()
        end
    end)
    scroll:EnableMouseWheel(true)
    scroll:SetScript("OnMouseWheel", function(self, delta)
        local step = WHEEL_ROWS * ((metrics and metrics.rowH) or 16)
        local v = self:GetVerticalScroll() - delta * step
        self:SetVerticalScroll(max(0, min(self:GetVerticalScrollRange(), v)))
    end)
    local banner = thread:CreateFontString(nil, "OVERLAY")
    banner:SetFontObject(GameFontDisableSmall)
    banner:SetPoint("TOPLEFT", thread, "TOPLEFT", SCROLL_L + 4, -SCROLL_T)
    banner:SetPoint("TOPRIGHT", thread, "TOPRIGHT", -SCROLL_R, -SCROLL_T)
    banner:SetHeight(BANNER_H)
    banner:SetText(L.CONV_BANNER)
    banner:Hide()
    W.banner = banner
    local empty = thread:CreateFontString(nil, "OVERLAY")
    empty:SetFontObject(GameFontDisable)
    empty:SetPoint("LEFT", thread, "LEFT", 24, 0)
    empty:SetPoint("RIGHT", thread, "RIGHT", -24, 0)
    empty:SetText(L.CONV_EMPTY)
    W.empty = empty
    local pill = CreateFrame("Button", nil, thread, "UIPanelButtonTemplate")
    pill:SetHeight(22)
    pill:SetPoint("BOTTOM", thread, "BOTTOM", 0, 8)
    pill:SetFrameLevel(scroll:GetFrameLevel() + 20)
    pill:Hide()
    pill:SetScript("OnClick", function()
        newBelow = 0
        ScrollToEnd()
        UpdatePill()
    end)
    W.newPill = pill
end

function ConvFrame.Build()
    if frame then
        return frame
    end
    frame = CreateFrame("Frame", FRAME_NAME, UIParent, "PortraitFrameTemplate")
    frame:Hide()
    frame:SetFrameStrata("HIGH")
    frame:SetToplevel(true)
    frame:SetClampedToScreen(true)
    frame:SetClampRectInsets(0, 0, 16, 0)   -- the top 16 px stay free for the bridge strip
    frame:SetMovable(true)
    frame:SetResizable(true)
    frame:SetResizeBounds(MIN_W, MIN_H, MAX_W, MAX_H)
    frame:SetDontSavePosition(true)
    frame:EnableMouse(true)
    frame:SetTitle(L.CONV_TITLE)
    -- the window's X hides it directly: HideUIPanel does nothing for an addon frame in combat
    frame.CloseButton:SetScript("OnClick", function()
        frame:Hide()
    end)
    RestorePosition()
    if not tContains(UISpecialFrames, FRAME_NAME) then
        tinsert(UISpecialFrames, FRAME_NAME)
    end
    -- moved by its title bar; the new VIEW goes out when the move ends
    frame:RegisterForDrag("LeftButton")
    frame:SetScript("OnDragStart", function(self)
        if self.TitleContainer:IsMouseOver() then
            self:StartMoving()
        end
    end)
    frame:SetScript("OnDragStop", function(self)
        self:StopMovingOrSizing()
        SavePosition()
        ns.Reserve.SyncView(true)
    end)
    local tabStrip = CreateFrame("Frame", nil, frame)
    tabStrip:SetPoint("TOPLEFT", frame, "TOPLEFT", TAB_LEFT, TAB_TOP)
    tabStrip:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -TAB_RIGHT, TAB_TOP)
    tabStrip:SetHeight(TAB_H)
    tabStrip.minTabWidth, tabStrip.maxTabWidth, tabStrip.tabPadding = MIN_TAB_W, MAX_TAB_W, TAB_PADDING
    W.tabStrip = tabStrip
    local overflow = CreateFrame("Button", nil, frame, "UIPanelButtonTemplate")
    overflow:SetSize(32, 22)
    overflow:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -12, -28)
    overflow:Hide()
    overflow:SetScript("OnClick", OverflowMenu)
    overflow:SetScript("OnEnter", function(self)
        GameTooltip:SetOwner(self, "ANCHOR_RIGHT")
        GameTooltip_SetTitle(GameTooltip, L.CONV_OVERFLOW_TIP)
        GameTooltip:Show()
    end)
    overflow:SetScript("OnLeave", GameTooltip_Hide)
    W.overflow = overflow
    for i = 1, MAX_SHOWN_TABS do
        NewTab(i):Hide()
    end
    tabs[1]:SetPoint("BOTTOMLEFT", tabStrip, "BOTTOMLEFT", 0, 0)
    BuildHeader()
    BuildThread()
    local host = CreateFrame("Frame", nil, frame)
    host:SetPoint("BOTTOMLEFT", frame, "BOTTOMLEFT", 0, 0)
    host:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", 0, 0)
    host:SetHeight(1)
    W.composerHost = host
    ConvFrame.composerHost = host
    local grip = CreateFrame("Button", nil, frame, "PanelResizeButtonTemplate")
    grip:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -4, 4)
    grip:SetFrameLevel(host:GetFrameLevel() + 30)
    W.grip = grip
    -- while the grip drags, the layout waits for the end of the resize (one relayout, not one per frame)
    frame:SetScript("OnSizeChanged", function()
        if frame:IsShown() and not laying and not W.grip.isActive then
            ConvFrame.Relayout()
        end
    end)
    grip:Init(frame, MIN_W, MIN_H, MAX_W, MAX_H)
    grip:SetOnResizeStoppedCallback(function()
        SavePosition()
        ConvFrame.Relayout()
        ns.Reserve.SyncView(true)
    end)
    frame:SetScript("OnShow", function()
        PlaySound(SOUNDKIT.IG_CHARACTER_INFO_OPEN)
        ConvFrame.Relayout()
        Conv.MarkRead(Conv.Selected())
        ns.Reserve.StartWatch()
        ns.Reserve.SyncView(true)
        if ns.db and not ns.db.convTipShown then
            ns.db.convTipShown = true
            ns.Print(L.CONV_FIRST_TIP)
        end
    end)
    frame:SetScript("OnHide", function()
        PlaySound(SOUNDKIT.IG_CHARACTER_INFO_CLOSE)
        ns.Popup.OwnerHidden(frame)
        -- the consent dialog belongs to this window: hidden with it, nothing decided
        if ns.Consent and ns.Consent.Cancel then
            ns.Consent.Cancel()
        end
        if GameTooltip:GetOwner() and GameTooltip:IsShown() then
            GameTooltip:Hide()
        end
        ns.Reserve.SyncView(true)
        ns.Reserve.MaybeStopWatch()
    end)
    local composer = ns.Composer
    if composer and composer.Attach then
        composer.Attach(host)
        if composer.SetConversation then
            composer.SetConversation(Conv.Selected())
        end
    end
    return frame
end

---------------------------------------------------------------------------- opening and toggling
function ConvFrame.Show()
    ConvFrame.Build()
    if not frame:IsShown() then
        frame:Show()
    end
    frame:Raise()
end

function ConvFrame.Hide()
    if frame and frame:IsShown() then
        frame:Hide()
    end
end

-- The binding and /tpr conv: shows the window (the last conversation, else the first one, else the
-- empty window) or hides it. Like a [T] click, showing it needs a confirmed Translate Companion
-- (else only its popup opens, and Continue there shows the window).
function ConvFrame.Toggle()
    if frame and frame:IsShown() then
        frame:Hide()
        return
    end
    ns.Presence.Ensure(function()
        if not Conv.Selected() then
            local first = Conv.List()[1]
            if first then
                Conv.Select(first.key)
            end
        end
        ConvFrame.Show()
    end)
end

-- A [T] click in window mode (ChatButton): see the top of this file. noCache: Shift was held.
function ConvFrame.OpenFromLine(lineID, noCache, chatFrame)
    if not ns.ChatButton.SourceText(lineID) and C_ChatInfo.InChatMessagingLockdown() then
        -- like an inline click: runs again once the restriction ends
        ns.Restriction.Defer(lineID, function()
            ConvFrame.OpenFromLine(lineID, false, chatFrame)
        end)
        return
    end
    local conv, msg, dup = Conv.FromLine(lineID)
    if not conv then
        ns.Print(msg == "full" and L.CONV_FULL or L.CONV_TOO_OLD)
        return
    end
    counters.opened = counters.opened + 1
    ConvFrame.Build()
    if noCache then
        msg.noCache = true
    end
    Conv.Select(conv.key)
    if not frame:IsShown() then
        frame:Show()        -- lays out (OnShow)
    else
        ConvFrame.Relayout()
    end
    frame:Raise()
    ScrollToMsg(msg)
    UpdateSlots()
    if dup and bubbles[msg] then
        counters.flashes = counters.flashes + 1
        bubbles[msg]:Flash(FLASH_SECONDS)
    end
    if Settings().convFocus and ns.Composer and ns.Composer.Focus then
        ns.Composer.Focus()
    end
end

-- Tab / Shift+Tab in the reply box: the next or previous conversation (wrapping).
function ConvFrame.CycleTab(dir)
    local list = Conv.List()
    if #list < 2 then
        return
    end
    local i = SelectedIndex(list) or 1
    local j = (i - 1 + (dir and dir < 0 and -1 or 1)) % #list + 1
    PlaySound(SOUNDKIT.IG_CHARACTER_INFO_TAB)
    Conv.Select(list[j].key)
end

-- The bubble of a message of the shown conversation (tests, the composer).
function ConvFrame.BubbleOf(msg)
    return bubbles[msg]
end

function ConvFrame.Widgets()
    return W, tabs
end

function ConvFrame.Frame()
    return frame
end

function ConvFrame.Counters()
    local c = {}
    for k, v in pairs(counters) do
        c[k] = v
    end
    c.shownTabs = shownCount
    return c
end

---------------------------------------------------------------------------- conversation events
Conv.On("select", function(conv)
    if not frame then
        return
    end
    if conv and frame:IsShown() then
        Conv.MarkRead(conv)
    end
    local composer = ns.Composer
    if composer and composer.SetConversation then
        composer.SetConversation(conv)
    end
    ConvFrame.Relayout()
    if frame:IsShown() then
        ns.Reserve.SyncView(true)
    end
end)

Conv.On("change", function(conv, what)
    if not frame then
        return
    end
    if conv and conv == shownConv and what == "msg" and not AtEnd() then
        local count = 0
        for msg in pairs(bubbles) do
            count = count + (msg and 1 or 0)
        end
        if #conv.msgs > count then
            newBelow = newBelow + (#conv.msgs - count)
        end
    end
    if what == "clear" then
        ReleaseAll()
        shownConv = nil
        if frame:IsShown() then
            frame:Hide()    -- /tpr conv clear, /tpr clear: nothing left to show
        end
    end
    if laying then
        layoutAgain = true
    elseif frame:IsShown() and (conv == nil or conv == shownConv or conv == Conv.Selected() or what == "order"
        or what == "evict") then
        ConvFrame.Relayout()
    else
        UpdateTabs()        -- another tab's message: only its label changes
    end
end)

Conv.On("close", function()
    if frame and frame:IsShown() and #Conv.List() == 0 then
        frame:Hide()        -- closing the last tab hides the window
    end
end)

Conv.On("incoming", function(conv, msg)
    if not Settings().convOpenWhisper or InCombatLockdown() then
        return
    end
    ConvFrame.Build()
    if not frame:IsShown() and (msg.event == "CHAT_MSG_WHISPER" or msg.event == "CHAT_MSG_BN_WHISPER") then
        if not Conv.Selected() then
            Conv.Select(conv.key)
        end
        frame:Show()        -- never with the keyboard focus
    end
end)

---------------------------------------------------------------------------- client events
local events = CreateFrame("Frame")
events:RegisterEvent("UI_SCALE_CHANGED")
events:RegisterEvent("DISPLAY_SIZE_CHANGED")
events:RegisterEvent("UPDATE_CHAT_COLOR")
events:RegisterEvent("ADDON_RESTRICTION_STATE_CHANGED")
events:RegisterEvent("PLAYER_REGEN_DISABLED")
events:RegisterEvent("PLAYER_REGEN_ENABLED")
local reopenAfterCombat = false
events:SetScript("OnEvent", function(_, event)
    if not frame then
        return
    end
    if event == "PLAYER_REGEN_DISABLED" then
        if Settings().convCombat == "hide" and frame:IsShown() then
            reopenAfterCombat = true
            frame:Hide()
        end
    elseif event == "PLAYER_REGEN_ENABLED" then
        if reopenAfterCombat then
            reopenAfterCombat = false
            frame:Show()
        end
    elseif event == "ADDON_RESTRICTION_STATE_CHANGED" then
        -- after Restriction.lua has read the new state
        C_Timer.After(0, function()
            if frame:IsShown() then
                ConvFrame.Relayout()
            end
        end)
    elseif frame:IsShown() then
        ConvFrame.Relayout()
        ns.Reserve.SyncView(true)
    end
end)

---------------------------------------------------------------------------- key bindings (Bindings.xml)
function TranslateProbe_ConvToggle()
    ConvFrame.Toggle()
end

function TranslateProbe_ConvReply()
    ns.Presence.Ensure(function()
        if not Conv.Selected() then
            local first = Conv.List()[1]
            if first then
                Conv.Select(first.key)
            end
        end
        ConvFrame.Show()
        if ns.Composer and ns.Composer.Focus then
            ns.Composer.Focus()
        end
    end)
end
