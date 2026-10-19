-- Bubbles of the conversation window: incoming messages (Helper-painted rows), my messages
-- (addon text) and the reply card of the composer.
--
-- An incoming bubble: a meta line (channel in its chat colour, time, an "Original" link), the
-- bubble itself (the ChatBubble nine-slice border, tinted with the channel colour, over an opaque
-- BUBBLE_RGB fill), its K rows (each a tag of 8 colour textures and a PLATE_RGB plate, touching,
-- at a pitch of exactly rowPx physical px), the tail, then the original (when shown) and the
-- glossary line (when glossShow is on). Untranslated ("plain", follow mode add), the bubble holds
-- the original drawn by the addon and a Translate button instead of rows.
-- My bubble: right-aligned, the sent text drawn by the addon (white, the window font), the
-- mirrored tail, "You wrote: ..." under it when the draft differs, and a meta line with the state.
-- The reply card: the same art without a tail; "Your text: ..." with Edit and cancel buttons, the
-- reply's rows (row 0 = the Helper's status), and the lane under the last row: one row of fill
-- with a hint the Helper covers when it paints the card (docs\BRIDGE.md section 8c).
--
-- Art (Bubble.Art, once): the ChatBubble layout and tail atlas of the 12.x UI (checked on the
-- Forever branch, NineSliceLayouts.lua), else the old Interface\Tooltips\ChatBubble-* backdrop,
-- else TooltipBackdropTemplate's border. The fill is always our own BUBBLE_RGB, so only the border
-- changes in a fallback. Nothing is stretched: the tail is drawn at its atlas size, mirrored with
-- SetTexCoord for my bubbles.
local _, ns = ...
local L = ns.L

local Bubble = {}
ns.Bubble = Bubble

local max, min = math.max, math.min

local META_H = 14               -- meta line above a bubble
local TAIL_SPACE = 12           -- room under a bubble for its tail
local FILL_UNDER = 6            -- the fill reaches this close to the bubble's edge, under the border art
local GAP = 6                   -- between two messages
local OUT_PAD_X, OUT_PAD_Y = 12, 10
local ORIGINAL_LINES = 2
local CARD_TOP, CARD_BOTTOM = 8, 8      -- reply card: margins above the draft line and below the lane
local CARD_DRAFT_H, CARD_GAP = 14, 4
Bubble.META_H, Bubble.GAP, Bubble.TAIL_SPACE = META_H, GAP, TAIL_SPACE

local pools = { ["in"] = {}, out = {}, reply = {} }
local art

---------------------------------------------------------------------------- shared art and font
local function Rgb255(t)
    return t[1] / 255, t[2] / 255, t[3] / 255
end

-- Which border the bubbles use: "chatbubble" (the 12.x ChatBubble nine-slice and tail atlas),
-- "legacy" (the old ChatBubble backdrop files) or "tooltip" (TooltipBackdropTemplate).
function Bubble.Art()
    if art then
        return art
    end
    if NineSliceUtil.GetLayout("ChatBubble") and C_Texture.GetAtlasInfo("ChatBubble-Tail") then
        art = "chatbubble"
    elseif C_XMLUtil.GetTemplateInfo("BackdropTemplate") then
        art = "legacy"
    else
        art = "tooltip"
    end
    return art
end

-- TranslateProbeConvFont: a copy of ChatFontNormal at the window's text size (convText, 12-20).
-- The reply composer's box uses it too.
local font
function Bubble.Font()
    if not font then
        font = CreateFont("TranslateProbeConvFont")
        font:CopyFontObject(ChatFontNormal)
    end
    local size = (ns.db and tonumber(ns.db.convText)) or 14
    size = min(20, max(12, math.floor(size + 0.5)))
    local path, oldSize, flags = font:GetFont()
    if oldSize ~= size then
        font:SetFont(path, size, flags or "")
    end
    return font
end

local function Snap(v, region)
    return PixelUtil.GetNearestPixelSize(v, region:GetEffectiveScale())
end

local function NoSnap(t)
    t:SetSnapToPixelGrid(false)
    t:SetTexelSnappingBias(0)
end

