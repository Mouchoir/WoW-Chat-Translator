-- Namespace, saved variables, session and print helpers.
--
-- TranslateProbeDB keeps settings only (whitelist below). Every other key, such as the
-- telemetry of the old Lua translator, is deleted on each load. Message text, sender names,
-- line ids, request ids and slots are never stored.
local addonName, ns = ...
local L = ns.L

ns.name = addonName
ns.VERSION = "0.6.0"                        -- keep in step with the TOC
-- Where Translate Companion (the Windows app, WoWTranslateCompanion.exe) is downloaded: the one
-- copy of the URL on the addon side (the dialog of CompanionDialog.lua and /tpr app show it).
ns.COMPANION_URL = "https://github.com/Mouchoir/WoW-Chat-Translator/releases"
ns.ADDON_BUILD = 5                          -- STATUS.addonBuild, bump on protocol-visible changes
                                            -- (2: TRANSLATE rows 1..6, 2026-10-18; 3: TRANSLATE
                                            -- reply and copy flags, 2026-10-19; 4: a hint lane
                                            -- under every card, 2026-10-19, which Translate
                                            -- Companion 0.4.0 covers: an older one shows "older
                                            -- than the addon"; 5: the link colours, a COLOURS
                                            -- message right before the TRANSLATE, 2026-10-19)
ns.sessionId = math.random(1, 65535)        -- u16 for the bridge, new on every UI load
ns.session = tostring(ns.sessionId)         -- string used in links (link clicks compare strings)
ns.inits = {}

local KEEP = {
    schema = true, target = true, names = true, glossShow = true, mode = true, glossary = true, helpShown = true,
    -- conversation window and reply settings (docs\ADDON.md, "Settings of the conversation window")
    display = true, follow = true, replyClipboard = true, convOriginal = true, convText = true, convScale = true,
    convCombat = true, convFocus = true, convOpenWhisper = true, convPos = true, convTipShown = true,
}
local MODES = { auto = true, dense = true, robust = true, mono = true }
local MAX_MEANING = 120
ns.MAX_MEANING = MAX_MEANING
local SCHEMA = 4

-- Settings with a fixed set of values: the first one is the default.
local CHOICES = {
    display = { "window", "inline" },               -- what a [T] click opens; an old "tab" is purged
    follow = { "smart", "off", "add", "translate" }, -- later lines of a conversation's sender
    replyClipboard = { "ask", "allow", "never" },    -- in-game consent to copy translated replies
    convCombat = { "keep", "hide" },                 -- the window during combat
}
ns.CHOICES = CHOICES
-- Numeric settings: { min, max, default }; values are rounded and clamped.
local RANGES = { convText = { 12, 20, 14 }, convScale = { 80, 150, 100 } }
ns.RANGES = RANGES
-- Boolean settings and their defaults.
local FLAGS = { names = false, glossShow = false, convOriginal = false, convFocus = false, convOpenWhisper = false }
local POINTS = {
    TOPLEFT = true, TOP = true, TOPRIGHT = true, LEFT = true, CENTER = true, RIGHT = true, BOTTOMLEFT = true,
    BOTTOM = true, BOTTOMRIGHT = true,
}

function ns.Print(text)
    print("|cff66bbff" .. L.PREFIX .. "|r " .. text)
end

function ns.Printf(fmt, ...)
    ns.Print(fmt:format(...))
end

