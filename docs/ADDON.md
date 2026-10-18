# Addon (Lua side of the exe route)

The addon no longer translates. A [T] click reserves rows under the chat line (a colour tag and a
dark plate per row) and sends a TRANSLATE request over the pixel bridge (`docs\BRIDGE.md`);
TranslateProbe Helper draws the translation on the plates. Modules, in TOC order:

| File | Role |
|---|---|
| `Locales.lua` | `ns.L`, English base and frFR override |
| `Core.lua` | saved variables (whitelist purge, schema 2), session id, print helpers, inits under `pcall` |
| `Bridge.lua` | pixel strip encoder, message builders, tag and plate markup (bridge package) |
| `Glossary.lua` | abbreviations: glossary row (`Describe`), prompt pairs (`Extract`), marked text (`PromptText`) |
| `Lang.lua` | targets, default target, script of a line |
| `Reserve.lua` | row metrics, row count estimate, reserved rows, slots, VIEW watcher, old-session cleanup |
| `ChatButton.lua` | the [T] link, rows under a line (`SetSuffix`), link clicks and tooltips |
| `Request.lua` | the click flow and active requests (memory only) |
| `Restriction.lua` | chat restrictions: deferred clicks, lines that arrived secret, [Show them] |
| `Commands.lua` | `/tpr` (`/tprobe`) commands |

Saved variables keep only `schema`, `target`, `names`, `glossShow`, `mode`, `glossary` and
`helpShown`. Message text, sender names, line ids, request ids and slots are never stored.

## Tests

- `node tests\run.js enUS` and `node tests\run.js frFR`: the harness (`tests\harness.lua`) on the
  mock client (`tests\mock.lua`) with the real `Bridge.lua`.
- Add `--stub-bridge` (or set `TPROBE_STUB_BRIDGE=1`) to load `tests\stubs\Bridge.lua` instead. A
  stub run also checks that the stub builds the same bytes as the real `Bridge.lua`.
- `luacheck addon`.

The mock API that `tests\e2e` relies on is listed at the top of `tests\mock.lua`. `tests\run.js` is
also a module: `require('./run.js')` gives `BIT_SHIM`, `newState`, `runLua`, `tocFiles` and
`addonSources`.

## Decisions and deviations from the design

1. **Who calls `Mock.Load`.** `tests\run.js` loads `mock.lua`, the `bit` shim and the TOC sources
   (`ADDON_FILES`, `ADDON_SOURCES`, `ADDON_FROM`), then runs `harness.lua`, which calls
   `Mock.Load`. Reason: the purge check needs a 70 KB saved-variables fixture in place before
   ADDON_LOADED, and the session check needs old rows in the chat history before
   PLAYER_ENTERING_WORLD.
2. **`--stub-bridge`.** The design loads a stub only for a missing file; the flag forces the stub
   while the real `Bridge.lua` exists, so both can be run.
3. **`Mock.Load` fires ADDON_LOADED, PLAYER_LOGIN, then PLAYER_ENTERING_WORLD**, as the client does
   (old-session rows are cut at the first PLAYER_ENTERING_WORLD). `print` also adds the line to
   `DEFAULT_CHAT_FRAME`'s history, and chat frames keep 128 entries, as on the client.
4. **Additions to the C.2 API** (all additive): `ns.VERSION`; `ChatButton.CutRows`,
   `ChatButton.LINK_PREFIX`, `ChatButton.SUFFIX_START`; `Reserve.Measure`, `Reserve.SyncView(force)`,
   `Reserve.SlotsInUse`, `Reserve.IsWatching`, `Reserve.Counters`; `Request.Drop`, `Request.Get`,
   `Request.Counters`; `Restriction.ClearDeferred`, and `Restriction.Defer(lineID, nil)` cancels.
   Reason: when the read-only scan frees the slot of a line that left the chat, `Request` must
   forget that request (`Drop` sends CANCEL); "send VIEW when it changed or on the first send"
   needs state shared by the click flow and the watcher (`SyncView`).
5. **`Request.ClearAll`** removes rows with one `CutRows` pass over every row block of this
   session (including waiting rows and `/tpr test rows` rows), instead of `SetSuffix(nil)` per
   active line. Same result for active lines.
6. **An empty VIEW is never sent.** When no chat rectangle is readable (secret during a
   restriction, or no chat frame shown), nothing is sent and the helper keeps the last VIEW. An
   empty VIEW would erase the sticky one.
7. **Restriction details.** "Active" is tracked per restriction type. Each deferred click runs in
   its own timer callback, so an error in one cannot stop the others. Lines read back after a
   restriction skip the player's own lines (sender GUID) and lines without words. If chat is still
   locked at +1 s, the +30 s attempt retries; a generation counter makes only the latest Inactive
   event act.
8. **Hiding rows also cancels a waiting click** for that line, and a waiting click whose line is
   gone removes its "Waiting for the end of the restriction..." row.
9. **`/tpr gloss add` keeps meanings to 120 bytes**, because the purge drops longer values at the
   next load.
10. **`/tpr info`** also prints the client build line.
11. **`.luacheckrc`**: besides the listed removals, read_globals that nothing references any more
    are gone too (`C_AddOnProfiler`, `UpdateAddOnMemoryUsage`, `GetAddOnMemoryUsage`,
    `C_RestrictedActions`).
12. **`Glossary.PromptText`** first deletes any 0x01-0x03 byte from the raw text (a player cannot
    forge a mark) and uses 0x01 internally for the escaped pipe `||`.

13. **Row estimate without rounding first (review fix).** The design rounds `rowsSrc` up to a
    whole row before applying the script factor, so every Latin-to-Latin line reserved at least 2
    rows, even "gg" (measured: "gg" at 5% of the plate gave K = 2). `Reserve.EstimateRows` now uses
    the unrounded width ratio: `K = min(4, max(1, ceil(width / plateW * f - 0.15)))`. Short lines
    reserve 1 row; long lines are unchanged.
14. **Periodic read-only scan (review fix).** The design reclaims slots only when all 127 are in
    use, so after one click the 2 Hz VIEW watcher ran for the rest of the session (the idle state
    of C.5, no ticker, was never reached) and the click that finally triggered the scan queued up
    to 126 CANCELs ahead of its own TRANSLATE. The watcher now also runs `Reserve.Scan` at most
    every 15 s while slots are in use; slots of lines gone from every chat frame are freed a few at
    a time (one CANCEL each) and the watch stops when no request is left. The scan predicate
    still returns false for every entry, so nothing is repackaged and nothing un-fades.
15. **Secret arrivals (review fixes).**
    - A line counts as arrived secret when any argument of its chat event is secret
      (`canaccessvalue(text, ...)`), because the client skips the message filters in that case,
      not only when the text is secret. The mock now applies the same rule to all arguments.
    - Before reading lines back, `Restriction` keeps only lines that some chat frame still shows
      (`ChatButton.ShownLines`, a read-only `TransformMessages` scan of `eventArgs[11]`). A
      channel no tab displays is never printed into the main chat, as on the readable path.
16. **Mode `auto` follows the colourblind filter (review fix).** `Bridge.Init` registers
    `CVAR_UPDATE`; when `colorblindSimulator` changes and the requested mode is `auto`, the mode
    is resolved again (mono or dense). The mock's `SetCVar` fires `CVAR_UPDATE` like the client.
17. **`{...}` groups are counted in UTF-8 characters** (1-32), as the design says; the first
    version counted bytes, which left out 12 Hangul characters.
18. **`Glossary.Lookup` removed.** Nothing called it (the old `Describe` uses `Find`); the design
    listed it as kept unchanged.
