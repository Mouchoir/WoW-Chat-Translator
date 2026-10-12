-- Namespace, saved variables and print helpers.
local addonName, ns = ...
local L = ns.L

ns.name = addonName
-- Button links carry this stamp, so a line replayed after a /reload never matches a new lineID.
ns.session = tostring(time())
ns.inits = {}

local DEFAULTS = {
    budgetMs = 4,     -- Lua time per frame for background work, before the script budget cap
    logging = true,   -- record whether incoming chat text is readable
    simulate = true,  -- a [T] click also times a simulated model pass
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

-- Appends a row to a list and drops the oldest rows past max.
function ns.PushCapped(list, row, max)
    list[#list + 1] = row
    while #list > max do
        table.remove(list, 1)
    end
end

local function ApplyDefaults(db)
    for key, value in pairs(DEFAULTS) do
        if db[key] == nil then
            db[key] = value
        end
    end
    db.log = db.log or {}                 -- recent chat readability samples (no message text)
    db.stats = db.stats or {}             -- readable/secret counts per context
    db.transitions = db.transitions or {} -- addon restriction state changes
    db.catchup = db.catchup or {}         -- re-reads of secret lines after a lockdown
    db.results = db.results or {}         -- bench, model and click timings
end

local loader = CreateFrame("Frame")
loader:RegisterEvent("ADDON_LOADED")
loader:SetScript("OnEvent", function(self, _, name)
    if name ~= addonName then
        return
    end
    self:UnregisterEvent("ADDON_LOADED")
    TranslateProbeDB = TranslateProbeDB or {}
    ApplyDefaults(TranslateProbeDB)
    ns.db = TranslateProbeDB
    for _, init in ipairs(ns.inits) do
        init()
    end
    ns.Print(L.LOADED)
end)
