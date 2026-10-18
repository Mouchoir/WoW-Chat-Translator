# TranslateProbe

A WoW Forever addon and a small Windows app that translate player chat lines on your own PC.

Click the `[T]` link after a player message: a few dark rows open under the line, and
**TranslateProbe Helper**, running beside the game, writes the translation in them. The
translation comes from a local AI model (llama.cpp with Qwen3 4B); nothing is sent over the
network. Abbreviations get an instant explanation row from the addon's glossary (LFM, LF2M, BGs,
DM, Stock, HoT...), with or without the Helper.

Status: development build (addon 0.2.0, Helper 0.1.0). See `docs\IN-GAME-CHECKS.md` for the
checks that only the real client can make.

## How it works

- The addon cannot send data out of the game, so it talks to the Helper through pixels: on each
  click a thin strip of coloured dots shows for about half a second in the top-left corner of the
  game window. The Helper reads it from the window's image (Windows.Graphics.Capture, or a screen
  copy as a fallback).
- Each reserved row starts with a small colour tag that names it. The Helper finds the tags in
  every capture and draws the translation only on the dark plate pixels it can see right now, so
  it follows scrolling, fading and tab switches, and never paints over a tooltip or a menu. Once
  read, the tags are painted over.
- The Helper starts the translator on the first click (on the GPU when enough video memory is
  free, else on the CPU) and unloads it after 10 minutes without use.

Design: `docs\DESIGN.md`. Components: `docs\ADDON.md`, `docs\BRIDGE.md`, `docs\ENGINE.md`,
`docs\HELPER.md`.

## Install (development)

1. The game may stay open: `/reload` picks up changed and new addon Lua files (measured on
   WoW Forever). New texture files were not tested.
2. `dev.cmd` mirrors `addon\TranslateProbe` into the AddOns folder given by `-WowAddOnsPath`,
   `TPROBE_WOW_ADDONS_PATH`, or a gitignored `scripts\dev.local.ps1` that sets `$WowAddOnsPaths`.
3. Build the Helper (Rust, offline): `scripts\helper.ps1 -Build`.
4. The Helper needs a llama.cpp runtime folder (with `llama-server.exe`) and the model:
   - runtime: `-RuntimeDir`, or `$LlamaRuntimeDir` in `scripts\dev.local.ps1`, or the
     `TPROBE_LLAMA_RUNTIME` environment variable, else the Ollama install
     (`%LOCALAPPDATA%\Programs\Ollama\lib\ollama`);
   - model: found in the Ollama store (`ollama pull hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`), or
     `--model <file.gguf>`.
5. Start the game (Windowed or Windowed (Fullscreen)), then `scripts\helper.ps1 -Run` (add
   `-Diag` for a diagnostics file). The Helper sits in the notification area; exit it from its
   icon. It can be started before or after a click, no `/reload` needed.
6. In game: `/tpr test`.

## The Helper

| Command | What it does |
|---|---|
| `scripts\helper.ps1 -Build` | release build of `helper\app` (about 1.1 MB, Windows DLLs only) |
| `scripts\helper.ps1 -Test` | `cargo test --offline` in the four helper crates |
| `scripts\helper.ps1 -Run [-Diag] [args]` | starts the Helper; extra arguments go to the exe (`--capture gdi`, `--backend cpu`...) |
| `TranslateProbeHelper.exe --selftest` | prints what it found (runtime, model, GPU, RAM, flags) without loading the model |

Settings: `%APPDATA%\TranslateProbe\settings.json`. Logs: `%LOCALAPPDATA%\TranslateProbe\logs`.
The tray menu has "Unload translator now", "Open log folder" and "Exit".

## Commands (`/tpr` or `/tprobe`)

| Command | What it does |
|---|---|
| `/tpr` | help |
| `/tpr info` | version, session, target, bridge mode, screen, chat frames |
| `/tpr target <code>` | language translations are written in: en, fr, de, es, it, pt, ru, ko, zh, zh-TW (default: the client language) |
| `/tpr show on\|off` | a click also shows the glossary row (on by default) |
| `/tpr names on\|off` | a click also spells a non-Latin sender name in Latin letters (off by default) |
| `/tpr mode auto\|dense\|robust\|mono` | how the addon talks to the Helper: `auto` uses `mono` while the colourblind filter is on, else `dense`; try `robust` when the Helper reports a weak signal |
| `/tpr translit <text>` | spell any text in Latin letters (the client's transliterator) |
| `/tpr gloss add <abbr> <meaning>` | add or replace a glossary entry (several meanings: separate them with `/`) |
| `/tpr gloss del <abbr>` | remove your entry, or disable a built-in one |
| `/tpr gloss list` / `test <text>` | your entries / what a message would give |
| `/tpr test` | checks the whole setup on a sample line |
| `/tpr test rows` | checks the rows reserved under a line (the Helper writes "Overlay OK") |
| `/tpr test lockdown` | what the addon can read during a chat restriction |
| `/tpr clear` | removes every translation under the chat lines |
| `/tpr app` | about the Helper |
| `/tpr debug` | counters of this session |

Click `[T]` to translate, Shift-click to translate again, click a translation to hide it. Lines
made only of links or icons get no `[T]`.

## Privacy

- Translations run on this PC; the Helper opens no network connection except to its own
  translator process on 127.0.0.1 (with a random key).
- The addon's saved variables keep only your settings and your glossary entries: never message
  text, sender names or line ids. Older telemetry is deleted at load.
- The Helper's log and diagnostics files hold counters, sizes, timings and positions only, never
  chat text, translations or names. Translations stay in memory.
- During a chat restriction (boss fights, rated content), lines that arrived unreadable are never
  read; afterwards one line offers to show them again, with a `[T]`.

## Safety

- The Helper never opens the game process, reads its memory or sends it input. It only looks at
  the game window's pixels and draws click-through windows above it that never take the focus.
- The addon uses only documented chat and frame APIs.
- The translator runs at below-normal priority, inside a job that ends it with the Helper.

## Troubleshooting

| Symptom | What to do |
|---|---|
| The rows stay dark | start the Helper (`/tpr app`); check its tray icon; `/tpr test` should show a "bridge OK" message in the top-left corner |
| "Not received - click [T] again" | the Helper started after the click, or missed the strip: click `[T]` again |
| No "bridge OK" message | run the game in Windowed (Fullscreen) mode; try `scripts\helper.ps1 -Run --capture gdi`; with strong colour filters try `/tpr mode robust` |
| "Translator files not found" | install Ollama and pull `hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`, or start the Helper with `--runtime` and `--model` |
| "Not enough free memory for the translator" | free some RAM (the CPU translator needs about 3.6 GB available) |
| Slow first translation | the translator is loading (a few seconds on the CPU; the first GPU run prepares its shaders, up to a minute) |
| Anything else | `scripts\helper.ps1 -Run -Diag` and send the files from `%LOCALAPPDATA%\TranslateProbe\logs` |

## Tests

```
npm install
npm test
luacheck addon
scripts\helper.ps1 -Test
python -I tests\e2e\run.py --all --capture wgc
```

- `npm test` runs the addon on a mock client in English and French (`tests\run.js`,
  `tests\harness.lua`, `tests\mock.lua`) and checks the Lua bridge byte for byte against the
  reference vectors (`tests\bridge\run.js`).
- The Python tools in `tools\bridge` hold the bridge reference, the vector generator, the
  simulation and the CRC Monte Carlo.
- `tests\e2e` plays scripted scenarios in a stand-in game window and checks what the Helper finds
  and draws (`tests\e2e\README.md`).
- `npm install` uses the lock file; everything else builds offline (`cargo --offline`).
