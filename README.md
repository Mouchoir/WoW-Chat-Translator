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

Status: development build (addon 0.5.0, protocol build 4; Translate Companion 0.4.0). See
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
- Nothing comes back into the game, so the addon cannot know whether Translate Companion runs.
  Under every card that waits for it, the addon writes a gold hint 1.5 s after the click ("No
  translation above? Start Translate Companion." and a `[Download]` link that opens a small dialog
  with the download address, ready to copy). Translate Companion paints over that line whenever it
  paints the card, so the hint shows only when it is not running. It also follows that line when
  newer messages push the card's rows out of the top of the chat. In the conversation window a
  bubble's hint hides while the window is dragged or resized and comes back 1.5 s after any move.
- Translate Companion starts the translator on the first click (on the GPU when enough video
  memory is free, else on the CPU) and unloads it after 10 minutes without use.
- The conversation window works the same way: each bubble carries tagged rows that Translate
  Companion paints over, so the translation sits in the bubble in light blue.

Design: `docs\DESIGN.md`. Components: `docs\ADDON.md`, `docs\BRIDGE.md`, `docs\ENGINE.md`,
`docs\HELPER.md`.

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
   from its icon (`taskkill /IM WoWTranslateCompanion.exe` also closes it cleanly). It can be
   started before or after a click, no `/reload` needed.
6. In game: `/tpr test`.

## Translate Companion

**Starting it:** double-click `WoWTranslateCompanion.exe` (a development build is in
`helper\app\target\release`). No console opens; a small icon appears in the notification area
(its tooltip: "Translate Companion 0.4.0 - ready, translator not loaded"). It finds the translator
runtime from `runtimeDir` in `%APPDATA%\TranslateProbe\settings.json` (set it once; else the Ollama
install) and the model in the Ollama store, so no argument is needed. Start it before or after
WoW, before or after a click: no `/reload`. To stop it: its icon, Exit (a second copy refuses to
start while one runs). In game, `/tpr app` prints the download address.

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
  never chat text, translations or names. Translations stay in memory.
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
| The rows stay dark, with a gold "Start Translate Companion" line under them | Translate Companion is not running: start it (`/tpr app`, or `[Download]` for the link); check its tray icon; `/tpr test` should show a "bridge OK" message in the top-left corner |
| The gold hint shows under a translation | something covers that line (a tooltip, a frame), or Translate Companion is older than the addon (it then shows "older than the TranslateProbe addon" once): update it ("Get updates" in its tray menu); over a very busy world behind a transparent chat a few columns may stay, report them (`docs\IN-GAME-CHECKS.md`, G19) |
| "Not received - click [T] again" | Translate Companion started after the click, or missed the strip: click `[T]` again |
| "Not received - click the bubble" (window) | the same in the conversation window: click the bubble |
| "Ready (not copied...)" on a reply | the clipboard was refused in the dialog (`/tpr clip ask` asks again) or is off in Translate Companion's tray menu: type the translation shown, or allow the clipboard |
| "Not copied - you copied something else" | you copied something after clicking Translate: Translate Companion left it alone; click Translate again |
| No status line above a reply ("No status above? ...") | start or update Translate Companion (a version older than the addon cannot read replies) |
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
  card included (scenario `conv`), and that it covers every hint lane while it paints the card.
  Every automated run uses `--no-overlay --no-clipboard`: no window is shown and the clipboard is
  never touched.
- `npm install` uses the lock file; everything else builds offline (`cargo --offline`).
