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
    frame = 0, time = 0, lockdown = false, shift = false, loaded = false,
    physW = 1920, physH = 1080, uiScale = 1, cvars = {},
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
UnitGUID = function()
    return "Player-1-0001"
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
    local p = r.points[1]
    if not p then
        return 0, 0, w, h
    end
    local rl, rt, rw, rh = Rect(p.rel, depth)
    local rp, sp = POINTS[p.relPoint] or POINTS.TOPLEFT, POINTS[p.point] or POINTS.TOPLEFT
    local ax, ay = rl + rp[1] * rw + p.x * ppu, rt + rp[2] * rh - p.y * ppu
    return ax - sp[1] * w, ay - sp[2] * h, w, h
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
    self.allPoints = nil
    self.points = { { point = point, rel = rel or UIParent, relPoint = relPoint or point, x = x or 0, y = y or 0 } }
end
function Methods:ClearAllPoints()
    self.points, self.allPoints = {}, nil
end
function Methods:SetAllPoints(rel)
    self.allPoints = rel or self.parent
end
function Methods:SetSize(w, h)
    self.w, self.h = w, h
end
function Methods:SetWidth(w)
    self.w = w
end
function Methods:SetHeight(h)
    self.h = h
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
function Methods:Hide()
    if not self.shown then
        return
    end
    local was = IsVisible(self)
    self.shown = false
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

-- Font strings: width = 0.5 x font height per UTF-8 character; inline textures count their
-- own width; codes (|c, |r, |H, |h) count nothing.
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
    s = s:gsub("|T.-|t", ""):gsub("|c%x%x%x%x%x%x%x%x", ""):gsub("|r", ""):gsub("|H.-|h", ""):gsub("|h", "")
    return 0.5 * size * Utf8Len(s) + tex
end
local function Segments(text)
    local out = {}
    for seg in (text .. "|n"):gmatch("(.-)|n") do
        out[#out + 1] = seg
    end
    return out
end
function Methods:SetFont(path, size, flags)
    self.fontPath, self.fontSize, self.fontFlags = path, size, flags
    return true
end
function Methods:GetFont()
    return self.fontPath, self.fontSize, self.fontFlags
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
        best = math.max(best, SegmentWidth(seg, self.fontSize or 0))
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
        local w = SegmentWidth(seg, self.fontSize or 0)
        if (self.w or 0) > 0 then
            lines = lines + math.max(1, math.ceil(w / self.w))
        else
            lines = lines + 1
        end
    end
    return lines
end
function Methods:GetStringHeight()
    return self:GetNumLines() * (self.fontSize or 0)
end

local MethodsMT = { __index = Methods }

local function NewRegion(kind, parent)
    return setmetatable({ kind = kind, parent = parent, shown = true, points = {}, w = 0, h = 0 }, MethodsMT)
end

local function NewFrame(frameType, name, parent)
    local f = NewRegion("frame", parent)
    f.frameType, f.name = frameType or "Frame", name
    f.scripts, f.events, f.scale = {}, {}, 1
    function f:CreateTexture(_, layer)
        local t = NewRegion("texture", self)
        t.layer = layer
        t.r, t.g, t.b, t.a = 1, 1, 1, 1
        textures[#textures + 1] = t
        return t
    end
    function f:CreateFontString(_, layer)
        local fs = NewRegion("fontstring", self)
        fs.layer, fs.text = layer, ""
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

CreateFrame = function(frameType, name, parent)
    return NewFrame(frameType, name, parent or UIParent)
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
GameTooltip.shown, GameTooltip.lines = false, {}
function GameTooltip:SetOwner(owner)
    self.owner, self.lines = owner, {}
end
function GameTooltip:SetText(text)
    self.lines = { text }
end
function GameTooltip:AddLine(text)
    self.lines[#self.lines + 1] = text
end
function GameTooltip:ClearLines()
    self.lines = {}
end
function GameTooltip:NumLines()
    return #self.lines
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

-- Delivers one chat event the way ChatFrameMixin:MessageEventHandler does, then to addon
-- frames. hidden = true runs the filters but no frame shows the line (a channel no tab shows).
function M.Deliver(event, text, sender, guid, hidden)
    nextLineID = nextLineID + 1
    local lineID = nextLineID
    sender = sender or "Someone"
    guid = guid or "Player-1-0002"
    lineStore[lineID] = { text = text, sender = sender, guid = guid }
    local secret = state.lockdown
    local args = pack(secret and Secret() or text, secret and Secret() or sender, "", "", "", "", 0, 0, "", 0,
        lineID, secret and Secret() or guid, 0, false)
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
            local msg = secret and Secret() or ("[" .. sender .. "]: " .. a[1])
            cf:AddMessage(msg, 1, 1, 1, 1, 0, 0, event, args, function(t)
                return t
            end)
        end
    end
    Fire(event, unpack(args, 1, args.n))
    return lineID
end

-- A click on a hyperlink (link = its data, such as "addon:TranslateProbe:t:123:456").
function M.Click(link, chatFrame, shift)
    local was = state.shift
    if shift ~= nil then
        state.shift = shift and true or false
    end
    EventRegistry:TriggerEvent("SetItemRef", link, "[T]", "LeftButton", chatFrame or ChatFrame1)
    state.shift = was
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
