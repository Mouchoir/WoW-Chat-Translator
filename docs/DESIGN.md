# TranslateProbe: design as built

TranslateProbe puts a [T] link after each readable player line in the WoW Forever chat. A click
opens the line in the **conversation window** (one tab per sender; since 2026-10-19, section K), or,
with Ctrl held, reserves a few rows under the line (inline mode), and asks **Translate Companion**
(`WoWTranslateCompanion.exe`; TranslateProbe Helper, `TranslateProbeHelper.exe`, until 2026-10-19;
"the Helper" in these docs and the code), a Windows app running beside the game, to translate the
line with a local AI model (llama.cpp and Qwen3 4B). The Helper draws the translation on the
reserved rows, in the chat or in the window's bubbles. From the window the player can also reply:
the Helper translates the reply and puts it in the clipboard, the player pastes it and clicks the
channel. Nothing leaves the PC, and the Helper never touches the game process: the addon and the
Helper talk through pixels only.

This file is the design as it was built (addon 0.7.0, protocol build 6; Translate Companion 0.6.0;
section L for the hint lanes and the rename of 2026-10-19 (the lanes are gone since build 6),
section M for the links of a message and the tokens the model must copy, section N for the
Translate Companion presence check and the compact bubbles, all the same day). Sections I to M keep
the numbers of their own dates.
The detailed specs, the measurements
behind them and every deviation from the original plan are in:

| File | Part |
|---|---|
| `docs\ADDON.md` | the addon modules, tests and decisions |
| `docs\BRIDGE.md` | the pixel bridge v2 (normative): strip, frames, messages, tags, cards, validation |
| `docs\ENGINE.md` | the translation engine: discovery, llama-server lifecycle, prompt, evaluations |
| `docs\HELPER.md` | the Helper app: threads, capture, overlay, CLI, diagnostics, CPU measurements |
| `tests\e2e\README.md` | the end-to-end rig (fakewow) and its checks |
| `docs\IN-GAME-CHECKS.md` | what only the real client can show (G1-G21) |

---

## A. Decisions

**Base.** Row tags put each line's identity into pixels, the strip shows only while a message is
queued, the header flags are always zero (no game state leaves the game), ChatML injection is
neutralized, and the bridge protocol is the validated v1 framing with minimal changes. On top:
plate occlusion masks, a CRC on each tag, slot reuse only after a read-only scan, alpha measured
from the tag's sync pair, a `WindowFromPoint` covering test, glossary false friends, link marks
(0x02/0x03), a whitelist purge of the saved variables, a mono mode picked from the colourblind
setting, no partial GPU offload, flag probing with `--help`, and the model found through the
Ollama store.

**1. Language and stack**
- Helper: Rust, built with `cargo build --offline --release` and `+crt-static`
  (`helper\.cargo\config.toml`, which also limits builds to 3 jobs). One exe of about 1.1 MB that
  imports Windows DLLs only.
- Crates: `windows 0.62.2`, `serde` / `serde_json`, `ureq 3.4.2` without TLS. No other crate, so
  nothing can trigger a download.
- Addon: Lua. Reference tools: Python 3.12 with numpy (bridge reference and simulation only).

**2. Capture**
- Primary: Windows.Graphics.Capture (WGC) on the WoW window. It sees the game under other windows
  and never sees the Helper's own overlay.
- Fallback: GDI `BitBlt` from the screen, used when WGC is unsupported, or after WGC delivered no
  fresh frame for 3 s with the game in front and one session re-creation did not help. In GDI
  mode the overlay windows are excluded from capture (`WDA_EXCLUDEFROMCAPTURE`).
- Never: a handle to the game process, memory reads or input injection. Since 2026-10-19 the
  Helper sets two out-of-context WinEvent hooks (foreground change, minimize start and end) so its
  overlay's z-order follows the game window at once (section K, D12); before, the foreground window
  was only polled.

**3. Bridge protocol v2 (addon to Helper)** - spec in `docs\BRIDGE.md`
- A 4 px strip of 2x2 blocks at the client's top-left: 8 calibration greys, then the frame bytes.
  Modes: dense (6 bits per block), robust (3), mono (2, grey only). Mode `auto` is mono when the
  colourblind filter is on, else dense, and follows that setting while playing.
- Frames: magic, version 2, seq, flags = 0, session, length, a STATUS record (screen size and
  addon build), then message chunks; CRC-32. Each chunk carries the CRC-32 of its whole message.
- Exposure: 400 ms of screen time per chunk, 50 ms hold between changes, 70 ms rotation, at most
  100 ms credit per game frame. No keepalive: the strip is hidden while its queue is empty, so an
  idle addon costs nothing per frame.
- The strip is never wider than the screen (`cols = min(option, physW / 2)`).
- Messages: TRANSLATE, CANCEL, CLEAR, TEST, VIEW. Parsers are strict.

**4. Return path: pixel-anchored cards**
- On a click the addon appends K reserved rows (1 to 6; 1 to 4 until 2026-10-18) under the line. Each row is a **tag**
  (8 inline colour blocks: a magenta/green sync pair and 6 base-6 digits carrying slot, row,
  session bit and a CRC-5) followed by a **plate** (one dark inline texture, RGB 32,40,64).
- WoW's own chat renderer lays the rows out, scrolls, clips, fades and hides them. Lua geometry
  never positions a line; the VIEW message (chat clip rectangles and font size) only bounds the
  scan, sizes the overlay windows and sets the font.
- Each capture, the Helper finds the tags, measures the plate right of each tag and draws only on
  plate pixels it sees right now (the mask), so a tooltip or menu over part of a row is never
  painted over. A row whose plate is expected but not found is dropped.
- Once read, each tag and the visible plate pixels are painted over, opaque, with the chat
  background sampled next to the row in the same capture (the "native look", 2026-10-18), so
  only the translation shows, in the chat font, as if it were a chat line. Without the Helper the
  tags and the dark plates stay visible.
- A display curve between the game and the capture (the game's Gamma setting, contrast) is
  measured on the strip's calibration blocks; when it is not linear, the scan area is linearized
  before the tag scan and the cards.
- Overlay windows are layered, click-through and never activate; one per view, hidden when empty.
  They are topmost while the game window is in front, and sit right above the game window in the
  z-order otherwise, so another application's window always hides them (section K, D12).

**5. llama-server lifecycle and resources** - details in `docs\ENGINE.md`
- Started on the first translation request only; hidden, below-normal priority, in a
  kill-on-close job object; 127.0.0.1 on a free port with a random API key in a key file.
- Flags `-c 1024 -np 1 --fit off -lm none --no-ui --offline` when the probed `--help` lists them.
- GPU (Vulkan first, else CUDA on NVIDIA) with full offload when free VRAM is at least
  2900 MB + a 1500 MB margin; otherwise CPU with `clamp(physicalCores / 2, 2, 4)` threads, which
  needs 3600 MB of available RAM. No partial offload.
- Unloaded after 10 minutes idle, or 60 s after the game window disappears. A 15 s VRAM guard
  moves the session to CPU below 700 MB free.

**6. Prompt** (`PROMPT_VERSION` 3 since 2026-10-19: the cache key holds the protected spans)
- Raw ChatML with thinking disabled. The default system prompt asks for natural, casual chat in
  the target language, keeping placeholders, player names and game abbreviations, using official
  WoW names, and replying with the translation only.
- Item names and `{...}` groups become placeholders `{1}..{9}`, then (2026-10-19, user request
  L4) the tokens the model must copy that no mark covers: URLs, e-mail addresses and IDs (letters
  with 2 digits or more, handles with `_` or `#1234`), full-width forms included; levels, prices,
  times and ratings in any of the supported languages' shorthand stay plain (`docs\ENGINE.md`
  section 6). A missing or duplicated placeholder triggers one retry with the names in brackets.
- Must-use glossary meanings (chat words, false friends, user entries) are expanded into the
  message before the call; keep-allowed game abbreviations may stay as typed.
- An unchanged answer counts as "Already in {language}" only with evidence that the line is in
  the target language; otherwise it is shown as the translation.
- Sampling: temperature 0.2, top_p 0.8, top_k 20, seed 42, stop `<|im_end|>`, `cache_prompt`.
  A per-target GBNF grammar blocks scripts the target cannot use.
- These choices were measured on CPU (E1, then E1b): the shipped tuning passes every gate on the
  40-line synthetic corpus (script 35/35, item names 22/22, must-use glossary 10/10, wrong
  "Already in" 0). Official localized names stay weak (1/6).

**7. Glossary**
- The addon sends only the pairs matched in the clicked line, inline in TRANSLATE (at most 8 pairs
  and 512 B). Meanings: French for target `fr`, English for every other target (English works as
  a pivot). Multi-meaning entries are joined with " / ". User entries are sent verbatim; `false`
  disables an entry. `FALSE_FRIENDS` (for example `cheap` = "pas cher" for French) is used in
  prompts only.
- The glossary row under a clicked line shows the meanings of the abbreviations at once, without
  the Helper.

**8. Item links**
- The addon wraps only real link names and `{...}` groups of 1-32 characters in 0x02/0x03 marks;
  brackets a player typed stay plain and are translated. A typed control byte is removed, so a
  player cannot forge a mark.

**9. Chat restrictions** (no secret value is ever read, encoded or rebuilt)
- A line readable at arrival is tracked in Lua memory and translatable at any time, including
  during a restriction.
- A click on an untracked line during a restriction shows "Waiting for the end of the
  restriction..." and runs once chat is readable (at +1 s, and +30 s if still locked).
- A line that arrived with any secret argument got no [T]. After the restriction, when such lines
  are still shown by a chat frame and readable again, one line offers **[Show them]**, which
  reprints them as addon lines with a [T].

**10. What is shown in each state**

| State | In chat (addon) | Over the chat (Helper, if running) |
|---|---|---|
| Readable player line arrives | ` [T]` link after the line | nothing |
| Click [T] | K reserved rows (tag and plate) under the line, then the glossary row (if `show on` and any match), then the romanized name row (if `names on`) | a status in the plate within about 100 ms ("Waiting (N ahead)...", "Starting the translator...", "Translating..."), then the translation |
| Helper not running when the addon loaded (status file absent, stopped or stale; section N) | a click opens only the centred Translate Companion popup: no rows, no window, nothing printed; Continue runs the click, Cancel or Escape drops it (until build 5: the rows and a gold hint lane, section L) | a green "Translate Companion detected" over the popup's status card while it runs |
| Helper running when the addon loaded | rows at once | the statuses, then the translation |
| Helper started after the click (after Continue) | rows stay dark | "Not received - click [T] again" once a plate has been seen for 1.5 s without a request |
| Re-click [T] | rows unchanged; the same request is sent again (served from the cache; repairs a missed request) | same card |
| Shift-click [T] | rows unchanged; new request with noCache | the card restarts |
| Click a reserved row (the overlay is click-through) | rows removed, CANCEL sent | the card disappears |
| Line scrolled out of every chat frame | its slot is freed by the next periodic scan (CANCEL sent) | nothing to draw |
| Restriction, untracked line | one row "Waiting for the end of the restriction...", no tag | - |
| After a restriction, with secret arrivals | one print: "N messages arrived during the restriction. [Show them]" | - |
| Line gone from every chat frame when clicked | print SUFFIX_TOO_OLD | - |

---

## B. Removing the Lua machine translation

