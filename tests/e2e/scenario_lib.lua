-- E2E scenario recorder. tests\e2e\make_scenario.js loads tests\mock.lua, the bit shim, this file,
-- then one tests\e2e\scenarios\<name>.lua, which drives the real addon through the E2E API below.
-- Every mock frame (1/60 s) is recorded when something changed: the strip blocks the addon draws,
-- the chat frame rectangles, fonts and scroll offsets, and the visible end of each chat history.
-- fakewow replays the result in a window the Helper captures.
--
-- All chat text in scenarios is synthetic test data. Nothing here reads or keeps real chat.
--
-- API (E2E.Start returns E2E):
--   E2E.Start{ name, helperStartMs, gamma, contrast, fade = {visible, duration}, cvars = {},
--              chat = {l, b, w, h} }   chat clip rectangle of both chat frames (scaled units)
--   E2E.Say(text, sender, event) -> lineID        delivers a chat line, then one frame
--   E2E.Filler()                                   a standard set of synthetic lines
--   E2E.Wait(seconds)
--   E2E.Click(lineID, {shift, frame, drop})        [T]; drop = fakewow hides the strip until the
--                                                  request has been fully shown (a missed request)
--   E2E.Remove(lineID)                             a click on one of the reserved rows
--   E2E.Scroll(chatFrame, offset)
--   E2E.Cover{line or slot, row, frame, fx = {from, to}, pad = {t, r, b, l}, rect = {x, y, w, h}} -> id
--   E2E.Uncover(id)
--   E2E.AltZ(on)                                   hides or shows UIParent (the whole UI)
--   E2E.ShowFrame(chatFrame, {l, b, w, h})          shows a chat frame (scaled-unit rectangle)
--   E2E.HideFrame(chatFrame)
--   E2E.Request(lineID) -> reqId, slot, K
--   E2E.Window{ bubbles = {{slot, rows, text}...}, scroll }   the conversation window (below)
--   E2E.WindowReply{ rows, text, copy }             its reply card (TRANSLATE flags plate|reply|copy)
--   E2E.WindowScroll(px)                            scrolls its thread (rows partly out are hidden)
--   E2E.Popup{ resend }                             the Translate Companion popup and its status card
--   E2E.PopupClose()                                closes it (Continue, Cancel)
--   E2E.Mark(text)                                  annotation in the truth file
--
-- Translate Companion runs in every scenario (user request R1, 2026-10-19): the addon reads its
-- status file at load (CompanionStatus.lua, which the addon ships empty) and a [T] click without a
-- confirmed Translate Companion only opens a popup. E2E.Start loads the file as a running app writes
-- it, so the clicks reserve rows and send TRANSLATE as before.
--   E2E.Finish(tailSeconds)                        sets the global E2E_JSON
local M = Mock
M.echo = false

E2E = {}
local E = E2E

