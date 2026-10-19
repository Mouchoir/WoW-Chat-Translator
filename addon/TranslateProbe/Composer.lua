-- The reply composer at the bottom of the conversation window (docs\ADDON.md, "Reply and
-- send"): the reply card, the "1 Write > 2 Paste > 3 Send" stepper, the reply box with its byte
-- count, Translate and the reply language, and the channel buttons.
--
-- States of the selected conversation (A.6.1):
--   EMPTY    box empty                   Enter clears the focus
--   DRAFT    text typed                  Enter translates it
--   WAITING  after Translate, box empty  Enter only says "Paste with Ctrl+V first"
--   PASTED   a paste landed in WAITING   Enter clears the focus ("Click a channel button")
--   SENT     just sent                   back to EMPTY after a few seconds or a keystroke
-- Enter never sends (user decision D1): only a click on a channel button does, once per click
-- with a 1 s guard. When a paste check fails (own text, password-like token, wrong language,
-- pasted too early; or typed text still in my own language), the first click shows the warning
-- and arms that button for 3 s; a second click sends. Any text change disarms.
--
-- Translate sends TRANSLATE on the reply slot (126): rows = 1 + the estimated rows of the
-- translation (row 0 carries the Helper's status), flags plate + reply, + copy when the user
-- allowed the clipboard (Consent), + noCache on Shift-click. One reply exists at a time, in one
-- conversation; a new Translate replaces it on the same slot. After a send, CANCEL lets the
-- Helper empty the clipboard if it still holds its own text. While chat is locked the send is
-- refused before any API call and nothing is cancelled, so the clipboard keeps the translation.
--
-- Drafts, the reply, sent texts and states live in Lua tables only (lost on /reload).
-- luacheck: read globals geterrorhandler
local _, ns = ...
local L = ns.L

local Composer = {}
ns.Composer = Composer

local BASE_HEIGHT = 88          -- send row, box, stepper (A.1 height budget)
local CARD_GAP = 6
local ARM_SECONDS = 3
local REPEAT_SECONDS = 1
local EARLY_SECONDS = 1         -- a paste this soon after Translate may be the old clipboard
local SENT_SECONDS = 4
local FLASH_SECONDS = 2.5
local FOCUS_KEEP_SECONDS = 1.5  -- combat start keeps the focus when a key was typed this recently
local MAX_TEXT_ROWS = 5         -- reply rows 1..5 carry the text, row 0 the status
local BUTTON_GAP = 6

-- Source script > target script: how much longer a translation usually is (Reserve.lua's table)
local FACTOR = {
    ["latin>latin"] = 1.35, ["cyrillic>latin"] = 1.2, ["latin>cyrillic"] = 1.2,
    ["han>latin"] = 2.2, ["kana>latin"] = 2.2, ["hangul>latin"] = 1.8,
    ["han>cyrillic"] = 2.2, ["hangul>cyrillic"] = 1.8,
    ["latin>han"] = 0.8, ["cyrillic>han"] = 0.8, ["latin>hangul"] = 1.0,
}
local DEFAULT_FACTOR = 1.3

local EMPTY, DRAFT, WAITING, PASTED, SENT = "EMPTY", "DRAFT", "WAITING", "PASTED", "SENT"

local ui                        -- widgets, built on the first Attach
local host                      -- the frame the composer lives in (ConvFrame.composerHost)
local conv                      -- the selected conversation, or nil
local reply                     -- the pending reply: { conv, reqId, draft, target, copy, rows, at, flags }
local card, cardH = nil, 0      -- the reply card (Bubble.AcquireReply) and its height
local states = setmetatable({}, { __mode = "k" })   -- conv -> { mode, lastText, lastDraft, pastedAt, ... }
local armed                     -- { id, at }: the button a failed check armed
local flash                     -- { text, color }: a short message in the stepper
local lastSendClick = -math.huge
local lastKeyAt = -math.huge
local suppress = false          -- the addon itself is changing the box text
local relaying = false
local counters = { translated = 0, cancelled = 0, pasted = 0, warned = 0, sent = 0, refused = 0 }

---------------------------------------------------------------------------- helpers
local function Plain(v)
    return type(v) == "string" and not issecretvalue(v)
end

local function Conv(name, ...)
    local C = ns.Conv
    if C and type(C[name]) == "function" then
        return C[name](...)
    end
end

-- Calls ConvFrame[name] when the window module has it; an error is reported, not raised, so a
-- window problem never stops a send or a reply.
local function Frame(name, ...)
    local F = ns.ConvFrame
    if F and type(F[name]) == "function" then
        local ok, a = pcall(F[name], ...)
        if ok then
            return a
        end
        geterrorhandler()(a)
    end
end

local function Slot()
    return ns.Reserve.REPLY_SLOT
end

local function Utf8Len(s)
    local n = 0
    for _ in s:gmatch("[^\128-\191]") do
        n = n + 1
    end
    return n
end

local function Wrap(color, text)
    color = color or NORMAL_FONT_COLOR
    return color:WrapTextInColorCode(text)
end

local function Orange()
    return ORANGE_FONT_COLOR or NORMAL_FONT_COLOR
end

-- "checkmark-minimal" at its atlas size (at most the stepper's 14 units high).
local function Check()
    local ok, info = pcall(C_Texture.GetAtlasInfo, "checkmark-minimal")
    if not (ok and type(info) == "table" and info.width and info.height and info.height > 0) then
        return ""
    end
    local h = math.min(14, info.height)
    local w = math.floor(info.width * h / info.height + 0.5)
    return ("|A:checkmark-minimal:%d:%d|a"):format(h, w)
end

local function State(c)
    local st = states[c]
    if not st then
        st = { mode = EMPTY }
        states[c] = st
    end
    return st
end

local function ReplyLang(c)
    local code = Conv("ReplyLang", c)
    return Plain(code) and code or nil
end

local function SameLang(c)
    return ReplyLang(c) == ns.db.target
end

local function LangKey(code)
    return "RPL_LANG_" .. code:upper():gsub("%-", "_")
end

local function LangName(code)
    if not Plain(code) then
        return "?"
    end
    return L[LangKey(code)] or code:upper()
end

local function BaseLang(code)
    return Plain(code) and (code:gsub("%-.*$", "")) or nil
end

local function Detect(text)
    local D = ns.LangDetect
    if D and type(D.Detect) == "function" then
        local ok, code = pcall(D.Detect, text)
        if ok and Plain(code) then
            return code
        end
    end
end

local function HasReply(c)
    return reply ~= nil and reply.conv == c
end

local function ShortName(c)
    local name = c and c.name
    if not Plain(name) then
        return "?"
    end
    return (name:gsub("%-.*$", ""))
end

---------------------------------------------------------------------------- paste checks
local function Norm(s)
    return strtrim(s or ""):lower()
end

-- A password-like token (8+ characters mixing 3 of lower, upper, digit, symbol), a URL or an
-- e-mail address. Apostrophes and hyphens are part of words, and the punctuation around a
-- token is not counted.
local TLD = { com = true, net = true, org = true, io = true, gg = true, fr = true, de = true, ru = true, me = true,
    tv = true, co = true, uk = true, es = true, it = true, pt = true, br = true, ly = true, info = true }
local function Suspicious(text)
    if text:find("[%w%.%-_]+@[%w%-]+%.%a%a+") then
        return true
    end
    for token in text:gmatch("%S+") do
        local lower = token:lower()
        if lower:find("^https?://") or lower:find("^www%.") then
            return true
        end
        local core = token:gsub("^%p+", ""):gsub("%p+$", "")
        local tld = core:lower():match("^[%w%-]+%.(%a%a+)$")
        if tld and TLD[tld] then
            return true
        end
        if #core >= 8 then
            local classes = (core:find("%l") and 1 or 0) + (core:find("%u") and 1 or 0)
                + (core:find("%d") and 1 or 0) + (core:gsub("['%-]", ""):find("%p") and 1 or 0)
            if classes >= 3 then
                return true
            end
        end
    end
    return false
end

-- The warning a send click must confirm, or nil.
local function Warning(st, text)
    local lang = ReplyLang(conv)
    if st.mode == PASTED then
        if HasReply(conv) and Norm(text) == Norm(reply.draft) then
            return L.RPL_WARN_OWN_TEXT .. L.RPL_CLICK_AGAIN
        end
        if Suspicious(text) then
            return L.RPL_WARN_SUSPICIOUS .. L.RPL_CLICK_AGAIN
        end
        local detected = Detect(text)
        if detected and lang and BaseLang(detected) ~= BaseLang(lang) then
            return L.RPL_WARN_LANG:format(LangName(detected), LangName(lang)) .. L.RPL_CLICK_AGAIN
        end
        if HasReply(conv) and st.pastedAt and st.pastedAt - reply.at < EARLY_SECONDS then
            return L.RPL_WARN_EARLY .. L.RPL_CLICK_AGAIN
        end
    elseif st.mode == DRAFT then
        local mine = ns.db.target
        if lang and BaseLang(lang) ~= BaseLang(mine) and BaseLang(Detect(text)) == BaseLang(mine) then
            return L.RPL_WARN_NOT_TRANSLATED
        end
    end
    return nil
end

---------------------------------------------------------------------------- text and rows
-- The window font (TranslateProbeConvFont from Bubble.Font, at the convText size), else a copy
-- of ChatFontNormal of our own.
local function ReplyFont()
    if ns.Bubble and type(ns.Bubble.Font) == "function" then
        local ok, font = pcall(ns.Bubble.Font)
        if ok and font then
            return font
        end
    end
    if not ui.font then
        ui.font = CreateFont("TranslateProbeReplyFont")
        ui.font:CopyFontObject(ChatFontNormal)
    end
    local path, _, flags = ChatFontNormal:GetFont()
    ui.font:SetFont(path, ns.db.convText or 14, flags or "")
    return ui.font
end

local function Metrics()
    local m = Frame("Metrics")
    if type(m) == "table" then
        return m
    end
end

-- Rows of the translation (1..5), from the draft's width in the window font against the
-- bubble plate width, times the script factor (and 10% for the window, as for incoming bubbles).
local function EstimateRows(draft, lang)
    local m = Metrics()
    local plateW = m and tonumber(m.plateWidth) or 0
    if plateW <= 0 then
        plateW = 300
    end
    local visible = ns.Glossary.VisibleText(draft)
    local width
    if ns.Bubble and type(ns.Bubble.TextWidth) == "function" then
        width = ns.Bubble.TextWidth(visible)
    else
        ui.measure:SetFontObject(ReplyFont())
        ui.measure:SetText(visible)
        width = ui.measure:GetUnboundedStringWidth()
    end
    width = tonumber(width) or 0
    local script = ns.Lang.Script(draft)
    local f = script and FACTOR[script .. ">" .. ns.Lang.TargetScript(lang)] or DEFAULT_FACTOR
    return math.max(1, math.min(MAX_TEXT_ROWS, math.ceil(width / plateW * f * 1.1 - 0.15)))
end

-- Changes the box text without counting it as typing.
local function SetBox(text)
    if not ui then
        return
    end
    suppress = true
    ui.box:SetText(text or "")
    suppress = false
    if conv then
        State(conv).lastText = ui.box:GetText() or ""
    end
end

---------------------------------------------------------------------------- refresh
local Refresh   -- defined below

local function SetFlash(text, color, seconds)
    local f = { text = text, color = color }
    flash = f
    C_Timer.After(seconds or FLASH_SECONDS, function()
        if flash == f then
            flash = nil
            Refresh()
        end
    end)
end

local function StepperText(st, over)
    if not conv then
        return ""
    end
    if flash then
        return Wrap(flash.color, flash.text)
    end
    if ns.Channels.Locked() then
        return Wrap(RED_FONT_COLOR, L.RPL_STEP_LOCKED)
    end
    if over > 0 then
        return Wrap(Orange(), L.RPL_STEP_TOO_LONG:format(over))
    end
    local check = Check()
    local sep = Wrap(GRAY_FONT_COLOR, " > ")
    if st.mode == SENT then
        return Wrap(GREEN_FONT_COLOR, check .. L.RPL_STEP_SENT:format(st.sentLabel or "?"))
    elseif st.mode == WAITING then
        return Wrap(GREEN_FONT_COLOR, check .. L.RPL_STEP_1) .. sep .. Wrap(NORMAL_FONT_COLOR, L.RPL_STEP_2)
            .. sep .. Wrap(GRAY_FONT_COLOR, L.RPL_STEP_3)
    elseif st.mode == PASTED and HasReply(conv) then
        return Wrap(GREEN_FONT_COLOR, check .. L.RPL_STEP_1_DONE) .. sep
            .. Wrap(GREEN_FONT_COLOR, check .. L.RPL_STEP_2_PASTED) .. sep .. Wrap(NORMAL_FONT_COLOR, L.RPL_STEP_3_CLICK)
    elseif SameLang(conv) then
        return Wrap(NORMAL_FONT_COLOR, L.RPL_STEP_SAME)
    end
    return Wrap(NORMAL_FONT_COLOR, L.RPL_STEP_WRITE)
end

local function Placeholder(st)
    if not conv then
        return ""
    end
    if st.mode == WAITING and HasReply(conv) then
        return reply.copy and L.RPL_PH_PASTE or L.RPL_PH_TYPE
    end
    return SameLang(conv) and L.RPL_PH_SAME or L.RPL_PH_WRITE
end

local function WhyText(key, over)
    local text = L[key] or L.RPL_WHY_UNKNOWN
    if key == "RPL_WHY_TOO_LONG" then
        return text:format(over or 0)
    end
    return text
end

local function ShowButtonTip(b)
    GameTooltip:SetOwner(b, "ANCHOR_TOP")
    GameTooltip:SetText(b.tipTitle or "", NORMAL_FONT_COLOR.r, NORMAL_FONT_COLOR.g, NORMAL_FONT_COLOR.b)
    for _, line in ipairs(b.tipLines or {}) do
        GameTooltip:AddLine(line[1], line[2].r, line[2].g, line[2].b, true)
    end
    GameTooltip:Show()
end

local function SendClick(b)
    Composer.SendTo(b.targetId)
end

local function NewSendButton()
    local b = CreateFrame("Button", nil, ui.root, "UIPanelButtonTemplate")
    b:SetHeight(22)
    b:SetMotionScriptsWhileDisabled(true)
    b:SetScript("OnClick", SendClick)
    b:SetScript("OnEnter", ShowButtonTip)
    b:SetScript("OnLeave", GameTooltip_Hide)
    return b
end

local function TextWidth(b)
    local fs = b:GetFontString()
    return fs and fs:GetUnboundedStringWidth() or 0
end

-- The channel buttons: label "Send to:", the targets, then [Other].
local function RefreshButtons(wire, bytes)
    local targets = conv and ns.Channels.Targets(conv) or {}
    local locked = ns.Channels.Locked()
    local x = 14 + ui.sendLabel:GetUnboundedStringWidth() + 8
    for i = 1, math.max(#targets, #ui.buttons) do
        local t = targets[i]
        local b = ui.buttons[i]
        if t and not b then
            b = NewSendButton()
            ui.buttons[i] = b
        end
        if t then
            b.targetId, b.targetLabel = t.id, t.label
            b:SetText(t.label)
            b:SetWidth(math.max(48, TextWidth(b) + 24))
            b:ClearAllPoints()
            b:SetPoint("BOTTOMLEFT", ui.root, "BOTTOMLEFT", x, 10)
            x = x + b:GetWidth() + BUTTON_GAP
            local reason
            if locked then
                reason = L.RPL_WHY_LOCKED
            elseif not t.enabled then
                reason = WhyText(t.reason)
            elseif not wire then
                reason = L.RPL_WHY_EMPTY
            elseif bytes > ns.Channels.MAX_BYTES then
                reason = WhyText("RPL_WHY_TOO_LONG", bytes - ns.Channels.MAX_BYTES)
            end
            b:SetEnabled(reason == nil)
            if t.isDefault then
                b:LockHighlight()
            else
                b:UnlockHighlight()
            end
            b.tipTitle = t.label
            b.tipLines = {}
            if t.isDefault then
                b.tipLines[#b.tipLines + 1] = { L.RPL_TIP_DEFAULT:format(ShortName(conv)), HIGHLIGHT_FONT_COLOR }
            end
            if reason then
                b.tipLines[#b.tipLines + 1] = { reason, RED_FONT_COLOR }
            else
                b.tipLines[#b.tipLines + 1] = { L.RPL_TIP_CLICK_SEND, GREEN_FONT_COLOR }
            end
            b:Show()
        elseif b then
            b.targetId = nil
            b:Hide()
        end
    end
    local bn = conv and type(conv.bnID) == "number" and conv.bnID > 0
    ui.other:SetShown(conv ~= nil and not bn)
    ui.other:ClearAllPoints()
    ui.other:SetPoint("BOTTOMLEFT", ui.root, "BOTTOMLEFT", x, 10)
    ui.sendLabel:SetShown(conv ~= nil)
end

Refresh = function()
    if not ui then
        return
    end
    local st = conv and State(conv) or { mode = EMPTY }
    local text = ui.box:GetText() or ""
    local wire, bytes = ns.Channels.Wire(text)
    local over = (wire and bytes or 0) - ns.Channels.MAX_BYTES
    ui.stepper:SetText(StepperText(st, over))
    ui.box.Instructions:SetText(Placeholder(st))
    ui.box.Instructions:SetShown(text == "")
    ui.bytes:SetText(L.RPL_BYTES:format(wire and bytes or 0))
    local c = over > 0 and RED_FONT_COLOR or GRAY_FONT_COLOR
    ui.bytes:SetTextColor(c.r, c.g, c.b)
    if conv then
        ui.box:Enable()
    else
        ui.box:Disable()
    end
    local lang = conv and ReplyLang(conv)
    local same = conv ~= nil and lang == ns.db.target
    ui.translate:SetShown(conv ~= nil and not same)
    ui.lang:SetShown(conv ~= nil)
    if ns.Lang.IsTarget(lang) then
        ui.translate:SetText(L.RPL_TRANSLATE_TO:format(ns.Lang.NAMES[lang] or lang:upper()))
    else
        ui.translate:SetText(L.RPL_TRANSLATE_UNKNOWN)
    end
    RefreshButtons(wire, wire and bytes or 0)
end

---------------------------------------------------------------------------- the reply card
local function Relayout()
    if relaying then
        return
    end
    relaying = true
    Frame("Relayout")
    relaying = false
end

local function SyncView()
    if ns.ConvFrame and type(ns.ConvFrame.SyncView) == "function" then
        Frame("SyncView", true)
    else
        ns.Reserve.SyncView(true)
    end
end

-- The card's Edit and cancel buttons (Bubble's EditButton and CancelButton), else our own.
local function CardButtons()
    if card.EditButton and card.CancelButton then
        return card.EditButton, card.CancelButton
    end
    if not ui.edit then
        local edit = CreateFrame("Button", nil, ui.root)
        edit:SetSize(60, 16)
        edit:SetNormalFontObject(GameFontNormalSmall)
        edit:SetHighlightFontObject(GameFontHighlightSmall)
        edit:SetText(L.RPL_EDIT)
        edit:GetFontString():SetPoint("RIGHT", edit, "RIGHT", 0, 0)
        edit:SetFrameLevel(ui.root:GetFrameLevel() + 20)
        ui.edit = edit
        local cancel = CreateFrame("Button", nil, ui.root, "UIPanelCloseButton")
        cancel:SetSize(20, 20)
        cancel:SetFrameLevel(ui.root:GetFrameLevel() + 20)
        ui.cancel = cancel
        for _, b in ipairs({ edit, cancel }) do
            b:SetScript("OnEnter", ShowButtonTip)
            b:SetScript("OnLeave", GameTooltip_Hide)
        end
        edit.tipTitle, edit.tipLines = L.RPL_EDIT, { { L.RPL_TIP_EDIT, HIGHLIGHT_FONT_COLOR } }
        cancel.tipTitle, cancel.tipLines = CLOSE or L.RPL_CANCEL, { { L.RPL_TIP_CANCEL, HIGHLIGHT_FONT_COLOR } }
    end
    return ui.edit, ui.cancel
end

local function ReleaseCard()
    if card then
        if ns.Bubble and type(ns.Bubble.Release) == "function" then
            pcall(ns.Bubble.Release, card)
        elseif card.Hide then
            card:Hide()
        end
        card = nil
    end
    cardH = 0
    if ui and ui.edit then
        ui.edit:Hide()
        ui.cancel:Hide()
    end
end

local function HideCard()
    local was = cardH > 0
    if card then
        card:Hide()
    end
    if ui and ui.edit then
        ui.edit:Hide()
        ui.cancel:Hide()
    end
    cardH = 0
    if was then
        Relayout()
    end
end

-- Shows the reply card when the pending reply belongs to the selected conversation. Without
-- force, a card already laid out for this reply at this width is left as it is.
local function ShowCard(force)
    if not (ui and HasReply(conv)) then
        HideCard()
        return
    end
    if not card then
        if not (ns.Bubble and type(ns.Bubble.AcquireReply) == "function") then
            return
        end
        card = ns.Bubble.AcquireReply(ui.root)
        if not card then
            return
        end
    end
    local m = Metrics()
    local x0 = m and tonumber(m.x0) or 20
    local width = m and tonumber(m.bubbleWidth) or math.max(100, (host:GetWidth() or 440) - 2 * x0 - 10)
    if not force and cardH > 0 and card.tpReq == reply.reqId and card.tpWidth == width and card:IsShown() then
        return
    end
    card.tpReq, card.tpWidth = reply.reqId, width
    card:SetDraft(reply.draft)
    card:SetRows(Slot(), reply.rows, ns.sessionId % 2)
    card:SetLaneHint(L.RPL_LANE_HINT)
    card:ClearAllPoints()
    if PixelUtil and PixelUtil.SetPoint then
        PixelUtil.SetPoint(card, "BOTTOMLEFT", ui.root, "BOTTOMLEFT", x0, BASE_HEIGHT)
    else
        card:SetPoint("BOTTOMLEFT", ui.root, "BOTTOMLEFT", x0, BASE_HEIGHT)
    end
    card:Show()
    local h = m and card:Layout(width, m)     -- no metrics (no window yet): the card keeps its size
    if type(h) ~= "number" or h <= 0 then
        h = card:GetHeight() or 0
    end
    cardH = h
    local edit, cancel = CardButtons()
    edit:SetScript("OnClick", function()
        Composer.EditReply()
    end)
    cancel:SetScript("OnClick", function()
        Composer.CancelReply()
    end)
    if edit == ui.edit then
        cancel:ClearAllPoints()
        cancel:SetPoint("TOPRIGHT", card, "TOPRIGHT", -2, -2)
        edit:ClearAllPoints()
        edit:SetPoint("RIGHT", cancel, "LEFT", -4, 0)
        edit:Show()
        cancel:Show()
    end
    Relayout()
end

---------------------------------------------------------------------------- translate
local function Start(c, draft, lang, copy, noCache)
    if c ~= conv or not ui then
        return
    end
    local B = ns.Bridge
    local rows = 1 + EstimateRows(draft, lang)
    local flags = B.FLAG_PLATE + B.FLAG_REPLY + (copy and B.FLAG_COPY or 0) + (noCache and B.FLAG_NOCACHE or 0)
    if reply and reply.conv then
        reply.conv.pendingReply = nil
    end
    reply = { conv = c, reqId = ns.Request.NextReqId(), draft = draft, target = lang, copy = copy, rows = rows,
        at = GetTime(), flags = flags }
    c.pendingReply = true       -- Conversations never evicts a conversation with a pending reply
    local st = State(c)
    st.lastDraft, st.mode, st.pastedAt = draft, WAITING, nil
    armed, flash = nil, nil
    SetBox("")
    c.draft = nil
    ShowCard()
    ui.box:SetFocus()
    SyncView()
    B.Send(B.TranslateMessage(reply.reqId, Slot(), rows, flags, lang, ns.Glossary.PromptText(draft), {}))
    counters.translated = counters.translated + 1
    Refresh()
end

local function OpenPopup(owner, items)
    local P = ns.Popup
    if P and type(P.Open) == "function" then
        P.Open(owner, items, owner)
        return true
    end
    return false
end

-- The reply language list; then() runs after a pick.
local function LangPopup(after)
    if not conv then
        return
    end
    local c = conv
    local current = ReplyLang(c)
    local items = { { text = L.RPL_POPUP_LANG_TITLE, title = true } }
    for _, code in ipairs(ns.Lang.TARGETS) do
        items[#items + 1] = { text = ns.Lang.NATIVE[code] or code, checked = code == current, onClick = function()
            Conv("SetReplyLang", c, code)
            Refresh()
            if after and c == conv then
                after()
            end
        end }
    end
    OpenPopup(ui.lang, items)
end

-- Translate (Enter in DRAFT, or the button). With the box empty while a reply of this
-- conversation is pending, its draft is translated again ("click Translate again").
function Composer.Translate(noCache)
    if not (ui and conv) then
        return
    end
    local st = State(conv)
    local text = strtrim(ui.box:GetText() or "")
    local draft
    if text ~= "" and st.mode ~= PASTED and st.mode ~= WAITING then
        draft = text
    elseif HasReply(conv) then
        draft = reply.draft
    end
    if not draft or draft == "" then
        SetFlash(L.RPL_STEP_WRITE_FIRST, Orange())
        Refresh()
        return
    end
    local lang = ReplyLang(conv)
    if not ns.Lang.IsTarget(lang) then
        SetFlash(L.RPL_STEP_PICK_LANG, NORMAL_FONT_COLOR)
        Refresh()
        LangPopup(function()
            Composer.Translate(noCache)
        end)
        return
    end
    if lang == ns.db.target then
        return
    end
    if ns.Consent.Mode() == "ask" then
        ui.box:ClearFocus()
    end
    local c = conv
    ns.Consent.Ask(function()
        Start(c, draft, lang, true, noCache)
    end, function()
        Start(c, draft, lang, false, noCache)
    end, host and host:GetParent())
end

-- Drops the pending reply: CANCEL on the reply slot (the Helper empties the clipboard if it
-- still holds its own text), the card goes.
function Composer.CancelReply()
    if not reply then
        return
    end
    local r = reply
    reply = nil
    r.conv.pendingReply = nil
    counters.cancelled = counters.cancelled + 1
    ns.Bridge.Send(ns.Bridge.CancelMessage(r.reqId, Slot()))
    local st = states[r.conv]
    if st and st.mode == WAITING then
        st.mode = EMPTY
    end
    HideCard()
    SyncView()
    Refresh()
end

-- [Edit]: the draft goes back into the box and the reply is cancelled.
function Composer.EditReply()
    if not HasReply(conv) then
        return
    end
    local draft = reply.draft
    Composer.CancelReply()
    SetBox(draft)
    State(conv).mode = DRAFT
    conv.draft = draft
    ui.box:SetFocus()
    Refresh()
end

---------------------------------------------------------------------------- send
-- A click on a channel button (a hardware event). Sends the box text to that target.
function Composer.SendTo(id)
    if not (ui and conv and id) then
        return
    end
    local now = GetTime()
    if now - lastSendClick < REPEAT_SECONDS then
        return
    end
    local st = State(conv)
    local text = ui.box:GetText() or ""
    local wire, bytes = ns.Channels.Wire(text)
    if not wire then
        Refresh()
        return
    end
    if ns.Channels.Locked() then
        counters.refused = counters.refused + 1
        flash = nil
        Refresh()
        return
    end
    if bytes > ns.Channels.MAX_BYTES then
        Refresh()
        return
    end
    local warn = Warning(st, text)
    if warn and not (armed and armed.id == id and now - armed.at <= ARM_SECONDS) then
        armed = { id = id, at = now }
        counters.warned = counters.warned + 1
        SetFlash(warn, Orange(), ARM_SECONDS)
        Refresh()
        return
    end
    armed = nil
    lastSendClick = now
    local target = ns.Channels.Find(conv, id)
    local label = target and target.label or "?"
    local draft = (st.mode == PASTED and HasReply(conv)) and reply.draft or nil
    local ok, why = ns.Channels.Send(conv, id, wire, { draft = draft })
    if not ok then
        if why ~= "RPL_WHY_REPEAT" then
            counters.refused = counters.refused + 1
            SetFlash(L.RPL_STEP_FAILED:format(WhyText(why, bytes - ns.Channels.MAX_BYTES)), RED_FONT_COLOR)
            Refresh()
        end
        return
    end
    counters.sent = counters.sent + 1
    local c = conv
    if HasReply(c) then
        Composer.CancelReply()
    end
    SetBox("")
    c.draft = nil
    ui.box:ClearFocus()
    Conv("MarkReplied", c)
    c.channelChoice = id
    st.mode, st.sentLabel, st.sentAt = SENT, label, now
    flash = nil
    C_Timer.After(SENT_SECONDS, function()
        if st.mode == SENT and st.sentAt == now then
            st.mode = EMPTY
            Refresh()
        end
    end)
    Refresh()
end

local function OtherPopup()
    if not conv then
        return
    end
    local c = conv
    local items = {}
    for _, t in ipairs(ns.Channels.Others(c)) do
        if t.separator and #items > 0 then
            items[#items + 1] = { separator = true }
        end
        items[#items + 1] = { text = t.label, disabled = not t.enabled, tooltip = t.reason and WhyText(t.reason),
            onClick = function()
                c.channelChoice = t.id
                if c == conv then
                    SetFlash(L.RPL_STEP_CHOSEN:format(t.label), NORMAL_FONT_COLOR)
                    Refresh()
                end
            end }
    end
    OpenPopup(ui.other, items)
end

---------------------------------------------------------------------------- box scripts
-- /tpr test paste (G12, G13): prints what the reply box reports, as counts only (never text).
local probe = false
local function Probe(box, what, userInput, extra)
    if not probe then
        return
    end
    local text = box:GetText() or ""
    local doubled = select(2, text:gsub("||", ""))
    local single = select(2, text:gsub("||", ""):gsub("|", ""))
    ns.Printf(L.RPL_PROBE, what, userInput == nil and "-" or (userInput and L.YES or L.NO),
        IsControlKeyDown() and L.YES or L.NO, #text, text:find("[\r\n]") and L.YES or L.NO, single, doubled,
        extra or 0)
end

local function OnTextChanged(box, userInput)
    Probe(box, "OnTextChanged", userInput)
    if suppress or not conv then
        Refresh()
        return
    end
    local st = State(conv)
    local text = box:GetText() or ""
    local prev = st.lastText or ""
    st.lastText = text
    if not userInput then
        -- changed by code outside the composer: an empty box starts over, text is a draft
        if text == "" then
            st.mode = HasReply(conv) and WAITING or (st.mode == SENT and SENT or EMPTY)
        elseif st.mode == EMPTY or st.mode == SENT then
            st.mode = DRAFT
        end
    else
        lastKeyAt = GetTime()
        armed = nil
        if flash then
            flash = nil
        end
        if text == "" then
            st.mode = HasReply(conv) and WAITING or EMPTY
        elseif st.mode == WAITING then
            -- a paste: Ctrl held (G12 checks it), or 2+ characters at once into the emptied box
            if IsControlKeyDown() or (prev == "" and Utf8Len(text) - Utf8Len(prev) >= 2) then
                if text:find("[\r\n]") then
                    text = text:match("^[\r\n]*([^\r\n]*)") or ""
                    SetBox(text)
                    SetFlash(L.RPL_STEP_ONE_LINE, Orange())
                end
                st.mode, st.pastedAt = PASTED, GetTime()
                counters.pasted = counters.pasted + 1
            else
                st.mode = DRAFT
            end
        elseif st.mode ~= PASTED then
            st.mode = DRAFT
        end
    end
    conv.draft = text ~= "" and text or nil
    Refresh()
end

local function OnEnterPressed(box)
    if not conv then
        box:ClearFocus()
        return
    end
    local st = State(conv)
    if st.mode == WAITING then
        SetFlash(L.RPL_STEP_PASTE_FIRST, Orange())
    elseif st.mode == DRAFT and not SameLang(conv) then
        Composer.Translate(IsShiftKeyDown())
        return
    elseif st.mode == DRAFT or st.mode == PASTED then
        box:ClearFocus()
        SetFlash(L.RPL_STEP_CLICK_CHANNEL, NORMAL_FONT_COLOR)
    else
        box:ClearFocus()
    end
    Refresh()
end

local function OnArrowPressed(box, key)
    if key ~= "UP" or not conv or (box:GetText() or "") ~= "" then
        return
    end
    local st = State(conv)
    local draft = st.lastDraft or (HasReply(conv) and reply.draft)
    if draft then
        SetBox(draft)
        st.mode = DRAFT
        conv.draft = draft
        Refresh()
    end
end

---------------------------------------------------------------------------- building
local function Layout()
    local root = ui.root
    root:SetParent(host)
    root:ClearAllPoints()
    root:SetAllPoints(host)
    ui.stepper:ClearAllPoints()
    ui.stepper:SetPoint("BOTTOMLEFT", root, "BOTTOMLEFT", 16, 68)
    ui.stepper:SetPoint("BOTTOMRIGHT", root, "BOTTOMRIGHT", -14, 68)
    ui.stepper:SetHeight(14)
    ui.box:ClearAllPoints()
    ui.box:SetPoint("BOTTOMLEFT", root, "BOTTOMLEFT", 20, 40)
    ui.box:SetPoint("BOTTOMRIGHT", root, "BOTTOMRIGHT", -150, 40)
    ui.box:SetHeight(22)
    ui.lang:ClearAllPoints()
    ui.lang:SetPoint("BOTTOMRIGHT", root, "BOTTOMRIGHT", -12, 40)
    ui.translate:ClearAllPoints()
    ui.translate:SetPoint("RIGHT", ui.lang, "LEFT", -2, 0)
    ui.sendLabel:ClearAllPoints()
    ui.sendLabel:SetPoint("BOTTOMLEFT", root, "BOTTOMLEFT", 14, 15)
end

local function Build()
    ui = { buttons = {} }
    local root = CreateFrame("Frame", nil, host)
    ui.root = root

    ui.measure = root:CreateFontString(nil, "ARTWORK")
    ui.measure:Hide()

    ui.stepper = root:CreateFontString(nil, "ARTWORK", "GameFontHighlightSmall")
    ui.stepper:SetJustifyH("LEFT")
    ui.stepper:SetWordWrap(false)

    local box = CreateFrame("EditBox", nil, root, "InputBoxInstructionsTemplate")
    box:SetAutoFocus(false)
    box:SetMaxBytes(1024)
    box:SetFontObject(ReplyFont())
    box:SetTextInsets(0, 44, 0, 0)
    box:HookScript("OnTextChanged", OnTextChanged)
    box:SetScript("OnEnterPressed", OnEnterPressed)
    box:SetScript("OnEscapePressed", function(self)
        self:ClearFocus()       -- the second Escape (no focus) closes the window
    end)
    box:SetScript("OnTabPressed", function()
        Frame("CycleTab", IsShiftKeyDown() and -1 or 1)
    end)
    box:SetScript("OnArrowPressed", OnArrowPressed)
    box:SetScript("OnChar", function(self, ch)
        lastKeyAt = GetTime()
        Probe(self, "OnChar", nil, type(ch) == "string" and #ch or 0)
    end)
    ui.box = box
    ui.bytes = box:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
    ui.bytes:SetPoint("RIGHT", box, "RIGHT", -6, 0)

    local translate = CreateFrame("Button", nil, root, "UIPanelButtonTemplate")
    translate:SetSize(112, 22)
    translate:SetMotionScriptsWhileDisabled(true)
    translate:SetScript("OnClick", function()
        Composer.Translate(IsShiftKeyDown())
    end)
    translate:SetScript("OnEnter", function(self)
        self.tipTitle = self:GetText()
        self.tipLines = { { L.RPL_TIP_TRANSLATE, HIGHLIGHT_FONT_COLOR }, { L.RPL_TIP_TRANSLATE_SHIFT, GREEN_FONT_COLOR } }
        ShowButtonTip(self)
    end)
    translate:SetScript("OnLeave", GameTooltip_Hide)
    ui.translate = translate

    local lang = CreateFrame("Button", nil, root, "UIPanelButtonTemplate")
    lang:SetSize(22, 22)
    local arrow = lang:CreateTexture(nil, "ARTWORK")
    arrow:SetTexture("Interface\\Buttons\\Arrow-Down-Up")
    arrow:SetSize(12, 12)
    arrow:SetPoint("CENTER", lang, "CENTER", 0, -2)
    lang.arrow = arrow
    lang:SetScript("OnClick", function()
        LangPopup(nil)
    end)
    lang.tipTitle, lang.tipLines = L.RPL_TIP_LANG, {}
    lang:SetScript("OnEnter", ShowButtonTip)
    lang:SetScript("OnLeave", GameTooltip_Hide)
    ui.lang = lang

    ui.sendLabel = root:CreateFontString(nil, "ARTWORK", "GameFontNormal")
    ui.sendLabel:SetText(L.RPL_SEND_TO)

    local other = CreateFrame("Button", nil, root, "UIPanelButtonTemplate")
    other:SetHeight(22)
    other:SetText(L.RPL_OTHER)
    other:SetWidth(math.max(48, TextWidth(other) + 24))
    other:SetScript("OnClick", OtherPopup)
    other.tipTitle, other.tipLines = L.RPL_OTHER, { { L.RPL_TIP_OTHER, HIGHLIGHT_FONT_COLOR } }
    other:SetScript("OnEnter", ShowButtonTip)
    other:SetScript("OnLeave", GameTooltip_Hide)
    ui.other = other
end

---------------------------------------------------------------------------- interface
-- Puts the composer in host (ConvFrame.composerHost: full window width, bottom-anchored).
-- Calling it again with another frame moves the composer there.
function Composer.Attach(newHost)
    if not newHost or newHost == host then
        return
    end
    host = newHost
    if not ui then
        Build()
    end
    ReleaseCard()
    Layout()
    local c = conv
    conv = nil
    Composer.SetConversation(c)
end

-- The conversation the composer works for (nil: none). Its draft comes back in the box.
function Composer.SetConversation(c)
    if c ~= nil and type(c) ~= "table" then
        c = nil
    end
    if c == conv then
        if ui then
            ShowCard()
            Refresh()
        end
        return
    end
    conv = c
    armed, flash = nil, nil
    if not ui then
        return
    end
    local text = c and Plain(c.draft) and c.draft or ""
    SetBox(text)
    if c then
        local st = State(c)
        if text == "" then
            if HasReply(c) then
                st.mode = WAITING
            elseif st.mode ~= SENT then
                st.mode = EMPTY
            end
        elseif st.mode ~= PASTED then
            st.mode = DRAFT
        end
    end
    ShowCard()
    Refresh()
end

-- Height in window units: 88, plus the reply card and its gap while it is shown.
function Composer.Height()
    if cardH > 0 and HasReply(conv) then
        return BASE_HEIGHT + cardH + CARD_GAP
    end
    return BASE_HEIGHT
end

-- The reply card's rectangle in GetScaledRect units (for VIEW 200), or nil without a card.
function Composer.ReplyCardRect()
    if not (card and cardH > 0 and HasReply(conv) and card:IsShown()) then
        return nil
    end
    if type(card.CardRect) == "function" then
        return card:CardRect()
    end
    return card:GetScaledRect()
end

-- Puts the cursor in the reply box (the "reply" key binding, or [T] with convFocus on).
function Composer.Focus()
    if ui and conv and ui.box:IsVisible() then
        ui.box:SetFocus()
    end
end

-- Re-reads the settings (text size) and redraws.
function Composer.ApplySettings()
    if not ui then
        return
    end
    ui.box:SetFontObject(ReplyFont())
    if HasReply(conv) then
        ShowCard(true)
    end
    Refresh()
end

-- Forgets drafts, states and the pending reply (/tpr conv clear).
function Composer.Reset()
    Composer.CancelReply()
    for c in pairs(states) do
        states[c] = nil
        if type(c) == "table" then
            c.draft = nil
        end
    end
    armed, flash = nil, nil
    if ui then
        SetBox("")
        if ui.box:HasFocus() then
            ui.box:ClearFocus()
        end
    end
    Refresh()
end

-- A conversation was closed: its pending reply is cancelled (the draft stays with it).
function Composer.OnClosed(c)
    if c and HasReply(c) then
        Composer.CancelReply()
    end
end

-- Redraws the stepper, the box and the channel buttons (a new line may change the default
-- channel; a group or channel change their availability).
function Composer.Refresh()
    if ui then
        Refresh()
    end
end

-- /tpr test paste: turns the reply box probe on or off; returns the new state.
function Composer.ToggleProbe()
    probe = not probe
    return probe
end

-- The conversation the pending reply belongs to, or nil (Conversations keeps it).
function Composer.PendingConv()
    return reply and reply.conv or nil
end

-- For tests and /tpr debug.
function Composer.Mode()
    return conv and State(conv).mode or nil
end
function Composer.Reply()
    return reply
end
function Composer.Widgets()
    return ui
end
function Composer.Host()
    return host
end
function Composer.Conversation()
    return conv
end
function Composer.Counters()
    local c = {}
    for k, v in pairs(counters) do
        c[k] = v
    end
    c.pending = reply ~= nil
    return c
end

---------------------------------------------------------------------------- events
local events = CreateFrame("Frame")
events:SetScript("OnEvent", function(_, event)
    if event == "PLAYER_REGEN_DISABLED" then
        -- dropping the focus mid-word would turn the next letters into ability keys
        if ui and ui.box:HasFocus() and GetTime() - lastKeyAt > FOCUS_KEEP_SECONDS then
            ui.box:ClearFocus()
        end
        return
    end
    if ui and host and host:IsVisible() then
        Refresh()
    end
end)
for _, event in ipairs({ "PLAYER_REGEN_DISABLED", "ADDON_RESTRICTION_STATE_CHANGED", "GROUP_ROSTER_UPDATE",
    "PLAYER_GUILD_UPDATE", "CHANNEL_UI_UPDATE", "CHAT_MSG_CHANNEL_NOTICE", "UPDATE_CHAT_COLOR" }) do
    events:RegisterEvent(event)
end

-- Follows the window's conversation list (WP-A's Conversations events) and attaches to the
-- window's host when it already exists.
ns.OnLoad(function()
    local C = ns.Conv
    if C and type(C.On) == "function" then
        local function Selected(arg)
            if type(arg) ~= "table" then
                arg = Conv("Selected")
            end
            Composer.SetConversation(arg)
        end
        C.On("select", Selected)
        C.On("change", function(arg, what)
            -- every conversation forgotten (Conv.Clear): the pending reply goes with them
            if what == "clear" and reply then
                Composer.CancelReply()
            elseif what == "evict" and reply and reply.conv == arg then
                Composer.OnClosed(arg)
            end
            Composer.Refresh()
        end)
        C.On("close", function(arg)
            if type(arg) == "table" then
                Composer.OnClosed(arg)
            elseif reply and reply.conv and reply.conv.key == arg then
                Composer.OnClosed(reply.conv)
            end
        end)
    end
    if ns.ConvFrame and ns.ConvFrame.composerHost then
        Composer.Attach(ns.ConvFrame.composerHost)
    end
end)