- **Deleted:** the in-addon translator (`Scheduler`, `Kernels`, `Model`, `Bench`, `Base64`,
  `Tokenizer`, `Engine`, `LangData`, `Clean`), the model conversion and golden tools, the golden
  test data and the native engine test. `LockdownLog.lua` became `Restriction.lua`. The generated
  `build\` and `models\` folders (about 414 MB) were moved to the Recycle Bin.
- **Saved variables** (`TranslateProbeDB`, schema 3 since 2026-10-18): on every load, keys
  outside the whitelist `schema, target, names, glossShow, mode, glossary, helpShown` are deleted
  (the old telemetry goes away), then each kept value is validated. A glossary value is `false`,
  a string (every target) or a table of meanings per target (C.5). Message text, sender names,
  line ids, request ids and slots are never stored.
- **Slash commands** (`/tpr`, `/tprobe`): `/tpr` alone and `options` open the options panel
  (C.5), `info`, `target <code>`, `names on|off`, `show on|off`, `mode auto|dense|robust|mono`,
  `translit <text>`, `gloss add|del|list|test`, `test`, `test rows`, `test lockdown`, `clear`,
  `app`, `debug`, `help`. The old `bench`, `model`, `budget`, `speed`, `simulate`, `free`, `log`,
  `catchup`, `report` and `reset` are gone.
- Targets: en, fr, de, es, it, pt, ru, ko, zh, zh-TW (the default follows the client locale).

---

## C. The addon

### C.1 Modules (TOC order)

```
Locales.lua      ns.L (English base, frFR override)
ConvLocales.lua  the conversation window's strings and key binding names (en, frFR)
ReplyLocales.lua the reply composer's strings (en, frFR)
CompanionStatus.lua  TranslateProbe_CompanionStatus = nil; Translate Companion overwrites the deployed copy (section N)
Core.lua         saved variables (whitelist purge, schema 4), session id, print helpers, inits under pcall
Presence.lua     the status file read at load (running, installed, absent), the session's confirmation, Presence.Ensure
Bridge.lua       strip encoder, message builders, tag and plate markup, ApplyTag, BUBBLE_RGB, PRESENCE_SLOT
Links.lua        12.x colour escapes and links of a message (section M)
Glossary.lua     Describe, Extract, PromptText, VisibleText, PlainText, Romanize
Lang.lua         TARGETS, NAMES, DefaultTarget, Script, TargetScript
LangData.lua     trigram profiles (restored from 8b7750e)
LangDetect.lua   the sender's language (writing system, trigrams, chat words, "unknown" rule, votes)
Reserve.lua      metrics, row estimate, reserved rows, slots (chat and window), VIEW collector and watcher, window metrics
ChatButton.lua   filter, [T] decoration and routing, Track, LineInfo, SetSuffix, CutRows, ShownLines, tooltips
Request.lua      click flow, request ids, active requests (memory only), window bubbles' TRANSLATE
Restriction.lua  restriction watcher, deferred clicks, secret arrivals, reprint
Popup.lua        the window's own popup menu
Conversations.lua conversations, messages, following, closed tabs, memory bounds (memory only)
Bubble.lua       bubbles, the reply card, TranslateProbeConvFont
Channels.lua     the reply's channel buttons, wire text, sending, echoes
Consent.lua      the in-game clipboard consent dialog
CompanionDialog.lua the Translate Companion popup and its status card (slot 125, VIEW 201)
Composer.lua     the reply composer (states, paste detection, checks, reply TRANSLATE and CANCEL)
ConvFrame.lua    the window: tabs, header, thread, slot manager, per-row visibility, VIEW 200, position
Commands.lua     slash commands
Options.lua      options panel (game settings) and its glossary page
```

`Bindings.xml` (loaded from the addon folder) declares the two key bindings of the window.

### C.2 Data flow

```
CHAT_MSG_* (readable) -> filter (notification only) -> next frame: TransformMessages adds " [T]" -> Track (memory)
click [T] -> Request.Start -> PromptText / Extract / AllocSlot / EstimateRows
          -> SetSuffix (per-frame rows: tag + plate, then glossary and name rows)
          -> Bridge.Send(VIEW) -> Bridge.Send(TRANSLATE)   (a VIEW before every TRANSLATE)
          -> strip shown (about 0.5 s) -> hidden when the queue drains
Helper: capture -> strip decode -> reassemble -> TRANSLATE -> engine -> status / result
        -> overlay drawn on the plates found in each capture
```

### C.3 Rows and slots

- **Metrics** per chat frame (nil when anything is missing or secret): physical px per unit
  `ppu = effScale * physH / 768`; an inline texture unit is measured in chat units (`tex`, 1.47 on
  the user's scaled chat frame); tag block width `W = max(2, ceil(5 / (ppu * tex)))` texture units
  (at least 5 physical px); row height `H = max(6, round(fontH / tex))`; plate width
  `floor((chatWidth - indent - 8 W tex - 2) * 0.88 / tex)`, usable from 40 units. The Helper
  relies on that 0.88 to tell a whole plate from one cut by a frame over the chat.
- **Row count** `K = min(6, max(1, ceil(width / plateW * f - 0.15)))`, with `f` from the source and
  target scripts (Latin to Latin 1.35, Han or kana to Latin 2.2, Hangul to Latin 1.8, Latin to Han
  0.8, default 1.3). The width ratio is not rounded up first, so a short line reserves one row.
  The tag's row field has 2 bits: rows 4 and 5 carry 0 and 1 again, and the Helper numbers a
  card's rows on from its first visible tag.
- **Slots** 0..99 for chat lines since 2026-10-19 (0..126 before; 100..124 are the window's
  bubbles (100..125 until build 5), 125 the Translate Companion popup's status card since build 6,
  126 the reply, 127 the `/tpr test rows` slot) come from a free list. A slot is freed when
  its rows are removed, or by the read-only scan once its line has left every chat frame. The scan
  is a `TransformMessages` predicate that records our row links and always returns false, so no
  entry is ever repackaged (no new timestamp, no un-fade). It runs at most every 15 s from the
  watcher while slots are in use, and when the free list is empty; only when all 127 slots are
  held by lines still in the chat does the oldest request expire.
- **VIEW**: sent before every TRANSLATE (so a Helper started late leaves its discovery scan at
  the next click), and by a 0.5 s ticker, only while requests are active, when the chat
  rectangles change (geometry reads only). An empty VIEW is never sent: the Helper keeps the last
  one.
- **Session hygiene**: rows of an older session (before a `/reload`) are cut at the first
  PLAYER_ENTERING_WORLD.

### C.4 Per-frame cost budget (WoW Lua)

| State | Budget |
|---|---|
| Idle | no OnUpdate, no ticker; the filter costs at most about 30 us per readable line |
| Click | at most about 1 ms (Extract, PromptText, metrics, rows, SetSuffix across frames, encoding) |
| Strip visible (about 0.5 s per click) | update at most 10 us per frame; compose and draw at most 0.3 ms per change |
| Active requests | VIEW watcher at 2 Hz (geometry reads), plus the read-only scan at most every 15 s |
| Memory | well under 1 MB |

### C.5 Options panel

- Registered with the 12.x Settings API (`Options.lua`, checked against the live UI source; the
  calls are listed in `docs\ADDON.md`): Esc > Options > AddOns > TranslateProbe, or `/tpr` and
  `/tpr options` (`Settings.OpenToCategory`). Without the Settings API nothing is registered and
  `/tpr` prints the command list.
- **Main page** (vertical layout, proxy settings on `TranslateProbeDB`): target language (each
  language named in its own language and script, `Lang.NATIVE`), "Explain abbreviations"
  (glossary row), "Spell names in Latin letters", bridge mode (advanced). The slash commands go
  through the same setters and call `Settings.NotifyUpdate`, so the panel and `/tpr` always agree.
- **Glossary page** (canvas subcategory, built when first shown): every abbreviation, user keys
  first then built-in ones, 12 rows at a time (buttons and mouse wheel); a mark (yours, edited,
  all languages, in English, disabled); the meaning for the current target in an edit box (Enter
  or leaving it saves, Escape restores, empty removes the meaning for this target only); Delete
  for user keys, Disable/Enable for built-in ones; an add row. A click on a button first saves the
  text being typed in a row.
- **Meanings follow the target, abbreviations stay.** A user value is a table of meanings per
  target; an old single string keeps serving every target (as `"*"` once one target gets its own
  meaning, and an empty marker hides it for one target). Built-in meanings are data in English and
  French; other targets get the English one, shown "in English", until the user types their own.
  The format is one way: schema 3, and `scripts\dev.ps1` backs the saved variables up before each
  deploy.

### C.6 User-visible strings

All in `Locales.lua` (English and frFR). The Helper's strings are in `helper\app\i18n\en.json` and
`fr.json` (status lines in the target language when it is en or fr, else English).

---

## D. Translate Companion (the Helper)

The Windows app is named Translate Companion for users since 2026-10-19 (exe
`WoWTranslateCompanion.exe`, tray, toasts, balloons, strings); the internal names below (crates,
the mutex `Local\TranslateProbeHelper`, the `TPHelper*` window classes, the
`%APPDATA%\TranslateProbe` and `%LOCALAPPDATA%\TranslateProbe` folders) did not change (section L).

### D.1 Layout (four crates, no Cargo workspace)

```
helper\.cargo\config.toml   +crt-static, [build] jobs = 3
helper\bridge\    tp_bridge   strip decoder, messages, reassembly, tag scan, cards, synthetic images (feature synth)
helper\engine\    tp_engine   discovery, flags probe, device policy, llama-server, prompt, post-processing, queue, cache
                              bin evalprompt (E1 evaluation)
helper\app\       tp_helper   the app (bin WoWTranslateCompanion, until 2026-10-19 TranslateProbeHelper;
                              bins capprobe and clipprobe)
