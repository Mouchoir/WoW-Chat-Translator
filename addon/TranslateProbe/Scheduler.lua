-- Runs heavy work in coroutines, a few milliseconds per frame, inside the client's per-addon
-- script budget. The client exposes only the budget's limits (GetScriptBucketThrottleLimits),
-- never its level, so the scheduler keeps its own "debt": Lua time spent and not yet refilled.
-- Each frame the debt shrinks at a share of the per-second allowance; a slice may run while the
-- debt stays under a share of the burst allowance. That gives a head start, then a steady pace.
--
-- Speed "safe" (default) always uses the Restricted limits, which hold whatever state the
-- client is in. Speed "fast" uses the Normal limits while no addon restriction is active (open
-- world out of combat); whether the client really grants them there is unverified, so a
-- LUA_WARNING about our execution limit switches back to safe for the session.
local addonName, ns = ...

local Scheduler = {}
ns.Scheduler = Scheduler

local create, resume, status, yield = coroutine.create, coroutine.resume, coroutine.status, coroutine.yield

local RATE_SHARE = 0.9      -- share of the per-second allowance used for refills
local BURST_SHARE = 0.5     -- share of the burst allowance the debt may reach (Restricted)
local FAST_BURST_MS = 5000  -- debt ceiling with Normal limits (a quarter of their 20 s burst)
local MIN_SLICE_MS = 0.5
local FALLBACK = {          -- values measured on Forever 1.60.1 if the getter is missing
    luaScriptBucketThrottleMaxMsPerSecondNormal = 800, luaScriptBucketThrottleMaxMsBurstNormal = 20000,
    luaScriptBucketThrottleMaxMsPerSecondRestricted = 500, luaScriptBucketThrottleMaxMsBurstRestricted = 1000,
}
Scheduler.SPEEDS = { safe = true, fast = true }

local queue = {}
local current
local sliceStart, sliceBudget, inSlice = 0, 4, false
local debt = 0
local throttled = false     -- a LUA_WARNING named us: safe limits for the rest of the session
local limitsCache, limitsAt = nil, -math.huge

-- Returns the ScriptBucketThrottleLimits table, or nil on clients without it.
function Scheduler.GetBucketLimits()
    if type(GetScriptBucketThrottleLimits) ~= "function" then
        return nil
    end
    local ok, limits = pcall(GetScriptBucketThrottleLimits)
    if ok and type(limits) == "table" then
        return limits
    end
end

local function Limits()
    local now = GetTime()
    if now - limitsAt > 10 then
        limitsCache, limitsAt = Scheduler.GetBucketLimits() or FALLBACK, now
    end
    return limitsCache
end

local function Positive(value, fallback)
    return (type(value) == "number" and value > 0) and value or fallback
end

-- True while any addon restriction is starting or active (combat, encounter, map, chat...).
local function Restricted()
    if InCombatLockdown() then
        return true
    end
    if not (C_RestrictedActions and C_RestrictedActions.GetAddOnRestrictionState and Enum.AddOnRestrictionType) then
        return false
    end
    for _, value in pairs(Enum.AddOnRestrictionType) do
        local ok, state = pcall(C_RestrictedActions.GetAddOnRestrictionState, value)
        if ok and state ~= nil and state ~= Enum.AddOnRestrictionState.Inactive then
            return true
        end
    end
    return false
end

-- Refill per second and debt ceiling (ms) for the current speed and client state.
function Scheduler.Pace()
    local l = Limits()
    local fast = ns.db and ns.db.speed == "fast" and not throttled and not Restricted()
    if fast then
        local perSecond = Positive(l.luaScriptBucketThrottleMaxMsPerSecondNormal, FALLBACK.luaScriptBucketThrottleMaxMsPerSecondNormal)
        local burst = Positive(l.luaScriptBucketThrottleMaxMsBurstNormal, FALLBACK.luaScriptBucketThrottleMaxMsBurstNormal)
        return RATE_SHARE * perSecond, math.min(FAST_BURST_MS, 0.25 * burst), "fast"
    end
    local perSecond = Positive(l.luaScriptBucketThrottleMaxMsPerSecondRestricted, FALLBACK.luaScriptBucketThrottleMaxMsPerSecondRestricted)
    local burst = Positive(l.luaScriptBucketThrottleMaxMsBurstRestricted, FALLBACK.luaScriptBucketThrottleMaxMsBurstRestricted)
    return RATE_SHARE * perSecond, BURST_SHARE * burst, "safe"
end

-- Slice length for the next frame, 0 when the debt leaves no room.
function Scheduler.Plan()
    local _, ceiling = Scheduler.Pace()
    local maxSlice = (ns.db and ns.db.budgetMs) or 8
    local room = math.min(maxSlice, ceiling - debt)
    if room < MIN_SLICE_MS then
        return 0, debt
    end
    return room, debt