local HISTORY_KEEP = 40
-- A chat window of about 23 rows (the mock's default holds 12), so a scenario's cards stay in view.
local DEFAULT_CHAT = { 20, 60, 430, 330 }
local ARRAY = {}

local ns
local opts = {}
local events = {}
local t0 = 0
local stripFrame
local last = { ui = nil, frames = nil, hist = {}, strip = nil }
local coverSeq = 0

local function Array(t)
    return setmetatable(t or {}, ARRAY)
end

---------------------------------------------------------------------------- JSON
local function EncodeString(s)
    s = s:gsub('[%c"\\]', function(c)
        if c == '"' then
            return '\\"'
        elseif c == "\\" then
            return "\\\\"
        end
        return string.format("\\u%04x", c:byte())
    end)
    return '"' .. s .. '"'
end

local Encode
local function EncodeNumber(v)
    if v ~= v or v == math.huge or v == -math.huge then
        return "null"
    end
    if v == math.floor(v) and math.abs(v) < 2 ^ 53 then
        return string.format("%d", v)
    end
    local s = string.format("%.6f", v):gsub("0+$", ""):gsub("%.$", "")
    return s
end

Encode = function(v)
    local ty = type(v)
    if ty == "nil" then
        return "null"
    elseif ty == "boolean" then
        return v and "true" or "false"
    elseif ty == "number" then
        return EncodeNumber(v)
    elseif ty == "string" then
        return EncodeString(v)
    elseif ty == "table" then
        if getmetatable(v) == ARRAY or #v > 0 then
            local parts = {}
            for i = 1, #v do
                parts[i] = Encode(v[i])
            end
            return "[" .. table.concat(parts, ",") .. "]"
        end
        local keys = {}
        for k in pairs(v) do
            keys[#keys + 1] = tostring(k)
        end
        table.sort(keys)
        local parts = {}
        for i, k in ipairs(keys) do
            parts[i] = EncodeString(k) .. ":" .. Encode(v[k])
        end
        return "{" .. table.concat(parts, ",") .. "}"
    end
    error("cannot encode a " .. ty)
end
E.Encode = Encode

---------------------------------------------------------------------------- recording
local function Round(v)
    return math.floor(v + 0.5)
end

local function NowMs()
    return (M.Now() - t0) * 1000
end

local function Add(ev)
    ev.t = math.floor(NowMs() * 10 + 0.5) / 10
    events[#events + 1] = ev
    return #events
end

local function FrameState(cf)
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local c = cf.FontStringContainer or cf
    local l, b, w, h = c:GetScaledRect()
    local _, fontH = cf:GetFont()
    local eff = cf:GetEffectiveScale()
    local spacing = cf.GetSpacing and cf:GetSpacing() or 0
    return {
        id = cf:GetID(),
        name = cf:GetName() or "",
        shown = cf:IsShown() and true or false,
        clip = Array({ Round(l * k), Round(physH - (b + h) * k), Round(w * k), Round(h * k) }),
        fontPx = Round(fontH * eff * k),
        ppu = eff * k,
        spacingPx = spacing * eff * k,
        scroll = cf:GetScrollOffset() or 0,
    }
end

local function HistoryKey(entries)
    local parts = {}
    local first = math.max(1, #entries - HISTORY_KEEP + 1)
    for i = first, #entries do
        parts[#parts + 1] = tostring(entries[i])
    end
    return table.concat(parts, ",")
end

local function HistoryList(cf)
    local entries = M.History(cf)
    local out = Array()
    local first = math.max(1, #entries - HISTORY_KEEP + 1)
    for i = first, #entries do
        local e = entries[i]
        if e then
            local m = e.message
            local ev = e.extra and e.extra[4]
            out[#out + 1] = {
                m = (type(m) == "string" and not issecretvalue(m)) and m or "",
                rgb = Array({ Round((e.r or 1) * 255), Round((e.g or 1) * 255), Round((e.b or 1) * 255) }),
                ev = type(ev) == "string" and ev or "",
                ts = math.floor((e.timestamp - t0) * 10000 + 0.5) / 10,
            }
        end
    end
    return out
end

-- Strip blocks in block order as hex (6 digits per block); nil when the strip is hidden.
local function StripHex()
    if not (stripFrame and stripFrame:IsVisible()) then
        return nil
    end
    local byK, n = {}, 0
    for _, t in ipairs(M.Textures()) do
        if t.frame == stripFrame and t.shown then
            local x, y = Round(t.x), Round(t.y)
            assert(Round(t.w) == 2 and Round(t.h) == 2 and x % 2 == 0 and (y == 0 or y == 2),
                "strip texture not on the 2x2 block grid")
            local k = x + y / 2
            byK[k] = string.format("%02x%02x%02x", Round(t.r * 255), Round(t.g * 255), Round(t.b * 255))
            n = math.max(n, k + 1)
        end
    end
    local parts = {}
    for k = 0, n - 1 do
        parts[#parts + 1] = assert(byK[k], "strip block missing")
    end
    return table.concat(parts)
end

local function Snapshot()
    local ui = UIParent:IsShown() and true or false
    if ui ~= last.ui then
        Add({ ui = ui })
        last.ui = ui
    end
    local frames = Array()
    for _, cf in ipairs(M.ChatFrames()) do
        frames[#frames + 1] = FrameState(cf)
    end
    local fkey = Encode(frames)
    if fkey ~= last.frames then
        Add({ frames = frames })
        last.frames = fkey
    end
    local hist = Array()
    for _, cf in ipairs(M.ChatFrames()) do
        local key = HistoryKey(M.History(cf))
        local id = cf:GetID()
        if key ~= last.hist[id] then
            last.hist[id] = key
            hist[#hist + 1] = { id = id, entries = HistoryList(cf) }
        end
    end
    if #hist > 0 then
        Add({ history = hist })
    end
    local strip = StripHex()
    if strip ~= last.strip then
        if strip then
            Add({ strip = { shown = true, rgb = strip } })
        else
            Add({ strip = { shown = false } })
        end
        last.strip = strip
    end
end

local PopupDue     -- the synthetic popup: its VIEW is due again (defined with the popup)

local function Step(frames)
    for _ = 1, frames do
        M.Tick(1)
        if PopupDue then
            PopupDue()
        end
        Snapshot()
    end
end

local KIND = { [0x10] = "translate", [0x11] = "cancel", [0x12] = "clear", [0x13] = "test", [0x14] = "view",
    [0x15] = "colours" }

local function RecordSent(msg)
    local ty = msg:byte(1)
    local ev = { kind = KIND[ty] or "unknown", bytes = #msg }
    if ty == 0x15 then
        ev.req = msg:byte(2) * 256 + msg:byte(3)
    elseif ty == 0x10 or ty == 0x11 then
        ev.req = msg:byte(2) * 256 + msg:byte(3)
        ev.slot = msg:byte(4)
        if ty == 0x10 then
            ev.rows = msg:byte(5)
        end
    elseif ty == 0x13 then
        ev.req = msg:byte(2) * 256 + msg:byte(3)
    end
    Add({ sent = ev })
end

---------------------------------------------------------------------------- API
function E.Start(o)
    opts = o or {}
    assert(type(opts.name) == "string", "E2E.Start needs a name")
    for name, value in pairs(opts.cvars or {}) do
        M.SetCVar(name, value)
    end
    local chat = opts.chat or DEFAULT_CHAT
    for _, cf in ipairs(M.ChatFrames()) do
        M.SetChatRect(cf, chat[1], chat[2], chat[3], chat[4])
    end
    -- what a running Translate Companion writes into the deployed CompanionStatus.lua (its
    -- helper\app\src\status_file.rs): the session starts confirmed (Presence.lua)
    local status = string.format('TranslateProbe_CompanionStatus = { v = 1, version = "0.6.0", build = 6, '
        .. 'state = "running", heartbeat = %d }', time())
    TranslateProbe_CompanionStatus = { v = 1, version = "0.6.0", build = 6, state = "running", heartbeat = time() }
    ns = M.Load(ADDON_FILES, function(file)
        if file == "CompanionStatus.lua" then
            return status
        end
        return ADDON_SOURCES[file]
    end)
    if ns.Presence and ns.Presence.IsConfirmed then
        assert(ns.Presence.IsConfirmed(), "the status file did not confirm Translate Companion")
    end
    -- E.Click tests the rows under a chat line: a [T] click opens the conversation window by
    -- default since 2026-10-19 (display = "window"), so the scenarios use the inline mode
    ns.db.display = "inline"
    for _, f in ipairs(M.Frames()) do
        if f:GetFrameStrata() == "TOOLTIP" and f:GetFrameLevel() == 9000 then
            stripFrame = f
        end
    end
    assert(stripFrame, "strip frame not found (Bridge.CreateStrip)")
    local send = ns.Bridge.Send
    ns.Bridge.Send = function(msg)
        RecordSent(msg)
        return send(msg)
    end
    t0 = M.Now()
    Snapshot()
    return E
end

function E.Wait(seconds)
    Step(math.max(1, Round(seconds * 60)))
end

function E.Say(text, sender, event)
    local id = M.Deliver(event or "CHAT_MSG_SAY", text, sender or "Aldric")
    Step(1)
    return id
end

-- Synthetic chat lines: several channels and colours, an item link, raid marks, other scripts.
function E.Filler()
    local ids = {}
    ids.party = E.Say("anyone up for Deadmines tonight? we need a healer", "Brenna", "CHAT_MSG_PARTY")
    ids.trade = E.Say("selling |cffffffff|Hitem:2589::::::::|h[Linen Cloth]|h|r x20, cheap, whisper me", "Caius",
        "CHAT_MSG_CHANNEL")
    ids.guild = E.Say("gg everyone, that was a clean run", "Doran", "CHAT_MSG_GUILD")
    ids.fr = E.Say("quelqu'un pour finir la quete de la mine ce soir ?", "Elowen", "CHAT_MSG_SAY")
    ids.whisper = E.Say("can you help me with the elite at the farm? |cff00fff6{skull}|r first", "Fenwick",
        "CHAT_MSG_WHISPER")
    ids.long = E.Say("lf2m for a full clear, tank and healer, we start at the gate in ten minutes, bring potions "
        .. "and food, summons available, first boss is on farm so it should be quick", "Garrick", "CHAT_MSG_SAY")
    return ids
end

function E.Click(lineID, o)
    o = o or {}
    local cf = o.frame or ChatFrame1
    local before = #events
    M.Click("addon:TranslateProbe:t:" .. ns.session .. ":" .. lineID, cf, o.shift and true or false)
    if o.drop then
        local req, slot = ns.Request.Get(lineID)
        assert(req, "dropped click sent no request")
        -- before this click's sent events, so fakewow flags them as dropped
        table.insert(events, before + 1, { t = math.floor(NowMs() * 10 + 0.5) / 10,
            drop = { on = true, req = req, slot = slot } })
        local guard = 0
        repeat
            Step(1)
            guard = guard + 1
        until not ns.Bridge.IsBusy() or guard > 600
        Add({ drop = { on = false, req = req, slot = slot } })
    end
    Step(1)
end

function E.Remove(lineID)
    M.Click("addon:TranslateProbe:x:" .. ns.session .. ":" .. lineID, ChatFrame1)
    Step(1)
end

function E.Scroll(cf, offset)
    cf:SetScrollOffset(offset)
    Step(1)
end

function E.Request(lineID)
    return ns.Request.Get(lineID)
end

function E.Cover(o)
    coverSeq = coverSeq + 1
    local c = { id = coverSeq, on = true }
    if o.rect then
        c.rect = Array({ o.rect[1], o.rect[2], o.rect[3], o.rect[4] })
    else
        local slot = o.slot
        if o.line then
            local _, s = ns.Request.Get(o.line)
            slot = s
        end
        assert(slot, "E2E.Cover needs a line with a request, a slot or a rect")
        c.slot, c.row, c.frame = slot, o.row or 0, o.frame or 1
        c.fx = Array(o.fx or { 0.5, 1.0 })
        if o.pad then
            c.pad = Array(o.pad)
        end
    end
    Add({ cover = c })
    Step(1)
    return coverSeq
end

function E.Uncover(id)
    Add({ cover = { id = id, on = false } })
    Step(1)
end

function E.AltZ(on)
    if on then
        UIParent:Hide()
    else
        UIParent:Show()
    end
    Step(1)
end

function E.ShowFrame(cf, rect)
    if rect then
        M.SetChatRect(cf, rect[1], rect[2], rect[3], rect[4])
    end
    cf:Show()
    Step(1)
end

function E.HideFrame(cf)
    cf:Hide()
    Step(1)
end

---------------------------------------------------------------------------- conversation window
-- A synthetic conversation window (design A.1, A.4 and A.6.4; docs\BRIDGE.md section 8c) drawn by
-- fakewow as a layer over the chat frames: opaque rectangles in physical px. Its rows are real
-- protocol tags (Bridge.TagColors) and plates on the opaque bubble fill (Bridge.BUBBLE_RGB), rows
-- touching at the row pitch, a reply card, and per-row visibility (a row not entirely inside the
-- thread is not drawn: the fill shows there). No hint lane any more (addon build 6, user request
-- R1, 2026-10-19): a bubble's box ends its padding under its last row. The VIEW (window first, then
-- the chat frame it overlaps) and the TRANSLATE messages go through the addon's own Bridge.Send.
-- The addon's window modules are not used: the rig checks the Helper against the protocol.
local WIN = {
    x = 126, y = 214, w = 620, h = 640,                -- the frame (px)
    threadTop = 320, threadBottom = 700,
    colX = 150, bw = 540, pad = 6, rowPx = 23, fontPx = 20, block = 6,
    cardGap = 8,
    border = { 150, 120, 70 }, frameBg = { 22, 20, 18 }, header = { 34, 30, 26 }, threadBg = { 12, 12, 12 },
    composer = { 40, 36, 30 },
}
local win = nil       -- { bubbles, scroll, reply, reqs }
local winReq = 900    -- reqIds of the synthetic window's requests (the addon's own start at 1)

local function Rect7(x, y, w, h, c)
    return Array({ x, y, w, h, c[1], c[2], c[3] })
end

-- The tag (8 blocks) and plate of one row at (x, y), appended to rects; the plate item for the truth.
local function Row(rects, items, x, y, slot, row)
    local sess = ns.sessionId % 2
    local colors = ns.Bridge.TagColors(slot, row, sess)
    for i = 1, 8 do
        rects[#rects + 1] = Rect7(x + (i - 1) * WIN.block, y, WIN.block, WIN.rowPx, colors[i])
    end
    local px0 = x + 8 * WIN.block
    local pw = WIN.colX + WIN.bw - WIN.pad - px0
    rects[#rects + 1] = Rect7(px0, y, pw, WIN.rowPx, ns.Bridge.PLATE_RGB)
    items[#items + 1] = { view = 200, slot = slot, row = row % 4, sess = sess, tag = Array({ x, y, 8 * WIN.block, WIN.rowPx }),
        plate = Array({ px0, y, pw, WIN.rowPx }), visible = true, alpha = 1, covered = Array() }
end

local function CardTop()
    return WIN.threadBottom + WIN.cardGap
end

local function CardHeight(k)
    return 2 * WIN.pad + k * WIN.rowPx    -- rows and the padding
end

local function ViewRect()
    local bottom = WIN.threadBottom
    if win.reply then
        bottom = CardTop() + CardHeight(win.reply.rows)
    end
    return { WIN.colX, WIN.threadTop, WIN.bw, bottom - WIN.threadTop }
end

local DrawWindow

function DrawWindow()
    local rects, items, hidden = Array(), Array(), Array()
    local B = ns.Bridge.BUBBLE_RGB
    rects[#rects + 1] = Rect7(WIN.x - 3, WIN.y - 3, WIN.w + 6, WIN.h + 6, WIN.border)
    rects[#rects + 1] = Rect7(WIN.x, WIN.y, WIN.w, WIN.h, WIN.frameBg)
    rects[#rects + 1] = Rect7(WIN.x, WIN.y, WIN.w, 90, WIN.header)
    rects[#rects + 1] = Rect7(WIN.x + 12, WIN.threadTop, WIN.w - 24, WIN.threadBottom - WIN.threadTop, WIN.threadBg)
    local y = WIN.threadTop + 10 - win.scroll
    local x0 = WIN.colX + WIN.pad
    for _, b in ipairs(win.bubbles) do
        local h = CardHeight(b.rows)
        local top, bot = math.max(y, WIN.threadTop), math.min(y + h, WIN.threadBottom)
        if bot > top then
            rects[#rects + 1] = Rect7(WIN.colX, top, WIN.bw, bot - top, B)
        end
        for r = 0, b.rows - 1 do
            local ry = y + WIN.pad + r * WIN.rowPx
            if ry >= WIN.threadTop and ry + WIN.rowPx <= WIN.threadBottom then
                Row(rects, items, x0, ry, b.slot, r)
            else
                hidden[#hidden + 1] = Array({ b.slot, r })
            end
        end
        y = y + h + 14
    end
    local card = nil
    if win.reply then
        local k, top = win.reply.rows, CardTop()
        card = Array({ WIN.colX, top, WIN.bw, CardHeight(k) })
        rects[#rects + 1] = Rect7(WIN.colX, top, WIN.bw, CardHeight(k), B)
        for r = 0, k - 1 do
            Row(rects, items, x0, top + WIN.pad + r * WIN.rowPx, 126, r)
        end
        rects[#rects + 1] = Rect7(WIN.x + 12, top + CardHeight(k) + 8, WIN.w - 24, 30, WIN.composer)
    end
    Add({ layer = { id = 1, on = true, rects = rects, items = items, meta = { view = Array(ViewRect()), scroll = win.scroll,
        hidden = hidden, card = card, replySlot = win.reply and 126 or nil, bubble = Array(ns.Bridge.BUBBLE_RGB) } } })
end

-- The VIEW entry of chat frame 1 (physical px), as the addon's Reserve.View writes it.
local function ChatViewEntry()
    local _, physH = GetPhysicalScreenSize()
    local k = physH / 768
    local cf = ChatFrame1
    local c = cf.FontStringContainer or cf
    local l, b, w, h = c:GetScaledRect()
    local _, fontH = cf:GetFont()
    local eff = cf:GetEffectiveScale()
    local spacing = cf.GetSpacing and cf:GetSpacing() or 0
    return { id = cf:GetID(), x = Round(l * k), y = Round(physH - (b + h) * k), w = Round(w * k), h = Round(h * k),
        fontPx = Round(fontH * eff * k), rowPx = Round((fontH + spacing) * eff * k) }
end

-- VIEW: the window first (VIEW id 200), then the chat frame it overlaps (the addon's Reserve.View
-- does the same once the window module is there).
local function SendView()
    local v = ViewRect()
    ns.Bridge.Send(ns.Bridge.ViewMessage({
        { id = 200, x = v[1], y = v[2], w = v[3], h = v[4], fontPx = WIN.fontPx, rowPx = WIN.rowPx },
        ChatViewEntry(),
    }))
end

local function Translate(slot, rows, flags, target, text)
    winReq = winReq + 1
    -- link colours go right before their TRANSLATE, as Request.lua sends them (docs\BRIDGE.md 3b)
    local colours = ns.Bridge.ColoursMessage(winReq, text)
    if colours then
        ns.Bridge.Send(colours)
    end
    ns.Bridge.Send(ns.Bridge.TranslateMessage(winReq, slot, rows, flags, target, text, {}))
    return winReq
end

-- o.bubbles = { { slot (100..124), rows (1..6), text }, ... }: the incoming bubbles, top first.
function E.Window(o)
    win = { bubbles = o.bubbles or {}, scroll = o.scroll or 0, reply = nil }
    SendView()
    -- each TRANSLATE is queued before the bubble's rows are shown (design A.3 step 4)
    for _, b in ipairs(win.bubbles) do
        Translate(b.slot, b.rows, ns.Bridge.FLAG_PLATE, o.target or "fr", b.text)
    end
    Step(1)
    DrawWindow()
    Step(1)
end

-- The reply card: TRANSLATE slot 126, rows = 1 + text rows, flags plate|reply (+copy).
function E.WindowReply(o)
    assert(win, "E2E.WindowReply needs E2E.Window first")
    win.reply = { rows = o.rows or 3 }
    local flags = ns.Bridge.FLAG_PLATE + ns.Bridge.FLAG_REPLY + (o.copy == false and 0 or ns.Bridge.FLAG_COPY)
    SendView()
    local req = Translate(126, win.reply.rows, flags, o.target or "de", o.text)
    Step(1)
    DrawWindow()
    Step(1)
    return req
end

function E.WindowScroll(px)
    assert(win, "E2E.WindowScroll needs E2E.Window first")
    win.scroll = px
    DrawWindow()
    Step(1)
end

---------------------------------------------------------------------------- Translate Companion popup
-- The popup a [T] click opens while no Translate Companion is confirmed (user request R1,
-- 2026-10-19; the addon's CompanionDialog.lua, docs\BRIDGE.md section 8d), drawn by fakewow as layer
-- 2 in the middle of the client: a dark dialog with a border, a gold title and two buttons, the
-- status card (one row of Bridge.PRESENCE_SLOT: a real tag and the plate over a BUBBLE_RGB fill
-- with `pad` px above, under and right of them and the wider `left` fill of the addon's
-- CompanionDialog.CardMetrics left of the tag, where a discovery scan samples; never a TRANSLATE)
-- and the addon's grey caption under the card (never on it). Its VIEW (id 201, the card's fill, then the chat frame) goes through the addon's own
-- Bridge.Send when the popup opens and again every `resend` seconds while it shows (the addon: 2 s;
-- 0: once); closing it sends the chat frame's VIEW alone, as the addon does.
local POP = {
    w = 420, h = 200, cardTop = 70, cardInset = 20, pad = 6, rowPx = 20, fontPx = 16, block = 6,
    border = { 150, 120, 70 }, bg = { 18, 16, 14 }, title = { 255, 209, 0 }, grey = { 128, 128, 128 },
    button = { 60, 52, 40 },
}
local PRESENCE_VIEW = 201
local popup = nil     -- { card = {x, y, w, h}, resend, nextView }

local function PopupGeometry()
    local physW, physH = GetPhysicalScreenSize()
    local x, y = Round((physW - POP.w) / 2), Round((physH - POP.h) / 2)
    local card = { x + POP.cardInset, y + POP.cardTop, POP.w - 2 * POP.cardInset, POP.rowPx + 2 * POP.pad }
    return x, y, card
end

local function DrawPopup()
    local rects, items = Array(), Array()
    local x, y, card = PopupGeometry()
    rects[#rects + 1] = Rect7(x - 3, y - 3, POP.w + 6, POP.h + 6, POP.border)
    rects[#rects + 1] = Rect7(x, y, POP.w, POP.h, POP.bg)
    for gx = x + 30, x + 200, 9 do    -- the title's glyphs
        rects[#rects + 1] = Rect7(gx, y + 22, 5, 14, POP.title)
    end
    -- the status card: its fill, the tag of slot 125 row 0, the plate to the fill's padding
    rects[#rects + 1] = Rect7(card[1], card[2], card[3], card[4], ns.Bridge.BUBBLE_RGB)
    local sess = ns.sessionId % 2
    local colors = ns.Bridge.TagColors(ns.Bridge.PRESENCE_SLOT, 0, sess)
    -- left of the tag: 0.75 of the row height (the Companion's discovery reach) and 3 px, as the addon
    local left = math.max(POP.pad, math.ceil(ns.CompanionDialog.DISCOVERY_LEFT * POP.rowPx + 3))
    local tx, ty = card[1] + left, card[2] + POP.pad
    for i = 1, 8 do
        rects[#rects + 1] = Rect7(tx + (i - 1) * POP.block, ty, POP.block, POP.rowPx, colors[i])
    end
    local px0 = tx + 8 * POP.block
    local pw = card[1] + card[3] - POP.pad - px0
    rects[#rects + 1] = Rect7(px0, ty, pw, POP.rowPx, ns.Bridge.PLATE_RGB)
    items[#items + 1] = { view = PRESENCE_VIEW, slot = ns.Bridge.PRESENCE_SLOT, row = 0, sess = sess,
        tag = Array({ tx, ty, 8 * POP.block, POP.rowPx }), plate = Array({ px0, ty, pw, POP.rowPx }), visible = true,
        alpha = 1, covered = Array() }
    -- the grey caption under the card, then the buttons
    for gx = card[1] + POP.pad, card[1] + POP.pad + 230, 7 do
        rects[#rects + 1] = Rect7(gx, card[2] + card[4] + 6, 4, 11, POP.grey)
    end
    for i = 0, 2 do
        rects[#rects + 1] = Rect7(x + 30 + i * 125, y + POP.h - 40, 112, 22, POP.button)
    end
    Add({ layer = { id = 2, on = true, rects = rects, items = items,
        meta = { presence = { view = PRESENCE_VIEW, slot = ns.Bridge.PRESENCE_SLOT, card = Array(card) } } } })
end

local function SendPopupView()
    local _, _, card = PopupGeometry()
    ns.Bridge.Send(ns.Bridge.ViewMessage({
        { id = PRESENCE_VIEW, x = card[1], y = card[2], w = card[3], h = card[4], fontPx = POP.fontPx,
            rowPx = POP.rowPx },
        ChatViewEntry(),
    }))
end

-- The popup's VIEW again every `resend` seconds while it shows.
function PopupDue()
    if popup and popup.resend > 0 and M.Now() >= popup.nextView then
        popup.nextView = popup.nextView + popup.resend
        SendPopupView()
    end
end

-- o.resend: seconds between two VIEWs while the popup shows (default 2, as the addon; 0: once).
function E.Popup(o)
    o = o or {}
    popup = { resend = o.resend or 2 }
    popup.nextView = M.Now() + popup.resend
    SendPopupView()
    Step(1)
    DrawPopup()
    Step(1)
end

function E.PopupClose()
    assert(popup, "E2E.PopupClose needs E2E.Popup first")
    popup = nil
    Add({ layer = { id = 2, on = false } })
    ns.Bridge.Send(ns.Bridge.ViewMessage({ ChatViewEntry() }))
    Step(1)
end

function E.Mark(text)
    Add({ mark = text })
end

function E.Finish(tail)
    E.Wait(tail or 3)
    local errs = M.Errors()
    if #errs > 0 then
        error("addon errors during the scenario: " .. table.concat(errs, " | "), 0)
    end
    local physW, physH = GetPhysicalScreenSize()
    local doc = {
        format = 1,
        name = opts.name,
        locale = GetLocale(),
        physW = physW,
        physH = physH,
        durationMs = Round(NowMs()),
        helperStartMs = opts.helperStartMs or 0,
        session = ns.sessionId,
        display = { gamma = opts.gamma or 1, contrast = opts.contrast or 1 },
        fade = opts.fade or { visible = 120, duration = 3 },
        events = Array(events),
    }
    E2E_JSON = Encode(doc)
    E2E_EVENTS = #events
end