helper\testkit\   tp_testkit  bin fakewow (the E2E stand-in game window)
```

Release profile: `opt-level = "s"`, LTO, one codegen unit, `panic = "abort"`, stripped.

### D.2 Interfaces between crates

- `tp_bridge`: `crc` (`crc32`, `crc5_tag`), `image` (`Rect`, `Bgra`), `strip` (`decode_strip`,
  `Frame`, `Calib`, `Mode`), `msg` (`ChatText`, which has no Debug, Display or Serialize;
  `parse_message`), `reasm` (`Reassembler`), `tags` (`scan`, `TagHit`, `tag_colors`), `cards`
  (`build`, `Card`, `RowBand { row, text_rect, mask, tag_rect }`, `Mask`). `tag_rect` is the area
  the overlay paints over to hide a tag (the tag and its blended edges).
- `tp_engine`: `Engine::start(cfg, on_update, on_state)`, `EngineHandle` (`submit`, `cancel`,
  `clear_session`, `set_wow_present`, `set_backend`, `unload_now`), `Status`, `EngineState`,
  `EngineEvent` (numbers, enums and fixed names only), `selftest::run` and `run_with`, `prompt`.

The full lists, with the additions to the original interface, are in `docs\BRIDGE.md` section 12
and `docs\ENGINE.md` section 1.

### D.3 Threads and rates

- **UI thread**: single-instance mutex `Local\TranslateProbeHelper` (a second copy exits with
  code 2); hidden window `TPHelperMain`; tray icon; overlay windows (`TPHelperOverlay`) and toast
  (`TPHelperToast`). It renders only on change. The tray menu uses the documented
  notification-icon pattern (foreground on the user's own right-click), hides the cards while it
  is open, and drains the scene and status messages again when it closes. A session end
  (`WM_ENDSESSION` with wParam TRUE) stops the capture thread and the engine synchronously; a
  cancelled shutdown is ignored.
- **Capture thread**:

  | State | Rate |
  |---|---|
  | No game window | look-up every 2 s, nothing else |
  | Minimized | no capture |
  | In front, nothing pending | 10 Hz |
  | In front, strip seen in the last 2 s | 20 Hz |
  | In front, card visible | `captureHzActive` (20-60, default 30) |
  | Not in front | 5 Hz, 20 Hz while the strip is active (strip and tags since 2026-10-19; the cards stay, D12) |

  Each tick reads the window geometry, grabs the strip (client rows 0..8) and the scan regions (one
  per VIEW rectangle, merged only when cheap, or a discovery area at 2 Hz while no VIEW is known), decodes,
  reassembles, feeds the engine, scans tags (only rows where the sync relation can hold; bands
  whose tags did not change keep their readings), builds cards (kept when the scan area is
  pixel-identical), runs the covering test (at most 10 Hz per card) and publishes the scene when
  it changed. WGC copies only the two regions to reused staging textures and waits on a D3D11
  fence, so nothing spins.
- **Engine threads** (tp_engine) and a **log thread**.

### D.4 Overlay drawing

- Per card (native look): the tag, the visible plate pixels, the plate's fractional edge pixels
  and the gaps of 4 px or less between rows are painted opaque with a left-to-right gradient
  between two chat background samples taken in the same capture (the indent left of the tag, and
  the end of the view right of the plate when the plate is whole); then the text over a 1 px
  black shadow, faded by the card alpha, only on mask pixels. No-plate mode fills the text
  rectangle. Details and limits: `docs\HELPER.md`.
- Fonts: "Arial Narrow" (else "Arial" narrowed) for Latin and Cyrillic, "Microsoft YaHei UI" (zh),
  "Microsoft JhengHei UI" (zh-TW), "Malgun Gothic" (ko); size from VIEW, capped so that the
  font's cell fits the band height + 1 px; text rasterized at 3x and filtered down. Colours: text
  #A8D8FF, statuses grey #A0A0A0, failures #E08080.
- Fit: wrap into K lines at the card's text width, else at the largest size down to 80% that
  fits, else cut with "...". Line i is drawn in band i, so rows clipped at the top of the chat
  still show the right lines.
- Slot 127 draws "Overlay OK (row N, block P px)". `--diag` adds outlines.

### D.5 Configuration, logs, privacy

- Settings: `%APPDATA%\TranslateProbe\settings.json` (numbers, flags and paths only: backend,
  GPU margin, CPU threads, idle unload, capture method and rate, recording exclusion, text colour,
  font scale, runtime folder, model file, log level).
- Data: `%LOCALAPPDATA%\TranslateProbe\logs\helper.log` (1 MB x 3) and `run\` (key file,
  `flags.json`, Vulkan marker).
- Logs and `--diag-json` hold lifecycle events, counters, ids, sizes, latencies and geometry,
  never message text, translations, glossary terms, names, prompts or server output (except an
  allowlisted ring of lifecycle lines after a crash). The type system enforces it: chat text
  travels in `ChatText`, and log events only carry numbers, enums and fixed names.
- CLI: `[--runtime DIR] [--model FILE] [--backend auto|cpu|vulkan|cuda] [--capture auto|wgc|gdi]
  [--translator llama|echo] [--wow-class CLASS] [--diag] [--diag-json FILE] [--log-dir DIR]
  [--no-tray] [--exit-after SECS] [--selftest]`, plus the test flags `--no-overlay`,
  `--assume-foreground`, `--overlay-probe` and `--no-clipboard` (implied by `--no-overlay`), and the
  hidden `--instance-name NAME` and `--assume-background`.
- Development: `scripts\helper.ps1 -Build | -Test | -Run | -Diag [-RuntimeDir DIR]`. The runtime
  folder comes from `-RuntimeDir`, `$LlamaRuntimeDir` in the gitignored `scripts\dev.local.ps1`,
  or `TPROBE_LLAMA_RUNTIME`; no personal path is committed.

### D.6 Release bundling (later; nothing is downloaded now)

- Runtime: copy the llama.cpp runtime of an on-disk Ollama build without the CUDA folders (about
  80 MB, MIT; provenance and licence text to be reviewed), or a pinned upstream llama.cpp release
  (needs the user's permission to download).
- Model: not bundled; the Ollama store is reused, or `--model`. A consented download of the 2.5 GB
  GGUF needs permission.
- The Helper ships outside CurseForge (executables are not accepted there). Third-party notices:
  windows-rs, serde, ureq (MIT/Apache), llama.cpp/ggml (MIT), Qwen3 (Apache-2.0).

---

## E. Tests

### E.1 Offline, automated

| Command (repo root unless noted) | What |
|---|---|
| `npm test` | the addon harness in enUS and frFR (`tests\run.js`, mock client, real `Bridge.lua`), then the Lua bridge against the reference vectors (`tests\bridge\run.js`) |
| `node tests\run.js enUS --stub-bridge` (and frFR) | the harness with `tests\stubs\Bridge.lua` |
| `luacheck addon` | lint |
| `python -I tools\bridge\make_vectors.py <dir>` | regenerates the vectors (byte-identical to `tests\vectors`) |
| `python -I tools\bridge\sim.py all` | bridge simulation: nominal, stress20, w1366, mono_mix |
| `python -I tools\bridge\crc_mc.py 2000000 1` | 2 M corrupted frames: 0 undetected |
| `cargo test --offline` in `helper\bridge`, `helper\engine`, `helper\app`, `helper\testkit` | unit and integration tests (one cargo at a time) |
| `cargo test --offline --release -- --ignored` in `helper\bridge` | 200 000 corrupted strips, per-call cost |
| `cargo test --offline --test window_pipeline -- --ignored` in `helper\app` | the real exe against a small window at the bottom of the z-order |
| `python -I tests\e2e\check.py --selftest --truth <truth>` | check.py on synthetic diags: a perfect one passes, a shifted one fails (and, for `conv`, a broken window fails `window_fill`) |

### E.2 End to end, without the game

`python -I tests\e2e\run.py --all --capture wgc` plays every scenario (basic, scroll, fade, cover,
tabs, altz, dropped, late_start, gamma22, and since 2026-10-19 conv: a synthetic conversation
window with three bubbles and a reply card; since build 6 presence and presence_late: the
Translate Companion popup's status card, with the Helper running before it or started while it
shows) in fakewow, placed off every monitor, with
the Helper on the echo translator and `--no-overlay --no-clipboard` (no window is ever shown, the
clipboard is never touched), then checks latency, band positions, hiding, masks, "Not received",
completion and the Helper's CPU use, plus the window checks for conv. `--capture gdi`
needs fakewow on screen and runs only when no game client is running. The real model runs with
`python -I tests\e2e\run.py --scenario basic --translator llama --helper-arg=--backend=cpu --helper-arg=--runtime=<runtime folder> --fakewow-arg=--tail-ms --fakewow-arg=20000 --done-within-s 60`
(the 20 s tail keeps the scene up while the translator starts on the CPU).

### E.3 Results of the integration run

See section I.

---

## F. In-game checks

`docs\IN-GAME-CHECKS.md` lists the setup (deploy, build and start the Helper) and the checks G1
(bridge and capture) to G10 (native look) for inline mode, then G11 (client facts) to G18
(clipboard end to end) for the conversation window and replies, G19 (hint lanes, builds 4 and 5
only), G20 (links of a message) and G21 (Translate Companion presence, the popup and the compact
bubbles), with what to report.

---

## G. Open risks and how they are settled

| # | Risk | Settled by | Fallback |
|---|---|---|---|
| 1 | Capture goes stale or black with the game in front | G1 (fresh-frame counts in `--diag`) | automatic switch to GDI; exclusive fullscreen gets a tray notice |
| 2 | Inline textures: sizes, vertex colours, no tint, gaps, hanging indent, clickable rows | G2 (`/tpr test rows` printout and tag geometry) | solid-colour TGA files; `Reserve.PLATE = false` (no-plate mode); plate width factor |
| 3 | UIParent covers the whole client; 2x2 blocks exact at `SetScale(768/physH)` | G1 margin, sub-pixel phase search | robust or mono mode |
| 4 | Colour path (gamma, contrast, colourblind filter, HDR, GPU filters) | G1 margin, G8 | level re-estimation, robust and mono modes, weak-signal balloon. A strong darkening curve still defeats the plate test (`docs\BRIDGE.md` section 11) |
| 5 | Drawing the strip and reading geometry during a restriction | G4 | `Bridge.Hold`; sticky VIEW and discovery scan |
| 6 | A topmost overlay costs FPS | G5 | shown only while cards are visible; lower `captureHzActive` |
| 7 | Model quality (official names, rare errors) | E1b, G3 | more glossary entries (official names as must-use entries) |
| 8 | VRAM pressure in crowded places | G5 and the VRAM guard log | CPU below 700 MB free; `backend: cpu` |
| 9 | Chat history survives `/reload` | G7 | old-session cut at load, session bit, known slots only |
| 10 | Tag false positives on busy chat | the synthetic suites; G3 reject counters | uniformity, alphabet fit, CRC-5, known slot, plate check |
| 11 | Scroll ghosting (overlay one capture behind) | G3, E2E latency | `captureHzActive` up to 60 |
| 12 | K underestimates long translations | E1 lengths, G3 | shrink to 85%, then "..."; tune the factors |
| 13 | Chat replacement addons with non-standard frames | `/tpr info` | documented limit |
| 14 | `ChatFrame.OnHyperlinkLeave` may not exist | G2 | remove the tooltips |
| 15 | First Vulkan run compiles shaders | the first GPU click shows "Preparing the GPU..." | warm-up at server start, marker file |

---

## H. Deviations from the original plan

Each is recorded with its evidence in the component docs; the main ones:

- **Addon** (`docs\ADDON.md` 1-23): extra functions for the scan and the watcher; empty VIEWs never
  sent; the row estimate without rounding first; the periodic read-only scan that lets the watcher
  stop; secret arrivals counted on any secret argument and reprinted only when still shown; mode
  `auto` following the colourblind setting; `Glossary.Lookup` removed; since 2026-10-18 up to 6
  rows (factor 1.35, addon build 2), the chat tab mode dropped, per-target glossary meanings
  (schema 3), `/tpr` opening the options panel, a VIEW before every TRANSLATE.
- **Bridge** (`docs\BRIDGE.md` 10): sync-pair test from the B0/B1 relation; sub-pixel block fit;
  the plate predicted from the tag's own colours (fades over coloured grounds); cover checks;
  two-way mask and look-ahead text end; the strip capped at the screen width; `tag_rect` widened
  over the blended edges and painted opaque; the B1 candidate bounded by the scan area; a display
  curve measured on the strip undone before the tag scan and the cards (section 8b there).
- **Engine** (`docs\ENGINE.md` 11): the casual prompt with a language line, the glossary policy
  (keep-allowed vs must-use) and expansion before the call, `Same` only with evidence, bracket
  repair, `PROMPT_VERSION` 2; NVML matched by PCI ids; the selftest discovery injectable so tests
  never start llama-server.
- **Helper** (`docs\HELPER.md` 1-34): library target; text at 3x; discovery over the full client
  width; `--no-overlay` and `--assume-foreground` for tests; lower idle capture rates, scan gating,
  tag search and card cache for CPU; the tray menu foreground pattern; session-end clean-up; the
  display-curve correction; the sticky text width; since 2026-10-18 the native look (opaque
  background fill, right sample rules, no right sample for a card cut on the right or in
  discovery, edge pixels only where fractional, sample hysteresis, font cell cap), rows 1..6 and
  an exit on `WM_CLOSE` from any window (`taskkill` without /F).
- **E2E rig** (`tests\e2e\README.md`): fakewow off-screen as a layered window (WGC cannot capture a
  normal off-screen window); `run.py`; check.py waits for the VIEW and compares rows modulo 4;
  since 2026-10-19 a window layer (scenario `conv`) and the window checks.
- **Conversation window** (2026-10-19): see section K.4.
- **Hint lanes and Translate Companion** (2026-10-19): see section L.2.
- **Links of a message** (2026-10-19): see section M.2.
- **Translate Companion presence and compact bubbles** (2026-10-19): see section N.2 and N.3.

---

## I. Integration results (2026-10-18)

Dev PC: Windows 11, 8 cores / 16 threads, RTX 3080, a WoW client running the whole time (so every
window-level test used the off-screen or bottom-of-z-order placements and `--no-overlay`).

### I.1 Offline suites

| Command | Result |
|---|---|
| `npm test` | enUS 178/178 checks, frFR 178/178, bridge byte identity: 30 traces, 1066 frames, 1024 tags, 32 builder vectors, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 177/177 each |
| `luacheck addon` (1.2.0) | 0 warnings, 0 errors in 10 files |
| `python -I tools\bridge\make_vectors.py <dir>` | 30 traces, 1066 frames, 387 messages, 1024 tags, 32 builder vectors, 36 strips; byte-identical to `tests\vectors` |
| `python -I tools\bridge\sim.py all` | 4/4 pass: nominal 100% within 300 ms (P99 126 ms), stress20 100% (P99 135 ms), w1366 99.965% (P99 170 ms), mono_mix 99.927% within 600 ms (P99 298 ms); 0 undetected frames or messages |
| `python -I tools\bridge\crc_mc.py 2000000 1` | 1,883,513 caught by the CRC-32, 72,257 decoded as the true frame, 0 undetected |
| `cargo test --offline`, `helper\bridge` | 55 passed, 0 failed, 2 ignored (lib 19, cards 10, distort 7, tags 9, tone 1, vectors 9) |
| `cargo test --offline --release -- --ignored`, `helper\bridge` | 2 passed: 200,000 corrupted strips, 0 undetected; per-call cost (1080p: strip decode 11 us, view scan 270 us, cards 239 us) |
| `cargo test --offline`, `helper\engine` | 96 + 5 passed, 0 failed, 3 ignored |
| `cargo test --offline`, `helper\app` | 78 + 1 + 4 passed, 0 failed, 4 ignored |
| `cargo test --offline --test window_pipeline -- --ignored`, `helper\app` | 2 passed |
| `cargo test --offline`, `helper\testkit` | 16 passed |
| `cargo build --offline --release`, `helper\app` | 0 warnings; `TranslateProbeHelper.exe` 1,187,328 bytes, Windows DLL imports only |

Suite metrics (from the same runs): tag recall 26,601 / 26,601 on 3,000 synthetic chat panels; 0
false hits on 10,000 tag-free panels; 1,000,000 digit mutations with 0 misdecodes; cover masks IoU
0.9999 (hard edges) and 0.9958 (soft); fade over coloured grounds 3,555 / 3,555 rows shown; tag
cover leaves at most a 9.5 colour step on 75 sub-pixel cases; display curves: rows shown with the
curve undone 120/120 for Gamma 2.2, 1.6 and the darkening 0.4545 curve (raw: 60, 120 and 0 of
120).

No llama-server ran during the test suites (a watcher polled for it every 0.5 s during the engine,
app, testkit, window and echo E2E runs).

### I.2 End to end, echo translator, WGC, `--no-overlay`

| Scenario | TRANSLATE P99 ms | band failures / checks | hidden | masks | missing | done | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|
| basic | 77.7 | 0 / 3160 | pass | pass | pass (0) | 3/3 | 2.63 | 1.34 |
| scroll | 93.9 | 0 / 3191 | pass | pass | pass (0) | 3/3 | 2.19 | 1.35 |
| fade | 191.3 | 0 / 1321 | pass | pass | pass (0) | 2/2 | 2.70 | 1.74 |
| cover | 130.1 | 0 / 3388 | pass | pass | pass (0) | 2/2 | 3.06 | 1.73 |
| tabs | 93.3 | 0 / 2007 | pass | pass | pass (0) | 2/2 | 2.44 | 1.46 |
| altz | 92.2 | 0 / 2392 | pass | pass | pass (0) | 2/2 | 1.94 | 1.00 |
| dropped | 116.5 | 0 / 1364 | pass | pass | pass (1) | 2/2 | 3.52 | 1.84 |
| late_start | 74.3 | 0 / 162 | pass | pass | pass (3) | 1/1 | 3.54 | 1.40 |
| gamma22 | 78.5 | 0 / 3152 | pass | pass | pass (0) | 3/3 | 3.55 | 1.96 |

- Every functional check passes in all 9 scenarios. The CPU rule (3% of one core, process times
  over the whole run) passes in 5 of 9 and fails in 4 (3.06 to 3.55%). The first 2 s of a run hold
  41 to 69% of its cycles (one-time start-up, mostly the D3D11 device, about 0.3 s of CPU); after
  them the Helper uses 1.0 to 2.0% of one core. Identical runs differ by more than 1% (gamma22 gave
  2.54% and 3.94% in two runs), because process times are sampled by the clock interrupt.
- GDI capture: deferred in all 9 scenarios (`run.py --capture gdi` exit 3), because GDI reads the
  screen and the WoW client was running.
- Log sentinel grep over every E2E log, diag, truth and report file (83 files) for 31 sentinel
  words (the scenarios' chat words, sender names, "ZZSENTINELZZ"): 0 hits.

### I.3 Real model on the CPU

`run.py --scenario basic --translator llama --helper-arg=--backend=cpu`, dev llama.cpp runtime,
Qwen3-4B Q4_K_M from the Ollama store, 4 threads, `-ngl 0`, a 60 s watchdog on every llama-server:

- Target English (the scenario's lines are English): the three cards reached their final state
  ("Already in English", the expected answer) 15.6, 15.6 and 19.0 s after their clicks, on a cold
  start (flags probe, server ready after 12.0 s, warm-up done at 13.9 s); band failures 0 / 7516.
- Target French (the same scenario recorded in frFR): the three cards reached `done` 7.2, 7.3 and
  11.6 s after their clicks (server ready after 3.1 s); band failures 0 / 7486.
- Each llama-server lived about 35 s (it ends with the Helper); the watchdog never had to act, and
  `tasklist` showed no llama-server afterwards. No GPU pass was run (the game was using the GPU).

---

## J. Integration results (2026-10-19: options panel, native look, rows 1..6)

Same PC; the WoW client was not running during these runs (GDI E2E could run), and the user's
running Helper was stopped before the release build and left stopped.

### J.1 Offline suites

| Command | Result |
|---|---|
| `npm test` (enUS, frFR, bridge) | enUS 242/242, frFR 242/242, bridge byte identity: 30 traces, 1066 frames, 1024 tags, 34 builder vectors, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 241/241 each |
| `luacheck addon` | 0 warnings, 0 errors in 11 files |
| `python -I tools\bridge\make_vectors.py` | 34 builder vectors (a 6-row TRANSLATE and a refused 7-row one added); only `messages.json` changed; byte-identical on re-run |
| `python -I tools\bridge\sim.py all` | 4/4 pass, the same figures as I.1 |
| `python -I tools\bridge\crc_mc.py` | 2,000,000 trials, 0 undetected |
| `cargo test --offline`, `helper\bridge` | 59 passed, 2 ignored; `--release -- --ignored`: 2 passed |
| `cargo test --offline`, `helper\engine` | 96 + 5 passed, 3 ignored |
| `cargo test --offline`, `helper\app` | lib 102, graceful_exit 2, native_look 5, sentinel 1, tagscan_equiv 4 passed; window_pipeline `--ignored` 2 passed; `--release --lib -- --ignored` 2 passed (render 0.569 ms, candidates 0.093 ms) |
| `cargo test --offline`, `helper\testkit` | 16 passed |
| `scripts\helper.ps1 -Build` | `TranslateProbeHelper.exe` 1,225,216 bytes, Windows DLL imports only |

### J.2 End to end, echo translator, `--no-overlay`

WGC (fakewow off-screen):

| Scenario | TRANSLATE P99 ms | band failures / checks | hidden | masks | missing | done | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|
| basic | 67.2 | 0 / 3457 | pass | pass | pass (0) | 3/3 | 2.86 | 1.38 |
| scroll | 114.0 | 0 / 3140 | pass | pass | pass (0) | 3/3 | 2.14 | 1.29 |
| fade | 73.3 | 0 / 1289 | pass | pass | pass (0) | 2/2 | 2.75 | 1.74 |
| cover | 74.7 | 0 / 3904 | pass | pass | pass (0) | 2/2 | 2.19 | 1.22 |
| tabs | 53.0 | 0 / 1983 | pass | pass | pass (0) | 2/2 | 2.69 | 1.54 |
| altz | 81.6 | 0 / 2740 | pass | pass | pass (0) | 2/2 | 2.28 | 1.05 |
| dropped | 61.9 | 0 / 1351 | pass | pass | pass (1) | 2/2 | 3.17 | 1.59 |
| late_start | 70.3 | 0 / 1644 | pass | pass | pass (3) | 1/1 | 3.35 | 1.39 |
| gamma22 | 69.4 | 0 / 3477 | pass | pass | pass (0) | 3/3 | 2.78 | 1.40 |

GDI (fakewow on screen, never activated, no WoW client running):

| Scenario | TRANSLATE P99 ms | band failures / checks | hidden | masks | missing | done | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|
| basic | 68.0 | 0 / 3522 | pass | pass | pass (0) | 3/3 | 2.32 | 2.36 |
| scroll | 71.1 | 0 / 3196 | pass | pass | pass (0) | 3/3 | 2.40 | 2.48 |
| fade | 83.1 | 0 / 1333 | pass | pass | pass (0) | 2/2 | 1.85 | 1.92 |
| cover | 71.5 | 0 / 3965 | pass | pass | pass (0) | 2/2 | 3.00 | 3.14 |
| tabs | 65.9 | 0 / 2013 | pass | pass | pass (0) | 2/2 | 3.75 | 3.94 |
| altz | 69.7 | 0 / 2796 | pass | pass | pass (0) | 2/2 | 2.14 | 2.23 |
| dropped | 68.4 | 0 / 1363 | pass | pass | pass (1) | 2/2 | 2.45 | 2.58 |
| late_start | 25.3 | 0 / 1680 | pass | pass | pass (3) | 1/1 | 3.55 | 3.53 |
| gamma22 | 66.9 | 0 / 3520 | pass | pass | pass (0) | 3/3 | 2.63 | 2.79 |

- Every functional check passes in all 18 runs. The CPU rule (3% of one core over the whole run)
  fails in dropped and late_start under WGC (3.17, 3.35%) and in tabs and late_start under GDI
  (3.75, 3.55%); after the first 2 s the WGC runs use 1.05 to 1.74% of one core. GDI copies the
  screen, hence its higher steady figure.
- A first WGC run failed bands and hidden in basic, cover, altz and gamma22: the 5-row card of
  the scenario's long line now uses row 4, whose tag carries row 0, and check.py compared the
  truth's tag row with the Helper's band number directly. check.py now compares rows modulo 4;
  the same diag files then pass (no Helper change).
- `late_start` now checks 1644 bands instead of 162: the re-click sends a VIEW, so the Helper
  leaves discovery at once.
- Log sentinel grep (the scenarios' chat words and sender names, "ZZSENTINELZZ") over 118 output
  files of `tests\e2e\out`: 0 hits.

### J.3 Real model on the CPU

`run.py --scenario basic --translator llama --helper-arg=--backend=cpu --helper-arg=--runtime=<dev
runtime>`, Qwen3-4B Q4_K_M, `-ngl 0`, 4 threads, with a separate watchdog that kills any
llama-server older than 60 s (it never had to act):

- First run (the scenario's normal length): the first two cards reached "Already in English" 11.2
  and 10.9 s after their TRANSLATE; the third (the 5-row line) was still streaming when the
  scenario ended, so `done` passed on 2 of 3 judged cards.
- Second run with `--fakewow-arg=--tail-ms=25000` (model files warm): all three reached their
  final state ("Already in English") 4.8, 4.4 and 6.5 s after their TRANSLATE; every check passed
  (CPU 2.13%). llama-server ended with the Helper both times; `tasklist` showed none afterwards.

---

## K. The conversation window and replies (2026-10-19)

Built from the conversation design of 2026-10-19 in four work packages (P protocol, A window, R
reply and settings, H Helper), then integrated and reviewed. The component docs hold the details:
`docs\ADDON.md` ("Conversation window", "Reply and send", decisions 24-51), `docs\BRIDGE.md`
(section 3, section 8c), `docs\HELPER.md` ("Conversation window", "Game window in the
background", deviations 35-42), `tests\e2e\README.md` (scenario `conv`) and
`docs\IN-GAME-CHECKS.md` (G11-G18).

### K.1 User decisions

| # | Decision |
|---|---|
| D1 | Enter never sends; only a click on a channel button does |
| D2 | The Helper empties the clipboard as soon as a consented reply request arrives (the consent dialog says so) |
| D3 | Following new messages is "smart": whispers and conversations the player replied to are translated while visible; a first contact in a public channel is added untranslated, one click translates it |
| D4 | [T] opens the conversation window; Ctrl-click keeps the rows under the line; Shift still means "without the cache" |
| D5 | The original is hidden under each bubble, with an "Original" link |
| D6 | The Helper's tray switch "Copy translated replies to the clipboard" is on by default (the in-game consent is the approval) |
| D7 | Phase A (the window) and Phase B (replies with the clipboard) in one round |
| D8 | Translated text in bubbles uses the Helper's light blue (#A8D8FF) |
| D9 | One guarded live test of the real clipboard path, by the integration step only; unit tests use a fake clipboard, every E2E run `--no-clipboard` |
| D10 | Each tab has a close cross (on hover and on the selected tab) and closes on middle-click; closing keeps the history in memory and a later translated message reopens the tab with it; 30 conversations of 100 messages, the least recently active closed one evicted first; at most 10 tabs shown, the rest behind "+N"; never saved |
| D11 | The inline glossary row has its own colour (pale gold `e6cc80`) and is off by default (saved variables schema 4 turns an old "on" off once) |
| D12 | After an Alt-Tab the Helper keeps painting where the game window is visible and never over another application: the overlay leaves the topmost band and sits right above the game window; hidden only when the game is minimized, the cards fully covered, or the UI hidden |

### K.2 How it works

- **Addon:** `ConvFrame` (a `PortraitFrameTemplate` window, top tabs, a header with the sender's
  language, a scrolling thread of bubbles, the composer at the bottom). Each incoming bubble is an
  opaque `BUBBLE_RGB` (36, 30, 24) fill under Blizzard's ChatBubble border, with K rows of a tag and
  a plate like the inline rows; rows not fully inside the visible thread are hidden. Bubbles get
  window slots 100..125 when they come within a page of the view (a freed slot waits 2 s before
  reuse); the reply card uses slot 126. One VIEW entry, id 200, covers the bubble column from the
  thread's top to the reply card's bottom, so the Helper's native-look samples land in the bubble
  fill and the painted tags and plates disappear into it.
- **Protocol:** TRANSLATE gains the flags reply (4) and copy (8): copy needs reply, reply needs the
  plate flag, 2..6 rows (row 0 is the Helper's status) and an explicit target code. `ADDON_BUILD`
  is 3 (build 2 already shipped rows 1..6); the Helper serves builds 2 and 3 without a toast.
- **Reply:** Enter in the composer (or "Translate to XX") sends TRANSLATE on slot 126 with the
  player's draft. With consent (the in-game dialog, or a remembered "allow") the copy bit is set;
  the Helper empties the clipboard at once, translates the reply as an urgent job, paints grey
  "Translating..." on row 0 and the text below, writes the final text to the clipboard only when
  nobody copied anything since its empty (kept out of the Win+V history and the cloud clipboard),
  and then paints green "Ready - Ctrl+V to paste" ("Ready (not copied)" in amber otherwise). The
  player pastes into the box (the addon shows "Pasted"), the paste checks run, and a click on a
  channel button sends; CANCEL then lets the Helper empty the clipboard if it still holds its own
  text. From addon build 3 to 5 a hint lane under the card said "No status above? Start Translate
  Companion." (section L); since build 6 the window opens only with a confirmed Translate
  Companion (section N), and the card has no lane.
- **Helper in the background (D12):** the overlay windows are topmost only with the game in front;
  otherwise they sit right above the game window, so other windows hide them pixel for pixel. The
  strip and the tags are read in the background too (5 Hz, 20 Hz while the strip is active), so a
  followed whisper's TRANSLATE or a CANCEL sent while the player is Alt-Tabbed is not lost.
- **Privacy:** conversations, drafts, replies and sent texts live in Lua tables and Helper memory
  only, lost on /reload, logout or Helper exit; the saved variables keep the window's settings
  only; the Helper never reads the clipboard's content (sequence numbers only), and its logs and
  diag records of the reply flow are names and numbers.

### K.3 What is shown in each state (window)

| State | Bubble | Reply card |
|---|---|---|
| Helper not running | since build 6 the window opens only after the popup (section N); after Continue without the Helper: coloured tags and navy plates in the bubble, "Nothing painted?" on hover | tags and plates (the lane hint until build 5) |
| Helper started late | "Not received - click the bubble" (each click asks again) | "Not received - click Translate again" |
| Waiting, starting, translating | grey statuses | the same on row 0, stepper "2 Paste (Ctrl+V)" |
| Done | the translation in light blue, or "Already in French" | green "Ready - Ctrl+V to paste" and the text |
| Not copied | - | amber, with the reason (consent refused, switch off, copied meanwhile, clipboard busy) |
| Failed | "Translation failed - Shift-click the bubble" | red "Translation failed - click Translate again" |
| Untranslated (follow add, or a public first contact in smart mode) | the original in the bubble and a Translate button | - |
| Pasted, warning | - | "Pasted" in green, or an orange warning and "click again to send" |
| Sending, sent, not confirmed, offline, blocked | my bubble's meta line | - |
| Chat locked | a banner | stepper in red with the manual path; send buttons greyed; the clipboard is kept |
| Helper older than the addon | chat-style wording in bubbles | the reply is dropped ("Not received") and the "Helper is older" toast |

### K.4 Deviations from the conversation design

The main ones (each with its evidence in the component docs):
- `ADDON_BUILD` 3 instead of 2, served with 2 (build 2 shipped rows 1..6 the day before).
- Chat slots 0..99 (were 0..126); the thread keeps 100 messages (the design said 60) and 30
  conversations (the design said 10) with closed tabs kept (D10).
- A pick in the [Other] popup puts the channel on button 3 instead of sending (D1);
  `Channels.Wire` also turns line breaks into spaces; two extra commands (`/tpr conv reset`,
  `/tpr test paste`).
- A channel button uses the old channel number only when the channel there has exactly the
  sender's channel name (review fix; the design's "else arg8" sent to another channel after a zone
  change).
- The consent dialog leaves Enter to the game instead of swallowing it (review fix: a swallowed
  key before combat left every key swallowed in combat).
- `Action::ReplyStart` carries the request's text (the "Already in" and "Nothing to translate"
  results copy the player's own text); three extra reply statuses for those results.
- A capture that decodes a different VIEW skips its tag scan; the strip is read in the background
  (review fix); a reply card's lane has its own covering test (review fix).
- The E2E `conv` window is synthetic (drawn by the rig with the real tag colours through the
  addon's own `Bridge.Send`), not the addon's window modules.

### K.5 Integration review findings

| Finding (severity) | Status |
|---|---|
| A channel button could send to another channel after a zone change or a channel swap (critical) | fixed: name match required, harness checks for both cases (ADDON.md decision 47); G13 records arg9 on the client |
| Enter in the consent dialog, then combat: every key swallowed (major) | fixed: only Escape is swallowed and it hides the dialog in the same call (decision 48) |
| A bubble click asked the Helper again only the first time (major) | fixed: every click sends a new TRANSLATE (decision 51) |
| The Helper did not read the strip in the background, so TRANSLATE and CANCEL sent while Alt-Tabbed were lost (major) | fixed: the strip is part of every capture, 20 Hz in the background while it is active (HELPER.md deviation 38) |
| A popup opened over another put the window back in the Escape list (minor) | fixed: generation counter (decision 49) |
| The consent dialog stayed open when the window hid (minor) | fixed: `Consent.Cancel()` from the window's `OnHide` (decision 50) |
| clipprobe: unreadable text treated as no text, then emptied (minor) | fixed: skipped (HELPER.md deviation 42) |
| clipprobe: two sequence windows between save, empty, write and restore (minor) | fixed: guarded `begin`, restore only over the probe's own last change |
| clipprobe: length and hash of the saved text on a child's command line (minor) | fixed: the child prints its reading on its stdout pipe, the parent compares |
| `Iterator::all` skipped the other exclusion formats after a failure (minor) | fixed: `clip::set_every_exclusion` (deviation 41) |
| The `capture` diag record said `scan: true` for a VIEW-skip capture, making the conv `bands` check flaky (minor) | fixed: written after the decode, `viewSkip` field (deviation 39) |
| The reply lane was not in the covering test (minor) | fixed: its own test, a covered lane is dropped (deviation 40) |
| check.py window checks had blind spots and no negative self-test (minor) | fixed: `window_fill` fails on a window card with no sample; `--selftest` on a conv truth checks a perfect and a broken window diag. Not done: an E2E variant driving the real `ConvFrame`/`Bubble` modules through the mock (deferred: the rig's synthetic window tests the Helper against the protocol; the real modules are covered by the harness) |

### K.6 Integration results (2026-10-19)

Same PC. No WoW client ran during these runs. The user's Helper was stopped before the release
build (`taskkill` without /F; it exited within about 1 s, and no llama-server was left) and left
stopped. The display went to sleep (the 60-minute idle timeout) at about 04:14, during the WGC
E2E set: from `late_start` on, WGC delivered about 4 fresh frames a second instead of about 30
and GDI ran 2 to 4 capture ticks a second. The previous build measured the same in an A/B run, so
those figures describe the sleeping display, not this build.

**Offline suites**

| Command | Result |
|---|---|
| `npm test` (enUS, frFR, bridge) | enUS 562/562, frFR 562/562; bridge byte identity: 30 traces, 16,892 ticks, 1,066 frames, 1,024 tags, 46 builder vectors + 6 parser-only, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 562/562 each |
| `luacheck addon` | 0 warnings, 0 errors in 22 files |
| `python -I tools\bridge\make_vectors.py <dir>` | 30 traces, 1,066 frames, 387 messages, 1,024 tags, 52 message vectors, 36 strips; byte-identical to `tests\vectors` |
| `python -I tools\bridge\sim.py all` | 4/4 pass: nominal P99 126 ms, stress20 135 ms, w1366 170 ms (99.965% within 300 ms), mono_mix 298 ms (99.927% within 600 ms); 0 undetected frames or messages (7 min) |
| `python -I tools\bridge\crc_mc.py 2000000 1` | 1,883,513 caught by the CRC-32, 72,257 decoded as the true frame, 0 undetected |
| `cargo test --offline`, `helper\bridge` | 62 passed, 2 ignored; `--release -- --ignored`: 2 passed (200,000 corrupted strips, 0 undetected; at 1080p strip decode 10.8 us, view scan 266.5 us, cards 239.0 us) |
| `cargo test --offline`, `helper\engine` | 99 + 5 passed, 3 ignored; `real_server_cpu_smoke` (`--ignored`, CPU, 4 threads): passed, server ready in 2.6 s, first result 6.3 s; `selftest_on_this_machine`: passed |
| `cargo test --offline`, `helper\app` | lib 138, clipprobe 4, graceful_exit 2, native_look 5, sentinel 2, tagscan_equiv 5: 156 passed, 5 ignored; `window_pipeline --ignored`: 2 passed; release timings: render 0.626 ms, candidates 0.096 ms, `sample_card` 347 us |
| `cargo test --offline`, `helper\testkit` | 17 passed |
| `scripts\helper.ps1 -Build` | `TranslateProbeHelper.exe` 1,287,680 bytes, no warning |
| `python -I tests\e2e\check.py --selftest` | basic, cover: pass; conv: pass (the perfect diag passes every check, the shifted one fails bands and lane, the broken window fails window_fill) |

**End to end, echo translator, WGC, `--no-overlay --no-clipboard`**

| Scenario | TRANSLATE P99 ms | band failures / checks | hidden | masks | missing | done | window checks | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|---|
| basic | 166.4 | 0 / 3509 | pass | pass | pass (0) | 3/3 | - | 3.54 | 1.78 |
| scroll | 116.5 | 0 / 3190 | pass | pass | pass (0) | 3/3 | - | 2.96 | 1.83 |
| fade | 191.4 | 0 / 1321 | pass | pass | pass (0) | 2/2 | - | 2.59 | 1.66 |
| cover | 66.8 | 0 / 3960 | pass | pass | pass (0) | 2/2 | - | 2.75 | 1.55 |
| tabs | 69.1 | 0 / 1996 | pass | pass | pass (0) | 2/2 | - | 2.99 | 1.78 |
| altz | 69.2 | 0 / 2782 | pass | pass | pass (0) | 2/2 | - | 2.68 | 1.36 |
| dropped | 70.8 | 0 / 1365 | pass | pass | pass (1) | 2/2 | - | 2.87 | 1.50 |
| late_start (display asleep) | 226.4 | 0 / 252 | pass | pass | pass (3) | 1/1 | - | 2.78 | 0.62 |
| gamma22 (display asleep) | 261.6 | 0 / 424 | pass | pass | pass (0) | 3/3 | - | 2.24 | 0.53 |
| conv (display asleep) | 258.5 | 0 / 341 | pass | pass | pass (0) | 4/4 | pass | 2.28 | 0.67 |

- Every functional check passes in all 10 scenarios. The CPU rule (3% of one core over the whole
  run) fails in basic only (3.54%); conv passes `cpu` and `cpu_vs_i2`, but at the sleeping
  display's frame rate. conv: 106 window background samples, all within 6 levels of `BUBBLE_RGB`,
  no unsampled card; the reply card final with the outcome "off"; the lane found where the truth
  has it and painted; the half-out bubble's hidden row never got a band; no slot shown twice and no
  `strip_delay`; no capture skipped its scan for a new VIEW in this run.
- **Background:** `run.py --scenario conv --helper-arg=--assume-background` (every capture with
  `fg: false`, the strip read in each): all 4 TRANSLATE decoded and done 4/4, no "Not received",
  bands 0 / 336. Latency (P99 379.5 ms) and `partial_rows` (one band on the hidden row in the first
  record after the scroll) failed at the sleeping display's 4 frames a second, where the
  background scene keeps its cards between frames. Before the fix the strip was never read with
  the game in the background.
- **GDI:** deferred. WoW was not running, but with the display asleep the GDI capture ran 2 to 4
  ticks a second: `basic` passed every check except latency (P99 332 ms). The previous build
  failed the same latency check under WGC at that point (313 ms).
- **Real model on the CPU:** `run.py --scenario basic --translator llama --helper-arg=--backend=cpu
  --helper-arg=--runtime=<dev runtime> --fakewow-arg=--tail-ms --fakewow-arg=25000 --done-within-s 60`,
  Qwen3-4B Q4_K_M, 4 threads, a watchdog killing any llama-server older than 60 s (it never had to
  act): every check passed; the three cards reached "Already in English" 13.5, 13.4 and 15.5 s
  after their TRANSLATE on a cold start (the translator loading from 4.4 s of the run, busy from 16.5 s);
  CPU 1.76%. llama-server ended with the Helper; `tasklist` showed none afterwards.
- **D9 live clipboard test: skipped.** Before touching anything, the clipboard's format names were
  listed (names only): Bitmap, PNG, `CanUploadToCloudClipboard`, `CanIncludeInClipboardHistory`,
  an image. As decision D9 says for anything other than plain text, `clipprobe live` was not run,
  so the user's image was never at risk. The real clipboard path is checked in game (G18).
- Log sentinel grep (the scenarios' chat words and sender names, "ZZSENTINELZZ") over 123 output
  files of `tests\e2e\out`: 0 hits.

---

## L. Hint lanes and Translate Companion (2026-10-19, user requests U1-U3)

**The hint lanes are removed since addon build 6 (addon 0.7.0, Translate Companion 0.6.0), the same
day:** in game the lane cover failed under a bubble with a links bar, so the gold hint stayed under
a painted translation. The not-running message is now the Translate Companion popup, opened by a
[T] click before anything else (section N; `docs\ADDON.md`, "Translate Companion presence"). U2 and
U3 (the name and the download link) stand. The rest of this section records the lanes as they were
built for builds 4 and 5.

Built in two work packages (addon, helper), reviewed twice (one review per side), then integrated.
The component docs hold the details: `docs\ADDON.md` ("Hint lanes and the Translate Companion
dialog", decisions 52-62), `docs\HELPER.md` ("Hint lanes", deviations 43-51), `docs\BRIDGE.md`
(STATUS build 4, section 8c), `tests\e2e\README.md` (the `lanes` check, deviation 9) and
`docs\IN-GAME-CHECKS.md` (G19).

### L.1 User requests

| # | Request | As built |
|---|---|---|
| U1 | When the Helper is not running, a [T] click says so, with the download page | The addon cannot know whether the Helper runs, so the message is self-revealing: under the last reserved row of every card that waits for the Helper (incoming bubbles, the reply card, the rows under a chat line) the addon draws one more row, the lane, with "No translation above? Start Translate Companion." in gold and a [Download] link (window: a button), 1.5 s after the rows (window: after the lane became visible, and again after any move); the Helper paints every lane over whenever it paints the card. [Download] opens a "Translate Companion" dialog (start it or download it, the URL in a read-only box already selected for Ctrl+C, Close, Escape); `/tpr app` prints the URL. The reply card keeps "No status above? Start Translate Companion." |
| U2 | Rename the Windows app, user-facing, to Translate Companion | `WoWTranslateCompanion.exe` (Cargo `[[bin]]` WoWTranslateCompanion), tray tooltip (with the version) and menu (new "Get updates"), toasts, balloons, `--help`, `--version`, the en and fr strings of both sides, the slash help, README and docs. Kept: the addon's name TranslateProbe, the crate names, the mutex `Local\TranslateProbeHelper` (an old and a new exe never run together), the `TPHelper*` window classes and the `%APPDATA%\TranslateProbe` / `%LOCALAPPDATA%\TranslateProbe` folders (settings and logs carry over). Versions: addon 0.5.0 (protocol build 4), Translate Companion 0.4.0 |
| U3 | The download link is https://github.com/Mouchoir/WoW-Chat-Translator/releases | One constant per side: `ns.COMPANION_URL` (`Core.lua`), `tp_helper::RELEASES_URL` (`lib.rs`) |

### L.2 Integration review findings

| Finding (severity, review) | Status |
|---|---|
| An inline lane whose card's rows left the top of the chat stayed visible (no card to find it under), for minutes in a quiet channel, and no E2E state reached it (major, addon) | fixed: the Helper remembers each chat lane with its ink and follows it at the chat's top line (orphan lanes, HELPER.md deviation 49); a last row cut by the top is measured from its bottom; the `scroll` scenario now pushes every card out of the top (61 orphan checks, all found) and the truth lists a lane whose tagged row is above the chat |
| `ADDON_BUILD` 3 on both sides: a Helper 0.3.x left every new hint visible with no toast (major, both reviews) | fixed: build 4 on both sides (2 to 4 served), and lanes searched only where the addon's build draws them (deviation 46) |
| A window lane was accepted under a bubble whose last row is hidden (over the reply card's draft line) or on a tooltip's body (major, helper) | fixed: the request's last row must be the card's last visible row and the fill the card's own (deviation 47) |
| A textured world or a feature neither sample sees behind a transparent chat left the gold hint visible under a painted card (major, helper) | fixed: local ground and texture per column for the columns failing the gradient (deviation 48); pixel tests up to +-30 texture and 20 to 45-level features; +-40 leaves some columns (documented), in-game check G19 step 4 |
| `/tpr test rows`: the addon's own print under the test rows was painted over (major, helper) | fixed: no lane search for slot 127, and the indent left of the tag column must be free of ink (the print's prefix starts there) |
| Window hints stayed shown during a drag, a resize or a scroll, so they flashed while the Helper ran (minor, addon) | fixed: hidden while dragged or resized, and the reveal waits 1.5 s again after any move (ADDON.md decision 62); G19 step 4 lists the inherent inline flash |
| The window lane's 2-unit margin is 1.6 px at 0.8 px per unit (minor, both) | fixed: 2 units and 2 px at least, the font a point under its box, and the Helper accepts the hint's glyph edges in a window lane's edge rows (decision 61, deviation 47); the E2E window draws its hint 2 px clear |
| [Download] in a bubble swallowed right-clicks and the hover (minor, addon) | fixed: forwarded to the bubble (decision 55) |
| A tooltip edge in a chat lane's middle rows over a dark chat was painted over (minor, helper) | fixed: straight horizontal cover edges inside the lane fail their columns (deviation 48) |
| The French "older than the addon" toast was 1,106 px on one line (minor, helper) | fixed: shortened; every toast at most 800 px, tested (deviation 51) |
| [Download] stays clickable under the paint (minor, helper) | kept and documented (decision 55): a left click opens the dialog, the rest goes to the bubble |
| Stale names and docs (DESIGN, BRIDGE, a Bridge.lua comment, the exe size, the dialog's anchor) (minor, both) | fixed |
| A revealed inline hint moves one capture before the overlay on a scroll or a new line (minor, helper) | kept, inherent (as the tags); G19 step 4 reworded |

### L.3 Integration results (2026-10-19)

Same PC, the WoW client running the whole time and the user playing: every window-level test used
fakewow off screen or a small window at the bottom of the z-order, the E2E Helper ran with
`--no-overlay --no-clipboard` and `--instance-name=e2e`, and GDI E2E was deferred (it reads the
screen). The user's Helper 0.3.0 (`TranslateProbeHelper.exe`) was stopped with `taskkill` without
/F (it exited in 0.7 s; no llama-server was running), the release built, the stale
`TranslateProbeHelper.exe` (and its `.pdb` and `.d`) removed from `helper\app\target\release`, the
addon deployed with `scripts\dev.ps1`, and Translate Companion 0.4.0 restarted detached (its log:
Start 0.4.0, settings loaded, tray icon, WowFound, WGC capture started).

**Offline suites**

| Command | Result |
|---|---|
| `npm test` (enUS, frFR, bridge) | enUS 663/663, frFR 663/663; bridge byte identity: 30 traces, 16,892 ticks, 1,066 frames, 1,024 tags, 46 builder vectors + 6 parser-only, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 663/663 each |
| `luacheck addon` | 0 warnings, 0 errors in 23 files |
| `python -I tools\bridge\make_vectors.py <dir>` | 30 traces, 1,066 frames, 387 messages, 1,024 tags, 52 message vectors, 36 strips; the 33 files byte-identical to `tests\vectors` (no message changed) |
| `cargo test --offline`, `helper\bridge` | 62 passed, 2 ignored; `--release -- --ignored`: 2 passed (200,000 corrupted captures, 0 undetected; at 1080p strip decode 11.3 us, view scan 283.5 us, cards 246.5 us) |
| `cargo test --offline`, `helper\engine` | 99 + 5 passed, 3 ignored (the ignored real-server smoke tests were not run: they load 4 CPU threads while the user plays, and the engine changed only in a doc comment) |
| `cargo test --offline`, `helper\app` | lib 160, clipprobe 4, graceful_exit 2, hint_lane 5, native_look 5, sentinel 2, tagscan_equiv 5 passed (5 ignored); `window_pipeline --ignored`: 2 passed; lane search (release, a 401 px lane): chat 152 us, chat over a busy world 240 to 270 us, window 19 to 24 us |
| `cargo test --offline`, `helper\testkit` | 19 passed |
| `cargo clippy --all-targets` (app, testkit) | no warning in the changed files |
| `scripts\helper.ps1 -Build` | `WoWTranslateCompanion.exe` 1,340,416 bytes; `--version` "Translate Companion 0.4.0" |
| `python -I tests\e2e\check.py --selftest` (basic, cover, late_start, conv, scroll truths) | all pass: the perfect diag passes (scroll: 63 orphan checks), the shifted one fails bands and lanes, no lane and a lane a row too low fail lanes, the broken window fails window_fill |

**End to end, echo translator, WGC, `--no-overlay --no-clipboard`**

| Scenario | TRANSLATE P99 ms | band failures / checks | lanes found / checks | orphan lanes | false lanes | revealed | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|
| basic | 141.9 | 0 / 3518 | 1216 / 1216 | - | 0 | 3 | 3.87 | 2.10 |
| scroll | 135.3 | 0 / 3051 | 1923 / 1923 | 61 / 61 | 0 | 3 | 1.55 | 1.13 |
| fade | 88.4 | 0 / 1462 | 974 / 974 | - | 0 | 2 | 3.44 | 2.31 |
| cover | 94.5 | 0 / 3958 | 1025 / 1025 | - | 0 | 2 | 3.31 | 2.01 |
| tabs | 93.0 | 0 / 2001 | 1345 / 1345 | - | 0 | 4 | 3.56 | 2.22 |
| altz | 95.2 | 0 / 2754 | 801 / 801 | - | 0 | 2 | 3.08 | 1.71 |
| dropped | 167.6 | 0 / 1360 | 925 / 925 | - | 0 | 2 | 3.31 | 1.83 |
| late_start | 64.4 | 0 / 1662 | 831 / 831 | - | 0 | 3 | 3.74 | 1.72 |
| gamma22 | 103.9 | 0 / 3520 | 1217 / 1217 | - | 0 | 3 | 2.86 | 1.64 |
| conv | 131.5 | 0 / 3004 | 1457 / 1457 | - | 0 | 4 | 4.91 | 2.96 |

- Every functional check passes in all 10 scenarios (latency, bands, hidden, masks, missing, done,
  lanes; conv: window_fill, reply_status, partial_rows, no_duplicate). The CPU rule (3% of one core
  over the whole run) fails in 8 of 10, as it did for the helper package's build (7 of 10, 2.80 to
  4.64%) on this machine with the game running; conv also fails `cpu_vs_i2`. Two more conv runs
  gave 3.95% (2.37% after 2 s) and 4.21% (2.52%): the spread between identical runs is larger than
  any difference from the review fixes. `scroll` is 45 s long now, so its start-up weighs less.
- A first `scroll` run of the integration build (before the orphan search required the lane's own
  ink) passed every orphan check but painted a 4 px sliver for one more line where the truth had no
  lane any more (the line under it): `false_lanes` 82, then 20 once the truth listed cut lanes,
  then 0.
- Log sentinel grep (the scenarios' chat words and sender names, the hint's text,
  "ZZSENTINELZZ") over the 65 diag, truth, report, fakewow and log files of these runs: 0 hits.
- No llama-server ran during the suites; run.py reported no leftover process; every cargo, node,
  python and fakewow process started here has exited.

## M. Links of a message and tokens to copy (2026-10-19, user requests L1-L4)

The component docs hold the details: `docs\ADDON.md` ("Links of a message", decisions 63-73),
`docs\BRIDGE.md` (sections 3a and 3b, STATUS build 5), `docs\ENGINE.md` (section 6 step 4, E1c,
deviations 23-27), `docs\HELPER.md` ("Link colours", build 5) and `docs\IN-GAME-CHECKS.md` (G20).

### M.1 User requests

| # | Request | As built |
|---|---|---|
| L1 | A WTS line showed "\|cnIQ3:[Necromancer Leggings]" in a bubble: strip every 12.x colour escape | `\|cAARRGGBB`, `\|cn<name>:` and `\|r` (grammar read in Gethe/wow-ui-source live and forever) stripped wherever the addon reads or measures text (`Links.StripColors`, `Links.Scan`); a typed `\|\|` is never an escape |
| L2 | Keep the links usable in the conversation window | A links bar inside each bubble with links, under the hint lane (the Helper's lane search never reaches it), every `\|H...\|h` of the message in its own colour: click = `SetItemRef` as the chat, hover = `GameTooltip:SetHyperlink` (else name and hints), Shift-click = `ChatFrameUtil.InsertLink` (the client's own handling for player, Battle.net, channel, appearance... links), Ctrl-click = dress-up. The Original text and my own bubbles' text are hyperlink-enabled the same way |
| L3 | Link names painted in their colours | `PromptText` resolves each link's colour (`\|cAARRGGBB`, `\|cnIQ<n>:` through the client's quality colour API with the user's overrides, `\|cn<NAME>:`, gold when unknown); the addon sends the colours in a COLOURS message right before the TRANSLATE (build 5; the TRANSLATE text never carries a colour code, so a Companion of build 4 paints plain names and shows its toast); the Companion writes them into the marks and draws each restored name in its colour (wrapping, the 80% shrink, shadow and alpha unchanged) |
| L4 | "JayJay58026 ou Jay55732" dropped at the end of a Chinese yell | Cause: the output length cap (3 x input characters + 40) cut long translations of Chinese lines; a wide character now counts 3. URLs, e-mail addresses and IDs (letters with 2 digits or more, handles with `_` or `#1234`, full-width forms too) become placeholders like item names (still at most 9, marks first); levels, prices, times and ratings in the supported languages' shorthand stay plain. E1 passes with IDs/URLs 15/15 |

