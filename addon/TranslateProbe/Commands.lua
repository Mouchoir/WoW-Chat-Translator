-- Slash commands: /tprobe (alias /tpr). /tpr alone opens the options panel (Options.lua).
local _, ns = ...
local L = ns.L

local TEST_ID_BASE = 900000000   -- lineIDs of the addon's own test lines
local testCount = 0

local function Yes(value)
    return value and L.YES or L.NO
end

local function TargetName(code)
    return ns.Lang.NAMES[code] or code
end

local function NextTestId()
    testCount = testCount + 1
    return TEST_ID_BASE + testCount
end

local function SendTest()
    local nonce = math.random(1, 65535)
    ns.Bridge.Send(ns.Bridge.TestMessage(nonce))
    ns.Printf(L.TEST_SENT, nonce)
end

local function PrintInfo()
    ns.Printf(L.INFO_VERSION, ns.VERSION, ns.ADDON_BUILD, ns.sessionId)
    local version, build, _, interface = GetBuildInfo()
    ns.Printf(L.INFO_CLIENT, tostring(version), tostring(build), tostring(interface), tostring(WOW_PROJECT_ID))
    ns.Printf(L.INFO_TARGET, TargetName(ns.db.target), ns.Bridge.ModeName())
    local okSize, w, h = pcall(GetPhysicalScreenSize)
    local okScale, scale = pcall(UIParent.GetEffectiveScale, UIParent)
    ns.Printf(L.INFO_SCREEN, okSize and tostring(w) or "?", okSize and tostring(h) or "?",
        okScale and ("%.3f"):format(scale) or "?")
    ns.Printf(L.INFO_REQUESTS, ns.Request.Count(), ns.Reserve.SlotsInUse())
    ChatFrameUtil.ForEachChatFrame(function(frame)
        local okName, name = pcall(frame.GetName, frame)
        ns.Printf(L.INFO_FRAME, okName and tostring(name) or "?", Yes(frame.TransformMessages ~= nil),
            Yes(frame.FontStringContainer ~= nil))
    end)
end

local function Help()
    ns.Print(L.HELP_HEADER)
    for _, key in ipairs({ "HELP_OPTIONS", "HELP_INFO", "HELP_TARGET", "HELP_NAMES", "HELP_SHOW", "HELP_MODE",
        "RPL_HELP_CONV", "RPL_HELP_DISPLAY", "RPL_HELP_CLIP", "HELP_TRANSLIT", "HELP_GLOSS", "HELP_TEST",
        "HELP_TEST_ROWS", "HELP_TEST_LOCKDOWN", "RPL_HELP_TEST_PASTE", "HELP_CLEAR", "HELP_APP", "HELP_DEBUG",
        "HELP_HELP", "HELP_CLICK" }) do
        ns.Print(L[key])
    end
end

-- /tpr and /tpr options: the options panel, or the command list when the panel is missing.
local function OpenOptions()
    if not ns.Options.Open() then
        ns.Print(L.OPTIONS_NA)
        Help()
    end
end

local function GlossHelp()
    ns.Printf(L.GLOSS_COUNT, ns.Glossary.Count())
    for _, key in ipairs({ "GLOSS_HELP_ADD", "GLOSS_HELP_DEL", "GLOSS_HELP_LIST", "GLOSS_HELP_TEST" }) do
        ns.Print(L[key])
    end
end

-- Upper case for display; string.upper only knows ASCII, so accented keys stay as typed.
local function Label(key)
    return key:find("[\128-\255]") and key or key:upper()
end

