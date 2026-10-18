// Records an E2E scenario: runs the real addon (TOC order) on the mock WoW client of tests/mock.lua,
// drives it with tests/e2e/scenarios/<name>.lua through tests/e2e/scenario_lib.lua, and writes the
// recording that fakewow replays (format in helper/testkit/src/scenario.rs).
//
//   node tests/e2e/make_scenario.js <scenario.lua> <out.json> [--locale enUS] [--stub-bridge]
//
// No downloads: fengari comes from the repo's node_modules, like tests/run.js.
const fs = require('fs');
const path = require('path');
const { lua, to_luastring } = require('fengari');
const run = require('../run.js');

function setGlobalString(L, name, value) {
    lua.lua_pushstring(L, to_luastring(value));
    lua.lua_setglobal(L, to_luastring(name));
}

// ADDON_FILES = { file... }, ADDON_SOURCES = { [file] = source } (same globals as tests/run.js)
function setAddonGlobals(L, sources) {
    lua.lua_newtable(L);
    sources.forEach((s, i) => {
        lua.lua_pushstring(L, to_luastring(s.file));
        lua.lua_rawseti(L, -2, i + 1);
    });
    lua.lua_setglobal(L, to_luastring('ADDON_FILES'));
    lua.lua_newtable(L);
    for (const s of sources) {
        lua.lua_pushstring(L, to_luastring(s.source));
        lua.lua_setfield(L, -2, to_luastring(s.file));
    }
    lua.lua_setglobal(L, to_luastring('ADDON_SOURCES'));
}

function getGlobalString(L, name) {
    lua.lua_getglobal(L, to_luastring(name));
    const v = lua.lua_isstring(L, -1) ? lua.lua_tojsstring(L, -1) : null;
    lua.lua_pop(L, 1);
    return v;
}

function main() {
    const args = process.argv.slice(2);
    const positional = [];
    let locale = 'enUS';
    let stubBridge = false;
    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--locale') {
            locale = args[++i];
        } else if (args[i] === '--stub-bridge') {
            stubBridge = true;
        } else {
            positional.push(args[i]);
        }
    }
    if (positional.length !== 2) {
        console.error('usage: node tests/e2e/make_scenario.js <scenario.lua> <out.json> [--locale enUS] [--stub-bridge]');
        process.exit(2);
    }
    const [scenarioFile, outFile] = positional;
    const L = run.newState();
    setGlobalString(L, 'LOCALE_UNDER_TEST', locale);
    try {
        run.runLua(L, fs.readFileSync(path.join(__dirname, '..', 'mock.lua'), 'utf8'), 'mock.lua');
        run.runLua(L, run.BIT_SHIM, 'bitshim');
        setAddonGlobals(L, run.addonSources(stubBridge));
        run.runLua(L, fs.readFileSync(path.join(__dirname, 'scenario_lib.lua'), 'utf8'), 'scenario_lib.lua');
        run.runLua(L, fs.readFileSync(scenarioFile, 'utf8'), path.basename(scenarioFile));
    } catch (e) {
        console.error('lua error: ' + e.message);
        process.exit(1);
    }
    const json = getGlobalString(L, 'E2E_JSON');
    if (!json) {
        console.error('the scenario did not call E2E.Finish()');
        process.exit(1);
    }
    const doc = JSON.parse(json); // validates the encoder's output
    fs.mkdirSync(path.dirname(path.resolve(outFile)), { recursive: true });
    fs.writeFileSync(outFile, json);
    const sent = doc.events.filter((e) => e.sent);
    const kinds = {};
    for (const e of sent) kinds[e.sent.kind] = (kinds[e.sent.kind] || 0) + 1;
    console.log(JSON.stringify({
        scenario: doc.name, out: outFile, durationMs: doc.durationMs, events: doc.events.length,
        strips: doc.events.filter((e) => e.strip && e.strip.shown).length, sent: kinds, bytes: json.length,
    }));
}

main();