### M.2 Integration review findings

| Finding (severity, review) | Status |
|---|---|
| A Companion of build 4 painted "0070DD" before every link name: the colour was in the TRANSLATE text, and its toast came on top of the codes (major, addon) | fixed: the colours travel in COLOURS (0x15), which a build-4 parser drops as an unknown type; the TRANSLATE never carries a 0x04 (BRIDGE.md 3b, ADDON.md decision 71) |
| Levels, prices, raid sizes and times in other languages' shorthand were protected as IDs and came back untranslated (`niv60`, `60лвл`, `5g50`, `50кк`, `10hm`, `20Uhr`, `pvp1800`) (major, companion) | fixed: the word lists cover fr, de, es, pt, it and ru shorthand and ratings; E1 lines fr22, fr23, en16, de08 with the new `free=` check (ENGINE.md deviation 27) |
| My own bubbles had no clickable links (minor, addon) | fixed: the out text is hyperlink-enabled while it has links (decision 72) |
| Shift-click inserted the raw link for types the client handles itself (minor, addon) | fixed: player, Battle.net, channel, appearance, club finder, api and instance lock links go straight to `SetItemRef` (decision 65) |
| `SetHyperlinksEnabled` deferred to after combat on a likely misreading of the protected flag (minor, addon) | kept as a conservative choice, decision 67 reworded; G20 step 7 settles it in game |
| README, DESIGN and BRIDGE still said build 4 and 52 message vectors (minor, addon) | fixed for the current state (this section, the BRIDGE validation table); the tables of sections I to L keep the numbers of their dates |
| The `/tpr test` samples used the old `\|cff` item form (minor, addon) | fixed: `\|cnIQ1:` (decision 73) |
| Chat words joined by a dot were protected as domains (`add.me`, `merci.de`, `lol.gg`) (minor, companion) | fixed: known hosts, a first label of 4 characters or 3 labels for a bare domain |
| Full-width IDs were never protected and came back respelled (minor, companion) | fixed: full-width letters, digits and punctuation read as ASCII, protected as written; E1 line en17 |
| The docs said the build-4 toast replaced the painting (minor, companion) | moot with COLOURS: the build-4 Companion paints plain names; docs updated |
| A spell's colour (71D5FF) is close to the default translation colour (A8D8FF) (minor, companion) | kept; G20 step 10 checks the legibility in game (an underline is the planned fix) |

