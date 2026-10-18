# TranslateProbe: design as built

TranslateProbe puts a [T] link after each readable player line in the WoW Forever chat. A click
reserves a few rows under the line and asks **TranslateProbe Helper**, a Windows app running beside
the game, to translate the line with a local AI model (llama.cpp and Qwen3 4B). The Helper draws the
translation on those rows. Nothing leaves the PC, and the Helper never touches the game process:
the addon and the Helper talk through pixels only.

This file is the design as it was built (addon 0.2.0, Helper 0.1.0). The detailed specs, the
measurements behind them and every deviation from the original plan are in:

| File | Part |
|---|---|
| `docs\ADDON.md` | the addon modules, tests and decisions |
| `docs\BRIDGE.md` | the pixel bridge v2 (normative): strip, frames, messages, tags, cards, validation |
| `docs\ENGINE.md` | the translation engine: discovery, llama-server lifecycle, prompt, evaluations |
| `docs\HELPER.md` | the Helper app: threads, capture, overlay, CLI, diagnostics, CPU measurements |
| `tests\e2e\README.md` | the end-to-end rig (fakewow) and its checks |
| `docs\IN-GAME-CHECKS.md` | what only the real client can show (G1-G8) |

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
- Never: a handle to the game process, memory reads, input injection, or WinEvent hooks (the
  foreground window is polled).

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
- On a click the addon appends K reserved rows (1 to 4) under the line. Each row is a **tag**
  (8 inline colour blocks: a magenta/green sync pair and 6 base-6 digits carrying slot, row,
  session bit and a CRC-5) followed by a **plate** (one dark inline texture, RGB 32,40,64).
- WoW's own chat renderer lays the rows out, scrolls, clips, fades and hides them. Lua geometry
  never positions a line; the VIEW message (chat clip rectangles and font size) only bounds the
  scan, sizes the overlay windows and sets the font.
- Each capture, the Helper finds the tags, measures the plate right of each tag and draws only on
  plate pixels it sees right now (the mask), so a tooltip or menu over part of a row is never
  painted over. A row whose plate is expected but not found is dropped.
- Once read, each tag is painted over (opaque, in the observed plate colour, including its blended
  edge pixels), so the coloured blocks are hidden while the Helper runs. Without the Helper the
  tags stay visible.
