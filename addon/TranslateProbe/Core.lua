-- Namespace, saved variables, session and print helpers.
--
-- TranslateProbeDB keeps settings only (whitelist below). Every other key, such as the
-- telemetry of the old Lua translator, is deleted on each load. Message text, sender names,
-- line ids, request ids and slots are never stored.
local addonName, ns = ...
local L = ns.L

ns.name = addonName
ns.VERSION = "0.2.0"                        -- keep in step with the TOC
ns.ADDON_BUILD = 1                          -- STATUS.addonBuild, bump on protocol-visible changes
ns.sessionId = math.random(1, 65535)        -- u16 for the bridge, new on every UI load
ns.session = tostring(ns.sessionId)         -- string used in links (link clicks compare strings)
ns.inits = {}

local KEEP = {
    schema = true, target = true, names = true, glossShow = true, mode = true, glossary = true, helpShown = true,
}
local MODES = { auto = true, dense = true, robust = true, mono = true }
local MAX_MEANING = 120

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

local function IsTarget(code)
    for _, t in ipairs(ns.Lang.TARGETS) do
        if t == code then
            return true
        end
    end
    return false
end

-- Whitelist purge and validation of the saved variables (schema 2).
local function Purge(db)
    for key in pairs(db) do
        if not KEEP[key] then
            db[key] = nil
        end
    end
    if not IsTarget(db.target) then
        db.target = ns.Lang.DefaultTarget()
    end
    if not MODES[db.mode] then
        db.mode = "auto"
    end
    if type(db.names) ~= "boolean" then
        db.names = false
    end
    if type(db.glossShow) ~= "boolean" then
        db.glossShow = true
    end
    if type(db.helpShown) ~= "boolean" then
        db.helpShown = nil
    end
    local glossary = type(db.glossary) == "table" and db.glossary or {}
    local keyPattern = "^" .. ns.Glossary.KEY_PATTERN .. "$"
    for key, value in pairs(glossary) do
        local validKey = type(key) == "string" and key:find(keyPattern) ~= nil
        local validValue = value == false or (type(value) == "string" and #value <= MAX_MEANING)
        if not (validKey and validValue) then
            glossary[key] = nil
        end
    end
    db.glossary = glossary
    db.schema = 2
end

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