-- The border of a bubble (box), drawn above its fill.
local function MakeBorder(box)
    local kind = Bubble.Art()
    local border
    if kind == "chatbubble" then
        border = CreateFrame("Frame", nil, box)
        border:SetAllPoints(box)
        NineSliceUtil.ApplyLayoutByName(border, "ChatBubble")
        -- the layout's own Center is translucent: our fill replaces it
        if border.Center then
            border.Center:Hide()
        end
    elseif kind == "legacy" then
        border = CreateFrame("Frame", nil, box, "BackdropTemplate")
        border:SetAllPoints(box)
        border:SetBackdrop({ edgeFile = "Interface\\Tooltips\\ChatBubble-Backdrop", edgeSize = 16, tile = true,
            tileEdge = true })
    else
        border = CreateFrame("Frame", nil, box, "TooltipBackdropTemplate")
        border:SetAllPoints(box)
        border:SetBackdropColor(0, 0, 0, 0)
    end
    border.kind = kind
    border:SetFrameLevel(box:GetFrameLevel() + 1)
    return border
end

local function TintBorder(border, r, g, b)
    if border.kind == "chatbubble" then
        for _, piece in ipairs({ "TopLeftCorner", "TopRightCorner", "BottomLeftCorner", "BottomRightCorner", "TopEdge",
            "BottomEdge", "LeftEdge", "RightEdge" }) do
            if border[piece] then
                border[piece]:SetVertexColor(r, g, b, 1)
            end
        end
    else
        border:SetBackdropBorderColor(r, g, b, 1)
    end
    if border.Tail then
        border.Tail:SetVertexColor(r, g, b, 1)
    end
end

local function MakeTail(border, mirrored)
    local tail = border:CreateTexture(nil, "ARTWORK", nil, 7)
    if border.kind == "chatbubble" then
        tail:SetAtlas("ChatBubble-Tail", true)
    else
        tail:SetTexture("Interface\\Tooltips\\ChatBubble-Tail")
        tail:SetSize(16, 16)
    end
    if mirrored then
        tail:SetTexCoord(1, 0, 0, 1)    -- art-ok: the tail mirrored for my bubbles, not stretched
    end
    border.Tail = tail
    return tail
end

-- The opaque interior of a bubble: BUBBLE_RGB, under the border art up to FILL_UNDER of the edge.
local function MakeFill(box)
    local fill = box:CreateTexture(nil, "BACKGROUND")
    fill:SetColorTexture(Rgb255(ns.Bridge.BUBBLE_RGB))   -- art-ok: protocol colour
    fill:SetPoint("TOPLEFT", box, "TOPLEFT", FILL_UNDER, -FILL_UNDER)
    fill:SetPoint("BOTTOMRIGHT", box, "BOTTOMRIGHT", -FILL_UNDER, FILL_UNDER)
    NoSnap(fill)
    return fill
end

---------------------------------------------------------------------------- rows (tags and plates)
local function RowAt(self, r)
    local row = self.rowsList[r]
    if not row then
        row = { tag = {} }
        for i = 1, 8 do
            local t = self.rowFrame:CreateTexture(nil, "ARTWORK")
            NoSnap(t)
            row.tag[i] = t
        end
        local plate = self.rowFrame:CreateTexture(nil, "ARTWORK")
        plate:SetColorTexture(Rgb255(ns.Bridge.PLATE_RGB))   -- art-ok: protocol colour
        NoSnap(plate)
        row.plate = plate
        self.rowsList[r] = row
    end
    return row
end

local function ShowRow(row, on)
    for i = 1, 8 do
        row.tag[i]:SetShown(on)
    end
    row.plate:SetShown(on)
end

-- Tags and plates for K rows of slot (sessBit: ns.sessionId % 2). slot = nil keeps the room for
-- K rows with nothing drawn in it (a bubble whose slot was given back).
local function SetRows(self, slot, K, sessBit)
    self.slot, self.K, self.sessBit = slot, max(1, min(6, K or 1)), sessBit or 0
    for r = 1, self.K do
        local row = RowAt(self, r)
        if slot then
            ns.Bridge.ApplyTag(row.tag, slot, r - 1, self.sessBit)
        end
    end
    self.rowsDirty = true
end

