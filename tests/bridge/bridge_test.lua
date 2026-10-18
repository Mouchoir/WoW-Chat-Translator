-- Bridge.lua against the Python reference vectors (driven by tests/bridge/run.js).
-- Globals set by run.js: SRC_BRIDGE (the addon file), then TRACE / TAGS / MESSAGES (vectors).
TEST_FAILURES = 0
local counts = { traces = 0, ticks = 0, frames = 0, blocks = 0, setColor = 0, tags = 0, markup = 0, messages = 0,
    api = 0 }

local function fail(msg)
    TEST_FAILURES = TEST_FAILURES + 1
    if TEST_FAILURES <= 40 then
        print("FAIL " .. msg)
    end
end

local function check(cond, msg)
    if not cond then
        fail(msg)
    end
    return cond
end

local function hex(s)
    return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end))
end

local function fromhex(h)
    return (h:gsub("..", function(x) return string.char(tonumber(x, 16)) end))
end

local function tablehex(t, n)
    local p = {}
    for i = 1, n do
        p[i] = string.format("%02x", t[i])
    end
    return table.concat(p)
end

---------------------------------------------------------------------------- mock WoW
local NOW = 0
function GetTime() return NOW end
local SCREEN_W, SCREEN_H = 1920, 1080
function GetPhysicalScreenSize() return SCREEN_W, SCREEN_H end
local CVARS = {}
function GetCVar(name) return CVARS[name] end

local function IsVisible(o)
    while o do
        if not o.shown then
            return false
        end
        o = o.parent
    end
    return true
end

local function FireShow(o)
    if o.scripts.OnShow then
        o.scripts.OnShow(o)
    end
    for _, c in ipairs(o.children) do
        if c.shown then
            FireShow(c)
        end
    end
end

