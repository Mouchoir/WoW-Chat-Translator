# TranslateProbe

A WoW Forever addon and a small Windows app that translate player chat lines on your own PC.

Click the `[T]` link after a player message: the **conversation window** opens with one tab per
player, and **Translate Companion** (`WoWTranslateCompanion.exe`), a small Windows app running
beside the game, writes the translation in the player's chat bubble. You can answer from the same
window: write in your language, Translate Companion translates your reply into the other player's
language and puts it in the clipboard, you paste it with Ctrl+V and click the channel to send it.
Ctrl-click `[T]` keeps the older way: a few rows open under the chat line and Translate Companion
writes the translation in them, in the chat's own font over the chat background. The translation
comes from a local AI model (llama.cpp with Qwen3 4B); nothing is sent over the network.
Abbreviations can get an explanation from the addon's glossary (LFM, LF2M, BGs, DM, Stock, HoT...),
with or without Translate Companion.

Download Translate Companion: https://github.com/Mouchoir/WoW-Chat-Translator/releases

Status: development build (addon 0.7.0, protocol build 6; Translate Companion 0.6.0, use them
together). See
`docs\IN-GAME-CHECKS.md` for the checks that only the real client can make. Translate Companion was called TranslateProbe
Helper (`TranslateProbeHelper.exe`) until 2026-10-19; the addon keeps its name, TranslateProbe.

## How it works

- The addon cannot send data out of the game, so it talks to Translate Companion through pixels:
  on each click a thin strip of coloured dots shows for about half a second in the top-left corner
  of the game window. Translate Companion reads it from the window's image
  (Windows.Graphics.Capture, or a screen copy as a fallback).
- Each reserved row starts with a small colour tag that names it. Translate Companion finds the
  tags in every capture and draws the translation only on the dark plate pixels it can see right
  now, so it follows scrolling, fading and tab switches, and never paints over a tooltip or a menu.
  The tags and the dark plates are painted over with the chat background sampled next to them, so
  only the translation shows (the "native look"). Without Translate Companion the rows stay dark.