-- Places the rows of a box: row r's top at rowTop0 + (r - 1) * rowH units from the box's top, the
-- tag at inset from its left. Returns the rows' height.
local function PlaceRows(self, m, rowTop0)
    local W, rowH = m.W, m.rowH
    local x = m.inset
    for r, row in ipairs(self.rowsList) do
        local on = r <= (self.K or 0) and self.slot ~= nil
        if r <= (self.K or 0) then
            local y = -(rowTop0 + (r - 1) * rowH)
            for i = 1, 8 do
                local t = row.tag[i]
                t:ClearAllPoints()
                t:SetPoint("TOPLEFT", self.box, "TOPLEFT", x + (i - 1) * W, y)
                t:SetSize(W, rowH)
            end
            row.plate:ClearAllPoints()
            row.plate:SetPoint("TOPLEFT", self.box, "TOPLEFT", x + 8 * W, y)
            row.plate:SetSize(m.plateWidth, rowH)
        end
        row.visible = on
        ShowRow(row, on and not row.clipped)
    end
    self.rowTop0, self.rowH = rowTop0, rowH
    return (self.K or 0) * rowH
end

-- Hides the tag and plate of every row that is not entirely inside [bottom, top] (GetScaledRect
-- units, y up): the Helper never sees a partly clipped row; the fill shows there.
local function ClipRows(self, top, bottom)
    local _, b, _, h = self.box:GetScaledRect()
    if not (b and h) or issecretvalue(b) then
        return
    end
    local eff = self.box:GetEffectiveScale()
    local boxTop = b + h
    local eps = 0.01
    for r, row in ipairs(self.rowsList) do
        if r <= (self.K or 0) then
            local rowTop = boxTop - (self.rowTop0 + (r - 1) * self.rowH) * eff
            local rowBottom = rowTop - self.rowH * eff
            row.clipped = top == nil or rowTop > top + eps or rowBottom < bottom - eps
            ShowRow(row, row.visible and not row.clipped)
        end
    end
end

-- Number of rows shown (tags drawn) right now, for tests and the slot manager.
local function ShownRows(self)
    local n = 0
    for r, row in ipairs(self.rowsList) do
        if r <= (self.K or 0) and row.plate:IsShown() then
            n = n + 1
        end
    end
    return n
end

---------------------------------------------------------------------------- common pieces
local function Text(parent, layer, fontObject)
    local fs = parent:CreateFontString(nil, layer or "ARTWORK")
    fs:SetFontObject(fontObject)
    fs:SetJustifyH("LEFT")
    fs:SetWordWrap(true)
    return fs
end

local function LinkButton(parent, fontObject)
    local b = CreateFrame("Button", nil, parent)
    local fs = b:CreateFontString(nil, "ARTWORK")
    fs:SetFontObject(fontObject or GameFontNormalSmall)
    fs:SetPoint("LEFT", b, "LEFT", 0, 0)
    b:SetFontString(fs)
    b:SetHeight(META_H)
    local hl = b:CreateTexture(nil, "HIGHLIGHT")
    hl:SetColorTexture(1, 1, 1, 0.15)
    hl:SetAllPoints(b)
    function b:SetLabel(text)
        fs:SetText(text)
        self:SetWidth(fs:GetUnboundedStringWidth() + 2)
    end
    return b
end

local function Fire(self, name, ...)
    local fn = self.handlers and self.handlers[name]
    if fn then
        fn(self, ...)
    end
end

-- The common block of incoming and outgoing messages: meta line, box with border and fill.
local function NewBlock(parent, kind)
    local self = CreateFrame("Frame", nil, parent)
    self.kind = kind
    self.rowsList = {}
    self.meta = Text(self, "ARTWORK", GameFontDisableSmall)
    self.meta:SetWordWrap(false)
    self.metaRight = Text(self, "ARTWORK", GameFontDisableSmall)
    self.metaRight:SetWordWrap(false)
    local box = CreateFrame("Button", nil, self)
    box:RegisterForClicks("LeftButtonUp", "RightButtonUp")
    self.box = box
    self.fill = MakeFill(box)
    self.border = MakeBorder(box)
    self.rowFrame = CreateFrame("Frame", nil, box)
    self.rowFrame:SetAllPoints(box)
    self.rowFrame:SetFrameLevel(box:GetFrameLevel() + 2)
    self.flash = self:CreateTexture(nil, "BACKGROUND")
    self.flash:SetColorTexture(1, 0.82, 0, 0.18)
    self.flash:SetPoint("TOPLEFT", self, "TOPLEFT", 0, 0)
    self.flash:SetPoint("RIGHT", self, "RIGHT", 0, 0)
    self.flash:SetHeight(META_H)
    self.flash:Hide()
    box:SetScript("OnClick", function(_, button)
        Fire(self, "click", button)
    end)
    box:SetScript("OnEnter", function()
        Fire(self, "enter")
    end)
    box:SetScript("OnLeave", function()
        Fire(self, "leave")
    end)
    self.SetRows, self.ClipRows, self.ShownRows = SetRows, ClipRows, ShownRows
    return self