- A display curve between the game and the capture (the game's Gamma setting, contrast) is
  measured on the strip's calibration blocks; when it is not linear, the scan area is linearized
  before the tag scan and the cards.
- Overlay windows are layered, topmost, click-through and never activate; one per chat view,
  hidden when empty.

**5. llama-server lifecycle and resources** - details in `docs\ENGINE.md`
- Started on the first translation request only; hidden, below-normal priority, in a
  kill-on-close job object; 127.0.0.1 on a free port with a random API key in a key file.
- Flags `-c 1024 -np 1 --fit off -lm none --no-ui --offline` when the probed `--help` lists them.
- GPU (Vulkan first, else CUDA on NVIDIA) with full offload when free VRAM is at least
  2900 MB + a 1500 MB margin; otherwise CPU with `clamp(physicalCores / 2, 2, 4)` threads, which
  needs 3600 MB of available RAM. No partial offload.
- Unloaded after 10 minutes idle, or 60 s after the game window disappears. A 15 s VRAM guard
  moves the session to CPU below 700 MB free.

**6. Prompt** (`PROMPT_VERSION` 2)
- Raw ChatML with thinking disabled. The default system prompt asks for natural, casual chat in
  the target language, keeping placeholders, player names and game abbreviations, using official
  WoW names, and replying with the translation only.
- Item names and `{...}` groups become placeholders `{1}..{9}`; a missing or duplicated
  placeholder triggers one retry with the names in brackets.
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
| Helper not running | rows stay dark; tooltips on [T] and rows name the Helper; the first click ever prints FIRST_USE once per account | - |
| Helper started after the click | same | "Not received - click [T] again" once a plate has been seen for 1.5 s without a request |
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
- **Saved variables** (`TranslateProbeDB`, schema 2): on every load, keys outside the whitelist
  `schema, target, names, glossShow, mode, glossary, helpShown` are deleted (the old telemetry
  goes away), then each kept value is validated. Message text, sender names, line ids, request
  ids and slots are never stored.
- **Slash commands** (`/tpr`, `/tprobe`): `info`, `target <code>`, `names on|off`, `show on|off`,
  `mode auto|dense|robust|mono`, `translit <text>`, `gloss add|del|list|test`, `test`,
  `test rows`, `test lockdown`, `clear`, `app`, `debug`, `help`. The old `bench`, `model`,
  `budget`, `speed`, `simulate`, `free`, `log`, `catchup`, `report` and `reset` are gone.
- Targets: en, fr, de, es, it, pt, ru, ko, zh, zh-TW (the default follows the client locale).

---

## C. The addon

### C.1 Modules (TOC order)

```
Locales.lua      ns.L (English base, frFR override)
Core.lua         saved variables (whitelist purge), session id, print helpers, inits under pcall
Bridge.lua       strip encoder, message builders, tag and plate markup
Glossary.lua     Describe, Extract, PromptText, VisibleText, PlainText, Romanize
Lang.lua         TARGETS, NAMES, DefaultTarget, Script, TargetScript
Reserve.lua      metrics, row estimate, reserved rows, slots, VIEW collector and watcher, old-session cleanup
ChatButton.lua   filter, [T] decoration, Track, SetSuffix, CutRows, ShownLines, link clicks, tooltips
Request.lua      click flow, request ids, active requests (memory only)
Restriction.lua  restriction watcher, deferred clicks, secret arrivals, reprint
Commands.lua     slash commands
```

### C.2 Data flow

```
CHAT_MSG_* (readable) -> filter (notification only) -> next frame: TransformMessages adds " [T]" -> Track (memory)
click [T] -> Request.Start -> PromptText / Extract / AllocSlot / EstimateRows
          -> SetSuffix (per-frame rows: tag + plate, then glossary and name rows)
          -> Bridge.Send(VIEW if changed) -> Bridge.Send(TRANSLATE)
          -> strip shown (about 0.5 s) -> hidden when the queue drains
Helper: capture -> strip decode -> reassemble -> TRANSLATE -> engine -> status / result
        -> overlay drawn on the plates found in each capture
```

### C.3 Rows and slots

- **Metrics** per chat frame (nil when anything is missing or secret): physical px per unit
  `ppu = effScale * physH / 768`; tag block width `W = max(2, ceil(5 / ppu))` units (at least
  5 physical px); row height `H = max(6, round(fontH))`; plate width
  `floor((chatWidth - indent - 8 W - 2) * 0.88)`, usable from 40 units.
- **Row count** `K = min(4, max(1, ceil(width / plateW * f - 0.15)))`, with `f` from the source and
  target scripts (Latin to Latin 1.25, Han or kana to Latin 2.2, Hangul to Latin 1.8, Latin to Han
  0.8, default 1.3). The width ratio is not rounded up first, so a short line reserves one row.
- **Slots** 0..126 (127 is the `/tpr test rows` slot) come from a free list. A slot is freed when
  its rows are removed, or by the read-only scan once its line has left every chat frame. The scan
  is a `TransformMessages` predicate that records our row links and always returns false, so no
  entry is ever repackaged (no new timestamp, no un-fade). It runs at most every 15 s from the
  watcher while slots are in use, and when the free list is empty; only when all 127 slots are
  held by lines still in the chat does the oldest request expire.
- **VIEW watcher**: a 0.5 s ticker, only while requests are active, that sends VIEW when the chat
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

### C.5 User-visible strings

All in `Locales.lua` (English and frFR). The Helper's strings are in `helper\app\i18n\en.json` and
`fr.json` (status lines in the target language when it is en or fr, else English).

---

## D. TranslateProbe Helper

### D.1 Layout (four crates, no Cargo workspace)

```
helper\.cargo\config.toml   +crt-static, [build] jobs = 3
helper\bridge\    tp_bridge   strip decoder, messages, reassembly, tag scan, cards, synthetic images (feature synth)
helper\engine\    tp_engine   discovery, flags probe, device policy, llama-server, prompt, post-processing, queue, cache
                              bin evalprompt (E1 evaluation)
helper\app\       tp_helper   the app (bin TranslateProbeHelper, bin capprobe)
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
  | Not in front | 5 Hz, tags only |

  Each tick reads the window geometry, grabs the strip (client rows 0..8) and the scan area (the
  union of the VIEW rectangles, or a discovery area at 2 Hz while no VIEW is known), decodes,
  reassembles, feeds the engine, scans tags (only rows where the sync relation can hold; bands
  whose tags did not change keep their readings), builds cards (kept when the scan area is
  pixel-identical), runs the covering test (at most 10 Hz per card) and publishes the scene when
  it changed. WGC copies only the two regions to reused staging textures and waits on a D3D11
  fence, so nothing spins.
- **Engine threads** (tp_engine) and a **log thread**.

### D.4 Overlay drawing

- Per card: the tags are painted over opaque with the observed plate colour; no-plate mode fills
  the text rectangle; gaps of 4 px or less between rows are filled where both neighbouring masks
  are set; then the text over a 1 px shadow, faded by the card alpha, only on mask pixels.
- Fonts: "Arial Narrow" (else "Arial" narrowed) for Latin and Cyrillic, "Microsoft YaHei UI" (zh),
  "Microsoft JhengHei UI" (zh-TW), "Malgun Gothic" (ko); size from VIEW, capped by the band height;
  text rasterized at 3x and filtered down.
- Fit: wrap into K lines at the card's text width, else at 85% size, else cut with "...". Line i
  is drawn in band i, so rows clipped at the top of the chat still show the right lines.
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
  `--assume-foreground` and `--overlay-probe`.
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

### E.2 End to end, without the game

`python -I tests\e2e\run.py --all --capture wgc` plays every scenario (basic, scroll, fade, cover,
tabs, altz, dropped, late_start, gamma22) in fakewow, placed off every monitor, with the Helper
on the echo translator and `--no-overlay` (no window is ever shown), then checks latency, band
positions, hiding, masks, "Not received", completion and the Helper's CPU use. `--capture gdi`
needs fakewow on screen and runs only when no game client is running. The real model runs with
`python -I tests\e2e\run.py --scenario basic --translator llama --helper-arg=--backend=cpu --helper-arg=--runtime=<runtime folder> --fakewow-arg=--tail-ms --fakewow-arg=20000 --done-within-s 60`
(the 20 s tail keeps the scene up while the translator starts on the CPU).

### E.3 Results of the integration run

See section I.

---

## F. In-game checks

`docs\IN-GAME-CHECKS.md` lists the setup (close the game, deploy, build and start the Helper) and
the checks G1 (bridge and capture) to G8 (colourblind filter), with what to report.

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

- **Addon** (`docs\ADDON.md` 1-18): extra functions for the scan and the watcher; empty VIEWs never
  sent; the row estimate without rounding first; the periodic read-only scan that lets the watcher
  stop; secret arrivals counted on any secret argument and reprinted only when still shown; mode
  `auto` following the colourblind setting; `Glossary.Lookup` removed.
- **Bridge** (`docs\BRIDGE.md` 10): sync-pair test from the B0/B1 relation; sub-pixel block fit;
  the plate predicted from the tag's own colours (fades over coloured grounds); cover checks;
  two-way mask and look-ahead text end; the strip capped at the screen width; `tag_rect` widened
  over the blended edges and painted opaque; the B1 candidate bounded by the scan area; a display
  curve measured on the strip undone before the tag scan and the cards (section 8b there).
- **Engine** (`docs\ENGINE.md` 11): the casual prompt with a language line, the glossary policy
  (keep-allowed vs must-use) and expansion before the call, `Same` only with evidence, bracket
  repair, `PROMPT_VERSION` 2; NVML matched by PCI ids; the selftest discovery injectable so tests
  never start llama-server.
- **Helper** (`docs\HELPER.md` 1-24): library target; text at 3x; discovery over the full client
  width; `--no-overlay` and `--assume-foreground` for tests; lower idle capture rates, scan gating,
  tag search and card cache for CPU; the tray menu foreground pattern; session-end clean-up; the
  display-curve correction; the sticky text width.
- **E2E rig** (`tests\e2e\README.md`): fakewow off-screen as a layered window (WGC cannot capture a
  normal off-screen window); `run.py`; check.py waits for the VIEW.

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
