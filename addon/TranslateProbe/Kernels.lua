-- Matrix-vector kernels for int8 weights stored row by row in one string (byte = q + 128 or raw).
-- string.byte costs per returned byte, so the fast kernels make each extracted byte serve more:
-- Local16 keeps a 16-value slice of x in locals (one token), Batch4 serves 4 tokens per byte.
-- All three add the products in the same order (per row: slice after slice, left to right inside
-- a slice), so their results are bit-identical. They write raw sums into y; callers apply scales.
-- check, when given, is called every few rows so a coroutine can yield (it never changes results).
local _, ns = ...

local Kernels = {}
ns.Kernels = Kernels

local byte = string.byte

-- Reference kernel: one row at a time, x read from its table.
function Kernels.Row16(S, cols, x, y, j0, j1, check)
    for j = j0, j1 do
        local acc, base = 0, (j - 1) * cols
        for i = 1, cols, 16 do
            local p = base + i
            local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16 = byte(S, p, p + 15)
            acc = acc + a1 * x[i] + a2 * x[i + 1] + a3 * x[i + 2] + a4 * x[i + 3]
                + a5 * x[i + 4] + a6 * x[i + 5] + a7 * x[i + 6] + a8 * x[i + 7]
                + a9 * x[i + 8] + a10 * x[i + 9] + a11 * x[i + 10] + a12 * x[i + 11]
                + a13 * x[i + 12] + a14 * x[i + 13] + a15 * x[i + 14] + a16 * x[i + 15]
        end
        y[j] = acc
        if check then
            check()
        end
    end
end

-- One token: for each 16-column slice, x is loaded into locals once and reused by every row.
function Kernels.Local16(S, cols, x, y, j0, j1, check)
    for j = j0, j1 do
        y[j] = 0
    end
    for i = 1, cols, 16 do
        local x1, x2, x3, x4, x5, x6, x7, x8 = x[i], x[i + 1], x[i + 2], x[i + 3], x[i + 4], x[i + 5], x[i + 6], x[i + 7]
        local x9, x10, x11, x12 = x[i + 8], x[i + 9], x[i + 10], x[i + 11]
        local x13, x14, x15, x16 = x[i + 12], x[i + 13], x[i + 14], x[i + 15]
        for j = j0, j1 do
            local p = (j - 1) * cols + i
            local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16 = byte(S, p, p + 15)
            y[j] = y[j] + a1 * x1 + a2 * x2 + a3 * x3 + a4 * x4 + a5 * x5 + a6 * x6 + a7 * x7 + a8 * x8
                + a9 * x9 + a10 * x10 + a11 * x11 + a12 * x12 + a13 * x13 + a14 * x14 + a15 * x15 + a16 * x16
            if check and j % 64 == 0 then
                check()
            end
        end
    end
end

-- Four tokens through the same weights: each extracted byte serves all four.
function Kernels.Batch4(S, cols, xa, xb, xc, xd, ya, yb, yc, yd, j0, j1, check)
    for j = j0, j1 do
        ya[j], yb[j], yc[j], yd[j] = 0, 0, 0, 0
    end
    for i = 1, cols, 16 do
        local a1, a2, a3, a4, a5, a6, a7, a8 = xa[i], xa[i + 1], xa[i + 2], xa[i + 3], xa[i + 4], xa[i + 5], xa[i + 6], xa[i + 7]
        local a9, a10, a11, a12, a13, a14, a15, a16 = xa[i + 8], xa[i + 9], xa[i + 10], xa[i + 11], xa[i + 12], xa[i + 13], xa[i + 14], xa[i + 15]
        local b1, b2, b3, b4, b5, b6, b7, b8 = xb[i], xb[i + 1], xb[i + 2], xb[i + 3], xb[i + 4], xb[i + 5], xb[i + 6], xb[i + 7]
        local b9, b10, b11, b12, b13, b14, b15, b16 = xb[i + 8], xb[i + 9], xb[i + 10], xb[i + 11], xb[i + 12], xb[i + 13], xb[i + 14], xb[i + 15]
        local c1, c2, c3, c4, c5, c6, c7, c8 = xc[i], xc[i + 1], xc[i + 2], xc[i + 3], xc[i + 4], xc[i + 5], xc[i + 6], xc[i + 7]
        local c9, c10, c11, c12, c13, c14, c15, c16 = xc[i + 8], xc[i + 9], xc[i + 10], xc[i + 11], xc[i + 12], xc[i + 13], xc[i + 14], xc[i + 15]
        local d1, d2, d3, d4, d5, d6, d7, d8 = xd[i], xd[i + 1], xd[i + 2], xd[i + 3], xd[i + 4], xd[i + 5], xd[i + 6], xd[i + 7]
        local d9, d10, d11, d12, d13, d14, d15, d16 = xd[i + 8], xd[i + 9], xd[i + 10], xd[i + 11], xd[i + 12], xd[i + 13], xd[i + 14], xd[i + 15]
        for j = j0, j1 do
            local p = (j - 1) * cols + i
            local w1, w2, w3, w4, w5, w6, w7, w8, w9, w10, w11, w12, w13, w14, w15, w16 = byte(S, p, p + 15)
            ya[j] = ya[j] + w1 * a1 + w2 * a2 + w3 * a3 + w4 * a4 + w5 * a5 + w6 * a6 + w7 * a7 + w8 * a8
                + w9 * a9 + w10 * a10 + w11 * a11 + w12 * a12 + w13 * a13 + w14 * a14 + w15 * a15 + w16 * a16
            yb[j] = yb[j] + w1 * b1 + w2 * b2 + w3 * b3 + w4 * b4 + w5 * b5 + w6 * b6 + w7 * b7 + w8 * b8
                + w9 * b9 + w10 * b10 + w11 * b11 + w12 * b12 + w13 * b13 + w14 * b14 + w15 * b15 + w16 * b16
            yc[j] = yc[j] + w1 * c1 + w2 * c2 + w3 * c3 + w4 * c4 + w5 * c5 + w6 * c6 + w7 * c7 + w8 * c8
                + w9 * c9 + w10 * c10 + w11 * c11 + w12 * c12 + w13 * c13 + w14 * c14 + w15 * c15 + w16 * c16
            yd[j] = yd[j] + w1 * d1 + w2 * d2 + w3 * d3 + w4 * d4 + w5 * d5 + w6 * d6 + w7 * d7 + w8 * d8
                + w9 * d9 + w10 * d10 + w11 * d11 + w12 * d12 + w13 * d13 + w14 * d14 + w15 * d15 + w16 * d16
            if check and j % 64 == 0 then
                check()
            end
        end
    end
end
