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
    ns.Printf(L.INFO_SLICE, ns.Scheduler.EffectiveBudget(), GetFramerate(), ns.db.budgetMs)

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
    local srcPieces, tgtPieces = pieces, math.floor(pieces * 1.1 + 0.5) + 1
    if not ns.Model.IsBuilt() then
        ns.Printf(L.MODEL_BUILD, ns.Model.ExpectedBytes() / 1048576)
        ns.Scheduler.Enqueue({
            run = function()
                ns.Model.Build()
            end,
            done = function(job, ok)
                if ok then
                    ns.Printf(L.MODEL_BUILT, ns.Model.Bytes() / 1048576, job.stats.busyMs / 1000, job.stats.wallMs / 1000)
                end
            end,
        })
    end
    ns.Scheduler.Enqueue({
        run = function(job)
            job.macs = ns.Model.Forward(srcPieces, tgtPieces)
        end,
        done = function(job, ok)
            if not ok then
                return
            end
            local s = job.stats
            ns.Printf(L.MODEL_RESULT, srcPieces, tgtPieces, job.macs / 1e6, s.busyMs / 1000, s.wallMs / 1000,
                s.frames, s.maxBudgetMs, s.maxSliceMs)
            ns.PushCapped(ns.db.results.models, {
                when = date("%Y-%m-%d %H:%M"), src = srcPieces, tgt = tgtPieces, macs = job.macs,
                busyMs = s.busyMs, wallMs = s.wallMs, frames = s.frames, maxSliceMs = s.maxSliceMs,
                budgetMs = s.maxBudgetMs, fps = GetFramerate(),
            }, 50)
        end,
    })
end

local function Help()
    ns.Print(L.HELP_HEADER)
    for _, key in ipairs({ "HELP_INFO", "HELP_BENCH", "HELP_MODEL", "HELP_BUDGET", "HELP_LOG",
        "HELP_SIMULATE", "HELP_GLOSS", "HELP_REPORT", "HELP_RESET", "HELP_FREE", "HELP_CLICK" }) do
        ns.Print(L[key])
    end
end

local function GlossHelp()
    ns.Printf(L.GLOSS_COUNT, ns.Glossary.Count())
    for _, key in ipairs({ "GLOSS_HELP_ADD", "GLOSS_HELP_DEL", "GLOSS_HELP_LIST", "GLOSS_HELP_TEST" }) do
        ns.Print(L[key])
    end
end

-- /tprobe gloss add|del|list|test ...
local function Gloss(arg)
    local action, rest = arg:match("^(%S*)%s*(.-)$")
    action = action:lower()
    local custom = ns.db.glossary
    if action == "add" then
        local key, text = rest:match("^(%w+)%s+(.-)$")
        if not key or text == "" then
            ns.Print(L.GLOSS_HELP_ADD)
            return
        end
        custom[key:lower()] = (text:gsub("|", "/"))
        ns.Printf(L.GLOSS_ADDED, key:upper(), custom[key:lower()])
    elseif action == "del" then
        local key = rest:match("^(%w+)$")
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
        ns.Printf(L.GLOSS_REMOVED, key:upper())
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
            ns.Printf(L.GLOSS_ENTRY, key:upper(), custom[key] == false and L.GLOSS_DISABLED or custom[key])
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
    simulate = function(arg)
        arg = arg:lower()
        if arg == "on" or arg == "off" then
            ns.db.simulate = arg == "on"
        end
        ns.Print(ns.db.simulate and L.SIMULATE_ON or L.SIMULATE_OFF)
    end,
    gloss = Gloss,
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
            ns.Print(L.ALREADY_RUNNING)
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
end)
