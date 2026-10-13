-- Slash commands: /tprobe (alias /tpr).
local addonName, ns = ...
local L = ns.L

local function Yes(value)
    return value and L.YES or L.NO
end

local function Number(value)
    return type(value) == "number" and ("%g"):format(value) or "?"
end

local function PrintInfo()
    local version, build, _, interface = GetBuildInfo()
    ns.Printf(L.INFO_CLIENT, version, build, interface, tostring(WOW_PROJECT_ID))

    local limits = ns.Scheduler.GetBucketLimits()
    if limits then
        ns.Printf(L.INFO_BUCKET,
            Number(limits.luaScriptBucketThrottleMaxMsPerSecondNormal), Number(limits.luaScriptBucketThrottleMaxMsBurstNormal),
            Number(limits.luaScriptBucketThrottleMaxMsPerSecondRestricted), Number(limits.luaScriptBucketThrottleMaxMsBurstRestricted))
    else
        ns.Print(L.INFO_BUCKET_NA)
    end
    local refill, ceiling, pace = ns.Scheduler.Pace()
    local _, debt = ns.Scheduler.Plan()
    ns.Printf(L.INFO_SLICE, pace, ns.db.budgetMs, refill, ceiling, debt)
    if ns.Scheduler.IsThrottled() then
        ns.Print(L.THROTTLED)
    end

    local active = ns.LockdownLog.ActiveRestrictions()
    ns.Printf(L.INFO_LOCK, Yes(C_ChatInfo.InChatMessagingLockdown()), active ~= "" and active or L.NONE,
        ns.LockdownLog.Context())

    if C_AddOnProfiler and C_AddOnProfiler.IsEnabled and C_AddOnProfiler.IsEnabled() then
        local metric = Enum.AddOnProfilerMetric
        ns.Printf(L.INFO_CPU,
            C_AddOnProfiler.GetAddOnMetric(addonName, metric.RecentAverageTime),
            C_AddOnProfiler.GetAddOnMetric(addonName, metric.PeakTime),
            C_AddOnProfiler.GetAddOnMetric(addonName, metric.CountTimeOver5Ms))
    end

    UpdateAddOnMemoryUsage()
    local model = ns.Model.IsBuilt() and L.MODEL_LOADED:format(ns.Model.Bytes() / 1048576) or L.MODEL_NOT_LOADED
    ns.Printf(L.INFO_MEM, GetAddOnMemoryUsage(addonName) / 1024, model)
end

local function RunModel(pieces)
    local srcPieces, tgtPieces = pieces, ns.Model.TargetFor(pieces)
    local queued = ns.Model.QueueBuild(function(job, ok)
        if ok then
            local s = job.stats
            ns.Printf(L.MODEL_BUILT, ns.Model.Bytes() / 1048576, s.busyMs / 1000, s.wallMs / 1000, s.maxSliceMs)
        else
            ns.Printf(L.JOB_FAILED, job.err or "?")
        end
    end)
    if queued then
        ns.Printf(L.MODEL_BUILD, ns.Model.ExpectedBytes() / 1048576)
    end
    ns.Scheduler.Enqueue({
        run = function(job)
            job.macs = ns.Model.Forward(srcPieces, tgtPieces)
        end,
        done = function(job, ok)
            if not ok then
                ns.Printf(L.JOB_FAILED, job.err or "?")
                return
            end
            local s = job.stats
            ns.Printf(L.MODEL_RESULT, srcPieces, tgtPieces, job.macs / 1e6, s.busyMs / 1000, s.wallMs / 1000,
                s.frames, s.maxBudgetMs, s.maxSliceMs)
            ns.PushCapped(ns.db.results.models, {
                when = date("%Y-%m-%d %H:%M"), src = srcPieces, tgt = tgtPieces,
                shortlist = ns.Model.ShortlistRows(), macs = job.macs,
                busyMs = s.busyMs, wallMs = s.wallMs, queuedMs = s.queuedMs, pausedMs = s.pausedMs,
                frames = s.frames, maxSliceMs = s.maxSliceMs, minBudgetMs = s.minBudgetMs,
                budgetMs = s.maxBudgetMs, every = s.maxEvery, fps = s.avgFps,
                context = ns.LockdownLog.Context(), restrictions = ns.LockdownLog.ActiveRestrictions(),
            }, 50)
        end,
    })
end

local function Help()
    ns.Print(L.HELP_HEADER)
    for _, key in ipairs({ "HELP_INFO", "HELP_BENCH", "HELP_MODEL", "HELP_BUDGET", "HELP_LOG",
        "HELP_TARGET", "HELP_SPEED", "HELP_SIMULATE", "HELP_NAMES", "HELP_TRANSLIT", "HELP_GLOSS", "HELP_CATCHUP", "HELP_REPORT", "HELP_RESET", "HELP_FREE", "HELP_CLICK" }) do
        ns.Print(L[key])
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

