"""Run the real Lua engine on a converted model pack in a native PUC Lua (through ctypes) and
compare every token with the Python reference (tests/golden/<pair>.json).

Needs a Lua 5.3 DLL already on the machine (default: Cheat Engine's lua53-64.dll; override with
the TPROBE_LUA_DLL environment variable). The DLL only hosts the test; the addon targets WoW's
Lua 5.1 and uses nothing newer.

Usage: python -I -X utf8 tests/native/run_engine.py <pair> [packs_dir]
"""
import base64
import ctypes
import json
import os
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DLL = os.environ.get('TPROBE_LUA_DLL', r'C:\Program Files\Cheat Engine\lua53-64.dll')
ADDON = os.path.join(ROOT, 'addon', 'TranslateProbe')
ENGINE_FILES = ['Scheduler.lua', 'Kernels.lua', 'Base64.lua', 'Tokenizer.lua', 'Engine.lua']

PRELUDE = r'''
unpack = table.unpack
local frames = {}
CreateFrame = function()
    local f = {}
    function f:SetScript() end
    function f:Show() end
    function f:Hide() end
    function f:RegisterEvent() end
    return f
end
debugprofilestop = function() return os.clock() * 1000 end
InCombatLockdown = function() return false end
GetFramerate = function() return 60 end
geterrorhandler = function() return print end
C_EncodingUtil = { DecodeBase64 = NATIVE_B64 }
NS = {}
function LOAD(path, ...)
    local chunk = assert(loadfile(path))
    return chunk(...)
end
function RUN(pair, text)
    local model = NS.Engine.Load(pair)
    local t0 = os.clock()
    local translation, src, out, short = NS.Engine.TranslateWith(model, text)
    local ms = (os.clock() - t0) * 1000
    return table.concat(src, ",") .. "|" .. table.concat(out, ",") .. "|" .. tostring(short) .. "|"
        .. tostring(model.macs) .. "|" .. string.format("%.0f", ms) .. "|" .. translation
end
'''


def main():
    pair = sys.argv[1]
    free_texts = sys.argv[3:] if len(sys.argv) > 3 and sys.argv[2] == '--text' else None
    packs = sys.argv[2] if len(sys.argv) > 2 and not free_texts else os.path.join(ROOT, 'build', 'addons')
    code = pair.replace('-', '')
    pack_dir = os.path.join(packs, 'TranslateProbe_' + code)
    if free_texts:
        golden = [{'text': t, 'src_ids': None, 'out_ids': None, 'shortlist': None} for t in free_texts]
    else:
        with open(os.path.join(ROOT, 'tests', 'golden', pair + '.json'), encoding='utf-8') as f:
            golden = json.load(f)['entries']

    os.add_dll_directory(os.path.dirname(DLL))
    d = ctypes.CDLL(DLL)
    d.luaL_newstate.restype = ctypes.c_void_p
    d.lua_tolstring.restype = ctypes.c_void_p
    d.lua_tolstring.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.POINTER(ctypes.c_size_t)]
    d.lua_pushlstring.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    d.luaL_loadbufferx.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_char_p, ctypes.c_char_p]
    d.lua_getglobal.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    d.lua_setglobal.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    d.lua_pcallk.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
    d.lua_settop.argtypes = [ctypes.c_void_p, ctypes.c_int]
    d.lua_pushcclosure.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
    L = ctypes.c_void_p(d.luaL_newstate())
    d.luaL_openlibs(L)

    def to_bytes(idx):
        size = ctypes.c_size_t(0)
        ptr = d.lua_tolstring(L, idx, ctypes.byref(size))
        return ctypes.string_at(ptr, size.value) if ptr else None

    CFUNC = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)

    @CFUNC
    def native_b64(state):
        raw = base64.b64decode(to_bytes(1))
        d.lua_pushlstring(L, raw, len(raw))
        return 1

    d.lua_pushcclosure(L, ctypes.cast(native_b64, ctypes.c_void_p), 0)
    d.lua_setglobal(L, b'NATIVE_B64')

    def call(name, *args):
        d.lua_getglobal(L, name.encode())
        for a in args:
            b = a.encode('utf-8') if isinstance(a, str) else a
            d.lua_pushlstring(L, b, len(b))
        if d.lua_pcallk(L, len(args), 1, 0, None, None) != 0:
            raise RuntimeError(to_bytes(-1).decode('utf-8', 'replace'))
        out = to_bytes(-1)
        d.lua_settop(L, 0)
        return out

    src = PRELUDE.encode()
    assert d.luaL_loadbufferx(L, src, len(src), b'=prelude', None) == 0, to_bytes(-1)
    assert d.lua_pcallk(L, 0, 0, 0, None, None) == 0, to_bytes(-1)
    d.lua_getglobal(L, b'NS')
    ns_ref_name = b'NS'
    d.lua_settop(L, 0)
    for name in ENGINE_FILES:
        # chunk("TranslateProbe", NS) through a tiny Lua wrapper
        code_line = 'LOAD(%r, "TranslateProbe", NS)' % os.path.join(ADDON, name).replace('\\', '/')
        b = code_line.encode()
        assert d.luaL_loadbufferx(L, b, len(b), b'=load', None) == 0, to_bytes(-1)
        if d.lua_pcallk(L, 0, 0, 0, None, None) != 0:
            raise RuntimeError(to_bytes(-1))
    with open(os.path.join(pack_dir, 'TranslateProbe_%s.toc' % code), encoding='utf-8') as f:
        files = [ln.strip() for ln in f if ln.strip() and not ln.startswith('#')]
    t0 = time.time()
    for name in files:
        b = ('LOAD(%r)' % os.path.join(pack_dir, name).replace('\\', '/')).encode()
        assert d.luaL_loadbufferx(L, b, len(b), b'=pack', None) == 0, to_bytes(-1)
        if d.lua_pcallk(L, 0, 0, 0, None, None) != 0:
            raise RuntimeError(to_bytes(-1))
    print('pack files loaded in %.1fs' % (time.time() - t0))
    del ns_ref_name

    failures = 0
    for e in golden:
        t1 = time.time()
        res = call('RUN', pair, e['text']).decode('utf-8')
        src_ids, out_ids, short, macs, ms, translation = res.split('|', 5)
        src_ids = [int(x) for x in src_ids.split(',') if x]
        out_ids = [int(x) for x in out_ids.split(',') if x]
        ok = free_texts is not None or (src_ids == e["src_ids"] and out_ids == e["out_ids"] and int(short) == e["shortlist"])
        failures += 0 if ok else 1
        print('%s %6s ms %6.1f M MACs (%5.1fs wall) %s -> %s' % (
            'ok  ' if ok else 'FAIL', ms, int(float(macs)) / 1e6, time.time() - t1, e['text'], translation))
        if not ok:
            print('     src lua %s\n     src ref %s\n     out lua %s\n     out ref %s\n     shortlist %s vs %d' % (
                src_ids, e['src_ids'], out_ids, e['out_ids'], short, e['shortlist']))
    print('ALL MATCH' if failures == 0 else '%d MISMATCH(ES)' % failures)
    return 0 if failures == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
