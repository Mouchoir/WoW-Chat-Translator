-- Runs heavy work in coroutines, a few milliseconds per frame, so the game never stalls
-- and the addon stays well inside the client's per-addon script budget.
local _, ns = ...

local Scheduler = {}
ns.Scheduler = Scheduler

local create, resume, status, yield = coroutine.create, coroutine.resume, coroutine.status, coroutine.yield

-- Share of the client's script allowance (per second and per burst) background work may use.
local BUCKET_SHARE = 0.5
-- Below this a slice is dominated by the granularity of Check() (one matrix row).
local MIN_SLICE_MS = 0.5
local PER_SECOND_KEYS = { "luaScriptBucketThrottleMaxMsPerSecondNormal", "luaScriptBucketThrottleMaxMsPerSecondRestricted" }
local BURST_KEYS = { "luaScriptBucketThrottleMaxMsBurstNormal", "luaScriptBucketThrottleMaxMsBurstRestricted" }

local queue = {}
local current
local sliceStart = 0
local sliceBudget = 4
local inSlice = false
local framesSinceSlice = 0

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

local function SmallestPositive(limits, keys)
    local smallest
    for _, key in ipairs(keys) do
        local value = limits and limits[key]
        if type(value) == "number" and value > 0 and (not smallest or value < smallest) then
            smallest = value
        end
    end
    return smallest
end

-- Slice length in ms and how many frames apart slices run. The user's setting is capped so
-- that background work uses at most half of the smallest per-second and burst allowances.
-- When half the per-second allowance is less than MIN_SLICE_MS per frame, slices keep that
-- minimum length but skip frames, so the average stays under the cap.
function Scheduler.Plan()
    local budget = (ns.db and ns.db.budgetMs) or 4
    local limits = Scheduler.GetBucketLimits()
    local burst = SmallestPositive(limits, BURST_KEYS)
    if burst then
        budget = math.min(budget, BUCKET_SHARE * burst)
    end
    budget = math.max(budget, MIN_SLICE_MS)
    local every = 1
    local perSecond = SmallestPositive(limits, PER_SECOND_KEYS)
    if perSecond then
        local fps = math.max(GetFramerate() or 60, 20)
        local cap = BUCKET_SHARE * perSecond / fps
        if cap >= MIN_SLICE_MS then
            budget = math.min(budget, cap)
        else
            budget = MIN_SLICE_MS
            every = math.ceil(MIN_SLICE_MS / cap)
        end
    end
    return budget, every
end

function Scheduler.EffectiveBudget()
    return (Scheduler.Plan())
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
    framesSinceSlice = 0
    if job.start then
        local ok, err = pcall(job.start, job)
        if not ok then
            geterrorhandler()(err)
        end
    end
end

driver:SetScript("OnUpdate", function(self, elapsed)
    if not current then
        local job = table.remove(queue, 1)
        if not job then
            self:Hide()
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

    local budget, every = Scheduler.Plan()
    framesSinceSlice = framesSinceSlice + 1
    if framesSinceSlice < every then
        return
    end
    framesSinceSlice = 0
    sliceBudget = budget
    if budget < stats.minBudgetMs then stats.minBudgetMs = budget end
    if budget > stats.maxBudgetMs then stats.maxBudgetMs = budget end
    if every > stats.maxEvery then stats.maxEvery = every end

    sliceStart = debugprofilestop()
    inSlice = true
    local ok, err = resume(job.co)
    inSlice = false
    local elapsedMs = debugprofilestop() - sliceStart

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