- Before anything opens, the addon checks that Translate Companion is there (see "Is Translate
  Companion running?" below). Without it, a `[T]` click opens only a small popup in the middle of the
  screen that offers to start it or download it: no window, no rows, nothing in the chat. With it,
  `[T]` opens the translation at once.
- Translate Companion starts the translator on the first click (on the GPU when enough video
  memory is free, else on the CPU) and unloads it after 10 minutes without use.
- The conversation window works the same way: each bubble carries tagged rows that Translate
  Companion paints over, so the translation sits in the bubble in light blue.
- Link names (items, spells, professions, map pins...) are copied into the translation as they
  are and painted in their own colours (a rare item in blue); IDs, web and Discord addresses and
  e-mail addresses are copied exactly as written, while levels, prices and times are translated.

Design: `docs\DESIGN.md`. Components: `docs\ADDON.md`, `docs\BRIDGE.md`, `docs\ENGINE.md`,
`docs\HELPER.md`.

## Is Translate Companion running?

Nothing comes back into the game while you play: an addon cannot read files, the network, the
clipboard or the screen. The game only reads the addon's own files, at login and at every `/reload`.
So the addon ships an empty `CompanionStatus.lua`, and Translate Companion writes into the copy in
your AddOns folder (that one file, nothing else) when it starts, every minute while it runs, and
"stopped" when you exit it. It finds the folder from the Battle.net launcher's list of games, or from
`wowRoot` in its settings.

- **Translate Companion started before the game, or before a `/reload`:** the addon reads "running"
  and `[T]` opens the translation at once, with no popup.
- **Not running, or never installed:** `[T]` opens only the popup: "Start it on your computer, then
  click Continue", with `[Download]` (the download address, ready to copy) when it was never seen.
  Cancel or Escape closes it and forgets the click.
- **Started while the game runs (no `/reload` needed):** click `[T]`; the popup shows a dark strip,
  and within about two seconds of Translate Companion starting it reads "Translate Companion
  detected" in green (Translate Companion paints it, the way it paints translations). Click Continue:
  the window opens with your click, and later `[T]` clicks act at once until you log out or
  `/reload`. The popup's text and its `[Download]` button stay as they were when it opened: the
  addon cannot learn by itself that Translate Companion started, only the green strip can show it.
- **When `/reload` helps:** only to skip that one popup after starting Translate Companion
  mid-session (the next `/reload` reads "running"). After a deploy or an update of the addon,
  Translate Companion writes its file again within 5 seconds.

## The conversation window

- **Opening:** click `[T]` after a line (Ctrl-click: the rows under the line instead; Shift: translate
  again without the cache). `/tpr conv` or a key binding (Key Bindings > TranslateProbe) shows or
  hides it. It never takes the keyboard focus unless you ask for it in the options, so your
  movement keys keep working.
- **Tabs:** one per player, in the order they were opened, at most 10 shown (the others behind
  "+N"). A tab shows " (N)" while it has unread messages. Close a tab with the small cross at its
  right (on the selected tab and on hover), a middle-click, or its right-click menu. **Closing keeps
  the conversation:** when that player's next message is translated, the tab comes back with the
  whole history. Up to 30 conversations of 100 messages are kept, in memory only.
- **Bubbles:** the translation in the player's bubble, with the channel and the time above it and
  an "Original" link. Click a bubble to translate it again (more rows when it was cut, and the fix
  for "Not received - click the bubble"); Shift-click: without the cache; right-click: translate
  again, copy the original, remove.
- **Links:** under a bubble whose message has links (items, spells, professions, map pins,
  quests, achievements...), a bar shows them in their own colours: hover for the tooltip, click as
  in the chat (a map pin opens the map, a profession its recipes), Shift-click to put the link in
  your chat message, Ctrl-click to try an item on. The links of the "Original" text and of your own
  whispers work the same way.
- **New messages from the same player** (option "Translate new messages", default "smart"):
  whispers and conversations you replied to are translated as they come while the window shows
  them; a first message in a public channel is added untranslated with a Translate button.
- **Replying:** type your reply in the box at the bottom and press Enter (or click "Translate to
  XX"). The first time, a dialog asks whether Translate Companion may put translated replies in the
  Windows clipboard ("Remember my choice" keeps the answer; `/tpr clip` changes it). Translate
  Companion empties the clipboard at once, then shows "Translating..." and, when the translation is
  in the clipboard, "Ready - Ctrl+V to paste" in green. Press Ctrl+V in the box, check the text, then
  click a channel button (the one the player used is highlighted). **Enter never sends**: only a
  click on a channel button does. Without the clipboard ("Deny"), the translation is shown above the
  box to type it yourself.

## Install (development)

1. The game may stay open: `/reload` picks up changed and new addon Lua files (measured on
   WoW Forever). New texture files were not tested.
2. `dev.cmd` mirrors `addon\TranslateProbe` into the AddOns folder given by `-WowAddOnsPath`,
   `TPROBE_WOW_ADDONS_PATH`, or a gitignored `scripts\dev.local.ps1` that sets `$WowAddOnsPaths`.
3. Build Translate Companion (Rust, offline): `scripts\helper.ps1 -Build` gives
   `helper\app\target\release\WoWTranslateCompanion.exe`. Exit an old `TranslateProbeHelper.exe`
   first: both take the same single-instance lock, so the second one to start exits at once.
4. Translate Companion needs a llama.cpp runtime folder (with `llama-server.exe`) and the model:
   - runtime: `-RuntimeDir`, or `$LlamaRuntimeDir` in `scripts\dev.local.ps1`, or the
     `TPROBE_LLAMA_RUNTIME` environment variable, else the Ollama install
     (`%LOCALAPPDATA%\Programs\Ollama\lib\ollama`);
   - model: found in the Ollama store (`ollama pull hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`), or
     `--model <file.gguf>`.
5. Start the game (Windowed or Windowed (Fullscreen)), then `scripts\helper.ps1 -Run` (add
   `-Diag` for a diagnostics file). Translate Companion sits in the notification area; exit it
   from its icon (`taskkill /IM WoWTranslateCompanion.exe` also closes it cleanly). Started before
   the game (or before a `/reload`), `[T]` works at once; started later, the popup a `[T]` click
   opens shows it as detected, and Continue goes on (see "Is Translate Companion running?").
6. In game: `/tpr test`.

## Translate Companion

**Starting it:** double-click `WoWTranslateCompanion.exe` (a development build is in
`helper\app\target\release`). No console opens; a small icon appears in the notification area
(its tooltip: "Translate Companion 0.6.0 - ready, translator not loaded"). It finds the translator
runtime from `runtimeDir` in `%APPDATA%\TranslateProbe\settings.json` (set it once; else the Ollama
install) and the model in the Ollama store, so no argument is needed. Start it before WoW and `[T]`
works at once; started later, confirm it once in the popup (above). It writes its status into
`Interface\AddOns\TranslateProbe\CompanionStatus.lua` of each WoW install that has the addon (from
the Battle.net launcher's database, or `wowRoot` in the settings file for an install it does not
know), never anywhere else, and never into a folder that is a link. To stop it: its icon, Exit (a
second copy refuses to start while one runs). In game, `/tpr app` prints the download address and
what the status file said.

| Command | What it does |
|---|---|
| `scripts\helper.ps1 -Build` | release build of `helper\app`: `WoWTranslateCompanion.exe` (about 1.3 MB, Windows DLLs only) |
| `scripts\helper.ps1 -Test` | `cargo test --offline` in the four helper crates |
| `scripts\helper.ps1 -Run [-Diag] [args]` | starts Translate Companion; extra arguments go to the exe (`--capture gdi`, `--backend cpu`...) |
| `WoWTranslateCompanion.exe --selftest` | prints what it found (runtime, model, GPU, RAM, flags) without loading the model |
| `WoWTranslateCompanion.exe --version` / `--help` | the version, or the options and the download address |

Settings: `%APPDATA%\TranslateProbe\settings.json`. Logs: `%LOCALAPPDATA%\TranslateProbe\logs`
(the folders keep the project's name, so settings and logs carry over from TranslateProbe Helper).
The tray icon's tooltip shows the version and the translator's state. Its menu has "Unload
translator now", "Get updates" (opens https://github.com/Mouchoir/WoW-Chat-Translator/releases in
the browser), "Open log folder", "Copy translated replies to the clipboard" (on: the in-game
consent is the approval; off: replies are shown, never copied), "Sound when a reply is copied"
(off) and "Exit".

## Options panel

`/tpr` (or Esc > Options > AddOns > TranslateProbe) opens the addon's page in the game settings:

- **Target language**: the language translations are written in, each listed in its own name
  (English, Français, Deutsch, Español, Italiano, Português, Русский, 한국어, 简体中文, 繁體中文).
- **Explain abbreviations** (off by default) and **Spell names in Latin letters**: the extra rows a
  click adds under a line (`/tpr show`, `/tpr names`); the glossary row has its own pale gold
  colour. In the conversation window they go under the bubble and in the title.
- **Conversation window:** open `[T]` translations in the window, "Translate new messages" (off,
  add, smart, always), "Copy translated replies" (ask, allow, never), originals under every bubble,
  text size, window scale, hide the window during combat, put the cursor in the reply box, open the
  window for a new whisper, and "Reset window position".
- **Bridge mode** (advanced): see `/tpr mode`.
- **Glossary** page: every abbreviation with its meaning in the current target language. Type a
  meaning to change it, clear it to remove it, Delete your own entries, Disable or Enable the
  built-in ones, and add new abbreviations at the bottom. Abbreviations stay when the target
  changes; meanings follow the target (empty when none was typed for it). Built-in meanings exist
  in English and French; other targets get the English one, marked "in English", until you
  type your own.

## Commands (`/tpr` or `/tprobe`)

| Command | What it does |
|---|---|
| `/tpr`, `/tpr options` | open the options panel (without the settings panel: the command list) |
| `/tpr help` | the command list |
| `/tpr info` | version, session, target, bridge mode, screen, chat frames |
| `/tpr target <code>` | language translations are written in: en, fr, de, es, it, pt, ru, ko, zh, zh-TW (default: the client language) |
| `/tpr show on\|off` | a click also shows the glossary row (off by default) |
| `/tpr names on\|off` | a click also spells a non-Latin sender name in Latin letters (off by default) |
| `/tpr mode auto\|dense\|robust\|mono` | how the addon talks to Translate Companion: `auto` uses `mono` while the colourblind filter is on, else `dense`; try `robust` when Translate Companion reports a weak signal |
| `/tpr translit <text>` | spell any text in Latin letters (the client's transliterator) |
| `/tpr gloss add <abbr> <meaning>` | add or replace a glossary entry for the current target language (several meanings: separate them with `/`) |
| `/tpr gloss del <abbr>` | remove your entry, or disable a built-in one |
| `/tpr gloss list` / `test <text>` | your entries with their meaning in the current target language / what a message would give |
| `/tpr test` | checks the whole setup on a sample line |
| `/tpr test rows` | checks the rows reserved under a line (Translate Companion writes "Overlay OK") |
| `/tpr test lockdown` | what the addon can read during a chat restriction |
| `/tpr conv` | show or hide the conversation window |
| `/tpr conv clear` | forget every conversation |
| `/tpr conv reset` | put the window back at its default place |
| `/tpr display window\|inline` | what a `[T]` click opens (Ctrl-click opens the other one) |
| `/tpr clip ask\|allow\|never` | whether translated replies may go to the clipboard |
| `/tpr test paste` | probe of the reply box for the in-game checks (prints counts only; again to stop) |
| `/tpr clear` | removes every translation under the chat lines and forgets every conversation |
| `/tpr app` | about Translate Companion, with its download address |
| `/tpr debug` | counters of this session |

Click `[T]` to translate (in the window), Ctrl-click for the rows under the line, Shift-click to
translate again without the cache; click a translation under a line to hide it. Lines made only of
links or icons get no `[T]`.

## Privacy

- Translations run on this PC; Translate Companion opens no network connection except to its own
  translator process on 127.0.0.1 (with a random key). "Get updates" only opens the releases page
  in your browser.
- The addon's saved variables keep only your settings and your glossary entries: never message
  text, sender names or line ids. Older telemetry is deleted at load.
- **Conversations are kept in memory only**: messages, translation requests, drafts, sent
  replies, names and channels are lost on `/reload` and logout, and never written to the saved
  variables.
- The clipboard is touched only for your own replies, only after you allowed it in the game's
  dialog and while Translate Companion's tray switch is on. Translate Companion never reads what is
  in the clipboard; it empties it when you click Translate, writes the translation kept out of the
  Win+V history and the cloud clipboard, and empties it again after the send only if it still holds
  its own text. Messages you receive are never copied.
- Translate Companion's log and diagnostics files hold counters, sizes, timings and positions only,
  never chat text, translations or names. Translations stay in memory. The status file it writes
  into the addon folder holds its version, the protocol build, "running" or "stopped" and the time
  of the last write, nothing else.
- During a chat restriction (boss fights, rated content), lines that arrived unreadable are never
  read; afterwards one line offers to show them again, with a `[T]`.

## Safety

- Translate Companion never opens the game process, reads its memory or sends it input. It only
  looks at the game window's pixels and draws click-through windows above it that never take the
  focus.
- The addon uses only documented chat and frame APIs.
- The translator runs at below-normal priority, inside a job that ends it with Translate Companion.

## Troubleshooting

| Symptom | What to do |
|---|---|
| `[T]` opens a "Translate Companion" popup | Translate Companion was not running when the game loaded the addon: start it (`[Download]` in the popup for the link), wait for the green "Translate Companion detected" on the popup's strip, then click Continue |
| The popup's strip stays dark after starting Translate Companion | it is not running (check its tray icon), or the game is minimized, or it is older than 0.6.0 (it then shows "Not received - click the bubble" there and an "older than the TranslateProbe addon" toast): update it ("Get updates" in its tray menu) |
| The popup shows although Translate Companion runs | it started after the game loaded the addon: wait for the green strip and click Continue, or `/reload` once; if it shows after a `/reload` too, its status file is not reaching the addon (`/tpr app` says what was read; set `wowRoot` in `%APPDATA%\TranslateProbe\settings.json` to your "World of Warcraft" folder) |
| Coloured blocks and dark rows stay in a bubble ("Nothing painted?" on hover) | Translate Companion is not running or missed the strip: start it, then click the bubble; `/tpr test` should show a "bridge OK" message in the top-left corner |
| A gold "Start Translate Companion" line under a translation | an addon older than 0.7.0 with Translate Companion 0.6.0 (it shows "the addon is older" once): update the addon |
| "Not received - click [T] again" | Translate Companion started after the click, or missed the strip: click `[T]` again |
| "Not received - click the bubble" (window) | the same in the conversation window: click the bubble |
| "Ready (not copied...)" on a reply | the clipboard was refused in the dialog (`/tpr clip ask` asks again) or is off in Translate Companion's tray menu: type the translation shown, or allow the clipboard |
| "Not copied - you copied something else" | you copied something after clicking Translate: Translate Companion left it alone; click Translate again |
| No status line above a reply | start or update Translate Companion (a version older than the addon cannot read replies), then click Translate again |
| No "bridge OK" message | run the game in Windowed (Fullscreen) mode; try `scripts\helper.ps1 -Run --capture gdi`; with strong colour filters try `/tpr mode robust` |
| "Translator files not found" | install Ollama and pull `hf.co/Qwen/Qwen3-4B-GGUF:Q4_K_M`, or start Translate Companion with `--runtime` and `--model` |
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
- `tests\e2e` plays scripted scenarios in a stand-in game window and checks what Translate
  Companion finds and draws (`tests\e2e\README.md`), the conversation window's bubbles and reply
  card included (scenario `conv`), and the popup's status card, with Translate Companion running
  before the popup (`presence`) or started while it shows (`presence_late`).
  Every automated run uses `--no-overlay --no-clipboard`: no window is shown and the clipboard is
  never touched.
- `npm install` uses the lock file; everything else builds offline (`cargo --offline`).

## License

Copyright (C) 2026 Mouchoir.

This project (the TranslateProbe addon and Translate Companion) is free software: you can
redistribute it and/or modify it under the terms of the GNU Affero General Public License as
published by the Free Software Foundation, either version 3 of the License, or (at your option)
any later version. It is distributed WITHOUT ANY WARRANTY; see [LICENSE](LICENSE) for details.

The translation model (Qwen3-4B, Apache-2.0) and the llama.cpp runtime (MIT) are not part of this
repository; they keep their own licenses.