end

function Scheduler.EffectiveBudget()
    return (Scheduler.Plan())
end

function Scheduler.IsThrottled()
    return throttled
end

-- Called by job code between small units of work. Yields once the slice is spent.
-- Never call it from inside pcall, string.gsub callbacks or chat frame callbacks.
function Scheduler.Check()
    if inSlice and debugprofilestop() - sliceStart >= sliceBudget then
        yield()
    end
end

function Scheduler.IsBusy()
    return current ~= nil or #queue > 0
end

local driver = CreateFrame("Frame")
driver:Hide()

-- job = { run = function(job), start = function(job) (optional), done = function(job, ok, err) }.
-- job.stats is filled in; on failure job.err (and job.stack when available) are set.
function Scheduler.Enqueue(job)
    job.stats = {
        frames = 0, activeFrames = 0, busyMs = 0, maxSliceMs = 0, pausedMs = 0, pausedFrames = 0,
        minBudgetMs = math.huge, maxBudgetMs = 0, maxEvery = 1, enqueuedAt = debugprofilestop(),
    }
    queue[#queue + 1] = job
    driver:Show()
end

local function Finish(job, ok, err)
    current = nil
    local s = job.stats
    s.wallMs = debugprofilestop() - s.startedAt - s.pausedMs
    s.avgFps = s.wallMs > 0 and s.activeFrames / (s.wallMs / 1000) or 0
    if s.minBudgetMs == math.huge then
        s.minBudgetMs = 0
    end
    s.pace = select(3, Scheduler.Pace())
    if not ok then
        job.err = tostring(err)
        local okStack, stack = pcall(debugstack, job.co)
        job.stack = okStack and stack or nil
    end
    if job.done then
        local okDone, doneErr = pcall(job.done, job, ok, job.err)
        if not okDone then
            geterrorhandler()(doneErr)
        end
    end
    if not ok then
        geterrorhandler()(job.err)
    end
end

local function Start(job)
    job.co = create(function()
        job.run(job)
    end)
    job.stats.startedAt = debugprofilestop()
    job.stats.queuedMs = job.stats.startedAt - job.stats.enqueuedAt
    current = job
    if job.start then
        local ok, err = pcall(job.start, job)
        if not ok then
            geterrorhandler()(err)
        end
    end
end

driver:SetScript("OnUpdate", function(self, elapsed)
    local refill = Scheduler.Pace()
    debt = math.max(0, debt - refill * (elapsed or 0))
    if not current then
        local job = table.remove(queue, 1)
        if not job then
            if debt <= 0 then
                self:Hide()
            end
            return
        end
        Start(job)
    end
    local job, stats = current, current.stats

    -- Players need every frame in combat, and the old watchdog is strictest there.
    if InCombatLockdown() then
        stats.pausedMs = stats.pausedMs + (elapsed or 0) * 1000
        stats.pausedFrames = stats.pausedFrames + 1
        return
    end
    stats.activeFrames = stats.activeFrames + 1

    local budget = Scheduler.Plan()
    if budget <= 0 then
        return
    end
    sliceBudget = budget
    if budget < stats.minBudgetMs then stats.minBudgetMs = budget end
    if budget > stats.maxBudgetMs then stats.maxBudgetMs = budget end

    sliceStart = debugprofilestop()
    inSlice = true
    local ok, err = resume(job.co)
    inSlice = false
    local elapsedMs = debugprofilestop() - sliceStart
    debt = debt + elapsedMs

    stats.frames = stats.frames + 1
    stats.busyMs = stats.busyMs + elapsedMs
    if elapsedMs > stats.maxSliceMs then
        stats.maxSliceMs = elapsedMs
    end

    if not ok then
        Finish(job, false, err)
    elseif status(job.co) == "dead" then
        Finish(job, true)
    end
end)

-- The client reports script budget overruns as LUA_WARNING text naming the addon.
local watcher = CreateFrame("Frame")
watcher:RegisterEvent("LUA_WARNING")
watcher:SetScript("OnEvent", function(_, _, ...)
    local text
    for i = 1, select("#", ...) do
        local v = select(i, ...)
        if type(v) == "string" and not issecretvalue(v) then
            text = v
        end
    end
    if not text or not text:find(addonName, 1, true) then
        return
    end
    local limit = ERROR_SCRIPT_EXECUTION_LIMIT and ERROR_SCRIPT_EXECUTION_LIMIT:format(addonName)
    if text:find("exceeded execution limit", 1, true) or (limit and text:find(limit, 1, true)) then
        throttled = true
        local _, ceiling = Scheduler.Pace()
        debt = ceiling
        if ns.db then
            ns.PushCapped(ns.db.results.throttles, { t = time(), speed = ns.db.speed, text = text:sub(1, 200) }, 20)
            ns.Print(ns.L.THROTTLED)
        end
    end
end)
