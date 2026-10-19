# End-to-end rig (no WoW needed)

The rig runs the real addon on the mock client, replays what it draws in a stand-in game window,
and checks what Translate Companion (`WoWTranslateCompanion.exe`, the Helper; TranslateProbe Helper
until 2026-10-19) finds and draws. Nothing is downloaded, and every chat line is synthetic.

```
scenarios\<name>.lua --make_scenario.js--> out\<name>.json --fakewow--> window + out\*.truth.jsonl
                                                                          |
                                    WoWTranslateCompanion --diag-json --> out\*.diag.jsonl
                                                                          |
                                                         check.py --> out\*.report.json
```

| File | Role |
|---|---|
| `scenario_lib.lua` | the `E2E` API scenarios use; records every mock frame (1/60 s) that changed: strip blocks, chat frame rectangles, fonts, scroll offsets, the visible end of each chat history, and the messages handed to `Bridge.Send` |
| `scenarios\*.lua` | the twelve scenarios below |
| `make_scenario.js` | runs `tests\mock.lua`, the addon files (TOC order, through `tests\run.js`), `scenario_lib.lua` and a scenario; writes the recording (format in `helper\testkit\src\scenario.rs`). The addon loads `CompanionStatus.lua` as a running Translate Companion writes it (`scenario_lib.lua`, `E2E.Start`), so its session starts confirmed and a [T] click reserves rows as before (since addon build 6 a click without a confirmed Translate Companion only opens the popup) |
| `helper\testkit` (crate `tp_testkit`, bin `fakewow`) | plays a recording in a window of class `TPFakeWow` and writes the truth |
| `check.py` | compares a truth file with the Helper's `--diag-json`; also makes synthetic diags for its self-test |
| `run.py` | runs whole scenarios: recording, fakewow, the Helper, check.py, process clean-up |

## Commands (from the repo root)

```
cd helper\testkit && cargo build --offline --release && cargo test --offline && cd ..\..
node tests\e2e\make_scenario.js tests\e2e\scenarios\basic.lua tests\e2e\out\basic.json
helper\testkit\target\release\fakewow.exe --scenario tests\e2e\out\basic.json --truth tests\e2e\out\basic.truth.jsonl --selfcheck --wgc-check
python -I tests\e2e\check.py --selftest --truth tests\e2e\out\basic.truth.jsonl
python -I tests\e2e\run.py --all --capture wgc
python -I tests\e2e\run.py --all --capture gdi
python -I tests\e2e\run.py --scenario basic --translator llama --helper-arg=--backend=cpu --done-within-s 60
```

