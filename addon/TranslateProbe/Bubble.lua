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
-- Incoming bubbles have the same lane under their rows (user request U1): "No translation above?
-- Start Translate Companion." and a [Download] button that opens the Translate Companion dialog,
-- shown 1.5 s after the lane became visible for the card's request (see "the hint lane" below).
-- A message with hyperlinks (items, spells, professions, map pins...) gets a links bar inside its
-- bubble, under the lane (under the Translate button of an untranslated bubble): its links in their
-- own colours, clickable as in the chat (Links.lua), since the Helper's painted text is not; and
-- its Original text has its hyperlinks enabled the same way (user request L2, 2026-10-19).
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
local LANE_EDGE, LANE_GAP = 2, 4        -- the lane's text: clear of its top and bottom 2 units; 4 before [Download]
local LANE_EDGE_PX = 2                  -- ...and 2 physical px at least (the Helper reads the outer 2 px rows)
local LANE_DELAY = 1.5                  -- seconds before a lane's hint shows (Reserve.LANE_DELAY)
local LINK_COLOR = "|cff66bbff"         -- our links' light blue ([T], [Show them], [Download])
local BAR_GAP, BAR_GAP_PX = 4, 4        -- the links bar: 4 units and 4 physical px under the lane at least
local BAR_SPACE = 6                     -- between two links of the bar
Bubble.META_H, Bubble.GAP, Bubble.TAIL_SPACE = META_H, GAP, TAIL_SPACE
Bubble.LANE_DELAY, Bubble.LANE_EDGE, Bubble.LANE_GAP = LANE_DELAY, LANE_EDGE, LANE_GAP
Bubble.BAR_SPACE = BAR_SPACE

local pools = { ["in"] = {}, out = {}, reply = {} }
local live = {}                         -- cards in use that have a lane (incoming bubbles, the reply card)
local moving = false                    -- the window is being dragged or resized (Bubble.SetMoving)
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
local UpdateLane
local function ClipRows(self, top, bottom)
    local _, b, _, h = self.box:GetScaledRect()
    if not (b and h) or issecretvalue(b) then
        self.laneClipped = true
        UpdateLane(self)
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
    -- the lane: the same rule as a row
    if self.laneTop and self.rowH then
        local laneTop = boxTop - self.laneTop * eff
        self.laneClipped = top == nil or laneTop > top + eps or laneTop - self.rowH * eff < bottom - eps
    end
    UpdateLane(self)
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
local Text, LinkButton