end

local Methods = {}

function Methods:SetMeta(text, r, g, b)
    self.meta:SetText(text or "")
    if r then
        self.meta:SetTextColor(r, g, b)
    else
        self.meta:SetTextColor(GRAY_FONT_COLOR:GetRGB())
    end
end

-- A second part of the meta line (my bubbles: the state), in its own colour.
function Methods:SetMetaRight(text, r, g, b)
    self.metaRight:SetText(text or "")
    if r then
        self.metaRight:SetTextColor(r, g, b)
    end
end

function Methods:Tint(r, g, b)
    self.tint = { r, g, b }
    TintBorder(self.border, r, g, b)
end

-- A static highlight of the meta line for seconds (a [T] on a line already in the thread). It never
-- covers the rows, so the Helper keeps reading the tags.
function Methods:Flash(seconds)
    self.flash:Show()
    self.flashToken = (self.flashToken or 0) + 1
    local token = self.flashToken
    C_Timer.After(seconds or 1.5, function()
        if self.flashToken == token then
            self.flash:Hide()
        end
    end)
end

-- Shows or hides the rows again after SetRows changed the slot (same K, same layout).
function Methods:RefreshRows()
    if self.metrics and self.rowTop0 and not self.plainText then
        PlaceRows(self, self.metrics, self.rowTop0)
    end
end

function Methods:SetHandlers(handlers)
    self.handlers = handlers
end

local function Mixin2(obj)
    for k, fn in pairs(Methods) do
        obj[k] = fn
    end
    return obj
end

---------------------------------------------------------------------------- incoming
local function NewIn(parent)
    local self = Mixin2(NewBlock(parent, "in"))
    self.tail = MakeTail(self.border, false)
    self.originalLink = LinkButton(self, GameFontNormalSmall)
    self.originalLink:SetScript("OnClick", function()
        Fire(self, "original")
    end)
    self.original = Text(self, "ARTWORK", ChatFontSmall)
    self.original:SetTextColor(GRAY_FONT_COLOR:GetRGB())
    self.original:SetMaxLines(ORIGINAL_LINES)
    self.gloss = Text(self, "ARTWORK", GameFontDisableSmall)
    self.plain = Text(self.rowFrame, "OVERLAY", Bubble.Font())
    self.plain:SetTextColor(0.85, 0.85, 0.85)
    self.translateBtn = LinkButton(self.rowFrame, GameFontNormalSmall)
    self.translateBtn:SetScript("OnClick", function()
        Fire(self, "translate")
    end)
    return self
end

-- The original under the bubble: text, and whether it is shown (the link reads Original or Hide).
function Methods:SetOriginal(text, shown)
    self.originalText = text
    self.originalShown = shown and true or false
end

function Methods:SetGloss(text)
    self.glossText = text
end

-- Untranslated: the original drawn inside the bubble with a Translate button (text = nil: the
-- bubble shows its rows).
function Methods:SetPlain(text)
    self.plainText = text
end