`tests\e2e\out\` is scratch output and is not committed.

`run.py` starts the Helper as
`WoWTranslateCompanion.exe --wow-class TPFakeWow --translator echo --capture wgc|gdi --no-overlay --no-clipboard --no-tray --diag-json F --log-dir D --exit-after S`
(`--no-clipboard`: the Helper never touches the clipboard; `--no-overlay` implies it, it is given
anyway). A test run never writes the status file into the user's WoW folders (since 0.6.0:
`--no-overlay` and `--wow-class` turn it off unless `--status-root` names a folder). `--helper EXE` and `--fakewow EXE` take builds from another target folder (for example a
`CARGO_TARGET_DIR` of one's own while the user's Helper locks `helper\app\target`). With a Helper of
the user's running (either exe name: they share the mutex), add `--helper-arg=--instance-name=e2e`
(the test Helper then takes its own single-instance mutex).
It gives fakewow and the Helper no console window, measures the Helper's CPU time from its process
handle, and kills anything still running at the end. A final `tasklist` check reports any process
it started that is still alive, by process id (since 2026-10-19: it looked for the image names, so
a Helper the user was running counted as a leftover). Exit codes: 0 all passed, 1 a check failed,
2 a run could not start, 3 everything deferred.

## Not disturbing the game (amendment A1)

- fakewow never activates: it uses WS_EX_NOACTIVATE and WS_EX_TOOLWINDOW, is shown with
  SWP_NOACTIVATE, answers WM_MOUSEACTIVATE with MA_NOACTIVATE, and never calls
  SetForegroundWindow. Its summary reports `ownForegroundTicks`, which must be 0.
- `--place offscreen` is the default: the window sits 100 px right of the virtual screen, so no
  monitor shows it. The summary's `exposedPoints` counts the points of a 5 x 5 grid where fakewow
  was the visible window; it was 0 in every run.
- `--place bottom` puts it at (0,0) at the bottom of the z-order instead.
- `--place onscreen` is for the Helper's GDI mode, which reads the screen. fakewow refuses it
  (exit 3) while a WoW client runs, and `run.py --capture gdi` then reports every scenario as
  deferred instead of running it.
- The Helper always runs with `--no-overlay`, so it never shows a window.

## fakewow

- **What it draws:** a 1920 x 1080 client. The game world is a static, noisy sky and ground with
  one moving patch. The patch matters because a capture API only delivers a frame when something
  changed, and a real game redraws every frame.
- **Strip:** the addon's strip blocks (2 x 2 px at the client's top-left).
- **Chat frames:** each one is a 30% black panel, with the history laid out bottom-up the way the
  ScrollingMessageFrame does it:
  - rows the height of the font, word wrap with an indent on continuation rows, `|n` rows at the
    left edge;
  - `|c`, `|r`, `|H` and `|h` handled;
  - `|T` inline textures at `units * ppu` px, at sub-pixel x positions with coverage blending;
  - chat colours by event;
  - fading after `fade.visible` s over `fade.duration` s, only while not scrolled; a scroll
    resets the fade;
  - scroll offsets.
- **Covers and Alt-Z:** game tooltips over plates, and Alt-Z periods (UIParent hidden: no chat,
  no strip, no tooltip).
- **Layers:** addon windows over the chat frames and under the covers: opaque rectangles in client
  px; their visible tag rows join the truth's `plates`, and their meta data goes to a `layers`
  truth record. Layer 1 is the conversation window of the `conv` scenario (VIEW 200; meta: view
  rectangle, hidden rows, reply card, `bubble`); layer 2 the Translate Companion popup of the
  `presence` scenarios (2026-10-19, user request R1; meta `presence`: `{view: 201, slot: 125,
  card}`), drawn in the middle of the client.
- **Hint lanes (2026-10-19, gone since addon build 6):** fakewow still writes a `lanes` truth
  record for a gold row right after a message's last tagged row (`truth::lanes`), but the addon no
  longer writes such rows, the synthetic window draws none, and check.py reads no lane.
- **Display curve:** applied last, `out = 255 * clamp((in/255 - 0.5) * contrast + 0.5) ^ (1/gamma)`.
  `--gamma 2.2` brightens, like a higher WoW Gamma setting. Values under 1 darken.
- **Truth:** JSON Lines, `t` = Unix epoch ms when the frame was painted. The events are `header`,
  `ui`, `strip`, `covers`, `plates`, `sent`, `dropped`, `mark`, `selfcheck`, `end` and `gone`.
  - `plates` items are `{view, slot, row, sess, tag:[x,y,w,h], plate:[x,y,w,h], visible, alpha, covered:[[x,y,w,h]...]}`.
  - `lanes` items are `{view, slot, rect:[x,y,w,h], inside, revealed, visible, alpha}` (inline cards).
  - Rectangles are the parts visible inside the chat clip rectangle, in client px.
- **Self-checks** run about once a second while the picture has been stable for 150 ms:
  - `--selfcheck` reads the window back with GDI: BitBlt from the window DC, or PrintWindow
    for the layered off-screen window.
  - `--wgc-check` reads it back with Windows.Graphics.Capture.
  - Each read-back is compared pixel for pixel with what was drawn (minus the moving patch for
    WGC). Then the tags are decoded from the image at the truth rectangles, and the plate colour
    and the strip calibration blocks are checked.

## Scenarios

| Name | What happens |
|---|---|
| `basic` | 3 clicks (2, 2 and 5 reserved rows since the 1.35 row factor) |
| `scroll` | 3 cards, 8 new lines push them up and out, scroll up 4 and 12 messages, back down, then 26 new lines push every card out of the top |
| `fade` | fade shortened to 5 s + 3 s; cards fade and hide; a scroll resets the fade and they come back |
| `cover` | a tooltip over the right half of a plate, then one over the middle of two rows |
| `tabs` | a second chat window shows the same lines, then tab switches |
| `altz` | Alt-Z for 3 s, then for 0.8 s |
| `dropped` | the strip is not painted during one request; "Not received" must show; a re-click repairs it |
| `late_start` | 3 clicks, then the Helper starts at 7 s (no VIEW, no TRANSLATE); "Not received", re-click |
| `gamma22` | `basic` through Gamma 2.2 (the Helper measures the curve on the strip and undoes it before the tag scan) |
| `conv` | the conversation window (`E2E.Window`, a synthetic layer with the real tag colours; VIEW 200 first, then the chat frame it overlaps): 3 bubbles over the opaque fill (no hint lane since addon build 6), a reply card with flags 14, then a scroll that leaves the first bubble half out |
| `presence` | the Translate Companion popup (`E2E.Popup`, layer 2 in the middle of the client): its status card of slot 125 (a real tag and the plate over a `BUBBLE_RGB` fill, never a TRANSLATE) and the addon's grey caption under it; its VIEW 201 (the card's fill, then the chat frame) when it opens and every 2 s, as the addon sends it; after 4 s it closes (Continue: the chat frame's VIEW alone) and the held click runs (VIEW and TRANSLATE) |
| `presence_late` | the same popup 3 s before the Helper starts, its VIEW sent once: the Helper has no VIEW and must find the card by its discovery scan in the middle of the client, then paint it; Continue and the click as in `presence` |

## check.py

Each check reports pass or fail, and the overall exit code is 0 only when every check passes.
See the docstring for details.

- **latency:** every TRANSLATE painted while the Helper ran is decoded, at P99 <= 300 ms.
- **bands:** visible plates (alpha >= 0.45, a tag at least 8 px tall) have a card band within
  +-2 px of the truth within 2 captures:
  - band top and bottom = the tag's;
  - band left = plate left + 3;
  - band right = plate right, unless a cover cuts the plate.
- **hidden:** no band for a plate that is gone (alpha <= 0.2, scrolled out, Alt-Z, frame hidden).
- **masks:** no mask pixel inside a cover.
- **missing:** dropped and pre-start requests show `missing`, then become requests after a resend.
- **done:** decoded cards reach done, same or nothing within `--done-within-s`.
- **cpu:** the Helper used at most 3% of one core (`--helper-cpu-pct`, measured by `run.py` from
  the process times of the whole run). The report also gives `steady_pct_one_core`: the same
  figure without the first 2 s, split by the Helper's exact cycle counts (its `perf` records).
  It is informational: the first 2 s hold a one-time start-up cost of about 0.3 s of CPU (mostly
  the D3D11 device), which is 1-2% of a 15-30 s run, and process times are sampled by the clock
  interrupt, so two identical runs can differ by more than 1%.
- **fakewow:** fakewow never had the focus.
- **lanes:** gone with the hint lanes (addon build 6, Translate Companion 0.6.0, 2026-10-19).
- **presence** (scenarios whose truth has the popup layer, 2026-10-19, user request R1): every
  time the popup shows while the Helper runs, a card of kind `presence` on slot 125 appears within
  1.5 s of the later of the popup and the Helper's first capture (`detect_ms`; by discovery when
  the Helper has no VIEW), the overlay draws text in its view (201, or the discovery view 0), its
  background samples are within 6 levels of `BUBBLE_RGB` in VIEW 201 and by discovery alike (the
  card's fill left of the tag is wider than the discovery scan's left span since the integration
  review; the discovery ones are also listed as `discovery_samples`), slot 125 is never another
  kind while the popup shows, and the card is gone within 400 ms of the popup closing.
  The popup's plate is also a VIEW 201 plate for `bands`, `hidden` and `masks`.
- **conv only** (the truth has the conversation window's layer, VIEW 200): `window_fill` (every background sample of a window
  card within 6 levels of `BUBBLE_RGB`, every window card with at least one sample, since a card
  with none is painted with the navy plate colour; tags painted), `reply_status` (final, clipboard outcome
  "off", never copied), `partial_rows` (a half-out bubble's hidden rows never get a band, its visible rows do),
  `no_duplicate` (one card per slot; a window slot in a chat view only within the strip delay of the
  VIEW that covers it, reported as `strip_delay`), `cpu_vs_i2` (within 10% of the highest CPU
  figures of DESIGN.md I.2).

`--selftest --truth T` builds a perfect diag for a truth file and a copy shifted by 5 px: the
first must pass and the second must fail. For a truth with the conversation window (`conv`) the
perfect diag also carries what the window checks read (background samples in `BUBBLE_RGB`, the
reply bit and the outcome "off" on slot 126, `overlay` and `clip` records), and a third, broken
diag (window samples in the plate colour or missing) must fail `window_fill` (integration review
fix: the window checks had never been shown to fail). For a truth with the Translate Companion
popup the perfect diag has a `presence` card on slot 125 with an `overlay` record holding text,
and a diag that takes slot 125 for an unknown slot (pending, then missing) must fail `presence`.
`--synth-diag OUT [--shift-px N]` writes such a diag.

## Deviations from the design (amendment A9) and why

1. **Off-screen placement uses a layered window.** Measured: a normal window placed off every
   monitor gets no DWM updates. WGC delivered one frame and then nothing, and a read-back of its
   window DC was black.
   - As a layered window fed with UpdateLayeredWindowIndirect, both WGC and PrintWindow read it
     exactly. Results: 0 mismatching pixels in every check; every tag, plate and strip
     calibration block at the truth rectangles read correctly.
   - The GDI self-check of a layered window uses PrintWindow (PW_CLIENTONLY |
     PW_RENDERFULLCONTENT), because BitBlt from its DC cannot read it.
   - `--place bottom` (a normal window, BitBlt read-back) passes the same checks.
2. **`helper\testkit` windows features beyond the design list:**
   - Win32_Graphics_Dwm (square corners);
   - Win32_System_Diagnostics_ToolHelp (the WoW process check);
   - Win32_System_Threading (CPU time);
   - Win32_Storage_Xps (PrintWindow);
   - the WGC feature set of the exe-tech probe (`--wgc-check`, which A1 requires).
3. **`run.py` added.** It runs the 18 E2E runs with A1 and A5 enforced (GDI deferral, no
   console windows, kill and tasklist checks), which is error-prone by hand.
4. **check.py counts the captures that grabbed the scan area, and waits for the VIEW.** The
   design grabs the tag scan area only for known VIEW rectangles, or every 10th tick while no
   VIEW is known (D.3 step 2), and the addon's VIEW watcher runs at 2 Hz. So "within 2 captures"
   cannot hold for:
   - the first click of a session (VIEW and TRANSLATE arrive together; the scan area is grabbed
     from the next tick);
   - a newly shown chat frame (up to about 280 ms until the watcher's VIEW arrives);
   - a Helper started late (no VIEW; discovery every 10th tick).

   check.py therefore counts fresh captures with `scan: true` (from the Helper's `capture` diag
   event, written after the strip decode: a capture that skipped its scan because it decoded a new
   VIEW says `scan: false, viewSkip: true` and is not counted). It expects a plate only once the Helper's sticky VIEW covers its chat frame, or once
   the Helper scans by discovery because it has no VIEW. The wait is reported as `view_wait_ms`.
   `--strict` applies the literal rule.
5. **Truth carries more events than the design lists** (`header`, `ui`, `covers`, `sent`, `mark`,
   `selfcheck`, `end`, `gone`). `sent` is needed for the latency check, `covers` for the mask
   check, and `gone` marks the moment the window disappears.
6. **Scenarios use a 430 x 330 unit chat frame** (about 23 rows) instead of the mock's default
   400 x 180 (12 rows), so three cards fit on screen. `E2E.Start{chat = {...}}` changes it.
7. **Rows are compared modulo 4 (2026-10-19).** Requests hold 1 to 6 rows since 2026-10-18 and a
   tag carries its row number modulo 4. The truth reports the tag's row field (fakewow draws what
   the addon drew), while the Helper numbers a card's bands 0 to 5, so check.py matches a band to
   a plate on the slot, the row modulo 4 and the y position. With the 1.35 row factor the `basic`
   scenario's long line reserves 5 rows, and its fifth row (tag row 0) first failed `bands` and
   `hidden` in basic, cover, altz and gamma22.

8. **A window plate is expected only once the VIEW rectangle holds it (2026-10-19).** check.py
   expected a plate as soon as a VIEW with its id was known, but the window's VIEW 200 grows when
   its reply card shows (the card lies below the previous rectangle), and both come with the same
   strip: in one `conv` run the strip was decoded 72 ms after fakewow painted the card, so the
   capture in between, which still scanned with the old rectangle, failed `bands` (3 rows) and
   `lanes`. `Diag.window_view_since` now also requires the rectangle the Helper scanned with
   (decoded in an earlier capture) to hold the tag or lane; the same diag then passes.

9. **Lanes at the top of the chat (integration review, 2026-10-19; moot since addon build 6, which
   draws no lane).** check.py skipped every lane
   without a shown row right above it, and no scenario reached that state, so a hint left visible
   at the chat's top line went unseen. The `scroll` scenario now pushes every card out of the top,
   the truth lists a lane whose tagged row is above the chat, and check.py requires the orphan lane
   in the diag; a first run of the new Helper also showed a 4 px sliver painted for one more line
   where the truth had no lane any more (the line under the lane): the orphan search now requires
   the lane's remembered ink, and the truth lists cut lanes, so the false-lane check sees them.

10. **The Translate Companion popup is synthetic too (2026-10-19, user request R1).** The addon's
    popup (`CompanionDialog.lua`) is a frame of the mock, which fakewow does not draw (only the chat
    frames, the strip and layers): the rig draws the popup as layer 2 with the real tag colours of
    slot 125 and sends its VIEW 201 through the addon's own `Bridge.Send`, as `E2E.Window` does for
    the conversation window. The addon's own popup flow (Presence, Continue) is covered by the
    harness (`tests\harness*.lua`). The card's geometry follows the addon's: one row, the tag and
    the plate over an opaque `BUBBLE_RGB` fill with `pad` px above, under and right of them and the
    addon's wider fill left of the tag (`CompanionDialog.DISCOVERY_LEFT`: 0.75 of the row height and
    3 px), VIEW 201 the fill. It is drawn at integer px for one size; the real metrics at fractional
    px are checked by `helper\app\tests\presence_card.rs` (1080p) and in game (G21 step 5, two UI
    scales).

## What the rig found in the Helper build of 2026-10-16 21:22 (echo, WGC)

- **Results with that build:**
  - 8 of 9 scenarios pass latency, bands, hidden, masks, missing and done.
  - Latency P99 per scenario was 51 to 89 ms (worst single decode 89.5 ms).
  - Every band in those 8 scenarios is within 1 px of the truth.
  - No band was left visible for a plate that was gone.
  - No mask pixel was over a tooltip.
  - "Not received" showed for the dropped request and the 3 late requests, and both re-clicks
    were repaired.
  - In `tabs` the Helper waited 248 ms (frame 1) and 302 ms (frame 3) for the VIEW of a newly
    shown chat frame.
- **CPU:** the Helper used 10 to 14% of one core (3% allowed). The figure is the same without
  `--diag-json`, so it is not the diag output.
- **`fade` fails `bands`:** two causes, both in the plate measurement. The Helper's alpha
  estimate itself tracks the truth within 0.01.
  - A faded plate over green or brown ground stops passing the plate test ("B > G > R") at about
    alpha 0.5 to 0.55, so rows are dropped there instead of at alpha 0.3.
  - At alpha 0.5 to 0.6, the plate's mask leaks into the dark chat panel, because the mask
    tolerance grows as the plate fades. The text rectangle then runs 4 to 96 px past the plate's
    right end, so text would be drawn outside the reserved row.
- **Darkening display curve:** with `--fakewow-arg=--gamma=0.4545` (captured = 255 * x ^ 2.2,
  inside the bridge simulation's gamma envelope), the strip still decodes, but no plate is ever
  detected. The plate (32,40,64) becomes about (3,4,12), and B - R < 10 fails the plate test.
- **GDI mode** was not run (deferred while WoW was running).