---------------------------------------------------------------------------- the hint lane
-- One row of fill right under a card's last row: its band is [rowTop0 + K * rowH, + rowH] in the
-- card's units, so row0.yTop + K * rowPx physical px, from the tag's x0 (inset) to the plate's end
-- (inset + 8 W + plateWidth). The hint (TranslateProbeLaneFont: GameFontNormalSmall, the gold of
-- NORMAL_FONT_COLOR, smaller than its box) starts at the plate's x, LaneEdge units under the
-- lane's top (LANE_EDGE units and LANE_EDGE_PX physical px at least) and as high as the row less
-- 2 LaneEdge; [Download] (our links' light blue) follows LANE_GAP units after it, inside the
-- plate's width.
-- The longest hint that fits beside the button is used (the full one, then CONV_LANE_SHORT), and
-- the tag column of the lane is plain fill (the Helper samples the fill colour there).
-- The Helper paints the lane flat whenever it paints the card, so the hint is seen only when no
-- Helper paints. It shows LANE_DELAY seconds after the lane became ready for the card's current
-- request, slot, rows and place (C_Timer, no OnUpdate), so it never flashes before the Helper's
-- first paint. Ready: the card has a slot, is visible, the window is not being dragged or resized,
-- and its lane and last row are entirely inside the visible thread (the Helper finds the lane right
-- under that row). Hidden otherwise. The delay starts again whenever the lane moves on the screen
-- (a scroll, a new message above it, the window dragged or resized): the Helper finds it at its
-- new place one capture later, and only once the addon sends the new VIEW after a drag.
-- TranslateProbeLaneFont: a copy of GameFontNormalSmall (its gold included) one point smaller than
-- the lane's text box (h units: its 1-unit shadow fits too), so the hint always fits its box and
-- clears the lane's edges (at text size 12 the box is about 10 units high).
local laneFont, laneBaseSize
local function LaneFont(h)
    if not laneFont then
        laneFont = CreateFont("TranslateProbeLaneFont")
        laneFont:CopyFontObject(GameFontNormalSmall)
        laneBaseSize = select(2, laneFont:GetFont()) or 10
    end
    local path, size, flags = laneFont:GetFont()
    local want = max(6, min(laneBaseSize, math.floor(h) - 1))
    if size ~= want then
        laneFont:SetFont(path, want, flags or "")
    end
    return laneFont
end

-- [Download] opens the Translate Companion dialog. In an incoming bubble it sits over the bubble's
-- box (which the Helper's overlay does not hide from the mouse, even while it paints the lane
-- over): a right-click and the hover go on to the bubble (its menu, its tooltip), so only a left
-- click on [Download] itself differs from a click on the bubble.
local Fire
local function MakeLane(self)
    local hint = Text(self.rowFrame, "OVERLAY", GameFontNormalSmall)
    hint:SetWordWrap(false)
    hint:Hide()
    local button = LinkButton(self.rowFrame, GameFontNormalSmall)
    local forward = self.box ~= self
    if forward then
        button:RegisterForClicks("LeftButtonUp", "RightButtonUp")
    end
    button:SetScript("OnClick", function(_, which)
        if forward and which == "RightButton" then
            Fire(self, "click", which)
            return
        end
        local cf = ns.ConvFrame
        ns.CompanionDialog.Open(cf and cf.Frame and cf.Frame() or nil)
    end)
    if forward then
        button:SetScript("OnEnter", function()
            Fire(self, "enter")
        end)
        button:SetScript("OnLeave", function()
            Fire(self, "leave")
        end)
    end
    button:Hide()
    self.lane, self.laneButton, self.laneShown = hint, button, false
end

local function ShowLane(self, on)
    on = (on and self.laneFits) and true or false
    self.laneShown = on
    self.lane:SetShown(on and (self.lane:GetText() or "") ~= "")
    self.laneButton:SetShown(on)
end

-- The lane's text margin at its top and bottom, in the card's units: LANE_EDGE units and
-- LANE_EDGE_PX physical px at least.
local function LaneEdge(m)
    return max(LANE_EDGE, LANE_EDGE_PX / (m.ppu or 1))
end
Bubble.LaneEdge = LaneEdge

-- Places the lane laneTop units under the card's top (full: the hint for this kind of card).
local function PlaceLane(self, m, laneTop, full)
    if self.laneTop ~= laneTop then
        self.laneKey = nil      -- moved: the delay starts again
        ShowLane(self, false)
    end
    self.laneTop = laneTop
    local x = m.inset + 8 * m.W
    local edge = LaneEdge(m)
    local h = max(1, m.rowH - 2 * edge)
    local laneFontObj = LaneFont(h)
    local button = self.laneButton
    self.lane:SetFontObject(laneFontObj)
    button:GetFontString():SetFontObject(laneFontObj)
    button:SetLabel(LINK_COLOR .. "[" .. L.CONV_DOWNLOAD .. "]|r")
    button:SetHeight(h)
    local bw = button:GetWidth()
    local hint = self.lane
    local text = ""
    for _, candidate in ipairs({ full or "", L.CONV_LANE_SHORT }) do
        hint:SetText(candidate)
        if candidate ~= "" and hint:GetUnboundedStringWidth() + 2 + LANE_GAP + bw <= m.plateWidth then
            text = candidate
            break
        end
    end
    hint:SetText(text)
    local tw = text ~= "" and (hint:GetUnboundedStringWidth() + 2) or 0
    hint:ClearAllPoints()
    hint:SetPoint("TOPLEFT", self.box, "TOPLEFT", x, -(laneTop + edge))
    hint:SetWidth(max(1, tw))
    hint:SetHeight(h)
    button:ClearAllPoints()
    button:SetPoint("TOPLEFT", self.box, "TOPLEFT", x + (tw > 0 and tw + LANE_GAP or 0), -(laneTop + edge))
    self.laneFits = bw <= m.plateWidth
end

-- No lane (an untranslated bubble).
local function ClearLane(self)
    self.laneTop, self.laneKey, self.laneClipped = nil, nil, nil
    ShowLane(self, false)
end

local function LaneReady(self)
    if moving or not (self.laneTop and self.slot and not self.plainText and not self.laneClipped and self.box:IsVisible()) then
        return false
    end
    local last = self.rowsList[self.K or 0]
    return last ~= nil and last.visible == true and not last.clipped
end

-- Shows the hint once it has been ready for LANE_DELAY seconds with the same key, else hides it.
function UpdateLane(self)
    if not self.lane then
        return
    end
    if not LaneReady(self) then
        self.laneKey = nil
        ShowLane(self, false)
        return
    end
    -- the request, the slot, the rows, and the lane's place in the box and on the screen (physical
    -- px): any move starts the delay again
    local l, b, _, h = self.box:GetScaledRect()
    local where = "?"
    if l and b and h and not issecretvalue(b) then
        local okSize, _, physH = pcall(GetPhysicalScreenSize)
        local k = (okSize and type(physH) == "number" and physH > 0) and physH / 768 or 1
        where = math.floor(l * k + 0.5) .. "," .. math.floor((b + h) * k + 0.5)
    end
    local key = table.concat({ self.slot, self.K, tostring(self.laneReq), math.floor(self.laneTop * 64 + 0.5), where }, ":")
    if key ~= self.laneKey then
        self.laneKey, self.laneSince = key, GetTime()
        self.laneToken = (self.laneToken or 0) + 1
        local token = self.laneToken
        ShowLane(self, false)
        C_Timer.After(LANE_DELAY, function()
            if self.laneToken == token then
                UpdateLane(self)
            end
        end)
        return
    end
    ShowLane(self, GetTime() - self.laneSince >= LANE_DELAY - 0.01)