-- Runs fn once the saved variables are ready.
function ns.OnLoad(fn)
    ns.inits[#ns.inits + 1] = fn
end

local function IsMeaning(value)
    return type(value) == "string" and #value <= MAX_MEANING
end

-- A user glossary value: false (entry disabled), a string (one meaning for every target, the
-- format before per-target meanings), or { [target or "*"] = string } (meanings per target; "*"
-- is the every-target meaning an old string becomes when one target gets its own, and "" for a
-- target hides it there). Invalid meanings inside a table are dropped, the table is kept.
-- Returns the value to keep, or nil.
local function CleanGlossaryValue(value)
    if value == false or IsMeaning(value) then
        return value
    end
    if type(value) ~= "table" then
        return nil
    end
    for target, meaning in pairs(value) do
        local valid = (target == "*" or ns.Lang.IsTarget(target)) and IsMeaning(meaning)
        if not valid or (meaning == "" and (target == "*" or value["*"] == nil)) then
            value[target] = nil
        end
    end
    return value
end

local function IsNumber(v)
    return type(v) == "number" and v == v and v ~= math.huge and v ~= -math.huge
end

-- The saved window position: { p = anchor point, x, y, w, h } (numbers and a point name only),
-- or nil.
local function CleanPos(pos)
    if type(pos) ~= "table" or not POINTS[pos.p] then
        return nil
    end
    for _, k in ipairs({ "x", "y", "w", "h" }) do
        if not IsNumber(pos[k]) then
            return nil
        end
    end
    if pos.w <= 0 or pos.h <= 0 then
        return nil
    end
    return { p = pos.p, x = pos.x, y = pos.y, w = pos.w, h = pos.h }
end

-- A choice setting is valid when it is one of its values.
function ns.IsChoice(key, value)
    for _, v in ipairs(CHOICES[key] or {}) do
        if v == value then
            return true
        end
    end
    return false
end

-- A numeric setting rounded and clamped to its range, or nil when value is not a number.
function ns.ClampSetting(key, value)
    local r = RANGES[key]
    if not (r and IsNumber(value)) then
        return nil
    end
    return math.max(r[1], math.min(r[2], math.floor(value + 0.5)))
end

-- Whitelist purge and validation of the saved variables.
-- * Schema 3 (2026-10-18): glossary values may be tables of meanings per target. Older data stays
--   valid as it is, but the change is one way: a build before schema 3 drops every table entry
--   at load (docs\ADDON.md, decision 21).
-- * Schema 4 (2026-10-19): the conversation window settings, and the glossary row off by default
--   (user decision D11). Before schema 4 a saved glossShow = true cannot be told apart from the
--   old default, so it is turned off once; false was always the user's own choice and stays.
local function Purge(db)
    for key in pairs(db) do
        if not KEEP[key] then
            db[key] = nil
        end
    end
    local schema = IsNumber(db.schema) and db.schema or 0
    if not ns.Lang.IsTarget(db.target) then
        db.target = ns.Lang.DefaultTarget()
    end
    if not MODES[db.mode] then
        db.mode = "auto"
    end
    if schema < 4 and db.glossShow == true then
        db.glossShow = false
    end
    for key, default in pairs(FLAGS) do
        if type(db[key]) ~= "boolean" then
            db[key] = default
        end
    end
    for key, values in pairs(CHOICES) do
        if not ns.IsChoice(key, db[key]) then
            db[key] = values[1]
        end
    end
    for key, r in pairs(RANGES) do
        db[key] = ns.ClampSetting(key, db[key]) or r[3]
    end
    db.convPos = CleanPos(db.convPos)
    if type(db.convTipShown) ~= "boolean" then
        db.convTipShown = nil
    end
    if type(db.helpShown) ~= "boolean" then
        db.helpShown = nil
    end
    local glossary = type(db.glossary) == "table" and db.glossary or {}
    local keyPattern = "^" .. ns.Glossary.KEY_PATTERN .. "$"
    for key, value in pairs(glossary) do
        local validKey = type(key) == "string" and key:find(keyPattern) ~= nil
        if not validKey or CleanGlossaryValue(value) == nil then
            glossary[key] = nil
        end
    end
    db.glossary = glossary
    db.schema = SCHEMA
end
ns.Purge = Purge   -- for tests: the same validation on any table

local loader = CreateFrame("Frame")
loader:RegisterEvent("ADDON_LOADED")
loader:SetScript("OnEvent", function(self, _, name)
    if name ~= addonName then
        return
    end
    self:UnregisterEvent("ADDON_LOADED")
    if type(TranslateProbeDB) ~= "table" then
        TranslateProbeDB = {}
    end
    Purge(TranslateProbeDB)
    ns.db = TranslateProbeDB
    for _, init in ipairs(ns.inits) do
        local ok, err = pcall(init)
        if not ok then
            ns.Printf(L.INIT_FAILED, tostring(err))
        end
    end
    ns.Print(L.LOADED)
end)
