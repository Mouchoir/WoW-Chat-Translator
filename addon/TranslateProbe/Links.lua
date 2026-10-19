-- Chat markup of the 12.x client: colour escapes and hyperlinks of a message, the colour of each
-- link name (for the TRANSLATE marks), and the message's links made usable in the conversation
-- window (the links bar under a bubble and its Original text, Bubble.lua).
--
-- Markup (read on 2026-10-19 in Gethe/wow-ui-source, branches live 12.1.5 and forever 1.60.1):
-- * Colours: "|cAARRGGBB" (the alpha is ignored) and the named colours "|cn<name>:" (10.0+).
--   <name> is a global colour (Blizzard_SharedXMLBase\Color.lua makes a global ColorMixin of each
--   C_UIColor.GetColors() baseTag, such as NORMAL_FONT_COLOR) or IQ<n>, the colour of item
--   quality n with the user's colour overrides (11.1.5+; Blizzard_Colors\Mainline\ColorManager.lua,
--   ColorManager.GetFormattedStringForItemQuality writes "|cnIQ%d:%s|r", and the client writes item
--   links that way: "|cnIQ3:|Hitem:...|h[Necromancer Leggings]|h|r"). "|r" closes the innermost
--   colour (colours nest since 9.0.1).
-- * Hyperlinks "|H<type>:<data>|h<name>|h", textures "|T...|t", atlases "|A...|a", protected
--   Battle.net names "|K...|k"; "||" is a pipe the player typed.
-- * The colour of a link name: the innermost colour open at the link (a colour at the start of its
--   name wins: that is what the client draws), as RRGGBB; a named colour through the client
--   (IQ<n>: ColorManager.GetColorDataForItemQuality, else C_ColorOverrides.GetColorForQuality,
--   else C_Item.GetItemQualityColor, else ITEM_QUALITY_COLORS; another name: its global colour);
--   none, or one the client cannot resolve: the gold of NORMAL_FONT_COLOR (FFD100).
-- * Clicks do what a chat frame does: ChatFrameMixin:OnHyperlinkClick calls SetItemRef(link, text,
--   button, chatFrame) (Blizzard_UIPanels_Game\Mainline\ItemRef.lua: LinkUtil.ProcessLink, the
--   link handlers of ItemRefHandlers.lua such as the map pin, then for a modified click
--   HandleModifiedItemClick: dress-up on Ctrl, else the ItemRefTooltip). A Shift-click
--   (IsModifiedClick("CHATLINK")) first puts the link in the active chat edit box with
--   ChatFrameUtil.InsertLink, the 12.x name of ChatEdit_InsertLink
--   (Blizzard_ChatFrameBase\Mainline\ChatFrameUtilOverrides.lua), so a map pin goes in too; with no
--   edit box open it goes on to SetItemRef, as in the chat. The types whose Shift-click the client
--   handles itself (player names, Battle.net players, channels, appearances...: CLIENT_CHATLINK)
--   go straight to SetItemRef.
-- * Hover: GameTooltip:SetHyperlink (a tooltip data accessor, Blizzard_SharedXMLGame\Tooltip\
--   TooltipDataHandler.lua: false when the client has no tooltip for that link), else the link's
--   name and what a click does.
-- * Hyperlinks in a font string work when its frame has Frame:SetHyperlinksEnabled(true) and
--   OnHyperlinkClick, OnHyperlinkEnter, OnHyperlinkLeave scripts (QuestInfo.lua uses them this
--   way). SimpleFrameAPIDocumentation marks SetHyperlinksEnabled IsProtectedFunction, the flag
--   Show and Hide also carry, which most likely restricts it on protected frames only; as a
--   conservative choice until G20 step 7 settles it in game, it is called out of combat only, and
--   after the combat when the frame was made during one.
-- Nothing here stores a text, a link or a name: the parsed links live with the message in memory.
local _, ns = ...
local L = ns.L

local Links = {}
ns.Links = Links

local DEFAULT_RGB = "FFD100"            -- NORMAL_FONT_COLOR: a link whose colour is not known
Links.DEFAULT_RGB = DEFAULT_RGB

-- Link types with no tooltip of their own (actions on a player, a channel, a URL, an addon, a
-- censored line): hovering them shows the name and what a click does.
local NO_TOOLTIP = {
    player = true, BNplayer = true, playerCommunity = true, BNplayerCommunity = true, playerGM = true, GMChat = true,
    channel = true, urlIndex = true, addon = true, censoredmessage = true, censoredmessagerewrite = true,
    censoredmessageconfirmsend = true, reportcensoredmessage = true,
}

