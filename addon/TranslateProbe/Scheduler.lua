-- Runs heavy work in coroutines, a few milliseconds per frame, so the game never stalls
-- and the addon stays well inside the client's per-addon script budget.
local _, ns = ...

local Scheduler = {}
ns.Scheduler = Scheduler

local create, resume, status, yield = coroutine.create, coroutine.resume, coroutine.status, coroutine.yield

-- Share of the client's per-second script allowance that background work may use.
local BUCKET_SHARE = 0.5
local MIN_SLICE_MS = 0.5
local BUCKET_KEYS = {
    "luaScriptBucketThrottleMaxMsPerSecondNormal",
    "luaScriptBucketThrottleMaxMsPerSecondRestricted",
}

local queue = {}
local current
local sliceStart = 0
local sliceBudget = 4
local inSlice = false

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

-- The user's setting, capped so that at the current frame rate background work uses at
-- most half of the smallest positive per-second allowance the client reports.
function Scheduler.EffectiveBudget()
    local budget = (ns.db and ns.db.budgetMs) or 4
    local limits = Scheduler.GetBucketLimits()
    if limits then
        local perSecond
        for _, key in ipairs(BUCKET_KEYS) do
            local value = limits[key]
            if type(value) == "number" and value > 0 and (not perSecond or value < perSecond) then
                perSecond = value
            end
        end
        if perSecond then
            local fps = math.max(GetFramerate() or 60, 20)
            budget = math.min(budget, BUCKET_SHARE * perSecond / fps)
        end
    end
    return math.max(budget, MIN_SLICE_MS)
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

-- Number of jobs that will run before a job enqueued now.
function Scheduler.Pending()
    return #queue + (current and 1 or 0)
end

local driver = CreateFrame("Frame")
driver:Hide()

-- job = { run = function(job), done = function(job, ok, err) } ; job.stats is filled in.
function Scheduler.Enqueue(job)
    job.stats = { frames = 0, busyMs = 0, maxSliceMs = 0, minBudgetMs = math.huge, maxBudgetMs = 0 }
    queue[#queue + 1] = job
    driver:Show()
end

local function Finish(job, ok, err)
    current = nil
    job.stats.wallMs = debugprofilestop() - job.stats.startedAt
    if job.done then
        local okDone, doneErr = pcall(job.done, job, ok, err)
        if not okDone then
            geterrorhandler()(doneErr)
        end
    end
    if not ok then
        geterrorhandler()(err)
    end
end

driver:SetScript("OnUpdate", function(self)
    if not current then
        local job = table.remove(queue, 1)
        if not job then
            self:Hide()
            return
        end
        job.co = create(function()
            job.run(job)
        end)
        job.stats.startedAt = debugprofilestop()
        current = job
    end

    -- Players need every frame in combat, and the old watchdog is strictest there.
    if InCombatLockdown() then
        return
    end

    local job, stats = current, current.stats
    sliceBudget = Scheduler.EffectiveBudget()
    if sliceBudget < stats.minBudgetMs then stats.minBudgetMs = sliceBudget end
    if sliceBudget > stats.maxBudgetMs then stats.maxBudgetMs = sliceBudget end

    sliceStart = debugprofilestop()
    inSlice = true
    local ok, err = resume(job.co)
    inSlice = false
    local elapsed = debugprofilestop() - sliceStart

    stats.frames = stats.frames + 1
    stats.busyMs = stats.busyMs + elapsed
    if elapsed > stats.maxSliceMs then
        stats.maxSliceMs = elapsed
    end

    if not ok then
        Finish(job, false, err)
    elseif status(job.co) == "dead" then
        Finish(job, true)
    end
end)
