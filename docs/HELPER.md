# TranslateProbe Helper (helper\app)

`TranslateProbeHelper.exe` reads the addon's pixel bridge from the WoW window, translates with the
local engine (`helper\engine`, llama.cpp + Qwen3 4B) and draws the translations over the chat rows
the addon reserved. It never opens a handle to the game process, never sends input to it and never
reads its memory: it only looks at the window's pixels.

## Files

| Path | Role |
|---|---|
| `src\main.rs` | Entry point (GUI subsystem: no console window) |
| `src\lib.rs` | Library `tp_helper` holding every module, so `capprobe` and the tests reuse them |
| `src\cli.rs` | Command line |
| `src\settings.rs` | `%APPDATA%\TranslateProbe\settings.json` |
| `src\log.rs` | `logs\helper.log` (1 MB x 3 files) and the `--diag-json` writer |
| `src\i18n.rs`, `i18n\en.json`, `i18n\fr.json` | User-visible strings |
| `src\wow.rs` | Game window lookup, geometry, covering test |
| `src\capture\{mod,wgc,gdi}.rs` | Windows.Graphics.Capture (primary) and GDI `BitBlt` (fallback) |
| `src\tagscan.rs` | Tag search of one capture around `tags::scan` (candidate rows, unchanged tags) and the card cache |
| `src\perf.rs` | Stage timers and process/thread CPU times for the `perf` diag record |
| `src\scene.rs` | Pure state machine: messages and cards in, scene out |
| `src\backdrop.rs` | The chat background around each card's rows, sampled in the current capture (native look) |
| `src\overlay\{mod,window,render,fit,fonts}.rs` | Layered windows, pixel composition, wrapping, fonts |
| `src\toast.rs`, `src\tray.rs` | Bridge toast, notification-area icon |
| `src\app.rs` | Process: UI thread, capture thread, engine wiring |
| `src\bin\capprobe.rs` | Capture self-check on its own window |
| `tests\sentinel.rs` | Log sentinel test |
| `tests\tagscan_equiv.rs` | The tag search and the card cache give exactly what `tags::scan` and `cards::build` give |
| `tests\window_pipeline.rs` | Window-level smoke tests of the whole pipeline (ignored by default) |
| `tests\native_look.rs` | Pixel test: a synthetic capture through tag scan, cards, background sampling and painting; the reserved rows disappear into the ground |
| `tests\graceful_exit.rs` | `taskkill` without /F stops a spawned Helper cleanly (exit code 0, log closed) |

Build, test and run with `scripts\helper.ps1` (`-Build`, `-Test`, `-Run`, `-Diag`, `-RuntimeDir`).
The translator runtime folder comes from `-RuntimeDir`, else `$LlamaRuntimeDir` in the gitignored
`scripts\dev.local.ps1`, else the `TPROBE_LLAMA_RUNTIME` environment variable.

## Command line

```
TranslateProbeHelper.exe [--runtime DIR] [--model FILE] [--backend auto|cpu|vulkan|cuda] [--capture auto|wgc|gdi]
  [--translator llama|echo] [--wow-class CLASS] [--diag] [--diag-json FILE] [--log-dir DIR] [--no-tray]
  [--exit-after SECS] [--selftest]
test flags: [--no-overlay] [--assume-foreground] [--overlay-probe]   other: [--help] [--version]
hidden test flag: [--instance-name NAME] (not in --help)
```

- `--selftest` prints the engine's discovery, flags, VRAM, RAM and backend decision as JSON and
  exits. It never loads the model (it may run `llama-server --help` once to read the flags, then
  caches them in `%LOCALAPPDATA%\TranslateProbe\run\flags.json`).
- `--no-overlay` (amendment A1): cards and toasts are rendered into in-memory DIBs and reported in
  `--diag-json`; no window is ever created. It implies `--assume-foreground` and skips the
  `WindowFromPoint` covering test (see Deviations).
