# Addon (Lua side of the exe route)

The addon no longer translates. A [T] click reserves rows under the chat line (a colour tag and a
dark plate per row) and sends a TRANSLATE request over the pixel bridge (`docs\BRIDGE.md`);
TranslateProbe Helper draws the translation on the plates. Modules, in TOC order:

| File | Role |
|---|---|
| `Locales.lua` | `ns.L`, English base and frFR override |
| `Core.lua` | saved variables (whitelist purge, schema 3), session id, print helpers, inits under `pcall` |
| `Bridge.lua` | pixel strip encoder, message builders, tag and plate markup (bridge package) |
| `Glossary.lua` | abbreviations: glossary row (`Describe`), prompt pairs (`Extract`), marked text (`PromptText`) |
| `Lang.lua` | targets, default target, script of a line |
| `Reserve.lua` | row metrics, row count estimate, reserved rows, slots, VIEW watcher, old-session cleanup |
| `ChatButton.lua` | the [T] link, rows under a line (`SetSuffix`), link clicks and tooltips |
| `Request.lua` | the click flow and active requests (memory only) |
| `Restriction.lua` | chat restrictions: deferred clicks, lines that arrived secret, [Show them] |
| `Commands.lua` | `/tpr` (`/tprobe`) commands |
| `Options.lua` | the options panel (Esc > Options > AddOns > TranslateProbe) and its glossary page |

Saved variables keep only `schema`, `target`, `names`, `glossShow`, `mode`, `glossary` and
`helpShown`. Message text, sender names, line ids, request ids and slots are never stored. Any
other key is deleted at load, including `display` (the dropped chat tab mode, see decision 20).

### User glossary (`TranslateProbeDB.glossary[key]`)

`key` is a lowercase whole word (`Glossary.KEY_PATTERN`, accents allowed). The value is one of:

| Value | Meaning |
|---|---|
| `false` | a built-in key the user disabled (in the glossary row and in the prompt) |
| `"text"` | one meaning for every target: the format before per-target meanings, still honoured as it is |
| `{ fr = "...", de = "...", ["*"] = "..." }` | a meaning per target; `"*"` is an old every-target string kept for the other targets once one target gets its own; `""` for a target hides the `"*"` meaning there only |
| `{}` | a user abbreviation with no meaning yet (shown empty, skipped in prompts) |