### M.3 Integration results (2026-10-19)

Same PC, the WoW client running and the user playing. Builds and tests used a target folder of
their own; window-level tests ran off screen or at the bottom of the z-order with
`--no-overlay --no-clipboard` and their own instance name. The E2E suite ran on a release build of
the final tree from that folder while the user's Translate Companion 0.4.0 kept running; then the
user's Companion was stopped with `taskkill` without /F (it exited in 0.2 s, no llama-server
running), `scripts\helper.ps1 -Build` gave `WoWTranslateCompanion.exe` 1,374,208 bytes
(`--version` "Translate Companion 0.5.0"), the addon was deployed with `scripts\dev.ps1` (every file
identical in the AddOns folder), and Translate Companion 0.5.0 was restarted detached (log: Start
0.5.0, settings loaded, tray icon, WowFound, WGC capture started; 58 s without a Companion). A
`basic` E2E run with that release exe gave the same results as below.

**Offline suites**

| Command | Result |
|---|---|
| `npm test` (enUS, frFR, bridge) | enUS 755/755, frFR 755/755; bridge byte identity: 30 traces, 16,892 ticks, 1,066 frames, 1,024 tags, 62 builder vectors (11 COLOURS) + 10 parser-only, 12 marks, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 755/755 each |
| `luacheck addon` | 0 warnings, 0 errors in 24 files |
| `python -I tools\bridge\make_vectors.py <dir>` | 30 traces, 1,066 frames, 387 messages, 1,024 tags, 72 message vectors, 36 strips, marks 12 encode + 11 decode + 7 merge; the 34 files byte-identical to `tests\vectors` (frames, tags and strips unchanged, so `sim.py` and `crc_mc.py` were not run again) |
| `cargo test --offline`, `helper\bridge` | 73 passed, 2 ignored; `--release -- --ignored`: 2 passed (corruption suite; at 1080p strip decode 11.2 us, view scan 289.0 us, cards 250.1 us) |
| `cargo test --offline`, `helper\engine` | 114 + 5 passed, 3 ignored; `real_server` (`--ignored`, CPU, 4 threads, 60 s watchdog): passed, server ready in 8.4 s, first result 13.3 s, the cached one at once |
| `cargo test --offline`, `helper\app` | lib 168, clipprobe 4, graceful_exit 2, hint_lane 5, native_look 5, sentinel 2, tagscan_equiv 5 passed (5 ignored); `--release -- --ignored`: window_pipeline 2/2, render 0.623 ms, candidates 0.110 ms, lane search 156.3 / 20.1 / 252.6 us, `sample_card` 384.4 us |
| `cargo test --offline`, `helper\testkit` | 19 passed |
| `cargo clippy --all-targets` (bridge, engine, app) | no warning in the changed code (older warnings elsewhere unchanged) |