end

-- The window is being dragged or resized (on), or that ended (off): every lane hides while it
-- moves (the Helper's VIEW is stale until the move ends), then waits its delay again.
function Bubble.SetMoving(on)
    moving = on and true or false
    for b in pairs(live) do
        UpdateLane(b)
    end
end

function Text(parent, layer, fontObject)
    local fs = parent:CreateFontString(nil, layer or "ARTWORK")
    fs:SetFontObject(fontObject)
    fs:SetJustifyH("LEFT")
    fs:SetWordWrap(true)
    return fs
end

function LinkButton(parent, fontObject)
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

function Fire(self, name, ...)
    local fn = self.handlers and self.handlers[name]
    if fn then
        fn(self, ...)
    end
end

---------------------------------------------------------------------------- the links bar
-- The message's hyperlinks (Links.Parse: each once, in order), one button each, labelled with the
-- link's name in the colour the chat draws it in, in the window font. A click and the hover do what
-- they do in the chat (Links.Click, Links.ShowTooltip). The bar lies inside the bubble's fill from
-- the tag's x0 to the plate's end, BarGap under the lane (the Helper reads the lane and 2 px around
-- it: the bar never reaches them), its lines rowH apart; a name wider than the bar is cut.
local function BarGap(m)
    return max(BAR_GAP, BAR_GAP_PX / (m.ppu or 1))
end
Bubble.BarGap = BarGap

local function BarButton(self, i)
    local b = self.linkButtons[i]
    if not b then
        b = LinkButton(self.rowFrame, Bubble.Font())
        b:RegisterForClicks("LeftButtonUp", "RightButtonUp")
        b:GetFontString():SetWordWrap(false)
        b:SetScript("OnClick", function(btn, which)
            local entry = btn.entry
            if entry then
                ns.Links.Click(entry.link, entry.text, which)
            end
        end)
        b:SetScript("OnEnter", function(btn)
            local entry = btn.entry
            if entry then
                ns.Links.ShowTooltip(btn, entry.link, entry.text)
            end
        end)
        b:SetScript("OnLeave", GameTooltip_Hide)
        self.linkButtons[i] = b
    end
    return b
end

local function HideBar(self)
    for _, b in ipairs(self.linkButtons) do
        b.entry = nil
        b:Hide()
    end
    self.barTop, self.barH = nil, 0
end

-- Places the bar top units under the box's top, from x, within width; returns its height (0 and
-- nothing shown without links).
local function PlaceBar(self, m, x, top, width)
    local list = self.links
    if type(list) ~= "table" or #list == 0 then
        HideBar(self)
        return 0
    end
    local fontObj = Bubble.Font()
    local lineH = m.rowH
    local cx, line = 0, 0
    for i, entry in ipairs(list) do
        local b = BarButton(self, i)
        b.entry = entry
        local fs = b:GetFontString()
        fs:SetFontObject(fontObj)
        fs:SetWidth(0)
        b:SetLabel(entry.label)
        local w = b:GetWidth()
        if w > width then
            w = width
            fs:SetWidth(max(1, width - 2))
        end
        b:SetWidth(w)
        b:SetHeight(lineH)
        if cx > 0 and cx + w > width then
            line, cx = line + 1, 0
        end
        b:ClearAllPoints()
        b:SetPoint("TOPLEFT", self.box, "TOPLEFT", x + cx, -(top + line * lineH))
        b:Show()
        cx = cx + w + BAR_SPACE
    end
    for i = #list + 1, #self.linkButtons do
        self.linkButtons[i].entry = nil
        self.linkButtons[i]:Hide()
    end
    self.barTop, self.barH = top, (line + 1) * lineH
    return self.barH
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