-- Lays the block out for a content width and the window metrics; returns its height. x positions
-- are from the content's left edge (the block spans the whole content width).
local function LayoutIn(self, width, m)
    local fontObj = Bubble.Font()
    self:SetWidth(width)
    local bw = m.bubbleWidth
    local y = 0
    -- meta line: channel and time, then the Original link
    self.meta:ClearAllPoints()
    self.meta:SetPoint("TOPLEFT", self, "TOPLEFT", m.margin + 4, -y)
    self.meta:SetHeight(META_H)
    self.originalLink:ClearAllPoints()
    local hasOriginal = self.plainText == nil and self.originalText ~= nil
    if hasOriginal then
        self.originalLink:SetLabel(self.originalShown and L.CONV_HIDE or L.CONV_ORIGINAL)
        self.originalLink:SetPoint("LEFT", self.meta, "LEFT", self.meta:GetUnboundedStringWidth() + 8, 0)
        self.originalLink:Show()
    else
        self.originalLink:Hide()
    end
    y = y + META_H + 2
    -- the bubble
    local box = self.box
    box:ClearAllPoints()
    box:SetPoint("TOPLEFT", self, "TOPLEFT", m.margin, -Snap(y, self))
    box:SetWidth(bw)
    local boxH
    if self.plainText then
        for _, row in ipairs(self.rowsList) do
            row.visible = false
            ShowRow(row, false)
        end
        local textW = bw - 2 * m.inset
        self.plain:SetFontObject(fontObj)
        self.plain:ClearAllPoints()
        self.plain:SetPoint("TOPLEFT", box, "TOPLEFT", m.inset, -m.inset)
        self.plain:SetWidth(textW)
        self.plain:SetText(self.plainText)
        self.plain:Show()
        local textH = self.plain:GetStringHeight()
        self.translateBtn:SetLabel(L.CONV_TRANSLATE)
        self.translateBtn:ClearAllPoints()
        self.translateBtn:SetPoint("TOPLEFT", self.plain, "BOTTOMLEFT", 0, -4)
        self.translateBtn:Show()
        boxH = 2 * m.inset + textH + 4 + META_H
    else
        self.plain:Hide()
        self.translateBtn:Hide()
        local rowTop0 = Snap(m.inset, self)
        boxH = 2 * rowTop0 + PlaceRows(self, m, rowTop0)
    end
    self.metrics = m
    boxH = Snap(boxH, self)
    box:SetHeight(boxH)
    y = y + boxH
    -- tail
    self.tail:ClearAllPoints()
    self.tail:SetPoint("TOPLEFT", box, "BOTTOMLEFT", 16, 4)
    y = y + TAIL_SPACE
    -- original and glossary lines
    if hasOriginal and self.originalShown then
        self.original:ClearAllPoints()
        self.original:SetPoint("TOPLEFT", self, "TOPLEFT", m.margin + 8, -y)
        self.original:SetWidth(bw - 16)
        self.original:SetText(self.originalText)
        self.original:Show()
        y = y + self.original:GetStringHeight() + 2
    else
        self.original:Hide()
    end
    if self.glossText then
        self.gloss:ClearAllPoints()
        self.gloss:SetPoint("TOPLEFT", self, "TOPLEFT", m.margin + 8, -y)
        self.gloss:SetWidth(bw - 16)
        self.gloss:SetText(self.glossText)
        self.gloss:Show()
        y = y + self.gloss:GetStringHeight() + 2
    else
        self.gloss:Hide()
    end
    y = y + GAP
    self:SetHeight(y)
    return y
end

---------------------------------------------------------------------------- mine
local function NewOut(parent)
    local self = Mixin2(NewBlock(parent, "out"))
    self.tail = MakeTail(self.border, true)
    self.text = Text(self.rowFrame, "OVERLAY", Bubble.Font())
    self.text:SetTextColor(1, 1, 1)
    self.wrote = Text(self, "ARTWORK", ChatFontSmall)
    self.wrote:SetTextColor(GRAY_FONT_COLOR:GetRGB())
    self.wrote:SetMaxLines(ORIGINAL_LINES)
    self.wrote:SetJustifyH("RIGHT")
    self.meta:SetJustifyH("RIGHT")
    return self
end