local function NewRegion(kind, parent)
    local o = { kind = kind, shown = true, parent = parent, children = {}, scripts = {}, events = {}, textures = {} }
    if parent then
        parent.children[#parent.children + 1] = o
    end
    function o:Show()
        if not self.shown then
            self.shown = true
            if IsVisible(self) then
                FireShow(self)
            end
        end
    end
    function o:Hide() self.shown = false end
    function o:IsShown() return self.shown end
    function o:SetFrameStrata(s) self.strata = s end
    function o:SetFrameLevel(l) self.level = l end
    function o:SetIgnoreParentScale(v) self.ignore = v end
    function o:SetPoint(p, rel, rp, x, y) self.point, self.rel, self.relPoint, self.x, self.y = p, rel, rp, x, y end
    function o:SetScale(s) self.scale = s end
    function o:SetSize(w, h) self.w, self.h = w, h end
    function o:SetScript(name, fn) self.scripts[name] = fn end
    function o:RegisterEvent(e) self.events[e] = true end
    function o:SetSnapToPixelGrid(v) self.snap = v end
    function o:SetTexelSnappingBias(v) self.bias = v end
    function o:SetColorTexture(r, g, b, a)
        self.r, self.g, self.b, self.a = r, g, b, a
        counts.setColor = counts.setColor + 1
    end
    function o:CreateTexture(_, layer)
        local t = NewRegion("texture", self)
        t.layer = layer
        self.textures[#self.textures + 1] = t
        return t
    end
    return o
end

UIParent = NewRegion("frame", nil)
function CreateFrame(_, _, parent)
    return NewRegion("frame", parent)
end

local function LoadBridge(ns)
    local chunk = assert(load(SRC_BRIDGE, "=Bridge.lua"))
    return chunk("TranslateProbe", ns)
end

local B = LoadBridge({})

-- Block key (r * 16 + g * 4 + b level indices) of a texture's colour.
local LEVEL_OF = { [0] = 0, [104] = 1, [146] = 2, [255] = 3 }
local function TexKey(t)
    local r, g, b = LEVEL_OF[math.floor(t.r * 255 + 0.5)], LEVEL_OF[math.floor(t.g * 255 + 0.5)],
        LEVEL_OF[math.floor(t.b * 255 + 0.5)]
    if not (r and g and b) or t.a ~= 1 then
        return nil
    end
    return r * 16 + g * 4 + b
end

local BLOCK_CHARS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"
local function KeyChar(k)
    return k and BLOCK_CHARS:sub(k + 1, k + 1) or "?"
end

---------------------------------------------------------------------------- encoder traces
function RunTrace(name)
    local T = TRACE
    counts.traces = counts.traces + 1
    SCREEN_W, SCREEN_H = T.physW, T.physH
    UIParent.shown = true
    local enc = B.NewEncoder({ session = T.session, physW = T.physW, physH = T.physH, mode = T.mode,
        maxCols = T.maxCols, addonBuild = T.addonBuild })
    local f = B.CreateStrip(enc)
    check(not f.shown and not f.on, name .. ": strip hidden while idle")
    check(f.parent == UIParent and f.strata == "TOOLTIP" and f.level == 9000 and f.ignore == true,
        name .. ": strip frame setup")
    check(math.abs(f.scale - 768 / T.physH) < 1e-12 and f.point == "TOPLEFT" and f.x == 0 and f.y == 0,
        name .. ": strip scale and anchor")
    local composed = false
    local compose = enc.Compose
    enc.Compose = function(self)
        compose(self)
        composed = true
    end
    local ev, fi = 1, 1
    local events, frames = T.events, T.frames
    for i = 1, #T.ticks do
        local tick = i - 1
        while events[ev] and events[ev].tick == tick do
            local e = events[ev]
            if e.queue then
                enc:Queue(fromhex(e.queue))
                f.Refresh()
            elseif e.screen then
                SCREEN_W, SCREEN_H = e.screen[1], e.screen[2]
                f.scripts.OnEvent(f, "DISPLAY_SIZE_CHANGED")
                check(math.abs(f.scale - 768 / SCREEN_H) < 1e-12, name .. ": rescale")
            elseif e.mode then
                enc:SetMode(e.mode)
            elseif e.parent ~= nil then
                if e.parent then
                    if not UIParent.shown then
                        UIParent.shown = true
                        FireShow(UIParent)
                    end
                else
                    UIParent:Hide()
                end
            elseif e.hold ~= nil then
                f.held = e.hold
                f.Refresh()
            elseif e.loading ~= nil then
                f.scripts.OnEvent(f, e.loading and "LOADING_SCREEN_ENABLED" or "LOADING_SCREEN_DISABLED")
            end
            ev = ev + 1
        end
        NOW = T.ticks[i] / 1000
        composed = false
        if IsVisible(f) then
            f.scripts.OnUpdate(f, 0.016)
        end
        counts.ticks = counts.ticks + 1
        local exp = frames[fi]
        if composed then
            if check(exp and exp.tick == tick, name .. ": unexpected frame at tick " .. tick) then
                fi = fi + 1
                counts.frames = counts.frames + 1
                check(tablehex(enc.fb, enc.nfb) == exp.bytes, name .. ": frame bytes at tick " .. tick)
                local n = enc:BlockCount()
                check(n == #exp.blocks, name .. ": block count at tick " .. tick)
                local p = {}
                for k = 0, n - 1 do
                    local t = f.textures[k + 1]
                    if not (t and t.shown and t.w == 2 and t.h == 2 and t.x == 2 * math.floor(k / 2)
                            and t.y == -2 * (k % 2) and t.snap == false and t.bias == 0) then
                        fail(name .. ": texture " .. k .. " placement at tick " .. tick)
                        break
                    end
                    p[k + 1] = KeyChar(TexKey(t))
                end
                for k = n + 1, #f.textures do
                    if f.textures[k].shown then
                        fail(name .. ": stale texture shown at tick " .. tick)
                        break
                    end
                end
                check(table.concat(p) == exp.blocks, name .. ": block colours at tick " .. tick)
                counts.blocks = counts.blocks + n
            end
        else
            check(not (exp and exp.tick == tick), name .. ": missing frame at tick " .. tick)
        end
        if not check((f.on and "1" or "0") == T.shown:sub(i, i), name .. ": strip visibility at tick " .. tick) then
            break
        end
        check(f.on == f.shown, name .. ": f.on mirrors the frame")
    end
    check(fi == #frames + 1, name .. ": frames composed " .. (fi - 1) .. " of " .. #frames)
end

---------------------------------------------------------------------------- tags
local function same(a, b)
    if #a ~= #b then
        return false
    end
    for i = 1, #a do
        if type(a[i]) == "table" then
            if not same(a[i], b[i]) then
                return false
            end
        elseif a[i] ~= b[i] then
            return false
        end
    end
    return true
end

function RunTags()
    for _, c in ipairs(TAGS.check) do
        check(B.Crc5(c.slot * 8 + c.row * 2 + c.sessBit) == c.crc5, "crc5 check value " .. c.slot)
        check(same(B.TagDigits(c.slot, c.row, c.sessBit), c.digits), "digits check value " .. c.slot)
    end
    for _, t in ipairs(TAGS.tags) do
        counts.tags = counts.tags + 1
        local id = t.slot .. "/" .. t.row .. "/" .. t.sessBit
        check(B.Crc5(t.payload) == t.crc5, "crc5 " .. id)
        check(same(B.TagDigits(t.slot, t.row, t.sessBit), t.digits), "digits " .. id)
        check(same(B.TagColors(t.slot, t.row, t.sessBit), t.colors), "colors " .. id)
    end
    for _, m in ipairs(TAGS.markup) do
        counts.markup = counts.markup + 1
        check(B.TagMarkup(m.slot, m.row, m.sessBit, m.w, m.h) == m.markup, "markup " .. m.slot)
    end
    for _, m in ipairs(TAGS.plates) do
        counts.markup = counts.markup + 1
        check(B.PlateMarkup(m.w, m.h) == m.markup, "plate markup " .. m.w)
    end
    check(B.TEST_SLOT == 127 and B.VERSION == 2 and same(B.PLATE_RGB, { 32, 40, 64 }), "constants")
end

---------------------------------------------------------------------------- messages
function RunMessages()
    for i, m in ipairs(MESSAGES) do
        local a = m.args
        local out
        if m.fn == "translate" then
            out = B.TranslateMessage(a[1], a[2], a[3], a[4], a[5], a[6], a[7])
        elseif m.fn == "cancel" then
            out = B.CancelMessage(a[1], a[2])
        elseif m.fn == "clear" then
            out = B.ClearMessage()
        elseif m.fn == "test" then
            out = B.TestMessage(a[1])
        elseif m.fn == "view" then
            out = B.ViewMessage(a[1])
        elseif m.fn == "utf8cut" then
            out = B.Utf8Cut(a[1], a[2])
        end
        counts.messages = counts.messages + 1
        check(out and hex(out) == m.hex, "message " .. i .. " " .. m.fn .. " " .. (m.note or ""))
    end
end

---------------------------------------------------------------------------- addon wiring
function RunApi()
    local loads = {}
    local ns = { sessionId = 4321, db = { mode = "auto" }, ADDON_BUILD = 1,
        OnLoad = function(fn) loads[#loads + 1] = fn end }
    SCREEN_W, SCREEN_H = 1920, 1080
    UIParent.shown = true
    CVARS = {}
    local before = #UIParent.children
    local Br = LoadBridge(ns)
    local function ok(cond, msg)
        counts.api = counts.api + 1
        check(cond, "api: " .. msg)
    end
    ok(ns.Bridge == Br and #loads == 1 and loads[1] == Br.Init, "Init registered with ns.OnLoad")
    ok(not Br.IsBusy(), "idle before Init")
    local enc = Br.Init()
    ok(enc and Br.Init() == enc, "Init runs once")
    local f = UIParent.children[before + 1]
    ok(f and not f.shown, "strip created hidden")
    ok(enc.session == 4321 and enc.addonBuild == 1 and enc.mode == 0, "encoder from ns")
    ok(Br.ModeName() == "dense", "auto is dense without the colourblind filter")
    local t = 1000
    local function Tick(n)
        local drawn = 0
        for _ = 1, n do
            t = t + 16
            NOW = t / 1000
            if IsVisible(f) then
                local seq = enc.seq
                f.scripts.OnUpdate(f, 0.016)
                if enc.seq ~= seq then
                    drawn = drawn + 1
                end
            end
        end
        return drawn
    end
    local id = Br.Send(Br.TestMessage(7))
    ok(id == 1 and Br.IsBusy() and f.shown, "Send shows the strip")
    local drawn = Tick(1)
    ok(drawn == 1 and enc.fb[1] == 0xD5 and enc.fb[2] == 2 and enc.fb[4] == 0, "first frame on the next game frame")
    ok(enc.fb[5] * 256 + enc.fb[6] == 4321 and enc.fb[9] == 1 and enc.fb[16] * 256 + enc.fb[17] == 1,
        "session and STATUS.addonBuild in the frame")
    Tick(60)
    ok(not Br.IsBusy() and not f.shown, "hidden again once drained")
    local calls = counts.setColor
    ok(Tick(30) == 0 and counts.setColor == calls, "no work while hidden")
    Br.Hold(true)
    Br.Send(Br.ClearMessage())
    ok(Br.IsBusy() and not f.shown, "Hold keeps the strip hidden with the queue intact")
    ok(Tick(10) == 0, "nothing drawn while held")
    Br.Hold(false)
    ok(f.shown, "Hold(false) shows it")
    Tick(60)
    ok(not Br.IsBusy() and not f.shown, "held message delivered then hidden")
    Br.Send(Br.ClearMessage())
    f.scripts.OnEvent(f, "LOADING_SCREEN_ENABLED")
    ok(not f.shown, "hidden during a loading screen")
    f.scripts.OnEvent(f, "LOADING_SCREEN_DISABLED")
    ok(f.shown, "shown after the loading screen while busy")
    UIParent:Hide()
    ok(Tick(5) == 0, "Alt-Z: no OnUpdate")
    UIParent.shown = true
    FireShow(UIParent)
    ok(Tick(1) == 1, "redrawn at once after Alt-Z")
    Tick(60)
    ok(not f.shown, "drained after Alt-Z")
    ok(Br.SetMode("mono") == "mono" and Br.ModeName() == "mono" and enc.mode == 2, "SetMode mono")
    ok(Br.SetMode("robust") == "robust" and enc.mode == 1, "SetMode robust")
    ok(Br.SetMode("bogus") == "dense" and enc.mode == 0, "unknown mode name is auto")
    CVARS.colorblindSimulator = "2"
    ok(Br.SetMode("auto") == "mono" and Br.ModeName() == "mono", "auto is mono with the colourblind filter")
    CVARS.colorblindSimulator = "0"
    ok(Br.SetMode("auto") == "dense", "auto is dense when the filter is 0")
    ok(ns.db.mode == "auto", "SetMode does not write ns.db.mode")
    SCREEN_W, SCREEN_H = 1366, 768
    f.scripts.OnEvent(f, "DISPLAY_SIZE_CHANGED")
    ok(enc.physW == 1366 and enc.maxCols == 683 and math.abs(f.scale - 1) < 1e-12, "screen change")
    -- CRC-32 check value through the public helper
    local bytes = { ("123456789"):byte(1, -1) }
    -- (signed 32-bit result, bit library convention; fengari integers are 32-bit too)
    ok(B.Crc32(bytes, 1, 9) == bit.tobit(0xCBF43926), "Crc32 check value")
    ok(B.MaxChunkData(1280, 0) == 1880 and B.MaxChunkData(1280, 1) == 923 and B.MaxChunkData(1280, 2) == 604
        and B.MaxChunkData(960, 0) == 1400 and B.MaxChunkData(960, 2) == 444 and B.MaxChunkData(683, 0) == 984
        and B.MaxChunkData(683, 2) == 305, "chunk capacity table")
end

function Summary()
    print(string.format("traces %d, ticks %d, frames %d, blocks %d, SetColorTexture %d, tags %d, markup %d, "
        .. "builder vectors %d, api checks %d", counts.traces, counts.ticks, counts.frames, counts.blocks,
        counts.setColor, counts.tags, counts.markup, counts.messages, counts.api))
end
