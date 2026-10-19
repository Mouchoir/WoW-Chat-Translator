-- Mock WoW Forever client for TranslateProbe tests. Runs in fengari (Lua 5.3) under
-- tests\run.js, and in any Lua that has a LuaBitOp-compatible `bit` (the addon needs it).
--
-- The mock follows the Forever 1.60.1 UI source where it matters:
-- * filters are skipped when arg1 is secret; history entries carry (event, eventArgs) with
--   lineID at eventArgs[11]; lines moved to a new window lose their eventArgs;
-- * TransformMessages repackages an entry (new timestamp); a transform that errors leaves a
--   hole (false) in the history buffer; chat frames keep the last 128 entries;
-- * ADDON_RESTRICTION_STATE_CHANGED arrives as (type, Activating / Inactive);
-- * secret values raise on any use except issecretvalue and concatenation.
--
-- Public API (tests\e2e depends on it; keep it stable):
--   Mock.Load(files, getSource) -> ns    loads each file with ("TranslateProbe", ns), then fires
--                                         ADDON_LOADED, PLAYER_LOGIN, PLAYER_ENTERING_WORLD
--   Mock.Deliver(event, text, sender, guid, hidden) -> lineID
--   Mock.Click(link, chatFrame, shift); Mock.Tick(n) (n frames of 1/60 s); Mock.Now() -> seconds
--   Mock.SetLockdown(on); Mock.Restriction(type, state)
--   Mock.ChatFrames() -> { ChatFrame1, ChatFrame3 }
--   Mock.History(cf) -> entries { message, r, g, b, extra, timestamp }, oldest first (false = hole)
--   Mock.Textures() -> { { frame, r, g, b, a, shown, x, y, w, h, strata, layer, texture } ... }
--                      x, y, w, h in physical px from the client's top-left corner (y down)
--   Mock.SetScreen(physW, physH, uiScale)
--   Mock.SetChatRect(cf, l, b, w, h)      chat clip rectangle in scaled units (screen = 768 high,
--                                          from the bottom-left), as GetScaledRect returns it
--   Mock.SetFont(cf, path, h)
--   Mock.Printed() -> { lines }; Mock.Errors() -> { messages }
--   Mock.SetCVar(name, value); Mock.SetShift(down)
-- Extras: Mock.Fire(event, ...), Mock.Secret(), Mock.SetRectSecret(cf, on), Mock.Frames(),
--   Mock.echo (default true: prints and errors also go to stdout), Mock.rawprint (stdout).
-- Conversation window support (see the section of that name below for the full list):
--   Mock.Deliver(event, text, sender, guid, hidden, opts) with opts { language, channelString, target,
--     flags, zoneChannelID, channelIndex, channelBase, languageID, bnID, isMobile, class, noInfo }
--   Mock.Click(link, chatFrame, shift, ctrl); Mock.SetCtrl(down); Mock.SetAlt(down)
--   Mock.Paste(box, text); Mock.Type(box, text, opts); Mock.Key(frame, key, mods)
--   Mock.ClickButton(button, mouseButton, mods); Mock.MouseDown/MouseUp(frame, mouseButton)
--   Mock.Hover(region); Mock.Leave(); Mock.GlobalMouseDown(mouseButton); Mock.Wheel(frame, delta)
--   Mock.Drag(frame, dx, dy); Mock.Resize(frame, w, h)
--   Mock.SentChat(); Mock.EchoSent(record, opts); Mock.SystemMessage(text); Mock.PlayerNotFound(name)
--   Mock.ActionBlocked(addonName, func); Mock.Sounds(); Mock.FocusLog(); Mock.Focused()
--   Mock.SetCombat(on); Mock.SetGroup(t); Mock.SetChannels(t); Mock.SetPlayerInfo(guid, info)
--   Mock.SetBNAccount(id, info); Mock.SetChatColor(chatType, r, g, b); Mock.SetAtlas(name, info)
--   Mock.SetLayoutMissing(name, missing); Mock.SetTemplateMissing(name, missing); Mock.Player()
-- Links of a message (see the section of that name): SetItemRef, IsModifiedClick, ChatFrameUtil.InsertLink,
--   DressUpLink, GameTooltip:SetHyperlink, the item quality colour APIs, Frame:SetHyperlinksEnabled;
--   Mock.LinkCalls(), Mock.ClearLinkCalls(), Mock.SetChatEditActive(on), Mock.SetQualityColor(q, r, g, b),
--   Mock.ClickHyperlink(frame, link, button, mods), Mock.HoverHyperlink(frame, link), Mock.LeaveHyperlink(frame)
-- Settings API (12.x Blizzard_Settings.lua): categories, proxy settings, dropdowns, checkboxes,
--   stock sliders (CreateSliderOptions, CreateSlider, MinimalSliderWithSteppersMixin.Label),
--   RegisterInitializer with CreateSettingsButtonInitializer and
--   CreateSettingsListSectionHeaderInitializer, canvas subcategories, OpenToCategory; arguments
--   are checked more strictly than the client does (types, defaults among the options, no
--   duplicate variable), to catch misuse early.
--   Mock.Settings() -> { categories, addOn, settings, initializers, opened, notified };
--   Mock.ShowCanvas(category) shows a canvas page the way the panel does (OnRefresh);
--   Mock.HideCanvas(category) leaves it (a focused edit box inside loses its focus).
-- Widgets: EditBox focus (SetFocus/ClearFocus fire OnEditFocusLost), Button:Click, Enable/Disable.
--   Mock.Press(editBox, script) runs OnEnterPressed / OnEscapePressed. CreateFrame and
--   CreateFontString refuse templates and font objects the mock does not know (typo guard).
-- print() records the line (Mock.Printed) and adds it to DEFAULT_CHAT_FRAME's history.
-- Defaults: 1920 x 1080 px, UI scale 1, ChatFrame1 shown, ChatFrame3 hidden (a docked tab),
--   chat font 14, chat clip rectangle (20, 100, 400, 180) scaled units.
Mock = {}
local M = Mock
local rawprint = print
unpack = unpack or table.unpack
local pack = table.pack or function(...)
    return { n = select("#", ...), ... }
end
local loadChunk = loadstring or load
math.randomseed(1)

M.echo = true
M.rawprint = rawprint

local state = {
    frame = 0, time = 0, lockdown = false, shift = false, ctrl = false, alt = false, combat = false,
    hardware = false, loaded = false, physW = 1920, physH = 1080, uiScale = 1, cvars = {},
}
local printed, errors = {}, {}