-- /tprobe gloss add|del|list|test ...
local function Gloss(arg)
    local action, rest = arg:match("^(%S*)%s*(.-)$")
    action = action:lower()
    local custom = ns.db.glossary
    if action == "add" then
        local key, text = rest:match("^(" .. ns.Glossary.KEY_PATTERN .. ")%s+(.-)$")
        if not key or text == "" then
            ns.Print(L.GLOSS_HELP_ADD)
            return
        end
        custom[key:lower()] = (text:gsub("|", "/"))
        ns.Printf(L.GLOSS_ADDED, Label(key), custom[key:lower()])
    elseif action == "del" then
        local key = rest:match("^(" .. ns.Glossary.KEY_PATTERN .. ")$")
        if not key then
            ns.Print(L.GLOSS_HELP_DEL)
            return
        end
        key = key:lower()
        if ns.Glossary.IsBuiltin(key) then
            custom[key] = false
        else
            custom[key] = nil
        end
        ns.Printf(L.GLOSS_REMOVED, Label(key))
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
            ns.Printf(L.GLOSS_ENTRY, Label(key), custom[key] == false and L.GLOSS_DISABLED or custom[key])
        end
    elseif action == "test" and rest ~= "" then
        ns.Print(ns.Glossary.Describe(rest) or L.SUFFIX_NO_GLOSS)
    else
        GlossHelp()
    end
end

local handlers = {
    info = PrintInfo,
    bench = function()
        ns.Bench.Start()
    end,
    model = function(arg)
        local pieces = tonumber(arg) or 8
        RunModel(math.min(64, math.max(1, math.floor(pieces))))
    end,
    budget = function(arg)
        local ms = tonumber(arg)
        if not ms or ms < 0.5 or ms > 50 then
            ns.Print(L.BUDGET_USAGE)
            return
        end
        ns.db.budgetMs = ms
        ns.Printf(L.BUDGET_SET, ms, ns.Scheduler.EffectiveBudget())
    end,
    log = function(arg)
        arg = arg:lower()
        if arg == "on" or arg == "off" then
            ns.LockdownLog.SetEnabled(arg == "on")
        end
        ns.Print(ns.db.logging and L.LOG_ON or L.LOG_OFF)
    end,
    target = function(arg)
        arg = arg:lower()
        for _, t in ipairs(ns.Lang.TARGETS) do
            if t == arg then
                ns.db.target = t
            end
        end
        ns.Printf(L.TARGET_IS, ns.Lang.NAMES[ns.db.target] or ns.db.target, table.concat(ns.Lang.TARGETS, ", "))
    end,
    names = function(arg)
        arg = arg:lower()
        if arg == "on" or arg == "off" then
            ns.db.names = arg == "on"
        end
        ns.Print(ns.db.names and L.NAMES_ON or L.NAMES_OFF)
    end,
    translit = function(arg)
        if arg == "" then
            ns.Print(L.HELP_TRANSLIT)
            return
        end
        ns.Print(ns.Glossary.Romanize(arg) or L.TRANSLIT_NA)
    end,
    speed = function(arg)
        arg = arg:lower()
        if ns.Scheduler.SPEEDS[arg] then
            ns.db.speed = arg
        end
        ns.Print(ns.db.speed == "fast" and L.SPEED_FAST or L.SPEED_SAFE)
    end,
    simulate = function(arg)
        arg = arg:lower()
        if arg == "on" or arg == "off" then
            ns.db.simulate = arg == "on"
        end
        ns.Print(ns.db.simulate and L.SIMULATE_ON or L.SIMULATE_OFF)
    end,
    gloss = Gloss,
    catchup = function()
        ns.LockdownLog.CatchupNow()
    end,
    report = function()
        ns.LockdownLog.Report()
        if ns.db.results.bench then
            ns.Bench.PrintResult(ns.db.results.bench)
        end
    end,
    reset = function()
        ns.LockdownLog.Reset()
        wipe(ns.db.results.models)
        wipe(ns.db.results.clicks)
        ns.db.results.bench = nil
        ns.Print(L.RESET_DONE)
    end,
    free = function()
        if ns.Scheduler.IsBusy() then
            ns.Print(L.BUSY)
            return
        end
        ns.Model.Free()
        ns.Print(L.MODEL_FREED)
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

ns.OnLoad(function()
    ns.db.results.models = ns.db.results.models or {}
    -- Keep the client's script budget with the results, for reading the file later.
    local limits, copy = ns.Scheduler.GetBucketLimits(), {}
    for key, value in pairs(limits or {}) do
        copy[key] = value
    end
    local version, build = GetBuildInfo()
    ns.db.results.client = { build = version .. "." .. build, limits = copy }
end)
