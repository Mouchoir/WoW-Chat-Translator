-- Raw Lua speed and memory cost inside the real client, the two numbers that decide whether
-- a local neural translator is usable. Runs as a Scheduler job.
local _, ns = ...
local L = ns.L

local Bench = {}
ns.Bench = Bench

local byte = string.byte
local random = math.random
local Check = ns.Scheduler.Check

Bench.targetMs = 1000           -- Lua time spent measuring each kernel
local ROWS, COLS, CHUNK = 1536, 256, 16

-- Dense matrix-vector product over a block of rows, weights as one Lua table per row.
local function KernelFloat(R, x, y, j0, j1)
    for j = j0, j1 do
        local acc, r = 0, R[j]
        for i = 1, COLS do
            acc = acc + r[i] * x[i]
        end
        y[j] = acc
    end
end

-- Same product, int8 weights in a string, 16 bytes per string.byte call (the model's kernel).
local function KernelInt8x16(S, x, y, j0, j1)
    for j = j0, j1 do
        local acc, base = 0, (j - 1) * COLS
        for i = 1, COLS, 16 do
            local p = base + i
            local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16 = byte(S, p, p + 15)
            acc = acc + a1 * x[i] + a2 * x[i + 1] + a3 * x[i + 2] + a4 * x[i + 3]
                + a5 * x[i + 4] + a6 * x[i + 5] + a7 * x[i + 6] + a8 * x[i + 7]
                + a9 * x[i + 8] + a10 * x[i + 9] + a11 * x[i + 10] + a12 * x[i + 11]
                + a13 * x[i + 12] + a14 * x[i + 13] + a15 * x[i + 14] + a16 * x[i + 15]
        end
        y[j] = acc
    end
end

-- Same product, one string.byte call per weight.
local function KernelInt8x1(S, x, y, j0, j1)
    for j = j0, j1 do
        local acc, base = 0, (j - 1) * COLS
        for i = 1, COLS do
            acc = acc + byte(S, base + i) * x[i]
        end
        y[j] = acc
    end
end

-- Millions of multiply-adds per second of Lua time, measured in small blocks between yields.
local function Rate(kernel, weights, x, y)
    local busy, macs, j = 0, 0, 1
    while busy < Bench.targetMs do
        local j1 = j + CHUNK - 1
        local t0 = debugprofilestop()
        kernel(weights, x, y, j, j1)
        busy = busy + (debugprofilestop() - t0)
        macs = macs + CHUNK * COLS
        j = (j1 >= ROWS) and 1 or (j1 + 1)
        Check()
    end
    return macs / busy / 1000
end

local function Measure(fn, ...)
    if not (C_AddOnProfiler and C_AddOnProfiler.MeasureCall) then
        return nil
    end
    local ok, results = pcall(C_AddOnProfiler.MeasureCall, fn, ...)
    if ok and type(results) == "table" then
        return results
    end
end

-- Bytes per table slot: one table.create allocation of N slots. Bytes per string byte: one
-- string.sub copy of the random bench string, which is never already interned. Both are single
-- allocations, so the profiler's allocatedBytes is not inflated by temporary buffers.
local function MemoryCosts(S)
    if not table.create then
        return nil
    end
    local N = 65536
    local slots = Measure(table.create, N)
    Check()
    local bytes = Measure(string.sub, S, 2)
    Check()
    if not (slots and bytes) then
        return nil
    end
    return {
        slot = tonumber(slots.allocatedBytes) / N,
        stringByte = tonumber(bytes.allocatedBytes) / (#S - 1),
    }
end

local function BuildData()
    local R = {}
    for j = 1, ROWS do
        local r = table.create and table.create(COLS) or {}
        for i = 1, COLS do
            r[i] = (random() - 0.5) * 0.1
        end
        R[j] = r
        Check()
    end
    local S = ns.Model.RandomBytes(ROWS * COLS, false)
    local x, y = {}, {}
    for i = 1, COLS do
        x[i] = random() - 0.5
    end
    for j = 1, ROWS do
        y[j] = 0
    end
    return R, S, x, y
end

function Bench.Run(job)
    local R, S, x, y = BuildData()
    local result = {
        when = date("%Y-%m-%d %H:%M"),
        floatRate = Rate(KernelFloat, R, x, y),
        int8Rate = Rate(KernelInt8x16, S, x, y),
        byteRate = Rate(KernelInt8x1, S, x, y),
    }
    result.memory = MemoryCosts(S)
    -- Cross-check debugprofilestop against the client's own profiler on one block.
    local probe = Measure(KernelInt8x16, S, x, y, 1, CHUNK)
    result.measureCallMs = probe and tonumber(probe.elapsedMilliseconds)
    job.result = result
end

-- Seconds of Lua and of waiting for a tiny model pass with the same shapes /tprobe model
-- uses (output = Model.TargetFor(input)), at the given speed, slice and frame rate.
function Bench.Project(int8Rate, srcPieces, budgetMs, fps)
    local tgtPieces = ns.Model.TargetFor(srcPieces)
    local macs = ns.Model.EstimateMacs(srcPieces, tgtPieces)
    local cpu = macs / (int8Rate * 1e6)
    return tgtPieces, macs, cpu, cpu * 1000 / (budgetMs * fps)
end

function Bench.Start()
    ns.Print(L.BENCH_START)
    ns.Scheduler.Enqueue({
        run = Bench.Run,
        done = function(job, ok)
            if not ok or not job.result then
                ns.Printf(L.JOB_FAILED, job.err or "?")
                return
            end
            local r, s = job.result, job.stats
            r.budgetMs, r.frames, r.wallMs, r.maxSliceMs = s.maxBudgetMs, s.frames, s.wallMs, s.maxSliceMs
            r.pausedMs, r.fps = s.pausedMs, s.avgFps
            ns.db.results.bench = r
            Bench.PrintResult(r)
        end,
    })
end

function Bench.PrintResult(r)
    ns.Printf(L.BENCH_KERNEL, L.KERNEL_FLOAT, r.floatRate)
    ns.Printf(L.BENCH_KERNEL, L.KERNEL_INT8, r.int8Rate)
    ns.Printf(L.BENCH_KERNEL, L.KERNEL_BYTE, r.byteRate)
    if r.memory then
        ns.Printf(L.BENCH_MEM, r.memory.slot, r.memory.stringByte)
    else
        ns.Print(L.BENCH_MEM_NA)
    end
    local fps = math.max(r.fps or 60, 20)
    for _, pieces in ipairs({ 8, 15 }) do
        local tgt, macs, cpu, wait = Bench.Project(r.int8Rate, pieces, r.budgetMs, fps)
        ns.Printf(L.BENCH_PROJECTION, pieces, tgt, macs / 1e6, cpu, wait)
    end
    ns.Printf(L.BENCH_DONE, (r.wallMs or 0) / 1000, r.budgetMs or 0, fps, r.maxSliceMs or 0)
end
