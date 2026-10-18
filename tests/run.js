// Runs tests/harness.lua against the real addon files (in TOC order) inside a Lua VM
// (fengari, Lua 5.3) with the mock WoW Forever client of tests/mock.lua. Usage:
//   npm install
//   node tests/run.js [enUS|frFR] [--stub-bridge]
// Load order: tests/mock.lua, a LuaBitOp-compatible `bit` (WoW's Lua 5.1 has it, Lua 5.3 does
// not), then the TOC file sources, then tests/harness.lua, which calls Mock.Load with them (so
// it can prepare saved variables first). A TOC file that does not exist loads
// tests/stubs/<file> instead when there is one; --stub-bridge (or TPROBE_STUB_BRIDGE=1) also
// replaces the real Bridge.lua with tests/stubs/Bridge.lua. The real Bridge.lua source is also
// passed as REAL_BRIDGE_SOURCE, so a stub run checks that the stub builds the same bytes.
//
// As a module (for tests/e2e): require('./run.js') gives { BIT_SHIM, newState, runLua,
// tocFiles, addonSources }.
const fs = require('fs');
const path = require('path');
const { lua, lauxlib, lualib, to_luastring } = require('fengari');

const root = path.join(__dirname, '..');
const addonDir = path.join(root, 'addon', 'TranslateProbe');
const stubDir = path.join(__dirname, 'stubs');

// LuaBitOp semantics: 32-bit operations, results normalised to signed int32.
const BIT_SHIM = [
    'local function tobit(x) x = math.floor(x) % 4294967296 if x >= 2147483648 then x = x - 4294967296 end',
    '  return math.tointeger(x) end',
    'bit = { tobit = tobit,',
    '  band = function(a, b) return tobit(tobit(a) & tobit(b)) end,',
    '  bor = function(a, b) return tobit(tobit(a) | tobit(b)) end,',
    '  bxor = function(a, b) return tobit(tobit(a) ~ tobit(b)) end,',
    '  bnot = function(a) return tobit(~tobit(a)) end,',
    '  lshift = function(a, n) return tobit(tobit(a) << (n % 32)) end,',
    '  rshift = function(a, n) return tobit((tobit(a) & 0xFFFFFFFF) >> (n % 32)) end }',
].join('\n');

function newState() {
    const L = lauxlib.luaL_newstate();
    lualib.luaL_openlibs(L);
    return L;
}

// Runs a chunk; throws with the Lua error message on failure.
function runLua(L, src, name) {
    if (lauxlib.luaL_loadbuffer(L, to_luastring(src), null, to_luastring('@' + name)) !== lua.LUA_OK ||
        lua.lua_pcall(L, 0, 0, 0) !== lua.LUA_OK) {
        const msg = lua.lua_tojsstring(L, -1);
        lua.lua_settop(L, 0);
        throw new Error(msg);
    }
}

function tocFiles() {
    const toc = fs.readFileSync(path.join(addonDir, 'TranslateProbe.toc'), 'utf8');
    return toc.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#'));
}

// [{ file, source, from }] in TOC order; from = 'addon' or 'stub'.
function addonSources(stubBridge) {
    return tocFiles().map((file) => {
        const real = path.join(addonDir, file);
        const stub = path.join(stubDir, file);
        const useStub = fs.existsSync(stub) && (!fs.existsSync(real) || (stubBridge && file === 'Bridge.lua'));
        const from = useStub ? 'stub' : 'addon';
        return { file, source: fs.readFileSync(useStub ? stub : real, 'utf8'), from };
    });
}

function setGlobalString(L, name, value) {
    lua.lua_pushstring(L, to_luastring(value));
    lua.lua_setglobal(L, to_luastring(name));
}

// ADDON_FILES = { file... }, ADDON_SOURCES = { [file] = source }, ADDON_FROM = { [file] = from }
function setAddonGlobals(L, sources) {
    lua.lua_newtable(L);
    sources.forEach((s, i) => {
        lua.lua_pushstring(L, to_luastring(s.file));
        lua.lua_rawseti(L, -2, i + 1);
    });
    lua.lua_setglobal(L, to_luastring('ADDON_FILES'));
    for (const [name, key] of [['ADDON_SOURCES', 'source'], ['ADDON_FROM', 'from']]) {
        lua.lua_newtable(L);
        for (const s of sources) {
            lua.lua_pushstring(L, to_luastring(s[key]));
            lua.lua_setfield(L, -2, to_luastring(s.file));
        }
        lua.lua_setglobal(L, to_luastring(name));
    }
}

function main() {
    const args = process.argv.slice(2);
    const locale = args.find((a) => !a.startsWith('--')) || 'enUS';
    const stubBridge = args.includes('--stub-bridge') || process.env.TPROBE_STUB_BRIDGE === '1';
    const sources = addonSources(stubBridge);
    const L = newState();
    setGlobalString(L, 'LOCALE_UNDER_TEST', locale);
    try {
        runLua(L, fs.readFileSync(path.join(__dirname, 'mock.lua'), 'utf8'), 'mock.lua');
        runLua(L, BIT_SHIM, 'bitshim');
        setAddonGlobals(L, sources);
        const realBridge = path.join(addonDir, 'Bridge.lua');
        if (fs.existsSync(realBridge)) {
            setGlobalString(L, 'REAL_BRIDGE_SOURCE', fs.readFileSync(realBridge, 'utf8'));
        }
        const bridge = sources.find((s) => s.file === 'Bridge.lua');
        console.log('== ' + locale + ', Bridge.lua from ' + (bridge ? bridge.from : 'nowhere'));
        runLua(L, fs.readFileSync(path.join(__dirname, 'harness.lua'), 'utf8'), 'harness.lua');
    } catch (e) {
        console.error('lua error: ' + e.message);
        process.exit(1);
    }
    lua.lua_getglobal(L, to_luastring('TEST_FAILURES'));
    const failures = lua.lua_tointeger(L, -1);
    lua.lua_getglobal(L, to_luastring('TEST_COUNT'));
    const total = lua.lua_tointeger(L, -1);
    console.log(failures === 0 ? 'ALL TESTS PASSED (' + total + ' checks)' : failures + ' TEST(S) FAILED of ' + total);
    process.exit(failures === 0 ? 0 : 1);
}

module.exports = { BIT_SHIM, newState, runLua, tocFiles, addonSources };

if (require.main === module) {
    main();
}
