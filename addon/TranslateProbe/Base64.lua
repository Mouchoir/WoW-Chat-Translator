-- Pure-Lua base64 decoder, used only when the client's C_EncodingUtil.DecodeBase64 is missing
-- (test harnesses). The game always has the native one.
local _, ns = ...

local byte, char, concat = string.byte, string.char, table.concat

local VALUE = {}
do
    local alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    for i = 1, #alphabet do
        VALUE[byte(alphabet, i)] = i - 1
    end
end

function ns.Base64Decode(s)
    local out, parts = {}, {}
    local n = 0
    for i = 1, #s - 3, 4 do
        local a, b, c, d = byte(s, i, i + 3)
        local va, vb = VALUE[a], VALUE[b]
        local vc, vd = VALUE[c], VALUE[d]
        local v = va * 262144 + vb * 4096 + (vc or 0) * 64 + (vd or 0)
        local b1 = math.floor(v / 65536)
        local b2 = math.floor(v / 256) % 256
        local b3 = v % 256
        n = n + 1
        if vc == nil then
            out[n] = char(b1)
        elseif vd == nil then
            out[n] = char(b1, b2)
        else
            out[n] = char(b1, b2, b3)
        end
        if n == 4096 then
            parts[#parts + 1] = concat(out, "", 1, n)
            n = 0
        end
    end
    parts[#parts + 1] = concat(out, "", 1, n)
    return concat(parts)
end