local function ReportError(e)
    errors[#errors + 1] = tostring(e)
    if M.echo then
        rawprint("ERROR: " .. tostring(e))
    end
end

-- Calls fn like the client calls a script: an error goes to the error handler, not the caller.
local function Call(fn, ...)
    local res = pack(pcall(fn, ...))
    if not res[1] then
        ReportError(res[2])
        return false
    end
    return true, unpack(res, 2, res.n)
end

---------------------------------------------------------------------------- Lua helpers
geterrorhandler = function()
    return ReportError
end
wipe = function(t)
    for k in pairs(t) do
        t[k] = nil
    end
    return t
end
time = os.time

-- Secret values: any use other than issecretvalue/concatenation raises, like on the client.
local SecretMT = {}
local function Secret()
    return setmetatable({}, SecretMT)
end
SecretMT.__concat = function()
    return Secret()
end
SecretMT.__len = function()
    error("attempt to get length of a secret value")
end
SecretMT.__index = function()
    error("attempt to index a secret value")
end
SecretMT.__lt = function()
    error("attempt to compare a secret value")
end
SecretMT.__le = SecretMT.__lt
SecretMT.__call = function()
    error("attempt to call a secret value")
end
issecretvalue = function(v)
    return getmetatable(v) == SecretMT
end
canaccessvalue = function(...)
    for i = 1, select("#", ...) do
        if issecretvalue((select(i, ...))) then
            return false
        end
    end
    return true
end
M.Secret = Secret

---------------------------------------------------------------------------- client state
GetLocale = function()
    return LOCALE_UNDER_TEST or "enUS"
end
GetTime = function()
    return state.time
end
IsShiftKeyDown = function()
    return state.shift
end
local PLAYER_GUID = "Player-1-0001"
UnitGUID = function(unit)
    if unit == "player" then
        return PLAYER_GUID
    end
    return nil
end
GetBuildInfo = function()
    return "1.60.1", "70338", "Oct 10 2026", 16001
end
WOW_PROJECT_ID = 18
GetCVar = function(name)
    return state.cvars[name]
end
GetPhysicalScreenSize = function()
    return state.physW, state.physH
end
Enum = {
    AddOnRestrictionType = { Combat = 0, Encounter = 1, ChallengeMode = 2, PvPMatch = 3, Map = 4, Chat = 5 },
    AddOnRestrictionState = { Inactive = 0, Activating = 1, Active = 2 },
}
C_RestrictedActions = {
    IsAddOnRestrictionActive = function(t)
        return t == 5 and state.lockdown
    end,
}
-- Stand-in for the ICU transliterator: every run of non-ASCII bytes becomes "ro".
C_Intl = {
    Transliterate = function(text, id)
        assert(id == "Any-Latin", "unexpected transliterator " .. tostring(id))
        return (text:gsub("[\128-\255]+", "ro"))
    end,
}
SlashCmdList = {}

---------------------------------------------------------------------------- regions
local frames, textures = {}, {}
local POINTS = {
    TOPLEFT = { 0, 0 }, TOP = { 0.5, 0 }, TOPRIGHT = { 1, 0 }, LEFT = { 0, 0.5 }, CENTER = { 0.5, 0.5 },
    RIGHT = { 1, 0.5 }, BOTTOMLEFT = { 0, 1 }, BOTTOM = { 0.5, 1 }, BOTTOMRIGHT = { 1, 1 },
}

local function EffScale(r)
    if r.kind ~= "frame" then
        return EffScale(r.parent)
    end
    local base = 1
    if r.parent and not r.ignoreParentScale then
        base = EffScale(r.parent)
    end
    return base * r.scale
end

local function PxPerUnit(r)
    return EffScale(r) * state.physH / 768
end

local function IsVisible(r)
    if not r.shown then
        return false
    end
    if r.parent then
        return IsVisible(r.parent)
    end
    return true
end

local function Strata(r)
    if r.kind ~= "frame" then
        return Strata(r.parent)
    end
    return r.strata or (r.parent and Strata(r.parent)) or "MEDIUM"
end

-- One axis of a region from its anchors: cons = { { fraction, position } ... }. Two anchors at
-- different fractions (LEFT and RIGHT, TOP and BOTTOM) set the extent, as on the client; one
-- anchor places the region's own extent.
local function AxisSolve(cons, own)
    local lo, hi
    for _, c in ipairs(cons) do
        if not lo or c[1] < lo[1] then
            lo = c
        end
        if not hi or c[1] > hi[1] then
            hi = c
        end
    end
    if hi[1] > lo[1] then
        local ext = (hi[2] - lo[2]) / (hi[1] - lo[1])
        return lo[2] - lo[1] * ext, ext
    end
    return lo[2] - lo[1] * own, own
end

-- left, top, width, height in physical px from the client's top-left (y down)
local function Rect(r, depth)
    depth = (depth or 0) + 1
    assert(depth < 50, "anchor loop")
    if r.isRoot then
        return 0, 0, state.physW, state.physH
    end
    if r.scaledRect then
        local l, b, w, h = unpack(r.scaledRect)
        local k = state.physH / 768
        return l * k, state.physH - (b + h) * k, w * k, h * k
    end
    if r.allPoints then
        return Rect(r.allPoints, depth)
    end
    local ppu = PxPerUnit(r)
    local w, h = (r.w or 0) * ppu, (r.h or 0) * ppu
    local sf = r.scrollFrame
    if sf then
        -- a scroll child sits at its scroll frame's top-left, moved by the scroll offsets
        local sl, st = Rect(sf, depth)
        local sppu = PxPerUnit(sf)
        return sl - (sf.hscroll or 0) * sppu, st - (sf.vscroll or 0) * sppu, w, h
    end
    if #r.points == 0 then
        return 0, 0, w, h
    end
    local xs, ys = {}, {}
    for _, p in ipairs(r.points) do
        local rl, rt, rw, rh = Rect(p.rel, depth)
        local rp, sp = POINTS[p.relPoint] or POINTS.TOPLEFT, POINTS[p.point] or POINTS.TOPLEFT
        xs[#xs + 1] = { sp[1], rl + rp[1] * rw + p.x * ppu }
        ys[#ys + 1] = { sp[2], rt + rp[2] * rh - p.y * ppu }
    end
    local l, ww = AxisSolve(xs, w)
    local t, hh = AxisSolve(ys, h)
    return l, t, ww, hh
end

local Methods = {}

function Methods:SetPoint(point, a, b, c, d)
    local rel, relPoint, x, y
    if a == nil or type(a) == "number" then
        rel, relPoint, x, y = self.parent, point, a, b
    elseif type(b) == "number" or b == nil and c == nil then
        rel, relPoint, x, y = a, point, b, c
    else
        rel, relPoint, x, y = a, b, c, d
    end
    if type(rel) == "string" then
        rel = _G[rel]
    end
    if self.allPoints then
        -- SetAllPoints is two anchors; setting one of them again keeps the other
        local all = self.allPoints
        self.allPoints = nil
        self.points = { { point = "TOPLEFT", rel = all, relPoint = "TOPLEFT", x = 0, y = 0 },
            { point = "BOTTOMRIGHT", rel = all, relPoint = "BOTTOMRIGHT", x = 0, y = 0 } }
    end
    -- setting an anchor point again replaces it; other points are kept (as on the client)
    local entry = { point = point, rel = rel or UIParent, relPoint = relPoint or point, x = x or 0, y = y or 0 }
    for i, p in ipairs(self.points) do
        if p.point == point then
            self.points[i] = entry
            return
        end
    end
    self.points[#self.points + 1] = entry
end
function Methods:ClearAllPoints()
    self.points, self.allPoints = {}, nil
end
function Methods:SetAllPoints(rel)
    self.points = {}
    self.allPoints = rel or self.parent
end
local function SizeChanged(self, w0, h0)
    if (self.w ~= w0 or self.h ~= h0) and self.scripts and self.scripts.OnSizeChanged then
        Call(self.scripts.OnSizeChanged, self, self.w, self.h)
    end
end
function Methods:SetSize(w, h)
    local w0, h0 = self.w, self.h
    self.w, self.h = w, h
    SizeChanged(self, w0, h0)
end
function Methods:SetWidth(w)
    local w0 = self.w
    self.w = w
    SizeChanged(self, w0, self.h)
end
function Methods:SetHeight(h)
    local h0 = self.h
    self.h = h
    SizeChanged(self, self.w, h0)
end
function Methods:GetWidth()
    local _, _, w = Rect(self)
    return w / PxPerUnit(self)
end
function Methods:GetHeight()
    local _, _, _, h = Rect(self)
    return h / PxPerUnit(self)
end
function Methods:GetSize()
    return self:GetWidth(), self:GetHeight()
end
function Methods:GetScaledRect()
    if self.secretRect then
        return Secret(), Secret(), Secret(), Secret()
    end
    local l, t, w, h = Rect(self)
    local k = state.physH / 768
    return l / k, (state.physH - t - h) / k, w / k, h / k
end
function Methods:GetRect()
    local l, b, w, h = self:GetScaledRect()
    if self.secretRect then
        return l, b, w, h
    end
    local s = EffScale(self)
    return l / s, b / s, w / s, h / s
end
function Methods:GetLeft()
    return (self:GetRect())
end
function Methods:GetBottom()
    return select(2, self:GetRect())
end
function Methods:GetTop()
    if self.secretRect then
        return Secret()
    end
    local _, b, _, h = self:GetRect()
    return b + h
end
function Methods:GetEffectiveScale()
    return EffScale(self)
end
function Methods:IsVisible()
    return IsVisible(self)
end
function Methods:IsShown()
    return self.shown
end
function Methods:GetParent()
    return self.parent
end
function Methods:GetName()
    return self.name
end
function Methods:SetAlpha(a)
    self.alpha = a
end
function Methods:GetAlpha()
    return self.alpha or 1
end
function Methods:Show()
    if self.shown then
        return
    end
    self.shown = true
    if self.scripts and self.scripts.OnShow and IsVisible(self) then
        Call(self.scripts.OnShow, self)
    end
end
local focused
function Methods:Hide()
    if not self.shown then
        return
    end
    local was = IsVisible(self)
    self.shown = false
    -- a focused edit box inside a frame that is hidden loses its focus, as on the client
    local p = focused
    while p and p ~= self do
        p = p.parent
    end
    if p == self then
        focused:ClearFocus()
    end
    if was and self.scripts and self.scripts.OnHide then
        Call(self.scripts.OnHide, self)
    end
end
function Methods:SetShown(on)
    if on then
        self:Show()
    else
        self:Hide()
    end
end

-- Frames
function Methods:RegisterEvent(e)
    self.events[e] = true
end
function Methods:UnregisterEvent(e)
    self.events[e] = nil
end
function Methods:UnregisterAllEvents()
    self.events = {}
end
function Methods:IsEventRegistered(e)
    return self.events[e] == true
end
function Methods:SetScript(k, fn)
    self.scripts[k] = fn
end
function Methods:GetScript(k)
    return self.scripts[k]
end
function Methods:SetScale(s)
    self.scale = s
end
function Methods:GetScale()
    return self.scale
end
function Methods:SetIgnoreParentScale(on)
    self.ignoreParentScale = on and true or false
end
function Methods:IsIgnoringParentScale()
    return self.ignoreParentScale
end
function Methods:SetFrameStrata(s)
    self.strata = s
end
function Methods:GetFrameStrata()
    return Strata(self)
end
function Methods:SetFrameLevel(n)
    self.level = n
end
function Methods:GetFrameLevel()
    return self.level or 0
end
function Methods:GetID()
    return self.id or 0
end
function Methods:SetID(id)
    self.id = id
end

-- Textures
function Methods:SetColorTexture(r, g, b, a)
    self.r, self.g, self.b, self.a = r, g, b, a or 1
end
function Methods:SetTexture(file)
    self.file = file
end
function Methods:SetVertexColor(r, g, b, a)
    self.r, self.g, self.b, self.a = r, g, b, a or 1
end
function Methods:SetSnapToPixelGrid(on)
    self.snap = on
end
function Methods:SetTexelSnappingBias(v)
    self.texelBias = v
end
function Methods:SetDrawLayer(layer)
    self.layer = layer
end

-- Font strings: width = 0.5 x font height per UTF-8 character; inline textures and atlases count
-- their own width; codes (|cAARRGGBB, |cn<name>:, |r, |H, |h) count nothing.
local function Utf8Len(s)
    local n = 0
    for _ in s:gmatch("[^\128-\191]") do
        n = n + 1
    end
    return n
end
local function SegmentWidth(s, size)
    local tex = 0
    s = s:gsub("|T[^|]-:(%d+):(%d+)[^|]-|t", function(_, w)
        tex = tex + tonumber(w)
        return ""
    end)
    s = s:gsub("|A[^|]-:(%d+):(%d+)[^|]-|a", function(_, w)
        tex = tex + tonumber(w)
        return ""
    end)
    s = s:gsub("|T.-|t", ""):gsub("|A.-|a", ""):gsub("|c%x%x%x%x%x%x%x%x", ""):gsub("|cn[^:|]*:", ""):gsub("|r", "")
    s = s:gsub("|H.-|h", ""):gsub("|h", "")
    return 0.5 * size * Utf8Len(s) + tex
end
local function Segments(text)
    local out = {}
    for seg in (text .. "|n"):gmatch("(.-)|n") do
        out[#out + 1] = seg
    end
    return out
end
-- An explicit SetFont wins; otherwise the font object's current font (a font object changed later
-- changes every font string that uses it, as on the client).
local function FontSize(self)
    if self.fontSize then
        return self.fontSize
    end
    local fo = self.fontObject
    if type(fo) == "string" then
        fo = _G[fo]
    end
    return type(fo) == "table" and fo.isFont and fo.size or 0
end
function Methods:SetFont(path, size, flags)
    self.fontPath, self.fontSize, self.fontFlags = path, size, flags
    return true
end
function Methods:GetFont()
    if self.fontSize or self.fontPath then
        return self.fontPath, self.fontSize, self.fontFlags
    end
    local fo = self.fontObject
    if type(fo) == "string" then
        fo = _G[fo]
    end
    if type(fo) == "table" and fo.isFont then
        return fo.path, fo.size, fo.flags
    end
    return nil, nil, nil
end
function Methods:SetText(text)
    self.text = text == nil and "" or tostring(text)
end
function Methods:GetText()
    return self.text
end
function Methods:SetWordWrap() end
function Methods:SetNonSpaceWrap() end
function Methods:SetIndentedWordWrap() end
function Methods:SetJustifyH() end
function Methods:GetUnboundedStringWidth()
    local best = 0
    for _, seg in ipairs(Segments(self.text or "")) do
        best = math.max(best, SegmentWidth(seg, FontSize(self)))
    end
    return best
end
function Methods:GetStringWidth()
    local w = self:GetUnboundedStringWidth()
    if (self.w or 0) > 0 then
        return math.min(w, self.w)
    end
    return w
end
function Methods:GetNumLines()
    local lines = 0
    for _, seg in ipairs(Segments(self.text or "")) do
        local w = SegmentWidth(seg, FontSize(self))
        if (self.w or 0) > 0 then
            lines = lines + math.max(1, math.ceil(w / self.w))
        else
            lines = lines + 1
        end
    end
    return lines
end
function Methods:GetStringHeight()
    return self:GetNumLines() * FontSize(self)
end

-- Widgets (edit boxes, buttons)
local focusLog = {}
function Methods:SetParent(parent)
    self.parent = parent or UIParent
end
function Methods:EnableMouse(on)
    self.mouse = on
end
function Methods:EnableMouseWheel(on)
    self.mouseWheel = on
end
function Methods:HookScript(k, fn)
    local old = self.scripts[k]
    self.scripts[k] = function(...)
        if old then
            old(...)
        end
        return fn(...)
    end
end
function Methods:SetAutoFocus(on)
    self.autoFocus = on
end
function Methods:SetMaxLetters(n)
    self.maxLetters = n
end
function Methods:HighlightText() end
function Methods:SetCursorPosition() end
function Methods:SetTextInsets() end
function Methods:SetFontObject(font)
    self.fontObject = font
end
function Methods:SetTextColor(r, g, b, a)
    self.textColor = { r, g, b, a or 1 }
end
function Methods:HasFocus()
    return focused == self
end
function Methods:ClearFocus()
    if focused == self then
        focused = nil
        if self.scripts.OnEditFocusLost then
            Call(self.scripts.OnEditFocusLost, self)
        end
    end
end
function Methods:SetFocus()
    if focused and focused ~= self then
        focused:ClearFocus()
    end
    focusLog[#focusLog + 1] = { box = self, at = state.time, hardware = state.hardware }
    focused = self
    if self.scripts.OnEditFocusGained then
        Call(self.scripts.OnEditFocusGained, self)
    end
end
function Methods:Enable()
    local was = self.disabled
    self.disabled = nil
    if was and self.scripts and self.scripts.OnEnable then
        Call(self.scripts.OnEnable, self)
    end
end
function Methods:Disable()
    local was = self.disabled
    self.disabled = true
    if focused == self then
        self:ClearFocus()
    end
    if not was and self.scripts and self.scripts.OnDisable then
        Call(self.scripts.OnDisable, self)
    end
end
function Methods:SetEnabled(on)
    if on then
        self:Enable()
    else
        self:Disable()
    end
end
function Methods:IsEnabled()
    return not self.disabled
end
function Methods:Click(button)
    if not self.disabled and self.scripts.OnClick then
        Call(self.scripts.OnClick, self, button or "LeftButton", false)
    end
end

local MethodsMT = { __index = Methods }
-- Widget types with methods of their own (filled in by the conversation window section): a frame of
-- such a type gets them on top of the shared ones.
local TypeMT = {}

local function NewRegion(kind, parent)
    return setmetatable({ kind = kind, parent = parent, shown = true, points = {}, w = 0, h = 0 }, MethodsMT)
end

local function NewFrame(frameType, name, parent)
    local f = NewRegion("frame", parent)
    f.frameType, f.name = frameType or "Frame", name
    f.scripts, f.events, f.scale = {}, {}, 1
    f.children, f.regions = {}, {}
    if TypeMT[f.frameType] then
        setmetatable(f, TypeMT[f.frameType])
    end
    if parent and parent.children then
        parent.children[#parent.children + 1] = f
    end
    function f:CreateTexture(_, layer, _, subLevel)
        local t = NewRegion("texture", self)
        t.layer, t.subLevel = layer, subLevel
        t.r, t.g, t.b, t.a = 1, 1, 1, 1
        textures[#textures + 1] = t
        self.regions[#self.regions + 1] = t
        return t
    end
    function f:CreateFontString(_, layer, font)
        assert(font == nil or M.KNOWN_FONTS[font] or (type(font) == "table" and font.isFont),
            "unknown font object " .. tostring(font))
        local fs = NewRegion("fontstring", self)
        fs.layer, fs.text, fs.fontObject = layer, "", font
        self.regions[#self.regions + 1] = fs
        return fs
    end
    frames[#frames + 1] = f
    if name then
        _G[name] = f
    end
    return f
end

UIParent = NewFrame("Frame", "UIParent", nil)
UIParent.isRoot = true
UIParent.scale = state.uiScale

-- Templates and font objects the addon may use (each checked in the 12.x UI source, see
-- docs\ADDON.md); anything else is a typo or an unchecked dependency.
-- Each template maps to its frame type (or a set of the types it may be created as); the
-- conversation window section adds the rest and builds each template's children (TEMPLATES).
M.KNOWN_TEMPLATES = { InputBoxTemplate = "EditBox", UIPanelButtonTemplate = "Button" }
local TEMPLATES, templateMissing = {}, {}
-- Font objects (globals of the same name); see the conversation window section.
M.KNOWN_FONTS = {}
local function TemplateFits(known, frameType)
    if type(known) == "table" then
        return known[frameType] == true
    end
    return known == frameType
end
CreateFrame = function(frameType, name, parent, template)
    local list = {}
    if template ~= nil then
        for t in tostring(template):gmatch("[^,%s]+") do
            if templateMissing[t] then
                error("CreateFrame(): Couldn't find inherited node \"" .. t .. "\"", 2)
            end
            assert(TemplateFits(M.KNOWN_TEMPLATES[t], frameType), "unknown template " .. tostring(t) .. " for "
                .. tostring(frameType))
            list[#list + 1] = t
        end
    end
    local f = NewFrame(frameType, name, parent or UIParent)
    f.template = template
    if frameType == "EditBox" then
        f.text = ""     -- the client returns "" for an empty edit box, never nil
        f.cursor = 0
    end
    if frameType == "Button" or frameType == "CheckButton" or frameType == "EditBox" then
        f.mouse = true  -- these are mouse enabled by default
    end
    for _, t in ipairs(list) do
        if TEMPLATES[t] then
            TEMPLATES[t](f)
        end
    end
    return f
end

local function Fire(event, ...)
    local list = {}
    for _, f in ipairs(frames) do
        if f.events[event] and f.scripts.OnEvent then
            list[#list + 1] = f
        end
    end
    for _, f in ipairs(list) do
        Call(f.scripts.OnEvent, f, event, ...)
    end
end
M.Fire = Fire

GameTooltip = NewFrame("GameTooltip", "GameTooltip", UIParent)
GameTooltip.shown, GameTooltip.lines, GameTooltip.colors = false, {}, {}
-- lines[i] is the text of line i (a double line: "left | right"), colors[i] = { r, g, b } or nil
function GameTooltip:SetOwner(owner, anchor, x, y)
    self.owner, self.anchor, self.lines, self.colors = owner, anchor, {}, {}
    self.anchorX, self.anchorY = x, y
end
function GameTooltip:SetText(text, r, g, b)
    self.lines, self.colors = { text }, { r and { r, g, b } or nil }
end
function GameTooltip:AddLine(text, r, g, b)
    self.lines[#self.lines + 1] = text
    self.colors[#self.lines] = r and { r, g, b } or nil
end
function GameTooltip:AddDoubleLine(left, right, lr, lg, lb)
    self:AddLine(tostring(left) .. " | " .. tostring(right), lr, lg, lb)
end
function GameTooltip:ClearLines()
    self.lines, self.colors = {}, {}
end
function GameTooltip:NumLines()
    return #self.lines
end
function GameTooltip:GetOwner()
    return self.owner
end
function GameTooltip:IsOwned(frame)
    return self.owner == frame
end
function GameTooltip:Hide()
    Methods.Hide(self)
    self.owner = nil
end
function GameTooltip:SetMinimumWidth(w)
    self.minWidth = w
end
function GameTooltip:SetPadding(r, b, l, t)
    self.padding = { r, b, l, t }
end

---------------------------------------------------------------------------- timers
local timers, timerSeq = {}, 0
local function AddTimer(delay, fn, interval, iterations)
    timerSeq = timerSeq + 1
    local handle = { cancelled = false }
    function handle:Cancel()
        self.cancelled = true
    end
    function handle:IsCancelled()
        return self.cancelled
    end
    timers[#timers + 1] = { at = state.time + (delay or 0), fn = fn, handle = handle, interval = interval,
        left = iterations, seq = timerSeq }
    return handle
end
C_Timer = {
    After = function(delay, fn)
        AddTimer(delay, fn)
    end,
    NewTimer = function(delay, fn)
        return AddTimer(delay, fn)
    end,
    NewTicker = function(interval, fn, iterations)
        return AddTimer(interval, fn, interval, iterations)
    end,
}

local function RunTimers()
    local due, keep = {}, {}
    for _, t in ipairs(timers) do
        if not t.handle.cancelled then
            if t.at <= state.time + 1e-9 then
                due[#due + 1] = t
            else
                keep[#keep + 1] = t
            end
        end
    end
    timers = keep
    table.sort(due, function(a, b)
        if a.at ~= b.at then
            return a.at < b.at
        end
        return a.seq < b.seq
    end)
    for _, t in ipairs(due) do
        if not t.handle.cancelled then
            Call(t.fn, t.handle)
            if t.interval and not t.handle.cancelled then
                t.left = t.left and t.left - 1
                if t.left == nil or t.left > 0 then
                    t.at = math.max(t.at + t.interval, state.time + 1e-6)
                    timers[#timers + 1] = t
                end
            end
        end
    end
end

function M.Tick(n)
    for _ = 1, n or 1 do
        state.frame = state.frame + 1
        state.time = state.frame / 60
        RunTimers()
        local list = {}
        for _, f in ipairs(frames) do
            if f.scripts.OnUpdate and IsVisible(f) then
                list[#list + 1] = f
            end
        end
        for _, f in ipairs(list) do
            if IsVisible(f) then
                Call(f.scripts.OnUpdate, f, 1 / 60)
            end
        end
    end
end

function M.Now()
    return state.time
end

---------------------------------------------------------------------------- EventRegistry
local callbacks = {}
EventRegistry = {
    RegisterCallback = function(_, event, fn, owner)
        callbacks[event] = callbacks[event] or {}
        callbacks[event][owner or fn] = fn
    end,
    UnregisterCallback = function(_, event, owner)
        if callbacks[event] then
            callbacks[event][owner] = nil
        end
    end,
    TriggerEvent = function(_, event, ...)
        for owner, fn in pairs(callbacks[event] or {}) do
            Call(fn, owner, ...)
        end
    end,
}

---------------------------------------------------------------------------- chat frames
CHAT_FRAMES = { "ChatFrame1", "ChatFrame3" }

local function NewChatFrame(name, id, shown)
    local cf = NewFrame("ScrollingMessageFrame", name, UIParent)
    cf.id, cf.shown = id, shown
    cf.history, cf.holes, cf.transforms, cf.maxLines, cf.scroll = {}, 0, 0, 128, 0
    cf.fontPath, cf.fontSize, cf.fontFlags, cf.spacing = "Fonts\\FRIZQT__.TTF", 14, "", 0
    cf.scaledRect = { 20, 100, 400, 180 }
    local container = NewFrame("Frame", nil, cf)
    container.scaledRect = cf.scaledRect
    cf.FontStringContainer = container
    cf.visibleLines = {}
    for i = 1, 2 do
        local line = container:CreateFontString(nil, "BACKGROUND")
        line.GetTop = function()
            if container.secretRect then
                return Secret()
            end
            local _, b, _, h = container:GetRect()
            return b + h - (i - 1) * cf.fontSize
        end
        cf.visibleLines[i] = line
    end

    function cf:AddMessage(message, r, g, b, ...)
        self.history[#self.history + 1] = { message = message, r = r, g = g, b = b, extra = pack(...),
            timestamp = state.time }
        while #self.history > self.maxLines do
            table.remove(self.history, 1)
        end
    end
    local function Unpack(e)
        return e.message, e.r, e.g, e.b, unpack(e.extra, 1, e.extra.n)
    end
    function cf:TransformMessages(predicate, transform)
        self.transforms = self.transforms + 1
        for i = 1, #self.history do
            local e = self.history[i]
            if e then
                local okP, match = pcall(predicate, Unpack(e))
                if okP and match then
                    local res = pack(pcall(transform, Unpack(e)))
                    if res[1] then
                        self.history[i] = { message = res[2], r = res[3], g = res[4], b = res[5],
                            extra = pack(unpack(res, 6, res.n)), timestamp = state.time }
                    else
                        self.history[i] = false
                        self.holes = self.holes + 1
                        ReportError("transform: " .. tostring(res[2]))
                    end
                elseif not okP then
                    ReportError("predicate: " .. tostring(match))
                end
            end
        end
    end
    function cf:GetFont()
        return self.fontPath, self.fontSize, self.fontFlags
    end
    function cf:GetSpacing()
        return self.spacing
    end
    function cf:GetNumMessages()
        return #self.history
    end
    function cf:GetMaxLines()
        return self.maxLines
    end
    function cf:SetMaxLines(n)
        self.maxLines = n
    end
    function cf:GetScrollOffset()
        return self.scroll
    end
    function cf:SetScrollOffset(n)
        self.scroll = n
    end
    return cf
end
NewChatFrame("ChatFrame1", 1, true)
NewChatFrame("ChatFrame3", 3, false)
DEFAULT_CHAT_FRAME = ChatFrame1

---------------------------------------------------------------------------- Settings API
local settingsState = { categories = {}, addOn = {}, settings = {}, initializers = {}, opened = {}, notified = {} }
local function NewCategory(name, kind, frame, parent)
    local category = { name = name, kind = kind, frame = frame, parent = parent, subcategories = {} }
    settingsState.categories[#settingsState.categories + 1] = category
    category.ID = #settingsState.categories
    function category:GetID()
        return self.ID
    end
    function category:GetName()
        return self.name
    end
    if parent then
        parent.subcategories[#parent.subcategories + 1] = category
    end
    return category, { category = category }
end
local VAR_TYPES = { boolean = true, string = true, number = true }
Settings = {
    VarType = { Boolean = "boolean", String = "string", Number = "number" },
    RegisterVerticalLayoutCategory = function(name)
        assert(type(name) == "string" and name ~= "", "category name")
        return NewCategory(name, "vertical")
    end,
    RegisterCanvasLayoutCategory = function(frame, name)
        assert(type(frame) == "table" and type(name) == "string", "canvas category")
        return NewCategory(name, "canvas", frame)
    end,
    RegisterCanvasLayoutSubcategory = function(parent, frame, name)
        assert(type(parent) == "table" and parent.kind and type(frame) == "table" and type(name) == "string",
            "canvas subcategory")
        return NewCategory(name, "canvas", frame, parent)
    end,
    RegisterAddOnCategory = function(category)
        for _, c in ipairs(settingsState.addOn) do
            assert(c ~= category, "category registered twice")
        end
        settingsState.addOn[#settingsState.addOn + 1] = category
    end,
    RegisterProxySetting = function(category, variable, varType, name, default, getValue, setValue)
        assert(type(category) == "table" and category.kind, "proxy setting: category")
        assert(type(variable) == "string" and settingsState.settings[variable] == nil, "proxy setting: variable "
            .. tostring(variable))
        assert(VAR_TYPES[varType] and type(default) == varType, "proxy setting: type of " .. variable)
        assert(type(name) == "string" and type(getValue) == "function" and type(setValue) == "function",
            "proxy setting: name and accessors")
        local setting = { category = category, variable = variable, varType = varType, name = name, default = default }
        function setting:GetValue()
            return getValue()
        end
        function setting:SetValue(value)
            assert(type(value) == varType, "SetValue: type")
            setValue(value)
        end
        function setting:GetVariable()
            return variable
        end
        function setting:GetVariableType()
            return varType
        end
        function setting:GetName()
            return name
        end
        function setting:GetDefaultValue()
            return default
        end
        settingsState.settings[variable] = setting
        return setting
    end,
    CreateControlTextContainer = function()
        local container = { data = {} }
        function container:Add(value, label, tooltip)
            self.data[#self.data + 1] = { value = value, label = label, text = label, tooltip = tooltip }
        end
        function container:GetData()
            return self.data
        end
        return container
    end,
    CreateDropdown = function(category, setting, options, tooltip)
        assert(type(category) == "table" and type(setting) == "table" and type(options) == "function", "dropdown")
        local data = options()
        assert(type(data) == "table" and #data > 0, "dropdown: options")
        local hasDefault = false
        for _, option in ipairs(data) do
            assert(type(option.label) == "string" and option.label ~= "" and type(option.value) == setting.varType,
                "dropdown: option")
            hasDefault = hasDefault or option.value == setting.default
        end
        assert(hasDefault, "dropdown: the default is not an option")
        local init = { kind = "dropdown", category = category, setting = setting, options = options, tooltip = tooltip }
        settingsState.initializers[#settingsState.initializers + 1] = init
        return init
    end,
    CreateCheckbox = function(category, setting, tooltip)
        assert(type(category) == "table" and setting:GetVariableType() == "boolean", "checkbox")
        local init = { kind = "checkbox", category = category, setting = setting, tooltip = tooltip }
        settingsState.initializers[#settingsState.initializers + 1] = init
        return init
    end,
    GetSetting = function(variable)
        return settingsState.settings[variable]
    end,
    NotifyUpdate = function(variable)
        settingsState.notified[#settingsState.notified + 1] = variable
    end,
    OpenToCategory = function(categoryID)
        assert(type(categoryID) == "number", "OpenToCategory takes a category ID")
        settingsState.opened[#settingsState.opened + 1] = categoryID
    end,
    -- Stock sliders (MinimalSliderWithSteppers), as SkillUp and Target In Combat use them on
    -- Forever: CreateSliderOptions(min, max, step), options:SetLabelFormatter(labelType, fn),
    -- CreateSlider(category, numberSetting, options, tooltip). The default must be a step value.
    CreateSliderOptions = function(minValue, maxValue, step)
        assert(type(minValue) == "number" and type(maxValue) == "number" and minValue < maxValue
            and type(step) == "number" and step > 0, "slider options")
        local options = { minValue = minValue, maxValue = maxValue, steps = (maxValue - minValue) / step, step = step,
            formatters = {} }
        function options:SetLabelFormatter(labelType, fn)
            assert(type(labelType) == "number" and type(fn) == "function", "SetLabelFormatter(labelType, fn)")
            self.formatters[labelType] = fn
        end
        return options
    end,
    CreateSlider = function(category, setting, options, tooltip)
        assert(type(category) == "table" and setting:GetVariableType() == "number", "slider: a number setting")
        assert(type(options) == "table" and options.minValue, "slider: options from CreateSliderOptions")
        local d = setting.default
        assert(d >= options.minValue and d <= options.maxValue
            and math.abs((d - options.minValue) / options.step - math.floor((d - options.minValue) / options.step + 0.5))
            < 1e-9, "slider: the default is not a step value")
        local init = { kind = "slider", category = category, setting = setting, options = options, tooltip = tooltip }
        settingsState.initializers[#settingsState.initializers + 1] = init
        return init
    end,
    -- Settings.RegisterInitializer(category, initializer): any initializer, such as a button or
    -- a section header, appended to the category's layout.
    RegisterInitializer = function(category, initializer)
        assert(type(category) == "table" and category.kind and type(initializer) == "table" and initializer.kind,
            "RegisterInitializer(category, initializer)")
        initializer.category = category
        settingsState.initializers[#settingsState.initializers + 1] = initializer
    end,
}
MinimalSliderWithSteppersMixin = { Label = { Left = 1, Right = 2, Top = 3, Min = 4, Max = 5 } }
-- CreateSettingsButtonInitializer(name, buttonText, buttonClick, tooltip, addSearchTags) and
-- CreateSettingsListSectionHeaderInitializer(name, tooltip): initializers for RegisterInitializer.
function CreateSettingsButtonInitializer(name, buttonText, buttonClick, tooltip, addSearchTags)
    assert(type(name) == "string" and type(buttonText) == "string" and type(buttonClick) == "function",
        "button initializer: name, text, click")
    return { kind = "button", name = name, buttonText = buttonText, click = buttonClick, tooltip = tooltip,
        addSearchTags = addSearchTags }
end
function CreateSettingsListSectionHeaderInitializer(name, tooltip)
    assert(type(name) == "string" and name ~= "", "section header: name")
    return { kind = "header", name = name, tooltip = tooltip }
end
function M.Settings()
    return settingsState
end
-- Shows a canvas page the way the settings panel does (SettingsPanelMixin:DisplayLayout):
-- reparent to the canvas, anchor to all of it, show, then OnRefresh (CallRefreshOnFrame).
function M.ShowCanvas(category)
    local frame = category.frame
    frame:SetParent(UIParent)
    frame:ClearAllPoints()
    frame:SetAllPoints(UIParent)
    frame:Show()
    if frame.OnRefresh then
        Call(frame.OnRefresh, frame)
    end
end
-- Leaves a canvas page the way the panel does (ClearCurrentCategoryCanvas): no parent, hidden.
-- A focused edit box loses its focus (OnEditFocusLost), as when a parent is hidden on the client.
function M.HideCanvas(category)
    local frame = category.frame
    if focused and focused ~= frame then
        local p = focused
        while p and p ~= frame do
            p = p.parent
        end
        if p == frame then
            focused:ClearFocus()
        end
    end
    frame:SetParent(nil)
    frame:ClearAllPoints()
    frame:Hide()
end
-- Runs an edit box script such as OnEnterPressed or OnEscapePressed.
function M.Press(editBox, script)
    if editBox.scripts[script] then
        Call(editBox.scripts[script], editBox)
    end
end

-- print goes to the default chat frame, like on the client.
print = function(...)
    local parts = {}
    for i = 1, select("#", ...) do
        parts[i] = tostring(select(i, ...))
    end
    local line = table.concat(parts, " ")
    printed[#printed + 1] = line
    DEFAULT_CHAT_FRAME:AddMessage(line, 1, 1, 1)
    if M.echo then
        rawprint("  | " .. line)
    end
end

local filters = {}
ChatFrameUtil = {
    AddMessageEventFilter = function(event, fn)
        filters[event] = filters[event] or {}
        table.insert(filters[event], fn)
    end,
    RemoveMessageEventFilter = function(event, fn)
        for i, f in ipairs(filters[event] or {}) do
            if f == fn then
                table.remove(filters[event], i)
                return
            end
        end
    end,
    ForEachChatFrame = function(fn)
        for _, name in ipairs(CHAT_FRAMES) do
            fn(_G[name])
        end
    end,
}

local nextLineID, lineStore = 100, {}
C_ChatInfo = {
    InChatMessagingLockdown = function()
        return state.lockdown
    end,
    GetChatLineText = function(id)
        if state.lockdown then
            return Secret()
        end
        return lineStore[id] and lineStore[id].text
    end,
    GetChatLineSenderName = function(id)
        if state.lockdown then
            return Secret()
        end
        return lineStore[id] and lineStore[id].sender
    end,
    GetChatLineSenderGUID = function(id)
        if state.lockdown then
            return Secret()
        end
        return lineStore[id] and lineStore[id].guid
    end,
    IsValidChatLine = function(id)
        return lineStore[id] ~= nil
    end,
    IsChatLineCensored = function()
        return false
    end,
}

local RememberPlayer   -- defined in the conversation window section

-- Delivers one chat event the way ChatFrameMixin:MessageEventHandler does, then to addon
-- frames. hidden = true runs the filters but no frame shows the line (a channel no tab shows).
-- opts (optional): language, channelString ("2. Trade - City"), target, flags, zoneChannelID,
-- channelIndex, channelBase (arg9, "Trade - City"), languageID, bnID (arg13), isMobile, class
-- (GetPlayerInfoByGUID's class for that GUID, default WARRIOR), noInfo (the GUID stays unknown).
-- guid defaults to "Player-1-0002", except on CHAT_MSG_BN_* events (nil); false means nil.
function M.Deliver(event, text, sender, guid, hidden, opts)
    opts = opts or {}
    nextLineID = nextLineID + 1
    local lineID = nextLineID
    sender = sender or "Someone"
    if guid == nil and not event:find("^CHAT_MSG_BN_") then
        guid = "Player-1-0002"
    end
    guid = guid or nil
    lineStore[lineID] = { text = text, sender = sender, guid = guid }
    if RememberPlayer then
        RememberPlayer(guid, sender, opts)
    end
    local secret = state.lockdown
    local args = pack(secret and Secret() or text, secret and Secret() or sender, opts.language or "",
        opts.channelString or "", opts.target or "", opts.flags or "", opts.zoneChannelID or 0, opts.channelIndex or 0,
        opts.channelBase or "", opts.languageID or 0, lineID, secret and Secret() or guid, opts.bnID or 0,
        opts.isMobile or false)
    for _, name in ipairs(CHAT_FRAMES) do
        local cf = _G[name]
        local a, discard = pack(unpack(args, 1, args.n)), false
        for _, fn in ipairs(filters[event] or {}) do
            -- the client runs filters only when every argument is accessible
            if canaccessvalue(unpack(a, 1, a.n)) then
                local res = pack(pcall(fn, cf, event, unpack(a, 1, a.n)))
                if not res[1] then
                    ReportError("filter: " .. tostring(res[2]))
                elseif res[2] then
                    discard = true
                    break
                elseif res[3] then
                    a = pack(unpack(res, 3, res.n))
                end
            end
        end
        if not discard and not hidden then
            local msg = secret and Secret() or (sender == "" and a[1] or ("[" .. sender .. "]: " .. a[1]))
            cf:AddMessage(msg, 1, 1, 1, 1, 0, 0, event, args, function(t)
                return t
            end)
        end
    end
    Fire(event, unpack(args, 1, args.n))
    return lineID
end

-- A click on a hyperlink (link = its data, such as "addon:TranslateProbe:t:123:456"), with Shift
-- and Ctrl held when given. It is a hardware event.
function M.Click(link, chatFrame, shift, ctrl)
    local was, wasCtrl, wasHw = state.shift, state.ctrl, state.hardware
    if shift ~= nil then
        state.shift = shift and true or false
    end
    if ctrl ~= nil then
        state.ctrl = ctrl and true or false
    end
    state.hardware = true
    EventRegistry:TriggerEvent("SetItemRef", link, "[T]", "LeftButton", chatFrame or ChatFrame1)
    state.shift, state.ctrl, state.hardware = was, wasCtrl, wasHw
end

function M.SetLockdown(on)
    state.lockdown = on and true or false
end

function M.Restriction(restrictionType, restrictionState)
    Fire("ADDON_RESTRICTION_STATE_CHANGED", restrictionType, restrictionState)
end

function M.ChatFrames()
    return { ChatFrame1, ChatFrame3 }
end

function M.History(cf)
    return cf.history
end

function M.Textures()
    local out = {}
    for _, t in ipairs(textures) do
        local x, y, w, h = Rect(t)
        out[#out + 1] = { frame = t.parent, texture = t, r = t.r, g = t.g, b = t.b, a = t.a, shown = IsVisible(t),
            x = x, y = y, w = w, h = h, strata = Strata(t), layer = t.layer }
    end
    return out
end

function M.Frames()
    return frames
end

function M.SetScreen(physW, physH, uiScale)
    local sizeChanged = physW ~= state.physW or physH ~= state.physH
    local scaleChanged = uiScale ~= nil and uiScale ~= state.uiScale
    state.physW, state.physH = physW, physH
    if uiScale then
        state.uiScale = uiScale
        UIParent.scale = uiScale
    end
    if state.loaded then
        if sizeChanged then
            Fire("DISPLAY_SIZE_CHANGED")
        end
        if scaleChanged then
            Fire("UI_SCALE_CHANGED")
        end
    end
end

function M.SetChatRect(cf, l, b, w, h)
    cf.scaledRect = { l, b, w, h }
    cf.FontStringContainer.scaledRect = cf.scaledRect
end

function M.SetRectSecret(cf, on)
    cf.secretRect = on and true or nil
    cf.FontStringContainer.secretRect = cf.secretRect
end

function M.SetFont(cf, path, h)
    cf.fontPath, cf.fontSize = path, h
end

function M.Printed()
    return printed
end

function M.Errors()
    return errors
end

-- Like the client, a changed value fires CVAR_UPDATE (name, value).
function M.SetCVar(name, value)
    local new = value ~= nil and tostring(value) or nil
    local changed = state.cvars[name] ~= new
    state.cvars[name] = new
    if changed then
        Fire("CVAR_UPDATE", name, new)
    end
end

function M.SetShift(down)
    state.shift = down and true or false
end

---------------------------------------------------------------------------- conversation window support
-- What the conversation window, the reply composer and their tests need from the client, modelled
-- on the Forever UI source (12.x mainline engine; Blizzard_SharedXML, Blizzard_ChatFrameBase) where
-- it matters. Values marked "mock value" (atlas sizes, sound kit ids, colours) are not verified on
-- the client; nothing should depend on their exact numbers. Behaviour that is an assumption rather
-- than a fact read in the UI source is marked "assumption" (G12/G13 in docs\IN-GAME-CHECKS.md check
-- those in game).
--
-- Input (each of these is a hardware event, so sends from it are allowed outdoors):
--   Mock.Paste(box, text)        Ctrl held, the text inserted at the cursor (replacing the highlight),
--                                one OnTextChanged(box, true), no OnChar; nothing when the box has no
--                                focus or SetSecurityDisablePaste was called. Returns true when pasted.
--   Mock.Type(box, text, opts)   per UTF-8 character: insert, OnTextChanged(box, true), OnChar(box, ch)
--                                (assumption: OnTextChanged before OnChar); opts.burst = true inserts
--                                the whole text with one OnTextChanged(true), then OnChar per
--                                character (an IME commit). Needs the focus. Returns true when typed.
--   Mock.Key(frame, key, mods)   OnKeyDown, then for a focused edit box ENTER -> OnEnterPressed,
--                                ESCAPE -> OnEscapePressed, TAB -> OnTabPressed, UP/DOWN/LEFT/RIGHT ->
--                                OnArrowPressed, BACKSPACE deletes (OnTextChanged(true)); then OnKeyUp.
--                                mods = { shift, ctrl, alt } held during the call.
--   Mock.ClickButton(b, mouseButton, mods)  a user click: OnMouseDown, OnMouseUp(b, button, true),
--                                then, only if enabled and registered with RegisterForClicks (default
--                                "LeftButtonUp"), PreClick, OnClick(b, button, false), PostClick; a
--                                CheckButton toggles first. (b:Click() is the programmatic API: no
--                                registration check and not a hardware event.)
--   Mock.MouseDown(f, button), Mock.MouseUp(f, button)   OnMouseDown / OnMouseUp only (they fire on a
--                                disabled button too: PanelResizeButtonMixin:OnMouseDown checks
--                                IsEnabled itself)
--   Mock.Hover(region) / Mock.Leave()   OnEnter / OnLeave (a disabled button only with
--                                SetMotionScriptsWhileDisabled); region:IsMouseOver() is true for the
--                                hovered region and its ancestors
--   Mock.GlobalMouseDown(button) GLOBAL_MOUSE_DOWN then GLOBAL_MOUSE_UP
--   Mock.Wheel(frame, delta)     OnMouseWheel
--   Mock.Drag(frame, dx, dy)     OnDragStart (when registered for drag), then, if the frame called
--                                StartMoving, every anchor moves by (dx, dy) units, then OnDragStop
--   Mock.Resize(frame, w, h)     a resize by the user: size clamped to SetResizeBounds, the frame
--                                re-anchored by its top-left when StartSizing was called, then
--                                OnSizeChanged (SetSize/SetWidth/SetHeight fire it too, at once)
--   Mock.SetCtrl(down), Mock.SetAlt(down), Mock.SetShift(down)
-- Chat:
--   Mock.SentChat() -> { { text, chatType, languageID, target, hardware, at, blocked, tooLong,
--                          singlePipe } }: every C_ChatInfo.SendChatMessage and C_BattleNet.SendWhisper
--                          call (chatType "BN_WHISPER", target = the account id). A SAY, YELL or
--                          CHANNEL send outside a hardware event (outdoors), or any send during the
--                          chat lockdown, is recorded with blocked = true and fires
--                          ADDON_ACTION_BLOCKED. Over 255 bytes: tooLong and an error.
--   Mock.EchoSent(record, opts)  the server's echo of a send (default: the last one): WHISPER ->
--                          CHAT_MSG_WHISPER_INFORM, BN_WHISPER -> CHAT_MSG_BN_WHISPER_INFORM, CHANNEL
--                          -> CHAT_MSG_CHANNEL, others CHAT_MSG_<type>, from the player (arg12 =
--                          UnitGUID("player")). opts.text replaces the echoed text (assumption: the
--                          echo carries the sent text unchanged). Returns the lineID.
--   Mock.SystemMessage(text); Mock.PlayerNotFound(name) (ERR_CHAT_PLAYER_NOT_FOUND_S)
--   Mock.ActionBlocked(addonName, func) fires ADDON_ACTION_BLOCKED
--   Mock.SetChannels({ [index] = "Trade - City", ... }) for GetChannelName / GetChannelList
--   Mock.SetChatColor(chatType, r, g, b) changes ChatTypeInfo and fires UPDATE_CHAT_COLOR
-- State: Mock.SetCombat(on) (fires PLAYER_REGEN_DISABLED / _ENABLED); Mock.SetGroup({ party, raid,
--   instance, guild, officer, inInstance, instanceType }); Mock.SetPlayerInfo(guid, info | false)
--   with info { class, name, realm }; Mock.SetBNAccount(id, info | nil); Mock.Player() -> { name,
--   realm, guid, fullName }
-- Art: Mock.SetAtlas(name, info | nil); Mock.SetLayoutMissing(name, on) (NineSliceUtil.GetLayout
--   returns nil, ApplyLayoutByName errors as on the client); Mock.SetTemplateMissing(name, on)
--   (C_XMLUtil.GetTemplateInfo returns nil, CreateFrame errors)
-- Recorders: Mock.Sounds() -> { soundKitID... }; Mock.FocusLog() -> { { box, at, hardware } } (every
--   SetFocus); Mock.Focused() -> the focused edit box or nil

local W = {}    -- this section's state (keeps the main chunk's local count low)
W.sentChat, W.sounds, W.channels, W.playerInfo, W.guidByName, W.bnAccounts = {}, {}, {}, {}, {}, {}
W.group = { party = false, raid = false, instance = false, guild = false, officer = false, inInstance = false,
    instanceType = "none" }
W.player = { name = "Tester", realm = "Mock Realm", normalized = "MockRealm", guid = PLAYER_GUID }
W.player.fullName = W.player.name .. "-" .. W.player.normalized
W.layoutMissing = {}

local function Utf8Chars(s)
    local out = {}
    for ch in s:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
        out[#out + 1] = ch
    end
    return out
end

-- Runs fn(...) with modifier keys held and as a hardware event (or not), then restores them.
local function WithInput(mods, hardware, fn, ...)
    local was = { state.shift, state.ctrl, state.alt, state.hardware }
    if mods then
        if mods.shift ~= nil then
            state.shift = mods.shift and true or false
        end
        if mods.ctrl ~= nil then
            state.ctrl = mods.ctrl and true or false
        end
        if mods.alt ~= nil then
            state.alt = mods.alt and true or false
        end
    end
    if hardware then
        state.hardware = true
    end
    local res = pack(pcall(fn, ...))
    state.shift, state.ctrl, state.alt, state.hardware = was[1], was[2], was[3], was[4]
    if not res[1] then
        error(res[2], 0)
    end
    return unpack(res, 2, res.n)
end

local function Script(frame, name, ...)
    local fn = frame.scripts and frame.scripts[name]
    if fn then
        return Call(fn, frame, ...)
    end
end

---------------------------------------------------------------------------- Lua helpers the game adds
date = os.date
GetServerTime = os.time
tinsert, tremove = table.insert, table.remove
function tContains(t, v)
    for _, x in ipairs(t) do
        if x == v then
            return true
        end
    end
    return false
end
function CopyTable(t)
    local c = {}
    for k, v in pairs(t) do
        c[k] = type(v) == "table" and CopyTable(v) or v
    end
    return c
end
function Round(v)
    if v < 0 then
        return math.ceil(v - 0.5)
    end
    return math.floor(v + 0.5)
end
function Clamp(v, lo, hi)
    if v < lo then
        return lo
    elseif v > hi then
        return hi
    end
    return v
end
local function CharClass(chars)
    return "[" .. chars:gsub("[%]%^%-%%]", "%%%0") .. "]"
end
function strtrim(s, chars)
    local cls = CharClass(chars or " \t\r\n")
    return (s:gsub("^" .. cls .. "+", ""):gsub(cls .. "+$", ""))
end
function strsplit(delim, str, pieces)
    local out, cls, start = {}, CharClass(delim), 1
    while not (pieces and #out == pieces - 1) do
        local a, b = str:find(cls, start)
        if not a then
            break
        end
        out[#out + 1] = str:sub(start, a - 1)
        start = b + 1
    end
    out[#out + 1] = str:sub(start)
    return unpack(out)
end
function strlenutf8(s)
    return Utf8Len(s)
end
function Mixin(object, ...)
    for i = 1, select("#", ...) do
        for k, v in pairs((select(i, ...))) do
            object[k] = v
        end
    end
    return object
end
function CreateFromMixins(...)
    return Mixin({}, ...)
end

---------------------------------------------------------------------------- colours
local ColorMixin = {}
function ColorMixin:SetRGBA(r, g, b, a)
    self.r, self.g, self.b, self.a = r, g, b, a
end
function ColorMixin:SetRGB(r, g, b)
    self:SetRGBA(r, g, b, nil)
end
function ColorMixin:GetRGB()
    return self.r, self.g, self.b
end
function ColorMixin:GetRGBA()
    return self.r, self.g, self.b, self.a or 1
end
function ColorMixin:GetRGBAsBytes()
    return Round(self.r * 255), Round(self.g * 255), Round(self.b * 255)
end
function ColorMixin:GenerateHexColor()
    return ("%02x%02x%02x%02x"):format(Round((self.a or 1) * 255), Round(self.r * 255), Round(self.g * 255),
        Round(self.b * 255))
end
function ColorMixin:GenerateHexColorMarkup()
    return "|c" .. self:GenerateHexColor()
end
function ColorMixin:WrapTextInColorCode(text)
    return "|c" .. self:GenerateHexColor() .. text .. "|r"
end
function ColorMixin:IsEqualTo(o)
    return self.r == o.r and self.g == o.g and self.b == o.b and (self.a or 1) == (o.a or 1)
end
function CreateColor(r, g, b, a)
    local c = Mixin({}, ColorMixin)
    c:SetRGBA(r, g, b, a)
    return c
end
NORMAL_FONT_COLOR = CreateColor(1.0, 0.82, 0.0)
HIGHLIGHT_FONT_COLOR = CreateColor(1.0, 1.0, 1.0)
GRAY_FONT_COLOR = CreateColor(0.5, 0.5, 0.5)
DISABLED_FONT_COLOR = CreateColor(0.5, 0.5, 0.5)
GREEN_FONT_COLOR = CreateColor(0.1, 1.0, 0.1)
RED_FONT_COLOR = CreateColor(1.0, 0.1, 0.1)
ORANGE_FONT_COLOR = CreateColor(1.0, 0.5, 0.25)
WHITE_FONT_COLOR = CreateColor(1.0, 1.0, 1.0)
YELLOW_FONT_COLOR = CreateColor(1.0, 1.0, 0.0)
LIGHTGRAY_FONT_COLOR = CreateColor(0.75, 0.75, 0.75)

-- Classes (mock values for the colours and icon coordinates)
local CLASSES = {
    WARRIOR = { 0.78, 0.61, 0.43, "Warrior", { 0, 0.25, 0, 0.25 } },
    MAGE = { 0.25, 0.78, 0.92, "Mage", { 0.25, 0.5, 0, 0.25 } },
    ROGUE = { 1.0, 0.96, 0.41, "Rogue", { 0.5, 0.75, 0, 0.25 } },
    DRUID = { 1.0, 0.49, 0.04, "Druid", { 0.75, 1, 0, 0.25 } },
    HUNTER = { 0.67, 0.83, 0.45, "Hunter", { 0, 0.25, 0.25, 0.5 } },
    SHAMAN = { 0.0, 0.44, 0.87, "Shaman", { 0.25, 0.5, 0.25, 0.5 } },
    PRIEST = { 1.0, 1.0, 1.0, "Priest", { 0.5, 0.75, 0.25, 0.5 } },
    WARLOCK = { 0.53, 0.53, 0.93, "Warlock", { 0.75, 1, 0.25, 0.5 } },
    PALADIN = { 0.96, 0.55, 0.73, "Paladin", { 0, 0.25, 0.5, 0.75 } },
    DEATHKNIGHT = { 0.77, 0.12, 0.23, "Death Knight", { 0.25, 0.5, 0.5, 0.75 } },
    MONK = { 0.0, 1.0, 0.6, "Monk", { 0.5, 0.75, 0.5, 0.75 } },
    DEMONHUNTER = { 0.64, 0.19, 0.79, "Demon Hunter", { 0.75, 1, 0.5, 0.75 } },
    EVOKER = { 0.2, 0.58, 0.5, "Evoker", { 0, 0.25, 0.75, 1 } },
}
RAID_CLASS_COLORS, CLASS_ICON_TCOORDS, LOCALIZED_CLASS_NAMES_MALE = {}, {}, {}
for file, c in pairs(CLASSES) do
    local color = CreateColor(c[1], c[2], c[3])
    color.colorStr = color:GenerateHexColor()
    RAID_CLASS_COLORS[file], CLASS_ICON_TCOORDS[file], LOCALIZED_CLASS_NAMES_MALE[file] = color, c[5], c[4]
end
C_ClassColor = {
    GetClassColor = function(file)
        return RAID_CLASS_COLORS[file]
    end,
}

---------------------------------------------------------------------------- global strings
do
    local STRINGS = {
        enUS = { SAY = "Say", YELL = "Yell", WHISPER = "Whisper", EMOTE = "Emote", PARTY = "Party", RAID = "Raid",
            RAID_WARNING = "Raid Warning", INSTANCE_CHAT = "Instance", GUILD = "Guild", OFFICER = "Officer",
            TRADE = "Trade", CHANNEL = "Channel", CLOSE = "Close", OKAY = "Okay", CANCEL = "Cancel", ACCEPT = "Accept",
            DECLINE = "Decline", HUD_EDIT_MODE_RESET_POSITION = "Reset Position",
            ERR_CHAT_PLAYER_NOT_FOUND_S = "No player named '%s' is currently playing." },
        frFR = { SAY = "Dire", YELL = "Crier", WHISPER = "Chuchoter", EMOTE = "Emote", PARTY = "Groupe", RAID = "Raid",
            RAID_WARNING = "Avertissement raid", INSTANCE_CHAT = "Instance", GUILD = "Guilde", OFFICER = "Officier",
            TRADE = "Commerce", CHANNEL = "Canal", CLOSE = "Fermer", OKAY = "D'accord", CANCEL = "Annuler",
            ACCEPT = "Accepter", DECLINE = "Refuser", HUD_EDIT_MODE_RESET_POSITION = "R\195\169initialiser la position",
            ERR_CHAT_PLAYER_NOT_FOUND_S = "Aucun joueur nomm\195\169 '%s' n'est actuellement en jeu." },
    }
    for k, v in pairs(STRINGS[GetLocale()] or STRINGS.enUS) do
        _G[k] = v
    end
end

---------------------------------------------------------------------------- chat types, channels, groups
do
    -- mock values close to the client's default chat colours
    local COLORS = {
        SAY = { 1, 1, 1 }, EMOTE = { 1, 0.5, 0.25 }, YELL = { 1, 0.25, 0.25 }, WHISPER = { 1, 0.5, 1 },
        WHISPER_INFORM = { 1, 0.5, 1 }, BN_WHISPER = { 0, 1, 0.965 }, BN_WHISPER_INFORM = { 0, 1, 0.965 },
        PARTY = { 0.667, 0.667, 1 }, PARTY_LEADER = { 0.463, 0.784, 1 }, RAID = { 1, 0.498, 0 },
        RAID_LEADER = { 1, 0.282, 0.035 }, RAID_WARNING = { 1, 0.282, 0 }, INSTANCE_CHAT = { 1, 0.498, 0 },
        INSTANCE_CHAT_LEADER = { 1, 0.282, 0.035 }, GUILD = { 0.251, 1, 0.251 }, OFFICER = { 0.251, 0.753, 0.251 },
        CHANNEL = { 1, 0.753, 0.753 }, SYSTEM = { 1, 1, 0 },
    }
    for i = 1, 20 do
        COLORS["CHANNEL" .. i] = { 1, 0.753, 0.753 }
    end
    ChatTypeInfo = {}
    local id = 0
    for k, c in pairs(COLORS) do
        id = id + 1
        ChatTypeInfo[k] = { r = c[1], g = c[2], b = c[3], id = id, sticky = 0 }
    end
end
function M.SetChatColor(chatType, r, g, b)
    local info = ChatTypeInfo[chatType]
    info.r, info.g, info.b = r, g, b
    Fire("UPDATE_CHAT_COLOR", chatType, r, g, b)
end

Constants = { ChatFrameConstants = { MaxChatChannels = 20, MaxChatWindows = 10 } }
LE_PARTY_CATEGORY_HOME, LE_PARTY_CATEGORY_INSTANCE = 1, 2

GetChannelName = function(x)
    local id = tonumber(x)
    if id then
        if W.channels[id] then
            return id, W.channels[id], 0, false
        end
        return 0, nil, 0, false
    end
    if type(x) == "string" then
        local want = x:lower()
        for i = 1, Constants.ChatFrameConstants.MaxChatChannels do
            local name = W.channels[i]
            if name and (name:lower() == want or (name:match("^(.-) %- ") or ""):lower() == want) then
                return i, name, 0, false
            end
        end
    end
    return 0, nil, 0, false
end
GetChannelList = function()
    local out = {}
    for i = 1, Constants.ChatFrameConstants.MaxChatChannels do
        if W.channels[i] then
            out[#out + 1], out[#out + 2], out[#out + 3] = i, W.channels[i], false
        end
    end
    return unpack(out)
end
function M.SetChannels(t)
    W.channels = {}
    for k, v in pairs(t or {}) do
        W.channels[k] = v
    end
end

IsInGroup = function(category)
    local g = W.group
    if category == LE_PARTY_CATEGORY_INSTANCE then
        return g.instance
    elseif category == LE_PARTY_CATEGORY_HOME then
        return g.party or g.raid
    end
    return g.party or g.raid or g.instance
end
IsInRaid = function(category)
    local g = W.group
    if category == LE_PARTY_CATEGORY_INSTANCE then
        return g.instance and g.raid
    end
    return g.raid
end
IsInGuild = function()
    return W.group.guild
end
IsInInstance = function()
    return W.group.inInstance, W.group.instanceType
end
CanEditOfficerNote = function()
    return W.group.guild and W.group.officer
end
C_GuildInfo = {
    IsGuildOfficer = function()
        return W.group.guild and W.group.officer
    end,
    CanEditOfficerNote = function()
        return W.group.guild and W.group.officer
    end,
    CanSpeakInGuildChat = function()
        return W.group.guild
    end,
}
function M.SetGroup(t)
    for k, v in pairs(t) do
        W.group[k] = v
    end
end

---------------------------------------------------------------------------- the player and senders
UnitName = function(unit)
    if unit == "player" then
        return W.player.name, nil
    end
    return nil
end
GetRealmName = function()
    return W.player.realm
end
GetNormalizedRealmName = function()
    return W.player.normalized
end
Ambiguate = function(fullName, context)
    local name, realm = tostring(fullName):match("^([^%-]+)%-(.+)$")
    if not name then
        return fullName
    end
    if context == "short" or realm == W.player.normalized then
        return name
    end
    return fullName
end
RememberPlayer = function(guid, sender, opts)
    if type(guid) ~= "string" or not guid:find("^Player%-") or opts.noInfo then
        return
    end
    if W.playerInfo[guid] and not opts.class then
        return
    end
    local name, realm = tostring(sender):match("^([^%-]+)%-(.+)$")
    name, realm = name or tostring(sender), realm or ""
    if realm == W.player.normalized then
        realm = ""
    end
    W.playerInfo[guid] = { class = opts.class or "WARRIOR", name = name, realm = realm }
    W.guidByName[sender] = guid
end
GetPlayerInfoByGUID = function(guid)
    local info
    if guid == PLAYER_GUID then
        info = { class = "WARRIOR", name = W.player.name, realm = "" }
    else
        info = W.playerInfo[guid]
    end
    if not info then
        return nil
    end
    return LOCALIZED_CLASS_NAMES_MALE[info.class], info.class, "Human", "Human", 2, info.name, info.realm
end
function M.SetPlayerInfo(guid, info)
    W.playerInfo[guid] = info or nil
end
function M.SetBNAccount(id, info)
    W.bnAccounts[id] = info
end
function M.Player()
    return { name = W.player.name, realm = W.player.normalized, guid = PLAYER_GUID, fullName = W.player.fullName }
end

---------------------------------------------------------------------------- sending chat
do
    local CHAT_TYPES = { SAY = true, EMOTE = true, YELL = true, PARTY = true, RAID = true, RAID_WARNING = true,
        INSTANCE_CHAT = true, GUILD = true, OFFICER = true, WHISPER = true, CHANNEL = true, AFK = true, DND = true,
        VOICE_TEXT = true }
    local HARDWARE_OUTDOORS = { SAY = true, YELL = true, CHANNEL = true }

    local function Record(text, chatType, languageID, target, api)
        local rec = { text = text, chatType = chatType, languageID = languageID, target = target,
            hardware = state.hardware, at = state.time }
        W.sentChat[#W.sentChat + 1] = rec
        if type(text) ~= "string" then
            ReportError(api .. ": the message must be a string")
            return rec
        end
        if #text > 255 then
            rec.tooLong = true
            ReportError(api .. ": message over 255 bytes (" .. #text .. ")")
        end
        -- canonical escaping leaves no single pipe (docs: Channels.Wire)
        rec.singlePipe = text:gsub("||", ""):find("|", 1, true) ~= nil
        local blocked = state.lockdown
            or (HARDWARE_OUTDOORS[chatType] and not state.hardware and not W.group.inInstance)
        if blocked then
            rec.blocked = true
            Fire("ADDON_ACTION_BLOCKED", "TranslateProbe", api .. "()")
        end
        return rec
    end

    C_ChatInfo.SendChatMessage = function(text, chatType, languageID, target)
        chatType = chatType or "SAY"
        if not CHAT_TYPES[chatType] then
            ReportError("SendChatMessage: unknown chat type " .. tostring(chatType))
        elseif chatType == "WHISPER" and (type(target) ~= "string" or target == "") then
            ReportError("SendChatMessage: a whisper needs a target name")
        elseif chatType == "CHANNEL" and not tonumber(target) then
            ReportError("SendChatMessage: a channel message needs the channel number")
        end
        Record(text, chatType, languageID, target, "C_ChatInfo.SendChatMessage")
    end

    C_BattleNet = {
        SendWhisper = function(bnetAccountID, text)
            if type(bnetAccountID) ~= "number" then
                ReportError("C_BattleNet.SendWhisper: the account id must be a number")
            end
            local rec = Record(text, "BN_WHISPER", nil, bnetAccountID, "C_BattleNet.SendWhisper")
            return not rec.blocked
        end,
        GetAccountInfoByID = function(id)
            return W.bnAccounts[id]
        end,
    }
end

function M.SentChat()
    return W.sentChat
end

function M.EchoSent(rec, opts)
    rec = rec or W.sentChat[#W.sentChat]
    opts = opts or {}
    local text = opts.text or rec.text
    local me = W.player.fullName
    if rec.chatType == "WHISPER" then
        return M.Deliver("CHAT_MSG_WHISPER_INFORM", text, rec.target, W.guidByName[rec.target] or false, false)
    elseif rec.chatType == "BN_WHISPER" then
        return M.Deliver("CHAT_MSG_BN_WHISPER_INFORM", text, opts.sender or ("|Kq" .. rec.target .. "|k"), false, false,
            { bnID = rec.target })
    elseif rec.chatType == "CHANNEL" then
        local index = tonumber(rec.target)
        local name = W.channels[index] or ""
        return M.Deliver("CHAT_MSG_CHANNEL", text, me, PLAYER_GUID, false, { channelString = index .. ". " .. name,
            channelIndex = index, channelBase = name })
    end
    return M.Deliver("CHAT_MSG_" .. rec.chatType, text, me, PLAYER_GUID, false)
end

function M.SystemMessage(text)
    return M.Deliver("CHAT_MSG_SYSTEM", text, "", false, false)
end

function M.PlayerNotFound(name)
    return M.SystemMessage(ERR_CHAT_PLAYER_NOT_FOUND_S:format(name))
end

function M.ActionBlocked(addonName, func)
    Fire("ADDON_ACTION_BLOCKED", addonName or "TranslateProbe", func or "SendChatMessage()")
end

---------------------------------------------------------------------------- screen size in UI units
GetScreenWidth = function()
    return UIParent:GetWidth()
end
GetScreenHeight = function()
    return UIParent:GetHeight()
end

---------------------------------------------------------------------------- input and combat state
IsControlKeyDown = function()
    return state.ctrl
end
IsAltKeyDown = function()
    return state.alt
end
GetCurrentKeyBoardFocus = function()
    return focused
end
InCombatLockdown = function()
    return state.combat
end
UnitAffectingCombat = function(unit)
    return unit == "player" and state.combat
end
function M.SetCtrl(down)
    state.ctrl = down and true or false
end
function M.SetAlt(down)
    state.alt = down and true or false
end
function M.SetCombat(on)
    on = on and true or false
    if on ~= state.combat then
        state.combat = on
        Fire(on and "PLAYER_REGEN_DISABLED" or "PLAYER_REGEN_ENABLED")
    end
end
function M.FocusLog()
    return focusLog
end
function M.Focused()
    return focused
end

---------------------------------------------------------------------------- sounds
SOUNDKIT = {   -- mock values
    IG_CHARACTER_INFO_OPEN = 839, IG_CHARACTER_INFO_CLOSE = 840, IG_CHARACTER_INFO_TAB = 841,
    IG_MAINMENU_OPEN = 850, IG_MAINMENU_CLOSE = 851, IG_MAINMENU_OPTION = 852,
    IG_MAINMENU_OPTION_CHECKBOX_ON = 856, IG_MAINMENU_OPTION_CHECKBOX_OFF = 857, U_CHAT_SCROLL_BUTTON = 1115,
    TELL_MESSAGE = 3081,
}
PlaySound = function(soundKitID)
    assert(type(soundKitID) == "number", "PlaySound takes a sound kit id")
    W.sounds[#W.sounds + 1] = soundKitID
    return true, #W.sounds
end
function M.Sounds()
    return W.sounds
end
UISpecialFrames = {}

---------------------------------------------------------------------------- font objects
do
    local FontMethods = {}
    FontMethods.__index = FontMethods
    function FontMethods:GetFont()
        return self.path, self.size, self.flags
    end
    function FontMethods:SetFont(path, size, flags)
        self.path, self.size, self.flags = path, size, flags or ""
        return true
    end
    function FontMethods:CopyFontObject(src)
        if type(src) == "string" then
            src = _G[src]
        end
        assert(type(src) == "table" and src.isFont, "CopyFontObject: not a font object")
        self.path, self.size, self.flags = src.path, src.size, src.flags
        self.color = { unpack(src.color) }
        self.shadowX, self.shadowY = src.shadowX, src.shadowY
    end
    function FontMethods:GetName()
        return self.name
    end
    function FontMethods:SetTextColor(r, g, b, a)
        self.color = { r, g, b, a or 1 }
    end
    function FontMethods:GetTextColor()
        return unpack(self.color)
    end
    function FontMethods:SetShadowOffset(x, y)
        self.shadowX, self.shadowY = x, y
    end
    function FontMethods:GetShadowOffset()
        return self.shadowX or 0, self.shadowY or 0
    end
    function FontMethods:SetShadowColor(r, g, b, a)
        self.shadowColor = { r, g, b, a or 1 }
    end
    function FontMethods:SetJustifyH(j)
        self.justifyH = j
    end
    function FontMethods:SetJustifyV(j)
        self.justifyV = j
    end
    function FontMethods:SetSpacing(n)
        self.spacing = n
    end
    function FontMethods:GetObjectType()
        return "Font"
    end
    local function NewFont(name, path, size, flags, color)
        local f = setmetatable({ isFont = true, name = name, path = path, size = size, flags = flags,
            color = { color[1], color[2], color[3], 1 }, shadowX = 1, shadowY = -1 }, FontMethods)
        if name then
            _G[name] = f
            M.KNOWN_FONTS[name] = f
        end
        return f
    end
    local FRIZ, ARIALN = "Fonts\\FRIZQT__.TTF", "Fonts\\ARIALN.TTF"
    local N, H, D = { 1, 0.82, 0 }, { 1, 1, 1 }, { 0.5, 0.5, 0.5 }
    for _, def in ipairs({ { "GameFontNormal", FRIZ, 12, N }, { "GameFontNormalSmall", FRIZ, 10, N },
        { "GameFontNormalLarge", FRIZ, 16, N }, { "GameFontHighlight", FRIZ, 12, H },
        { "GameFontHighlightSmall", FRIZ, 10, H }, { "GameFontDisable", FRIZ, 12, D },
        { "GameFontDisableSmall", FRIZ, 10, D }, { "ChatFontNormal", ARIALN, 14, H }, { "ChatFontSmall", ARIALN, 12, H } }) do
        NewFont(def[1], def[2], def[3], "", def[4])
    end
    CreateFont = function(name)
        return NewFont(name, FRIZ, 12, "", H)
    end
end

---------------------------------------------------------------------------- atlases, nine-slice, templates info
W.atlas = {   -- mock values (widths and heights)
    ["ChatBubble-Tail"] = { 16, 16 }, ["checkmark-minimal"] = { 16, 16 },
    ["ChatBubble-NineSlice-CornerTopLeft"] = { 16, 16 }, ["ChatBubble-NineSlice-CornerTopRight"] = { 16, 16 },
    ["ChatBubble-NineSlice-CornerBottomLeft"] = { 16, 16 }, ["ChatBubble-NineSlice-CornerBottomRight"] = { 16, 16 },
    ["_ChatBubble-NineSlice-EdgeTop"] = { 16, 16 }, ["_ChatBubble-NineSlice-EdgeBottom"] = { 16, 16 },
    ["!ChatBubble-NineSlice-EdgeLeft"] = { 16, 16 }, ["!ChatBubble-NineSlice-EdgeRight"] = { 16, 16 },
    ["ChatBubble-NineSlice-Center"] = { 16, 16 },
    ["uiframe-tab-left"] = { 15, 32 }, ["uiframe-tab-right"] = { 15, 32 }, ["_uiframe-tab-center"] = { 1, 32 },
    ["uiframe-activetab-left"] = { 16, 35 }, ["uiframe-activetab-right"] = { 16, 35 },
    ["_uiframe-activetab-center"] = { 1, 35 },
    ["common-search-border-left"] = { 8, 20 }, ["common-search-border-right"] = { 8, 20 },
    ["common-search-border-middle"] = { 10, 20 },
    ["RedButton-Exit"] = { 24, 24 }, ["RedButton-Exit-Disabled"] = { 24, 24 }, ["RedButton-exit-pressed"] = { 24, 24 },
    ["RedButton-Highlight"] = { 24, 24 },
}
C_Texture = {
    GetAtlasInfo = function(name)
        local a = W.atlas[name]
        if not a then
            return nil
        end
        return { width = a[1], height = a[2], leftTexCoord = 0, rightTexCoord = 1, topTexCoord = 0, bottomTexCoord = 1,
            tilesHorizontally = false, tilesVertically = false, file = 1, filename = "Interface\\Mock\\" .. name }
    end,
}
function M.SetAtlas(name, info)
    W.atlas[name] = info and { info.width, info.height } or nil
end

do
    local PIECES = { "TopLeftCorner", "TopRightCorner", "BottomLeftCorner", "BottomRightCorner", "TopEdge",
        "BottomEdge", "LeftEdge", "RightEdge", "Center" }
    local function Layout()
        local t = {}
        for _, p in ipairs(PIECES) do
            t[p] = {}
        end
        return t
    end
    -- the layout names of the Forever NineSliceLayouts.lua; pieces only matter for ChatBubble here
    local NAMES = { "SimplePanelTemplate", "PortraitFrameTemplate", "PortraitFrameTemplateMinimizable",
        "ButtonFrameTemplateNoPortrait", "ButtonFrameTemplateNoPortraitMinimizable",
        "ButtonFrameTemplateNoPortraitLessPadding", "InsetFrameTemplate", "GenericMetal", "Dialog",
        "WoodenNeutralFrameTemplate", "CharacterCreateDropdown", "SelectionFrameTemplate", "TooltipDefaultLayout",
        "TooltipDefaultDarkLayout", "TooltipMixedLayout", "IdenticalCornersLayoutNoCenter", "IdenticalCornersLayout",
        "UniqueCornersLayout", "ThreeSliceVerticalLayout", "ThreeSliceHorizontalLayout" }
    local layouts = {}
    for _, n in ipairs(NAMES) do
        layouts[n] = Layout()
    end
    layouts.ChatBubble = {
        TopLeftCorner = { atlas = "ChatBubble-NineSlice-CornerTopLeft" },
        TopRightCorner = { atlas = "ChatBubble-NineSlice-CornerTopRight" },
        BottomLeftCorner = { atlas = "ChatBubble-NineSlice-CornerBottomLeft" },
        BottomRightCorner = { atlas = "ChatBubble-NineSlice-CornerBottomRight" },
        TopEdge = { atlas = "_ChatBubble-NineSlice-EdgeTop" }, BottomEdge = { atlas = "_ChatBubble-NineSlice-EdgeBottom" },
        LeftEdge = { atlas = "!ChatBubble-NineSlice-EdgeLeft" }, RightEdge = { atlas = "!ChatBubble-NineSlice-EdgeRight" },
        Center = { atlas = "ChatBubble-NineSlice-Center" },
    }
    -- each piece's two anchors: { point, piece it is anchored to, that piece's point } (NineSlice.lua)
    local ANCHORS = {
        TopLeftCorner = { { "TOPLEFT", nil, "TOPLEFT" } }, TopRightCorner = { { "TOPRIGHT", nil, "TOPRIGHT" } },
        BottomLeftCorner = { { "BOTTOMLEFT", nil, "BOTTOMLEFT" } },
        BottomRightCorner = { { "BOTTOMRIGHT", nil, "BOTTOMRIGHT" } },
        TopEdge = { { "TOPLEFT", "TopLeftCorner", "TOPRIGHT" }, { "TOPRIGHT", "TopRightCorner", "TOPLEFT" } },
        BottomEdge = { { "BOTTOMLEFT", "BottomLeftCorner", "BOTTOMRIGHT" },
            { "BOTTOMRIGHT", "BottomRightCorner", "BOTTOMLEFT" } },
        LeftEdge = { { "TOPLEFT", "TopLeftCorner", "BOTTOMLEFT" }, { "BOTTOMLEFT", "BottomLeftCorner", "TOPLEFT" } },
        RightEdge = { { "TOPRIGHT", "TopRightCorner", "BOTTOMRIGHT" }, { "BOTTOMRIGHT", "BottomRightCorner", "TOPRIGHT" } },
        Center = { { "TOPLEFT", "TopLeftCorner", "BOTTOMRIGHT" }, { "BOTTOMRIGHT", "BottomRightCorner", "TOPLEFT" } },
    }
    NineSliceUtil = {
        GetLayout = function(name)
            if W.layoutMissing[name] then
                return nil
            end
            return layouts[name]
        end,
        AddLayout = function(name, layout)
            layouts[name] = layout
        end,
        -- as on the client, a missing layout (nil) is an error: callers check GetLayout first
        ApplyLayout = function(container, layout)
            for _, name in ipairs(PIECES) do
                local info = layout[name]
                if info then
                    local piece = container[name]
                    if not piece then
                        piece = container:CreateTexture(nil, "BORDER")
                        container[name] = piece
                    end
                    piece:ClearAllPoints()
                    if info.atlas then
                        piece:SetAtlas(info.atlas, true)
                    end
                    for _, a in ipairs(ANCHORS[name]) do
                        piece:SetPoint(a[1], a[2] and container[a[2]] or container, a[3], 0, 0)
                    end
                end
            end
            container.nineSliceLayout = layout
        end,
        ApplyLayoutByName = function(container, name)
            local layout = NineSliceUtil.GetLayout(name)
            if layout == nil then
                error("attempt to index a nil value (local 'userLayout')", 2)
            end
            NineSliceUtil.ApplyLayout(container, layout)
            container.nineSliceLayoutName = name
        end,
        SetLayoutShown = function(container, show)
            for _, name in ipairs(PIECES) do
                if container[name] then
                    container[name]:SetShown(show)
                end
            end
        end,
        DisableSharpening = function(container)
            for _, name in ipairs(PIECES) do
                if container[name] then
                    container[name]:SetTexelSnappingBias(0)
                    container[name]:SetSnapToPixelGrid(false)
                end
            end
        end,
    }
end
function M.SetLayoutMissing(name, missing)
    W.layoutMissing[name] = missing and true or nil
end

-- Template sizes (from the Forever XML where read, else mock values)
W.templateInfo = {
    UIPanelButtonTemplate = { "Button", 40, 22 }, InputBoxTemplate = { "EditBox", 0, 0 },
    InputBoxInstructionsTemplate = { "EditBox", 0, 0 }, PortraitFrameTemplate = { "Frame", 338, 424 },
    InsetFrameTemplate = { "Frame", 0, 0 }, PanelTabButtonTemplate = { "Button", 10, 32 },
    PanelTopTabButtonTemplate = { "Button", 10, 32 }, TabSystemTopButtonTemplate = { "Button", 10, 37 },
    PanelResizeButtonTemplate = { "Button", 16, 16 }, MinimalScrollBar = { "EventFrame", 8, 560 },
    UIPanelCloseButton = { "Button", 24, 24 }, UICheckButtonTemplate = { "CheckButton", 32, 32 },
    DialogBorderDarkTemplate = { "Frame", 0, 0 }, TooltipBackdropTemplate = { "Frame", 0, 0 },
    BackdropTemplate = { "Frame", 0, 0 },
}
C_XMLUtil = {
    GetTemplateInfo = function(name)
        local t = W.templateInfo[name]
        if not t or templateMissing[name] then
            return nil
        end
        return { type = t[1], width = t[2], height = t[3], keyValues = {}, inherits = nil }
    end,
}
function M.SetTemplateMissing(name, missing)
    templateMissing[name] = missing and true or nil
end

---------------------------------------------------------------------------- PixelUtil (Forever PixelUtil.lua semantics)
PixelUtil = {}
function PixelUtil.GetPixelToUIUnitFactor()
    local _, physH = GetPhysicalScreenSize()
    return 768.0 / physH
end
function PixelUtil.GetNearestPixelSize(uiUnitSize, layoutScale, minPixels)
    if uiUnitSize == 0 and (not minPixels or minPixels == 0) then
        return 0
    end
    local factor = PixelUtil.GetPixelToUIUnitFactor()
    local numPixels = Round((uiUnitSize * layoutScale) / factor)
    if minPixels then
        if uiUnitSize < 0 then
            numPixels = math.min(numPixels, -minPixels)
        else
            numPixels = math.max(numPixels, minPixels)
        end
    end
    return numPixels * factor / layoutScale
end
function PixelUtil.ConvertPixelsToUI(px, layoutScale)
    return PixelUtil.GetNearestPixelSize(px, layoutScale)
end
function PixelUtil.ConvertPixelsToUIForRegion(px, region)
    return PixelUtil.GetNearestPixelSize(px, region:GetEffectiveScale())
end
function PixelUtil.SetWidth(region, w, minPixels)
    region:SetWidth(PixelUtil.GetNearestPixelSize(w, region:GetEffectiveScale(), minPixels))
end
function PixelUtil.SetHeight(region, h, minPixels)
    region:SetHeight(PixelUtil.GetNearestPixelSize(h, region:GetEffectiveScale(), minPixels))
end
function PixelUtil.SetSize(region, w, h, minW, minH)
    PixelUtil.SetWidth(region, w, minW)
    PixelUtil.SetHeight(region, h, minH)
end
function PixelUtil.SetPoint(region, point, rel, relPoint, x, y, minX, minY)
    region:SetPoint(point, rel, relPoint, PixelUtil.GetNearestPixelSize(x, region:GetEffectiveScale(), minX),
        PixelUtil.GetNearestPixelSize(y, region:GetEffectiveScale(), minY))
end

---------------------------------------------------------------------------- shared widget methods
-- Region and frame methods of the 12.x API documentation (Simple*API) that the window may use.
function Methods:GetObjectType()
    if self.kind == "texture" then
        return "Texture"
    elseif self.kind == "fontstring" then
        return "FontString"
    end
    return self.frameType
end
function Methods:IsObjectType(t)
    local own = self:GetObjectType()
    return own == t or t == "Region" or t == "Object" or (self.kind == "frame" and t == "Frame")
        or (own == "CheckButton" and t == "Button")
end
function Methods:GetPoint(i)
    if self.allPoints then
        local pts = { { "TOPLEFT", self.allPoints, "TOPLEFT", 0, 0 }, { "BOTTOMRIGHT", self.allPoints, "BOTTOMRIGHT", 0, 0 } }
        local p = pts[i or 1]
        if p then
            return unpack(p)
        end
        return nil
    end
    local p = self.points[i or 1]
    if p then
        return p.point, p.rel, p.relPoint, p.x, p.y
    end
end
function Methods:GetNumPoints()
    return self.allPoints and 2 or #self.points
end
function Methods:ClearPoint(name)
    for i, p in ipairs(self.points) do
        if p.point == name then
            table.remove(self.points, i)
            return
        end
    end
end
function Methods:GetCenter()
    local l, b, w, h = self:GetRect()
    return l + w / 2, b + h / 2
end
function Methods:GetRight()
    local l, _, w = self:GetRect()
    return l + w
end
function Methods:SetIgnoreParentAlpha(on)
    self.ignoreParentAlpha = on
end
function Methods:IsIgnoringParentAlpha()
    return self.ignoreParentAlpha or false
end
function Methods:GetEffectiveAlpha()
    local a, r = 1, self
    while r do
        a = a * (r.alpha or 1)
        if r.ignoreParentAlpha then
            break
        end
        r = r.parent
    end
    return a
end
function Methods:IsMouseOver()
    local r = W.hovered
    while r do
        if r == self then
            return true
        end
        r = r.parent
    end
    return false
end
function Methods:IsMouseEnabled()
    return self.mouse or false
end
function Methods:IsMouseWheelEnabled()
    return self.mouseWheel or false
end
function Methods:EnableMouseMotion(on)
    self.mouseMotion = on
end
function Methods:IsMouseMotionEnabled()
    return self.mouseMotion or self.mouse or false
end
function Methods:SetMouseClickEnabled(on)
    self.mouseClick = on
end
function Methods:IsMouseClickEnabled()
    if self.mouseClick ~= nil then
        return self.mouseClick
    end
    return self.mouse or false
end
function Methods:SetMouseMotionEnabled(on)
    self.mouseMotion = on
end
function Methods:SetPropagateMouseClicks(on)
    self.propagateClicks = on
end
function Methods:SetPropagateMouseMotion(on)
    self.propagateMotion = on
end
function Methods:SetPassThroughButtons(...)
    self.passThrough = { ... }
end
function Methods:HasScript(name)
    return self.scripts ~= nil
end
-- Frames
function Methods:SetMovable(on)
    self.movable = on
end
function Methods:IsMovable()
    return self.movable or false
end
function Methods:SetResizable(on)
    self.resizable = on
end
function Methods:IsResizable()
    return self.resizable or false
end
function Methods:SetResizeBounds(minW, minH, maxW, maxH)
    self.resizeBounds = { minW, minH, maxW, maxH }
end
function Methods:GetResizeBounds()
    local b = self.resizeBounds or { 0, 0, 0, 0 }
    return b[1], b[2], b[3], b[4]
end
function Methods:StartMoving()
    assert(self.movable, "StartMoving: the frame is not movable")
    self.moving = true
end
function Methods:StartSizing(point)
    assert(self.resizable, "StartSizing: the frame is not resizable")
    self.sizing = point or "BOTTOMRIGHT"
end
function Methods:StopMovingOrSizing()
    self.moving, self.sizing = nil, nil
end
function Methods:IsDragging()
    return self.moving or false
end
function Methods:RegisterForDrag(...)
    self.dragButtons = {}
    for i = 1, select("#", ...) do
        self.dragButtons[(select(i, ...))] = true
    end
end
function Methods:SetClampedToScreen(on)
    self.clamped = on
end
function Methods:IsClampedToScreen()
    return self.clamped or false
end
function Methods:SetClampRectInsets(l, r, t, b)
    self.clampInsets = { l, r, t, b }
end
function Methods:GetClampRectInsets()
    local c = self.clampInsets or { 0, 0, 0, 0 }
    return c[1], c[2], c[3], c[4]
end
function Methods:SetToplevel(on)
    self.toplevel = on
end
function Methods:IsToplevel()
    return self.toplevel or false
end
function Methods:Raise()
    self.raised = (self.raised or 0) + 1
end
function Methods:Lower()
    self.lowered = (self.lowered or 0) + 1
end
function Methods:SetDontSavePosition(on)
    self.dontSavePosition = on
end
function Methods:GetDontSavePosition()
    return self.dontSavePosition or false
end
function Methods:SetUserPlaced(on)
    self.userPlaced = on
end
function Methods:IsUserPlaced()
    return self.userPlaced or false
end
function Methods:EnableKeyboard(on)
    self.keyboard = on
end
function Methods:IsKeyboardEnabled()
    return self.keyboard or false
end
function Methods:SetPropagateKeyboardInput(on)
    self.propagateKeyboard = on
end
function Methods:GetPropagateKeyboardInput()
    return self.propagateKeyboard or false
end
function Methods:SetHitRectInsets(l, r, t, b)
    self.hitInsets = { l, r, t, b }
end
function Methods:GetHitRectInsets()
    local c = self.hitInsets or { 0, 0, 0, 0 }
    return c[1], c[2], c[3], c[4]
end
function Methods:SetFixedFrameStrata(on)
    self.fixedStrata = on
end
function Methods:HasFixedFrameStrata()
    return self.fixedStrata or false
end
function Methods:SetFixedFrameLevel(on)
    self.fixedLevel = on
end
function Methods:HasFixedFrameLevel()
    return self.fixedLevel or false
end
function Methods:SetUsingParentLevel(on)
    self.useParentLevel = on
end
function Methods:SetClipsChildren(on)
    self.clipsChildren = on
end
function Methods:DoesClipChildren()
    return self.clipsChildren or false
end
function Methods:SetHighlightLocked(on)
    self.highlightLocked = on and true or false
end
function Methods:IsHighlightLocked()
    return self.highlightLocked or false
end
function Methods:LockHighlight()
    self.highlightLocked = true
end
function Methods:UnlockHighlight()
    self.highlightLocked = false
end
function Methods:GetChildren()
    return unpack(self.children or {})
end
function Methods:GetNumChildren()
    return #(self.children or {})
end
function Methods:GetRegions()
    return unpack(self.regions or {})
end
function Methods:GetNumRegions()
    return #(self.regions or {})
end
-- Textures
function Methods:SetAtlas(atlas, useAtlasSize)
    self.atlas, self.file = atlas, nil
    local info = W.atlas[atlas]
    self.atlasMissing = info == nil
    if useAtlasSize and info then
        self.w, self.h = info[1], info[2]
    end
end
function Methods:GetAtlas()
    return self.atlas
end
function Methods:SetTexture(file)
    self.file, self.atlas = file, nil
end
function Methods:GetTexture()
    return self.file or self.atlas
end
function Methods:SetTexCoord(...)
    self.texCoord = { ... }
end
function Methods:GetTexCoord()
    return unpack(self.texCoord or { 0, 0, 0, 1, 1, 0, 1, 1 })
end
function Methods:ResetTexCoord()
    self.texCoord = nil
end
function Methods:SetBlendMode(mode)
    self.blendMode = mode
end
function Methods:GetBlendMode()
    return self.blendMode or "BLEND"
end
function Methods:SetDesaturated(on)
    self.desaturated = on
end
function Methods:IsDesaturated()
    return self.desaturated or false
end
function Methods:SetDesaturation(v)
    self.desaturated = v > 0
end
function Methods:SetRotation(r)
    self.rotation = r
end
function Methods:GetRotation()
    return self.rotation or 0
end
function Methods:SetGradient(orientation, minColor, maxColor)
    self.gradient = { orientation, minColor, maxColor }
end
function Methods:SetHorizTile(on)
    self.horizTile = on
end
function Methods:GetHorizTile()
    return self.horizTile or false
end
function Methods:SetVertTile(on)
    self.vertTile = on
end
function Methods:GetVertTile()
    return self.vertTile or false
end
function Methods:SetTextureSliceMargins(l, t, r, b)
    self.sliceMargins = { l, t, r, b }
end
function Methods:SetTextureSliceMode(mode)
    self.sliceMode = mode
end
function Methods:GetDrawLayer()
    return self.layer, self.subLevel or 0
end
function Methods:SetDrawLayer(layer, subLevel)
    self.layer, self.subLevel = layer, subLevel
end
function Methods:GetVertexColor()
    return self.r, self.g, self.b, self.a
end
function Methods:IsSnappingToPixelGrid()
    return self.snap ~= false
end
function Methods:GetTexelSnappingBias()
    return self.texelBias or 0
end
-- Font strings
function Methods:SetFormattedText(fmt, ...)
    self:SetText(fmt:format(...))
end
function Methods:SetFontObject(font)
    if type(font) == "string" then
        font = _G[font]
    end
    self.fontObject = font
    self.fontPath, self.fontSize, self.fontFlags = nil, nil, nil
end
function Methods:GetFontObject()
    return self.fontObject
end
function Methods:SetMaxLines(n)
    self.maxLines = n
end
function Methods:GetMaxLines()
    return self.maxLines or 0
end
function Methods:SetSpacing(n)
    self.lineSpacing = n
end
function Methods:GetSpacing()
    return self.lineSpacing or 0
end
function Methods:SetShadowOffset(x, y)
    self.shadowX, self.shadowY = x, y
end
function Methods:GetShadowOffset()
    return self.shadowX or 0, self.shadowY or 0
end
function Methods:SetShadowColor(r, g, b, a)
    self.shadowColor = { r, g, b, a or 1 }
end
function Methods:GetShadowColor()
    return unpack(self.shadowColor or { 0, 0, 0, 0 })
end
function Methods:SetJustifyH(j)
    self.justifyH = j
end
function Methods:GetJustifyH()
    return self.justifyH or "CENTER"
end
function Methods:SetJustifyV(j)
    self.justifyV = j
end
function Methods:GetJustifyV()
    return self.justifyV or "MIDDLE"
end
function Methods:GetTextColor()
    local c = self.textColor
    if c then
        return c[1], c[2], c[3], c[4]
    end
    local fo = self.fontObject
    if type(fo) == "table" and fo.isFont then
        return fo:GetTextColor()
    end
    return 1, 1, 1, 1
end
function Methods:IsTruncated()
    return (self.w or 0) > 0 and self:GetUnboundedStringWidth() > self.w + 1e-9
end
function Methods:GetLineHeight()
    return FontSize(self)
end
function Methods:SetTextScale(s)
    self.textScale = s
end
function Methods:GetTextScale()
    return self.textScale or 1
end

---------------------------------------------------------------------------- widget types
local function TypeTable(base)
    local t = setmetatable({}, { __index = base })
    return t, { __index = t }
end
local ButtonMethods, CheckButtonMethods, EditBoxMethods, ScrollFrameMethods
ButtonMethods, TypeMT.Button = TypeTable(Methods)
CheckButtonMethods, TypeMT.CheckButton = TypeTable(ButtonMethods)
EditBoxMethods, TypeMT.EditBox = TypeTable(Methods)
ScrollFrameMethods, TypeMT.ScrollFrame = TypeTable(Methods)

-- Buttons: the text lives in a font string (the template's Text, or one made on first SetText).
local function ButtonFont(self)
    if not self.fontString then
        local fs = self:CreateFontString(nil, "ARTWORK", self.normalFont)
        fs:SetPoint("CENTER", self, "CENTER", 0, 0)
        self.fontString = fs
    end
    return self.fontString
end
function ButtonMethods:SetText(text)
    self.text = text == nil and "" or tostring(text)
    ButtonFont(self):SetText(self.text)
end
function ButtonMethods:GetText()
    return self.text
end
function ButtonMethods:SetFormattedText(fmt, ...)
    self:SetText(fmt:format(...))
end
function ButtonMethods:GetFontString()
    return self.fontString
end
function ButtonMethods:SetFontString(fs)
    self.fontString = fs
end
function ButtonMethods:GetTextWidth()
    return self.fontString and self.fontString:GetStringWidth() or 0
end
function ButtonMethods:GetTextHeight()
    return self.fontString and self.fontString:GetStringHeight() or 0
end
function ButtonMethods:SetNormalFontObject(font)
    self.normalFont = font
    if self.fontString then
        self.fontString:SetFontObject(font)
    end
end
function ButtonMethods:GetNormalFontObject()
    return self.normalFont
end
function ButtonMethods:SetHighlightFontObject(font)
    self.highlightFont = font
end
function ButtonMethods:GetHighlightFontObject()
    return self.highlightFont
end
function ButtonMethods:SetDisabledFontObject(font)
    self.disabledFont = font
end
function ButtonMethods:GetDisabledFontObject()
    return self.disabledFont
end
function ButtonMethods:SetButtonState(s, locked)
    self.buttonState, self.buttonStateLocked = s, locked
end
function ButtonMethods:GetButtonState()
    return self.buttonState or (self.disabled and "DISABLED" or "NORMAL")
end
function ButtonMethods:RegisterForClicks(...)
    self.clicks = {}
    for i = 1, select("#", ...) do
        self.clicks[(select(i, ...))] = true
    end
end
function ButtonMethods:SetMotionScriptsWhileDisabled(on)
    self.motionWhileDisabled = on and true or false
end
function ButtonMethods:GetMotionScriptsWhileDisabled()
    return self.motionWhileDisabled or false
end
function ButtonMethods:SetPushedTextOffset(x, y)
    self.pushedTextOffset = { x, y }
end
for _, part in ipairs({ "Normal", "Pushed", "Highlight", "Disabled" }) do
    local key = part:lower() .. "Texture"
    local function Get(self)
        if not self[key] then
            self[key] = self:CreateTexture(nil, part == "Highlight" and "HIGHLIGHT" or "ARTWORK")
        end
        return self[key]
    end
    ButtonMethods["Set" .. part .. "Texture"] = function(self, asset)
        if type(asset) == "table" then
            self[key] = asset
        else
            Get(self):SetTexture(asset)
        end
    end
    ButtonMethods["Set" .. part .. "Atlas"] = function(self, atlas)
        Get(self):SetAtlas(atlas)
    end
    ButtonMethods["Get" .. part .. "Texture"] = function(self)
        return self[key]
    end
end
-- Programmatic click (the addon may call it): no registration check, not a hardware event.
function ButtonMethods:Click(button)
    if self.disabled then
        return
    end
    button = button or "LeftButton"
    if self.frameType == "CheckButton" then
        self.checked = not self.checked
    end
    Script(self, "PreClick", button, false)
    Script(self, "OnClick", button, false)
    Script(self, "PostClick", button, false)
end
function CheckButtonMethods:SetChecked(on)
    self.checked = on and true or false
end
function CheckButtonMethods:GetChecked()
    return self.checked or false
end
function CheckButtonMethods:SetCheckedTexture(asset)
    self.checkedTexture = asset
end
function CheckButtonMethods:GetCheckedTexture()
    return self.checkedTexture
end

-- Edit boxes: text, cursor (a byte offset), highlight, byte and letter limits.
local function CutToLimits(self, text)
    local maxBytes, maxLetters = self.maxBytes or 0, self.maxLetters or 0
    if maxLetters > 0 and Utf8Len(text) > maxLetters then
        local chars = Utf8Chars(text)
        text = table.concat(chars, "", 1, maxLetters)
    end
    if maxBytes > 0 and #text > maxBytes then
        local i = maxBytes
        while i > 0 and text:byte(i + 1) and text:byte(i + 1) >= 128 and text:byte(i + 1) < 192 do
            i = i - 1
        end
        text = text:sub(1, i)
    end
    return text
end
-- Inserts at the cursor (replacing the highlight); returns true when the text changed.
local function InsertAtCursor(self, s)
    local text = self.text or ""
    local a, b = self.cursor or #text, self.cursor or #text
    if self.highlight then
        a, b = self.highlight[1], self.highlight[2]
        self.highlight = nil
    end
    a, b = math.max(0, math.min(a, #text)), math.max(0, math.min(b, #text))
    local before, after = text:sub(1, a), text:sub(b + 1)
    local new = CutToLimits(self, before .. s .. after)
    local changed = new ~= text
    self.text = new
    self.cursor = math.min(#new, #CutToLimits(self, before .. s))
    return changed
end
function EditBoxMethods:SetText(text)
    text = CutToLimits(self, text == nil and "" or tostring(text))
    local changed = text ~= self.text
    self.text, self.cursor, self.highlight = text, #text, nil
    if changed then
        Script(self, "OnTextChanged", false)
    end
end
function EditBoxMethods:Insert(s)
    if InsertAtCursor(self, tostring(s)) then
        Script(self, "OnTextChanged", false)
    end
end
function EditBoxMethods:HasText()
    return (self.text or "") ~= ""
end
function EditBoxMethods:GetNumLetters()
    return Utf8Len(self.text or "")
end
function EditBoxMethods:SetMaxBytes(n)
    self.maxBytes = n
end
function EditBoxMethods:GetMaxBytes()
    return self.maxBytes or 0
end
function EditBoxMethods:GetMaxLetters()
    return self.maxLetters or 0
end
function EditBoxMethods:GetCursorPosition()
    return self.cursor or 0
end
function EditBoxMethods:SetCursorPosition(pos)
    self.cursor = math.max(0, math.min(pos or 0, #(self.text or "")))
end
function EditBoxMethods:GetUTF8CursorPosition()
    return Utf8Len((self.text or ""):sub(1, self.cursor or 0))
end
function EditBoxMethods:HighlightText(start, stop)
    local n = #(self.text or "")
    start, stop = start or 0, stop or n
    if stop < 0 then
        stop = n
    end
    self.highlight = start < stop and { start, stop } or nil
end
function EditBoxMethods:ClearHighlightText()
    self.highlight = nil
end
function EditBoxMethods:SetMultiLine(on)
    self.multiLine = on
end
function EditBoxMethods:IsMultiLine()
    return self.multiLine or false
end
function EditBoxMethods:SetSecurityDisablePaste()
    self.pasteDisabled = true
end
function EditBoxMethods:SetCountInvisibleLetters(on)
    self.countInvisible = on
end
function EditBoxMethods:SetHistoryLines(n)
    self.historyLines = n
end
function EditBoxMethods:GetHistoryLines()
    return self.historyLines or 0
end
function EditBoxMethods:AddHistoryLine(line)
    self.history = self.history or {}
    self.history[#self.history + 1] = line
end
function EditBoxMethods:ClearHistory()
    self.history = {}
end
function EditBoxMethods:IsInIMECompositionMode()
    return false
end
function EditBoxMethods:SetAltArrowKeyMode(on)
    self.altArrow = on
end
function EditBoxMethods:GetAltArrowKeyMode()
    return self.altArrow or false
end
function EditBoxMethods:SetVisibleTextByteLimit(n)
    self.visibleBytes = n
end
function EditBoxMethods:GetVisibleTextByteLimit()
    return self.visibleBytes or 0
end
function EditBoxMethods:SetTextInsets(l, r, t, b)
    self.insets = { l, r, t, b }
end
function EditBoxMethods:GetTextInsets()
    local c = self.insets or { 0, 0, 0, 0 }
    return c[1], c[2], c[3], c[4]
end
function EditBoxMethods:IsAutoFocus()
    return self.autoFocus ~= false
end
function EditBoxMethods:GetDisplayText()
    return self.text
end

-- Scroll frames: a plain ScrollFrame with a scroll child moved by the scroll offsets (Rect).
function ScrollFrameMethods:SetScrollChild(child)
    if self.scrollChild then
        self.scrollChild.scrollFrame = nil
    end
    self.scrollChild = child
    child.scrollFrame = self
end
function ScrollFrameMethods:GetScrollChild()
    return self.scrollChild
end
function ScrollFrameMethods:GetVerticalScrollRange()
    if not self.scrollChild then
        return 0
    end
    return math.max(0, self.scrollChild:GetHeight() - self:GetHeight())
end
function ScrollFrameMethods:GetHorizontalScrollRange()
    if not self.scrollChild then
        return 0
    end
    return math.max(0, self.scrollChild:GetWidth() - self:GetWidth())
end
function ScrollFrameMethods:SetVerticalScroll(v)
    self.vscroll = v
    Script(self, "OnVerticalScroll", v)
end
function ScrollFrameMethods:GetVerticalScroll()
    return self.vscroll or 0
end
function ScrollFrameMethods:SetHorizontalScroll(v)
    self.hscroll = v
    Script(self, "OnHorizontalScroll", v)
end
function ScrollFrameMethods:GetHorizontalScroll()
    return self.hscroll or 0
end
function ScrollFrameMethods:UpdateScrollChildRect()
    Script(self, "OnScrollRangeChanged", self:GetHorizontalScrollRange(), self:GetVerticalScrollRange())
end

---------------------------------------------------------------------------- Blizzard UI functions the templates use
GetAppropriateTooltip = function()
    return GameTooltip
end
HideUIPanel = function(frame)
    frame:Hide()
end
function HideParentPanel(self)
    HideUIPanel(self:GetParent())
end
function UIPanelCloseButton_OnClick(self)
    local parent = self:GetParent()
    if parent and (not parent.onCloseCallback or parent.onCloseCallback(self)) then
        HideUIPanel(parent)
    end
end
function GameTooltip_SetTitle(tooltip, text, color)
    color = color or NORMAL_FONT_COLOR
    tooltip:SetText(text, color.r, color.g, color.b)
end
function GameTooltip_AddColoredLine(tooltip, text, color)
    tooltip:AddLine(text, color.r, color.g, color.b)
end
function GameTooltip_AddNormalLine(tooltip, text)
    GameTooltip_AddColoredLine(tooltip, text, NORMAL_FONT_COLOR)
end
function GameTooltip_AddHighlightLine(tooltip, text)
    GameTooltip_AddColoredLine(tooltip, text, HIGHLIGHT_FONT_COLOR)
end
function GameTooltip_AddInstructionLine(tooltip, text)
    GameTooltip_AddColoredLine(tooltip, text, GREEN_FONT_COLOR)
end
function GameTooltip_AddErrorLine(tooltip, text)
    GameTooltip_AddColoredLine(tooltip, text, RED_FONT_COLOR)
end
function GameTooltip_Hide()
    GameTooltip:Hide()
end
function EditBox_ClearFocus(self)
    self:ClearFocus()
end
function EditBox_HighlightText(self)
    self:HighlightText()
end
function EditBox_ClearHighlight(self)
    self:HighlightText(0, 0)
end
function EditBox_OnTabPressed(self)
    if self.previousEditBox and IsShiftKeyDown() then
        self.previousEditBox:SetFocus()
    elseif self.nextEditBox then
        self.nextEditBox:SetFocus()
    end
end
function InputBoxInstructions_OnTextChanged(self)
    self.Instructions:SetShown(self:GetText() == "")
end
function InputBoxInstructions_OnDisable(self)
    if self.disabledColor then
        self:SetTextColor(self.disabledColor:GetRGBA())
    end
end
function InputBoxInstructions_OnEnable(self)
    if self.enabledColor then
        self:SetTextColor(self.enabledColor:GetRGBA())
    end
end

-- PanelTemplates_* (Forever SharedUIPanelTemplates.lua semantics: the selected tab is disabled, so
-- it gets no OnClick; motion scripts still run on it)
local function TabAt(frame, i)
    return frame.Tabs and frame.Tabs[i] or _G[(frame:GetName() or "") .. "Tab" .. i]
end
local TAB_SIDES_PADDING = 20
function PanelTemplates_GetSelectedTab(frame)
    return frame.selectedTab
end
function PanelTemplates_SelectTab(tab)
    tab.Left:Hide()
    tab.Middle:Hide()
    tab.Right:Hide()
    tab:Disable()
    tab:SetDisabledFontObject(GameFontHighlightSmall)
    local y = tab.selectedTextY or -3
    if tab.isTopTab then
        y = -y - 7
    end
    tab.Text:SetPoint("CENTER", tab, "CENTER", tab.selectedTextX or 0, y)
    tab.LeftActive:Show()
    tab.MiddleActive:Show()
    tab.RightActive:Show()
    if GameTooltip:IsOwned(tab) then
        GameTooltip:Hide()
    end
end
function PanelTemplates_DeselectTab(tab)
    tab.Left:Show()
    tab.Middle:Show()
    tab.Right:Show()
    tab:Enable()
    local y = tab.deselectedTextY or 2
    if tab.isTopTab then
        y = -y - 6
    end
    tab.Text:SetPoint("CENTER", tab, "CENTER", tab.deselectedTextX or 0, y)
    tab.LeftActive:Hide()
    tab.MiddleActive:Hide()
    tab.RightActive:Hide()
end
local function DisabledTabState(tab)
    tab.Left:Show()
    tab.Middle:Show()
    tab.Right:Show()
    tab:Disable()
    tab:SetDisabledFontObject(GameFontDisableSmall)
    tab.LeftActive:Hide()
    tab.MiddleActive:Hide()
    tab.RightActive:Hide()
end
function PanelTemplates_UpdateTabs(frame)
    if frame.selectedTab then
        for i = 1, frame.numTabs do
            local tab = TabAt(frame, i)
            if tab.isDisabled then
                DisabledTabState(tab)
            elseif i == frame.selectedTab then
                PanelTemplates_SelectTab(tab)
            else
                PanelTemplates_DeselectTab(tab)
            end
        end
    end
end
function PanelTemplates_SetTab(frame, id)
    frame.selectedTab = id
    PanelTemplates_UpdateTabs(frame)
end
function PanelTemplates_GetTabWidth(tab)
    return tab:GetTextWidth() + tab.Left:GetWidth() + tab.Right:GetWidth()
end
function PanelTemplates_TabResize(tab, padding, absoluteSize, minWidth, maxWidth, absoluteTextSize)
    tab.Text:SetWidth(absoluteTextSize or 0)
    local textWidth = tab.Text:GetStringWidth()
    local width = textWidth + TAB_SIDES_PADDING + (padding or 0)
    local sides = tab.Left:GetWidth() + tab.Right:GetWidth()
    minWidth = minWidth or sides
    if absoluteSize then
        width = math.max(absoluteSize, sides)
        textWidth = width - TAB_SIDES_PADDING - (padding or 0)
    elseif maxWidth and width > maxWidth then
        width = maxWidth
        textWidth = width - TAB_SIDES_PADDING - (padding or 0)
    elseif minWidth and width < minWidth then
        width = minWidth
        textWidth = width - TAB_SIDES_PADDING - (padding or 0)
    end
    tab.Text:SetWidth(textWidth)
    tab:SetWidth(width)
end
function PanelTemplates_ResizeTabsToFit(frame, maxWidthForAllTabs)
    local selected = frame.selectedTab
    if not selected then
        return
    end
    local total, truncated = 0, false
    for i = 1, frame.numTabs do
        local tab = TabAt(frame, i)
        total = total + tab:GetWidth()
        truncated = truncated or (tab.Text and tab.Text:IsTruncated())
    end
    if not truncated and total <= maxWidthForAllTabs then
        return
    end
    local current = TabAt(frame, selected)
    PanelTemplates_TabResize(current, 0)
    local each = (maxWidthForAllTabs - current:GetWidth()) / (frame.numTabs - 1)
    for i = 1, frame.numTabs do
        if i ~= selected then
            PanelTemplates_TabResize(TabAt(frame, i), 0, each)
        end
    end
end
function PanelTemplates_AnchorTabs(frame)
    for i = 2, frame.numTabs do
        TabAt(frame, i):SetPoint("TOPLEFT", TabAt(frame, i - 1), "TOPRIGHT", 3, 0)
    end
end
function PanelTemplates_SetNumTabs(frame, numTabs)
    frame.numTabs = numTabs
    PanelTemplates_AnchorTabs(frame)
end
function PanelTemplates_DisableTab(frame, index)
    TabAt(frame, index).isDisabled = 1
    PanelTemplates_UpdateTabs(frame)
end
function PanelTemplates_EnableTab(frame, index)
    local tab = TabAt(frame, index)
    tab.isDisabled = nil
    tab:SetDisabledFontObject(GameFontHighlightSmall)
    PanelTemplates_UpdateTabs(frame)
end
function PanelTemplates_SetTabEnabled(frame, index, enabled)
    if enabled then
        PanelTemplates_EnableTab(frame, index)
    else
        PanelTemplates_DisableTab(frame, index)
    end
end
function PanelTemplates_HideTab(frame, index)
    TabAt(frame, index):Hide()
end
function PanelTemplates_ShowTab(frame, index)
    TabAt(frame, index):Show()
end
function PanelTemplates_SetTabShown(frame, index, shown)
    TabAt(frame, index):SetShown(shown)
end

ScrollUtil = {
    -- a plain ScrollFrame driven by a scroll bar (the bar follows the frame's scroll offset)
    InitScrollFrameWithScrollBar = function(scrollFrame, scrollBar)
        scrollFrame.scrollBar, scrollBar.scrollFrame = scrollBar, scrollFrame
        local function Sync()
            local range = scrollFrame:GetVerticalScrollRange()
            scrollBar.scrollPercentage = range > 0 and scrollFrame:GetVerticalScroll() / range or 0
            local h = scrollFrame:GetHeight()
            local ch = scrollFrame.scrollChild and scrollFrame.scrollChild:GetHeight() or h
            scrollBar.visibleExtent = ch > 0 and math.min(1, h / ch) or 1
        end
        scrollFrame:HookScript("OnVerticalScroll", Sync)
        scrollFrame:HookScript("OnScrollRangeChanged", Sync)
        Sync()
    end,
}

---------------------------------------------------------------------------- templates
-- Each builds the children and scripts the addon may rely on (parentKey names of the Forever XML).
local function Tex(f, key, layer, atlas, useAtlasSize)
    local t = f:CreateTexture(nil, layer)
    if atlas then
        t:SetAtlas(atlas, useAtlasSize)
    end
    f[key] = t
    return t
end
local function Fs(f, key, layer, font)
    local fs = f:CreateFontString(nil, layer, font)
    f[key] = fs
    return fs
end
local function Size(f, name)
    local info = W.templateInfo[name]
    if info and (info[2] > 0 or info[3] > 0) then
        f.w, f.h = info[2], info[3]
    end
end

function TEMPLATES.UIPanelButtonTemplate(f)
    Size(f, "UIPanelButtonTemplate")
    Tex(f, "Left", "BACKGROUND")
    Tex(f, "Right", "BACKGROUND")
    Tex(f, "Middle", "BACKGROUND")
    f.normalFont, f.highlightFont, f.disabledFont = GameFontNormal, GameFontHighlight, GameFontDisable
    local text = Fs(f, "Text", "ARTWORK", "GameFontNormal")
    text:SetPoint("CENTER", f, "CENTER", 0, 0)
    f.fontString = text
    f.scripts.OnEnter = function(self)
        if self.tooltipText then
            GameTooltip:SetOwner(self, self.tooltipAnchor or "ANCHOR_RIGHT")
            GameTooltip_SetTitle(GameTooltip, self.tooltipText)
            GameTooltip:Show()
        end
    end
    f.scripts.OnLeave = function()
        GameTooltip:Hide()
    end
end

function TEMPLATES.UIPanelCloseButton(f)
    Size(f, "UIPanelCloseButton")
    f:SetNormalAtlas("RedButton-Exit")
    f:SetPushedAtlas("RedButton-exit-pressed")
    f:SetHighlightAtlas("RedButton-Highlight")
    f:SetDisabledAtlas("RedButton-Exit-Disabled")
    f:SetFrameLevel(510)
    f.scripts.OnClick = UIPanelCloseButton_OnClick
end

local function InputBox(f)
    Tex(f, "Left", "BACKGROUND", "common-search-border-left")
    Tex(f, "Right", "BACKGROUND", "common-search-border-right")
    Tex(f, "Middle", "BACKGROUND", "common-search-border-middle")
    f:SetFontObject(ChatFontNormal)
    f.scripts.OnTabPressed = EditBox_OnTabPressed
    f.scripts.OnEscapePressed = EditBox_ClearFocus
    f.scripts.OnEditFocusLost = EditBox_ClearHighlight
    f.scripts.OnEditFocusGained = EditBox_HighlightText
end
TEMPLATES.InputBoxTemplate = InputBox

function TEMPLATES.InputBoxInstructionsTemplate(f)
    InputBox(f)
    f:SetFontObject(GameFontHighlightSmall)
    local ins = Fs(f, "Instructions", "ARTWORK", "GameFontDisableSmall")
    ins:SetPoint("TOPLEFT", f, "TOPLEFT", 16, 0)
    ins:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", -20, 0)
    ins:SetTextColor(0.35, 0.35, 0.35)
    ins:SetJustifyH("LEFT")
    f.scripts.OnTextChanged = InputBoxInstructions_OnTextChanged
    f.scripts.OnDisable = InputBoxInstructions_OnDisable
    f.scripts.OnEnable = InputBoxInstructions_OnEnable
end

local TAB_PIECES = {
    { "LeftActive", "BACKGROUND", "uiframe-activetab-left" }, { "RightActive", "BACKGROUND", "uiframe-activetab-right" },
    { "MiddleActive", "BACKGROUND", "_uiframe-activetab-center" }, { "Left", "BACKGROUND", "uiframe-tab-left" },
    { "Right", "BACKGROUND", "uiframe-tab-right" }, { "Middle", "BACKGROUND", "_uiframe-tab-center" },
    { "LeftHighlight", "HIGHLIGHT", "uiframe-tab-left" }, { "MiddleHighlight", "HIGHLIGHT", "_uiframe-tab-center" },
    { "RightHighlight", "HIGHLIGHT", "uiframe-tab-right" },
}
local function TabArt(f, list)
    f[list] = {}
    for _, p in ipairs(TAB_PIECES) do
        f[list][#f[list] + 1] = Tex(f, p[1], p[2], p[3], true)
    end
    f.Left:SetPoint("TOPLEFT", f, "TOPLEFT", -3, 0)
    f.Right:SetPoint("TOPRIGHT", f, "TOPRIGHT", 7, 0)
    f.Middle:SetPoint("TOPLEFT", f.Left, "TOPRIGHT")
    f.Middle:SetPoint("TOPRIGHT", f.Right, "TOPLEFT")
    f.LeftActive:SetPoint("TOPLEFT", f, "TOPLEFT", -1, 0)
    f.RightActive:SetPoint("TOPRIGHT", f, "TOPRIGHT", 8, 0)
    f.MiddleActive:SetPoint("TOPLEFT", f.LeftActive, "TOPRIGHT")
    f.MiddleActive:SetPoint("TOPRIGHT", f.RightActive, "TOPLEFT")
end
local function PanelTab(f)
    Size(f, "PanelTabButtonTemplate")
    TabArt(f, "TabTextures")
    f.motionWhileDisabled = true
    f.normalFont, f.highlightFont, f.disabledFont = GameFontNormalSmall, GameFontHighlightSmall, GameFontHighlightSmall
    local text = Fs(f, "Text", "ARTWORK", "GameFontNormalSmall")
    text:SetPoint("CENTER", f, "CENTER", 0, 2)
    f.fontString = text
    -- parentArray="Tabs": the tab joins its parent's Tabs in creation order
    local p = f.parent
    p.Tabs = p.Tabs or {}
    p.Tabs[#p.Tabs + 1] = f
    f:SetFrameLevel(f:GetFrameLevel() + 4)
    f:RegisterEvent("DISPLAY_SIZE_CHANGED")
    local function Resize(self)
        local parent = self:GetParent()
        PanelTemplates_TabResize(self, parent.tabPadding, nil, parent.minTabWidth, parent.maxTabWidth)
    end
    f.scripts.OnShow = Resize
    f.scripts.OnEvent = function(self)
        if self:IsVisible() then
            Resize(self)
        end
    end
    f.scripts.OnEnter = function(self)
        GameTooltip:Hide()
        if self.Text:IsTruncated() then
            GameTooltip:SetOwner(self, "ANCHOR_RIGHT")
            GameTooltip:SetText(self.Text:GetText())
            GameTooltip:Show()
        end
    end
    f.scripts.OnLeave = function()
        GameTooltip:Hide()
    end
end
TEMPLATES.PanelTabButtonTemplate = PanelTab
function TEMPLATES.PanelTopTabButtonTemplate(f)
    PanelTab(f)
    for _, t in ipairs(f.TabTextures) do
        t:SetTexCoord(0, 1, 1, 0.25)
        t:SetHeight(t.h * 0.75)
    end
    f.Left:ClearAllPoints()
    f.Left:SetPoint("BOTTOMLEFT", f, "BOTTOMLEFT", -3, 0)
    f.Right:ClearAllPoints()
    f.Right:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", 7, 0)
    f.LeftActive:ClearAllPoints()
    f.LeftActive:SetPoint("BOTTOMLEFT", f, "BOTTOMLEFT", -1, 0)
    f.RightActive:ClearAllPoints()
    f.RightActive:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", 8, 0)
    f.isTopTab = true
end

-- TabSystemTopButtonTemplate: TabSystemButtonMixin's public methods. GetTabSystem() is the
-- parent (a TabSystem frame); its GetTabWidthConstraints and SetTab are used when present.
function TEMPLATES.TabSystemTopButtonTemplate(f)
    Size(f, "TabSystemTopButtonTemplate")
    TabArt(f, "RotatedTextures")
    Tex(f, "Icon", "ARTWORK")
    Tex(f, "IconMask", "ARTWORK")
    f.Icon:Hide()
    f.IconMask:Hide()
    local text = Fs(f, "Text", "ARTWORK", "GameFontNormalSmall")
    text:SetPoint("CENTER", f, "CENTER", 0, 0)
    f.fontString = text
    f.motionWhileDisabled = true
    f.isTabOnTop = true
    function f:GetTabSystem()
        return self.tabSystem or self:GetParent()
    end
    function f:Init(tabID, tabText, tabIcon)
        self.tabID, self.tabText, self.tabIcon = tabID, tabText, tabIcon
        if tabText then
            self:SetText(tabText)
        end
        self:UpdateTabWidth()
        self:SetTabSelected(false)
    end
    function f:GetTabID()
        return self.tabID
    end
    function f:GetTabText()
        return self.tabText
    end
    function f:IsSelected()
        return not not self.isSelected
    end
    function f:IsForceDisabled()
        return self.forceDisabled or false, self.errorReason
    end
    function f:SetTabSelected(selected)
        self.isSelected = selected
        self.Left:SetShown(not selected)
        self.Middle:SetShown(not selected)
        self.Right:SetShown(not selected)
        self.LeftActive:SetShown(selected)
        self.MiddleActive:SetShown(selected)
        self.RightActive:SetShown(selected)
        self:SetNormalFontObject(selected and GameFontHighlightSmall or GameFontNormalSmall)
        self:SetEnabled(not selected and not self.forceDisabled)
        if GameTooltip:IsOwned(self) then
            GameTooltip:Hide()
        end
    end
    function f:SetTabEnabled(enabled, errorReason)
        self.forceDisabled = not enabled
        self:SetEnabled(enabled and not self.isSelected)
        self.errorReason = errorReason
    end
    function f:SetTooltipText(t)
        self.tooltipText = t
    end
    function f:SetTabWidth(w)
        self:SetWidth(w)
    end
    function f:UpdateTabWidth()
        local width = self.Left:GetWidth() + self.Right:GetWidth() + 20
        local sys = self:GetTabSystem()
        local minW, maxW
        if sys and sys.GetTabWidthConstraints then
            minW, maxW = sys:GetTabWidthConstraints()
        end
        local textWidth = self.Text:GetUnboundedStringWidth() + (self.textPadding or 0)
        if width < textWidth then
            width = textWidth + 10
        end
        if maxW and width > maxW then
            width, textWidth = maxW, maxW - 10
        end
        if minW and width < minW then
            width, textWidth = minW, minW - 10
        end
        self.Text:SetWidth(textWidth)
        self:SetTabWidth(width)
    end
    f.scripts.OnClick = function(self)
        local sys = self:GetTabSystem()
        if sys and sys.SetTab then
            sys:SetTab(self:GetTabID(), true)
        end
    end
    f.scripts.OnLeave = function()
        GameTooltip:Hide()
    end
end

function TEMPLATES.PanelResizeButtonTemplate(f)
    Size(f, "PanelResizeButtonTemplate")
    f:SetNormalTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Up")
    f:SetHighlightTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Highlight")
    f:SetPushedTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Down")
    function f:Init(target, minWidth, minHeight, maxWidth, maxHeight)
        self.target, self.minWidth, self.minHeight, self.maxWidth, self.maxHeight = target, minWidth, minHeight, maxWidth,
            maxHeight
        local original = target:GetScript("OnSizeChanged") or function() end
        target:SetScript("OnSizeChanged", function(t, w, h)
            original(t, w, h)
            if not self.isActive then
                return
            end
            local nw, nh = w, h
            if w < self.minWidth then
                nw = self.minWidth
                t:SetWidth(nw)
            elseif self.maxWidth and w > self.maxWidth then
                nw = self.maxWidth
                t:SetWidth(nw)
            end
            if h < self.minHeight then
                nh = self.minHeight
                t:SetHeight(nh)
            elseif self.maxHeight and h > self.maxHeight then
                nh = self.maxHeight
                t:SetHeight(nh)
            end
            if self.resizeCallback then
                self.resizeCallback(self, nw, nh, self.isActive)
            end
        end)
    end
    function f:IsActive()
        return not not self.isActive
    end
    function f:SetOnResizeStoppedCallback(cb)
        self.resizeStoppedCallback = cb
    end
    function f:SetOnResizeCallback(cb)
        self.resizeCallback = cb
    end
    f.scripts.OnMouseDown = function(self)
        if not self:IsEnabled() then
            return
        end
        self.isActive = true
        if self.target and (not self.target.onResizeStartCallback or self.target.onResizeStartCallback(self)) then
            self.target:StartSizing("BOTTOMRIGHT", true)
        end
    end
    f.scripts.OnMouseUp = function(self)
        self.isActive = false
        if not self.target then
            return
        end
        if not self.target.onResizeStopCallback or self.target.onResizeStopCallback(self) then
            self.target:StopMovingOrSizing()
        end
        if self.resizeStoppedCallback then
            self.resizeStoppedCallback(self.target)
        end
    end
end

function TEMPLATES.MinimalScrollBar(f)
    Size(f, "MinimalScrollBar")
    f.scrollPercentage, f.visibleExtent, f.panExtent = 0, 1, 0.1
    function f:SetScrollPercentage(p)
        self.scrollPercentage = math.max(0, math.min(1, p))
        local sf = self.scrollFrame
        if sf then
            sf:SetVerticalScroll(self.scrollPercentage * sf:GetVerticalScrollRange())
        end
    end
    function f:GetScrollPercentage()
        return self.scrollPercentage
    end
    function f:SetVisibleExtentPercentage(p)
        self.visibleExtent = p
    end
    function f:GetVisibleExtentPercentage()
        return self.visibleExtent
    end
    function f:SetPanExtentPercentage(p)
        self.panExtent = p
    end
    function f:HasScrollableExtent()
        return self.visibleExtent < 1
    end
    function f:SetHideIfUnscrollable(on)
        self.hideIfUnscrollable = on
    end
end

function TEMPLATES.UICheckButtonTemplate(f)
    Size(f, "UICheckButtonTemplate")
    local text = Fs(f, "Text", "ARTWORK", "GameFontNormalSmall")
    text:SetPoint("LEFT", f, "RIGHT", -2, 0)
    f.fontString = text
    f:SetNormalTexture("Interface\\Buttons\\UI-CheckBox-Up")
    f:SetPushedTexture("Interface\\Buttons\\UI-CheckBox-Down")
    f:SetHighlightTexture("Interface\\Buttons\\UI-CheckBox-Highlight")
    f.checkedTexture = f:CreateTexture(nil, "ARTWORK")
end

function TEMPLATES.PortraitFrameTemplate(f)
    Size(f, "PortraitFrameTemplate")
    local nine = CreateFrame("Frame", nil, f)
    nine:SetAllPoints(f)
    f.NineSlice = nine
    f.layoutType = "PortraitFrameTemplate"
    NineSliceUtil.ApplyLayout(nine, NineSliceUtil.GetLayout("PortraitFrameTemplate") or {})
    local bg = Tex(f, "Bg", "BACKGROUND")
    bg:SetTexture("Interface\\FrameGeneral\\UI-Background-Rock")
    bg:SetPoint("TOPLEFT", f, "TOPLEFT", 2, -21)
    bg:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", -2, 2)
    local streaks = Tex(f, "TopTileStreaks", "BORDER")
    streaks:SetPoint("TOPLEFT", f, "TOPLEFT", 6, -21)
    streaks:SetPoint("TOPRIGHT", f, "TOPRIGHT", -2, -21)
    local pc = CreateFrame("Frame", nil, f)
    pc:SetSize(1, 1)
    pc:SetPoint("TOPLEFT", f, "TOPLEFT")
    pc:SetFrameLevel(400)
    f.PortraitContainer = pc
    local portrait = Tex(pc, "portrait", "OVERLAY")
    portrait:SetSize(62, 62)
    portrait:SetPoint("TOPLEFT", f, "TOPLEFT", -5, 7)
    local tc = CreateFrame("Frame", nil, f)
    tc:SetHeight(20)
    tc:SetPoint("TOPLEFT", f, "TOPLEFT", 58, -1)
    tc:SetPoint("TOPRIGHT", f, "TOPRIGHT", -24, -1)
    tc:SetFrameLevel(510)
    f.TitleContainer = tc
    local title = Fs(tc, "TitleText", "OVERLAY", "GameFontNormal")
    title:SetPoint("TOP", tc, "TOP", 0, -5)
    title:SetPoint("LEFT", tc, "LEFT")
    title:SetPoint("RIGHT", tc, "RIGHT")
    -- Camelot (Forever) close button: 28 x 28 at TOPRIGHT (0, 3)
    local close = CreateFrame("Button", nil, f, "UIPanelCloseButton")
    close:SetSize(28, 28)
    close:SetPoint("TOPRIGHT", f, "TOPRIGHT", 0, 3)
    f.CloseButton = close
    function f:GetTitleText()
        return self.TitleContainer.TitleText
    end
    function f:SetTitle(t)
        self:GetTitleText():SetText(t)
    end
    function f:SetTitleFormatted(fmt, ...)
        self:GetTitleText():SetFormattedText(fmt, ...)
    end
    function f:SetTitleColor(color)
        self:GetTitleText():SetTextColor(color:GetRGBA())
    end
    function f:SetTitleOffsets(l, r)
        self.TitleContainer:SetPoint("TOPLEFT", self, "TOPLEFT", l or 58, -1)
        self.TitleContainer:SetPoint("TOPRIGHT", self, "TOPRIGHT", r or -24, -1)
    end
    function f:GetPortrait()
        return self.PortraitContainer.portrait
    end
    function f:SetPortraitToAsset(texture)
        self:GetPortrait():SetTexture(texture)
    end
    function f:SetPortraitTextureRaw(texture)
        self:GetPortrait():SetTexture(texture)
    end
    function f:SetPortraitAtlasRaw(atlas, ...)
        self:GetPortrait():SetAtlas(atlas, ...)
    end
    function f:SetPortraitTexCoord(...)
        self:GetPortrait():SetTexCoord(...)
    end
    -- as on the client, an unknown class file is an error (unpack of nil)
    function f:SetPortraitToClassIcon(classFilename)
        self:SetPortraitTextureRaw("Interface/TargetingFrame/UI-Classes-Circles")
        self:SetPortraitTexCoord(unpack(CLASS_ICON_TCOORDS[string.upper(classFilename)]))
    end
    function f:SetPortraitShown(shown)
        self:GetPortrait():SetShown(shown)
    end
    function f:SetBorder(layoutName)
        NineSliceUtil.ApplyLayout(self.NineSlice, NineSliceUtil.GetLayout(layoutName))
    end
end

function TEMPLATES.InsetFrameTemplate(f)
    local bg = Tex(f, "Bg", "BACKGROUND")
    bg:SetTexture("Interface\\FrameGeneral\\UI-Background-Marble")
    bg:SetAllPoints(f)
    local nine = CreateFrame("Frame", nil, f)
    nine:SetAllPoints(f)
    f.NineSlice = nine
    NineSliceUtil.ApplyLayout(nine, NineSliceUtil.GetLayout("InsetFrameTemplate") or {})
end

local function Backdrop(f)
    function f:SetBackdrop(info)
        self.backdropInfo = info
    end
    function f:ApplyBackdrop()
    end
    function f:SetBackdropColor(r, g, b, a)
        self.backdropColor = { r, g, b, a or 1 }
    end
    function f:GetBackdropColor()
        return unpack(self.backdropColor or { 1, 1, 1, 1 })
    end
    function f:SetBackdropBorderColor(r, g, b, a)
        self.backdropBorderColor = { r, g, b, a or 1 }
    end
    function f:GetBackdropBorderColor()
        return unpack(self.backdropBorderColor or { 1, 1, 1, 1 })
    end
end
TEMPLATES.BackdropTemplate = Backdrop
function TEMPLATES.TooltipBackdropTemplate(f)
    Backdrop(f)
    f:SetBackdropColor(0.09, 0.09, 0.19)
end

-- DialogBorderDarkTemplate: a nine-slice border with the Dialog layout and a dark Bg (not read in
-- the Forever sources at hand; C_XMLUtil.GetTemplateInfo on the client confirms it, G11).
function TEMPLATES.DialogBorderDarkTemplate(f)
    NineSliceUtil.ApplyLayout(f, NineSliceUtil.GetLayout("Dialog") or {})
    local bg = Tex(f, "Bg", "BACKGROUND")
    bg:SetColorTexture(0, 0, 0, 0.8)
    bg:SetAllPoints(f)
end

for name, info in pairs(W.templateInfo) do
    if not M.KNOWN_TEMPLATES[name] then
        M.KNOWN_TEMPLATES[name] = info[1]
    end
end
-- MinimalScrollBar is an EventFrame template; created as a Frame it works the same
M.KNOWN_TEMPLATES.MinimalScrollBar = { EventFrame = true, Frame = true }

---------------------------------------------------------------------------- input helpers
local function FocusedBox(box, what)
    if focused ~= box then
        return false
    end
    assert(box.frameType == "EditBox", what .. " needs an edit box")
    return true
end

function M.Paste(box, text)
    if not FocusedBox(box, "Mock.Paste") or box.pasteDisabled then
        return false
    end
    WithInput({ ctrl = true }, true, function()
        local changed = InsertAtCursor(box, text)
        if changed then
            Script(box, "OnTextChanged", true)
        end
    end)
    return true
end

function M.Type(box, text, opts)
    if not FocusedBox(box, "Mock.Type") then
        return false
    end
    opts = opts or {}
    WithInput({ ctrl = false }, true, function()
        local chars = Utf8Chars(text)
        if opts.burst then
            if InsertAtCursor(box, text) then
                Script(box, "OnTextChanged", true)
            end
            for _, ch in ipairs(chars) do
                Script(box, "OnChar", ch)
            end
        else
            for _, ch in ipairs(chars) do
                if InsertAtCursor(box, ch) then
                    Script(box, "OnTextChanged", true)
                end
                Script(box, "OnChar", ch)
            end
        end
    end)
    return true
end

local ARROWS = { UP = true, DOWN = true, LEFT = true, RIGHT = true }
function M.Key(frame, key, mods)
    WithInput(mods, true, function()
        Script(frame, "OnKeyDown", key)
        if frame.frameType == "EditBox" and focused == frame then
            if key == "ENTER" then
                Script(frame, "OnEnterPressed")
            elseif key == "ESCAPE" then
                Script(frame, "OnEscapePressed")
            elseif key == "TAB" then
                Script(frame, "OnTabPressed")
            elseif ARROWS[key] then
                if key == "LEFT" or key == "RIGHT" then
                    local text, pos = frame.text or "", frame.cursor or 0
                    if key == "LEFT" then
                        repeat
                            pos = pos - 1
                        until pos <= 0 or not (text:byte(pos + 1) >= 128 and text:byte(pos + 1) < 192)
                    else
                        repeat
                            pos = pos + 1
                        until pos >= #text or not (text:byte(pos + 1) >= 128 and text:byte(pos + 1) < 192)
                    end
                    frame:SetCursorPosition(pos)
                end
                Script(frame, "OnArrowPressed", key)
            elseif key == "BACKSPACE" then
                local text = frame.text or ""
                if frame.highlight then
                    InsertAtCursor(frame, "")
                elseif (frame.cursor or 0) > 0 then
                    local b = frame.cursor
                    local a = b - 1
                    while a > 0 and text:byte(a + 1) >= 128 and text:byte(a + 1) < 192 do
                        a = a - 1
                    end
                    frame.text, frame.cursor = text:sub(1, a) .. text:sub(b + 1), a
                end
                if frame.text ~= text then
                    Script(frame, "OnTextChanged", true)
                end
            end
        end
        Script(frame, "OnKeyUp", key)
    end)
end

function M.MouseDown(frame, button)
    WithInput(nil, true, Script, frame, "OnMouseDown", button or "LeftButton")
end

function M.MouseUp(frame, button)
    WithInput(nil, true, Script, frame, "OnMouseUp", button or "LeftButton", true)
end

function M.ClickButton(b, mouseButton, mods)
    mouseButton = mouseButton or "LeftButton"
    assert(b and b.kind == "frame", "Mock.ClickButton needs a frame")
    if not b:IsVisible() then
        return false
    end
    local clicked = false
    WithInput(mods, true, function()
        Script(b, "OnMouseDown", mouseButton)
        Script(b, "OnMouseUp", mouseButton, true)
        local clicks = b.clicks or { LeftButtonUp = true }
        local registered = clicks.AnyUp or clicks[mouseButton .. "Up"] or clicks.AnyDown or clicks[mouseButton .. "Down"]
        if (b.frameType == "Button" or b.frameType == "CheckButton") and not b.disabled and registered then
            if b.frameType == "CheckButton" then
                b.checked = not b.checked
            end
            Script(b, "PreClick", mouseButton, false)
            Script(b, "OnClick", mouseButton, false)
            Script(b, "PostClick", mouseButton, false)
            clicked = true
        end
    end)
    return clicked
end

local function MotionAllowed(r)
    if not r.scripts or not r:IsVisible() then
        return false
    end
    if r.disabled and not r.motionWhileDisabled then
        return false
    end
    return r.mouse or r.mouseMotion
end
function M.Hover(region)
    if W.hovered and W.hovered ~= region then
        M.Leave()
    end
    W.hovered = region
    if region and MotionAllowed(region) then
        Script(region, "OnEnter", true)
    end
end
function M.Leave()
    local r = W.hovered
    W.hovered = nil
    if r and MotionAllowed(r) then
        Script(r, "OnLeave", true)
    end
end
function M.GlobalMouseDown(button)
    WithInput(nil, true, function()
        Fire("GLOBAL_MOUSE_DOWN", button or "LeftButton")
        Fire("GLOBAL_MOUSE_UP", button or "LeftButton")
    end)
end
function M.Wheel(frame, delta)
    WithInput(nil, true, Script, frame, "OnMouseWheel", delta)
end
function M.Drag(frame, dx, dy)
    WithInput(nil, true, function()
        if frame.dragButtons and frame.dragButtons.LeftButton then
            Script(frame, "OnDragStart", "LeftButton")
        end
        if frame.moving then
            for _, p in ipairs(frame.points) do
                p.x, p.y = p.x + dx, p.y + dy
            end
        end
        Script(frame, "OnDragStop")
    end)
end
function M.Resize(frame, w, h)
    local b = frame.resizeBounds
    if b then
        w = math.max(b[1] or 0, (b[3] and b[3] > 0) and math.min(w, b[3]) or w)
        h = math.max(b[2] or 0, (b[4] and b[4] > 0) and math.min(h, b[4]) or h)
    end
    if frame.sizing then
        -- the client keeps the opposite corner in place: re-anchor by the top-left
        local l, bottom, _, height = frame:GetRect()
        frame:ClearAllPoints()
        frame:SetPoint("TOPLEFT", UIParent, "BOTTOMLEFT", l, bottom + height)
    end
    WithInput(nil, true, frame.SetSize, frame, w, h)
end

---------------------------------------------------------------------------- links of a message
-- What a chat frame does with a hyperlink, item quality colours and hyperlinks in a frame's font
-- strings, modelled on the 12.x UI source (live 12.1.5 and forever 1.60.1; docs\ADDON.md, "Links of
-- a message"). Colour values are the client's defaults (mock values: nothing should depend on them
-- but the tests that set them).
--   SetItemRef(link, text, button, frame): recorded (Mock.LinkCalls().refs, with the modifiers
--     held), then as ItemRef.lua: an "addon" link goes to EventRegistry "SetItemRef"; "worldmap"
--     opens the map (result "map"); an item Shift+Left click inserts the link first (the Item
--     handler); then a modified click runs HandleModifiedItemClick (CHATLINK: InsertLink, else
--     DRESSUP: DressUpLink; result "insert", "dressup" or "none"), else the ItemRefTooltip
--     ("itemref").
--   IsModifiedClick(action): CHATLINK = Shift, DRESSUP = Ctrl, none = any modifier.
--   ChatFrameUtil.InsertLink(text): recorded (Mock.LinkCalls().inserts); true only while a chat
--     edit box is active (Mock.SetChatEditActive(on)).
--   DressUpLink(link): recorded (Mock.LinkCalls().dressUps), true.
--   GameTooltip:SetHyperlink(link): recorded (Mock.LinkCalls().tooltips); a tooltip for the types
--     in Mock.TOOLTIP_TYPES (shown, true), else hidden and false, as the 12.x tooltip data accessor.
--   ITEM_QUALITY_COLORS, ColorManager.GetColorDataForItemQuality, C_ColorOverrides.GetColorForQuality,
--     C_Item.GetItemQualityColor: the default quality colours; Mock.SetQualityColor(q, r, g, b) is a
--     user override (ColorManager and C_ColorOverrides follow it, as on the client).
--   Frame:SetHyperlinksEnabled(on) (protected: during combat it does nothing and fires
--     ADDON_ACTION_BLOCKED, the conservative reading), Frame:GetHyperlinksEnabled().
--   Mock.ClickHyperlink(frame, link, button, mods) / Mock.HoverHyperlink(frame, link) /
--     Mock.LeaveHyperlink(frame): a click or the cursor on a hyperlink of one of frame's font strings
--     (OnHyperlinkClick(frame, link, text, button), OnHyperlinkEnter(frame, link, text, fontString),
--     OnHyperlinkLeave(frame)) when the frame is visible, mouse enabled and its hyperlinks enabled,
--     and the link is in one of its shown font strings; text is the whole link with the colour
--     escape right before it and the |r right after it (assumption: as a chat frame passes it).
--     Returns true when the script ran.
--   Mock.LinkCalls() -> { refs, inserts, dressUps, tooltips }; Mock.ClearLinkCalls()
W.linkCalls = { refs = {}, inserts = {}, dressUps = {}, tooltips = {} }
W.chatEditActive = false
M.TOOLTIP_TYPES = { item = true, spell = true, enchant = true, quest = true, achievement = true, currency = true,
    talent = true, mount = true, keystone = true }
W.qualityDefault = { [0] = { 0.616, 0.616, 0.616 }, { 1, 1, 1 }, { 0.118, 1, 0 }, { 0, 0.439, 0.867 },
    { 0.639, 0.208, 0.933 }, { 1, 0.502, 0 }, { 0.902, 0.8, 0.502 }, { 0, 0.8, 1 }, { 0, 0.8, 1 } }
W.qualityOverride = {}
local function QualityColor(q, default)
    local c = (not default and W.qualityOverride[q]) or W.qualityDefault[q]
    if not c then
        return nil
    end
    return CreateColor(c[1], c[2], c[3])
end
local function UpdateQualityColors()
    ITEM_QUALITY_COLORS = {}
    for q = 0, 8 do
        local color = QualityColor(q)
        ITEM_QUALITY_COLORS[q] = { r = color.r, g = color.g, b = color.b, hex = color:GenerateHexColorMarkup(), color = color }
    end
end
UpdateQualityColors()
C_ColorOverrides = {
    GetColorForQuality = function(q)
        return QualityColor(q)
    end,
    GetDefaultColorForQuality = function(q)
        return QualityColor(q, true)
    end,
}
ColorManager = {
    GetColorDataForItemQuality = function(q)
        return ITEM_QUALITY_COLORS[q]
    end,
}
C_Item = C_Item or {}
C_Item.GetItemQualityColor = function(q)
    local c = QualityColor(q)
    if c then
        return c.r, c.g, c.b, c:GenerateHexColor()
    end
end
function M.SetQualityColor(q, r, g, b)
    W.qualityOverride[q] = r and { r, g, b } or nil
    UpdateQualityColors()
end

IsModifiedClick = function(action)
    if action == "CHATLINK" then
        return state.shift
    elseif action == "DRESSUP" then
        return state.ctrl
    elseif action == nil then
        return state.shift or state.ctrl or state.alt
    end
    return false
end
ChatFrameUtil.InsertLink = function(text)
    W.linkCalls.inserts[#W.linkCalls.inserts + 1] = { text = text, inserted = W.chatEditActive }
    return W.chatEditActive
end
DressUpLink = function(link)
    W.linkCalls.dressUps[#W.linkCalls.dressUps + 1] = link
    return true
end
SetItemRef = function(link, text, button, frame)
    local rec = { link = link, text = text, button = button, frame = frame, shift = state.shift, ctrl = state.ctrl }
    W.linkCalls.refs[#W.linkCalls.refs + 1] = rec
    local kind = tostring(link):match("^([^:]*)")
    if kind == "addon" then
        rec.result = "addon"
        EventRegistry:TriggerEvent("SetItemRef", link, text, button, frame)
    elseif kind == "worldmap" then
        rec.result = "map"
    elseif (kind == "player" or kind == "playerCommunity" or kind == "playerGM") and IsModifiedClick("CHATLINK") then
        -- HandlePlayerLink (ItemRefHandlers.lua): the plain name into the edit box (else popups, /who)
        rec.result = ChatFrameUtil.InsertLink(tostring(link):match("^[^:]*:([^:]*)") or "") and "name" or "who"
    elseif (kind == "BNplayer" or kind == "BNplayerCommunity") and IsModifiedClick("CHATLINK") then
        rec.result = "none"     -- disabled by the client: no encoded account id in a message
    elseif kind == "item" and IsModifiedClick("CHATLINK") and button == "LeftButton" and ChatFrameUtil.InsertLink(text) then
        rec.result = "insert"
    elseif IsModifiedClick() then
        if IsModifiedClick("CHATLINK") and ChatFrameUtil.InsertLink(text) then
            rec.result = "insert"
        elseif IsModifiedClick("DRESSUP") and DressUpLink(text) then
            rec.result = "dressup"
        else
            rec.result = "none"
        end
    else
        rec.result = "itemref"
    end
end
function GameTooltip:SetHyperlink(link)
    local kind = tostring(link):match("^([^:]*)")
    W.linkCalls.tooltips[#W.linkCalls.tooltips + 1] = link
    if M.TOOLTIP_TYPES[kind] then
        self.lines, self.colors = { "hyperlink " .. tostring(link) }, {}
        self:Show()
        return true
    end
    self:Hide()
    return false
end
function M.SetChatEditActive(on)
    W.chatEditActive = on and true or false
end
function M.LinkCalls()
    return W.linkCalls
end
function M.ClearLinkCalls()
    W.linkCalls = { refs = {}, inserts = {}, dressUps = {}, tooltips = {} }
end

function Methods:SetHyperlinksEnabled(on)
    if state.combat then
        Fire("ADDON_ACTION_BLOCKED", "TranslateProbe", "SetHyperlinksEnabled()")
        return
    end
    self.hyperlinks = on and true or false
end
function Methods:GetHyperlinksEnabled()
    return self.hyperlinks == true
end

-- The whole link (with the colour escape right before it and the |r right after it) and the font
-- string that shows it, or nil.
local function FindHyperlink(frame, link)
    for _, r in ipairs(frame.regions or {}) do
        if r.kind == "fontstring" and r.shown and type(r.text) == "string" then
            local a, b = r.text:find("|H" .. link .. "|h", 1, true)
            if a then
                local c = r.text:find("|h", b + 1, true)
                if c then
                    local s, e = a, c + 1
                    local before = r.text:sub(1, a - 1)
                    local esc = before:match("(|c%x%x%x%x%x%x%x%x)$") or before:match("(|cn[^:|]*:)$")
                    if esc then
                        s = a - #esc
                        if r.text:sub(e + 1, e + 2) == "|r" then
                            e = e + 2
                        end
                    end
                    return r.text:sub(s, e), r
                end
            end
        end
    end
end
local function HyperlinkTarget(frame, link)
    if not (frame and frame:IsVisible() and frame.hyperlinks and frame.mouse) then
        return nil
    end
    return FindHyperlink(frame, link)
end
function M.ClickHyperlink(frame, link, button, mods)
    local text = HyperlinkTarget(frame, link)
    if not text then
        return false
    end
    WithInput(mods, true, Script, frame, "OnHyperlinkClick", link, text, button or "LeftButton")
    return true
end
function M.HoverHyperlink(frame, link)
    local text, fs = HyperlinkTarget(frame, link)
    if not text then
        return false
    end
    WithInput(nil, true, Script, frame, "OnHyperlinkEnter", link, text, fs)
    return true
end
function M.LeaveHyperlink(frame)
    WithInput(nil, true, Script, frame, "OnHyperlinkLeave")
end

function M.Load(files, getSource)
    local ns = {}
    for _, file in ipairs(files) do
        local chunk, err = loadChunk(getSource(file), "@" .. file)
        if not chunk then
            error(err, 0)
        end
        chunk("TranslateProbe", ns)
    end
    state.loaded = true
    Fire("ADDON_LOADED", "TranslateProbe")
    Fire("PLAYER_LOGIN")
    Fire("PLAYER_ENTERING_WORLD", true, false)
    M.ns = ns
    return ns
end

return M
