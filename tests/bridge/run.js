// Checks addon/TranslateProbe/Bridge.lua against the Python reference vectors in tests/vectors
// (written by tools/bridge/make_vectors.py): encoder traces (frame bytes, block colours and strip
// visibility at every game frame), every row tag, the markup and the message builders. Runs the
// Lua 5.1 code in fengari (Lua 5.3) with a LuaBitOp-compatible `bit` shim.
// Usage: node tests/bridge/run.js
const fs = require('fs');
const path = require('path');
const { lua, lauxlib, lualib, to_luastring } = require('fengari');

const root = path.join(__dirname, '..', '..');
const vectors = path.join(root, 'tests', 'vectors');

// JSON value -> Lua constructor source. {"s": hex} is a byte string.
const toLua = (v) => {
    if (v === null || v === undefined) return 'nil';
    if (typeof v === 'boolean') return v ? 'true' : 'false';
    if (typeof v === 'number') return Number.isInteger(v) ? String(v) : v.toPrecision(17);
    if (typeof v === 'string') {
        return '"' + Array.from(Buffer.from(v, 'utf8')).map((b) => '\\' + String(b).padStart(3, '0')).join('') + '"';
    }
    if (Array.isArray(v)) return '{' + v.map(toLua).join(',') + '}';
    const keys = Object.keys(v);
    if (keys.length === 1 && keys[0] === 's' && typeof v.s === 'string') {
        return '"' + (v.s.match(/../g) || []).map((h) => '\\' + String(parseInt(h, 16)).padStart(3, '0')).join('') + '"';
    }
    return '{' + keys.map((k) => '[' + JSON.stringify(k) + ']=' + toLua(v[k])).join(',') + '}';
};

const L = lauxlib.luaL_newstate();
lualib.luaL_openlibs(L);
const run = (src, name) => {
    if (lauxlib.luaL_loadbuffer(L, to_luastring(src), null, to_luastring('=' + name)) !== lua.LUA_OK ||
        lua.lua_pcall(L, 0, 0, 0) !== lua.LUA_OK) {
        console.error('lua error: ' + lua.lua_tojsstring(L, -1));
        process.exit(1);
    }
};
const setString = (name, s) => {
    lua.lua_pushstring(L, to_luastring(s));
    lua.lua_setglobal(L, to_luastring(name));
};

// LuaBitOp semantics: 32-bit operations, results normalised to signed int32 (as in WoW).
run([
    'local function tobit(x) x = math.floor(x) % 4294967296 if x >= 2147483648 then x = x - 4294967296 end',
    '  return math.tointeger(x) end',
    'bit = { tobit = tobit,',
    '  band = function(a, b) return tobit(tobit(a) & tobit(b)) end,',
    '  bor = function(a, b) return tobit(tobit(a) | tobit(b)) end,',
    '  bxor = function(a, b) return tobit(tobit(a) ~ tobit(b)) end,',
    '  bnot = function(a) return tobit(~tobit(a)) end,',
    '  lshift = function(a, n) return tobit(tobit(a) << (n % 32)) end,',
    '  rshift = function(a, n) return tobit((tobit(a) & 0xFFFFFFFF) >> (n % 32)) end,',
    '  arshift = function(a, n) return tobit(tobit(a) >> (n % 32)) end }',
].join('\n'), 'bitshim');

setString('SRC_BRIDGE', fs.readFileSync(path.join(root, 'addon', 'TranslateProbe', 'Bridge.lua'), 'utf8'));
run(fs.readFileSync(path.join(__dirname, 'bridge_test.lua'), 'utf8'), 'bridge_test.lua');

const framesDir = path.join(vectors, 'frames');
const traces = fs.readdirSync(framesDir).filter((f) => f.endsWith('.json')).sort();
for (const f of traces) {
    const t = JSON.parse(fs.readFileSync(path.join(framesDir, f), 'utf8'));
    run('TRACE = ' + toLua(t), f);
    run('RunTrace(' + JSON.stringify(f) + ')', 'trace ' + f);
}
run('TAGS = ' + toLua(JSON.parse(fs.readFileSync(path.join(vectors, 'tags.json'), 'utf8'))), 'tags.json');
run('RunTags()', 'tags');
run('MESSAGES = ' + toLua(JSON.parse(fs.readFileSync(path.join(vectors, 'messages.json'), 'utf8'))), 'messages.json');
run('RunMessages()', 'messages');
run('RunApi()', 'api');
run('Summary()', 'summary');

lua.lua_getglobal(L, to_luastring('TEST_FAILURES'));
const failures = lua.lua_tointeger(L, -1);
console.log(failures === 0 ? 'ALL BRIDGE TESTS PASSED' : failures + ' BRIDGE TEST(S) FAILED');
process.exit(failures === 0 ? 0 : 1);