**E1 on CPU** (`docs\ENGINE.md` E1c, 59 synthetic lines, shipped tuning): before the review fixes
script 54/54, item names 23/23, must-use 10/10, IDs/URLs 14/15, free tokens 0/4, wrong Same 0:
fail; after: the same with IDs/URLs **15/15** and free tokens **4/4**: pass. The 54 older lines gave
identical per-line results. Each run: 10 servers, each killed and seen exited, the watchdog never
fired, at most one llama-server at any second, none left.

**End to end, echo translator, WGC, `--no-overlay --no-clipboard`**

| Scenario | TRANSLATE P99 ms | band failures / checks | lanes found / checks | orphan lanes | false lanes | COLOURS received | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|
| basic | 146.2 | 0 / 3460 | 1196 / 1196 | - | 0 | 1 | 3.85 | 2.06 |
| scroll | 95.6 | 0 / 3043 | 1920 / 1920 | 63 / 63 | 0 | 0 | 2.26 | 1.60 |
| fade | 84.7 | 0 / 1462 | 974 / 974 | - | 0 | 0 | 3.48 | 2.35 |
| cover | 146.3 | 0 / 3947 | 1022 / 1022 | - | 0 | 1 | 2.55 | 1.50 |
| tabs | 166.9 | 0 / 1992 | 1338 / 1338 | - | 0 | 0 | 3.30 | 1.88 |
| altz | 136.8 | 0 / 2750 | 799 / 799 | - | 0 | 0 | 2.94 | 1.41 |
| dropped | 166.0 | 0 / 1360 | 925 / 925 | - | 0 | 0 | 3.02 | 1.52 |
| late_start | 76.1 | 0 / 1662 | 831 / 831 | - | 0 | 0 | 3.81 | 1.58 |
| gamma22 | 164.4 | 0 / 3487 | 1205 / 1205 | - | 0 | 1 | 3.70 | 2.07 |
| conv | 142.0 | 0 / 2982 | 1447 / 1447 | - | 0 | 0 | 4.19 | 2.49 |