-- Link types whose Shift-click the client handles itself in SetItemRef (read on 2026-10-19 in
-- ItemRefHandlers.lua, ItemRefHandlersShared.lua and ItemButtonTemplate.lua, branch live): a
-- player link gives the plain name, fills an open add-friend, ignore, guild or channel-invite
-- popup, else sends /who; a Battle.net player link is never put in a message (it would carry an
-- encoded account id); a channel link toggles the channel list; an appearance link inserts its
-- source item; a club finder link its own text; another player's instance lock is refused
-- (HandleModifiedItemClick). They go straight to SetItemRef, as from the chat.
local CLIENT_CHATLINK = {
    player = true, playerCommunity = true, playerGM = true, BNplayer = true, BNplayerCommunity = true,
    channel = true, transmogappearance = true, clubFinder = true, instancelock = true, api = true,
}

local counters = { clicks = 0, inserted = 0, refs = 0, refFailed = 0, tooltips = 0, plainTips = 0, deferred = 0 }

local function IsText(v)
    return not issecretvalue(v) and type(v) == "string"
end

---------------------------------------------------------------------------- colours
local function Byte(v)
    if issecretvalue(v) or type(v) ~= "number" or v ~= v then
        return nil
    end
    return math.max(0, math.min(255, math.floor(v * 255 + 0.5)))
end

-- RRGGBB of the client's own colour code ("|cff0070dd" or "ff0070dd"), or nil.
local function CodeHex(code)
    if not IsText(code) then
        return nil
    end
    local hex = code:match("^|c%x%x(%x%x%x%x%x%x)$") or code:match("^%x%x(%x%x%x%x%x%x)$")
    return hex and hex:upper() or nil
end