- `--assume-foreground`: the game window counts as foreground (the E2E window never takes focus).
- `--overlay-probe`: no capture; one synthetic 2-row test card (376 x 52 px, bottom right of the
  primary monitor's work area, at most 5 s) checks that `WindowFromPoint` at the card centre never
  returns the overlay and that the foreground window does not change. Prints one JSON line, exit
  code 0 on success. Use `--overlay-probe --diag --exit-after 5 --no-tray`.
- `--instance-name NAME` (tests only, 1-32 ASCII letters, digits, '-' or '_'): the
  single-instance mutex becomes `Local\TranslateProbeHelper.NAME`, so a test Helper can run next
  to the one the user is running.
- Exit codes: 0 normal, 1 failed start or failed probe, 2 another instance is running
  (`Local\TranslateProbeHelper`), 64 bad arguments.
- Stopping: the tray's Exit, `--exit-after`, or `taskkill /IM TranslateProbeHelper.exe` without
  /F. All three end the message loop, then stop the capture thread (waiting at most 3 s), shut the
  engine down (llama-server killed, key file deleted) and close the log (exit code 0). `taskkill
  /F` still works but skips that (the job object still kills llama-server).

## Threads

- **UI thread**: hidden window `TPHelperMain` (a plain top-level window, never shown), tray icon,
  overlay windows (`TPHelperOverlay`, one per VIEW id) and the toast (`TPHelperToast`, created
  hidden at start unless `--no-overlay`). `WM_CLOSE` on any of these windows exits like the tray's
  Exit (see "Measured facts" for why every window needs it). It handles `WM_APP_SCENE`,
  `WM_APP_RESULT`, `WM_APP_STATE` and `WM_APP_EVENT`, renders only on change (once for a burst of
  scene and status messages), and runs a 10 Hz timer only while a status is animated (queued,
  loading, translating). Each view keeps its canvas; only the dirty rectangle is cleared, copied
  and handed to `UpdateLayeredWindowIndirect`; wrapped lines and rasterized text are cached.
- **Capture thread** (MTA): finds the window (`FindWindowEx` on the class, every 2 s while absent),
  grabs the strip (client rows 0..8, full width, only with the game in front) and the scan area
  (union of the VIEW rectangles, or the discovery area at 2 Hz), decodes, reassembles, feeds the
  engine, scans tags, builds cards, runs the covering test (at most 10 Hz per card) and publishes
  the scene when it changed. The Helper speaks `STATUS.addonBuild` 2 (`scene::ADDON_BUILD`; 2:
  TRANSLATE rows 1..6, since 2026-10-18): a strip frame from another build shows the
  "addon is older" or "Helper is older" toast once per session.

  | State | Rate |
  |---|---|
  | No game window | look-up every 2 s, nothing else (the thread sleeps until the next look-up) |
  | Minimized | no capture, 2 Hz check |
  | In front, nothing pending (no strip seen for 2 s, no card visible) | 10 Hz |
  | In front, strip seen in the last 2 s | 20 Hz |
  | In front, card visible | `captureHzActive` (20-60, default 30) |
  | Not in front | 5 Hz, tags only (no strip) |

  The tag scan runs on every capture only while a request of this session is known, the last
  scan found tags, a card is shown or the strip is active; otherwise (and for discovery) at 2 Hz.
  WGC delivers frames at most every 3/4 of the current period (`SetMinUpdateInterval`, changed
  with the rate). WGC with no fresh frame for 3 s in front: the session is recreated once (with a
  new D3D11 device), then (capture `auto`) the Helper switches to GDI and shows the "Windowed
  (Fullscreen)" balloon.

  CPU (package P4b; measured values under "Measured facts"):
  - WGC: only the strip and the scan area are copied out of the frame, each into its own pair of
    reusable staging textures (used in turn, so the previous capture stays readable). All copies
    are issued, then the thread sleeps on an `ID3D11Fence` event until the GPU has done them, then
    the textures are mapped and read in place by the decoder and the tag scan (no copy, no
    allocation per tick) until the end of the tick. The device is created with
    `D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS` and kept across sessions.
  - GDI: two reusable buffers per region; the DIB only grows.
  - Tag search (`tagscan.rs`): an SSE2 pass finds the rows where the sync-pair relation of
    `tags::scan` can hold (B0 above B1 by 3/4 of `LIKE_MIN` on red and blue, below on green, within
    5 px, every second row, the same x range); `tags::scan` runs only on those row bands (widened
    by 6 rows, with the area's own x and width). A band whose tags show the same inner pixels as in
    the previous scan, with the same calibration, keeps their readings.
  - Cards: kept when the tags, views, plate flags, session bit and every pixel of the scan area
    are the same as for the previous result (compared with the grabber's previous capture; the
    comparison stops at the first difference). Each card's text width is then the widest of the
    last 2 s (`cards::StickyWidth`, design step 7), so a short misread does not re-wrap the text.
  - Display curve (`tp_bridge::tone`, `docs\BRIDGE.md` section 8b): each decoded strip frame
    gives the four measured reference levels; when they show a non-linear curve, the scan area is
    copied through the inverse curve before the tag search and the cards (an unchanged capture
    reuses the previous copy), tags are read with the nominal 0 / 255 calibration, and the cards'
    plate colours go back to display colours for the overlay. A `tone` diag record logs each
    change of curve (exponents per channel). A linear calibration costs nothing.
  - Window geometry: the DWM frame bounds are read again only when the window rectangle or client
    origin changes.
  - Diag lines are buffered and written about once a second (and at exit); the per-capture
    records are formatted directly.
- **Engine threads**: owned by `tp_engine`.
- **Log thread**: writes `helper.log`.

## Overlay drawing (native look)

The plate exists only so that the Helper can find the rows; the overlay makes it disappear and the
translation looks like ordinary chat text sitting on the chat background.

- **Background** (`backdrop.rs`), per row band, from the same capture as the card:
  - left sample: the indent between the view's left edge (at most 48 px away) and the tag, up to
    2 px before the tag;
  - right sample: the last 40 px of the view, never left of the plate's end + 2 px, without the
    pixels the mask takes for plate, and only for a row whose plate reaches the card's plate end
    (a plate cut short by a tooltip has the tooltip after it, not the chat). The far end of the
    view is used because a faded plate over a busy ground can be measured shorter than it is.
    No right sample at all when the card's plate end is more than 24 px short of where the addon
    draws it (0.88 of the room between the tag and the view's right edge; measured in game: 1 px
    short): every row is then cut at the same x by something over the chat, such as a frame docked
    on the right or a bag, and the far end of the view lies under it (deviation 30);
  - discovery scan (no VIEW): no right sample (the view is the whole client width), and the left
    span reaches only 0.75 x the band height left of the tag, the indent that is surely chat;
  - both spans are read every second column;
  - each sample is a per-channel median after dropping text-like pixels (more than 28 luma levels
    brighter or 40 chroma levels more saturated than the first median, or more than 48 away from
    it on a channel);
  - a row without a side borrows that side from the nearest row of the card that has it (a row
    whose plate end is under a tooltip still gets the gradient of its neighbours);
  - the colour at column x is the linear gradient between the two samples (flat with one sample).
    Under a display curve the samples go back to display colours like the plate colour;
  - a sample within 4 levels (every channel, same x) of the previous capture's sample of the same
    card keeps the previous value (`backdrop::settle`), so a game world moving behind a
    translucent chat does not republish the scene and re-render the overlay at every capture.
- **Painting**, per card: the tag rectangle (amendment A2: only tags whose 8 blocks passed the
  uniformity check exist), the visible plate pixels (the occlusion mask), the plate's blended edge
  pixels just outside the mask (the row above or below it only where the tag has a partial row
  there, as measured on all eight blocks by `cards`; the column after its end only when its pixels
  hold one blend fraction down the band and the next column does not; each pixel checked in the
  capture to be a blend of the observed plate colour and the chat colour there, never a tooltip
  edge or a glyph) and the gaps of 4 px or less between rows (where both
  neighbouring masks are set; the mean of the two rows' gradients) are filled with the gradient,
  opaque (see Deviations). No-plate mode fills the text rectangle with the gradient at 0.92 x
  alpha. Without any sample (the overlay probe) the tags are covered with the observed plate
  colour as before.
- **Text**, like WoW chat: the chat font size, a 1 px black shadow offset (1, 1) at full opacity,
  greyscale antialiasing (rasterized at 3x and box-filtered). The text and its shadow are composed
  as one layer and faded by the card alpha: alpha = layer coverage x mask x card alpha, so text
  lands only on plate pixels visible in this capture.
- Fonts: "Arial Narrow" when installed (WoW's default chat font), else "Arial" at 0.82 x its
  average width; zh "Microsoft YaHei UI", zh-TW "Microsoft JhengHei UI", ko "Malgun Gothic". No
  italics anywhere. Size = VIEW.fontPx x fontScale (the band height when no VIEW is known),
  capped so that the font's cell (ascent + descent, `tmHeight`, about 1.15 x the character height
  for Arial Narrow) is at most the band height + 1 px, in thirds of a pixel: the text is drawn
  vertically centred in a box one band tall (deviation 31).
- Fit: wrap into K lines (K = TRANSLATE rows, 1..6) at the card's text width at the full size;
  else the largest size down to 80% of it at which the text fits (thirds of a pixel, binary
  search); else cut the last line at 80% and end it with "..." (three dots). Line i goes to the
  band of row i (rows numbered 0..5 by `cards::build_with_rows`). A status (one line) goes to the
  first visible band.
- Colours: text `textColor` (default #A8D8FF, a soft light blue that reads as a translation; a
  settings file still holding the old default #9FD4FF gets the new one), statuses in a muted grey
  #A0A0A0, failures (failed, low memory, files not found) #E08080.
- Display affinity: `WDA_NONE` under WGC unless `hideFromRecordings`; `WDA_EXCLUDEFROMCAPTURE`
  under GDI.
- `--diag` draws 1 px outlines around tags (yellow) and text rectangles (cyan) and logs at debug
  level. Without `--diag` no outline is drawn (render tests check both).

## `--diag-json` (JSON Lines, `t` = Unix epoch ms, never any text)

Design D.6 records, with extra fields and events:

```
{"t":..,"ev":"frame","ok":true,"mode":"dense","margin":42,"session":1234,"seq":7,"chunks":2}
{"t":..,"ev":"frame","ok":false,"why":"torn|rejected"}
{"t":..,"ev":"msg","kind":"translate","req":5,"slot":3,"rows":2,"bytes":48,"pairs":1,"noCache":false,"plate":true}
{"t":..,"ev":"msg","kind":"cancel","req":5,"slot":3} / {"kind":"clear"} / {"kind":"test","nonce":9}
{"t":..,"ev":"msg","kind":"view","n":1,"views":[[id,x,y,w,h,fontPx,rowPx]]}
{"t":..,"ev":"tags","view":1,"hits":[{"slot":3,"row":0,"x":20,"y":1200,"w":46,"h":14,"alpha":1.0,"block":6.0,"plate":true}]}
{"t":..,"ev":"cards","hidden":null|"no_wow"|"minimized"|"stale","cards":[{"slot":3,"req":5,"view":1,
   "kind":"request|test|missing|pending","rows":[0,1],"bands":[[x,y,w,h],..],"tags":[[x,y,w,h],..],
   "gaps":[[x,y,w,h]],"maskPx":1234,"masks":[{"rect":[x,y,w,h],"runs":[[y,x0,x1],..]}|null,..],
   "alpha":1.0,"plate":true,"textWidth":297,"state":"sent|queued|loading|translating|partial|done|same|nothing|failed|lowmem|notinstalled|test|missing|pending","covered":false}]}
{"t":..,"ev":"overlay","view":1,"shown":true,"rect":[x,y,w,h],"window":false,"px":13240,"textPx":1812,"tagPx":1536,"fillPx":0}
{"t":..,"ev":"engine","req":5,"session":1234,"status":"done","ms":200}
{"t":..,"ev":"capture","fresh":true,"method":"wgc","fg":true,"strip":true,"scan":true}
{"t":..,"ev":"wow","found":true,"w":1920,"h":1080,"x":0,"y":0} / {"ev":"wow","found":false}
{"t":..,"ev":"session","old":1,"new":2}   {"t":..,"ev":"state","state":"ready_cpu"}
{"t":..,"ev":"tone","linear":false,"exp":[0.453,0.453,0.453]}
{"t":..,"ev":"toast","kind":"bridge_ok","shown":false,"info":{"mode":"dense","margin":42,"capture":"wgc","hz":30}}
{"t":..,"ev":"balloon","kind":"weak_signal","shown":true}   {"t":..,"ev":"probe","result":{..}}
{"t":..,"ev":"perf","state":"nowow|minimized|idle|strip|cards","wallMs":1000.4,"cpuMs":15.63,"cycles":30515962,"captureCpuMs":15.63,
   "uiCpuMs":0.0,"renders":0,"renderMs":0.0,"ticks":30,"fresh":30,"stripOk":0,"scans":30,"grabPx":295680,
   "areaRows":13920,"scannedRows":0,"bands":90,"reusedBands":90,"cardsKept":30,
   "stages":{"window":[0.2,30],"acquire":[0.6,30],"copy":[0.3,30],"wait":[25.1,30],"read":[0.1,30],"strip":[0.03,30],
             "scan":[3.3,30],"cards":[1.0,30],"scene":[0.2,30],"publish":[0.4,30],"diag":[1.1,90]}}
```

All rectangles are client coordinates (physical px from the client's top-left). `cards` is
written when the scene changes or a card's status changes; `overlay` when a view is shown, hidden,
moved or repainted with a different painted or text pixel count (the native look paints the plate
before any text, so a status appearing leaves the painted count unchanged; text pixels are
counted before the alpha, so a pulsing status does not repeat the record); `capture` once per
capture tick.

`perf` is written once a second: process CPU (`cpuMs`), capture-thread and UI-thread CPU
(`GetProcessTimes` / `GetThreadTimes`: the clock interrupt samples them, so per-second values are
multiples of 15.6 ms, and a thread that runs in short periodic bursts can be under-counted for a
whole run; measured: two identical 26 s idle runs reported 0.12% and 3.24%), the process cycle
count (`cycles`, `QueryProcessCycleTime`, exact; divide by the cycle-counter rate, about the
nominal CPU clock, to get seconds), render count and time, capture counters, tag search counters, and the wall
time and call count of each capture stage (`wait` is a sleep until the GPU copy is done, not CPU).
`state` is `strip` when a strip frame was decoded in that second, else `cards` when a card was
visible, else `idle`; `nowow` / `minimized` when there was no usable window.

## Settings, logs, privacy

- `%APPDATA%\TranslateProbe\settings.json` (design D.6 fields). Invalid values fall back to their
  defaults (each fix is logged by field name); an unreadable file is left untouched and the
  defaults are used.
- `%LOCALAPPDATA%\TranslateProbe\logs\helper.log` (or `--log-dir`): lifecycle, capture method,
  per-minute capture/decode/tag counters, message kinds with reqId/slot/byte counts, engine events.
- The logger only accepts `AppEvent` and `tp_engine::EngineEvent` (numbers, enums, fixed names).
  Chat text travels in `ChatText` (no Debug, no Display). A panic logs its line number only.
- Translations stay in memory (last 512 jobs) and are never written anywhere.

## Tests

| Command | What |
|---|---|
| `cargo test --offline` (helper\app) | unit tests: CLI (incl. the hidden `--instance-name`), settings round trip, validation and the old default colour, i18n completeness, fit (shrink to the largest size down to 80%, then "...", 6 rows), render (canvas and DIB: mask, alpha, full-opacity shadow, gap fill, tag cover, no-plate, background gradient over tag, plate, edges and gaps, no outline without `--diag`, dirty-rectangle reuse), background sampling (gradient, text-like pixels, borrowed sides, blended edges only at partial tag rows and at a consistent end column, no right sample for a card cut on the right, discovery spans, a borderless panel next to a plate, sample hysteresis), the 80% shrink with real fonts, the font cell cap (a French line with accents and descenders loses at most 1.5% of its ink at bands of 12-22 px), scene state machine (missing after 1.5 s, covered, stale 500 ms, minimized, session change, messages, weak signal, row counts, background in the fingerprint), diag records, candidate rows (SSE2 against scalar), GDI double buffers, WM_CLOSE on a hidden layered window quits the UI loop; `tests\sentinel.rs`; `tests\tagscan_equiv.rs` (the tag search finds exactly what `tags::scan` finds on 400 synthetic panels, and kept cards equal a new `cards::build`); `tests\native_look.rs` (plates sized as the addon sizes them; alpha 1.0 over a 70-level gradient, alpha 1.0 and 0.6 over a 35-level one: every pixel of the reserved rows within 6 levels of the ground, no plate colour; a tooltip over a plate is never painted, its outer rows included; a frame over the right end of every row of a 1- or 2-row card, parchment or tooltip coloured, leaves the visible plate within 6 levels of the chat; discovery with the game world on both sides of the chat: within 6 levels; a borderless panel in WoW's tooltip colour right under a plate is never painted; an ignored timing test); `tests\graceful_exit.rs` (a spawned Helper, own mutex, echo, no tray, no game window, hidden windows only: `taskkill` without /F through the hidden toast window and, with `--no-overlay`, through the main window; exit code 0 within 8 s and the log's Exit record) |
| `cargo test --offline --release --lib -- --ignored --nocapture render_cost candidate_cost` | render and candidate-row timings |
| `cargo test --offline --release --test native_look -- --ignored --nocapture` | background sampling cost of a 6-row card |
| `cargo test --offline --test window_pipeline -- --ignored` | real exe against a 400 x 260 window at the bottom of the z-order: strip, VIEW, TRANSLATE, echo translation, card, in-memory rendering; and a late start (discovery, "missing") |
| `target\release\capprobe.exe` | WGC and GDI read a known pattern exactly (popup and captioned windows), plus WGC on an off-screen window and on a window at the bottom of the z-order (informational) |
| `TranslateProbeHelper.exe --overlay-probe --diag --exit-after 5 --no-tray` | click-through and focus check |
| `TranslateProbeHelper.exe --selftest` | discovery JSON |

## Measured facts

- capprobe (Windows 11, RTX 3080, WoW running): WGC and GDI read every pixel of the pattern
  exactly on a popup window and on a captioned window (client offset in the WGC frame (1, 31)).
- WGC on a window placed beyond the right edge of the virtual screen: the first frame did not show
  the window's content and no frame arrived after a repaint. An off-screen E2E window therefore
  cannot be captured; a window at the bottom of the z-order (`HWND_BOTTOM`) is captured exactly.
- Windows 11 rounds window corners: 36 pixels at the bottom corners of a captioned window's client
  area were not the window's own. capprobe turns rounding off (`DWMWCP_DONOTROUND`).
- In 2 of the first 6 capprobe runs, another application's topmost window (class
  `FLUTTER_RUNNER_WIN32_WINDOW`) sat over the bottom-right corner, so GDI (which reads the screen)
  saw it instead of the probe window while WGC still read the probe exactly. capprobe now checks a
  3 x 3 grid with `WindowFromPoint` and moves to the next spot of the bottom-right area when its
  window is covered (`spot`, `coveredBy` in its output).
- GDI `ANTIALIASED_QUALITY` drew the Latin face at 16 px with no intermediate coverage value.
- **`taskkill` without /F** (2026-10-18; a test process with a hidden main window and two hidden
  topmost layered windows, taskkill run twice on it): each run posted `WM_CLOSE` to one window
  only, the first top-level window of the process in z-order (the last-created topmost one); the
  main window and the other topmost window never received it. The Helper's main window was never
  message-only: its first window in z-order is an overlay or the toast, whose `DefWindowProc`
  destroyed just that window, so the Helper kept running. Now every Helper window exits on
  `WM_CLOSE` (`tests\graceful_exit.rs`).
- **CPU (P4b, RTX 3080, WoW running, E2E rig with fakewow off-screen, echo, `--no-overlay`):**
  - `D3D11CreateDevice` took 0.25-0.29 s (0.22-0.27 s of CPU: the driver starts); the WGC open
    took 0.08-0.33 s in the E2E runs, and the first second of a run costs about 0.3 s of CPU.
  - A blocking `Map` of the staging copy spin-waits until the GPU has done the copy, and with the
    driver's threading optimizations an NVIDIA driver thread (`nvwgf2umx.dll`) used 7.2% of a core
    at 30 Hz. With `PREVENT_INTERNAL_THREADING_OPTIMIZATIONS` that work moved into the capture
    thread; with the fence wait it is gone (the wait is about 4% of the thread's cycles).
  - With WGC delivering up to 60 frames a second (interval 500/30 ms) and blocking maps, the
    threads other than the capture and UI threads (WGC, NVIDIA driver) used 5.5% of a core at idle
    and 8% with cards; with the frame cap at the tick rate, the flag and the fence wait, under 0.5%.
  - `tags::scan` over a 645 x 464 chat area: 1.5-2.3 ms per capture; the candidate-row pass:
    0.09 ms (SSE2; the scalar version 1.0 ms).
  - A render of 3 finished cards in a 645 x 433 view went from 2.1 ms to 0.48 ms (`f32::round`
    is a C runtime call on the baseline target; canvas reuse; integer compositing). With the
    native look (tags and plates painted with the background gradient) the same render takes
    0.57-0.63 ms (2026-10-18 and 10-19, release, `render_cost`).
  - Background sampling of a 6-row card in a 645 px view (release, `sample_card_cost`, plates at
    fractional rows so every edge row is scanned): 451 us before the review fixes (reviewer's
    measure), 343 us after (2026-10-19; `cards::build_with_rows` on the same card: 372 us). It runs
    only on captures whose scan area changed, and a drifting ground no longer republishes the scene
    (settled samples).
  - WGC staging textures are cached memory: reading them in place is as fast as a copy.
  - Before and after P4b, same bridge, 2 interleaved runs per case, % of one core from the process
    cycle count: the 9 echo E2E scenarios went from 7.7-10.0% to 2.2-3.2% for a whole run; after
    the first 2 s, captures with cards went from 9.0% to 1.35%, the strip active from 8.3% to
    1.4%, idle with the game from 4.9% to 0.4%; no game window 0.03% in both. The first 2 s of a
    run cost about 0.3 s of CPU (the D3D11 device), which is 1-2% of a 15-35 s E2E run.

## Deviations from the design

1. **Library target.** `src\lib.rs` (crate `tp_helper`) holds the modules, so `capprobe` and the
   integration tests reuse them; `main.rs` only parses the arguments and calls `app::run`.
2. **Extra `windows` features**: `Win32_System_Console` and `Win32_Storage_FileSystem` (a GUI exe
   printing `--selftest`, `--help` and probe results to the console it was started from),
   `Win32_Globalization` (Windows UI language for the tray and for toasts before any request).
3. **Text rasterized at 3x and box-filtered.** Fact: GDI `ANTIALIASED_QUALITY` follows the font's
   gasp table and gave black-and-white text for the Latin face at 16 px, in the chat font range.
   Measurement uses the same 3x metrics.
4. **Discovery grabs the full client width** below row 16 (not the left 35%). Fact: `cards::build`
   measures the plate up to the view's right edge, so a 35%-wide image would cut every card at 35%
   of the client width; `tags::scan` itself searches only the left 35% of its area (design D.2
   step 1). Without VIEW, the synthetic view's `fontPx`/`rowPx` come from the median tag height
   (`rowPx` = 1.15 x height) so that `cards::build` can group rows.
5. **`--no-overlay` implies `--assume-foreground` and skips the covering test.** Facts: amendment
   A1 forbids the E2E window from taking focus, and capprobe shows WGC cannot capture an off-screen
   window, so the E2E window must sit at the bottom of the z-order, where every `WindowFromPoint`
   sample hits another window. With no overlay on screen there is nothing to protect.
6. **Points on no monitor count as uncovered** in the covering test (nothing can cover them, and
   nothing drawn there is visible).
7. **Tray menu: the documented notification-icon pattern** (review fix; it replaces the first
   version, which showed the menu without `SetForegroundWindow` to honour amendment A1). Fact
   (`TrackPopupMenu` documentation): for a notification icon the window must be in front first,
   or the menu does not close when the user clicks elsewhere and Esc goes to another thread. The
   Helper calls `SetForegroundWindow` on its hidden main window only in answer to the user's own
   right-click on the icon (the shell grants that right for the click, and the taskbar already
   holds the foreground), never on a timer or in a test, then posts `WM_NULL` after the menu.
8. **The "translator failed" balloon** follows `EngineState::Error` (rate-limited to once per 10
   minutes), not each `ServerStartFailed` event, so a GPU start failure followed by a working CPU
   fallback shows no balloon.
9. **Status lines** go to the first visible band (translations keep "line i in band i").
10. **Toast and Missing/Test language**: the target of the latest request this session, else the
    Windows UI language.
11. **WGC frame cap**: `SetMinUpdateInterval`, set by P4b to follow the capture rate (see 16).
12. **Extra i18n keys**: `status.overlay_ok`, `balloon.*`, `tray.*`, `lang.*` (en and fr).
13. **Extra diag fields and events** as listed above (all numeric or fixed names).
14. **P4b capture rates.** In front with nothing pending (no strip for 2 s, no card) the capture
    runs at 10 Hz instead of 20 Hz, and 20 Hz while the strip is active. Fact: the whole idle cost
    is per tick, and a strip chunk stays on screen 400 ms, so 10 Hz still sees each chunk about 4
    times; measured TRANSLATE latency P99 61-128 ms in the 9 E2E scenarios (limit 300 ms).
15. **P4b scan gating.** The design scans every tick whenever a VIEW is known. Here the scan runs
    every tick only while a request of this session is known, the last scan found tags, a card is
    shown or the strip is active, else at 2 Hz (no card can then be on screen that needs a quick
    reaction). Discovery is time-based (2 Hz, the design's figure) instead of every 10th tick,
    because the tick rate now varies.
16. **P4b WGC frame cap** is 3/4 of the current capture period instead of
    `clamp(500/captureHzActive, 8, 50)` ms (the extra frames cost CPU and were never read).
17. **P4b tag search and card cache** (`tagscan.rs`) instead of `tags::scan` per area and
    `cards::build` on every capture. Fact: at 30 Hz the full scan alone cost 4.6-6.8% of a core.
    The candidate rows are a superset of where `tags::scan` can start a tag (its own sync relation
    at 3/4 of its threshold and one more pixel of reach), so the hits are identical
    (`tests\tagscan_equiv.rs`). Reusing the readings of a band whose tags kept the same inner pixels
    is an approximation: a full scan could measure such a tag a fraction of a pixel differently when
    the background next to it changes; any change of the tag's own pixels reads it again, and the
    plate and mask still come from the current capture. Cards are reused only when the whole scan
    area is pixel-identical, so they are exactly what `cards::build` would return.
18. **P4b D3D11 device flags and GPU wait**: `PREVENT_INTERNAL_THREADING_OPTIMIZATIONS` and an
    `ID3D11Fence` event wait before `Map` (facts above). Without `ID3D11Device5` (older systems) the
    `Map` waits as before.
19. **P4b diag buffering**: `--diag-json` lines reach the file about once a second and at exit,
    not line by line (a killed process loses at most the last second).
20. **Tray menu and the UI messages (review fix).** While the menu is open the UI thread runs the
    menu's modal loop, which dispatches the posted scene, result, state and event messages to the
    window procedure, where they were dropped: the cards froze on top of the game and a finished
    translation could stay on "Translating...". The Helper now hides the cards before the menu,
    and after it posts the four messages again (their handlers drain shared inboxes, so nothing is
    lost) and redraws.
21. **Session end (review fix).** `WM_ENDSESSION` with wParam FALSE (a shutdown or logoff cancelled
    by another application) no longer exits. With wParam TRUE, Windows may end the process as soon
    as the message returns, so the Helper stops and joins the capture thread and shuts the engine
    down (llama-server killed, key file deleted) and closes the log before returning.
22. **`scripts\helper.ps1 -Run` argument quoting (review fix).** Arguments are quoted with the
    command-line rules of the exe (backslashes doubled before a quote, embedded quotes escaped),
    so a runtime folder such as `C:\Program Files\rt\` no longer swallows the arguments after it
    (checked by round-tripping through a program that prints its argv).
23. **Display curve correction** (integration fix; `docs\BRIDGE.md` deviation 10). Fact: the E2E
    scenario gamma22 failed its band and completion checks (4 of 8 tags refused after the first
    scroll, no plate found). With the curve fitted from the strip and undone, gamma22 passes every
    check except, in some runs, the whole-run CPU figure.
24. **Sticky text width wired** (`cards::StickyWidth`, design step 7). The P4 app built cards
    without it.
25. **Native look: opaque background fill.** The task asked for the tag and plate to be filled
    with the background gradient "at the card alpha". Fact: a faded chat line at alpha a shows the
    plate as `a P + (1 - a) G` over the ground G; a fill of G at opacity a leaves
    `a (1 - a) (P - G)` of the plate visible (a quarter of its contrast at a = 0.5). The samples
    come from the same capture and already show the line's fade, so the fill is opaque; the text
    is still faded by the card alpha. `tests\native_look.rs` checks alpha 0.6.
26. **Native look: the right sample** is the last 40 px of the view rather than every pixel from
    the plate's end to the view's right edge. Fact: a faded plate over a busy ground can be
    measured shorter than it is (`docs\BRIDGE.md` section 11), and the pixels just after the
    measured end would then be plate; the far end of the view surely shows the chat.
27. **Known limit of the native look.** The gradient is linear between two samples per row: the
    texture of a game world seen through a translucent chat is not reproduced under the
    translation, only its average colour on each side. A plate faded to alpha 0.6 over a ground
    whose colour changes by about 70 levels across the row is masked only up to where its colour
    leaves the mask's tolerance; the rest of the plate stays visible until the line fades out (the
    bridge's mask follows the plate colour within its tolerance only). At alpha 1 (a line that has
    not started fading) the plate is opaque and fully hidden whatever the ground.
28. **Rows 1..6.** TRANSLATE rows go up to 6; a tag still carries its row modulo 4, and
    `cards::build_with_rows` numbers the rows of a card on from the first visible one with each
    slot's TRANSLATE row count (`docs\BRIDGE.md` section 8). The scene passes the counts
    (`SceneMachine::rows_expected`), the card cache keys on them, and the diag `tags` record and
    the block size lookup compare rows modulo 4.
29. **Graceful exit through any window.** A `taskkill`-friendly main window alone was not enough
    (fact above): the overlay and toast windows forward `WM_CLOSE` to the same exit, and the toast
    window is created hidden at start so the process always has a topmost window of its own (with
    `--no-overlay` the main window is the only one and handles it). The capture thread is joined
    with a 3 s limit so a stuck capture cannot hold the exit.
30. **Native look: no right sample for a card cut on the right (review fix, 2026-10-19).** Fact
    (review scratch test, real scan and paint): a frame over x 300..480 of every row of a 1- or
    2-row card made the card's own plate end the cut, so each row "reached the end" and the right
    sample came from under the frame; the visible plate was painted up to 117 levels off the chat
    (parchment) or 18 (WoW's tooltip colour). The plate's expected end is known (the addon draws
    it 0.88 x the room right of the tag; 1 px short in the in-game diag of 2026-10-18), so a card
    more than 24 px short gets a flat fill from its left sample. In discovery (no VIEW; after any
    Helper restart while WoW runs, until the next click) the view is the whole client: the right
    sample came from the game world at the client's right edge (71 levels off in the review test);
    there is now none, and the left span stays within one em of the tag. The addon also sends a
    VIEW before every TRANSLATE now (`docs\ADDON.md` decision 23), so discovery ends at the first
    click.
31. **Font cell cap (review fix).** The font was capped at the band height (`FONT_CAP` 1.0), and
    the text box is one band tall with vertical centring: accented capitals and descenders lost
    2.1-2.6% of a French line's ink (review measure). The cap is now on the cell: `tmHeight` at
    most band + 1 px (`FONT_CELL_SLACK`), which loses 0.3-1.4% (test above); the text is about 8%
    smaller than the chat font at a 22 px band.
32. **Edge pixels only where the edge is fractional (review fix).** Fact: WoW's default tooltip
    background (23, 23, 48) next to a plate over a dark chat (12, 12, 14) passes the blend test,
    so a borderless panel starting on the row right under a plate (whole-pixel edges) had its top
    row painted with the chat colour (269 of 280 px). A row above or below a plate is now looked at
    only when the tag has a partial row there, and the end column only when it holds one blend
    fraction down the band and the column after it does not (a panel fills both alike).
33. **Background hysteresis and cheaper sampling (review fix).** Fact: with the world behind the
    chat drifting by 1 level per capture, the quantized background in the scene fingerprint
    changed on 15 of 29 captures, so the scene was republished and the overlay re-rendered about
    every other capture. Samples now settle (4 levels, `backdrop::settle`), the side spans read
    every second column, and edge rows are scanned only when fractional (cost above).
34. **Known limit: the end of a fade.** Cards fainter than alpha 0.3 are dropped (`cards`
    `ALPHA_MIN`, scene `MIN_ALPHA`), and the plate is hidden only while its card is painted: for
    the rest of the fade the plate and the tags show at 30% contrast or less, for a plate
    (32, 40, 64) over (15, 15, 15) about (20, 23, 30), a faint slab where the reader saw only the
    translation before. Before the native look the plate was visible during the whole fade.