- Every functional check passes in all 10 scenarios (latency, bands, hidden, masks, missing,
  done, lanes; conv: window_fill, reply_status, partial_rows, no_duplicate). The CPU rule (3% of
  one core over the whole run) fails in 7 of 10 (8 of 10 at L.3) with the game running; conv also
  fails `cpu_vs_i2`. The lines with an item link (basic, cover, gamma22) sent their COLOURS, and
  their cards pass every check (the checks measure where the text is drawn, not its colour; the
  colour is checked by the overlay's unit tests and G20).
- A grep of the 50 diag, truth, report, fakewow and log files for the scenarios' chat words and
  sender names: 0 hits. run.py reported no leftover process.
- No process started here is still running except the user's Translate Companion 0.5.0.

## N. Translate Companion presence and compact bubbles (2026-10-19, user requests R1-R2)

Built from the lead's design D1-D6 in two work packages (addon, companion), reviewed once per side,
then integrated (2026-10-20). Versions: addon 0.7.0 (protocol build 6) and Translate Companion
0.6.0, which go together. The component docs hold the details: `docs\ADDON.md` ("Translate
Companion presence", decisions 74-90), `docs\HELPER.md` ("Status file", "Presence card", "Hint
lanes (removed ...)", deviations 52-61), `docs\BRIDGE.md` (STATUS build 6, section 8d),
`docs\ENGINE.md` (E1d), `tests\e2e\README.md` (scenarios `presence` and `presence_late`, the
`presence` check) and `docs\IN-GAME-CHECKS.md` (G21).

### N.1 User requests

An in-game screenshot: Translate Companion ran and painted the translation, but the gold "Start
Translate Companion. [Download]" stayed under the bubble (the lane cover failed under a bubble
with a links bar), the bubble had big empty bands (3 reserved rows for a 1-row translation, the
lane, the links bar) and the item name appeared twice.

| # | Request | As built |
|---|---|---|
| R1 | Detect a running Translate Companion; never offer to start or download it while it runs | The status file (D1): the addon ships `CompanionStatus.lua` (`TranslateProbe_CompanionStatus = nil`, in the TOC before `Core.lua`); Translate Companion overwrites the deployed copy of that one file in every WoW flavour folder whose TOC lists it (product.db or settings `wowRoot`), atomically, with `{ v, version, build, state, heartbeat }`: at its start, every 60 s, within 5 s of a deploy that put the nil file back, and "stopped" on a clean exit. At `ADDON_LOADED`: "running" (state running, heartbeat within 150 s) confirms the session, and [T] acts at once: no popup, no start or download offer |
| R1 | [T] without it: only a popup in the centre, no window, no chat line | `Presence.Ensure` in front of every way to a translation or the window ([T] plain, Ctrl, Shift, an old-session [T], a bubble's Translate and click, the reply's Translate, the key bindings, `/tpr conv`): unconfirmed, only the popup opens (D3 texts, [Continue] [Download] [Cancel], centred, Escape closes it, never the keyboard focus); Continue confirms the session and runs the click; Cancel and Escape drop it. Hint lanes, their timers and strings and the first-use chat line are gone (D4) |
| R1 | A Translate Companion started later is detected, ideally without /reload | The popup's status card (one row of slot 125, VIEW 201): a running Translate Companion 0.6.0 paints "Translate Companion detected" in green over it (E2E: 104 ms after its start by its discovery scan, 210 ms through VIEW 201), and the user clicks Continue. The addon itself cannot learn it (below), so the popup's text and [Download] stay as they were when it opened |
| R2 | The [Encantamiento] painted at the top is not clickable; keep the bottom one otherwise | Not possible: the painted name is pixels of Translate Companion's click-through overlay, a window of another process, and the addon never learns where it was painted. The clickable links bar under the bubble stays |
| R2 | No big empty spaces | Compact bubbles (D5): no lane row, the links bar 4 units (4 px at least) under the last row, rows estimated from the bubble's own text width with link names counted as written and a Latin-to-Latin factor of 1.25 (E1d lengths, `tests\eval\out\e1d_lengths.json`); "Cherche [Encantamiento: Encantar arma 2M: impacto]" reserves 1 row at the in-game font width (2 in the mock), 3 before; the Companion paints from the first row down and fills unused rows with the bubble fill |

**Why a Translate Companion started later still needs one click.** Data goes one way only while the
game runs: from the addon to Translate Companion, as pixels. Addon Lua can read no file, socket,
clipboard or pixel; the client reads the addon's own files at login and at `/reload` only, and the
status file uses exactly that. So a Translate Companion running when the addon loads is detected
with no popup at all; one started later cannot be known to the addon before the next `/reload`. The
popup's card is the live proof (Translate Companion paints it, about a second after its start), and
Continue is the user's confirmation; it lasts until logout or `/reload`. Best practice: start
Translate Companion before the game, or `/reload` once after starting it.

### N.2 Integration review findings

| Finding (severity, review) | Status |
|---|---|
| Translate Companion 0.6.0 served addon builds 3 to 5 without a toast while their gold hint lanes, no longer painted over, stayed under every translation: the user's bug, unexplained (major, companion) | fixed: `ADDON_BUILD_MIN` 6, builds 1 to 5 get "the addon is older" once per session (HELPER.md deviation 58) |
| The popup's `OnHide` dropped the waiting click when the whole UI hid (Alt+Z, a cinematic); Continue then ran nothing (minor, both) | fixed: an ancestor hide keeps the click, the Escape hold and the events; a harness case hides UIParent (ADDON.md decision 87) |
| The docs said an older Translate Companion leaves the card unpainted; 0.5.0 shows "Not received - click the bubble" there and writes no status file (minor, both) | fixed in ADDON, BRIDGE, HELPER, IN-GAME-CHECKS and README; 0.6.0 restarted with addon 0.7.0 deployed |
| README and DESIGN still described the gold hint lane and [Download] (minor, both) | fixed: README rewritten (versions, "Is Translate Companion running?", troubleshooting, tests); DESIGN A.10, C.1, C.3, E.2, F, H, K.2, K.3 updated, section L marked removed |
| `harness_conv.lua` accepted slot 125 for a bubble (minor, addon) | fixed: `Reserve.WIN_FIRST..WIN_LAST` (decision 90) |
| The card's 6-unit left fill was shorter than the discovery scan's left sample: a darker strip until the next VIEW 201 (minor, both) | fixed on the addon side: `left = max(pad, ceil((0.75 rowPx + 3) / ppu))` units (decision 89). E2E `presence_late`: discovery samples (36, 30, 24), the fill, against (18, 16, 14) in the package's run; check.py now judges them; `presence_card.rs` draws the addon's metrics at 1080p and fails with the old fill |
| A Translate Companion started later: the popup still says "not installed or not running ... Download it" while its card turns green (minor, both) | kept: the texts are the lead's (D3) and the addon cannot learn the start; README ("Is Translate Companion running?") and section N.1 say so |
| A [T] of a line from before the last `/reload` printed a chat line with no confirmed Translate Companion (minor, addon) | fixed: through `Presence.Ensure`, the notice after Continue (decision 88) |
| The evidence for the 1.25 factor was not in the repo (minor, addon) | fixed: `evalprompt` keeps per-line lengths (counts only); E1d, `tests\eval\out\e1d_lengths.json` (ENGINE.md E1d, ADDON.md decision 79) |
| The E2E popup is synthetic: the addon's real geometry and VIEW 201 never reached the Companion (minor, addon) | partly: the rig's left fill follows `CompanionDialog.DISCOVERY_LEFT`, `presence_card.rs` uses `CardMetrics` at 1080p with fractional px and VIEW 201 as the addon rounds it, G21 step 5 checks two UI scales in game; the rig still draws integer px at one size |
| `write_atomic` followed links: a hard link at the `.tmp` name was written through, a junctioned addon folder made the Companion rewrite the repo's file (minor, companion) | fixed: the `.tmp` name removed then `create_new`, reparse points skipped and counted; tests with a hard link and a `mklink /J` junction in temp folders (deviation 59) |
| The status file stayed nil up to 60 s after a deploy; a refused "stopped" left "running" for up to 150 s (minor, companion) | fixed: a check every 5 s writes over a file that is not the Companion's own; "stopped" retried for 2.5 s (deviation 60); measured live: the shipped file copied over the deployed one 20 s after a heartbeat was rewritten 0.3 s later |
| Leftovers: the old caption in two comments, the testkit's lane truth, clippy `zombie_processes` in `graceful_exit.rs` (minor, companion) | fixed (the same warning in `window_pipeline.rs`, older, too) |

### N.3 Integration results (2026-10-20)

Same PC. The WoW client was not running during these runs (the user had stopped playing); window
tests still ran off screen or at the bottom of the z-order with `--no-overlay --no-clipboard`, and
every build and test used a target folder of its own except `scripts\helper.ps1 -Build`. The
user's Translate Companion 0.5.0 was stopped with `taskkill` without /F (it exited in 0.3 s; no
llama-server was running) before the release build. `scripts\dev.ps1` then deployed the addon (28
files identical; `CompanionStatus.lua` copied as shipped, none existed), and Translate Companion
0.6.0 was restarted detached: its log has Start 0.6.0, Settings loaded, the engine's Discovery,
TrayIcon and `StatusFile { state: "running", roots: 1, targets: 1, written: 1, failed: 0, skipped:
0 }`, and the `_classic_beta_` file says "running" with the heartbeat of its start; `WowFound` comes
when the game starts. A second deploy kept that file byte for byte (`/XF`) and updated the rest.