-- My bubble: the exact text sent (or typed), and the draft it was translated from (shown as "You
-- wrote: ..." when it differs).
function Methods:SetOut(text, draft)
    self.outText, self.draftText = text or "", draft
end

local function LayoutOut(self, width, m)
    local fontObj = Bubble.Font()
    self:SetWidth(width)
    local right = width - m.margin
    local y = 0
    self.metaRight:ClearAllPoints()
    self.metaRight:SetPoint("TOPRIGHT", self, "TOPRIGHT", -m.margin - 4, -y)
    self.meta:ClearAllPoints()
    self.meta:SetPoint("RIGHT", self.metaRight, "LEFT", -6, 0)
    self.meta:SetHeight(META_H)
    y = y + META_H + 2
    self.text:SetFontObject(fontObj)
    self.text:SetWidth(0)
    self.text:SetText(self.outText or "")
    local maxText = m.bubbleWidth - 2 * OUT_PAD_X
    local textW = min(maxText, self.text:GetUnboundedStringWidth() + 1)
    self.text:SetWidth(textW)
    local textH = self.text:GetStringHeight()
    local boxW = textW + 2 * OUT_PAD_X
    local box = self.box
    box:ClearAllPoints()
    box:SetPoint("TOPLEFT", self, "TOPLEFT", right - boxW, -Snap(y, self))
    box:SetWidth(boxW)
    local boxH = Snap(textH + 2 * OUT_PAD_Y, self)
    box:SetHeight(boxH)
    self.text:ClearAllPoints()
    self.text:SetPoint("TOPLEFT", box, "TOPLEFT", OUT_PAD_X, -OUT_PAD_Y)
    y = y + boxH
    self.tail:ClearAllPoints()
    self.tail:SetPoint("TOPRIGHT", box, "BOTTOMRIGHT", -16, 4)
    y = y + TAIL_SPACE
    if self.draftText and self.draftText ~= "" and self.draftText ~= self.outText then
        self.wrote:ClearAllPoints()
        self.wrote:SetPoint("TOPRIGHT", self, "TOPRIGHT", -m.margin - 8, -y)
        self.wrote:SetWidth(m.bubbleWidth - 16)
        self.wrote:SetText(L.CONV_YOU_WROTE:format(self.draftText))
        self.wrote:Show()
        y = y + self.wrote:GetStringHeight() + 2
    else
        self.wrote:Hide()
    end
    y = y + GAP
    self:SetHeight(y)
    return y
end

---------------------------------------------------------------------------- reply card
local function NewReply(parent)
    local self = CreateFrame("Frame", nil, parent)
    Mixin2(self)
    self.kind = "reply"
    self.rowsList = {}
    self.box = self
    self.fill = MakeFill(self)
    self.border = MakeBorder(self)
    self.rowFrame = CreateFrame("Frame", nil, self)
    self.rowFrame:SetAllPoints(self)
    self.rowFrame:SetFrameLevel(self:GetFrameLevel() + 2)
    self.draftLabel = Text(self.rowFrame, "OVERLAY", GameFontDisableSmall)
    self.draftLabel:SetWordWrap(false)
    self.draftText = Text(self.rowFrame, "OVERLAY", GameFontHighlightSmall)
    self.draftText:SetWordWrap(false)
    self.EditButton = LinkButton(self.rowFrame, GameFontNormalSmall)
    self.EditButton:SetLabel(L.CONV_EDIT)
    self.CancelButton = CreateFrame("Button", nil, self.rowFrame, "UIPanelCloseButton")
    self.CancelButton:SetSize(16, 16)
    self.CancelButton:SetScript("OnClick", nil)
    self.CancelButton:SetScript("OnEnter", function(b)
        GameTooltip:SetOwner(b, "ANCHOR_RIGHT")
        GameTooltip_SetTitle(GameTooltip, L.CONV_CANCEL_TIP)
        GameTooltip:Show()
    end)
    self.CancelButton:SetScript("OnLeave", GameTooltip_Hide)
    self.lane = Text(self.rowFrame, "OVERLAY", GameFontDisableSmall)
    self.lane:SetWordWrap(false)
    self.laneText = L.CONV_LANE_HINT
    self.SetRows, self.ClipRows, self.ShownRows = SetRows, ClipRows, ShownRows
    self.K = 2
    return self
end

-- "Your text: " .. text on the card's first line.
function Methods:SetDraft(text)
    self.draft = text or ""
end

function Methods:SetLaneHint(text)
    self.laneText = text or ""
end

local function LayoutReply(self, width, m)
    width = width or m.bubbleWidth
    self:SetWidth(width)
    local y = CARD_TOP
    self.draftLabel:ClearAllPoints()
    self.draftLabel:SetPoint("TOPLEFT", self, "TOPLEFT", m.inset, -y)
    self.draftLabel:SetText(L.CONV_YOUR_TEXT)
    self.CancelButton:ClearAllPoints()
    self.CancelButton:SetPoint("TOPRIGHT", self, "TOPRIGHT", -m.border, -y + 1)
    self.EditButton:ClearAllPoints()
    self.EditButton:SetPoint("RIGHT", self.CancelButton, "LEFT", -6, 0)
    self.draftText:ClearAllPoints()
    self.draftText:SetPoint("LEFT", self.draftLabel, "RIGHT", 4, 0)
    self.draftText:SetPoint("RIGHT", self.EditButton, "LEFT", -6, 0)
    self.draftText:SetText(self.draft or "")
    y = y + CARD_DRAFT_H + CARD_GAP
    local rowTop0 = Snap(y, self)
    local rowsH = PlaceRows(self, m, rowTop0)
    -- the lane: one row of fill right under the last row, the hint clear of its top and bottom 2 px
    self.lane:ClearAllPoints()
    self.lane:SetPoint("TOPLEFT", self, "TOPLEFT", m.inset + 8 * m.W, -(rowTop0 + rowsH + 2))
    self.lane:SetWidth(m.plateWidth)
    self.lane:SetHeight(max(1, m.rowH - 4))
    self.lane:SetText(self.laneText or "")
    self.laneTop = rowTop0 + rowsH
    local h = Snap(rowTop0 + rowsH + m.rowH + CARD_GAP + CARD_BOTTOM, self)
    self:SetHeight(h)
    self.metrics = m
    return h
end

-- The card's clean fill (inside the border art) in GetScaledRect units: l, b, w, h, or nil when
-- hidden. VIEW 200 reaches down to its bottom.
function Methods:CardRect()
    if not self:IsVisible() then
        return nil
    end
    local l, b, w, h = self:GetScaledRect()
    if not l or issecretvalue(l) then
        return nil
    end
    local inset = ((self.metrics and self.metrics.border) or 10) * self:GetEffectiveScale()
    return l + inset, b + inset, w - 2 * inset, h - 2 * inset
end

---------------------------------------------------------------------------- pools
local function Acquire(kind, parent, make)
    local pool = pools[kind]
    local b = table.remove(pool)
    if b then
        b:SetParent(parent)
    else
        b = make(parent)
    end
    b.inUse = true
    b:Show()
    return b
end

function Bubble.AcquireIn(parent)
    local b = Acquire("in", parent, NewIn)
    b.Layout = LayoutIn
    b.handlers, b.plainText, b.originalText, b.glossText, b.originalShown = nil, nil, nil, nil, false
    b.slot, b.K = nil, 1
    return b
end

function Bubble.AcquireOut(parent)
    local b = Acquire("out", parent, NewOut)
    b.Layout = LayoutOut
    b.handlers, b.outText, b.draftText = nil, nil, nil
    return b
end

-- The reply card (one per window): the composer shows, lays out and hides it.
function Bubble.AcquireReply(parent)
    local b = Acquire("reply", parent, NewReply)
    b.Layout = LayoutReply
    return b
end

function Bubble.Release(b)
    if not b or not b.inUse then
        return
    end
    b.inUse = false
    b:Hide()
    b:ClearAllPoints()
    b.handlers, b.msg = nil, nil
    b.slot = nil
    for _, row in ipairs(b.rowsList) do
        row.visible, row.clipped = false, false
        ShowRow(row, false)
    end
    if b.flash then
        b.flash:Hide()
    end
    local pool = pools[b.kind]
    pool[#pool + 1] = b
end

-- Bubbles in use and pooled, for tests and /tpr debug.
function Bubble.Counts()
    return #pools["in"], #pools.out, #pools.reply
end

-- Measures text in the window font, unwrapped (for the row estimate).
local measure
function Bubble.TextWidth(text)
    if not measure then
        local f = CreateFrame("Frame", nil, UIParent)
        f:Hide()
        measure = f:CreateFontString(nil, "ARTWORK")
    end
    measure:SetFontObject(Bubble.Font())
    measure:SetWidth(0)
    measure:SetText(text or "")
    return measure:GetUnboundedStringWidth()
end