Each meaning is at most 120 bytes (cut on a UTF-8 boundary, pipes become "/"); the purge drops
longer strings and unknown targets inside a table, drops an empty marker when there is no `"*"`
meaning, and drops any other value. Nothing is migrated: an old string stays a string until the
user edits it for one target. `Glossary.Extract`, `Describe` and `Entries` read a value through
`Glossary.UserMeaning(value, target)` (the target's meaning, else `"*"`, else none; `""` is none);
a user key without a meaning for the target is skipped there, a built-in key falls back to its
built-in meaning. `/tpr gloss add` and the panel both write through `Glossary.SetMeaning` (current
target only), `Glossary.Remove` and `Glossary.Enable`.

Clearing a meaning (an empty text) acts on the current target only:
- the target has its own meaning: it is removed, and an every-target (`"*"`) meaning, if any,
  shows again (so an old string comes back unchanged once its edits are cleared);
- the target shows the every-target meaning: an empty marker (`""`) hides it for this target,
  while the other targets keep it (review fix, 2026-10-18: it used to delete it for every
  target).

Built-in meanings are bilingual data (en + fr). The prompt gets the French one for target `fr` and
the English one (a pivot the model reads) for every other target; the panel shows that English
meaning marked "in English" so the user can type their own for that language.

## Options panel

`/tpr` alone and `/tpr options` open it; `/tpr help` prints the commands. Built with the 12.x
Settings API, checked on 2026-10-18 against Gethe/wow-ui-source, branch `live` (line numbers are
approximate: they were read through a page summary, not a local checkout):

| Call | Where |
|---|---|
| `Settings.VarType` (`Boolean`, `String`, `Number`) | `Interface\AddOns\Blizzard_Settings_Shared\Blizzard_Settings.lua:11` |
| `Settings.RegisterAddOnCategory(category)` | same file `:131` |
| `Settings.OpenToCategory(categoryID, scrollToElementName)` | same file `:141` |
| `Settings.RegisterVerticalLayoutCategory(name)` | same file `:151` (returns category, layout) |
| `Settings.RegisterCanvasLayoutSubcategory(parentCategory, frame, name)` | same file `:163` |
| `Settings.RegisterProxySetting(category, variable, variableType, name, default, getValue, setValue)` | same file `:175` |
| `Settings.GetSetting(variable)`, `Settings.NotifyUpdate(variable)` | same file `:199`, `:203` |
| `Settings.CreateControlTextContainer()`, `container:Add(value, label, tooltip)`, `:GetData()` | same file `:228-262` |
| `Settings.CreateCheckbox(category, setting, tooltip)` | same file `:385` |
| `Settings.CreateDropdown(category, setting, options, tooltip)` (`options` is a function returning `GetData()`) | same file `:407` |
| `SettingsCategoryMixin:GetID()` | `Blizzard_Settings_Shared\Blizzard_Category.lua:13` |
| canvas page shown: reparented, `SetAllPoints`, `Show`, then `frame:OnRefresh()` (`CallRefreshOnFrame`); left: `SetParent(nil)`, `Hide` | `Blizzard_Settings_Shared\Blizzard_SettingsPanel.lua:8` and `DisplayLayout` |
| settings calls refuse function arguments (`SecureUnpackArgs`), proxy accessors are wrapped | `Blizzard_Settings_Shared\Blizzard_SettingsInbound.lua:21`, `:50` |
| `InputBoxTemplate` (its `OnEditFocusLost` clears the highlight, so ours is hooked, not set) | `Blizzard_SharedXML\Shared\InputBox\InputBoxTemplates.xml:71` |
| `UIPanelButtonTemplate` | `Blizzard_SharedXML\Mainline\SharedUIPanelTemplates.xml:365` |

- **Main page** (vertical layout): target language dropdown (each language named in its own
  language: `Lang.NATIVE`), glossary row and romanized names checkboxes, bridge mode dropdown
  (advanced). Every control is a proxy setting on `TranslateProbeDB`; the commands go through the
  same setters (`Options.SetTarget`, `Options.SetMode`) and call `Options.Changed(key)`, which
  runs `Settings.NotifyUpdate` so an open panel shows the new value.
- **Glossary page** (canvas subcategory, built the first time it is shown): 12 rows at a time
  (Previous/Next buttons, mouse wheel), user keys first, then the built-in ones. Each row: the
  abbreviation, a mark (yours, edited, all languages, in English, disabled), the meaning for the
  current target in an edit box (Enter or leaving the box saves, Escape restores, an empty text
  removes the meaning for this target), and Delete (user key), Disable or Enable (built-in key).
  An add row takes an abbreviation (one word) and its meaning for the current target. Changing
  the target refreshes the page: the abbreviations stay, the meanings follow.
- A click on a row button or on Add first saves the text being typed in a row (a WoW button
  click does not take the keyboard focus, so the box would keep it while the rows are rebound to
  other entries, and the text would be lost or saved to another abbreviation; review fix).
- `/tpr gloss list` shows each user entry with what the current target gets: its user meaning,
  else the built-in meaning of a built-in key, else "(no meaning in XX)".
- Without the Settings API nothing is registered and `/tpr` prints `OPTIONS_NA` and the commands.

## Tests

- `node tests\run.js enUS` and `node tests\run.js frFR`: the harness (`tests\harness.lua`) on the
  mock client (`tests\mock.lua`) with the real `Bridge.lua`.
- Add `--stub-bridge` (or set `TPROBE_STUB_BRIDGE=1`) to load `tests\stubs\Bridge.lua` instead. A
  stub run also checks that the stub builds the same bytes as the real `Bridge.lua`.
- `luacheck addon`.
- The harness also covers the options panel on the mock Settings API (registration, dropdowns and
  checkboxes, the glossary page per target, old-format entries), the 6-row cap and the removal
  of the chat tab mode. The mock refuses templates and font objects it does not list
  (`Mock.KNOWN_TEMPLATES`, `Mock.KNOWN_FONTS`), so a new one has to be checked in the UI source
  first.

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
    the unrounded width ratio: `K = min(Reserve.MAX_ROWS, max(1, ceil(width / plateW * f - 0.15)))`
    (the cap was 4, see decision 19). Short lines reserve 1 row; long lines are unchanged.
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
19. **Up to 6 reserved rows, Latin-to-Latin factor 1.35 (2026-10-18, in-game fact).** A German line
    translated into French needed more than the 4 rows it got and was cut with "..." in game. The
    cap is now `Reserve.MAX_ROWS = 6` (TRANSLATE rows 1..6, `docs\BRIDGE.md`) and the
    `latin>latin` factor went from 1.25 to 1.35. The tag's row field keeps its 2 bits, so rows 4
    and 5 carry tag rows 0 and 1 (`Bridge.TagDigits` takes `row % 4`); the helper numbers a card's
    rows on from its first visible tag. `Bridge.lua` needed no change (it never checked the range).
    `ns.ADDON_BUILD` is 2 (review fix: a Helper built for 1..4 rows silently drops a 5- or 6-row
    TRANSLATE, a parse error it only counts; with the bump it shows "this Helper is older than the
    addon", and a new Helper tells a build-1 addon to update). The Python reference
    (`tools\bridge\proto.py`) accepts 1..6 too, with a 6-row vector and a refused 7-row one in
    `tests\vectors\messages.json`.
20. **Chat tab display mode dropped (2026-10-18, user decision).** A first attempt of this round
    started a "tab" display mode (a dedicated chat tab with the original and its rows). The user
    prefers a separate conversation window, planned for the next round, so every part of it was
    removed before release: no `Tab.lua`, no `display` setting (an old `display = "tab"` is purged
    at load), no `/tpr show tab|inline` (`/tpr show` takes `on|off` for the glossary row only), no
    tab strings. Nothing of it reached a release.
21. **Per-target user glossary meanings (user request; the design kept `string|false`).** The
    design (B.4) left the user glossary schema unchanged, with one meaning for every target. The
    user asked for meanings that follow the target language, so a value may now also be a table
    of meanings per target (see "User glossary" above). Old values are valid as they are; nothing
    is rewritten at load. The change is one way: a build before it (b8a3e23 and older) accepts
    only `string|false` and deletes every table entry at load, including an old string folded
    into `"*"`. So `db.schema` is 3 since this change (review fix), and `scripts\dev.ps1` copies
    the saved variables to `%LOCALAPPDATA%\TranslateProbe\savedvariables-backup\` before each
    deploy; a rollback restores that copy with the game closed.
22. **`/tpr` alone opens the options panel** (it printed the command list); `/tpr help` prints the
    list, which now also names `/tpr options` and `/tpr help`.
23. **A VIEW before every TRANSLATE (review fix, 2026-10-18).** The design sends VIEW only when the
    chat rectangles changed or on the first send of a session. A Helper started or restarted while
    WoW keeps running then stayed in its discovery scan, whose view is the whole client width
    (the Helper took its background samples from the game world there). `Request` now calls
    `Reserve.SyncView(true)` before each TRANSLATE, re-clicks included: about 15 more bytes on the
    strip, in the same frame as the TRANSLATE. The 2 Hz watcher still sends changes.