-- The request the lane belongs to (an incoming bubble: its reqId; nil: none yet). A new one starts
-- the lane's delay again.
function Methods:SetLaneReq(req)
    self.laneReq = req
end

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
    -- the original in a frame of its own whose hyperlinks work as in the chat (Links.lua)
    self.originalFrame = CreateFrame("Frame", nil, self)
    self.original = Text(self.originalFrame, "ARTWORK", ChatFontSmall)
    self.original:SetTextColor(GRAY_FONT_COLOR:GetRGB())
    self.original:SetMaxLines(ORIGINAL_LINES)
    self.original:SetPoint("TOPLEFT", self.originalFrame, "TOPLEFT", 0, 0)
    ns.Links.EnableHyperlinks(self.originalFrame)
    self.linkButtons = {}
    self.gloss = Text(self, "ARTWORK", GameFontDisableSmall)
    self.plain = Text(self.rowFrame, "OVERLAY", Bubble.Font())
    self.plain:SetTextColor(0.85, 0.85, 0.85)
    self.translateBtn = LinkButton(self.rowFrame, GameFontNormalSmall)
    self.translateBtn:SetScript("OnClick", function()
        Fire(self, "translate")
    end)
    MakeLane(self)
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

-- The message's hyperlinks for the links bar (Links.Parse; nil or empty: no bar).
function Methods:SetLinks(list)
    self.links = list
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
        -- the links bar under the Translate button
        local barH = PlaceBar(self, m, m.inset, m.inset + textH + 4 + META_H + BAR_GAP, textW)
        if barH > 0 then
            boxH = boxH + BAR_GAP + barH
        end
        ClearLane(self)
    else
        self.plain:Hide()
        self.translateBtn:Hide()
        local rowTop0 = Snap(m.inset, self)
        local rowsH = PlaceRows(self, m, rowTop0)
        -- the hint lane: one more row of fill under the rows (shown by ClipRows' UpdateLane)
        PlaceLane(self, m, rowTop0 + rowsH, L.CONV_LANE_IN)
        boxH = 2 * rowTop0 + rowsH + m.rowH
        -- the links bar: under the lane, from the tag's x0 to the plate's end; the rows, the tags,
        -- the plates and the lane keep their places
        local gap = BarGap(m)
        local barH = PlaceBar(self, m, m.inset, rowTop0 + rowsH + m.rowH + gap, 8 * m.W + m.plateWidth)
        if barH > 0 then
            boxH = boxH + gap + barH
        end
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
        local frame = self.originalFrame
        frame:ClearAllPoints()
        frame:SetPoint("TOPLEFT", self, "TOPLEFT", m.margin + 8, -y)
        frame:SetWidth(bw - 16)
        self.original:SetWidth(bw - 16)
        self.original:SetText(self.originalText)
        local h = self.original:GetStringHeight()
        frame:SetHeight(max(1, h))
        frame:Show()
        self.original:Show()
        y = y + h + 2
    else
        self.originalFrame:Hide()
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
    -- the text in a frame of its own whose hyperlinks work as in the chat (Links.lua): a whisper I
    -- wrote in the normal chat with an item or a spell keeps usable links (L2). Its mouse is on only
    -- while the text has links, so the box's hover works as before on the others.
    self.textFrame = CreateFrame("Frame", nil, self.rowFrame)
    self.text = Text(self.textFrame, "OVERLAY", Bubble.Font())
    self.text:SetPoint("TOPLEFT", self.textFrame, "TOPLEFT", 0, 0)
    self.text:SetTextColor(1, 1, 1)
    ns.Links.EnableHyperlinks(self.textFrame)
    self.textFrame:SetScript("OnEnter", function()
        Fire(self, "enter")
    end)
    self.textFrame:SetScript("OnLeave", function()
        Fire(self, "leave")
    end)
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
    self.outLinks = #ns.Links.Parse(self.outText) > 0
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
    local frame = self.textFrame
    frame:ClearAllPoints()
    frame:SetPoint("TOPLEFT", box, "TOPLEFT", OUT_PAD_X, -OUT_PAD_Y)
    frame:SetSize(textW, max(1, textH))
    frame:EnableMouse(self.outLinks and true or false)
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
    MakeLane(self)
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
    -- the lane: one row of fill right under the last row, the hint clear of its top and bottom 2
    -- units; each layout is a new reply, a new width or new settings: its delay starts again
    PlaceLane(self, m, rowTop0 + rowsH, self.laneText)
    self.laneGen = (self.laneGen or 0) + 1
    self.laneReq = "reply" .. self.laneGen
    local h = Snap(rowTop0 + rowsH + m.rowH + CARD_GAP + CARD_BOTTOM, self)
    self:SetHeight(h)
    self.metrics = m
    UpdateLane(self)
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
    b.links = nil
    b.slot, b.K = nil, 1
    b.laneReq = nil
    live[b] = true
    return b
end

function Bubble.AcquireOut(parent)
    local b = Acquire("out", parent, NewOut)
    b.Layout = LayoutOut
    b.handlers, b.outText, b.draftText, b.outLinks = nil, nil, nil, false
    return b
end

-- The reply card (one per window): the composer shows, lays out and hides it.
function Bubble.AcquireReply(parent)
    local b = Acquire("reply", parent, NewReply)
    b.Layout = LayoutReply
    live[b] = true
    return b
end

-- Every lane in use is checked again: the window calls it when it shows (the delay starts again)
-- and when it hides (every hint goes).
function Bubble.RefreshLanes()
    for b in pairs(live) do
        UpdateLane(b)
    end
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
    live[b] = nil
    if b.lane then
        b.laneToken = (b.laneToken or 0) + 1   -- a pending reveal does nothing
        b.laneReq = nil
        ClearLane(b)
    end
    for _, row in ipairs(b.rowsList) do
        row.visible, row.clipped = false, false
        ShowRow(row, false)
    end
    if b.linkButtons then
        b.links = nil
        HideBar(b)
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