-- "RRGGBB" (uppercase) of a colour: the client's own code when it gives one (a colour data table's
-- hex, a ColorMixin's GenerateHexColor, the 4th value of C_Item.GetItemQualityColor: the code the
-- client writes in its links), else r, g, b in 0..1 (a table's or given apart); or nil.
local function Hex(r, g, b, code)
    if type(r) == "table" and not issecretvalue(r) then
        local color = r
        local hex = CodeHex(color.hex)
        if not hex and type(color.GenerateHexColor) == "function" then
            local ok, generated = pcall(color.GenerateHexColor, color)
            hex = ok and CodeHex(generated) or nil
        end
        if hex then
            return hex
        end
        r, g, b = color.r, color.g, color.b
    elseif CodeHex(code) then
        return CodeHex(code)
    end
    r, g, b = Byte(r), Byte(g), Byte(b)
    if r and g and b then
        return ("%02X%02X%02X"):format(r, g, b)
    end
end
Links.Hex = Hex

local function Try(fn, ...)
    if type(fn) ~= "function" then
        return nil
    end
    local ok, a, b, c, d = pcall(fn, ...)
    if ok then
        return Hex(a, b, c, d)
    end
end

-- RRGGBB of item quality q as the client draws it (the user's colour overrides included), or nil.
function Links.QualityRGB(q)
    q = tonumber(q)
    if not q then
        return nil
    end
    local rgb = (type(ColorManager) == "table" and Try(ColorManager.GetColorDataForItemQuality, q))
        or (type(C_ColorOverrides) == "table" and Try(C_ColorOverrides.GetColorForQuality, q))
        or (type(C_Item) == "table" and Try(C_Item.GetItemQualityColor, q))
    if rgb then
        return rgb
    end
    if type(ITEM_QUALITY_COLORS) == "table" then
        return Hex(ITEM_QUALITY_COLORS[q])
    end
end

-- RRGGBB of a named colour ("IQ3", "NORMAL_FONT_COLOR"), or nil.
function Links.NamedRGB(name)
    if not IsText(name) then
        return nil
    end
    local q = name:match("^IQ(%d+)$")
    if q then
        return Links.QualityRGB(q)
    end
    if name:find("^[%a_][%w_]*$") then
        local color = _G[name]
        if type(color) == "table" and not issecretvalue(color) then
            return Hex(color)
        end
    end
end

-- RRGGBB of a colour escape ("|cff0070dd", "|cnIQ3:"), or nil.
function Links.EscapeRGB(escape)
    if not IsText(escape) then
        return nil
    end
    local hex = escape:match("^|c%x%x(%x%x%x%x%x%x)$")
    if hex then
        return hex:upper()
    end
    local name = escape:match("^|cn([^:|]*):$")
    if name then
        return Links.NamedRGB(name)
    end
end

-- The text with every colour escape ("|cAARRGGBB", "|cn<name>:", "|r") replaced by repl.
function Links.StripColors(text, repl)
    repl = repl or ""
    return (text:gsub("|c%x%x%x%x%x%x%x%x", repl):gsub("|cn[^:|]*:", repl):gsub("|r", repl))
end

-- A link's name as a reader sees it: colours, textures, atlases and protected names removed.
local function CleanName(name)
    name = Links.StripColors(name, "")
    return (name:gsub("|T.-|t", ""):gsub("|A.-|a", ""):gsub("|K.-|k", ""))
end

---------------------------------------------------------------------------- the scanner
-- One hyperlink: link (its data, "item:2589:..."), kind ("item"), name (as drawn, codes inside
-- kept), plain (the name a reader reads), escape (the colour escape it is drawn in, or nil), rgb
-- (RRGGBB of that colour, or nil when unknown), text (the whole link as a chat frame passes it to
-- SetItemRef: the colour, |H...|h...|h and |r) and label (its name in its colour, for a button).
local function NewEntry(data, name, escape)
    local inner = name:match("^(|c%x%x%x%x%x%x%x%x)") or name:match("^(|cn[^:|]*:)")
    local drawn = inner or escape
    local whole = "|H" .. data .. "|h" .. name .. "|h"
    if escape then
        whole = escape .. whole .. "|r"
    end
    return {
        link = data, kind = data:match("^([^:]*)"), name = name, plain = CleanName(name), escape = drawn,
        rgb = drawn and Links.EscapeRGB(drawn) or nil, text = whole,
        label = (drawn or ("|cff" .. DEFAULT_RGB)) .. name .. "|r",
    }
end

-- Walks a message: emit("text", s) for each run of text (escapes other than those below, such as
-- "|n" and a typed "||", stay in it) and emit("link", entry) for each hyperlink, in order. Colour
-- escapes, textures, atlases and protected names produce nothing (the colours set the colour of the
-- links inside them).
function Links.Scan(s, emit)
    local stack = {}
    local pos, from = 1, 1
    local function Flush(to)
        if to >= from then
            emit("text", s:sub(from, to))
        end
    end
    while true do
        local p = s:find("|", pos, true)
        if not p then
            break
        end
        local c = s:sub(p + 1, p + 1)
        local stop
        if c == "|" then
            pos = p + 2
        elseif c == "c" then
            local e = s:match("^|c%x%x%x%x%x%x%x%x()", p) or s:match("^|cn[^:|]*:()", p)
            if e then
                Flush(p - 1)
                stack[#stack + 1] = s:sub(p, e - 1)
                stop = e - 1
            end
        elseif c == "r" then
            Flush(p - 1)
            stack[#stack] = nil
            stop = p + 1
        elseif c == "H" then
            local data, name, e = s:match("^|H(.-)|h(.-)|h()", p)
            if data then
                Flush(p - 1)
                emit("link", NewEntry(data, name, stack[#stack]))
                stop = e - 1
            end
        elseif c == "T" or c == "A" or c == "K" then
            local q = s:find("|" .. c:lower(), p + 2, true)
            if q then
                Flush(p - 1)
                stop = q + 1
            end
        end
        if stop then
            pos, from = stop + 1, stop + 1
        elseif c ~= "|" then
            pos = p + 1
        end
    end
    Flush(#s)
end

-- The message's hyperlinks in order, each once (same link and name), without the nameless ones.
function Links.Parse(text)
    local list = {}
    if not IsText(text) then
        return list
    end
    local seen = {}
    Links.Scan(text, function(kind, entry)
        if kind == "link" and entry.plain:find("%S") then
            local key = entry.link .. "\0" .. entry.name
            if not seen[key] then
                seen[key] = true
                list[#list + 1] = entry
            end
        end
    end)
    return list
end

---------------------------------------------------------------------------- clicks and hovers
local function Modified(action)
    if type(IsModifiedClick) == "function" then
        local ok, on = pcall(IsModifiedClick, action)
        if ok and not issecretvalue(on) then
            return on and true or false
        end
    end
    if action == "CHATLINK" then
        return IsShiftKeyDown() and true or false
    end
    return IsControlKeyDown() and true or false
end

-- Puts a link in the active chat edit box (as HandleModifiedItemClick does: GetFixedLink first).
-- True when it went in.
local function Insert(text)
    if type(GetFixedLink) == "function" then
        local ok, fixed = pcall(GetFixedLink, text)
        if ok and IsText(fixed) and fixed ~= "" then
            text = fixed
        end
    end
    local insert = (type(ChatFrameUtil) == "table" and ChatFrameUtil.InsertLink) or ChatEdit_InsertLink
    if type(insert) ~= "function" then
        return false
    end
    local ok, done = pcall(insert, text)
    return ok and done == true
end

-- A click on a link of the message (the links bar or the Original text): link is its data, text
-- the whole link, button the mouse button. Returns what was done: "insert", "ref" or nil.
function Links.Click(link, text, button)
    if not IsText(link) or link == "" then
        return nil
    end
    if not IsText(text) or text == "" then
        text = "|H" .. link .. "|h|h"
    end
    button = button or "LeftButton"
    counters.clicks = counters.clicks + 1
    if button == "LeftButton" and not CLIENT_CHATLINK[link:match("^([^:]*)")] and Modified("CHATLINK") and Insert(text) then
        counters.inserted = counters.inserted + 1
        return "insert"
    end
    if type(SetItemRef) == "function" then
        if pcall(SetItemRef, link, text, button, DEFAULT_CHAT_FRAME) then
            counters.refs = counters.refs + 1
            return "ref"
        end
        counters.refFailed = counters.refFailed + 1
    end
    return nil
end

-- The tooltip of a link under the cursor: the client's own (GameTooltip:SetHyperlink) when it has
-- one for that link, else the name in its colour and what a click does. Returns "hyperlink",
-- "plain" or nil.
function Links.ShowTooltip(owner, link, text)
    if not IsText(link) or link == "" then
        return nil
    end
    local tip = GameTooltip
    tip:SetOwner(owner, "ANCHOR_RIGHT")
    local kind = link:match("^([^:]*)")
    if not NO_TOOLTIP[kind] and type(tip.SetHyperlink) == "function" then
        local ok, shown = pcall(tip.SetHyperlink, tip, link)
        if ok and shown ~= false and tip:IsShown() then
            counters.tooltips = counters.tooltips + 1
            return "hyperlink"
        end
        tip:SetOwner(owner, "ANCHOR_RIGHT")
    end
    local entry = IsText(text) and Links.Parse(text)[1]
    GameTooltip_SetTitle(tip, entry and entry.label or kind)
    GameTooltip_AddInstructionLine(tip, L.CONV_LINK_TIP_CLICK)
    GameTooltip_AddInstructionLine(tip, L.CONV_LINK_TIP_SHIFT)
    tip:Show()
    counters.plainTips = counters.plainTips + 1
    return "plain"
end

---------------------------------------------------------------------------- hyperlinks in a frame
local waiting = {}              -- frames made during combat: hyperlinks enabled when it ends
local events = CreateFrame("Frame")

local function EnableNow(frame)
    if InCombatLockdown() then
        if not waiting[frame] then
            waiting[frame] = true
            counters.deferred = counters.deferred + 1
        end
        events:RegisterEvent("PLAYER_REGEN_ENABLED")
        return false
    end
    waiting[frame] = nil
    return pcall(frame.SetHyperlinksEnabled, frame, true)
end

events:SetScript("OnEvent", function()
    for frame in pairs(waiting) do
        EnableNow(frame)
    end
    if next(waiting) == nil then
        events:UnregisterEvent("PLAYER_REGEN_ENABLED")
    end
end)

-- Makes the hyperlinks of frame's font strings work like the chat's (Links.Click, Links.ShowTooltip).
-- False when they are not enabled yet (combat: they are when it ends) or the client lacks the API.
function Links.EnableHyperlinks(frame)
    if type(frame.SetHyperlinksEnabled) ~= "function" then
        return false
    end
    frame:EnableMouse(true)
    frame:SetScript("OnHyperlinkClick", function(_, link, text, button)
        Links.Click(link, text, button)
    end)
    frame:SetScript("OnHyperlinkEnter", function(self, link, text)
        Links.ShowTooltip(self, link, text)
    end)
    frame:SetScript("OnHyperlinkLeave", GameTooltip_Hide)
    return EnableNow(frame)
end

-- Counts only (for /tpr debug and the tests): never a link or a text.
function Links.Counters()
    local c = {}
    for key, value in pairs(counters) do
        c[key] = value
    end
    return c
end