-- /tpr gloss add|del|list|test ... Meanings are stored for the current target language (the
-- options panel stores the same way, through the same Glossary functions).
local function Gloss(arg)
    local action, rest = arg:match("^(%S*)%s*(.-)$")
    action = action:lower()
    local custom = ns.db.glossary
    local target = ns.db.target
    if action == "add" then
        local key, text = rest:match("^(" .. ns.Glossary.KEY_PATTERN .. ")%s+(.-)$")
        if not key or ns.Glossary.CleanMeaning(text) == "" then
            ns.Print(L.GLOSS_HELP_ADD)
            return
        end
        key = key:lower()
        local stored = ns.Glossary.SetMeaning(key, target, text) or ns.Glossary.BuiltinMeaning(key, target) or ""
        ns.Printf(L.GLOSS_ADDED, Label(key), stored)
        ns.Options.Changed("glossary")
    elseif action == "del" then
        local key = rest:match("^(" .. ns.Glossary.KEY_PATTERN .. ")$")
        if not key then
            ns.Print(L.GLOSS_HELP_DEL)
            return
        end
        key = key:lower()
        ns.Glossary.Remove(key)
        ns.Printf(L.GLOSS_REMOVED, Label(key))
        ns.Options.Changed("glossary")
    elseif action == "list" then
        local keys = {}
        for key in pairs(custom) do
            keys[#keys + 1] = key
        end
        table.sort(keys)
        ns.Printf(L.GLOSS_COUNT, ns.Glossary.Count())
        if #keys == 0 then
            ns.Print(L.GLOSS_NO_CUSTOM)
        end
        for _, key in ipairs(keys) do
            local value = custom[key]
            -- a built-in key without a user meaning for this target sends its built-in one
            local shown = value == false and L.GLOSS_DISABLED or ns.Glossary.UserMeaning(value, target)
                or ns.Glossary.BuiltinMeaning(key, target) or L.GLOSS_NO_MEANING:format(TargetName(target))
            ns.Printf(L.GLOSS_ENTRY, Label(key), shown)
        end
    elseif action == "test" and rest ~= "" then
        ns.Print(ns.Glossary.Describe(rest, ns.db.target) or L.SUFFIX_NO_GLOSS)
    else
        GlossHelp()
    end
end

-- /tpr test: the whole pipeline on a sample line (TEST toast, then a real request).
local function TestPipeline()
    SendTest()
    local id = NextTestId()
    local sample = ns.db.target == "en" and L.TEST_SAMPLE_FR or L.TEST_SAMPLE_EN
    DEFAULT_CHAT_FRAME:AddMessage(sample .. ns.ChatButton.ButtonLink(id))
    ns.ChatButton.Track(id, sample, nil)
    ns.Request.Start(id, DEFAULT_CHAT_FRAME, true)
end

local function PrintHeight(label, chatFrame, text, width)
    local ok, _, height, lines = pcall(ns.Reserve.Measure, chatFrame, text, width)
    if ok then
        ns.Printf(L.TEST_ROWS_HEIGHT, label, height, lines)
    else
        ns.Printf(L.TEST_ROWS_HEIGHT_NA, label)
    end
end

-- /tpr test rows: the row layout check (G2). No TRANSLATE is sent; the helper writes
-- "Overlay OK" in rows that carry the test slot.
local function TestRows()
    local chatFrame = DEFAULT_CHAT_FRAME
    local m = ns.Reserve.Metrics(chatFrame)
    if not m then
        ns.Print(L.TEST_ROWS_NA)
        return
    end
    ns.Printf(L.TEST_ROWS_METRICS, m.ppu, m.fontH, m.W, m.H, m.plateW, m.cw)
    local oneRow = ns.ChatButton.SuffixLinks(0, ns.Reserve.Rows(m, ns.Bridge.TEST_SLOT, 1))
    PrintHeight("a", chatFrame, "a", m.cw)
    PrintHeight("a|n ", chatFrame, "a|n ", m.cw)
    PrintHeight(L.TEST_ROWS_ONE_ROW, chatFrame, "a" .. oneRow, m.cw)
    local id = NextTestId()
    DEFAULT_CHAT_FRAME:AddMessage(L.TEST_ROWS_LINE .. ns.ChatButton.ButtonLink(id))
    ns.ChatButton.Track(id, L.TEST_ROWS_LINE, nil)
    local n = ns.ChatButton.SetSuffix(id, function(frame)
        local fm = ns.Reserve.Metrics(frame)
        if not (fm and fm.ok) then
            return nil
        end
        return ns.Reserve.Rows(fm, ns.Bridge.TEST_SLOT, 3)
    end)
    ns.Reserve.SyncView(true)
    ns.Printf(L.TEST_ROWS_SHOWN, n)
end

local function SecretState(fn)
    local results = { pcall(fn) }
    if not results[1] or #results < 2 then
        return L.TEST_NA
    end
    for i = 2, #results do
        if issecretvalue(results[i]) then
            return L.YES
        end
    end
    return L.NO
end

-- /tpr test lockdown: what is readable right now (G4), then a TEST message.
local function TestLockdown()
    ns.Printf(L.TEST_LOCKDOWN_STATE, Yes(C_ChatInfo.InChatMessagingLockdown()), Yes(ns.Restriction.Active()))
    local chatFrame = DEFAULT_CHAT_FRAME
    local container = chatFrame and chatFrame.FontStringContainer
    ns.Printf(L.TEST_LOCKDOWN_RECT, container and SecretState(function()
        return container:GetScaledRect()
    end) or L.TEST_NA)
    local line = chatFrame and chatFrame.visibleLines and chatFrame.visibleLines[2]
    ns.Printf(L.TEST_LOCKDOWN_LINE, line and SecretState(function()
        return line:GetTop()
    end) or L.TEST_NA)
    SendTest()
end

local MODES = { auto = true, dense = true, robust = true, mono = true }

-- Forgets every conversation (open and closed), the drafts, the pending reply (CANCEL) and the
-- sends waiting for their echo. Memory only: nothing of it was ever saved. silent = true right
-- after a CLEAR (the Helper forgot every card): window slots are freed without a CANCEL each.
local function ClearConversations(silent)
    if ns.Composer and ns.Composer.Reset then
        ns.Composer.Reset()
    end
    if ns.Channels and ns.Channels.Clear then
        ns.Channels.Clear()
    end
    if ns.Conv and type(ns.Conv.Clear) == "function" then
        ns.Conv.Clear(silent)
    end
end

-- /tpr conv [clear|reset]
local function Conversation(arg)
    arg = arg:lower()
    if arg == "clear" then
        ClearConversations(false)
        ns.Print(L.RPL_CONV_CLEARED)
    elseif arg == "reset" then
        ns.Options.ResetConvPosition()
        ns.Print(L.RPL_CONV_RESET)
    elseif ns.ConvFrame and type(ns.ConvFrame.Toggle) == "function" then
        ns.ConvFrame.Toggle()
    else
        ns.Print(L.RPL_CONV_NA)
    end
end

-- /tpr clip ask|allow|never and /tpr display window|inline: set when given, then print.
local function Choice(key, arg, help)
    arg = arg:lower()
    if arg ~= "" then
        if ns.Options.SetChoice(key, arg) then
            ns.Options.Changed(key)
        else
            ns.Print(help)
        end
    end
end

local function OnOff(arg, key, onText, offText)
    arg = arg:lower()
    if arg == "on" or arg == "off" then
        ns.db[key] = arg == "on"
        ns.Options.Changed(key)
    end
    ns.Print(ns.db[key] and onText or offText)
end

local handlers = {
    [""] = OpenOptions,
    options = OpenOptions,
    info = PrintInfo,
    help = Help,
    target = function(arg)
        arg = arg:lower()
        for _, t in ipairs(ns.Lang.TARGETS) do
            if t:lower() == arg then
                ns.Options.SetTarget(t)
                ns.Options.Changed("target")
            end
        end
        ns.Printf(L.TARGET_IS, TargetName(ns.db.target), table.concat(ns.Lang.TARGETS, ", "))
    end,
    names = function(arg)
        OnOff(arg, "names", L.NAMES_ON, L.NAMES_OFF)
    end,
    show = function(arg)
        OnOff(arg, "glossShow", L.SHOW_ON, L.SHOW_OFF)
    end,
    mode = function(arg)
        arg = arg:lower()
        if MODES[arg] then
            ns.Options.SetMode(arg)
            ns.Options.Changed("mode")
        else
            ns.Print(L.HELP_MODE)
        end
        ns.Printf(L.MODE_SET, ns.db.mode, ns.Bridge.ModeName())
    end,
    translit = function(arg)
        if arg == "" then
            ns.Print(L.HELP_TRANSLIT)
            return
        end
        ns.Print(ns.Glossary.Romanize(arg) or L.TRANSLIT_NA)
    end,
    gloss = Gloss,
    test = function(arg)
        arg = arg:lower()
        if arg == "rows" then
            TestRows()
        elseif arg == "lockdown" then
            TestLockdown()
        elseif arg == "paste" then
            ns.Print(ns.Composer.ToggleProbe() and L.RPL_PROBE_ON or L.RPL_PROBE_OFF)
        else
            TestPipeline()
        end
    end,
    clear = function()
        ns.Request.ClearAll()
        ClearConversations(true)
        ns.Print(L.CLEARED)
    end,
    conv = Conversation,
    clip = function(arg)
        Choice("replyClipboard", arg, L.RPL_HELP_CLIP)
        ns.Print(L["RPL_CLIP_" .. ns.db.replyClipboard:upper() .. "_IS"])
    end,
    display = function(arg)
        Choice("display", arg, L.RPL_HELP_DISPLAY)
        ns.Print(ns.db.display == "window" and L.RPL_DISPLAY_WINDOW or L.RPL_DISPLAY_INLINE)
    end,
    app = function()
        for _, key in ipairs({ "APP_1", "APP_2", "APP_3", "APP_4" }) do
            ns.Print(L[key])
        end
        ns.Printf(L.APP_DOWNLOAD, ns.COMPANION_URL)
    end,
    debug = function()
        local r, s, x = ns.Request.Counters(), ns.Reserve.Counters(), ns.Restriction.Counters()
        ns.Printf(L.DEBUG_REQUESTS, r.active, r.clicks, r.sent, r.resent, r.restarted, r.debounced, r.removed,
            r.expired, r.dropped, r.tooOld)
        ns.Printf(L.DEBUG_SLOTS, s.inUse, s.free, s.scans, s.scanFreed, s.viewsSent, s.stripped)
        ns.Printf(L.DEBUG_BRIDGE, ns.Bridge.ModeName(), Yes(ns.Bridge.IsBusy()), Yes(ns.Reserve.IsWatching()))
        ns.Printf(L.DEBUG_RESTRICTION, Yes(x.active), x.secret, x.dropped, x.waiting, x.batches, x.reprinted,
            x.deferred, x.ran)
        local c, ch = ns.Composer.Counters(), ns.Channels.Counters()
        ns.Printf(L.RPL_DEBUG, Yes(c.pending), ch.sent, ch.confirmed, ch.unconfirmed, ch.offline, ch.blocked,
            ch.refused + c.refused, c.pasted, c.warned)
    end,
}

SLASH_TRANSLATEPROBE1 = "/tprobe"
SLASH_TRANSLATEPROBE2 = "/tpr"
SlashCmdList.TRANSLATEPROBE = function(input)
    local command, arg = (input or ""):match("^%s*(%S*)%s*(.-)%s*$")
    local handler = handlers[command:lower()]
    if handler then
        handler(arg)
    else
        Help()
    end
end