**Offline suites**

| Command | Result |
|---|---|
| `npm test` (enUS, frFR, bridge) | enUS 789/789, frFR 789/789; bridge byte identity: 30 traces, 16,892 ticks, 1,066 frames, 1,024 tags, 62 builder vectors (+10 parser-only; 11 COLOURS), 12 marks, 29 API checks, all passed |
| `node tests\run.js enUS --stub-bridge` / `frFR --stub-bridge` | 789/789 each |
| `luacheck addon` | 0 warnings, 0 errors in 26 files |
| `python -I tools\bridge\make_vectors.py <dir>` | 30 traces, 1,066 frames, 387 messages, 1,024 tags, 72 message vectors, 36 strips; the 34 generated files byte-identical to `tests\vectors` (no message changed) |
| `cargo test --offline`, `helper\bridge` | 73 passed, 2 ignored; `--release -- --ignored`: 2 passed (200,000 corrupted captures, 0 undetected; at 1080p strip decode 12.8 us, view scan 325.5 us, cards 244.2 us) |
| `cargo test --offline`, `helper\engine` | 114 + 5 passed, 3 ignored; `--ignored` (CPU, 4 threads, a 60 s watchdog that never acted): 3 passed, `real_server_cpu_smoke` server ready in 10.5 s, first result 15.5 s, the cached one at once, shutdown 168 ms |
| `cargo test --offline`, `helper\app` | lib 160, clipprobe 4, graceful_exit 3, native_look 5, presence_card 2, sentinel 2, tagscan_equiv 5 passed (6 ignored); `--release -- --ignored`: window_pipeline 2/2, render 0.663 ms, candidates 0.116 ms, `sample_card` 396.1 us, discovery scan 0.663 ms (whole client) and 1.421 ms (with the presence areas) |
| `cargo test --offline`, `helper\testkit` | 17 passed (the 2 lane truth tests went with the code) |
| `cargo clippy --all-targets` (app, testkit, engine) | no warning in the changed code; older warnings elsewhere unchanged |
| `scripts\helper.ps1 -Build` | `WoWTranslateCompanion.exe` 1,336,832 bytes; `--version` "Translate Companion 0.6.0" |
| `python -I tests\e2e\check.py --selftest` (basic, cover, conv, scroll, late_start, presence, presence_late truths) | all pass: the perfect diag passes, the shifted one fails bands (cover: and masks), the broken window fails window_fill, a diag that takes slot 125 for an unknown slot fails presence |
| E1d (`evalprompt`, CPU, `docs\ENGINE.md`) | every gate passes, per-line results identical to `out\report.json`; 10 servers each killed and seen exited, at most one at a time, none left; Latin to Latin 30+ characters: P90 1.387, max 1.421 |

**End to end, echo translator, WGC, `--no-overlay --no-clipboard`**

| Scenario | TRANSLATE P99 ms | band failures / checks | hidden | masks | missing | done | presence | CPU whole run % | CPU after 2 s % |
|---|---|---|---|---|---|---|---|---|---|
| basic | 184.8 | 0 / 3507 | pass | pass | pass (0) | 3/3 | - | 3.71 | 1.75 |
| scroll | 88.0 | 0 / 3349 | pass | pass | pass (0) | 3/3 | - | 1.89 | 1.27 |
| fade | 105.9 | 0 / 1317 | pass | pass | pass (0) | 2/2 | - | 2.64 | 1.67 |
| cover | 107.6 | 0 / 3963 | pass | pass | pass (0) | 2/2 | - | 2.75 | 1.59 |
| tabs | 108.6 | 0 / 1997 | pass | pass | pass (0) | 2/2 | - | 2.81 | 1.67 |
| altz | 98.1 | 0 / 2768 | pass | pass | pass (0) | 2/2 | - | 2.81 | 1.40 |
| dropped | 82.8 | 0 / 1365 | pass | pass | pass (1) | 2/2 | - | 3.52 | 1.86 |
| late_start | 57.6 | 0 / 1668 | pass | pass | pass (3) | 1/1 | - | 3.15 | 1.30 |
| gamma22 | 101.7 | 0 / 3513 | pass | pass | pass (0) | 3/3 | - | 2.40 | 1.24 |
| conv | 121.0 | 0 / 3009 | pass | pass | pass (0) | 4/4 | - | 4.03 | 2.25 |
| presence | 45.3 | 0 / 593 | pass | pass | pass (0) | 1/1 | pass: 209.8 ms, VIEW 201 | 3.25 | 1.40 |
| presence_late | 96.1 | 0 / 505 | pass | pass | pass (0) | 1/1 | pass: 104.1 ms, discovery, samples (36, 30, 24) | 3.51 | 1.32 |

- Every functional check passes in all 12 scenarios (conv: window_fill, reply_status,
  partial_rows, no_duplicate too). The CPU rule (3% of one core over the whole run) fails in 6 of
  12 (3.15 to 4.03%; 1.24 to 2.25% after the first 2 s), as in every run on this machine; conv
  also fails `cpu_vs_i2`. A 25 s release build of `evalprompt` overlapped the first scenarios.
- A grep of the 60 diag, truth, report, fakewow and log files for the words of the scenarios'
  player lines and their senders (24 words, from the recordings): 0 hits. run.py reported no
  leftover process.

### N.4 In-game steps (G21 has the full list)

1. `/reload` with Translate Companion 0.6.0 running (it is): the status file is read at load, so
   [T] opens the conversation window at once, with no popup.
2. Exit Translate Companion from its icon, `/reload`, click [T]: only the popup in the middle of the
   screen ("installed but not running"), nothing in the chat, no window.
3. With the popup open, start Translate Companion: within about two seconds the dark strip reads
   "Translate Companion detected" in green; Continue opens the window with the line.
4. A line like "Cherche [Encantamiento: Encantar arma 2M: impacto]": a compact bubble, one row
   (two at most) of translation, the links bar right under it, no gold hint anywhere.
