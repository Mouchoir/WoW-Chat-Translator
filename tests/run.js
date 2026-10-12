// Runs tests/harness.lua against the real addon files (in TOC order) inside a Lua VM
// (fengari, Lua 5.3) with a mock WoW Forever client. Usage:
//   npm install
//   node tests/run.js [enUS|frFR]
const fs = require('fs');
const path = require('path');
const { lua, lauxlib, lualib, to_luastring } = require('fengari');

const root = path.join(__dirname, '..');
const addonDir = path.join(root, 'addon', 'TranslateProbe');
const toc = fs.readFileSync(path.join(addonDir, 'TranslateProbe.toc'), 'utf8');
const files = toc.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#'));

const L = lauxlib.luaL_newstate();
lualib.luaL_openlibs(L);

const setGlobal = (name, value) => {
    lua.lua_pushstring(L, to_luastring(value));
    lua.lua_setglobal(L, to_luastring(name));
};

setGlobal('ADDON_FILES', files.join(','));
for (const file of files) {
    setGlobal('SRC_' + file, fs.readFileSync(path.join(addonDir, file), 'utf8'));
}
setGlobal('LOCALE_UNDER_TEST', process.argv[2] || 'enUS');

const harness = fs.readFileSync(path.join(__dirname, 'harness.lua'), 'utf8');
if (lauxlib.luaL_dostring(L, to_luastring(harness)) !== lua.LUA_OK) {
    console.error('lua error: ' + lua.lua_tojsstring(L, -1));
    process.exit(1);
}

lua.lua_getglobal(L, to_luastring('TEST_FAILURES'));
const failures = lua.lua_tointeger(L, -1);
console.log(failures === 0 ? 'ALL TESTS PASSED' : failures + ' TEST(S) FAILED');
process.exit(failures === 0 ? 0 : 1);
