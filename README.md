# TranslateProbe

Dev-only WoW Forever addon. It answers, inside the real client, whether a **fully local chat translator** (a `[T]` button at the end of each player message, translation shown under that line) can work:

1. **Can the addon read player chat?** Forever runs the 12.x engine: every player `CHAT_MSG_*` text becomes a *secret value* during "chat messaging lockdown". The probe logs, per context read from the client (instance type, boss fight via `C_InstanceEncounter`, battleground match phase via `C_PvP.GetActiveMatchState`), how many lines were readable or secret, announces each restriction start and end in chat, and re-reads secret lines by `lineID` 1 s and 30 s after the lockdown ends, grouped by the context they arrived in.
2. **How fast is WoW's Lua?** `/tprobe bench` measures multiply-adds per second for the kernels a neural model needs, and memory per table slot and per string byte (via `C_AddOnProfiler.MeasureCall`).
3. **How long would a real model take?** `/tprobe model` and the `[T]` button run a model with the exact shape of Mozilla's Firefox Translations "tiny" student (16.9M parameters, int8 weights in strings, 6-layer encoder, 2-layer SSRU decoder, 500-row shortlist) with random weights, time-sliced a few ms per frame inside the client's per-addon script budget (`GetScriptBucketThrottleLimits`).

It also contains the instant layer of the future translator: a **glossary** of WoW chat abbreviations (LFM, LF2M, BGs, DM, Stock, HoT...) matched as whole words, case ignored, with your own entries on top.

The probe never stores message text or sender names: only counts, flags and timings.

## Install

```
dev.cmd
```

`scripts\dev.ps1` mirrors `addon\TranslateProbe` into the WoW AddOns folder given by `-WowAddOnsPath`, `TPROBE_WOW_ADDONS_PATH`, or a gitignored `scripts\dev.local.ps1` that sets `$WowAddOnsPaths`. Then `/reload` in game.

## Commands

| Command | What it does |
|---|---|
| `/tprobe info` | Client build, script budget limits, background slice, chat lockdown, active restrictions, CPU and memory of the addon |
| `/tprobe bench` | Lua speed (3 kernels) and memory costs, about 20 s, results saved |
| `/tprobe model [pieces]` | One simulated model pass (default 8 pieces) |
| `/tprobe budget <ms>` | Lua time per frame for background work (default 4, capped at half the per-second and burst script allowances; below 0.5 ms per frame it skips frames instead) |
| `/tprobe log on\|off` | Chat readability log (on by default) |
| `/tprobe simulate on\|off` | Whether a `[T]` click also times the simulated model (on by default) |
| `/tprobe names on\|off` | Whether a `[T]` click also spells a non-Latin sender name in Latin letters, with the client's ICU transliterator `C_Intl.Transliterate(name, "Any-Latin")` (off by default) |
| `/tprobe translit <text>` | Spell any text in Latin letters, to see which scripts the client can transliterate |
| `/tprobe gloss add <abbr> <meaning>` | Add or replace a glossary entry; separate several meanings with `/` |
| `/tprobe gloss del <abbr>` | Remove your entry, or disable a built-in one |
| `/tprobe gloss list` / `test <text>` | Your entries / what a message would give |
| `/tprobe report` | Readability log summary, catch-up results, last bench |
| `/tprobe reset` / `free` | Clear logs and results / release the simulated model |

Lines made only of links or icons (a shared quest, an item) get no `[T]`. Click `[T]` after a player message: the glossary line appears under it at once, then the simulated model timing (the first click of a session builds the model first, in a separate job that is not counted). Click the lines to hide them, `[T]` again to show them, Shift-click `[T]` to run again.

## In-game test session

1. `/tprobe info` once, in a city.
2. `/tprobe bench`, standing still, then again in a busy city.
3. Click `[T]` on a few Trade or General lines (short and long).
4. Run a dungeon: watch the restriction lines in chat at the entrance, on trash, on boss pulls and after the last boss.
5. A battleground if possible: before the gates open, during the match, on the scoreboard.
6. `/tprobe report`, then `/reload` or log out so `WTF\Account\<account>\SavedVariables\TranslateProbe.lua` is written, and share that file.

`/console addonChatRestrictionsForced 1` simulates the chat lockdown in the open world (set it back to 0 afterwards).

## Tests

```
npm install
npm test
```

`tests/run.js` runs the real addon files in fengari (Lua 5.3) against a mock Forever client (`tests/harness.lua`): chat pipeline with message filters and history entries carrying `eventArgs[11]`, secret values that raise on misuse, restrictions, timers and frames. Lint with `luacheck addon`.
