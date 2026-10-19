-- Is Translate Companion there? (user request of 2026-10-19: never offer to start or download it
-- while it runs; a [T] click without it opens only a popup in the centre of the screen.)
--
-- Nothing ever comes back from Translate Companion while the game runs, but files inside the
-- addon's folder are read at login and at /reload: Translate Companion writes the deployed copy of
-- CompanionStatus.lua (TranslateProbe_CompanionStatus, a table of numbers and short strings) at its
-- start, every 60 s while it runs and on a clean exit. At ADDON_LOADED the presence is:
--   "running"    state "running" and a heartbeat at most 150 s away from time()
--   "installed"  the global exists otherwise (stopped, stale heartbeat, malformed)
--   "absent"     the global is nil (the file as the addon ships it)
-- The session is "confirmed" (memory only) when it said running at load, or once the user clicks
-- Continue in the popup. Every action that asks for a translation ([T] in both modes, Shift too,
-- a bubble's Translate button and click, the reply's Translate) goes through Presence.Ensure: when
-- confirmed it runs at once; otherwise only the popup opens (no window, no rows, no request, no
-- chat line), the action waits, and Continue runs it. The popup shows a small card of slot 125
-- that a running Translate Companion paints "detected" over (CompanionDialog.lua), so a Companion
-- started after the login is seen without a /reload.
local _, ns = ...

local Presence = {}
ns.Presence = Presence

local FRESH_SECONDS = 150       -- a heartbeat every 60 s: one missed write still counts as running
local MAX_TEXT = 32             -- version strings longer than this are ignored
Presence.FRESH_SECONDS = FRESH_SECONDS

local state = "absent"
local info = {}                 -- { version, build, heartbeat, age } read from the status file
local confirmed = false
local pending                   -- the action waiting for Continue (the latest click wins)
local counters = { asked = 0, ranAtOnce = 0, continued = 0, dropped = 0 }

local function IsNumber(v)
    return type(v) == "number" and v == v and v ~= math.huge and v ~= -math.huge
end

-- Reads one field of the status table without any metamethod.
local function Field(t, key)
    local ok, v = pcall(rawget, t, key)
    if ok then
        return v
    end
end

-- The presence a status value means at time now: "running", "installed" or "absent", and what
-- was readable of it.
function Presence.Classify(status, now)
    if status == nil then
        return "absent", {}
    end
    if type(status) ~= "table" then
        return "installed", {}
    end
    local got = {}
    local version, build, heartbeat = Field(status, "version"), Field(status, "build"), Field(status, "heartbeat")
    if type(version) == "string" and #version <= MAX_TEXT and not version:find("[^%w%.%-%+]") then
        got.version = version
    end
    if IsNumber(build) then
        got.build = math.floor(build)
    end
    if IsNumber(heartbeat) and IsNumber(now) then
        got.heartbeat = heartbeat
        got.age = now - heartbeat
    end
    if Field(status, "state") == "running" and got.age and math.abs(got.age) <= FRESH_SECONDS then
        return "running", got
    end
    return "installed", got
end

-- Reads TranslateProbe_CompanionStatus (ADDON_LOADED, and tests): the presence, and a session that
-- starts confirmed when Translate Companion said it runs.
function Presence.Load()
    local okTime, now = pcall(time)
    state, info = Presence.Classify(_G.TranslateProbe_CompanionStatus, okTime and now or nil)
    confirmed = state == "running"
    pending = nil
    return state
end

-- "running", "installed" or "absent" (as read at load).
function Presence.State()
    return state
end

-- { version, build, heartbeat, age } of the status file, as far as it was readable (a copy).
function Presence.Info()
    local copy = {}
    for k, v in pairs(info) do
        copy[k] = v
    end
    return copy
end

function Presence.IsConfirmed()
    return confirmed
end

-- Runs action at once when the session is confirmed; else keeps it (the latest replaces an older
-- one) and opens only the Translate Companion popup. Returns true when the action ran.
function Presence.Ensure(action)
    if confirmed then
        counters.ranAtOnce = counters.ranAtOnce + 1
        if action then
            action()
        end
        return true
    end
    counters.asked = counters.asked + 1
    pending = action
    ns.CompanionDialog.Open(state)
    return false
end

-- The popup's Continue: the session is confirmed and the waiting action runs, once.
function Presence.Continue()
    confirmed = true
    counters.continued = counters.continued + 1
    local action = pending
    pending = nil
    if action then
        action()
    end
end

-- The popup closed any other way (Cancel, Escape): the waiting action is dropped.
function Presence.Drop()
    if pending then
        counters.dropped = counters.dropped + 1
    end
    pending = nil
end

function Presence.HasPending()
    return pending ~= nil
end

function Presence.Counters()
    local c = {}
    for k, v in pairs(counters) do
        c[k] = v
    end
    c.state, c.confirmed = state, confirmed
    return c
end

ns.OnLoad(Presence.Load)
