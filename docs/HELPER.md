# Translate Companion (helper\app)

**Translate Companion** (`WoWTranslateCompanion.exe`; TranslateProbe Helper, `TranslateProbeHelper.exe`,
until 2026-10-19) reads the TranslateProbe addon's pixel bridge from the WoW window, translates with
the local engine (`helper\engine`, llama.cpp + Qwen3 4B) and draws the translations over the chat
rows the addon reserved. It never opens a handle to the game process, never sends input to it and
never reads its memory: it only looks at the window's pixels. This document calls it the Helper,
as the code does.

**Version 0.6.0 (2026-10-19, user request R1).** Three changes: the **status file** (the Helper
writes `CompanionStatus.lua` into the deployed addon folders, so the addon knows at login and
`/reload` that it runs), the **presence card** (slot 125 of the addon's Translate Companion popup is
painted "Translate Companion detected" in green whenever the Helper sees it, also when it starts
while the popup is open) and **no hint lanes** any more (the addon of build 6 draws none; the lane
search and its cover are gone). The Helper speaks `STATUS.addonBuild` 6 and serves builds 2 to 6.

**Naming (user request U2, 2026-10-19).** Everything users see says Translate Companion: the exe
(Cargo `[[bin]]` `WoWTranslateCompanion`), the tray tooltip (with the version) and menu, the
balloons, the toasts, the status strings, `--help` and `--version`. Internal identifiers users
never see keep the old name, so nothing else has to move: the crate names (`translateprobe-helper`,
`tp_helper`, `tp_engine`, `tp_bridge`, `tp_testkit`), the single-instance mutex
`Local\TranslateProbeHelper` (an old `TranslateProbeHelper.exe` and the new exe therefore never run
together: the second one exits with code 2), the window classes `TPHelperMain`, `TPHelperOverlay`
and `TPHelperToast`, and the folders `%APPDATA%\TranslateProbe` (settings) and
`%LOCALAPPDATA%\TranslateProbe` (logs, run files), so settings and logs carry over. The hidden
windows' titles say "Translate Companion". Download page (user request U3): `tp_helper::RELEASES_URL`
= https://github.com/Mouchoir/WoW-Chat-Translator/releases (the addon has its own copy), shown by
`--help` and opened by the tray's "Get updates".

## Files

| Path | Role |
|---|---|
| `src\main.rs` | Entry point (GUI subsystem: no console window) |
| `src\lib.rs` | Library `tp_helper` holding every module, so `capprobe` and the tests reuse them |
| `src\cli.rs` | Command line |
| `src\settings.rs` | `%APPDATA%\TranslateProbe\settings.json` |
| `src\status_file.rs` | The status file (`CompanionStatus.lua` in every deployed addon folder): WoW install discovery (settings `wowRoot`, else the Battle.net agent's product.db), atomic writes, the writer thread |
| `src\log.rs` | `logs\helper.log` (1 MB x 3 files) and the `--diag-json` writer |
| `src\i18n.rs`, `i18n\en.json`, `i18n\fr.json` | User-visible strings |
| `src\wow.rs` | Game window lookup, geometry, covering test |
| `src\capture\{mod,wgc,gdi}.rs` | Windows.Graphics.Capture (primary) and GDI `BitBlt` (fallback) |
| `src\tagscan.rs` | Tag search of one capture around `tags::scan` (candidate rows, unchanged tags) and the card cache |
| `src\perf.rs` | Stage timers and process/thread CPU times for the `perf` diag record |
| `src\scene.rs` | Pure state machine: messages and cards in, scene out (replies, view kinds, the background rule) |
| `src\clip.rs` | Clipboard of translated replies: `ReplyClip` (begin, finish, release), `ClipOs` with the real `WinClip` and the tests' `FakeClip`, `one_line_255` |
| `src\reply.rs` | The UI thread's replies: one `ReplyBook` per reply request (clipboard state, the request's own text), memory only |
| `src\backdrop.rs` | The chat background around each card's rows, sampled in the current capture (native look) |
| `src\overlay\{mod,window,render,fit,fonts}.rs` | Layered windows, pixel composition, wrapping, fonts |
| `src\toast.rs`, `src\tray.rs` | Bridge toast, notification-area icon |
| `src\app.rs` | Process: UI thread, capture thread, engine wiring |
| `src\bin\capprobe.rs` | Capture self-check on its own window |
| `src\bin\clipprobe.rs` | The one live check of the real clipboard path (decision D9), run by hand, never by the tests |
| `tests\sentinel.rs` | Log sentinel tests (chat request, and the reply flow with a fake clipboard) |
| `tests\tagscan_equiv.rs` | The tag search and the card cache give exactly what `tags::scan` and `cards::build` give |
| `tests\window_pipeline.rs` | Window-level smoke tests of the whole pipeline (ignored by default) |
| `tests\native_look.rs` | Pixel test: a synthetic capture through tag scan, cards, background sampling and painting; the reserved rows disappear into the ground |
| `tests\graceful_exit.rs` | `taskkill` without /F stops a spawned Helper cleanly (exit code 0, log closed); with `--status-root`, the status file says "running", then "stopped" |
| `tests\presence_card.rs` | Pixel test of the presence card: a synthetic game capture with the addon's popup in the middle, by discovery and through VIEW 201: the card found, "Translate Companion detected" in green from the tag, the tag and the plate gone into the popup's background, nothing else painted |

Build, test and run with `scripts\helper.ps1` (`-Build`, `-Test`, `-Run`, `-Diag`, `-RuntimeDir`).
The translator runtime folder comes from `-RuntimeDir`, else `$LlamaRuntimeDir` in the gitignored
`scripts\dev.local.ps1`, else the `TPROBE_LLAMA_RUNTIME` environment variable.

## Command line

```
WoWTranslateCompanion.exe [--runtime DIR] [--model FILE] [--backend auto|cpu|vulkan|cuda] [--capture auto|wgc|gdi]
  [--translator llama|echo] [--wow-class CLASS] [--diag] [--diag-json FILE] [--log-dir DIR] [--no-tray]
  [--exit-after SECS] [--selftest]
test flags: [--no-overlay] [--assume-foreground] [--overlay-probe] [--no-clipboard]   other: [--help] [--version]
hidden test flags: [--instance-name NAME] [--assume-background] [--status-root DIR] [--no-status-file]
  (not in --help)
```

- `--selftest` prints the engine's discovery, flags, VRAM, RAM and backend decision as JSON and
  exits. It never loads the model (it may run `llama-server --help` once to read the flags, then
  caches them in `%LOCALAPPDATA%\TranslateProbe\run\flags.json`).
- `--no-overlay` (amendment A1): cards and toasts are rendered into in-memory DIBs and reported in
  `--diag-json`; no window is ever created. It implies `--assume-foreground` and skips the
  `WindowFromPoint` covering test (see Deviations).
- `--assume-foreground`: the game window counts as foreground (the E2E window never takes focus).
- `--no-clipboard`: the clipboard is never touched; a reply shows "Ready (not copied - off in the
  Translate Companion menu)", as with the tray switch off. `--no-overlay` implies it, so no automated run ever
  touches the user's clipboard; `tests\e2e\run.py` gives it anyway.
- `--overlay-probe`: no capture; one synthetic 2-row test card (376 x 52 px, bottom right of the
  primary monitor's work area, at most 5 s) checks that `WindowFromPoint` at the card centre never
  returns the overlay and that the foreground window does not change. Prints one JSON line, exit
  code 0 on success. Use `--overlay-probe --diag --exit-after 5 --no-tray`.
- `--instance-name NAME` (tests only, 1-32 ASCII letters, digits, '-' or '_'): the
  single-instance mutex becomes `Local\TranslateProbeHelper.NAME`, so a test Helper can run next
  to the one the user is running.
- `--assume-background` (tests only): the capture thread treats the game window as not in front
  whatever Windows says (background rate, background covering rule); it wins over
  `--assume-foreground` and the one `--no-overlay` implies, so an E2E run can check that the strip
  is read while the game is in the background (review fix, deviation 38).
- `--status-root DIR` (tests only): the status file goes under DIR only (a "World of Warcraft"
  folder or one flavour folder), never under the user's installs, and is written even by a test
  run. `--no-status-file`: never write it. A test run (`--instance-name`, `--no-overlay`,
  `--wow-class`, `--assume-foreground`, `--assume-background`, `--overlay-probe`) writes no status
  file without `--status-root`, so the E2E rig and the window tests never touch the user's WoW
  folders (`Cli::status_file_on`).
- `--help` starts with the app's name and the download page; `--version` prints
  "Translate Companion 0.6.0" (0.4.0 at the rename, 0.5.0 since the link colours and the verbatim
  tokens, 0.6.0 since the status file and the presence card, all 2026-10-19).
- Exit codes: 0 normal, 1 failed start or failed probe, 2 another instance is running
  (`Local\TranslateProbeHelper`, also taken by an old `TranslateProbeHelper.exe`), 64 bad arguments.
- Stopping: the tray's Exit, `--exit-after`, or `taskkill /IM WoWTranslateCompanion.exe` without
  /F. All three end the message loop, then write "stopped" into the status files, stop the capture
  thread (waiting at most 3 s), shut the engine down (llama-server killed, key file deleted) and
  close the log (exit code 0); a session end does the same from `WM_ENDSESSION`. `taskkill /F`
  still works but skips that (the job object still kills llama-server, and the status file keeps
  "running" with a heartbeat that goes stale after 150 s for the addon).

## Threads

- **UI thread**: hidden window `TPHelperMain` (a plain top-level window, never shown, also the
  clipboard owner of replies), tray icon (tooltip "Translate Companion 0.6.0 - <translator state>";
  menu: Unload translator now, Get updates (opens `RELEASES_URL` in the default browser with
  `ShellExecuteW`, on the user's own click only; logged as `TrayUpdates`), Open log folder, the two
  reply switches, Exit), overlay windows (`TPHelperOverlay`, one per VIEW id) and
  the toast (`TPHelperToast`, created hidden at start unless `--no-overlay`). With overlay windows
  it sets two out-of-context WinEvent hooks (foreground change, minimize start/end) so that the
  overlay's z-order follows the game window at once (decision D12, below). `WM_CLOSE` on any of these windows exits like the tray's
  Exit (see "Measured facts" for why every window needs it). It handles `WM_APP_SCENE`,
  `WM_APP_RESULT`, `WM_APP_STATE` and `WM_APP_EVENT`, renders only on change (once for a burst of
  scene and status messages), and runs a 10 Hz timer only while a status is animated (queued,
  loading, translating). Each view keeps its canvas; only the dirty rectangle is cleared, copied
  and handed to `UpdateLayeredWindowIndirect`; wrapped lines and rasterized text are cached.
- **Capture thread** (MTA): finds the window (`FindWindowEx` on the class, every 2 s while absent),
  grabs the strip (client rows 0..8, full width, in front and in the background) and the scan
  regions (one per VIEW rectangle, two merged only when their union is at most 1.3 times their
  total area, window views first; or the discovery area at 2 Hz), decodes, reassembles, feeds the
  engine, scans tags (one tag search and one card cache per region; a tag read in two overlapping
  views is kept once, in the first), builds cards, runs the covering test (at most 10 Hz per card)
  and publishes the scene when it changed. The Helper speaks `STATUS.addonBuild` 6
  (`scene::ADDON_BUILD`; 6: the Translate Companion popup with its presence card on slot 125, and no
  hint lanes, 2026-10-19, user request R1; 5: the link colours, a COLOURS message right before the
  TRANSLATE, 2026-10-19, user request L3; 4: a hint lane under every card, 2026-10-19; 3: TRANSLATE
  reply and copy bits, the conversation window, 2026-10-19; 2: rows 1..6, 2026-10-18) and serves
  only build 6 without a toast (`scene::ADDON_BUILD_MIN` 6 since the integration review; it was 2):
  a strip frame from another build shows the "addon is older" or "app is older" toast once per
  session. Builds 3 to 5 draw hint lanes (3 under the reply card, 4 and 5 under every card) that
  this Helper no longer paints over, so their gold hints would stay under every translation with
  nothing saying why: the toast tells the player to update the addon. A Helper of build 5 takes the
  presence card for an unknown bubble (its tag and plate painted over at once, then "Not received -
  click the bubble" after 1.5 s) and shows "app is older" for an addon of build 6. A Helper of build 4 drops COLOURS as an unknown message (`reasm` counts a parse error),
  paints the link names in the translation's colour as before, and shows "app is older" for an
  addon of build 5. The colours never reach it as text: the TRANSLATE carries no 0x04
  (`docs\BRIDGE.md` section 3b; a first design with the colour in the text made a build-4 Helper
  paint "0070DD" before each name, integration review).

  | State | Rate |
  |---|---|
  | No game window | look-up every 2 s, nothing else (the thread sleeps until the next look-up) |
  | Minimized | no capture, 2 Hz check |
  | In front, nothing pending (no strip seen for 2 s, no card visible) | 10 Hz |
  | In front, strip seen in the last 2 s | 20 Hz |
  | In front, card visible | `captureHzActive` (20-60, default 30) |
  | Not in front, strip not seen for 2 s | 5 Hz, strip and tags; the cards stay (decision D12) |
  | Not in front, strip seen in the last 2 s | 20 Hz |

  The tag scan runs on every capture only while a request of this session is known, the last
  scan found tags, a card is shown or the strip is active; otherwise (and for discovery) at 2 Hz,
  and discovery at 10 Hz while its last scan found the presence card (`app::PRESENCE_SCAN_MS`).
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
- **Status thread** (`tp-status`, `status_file::StatusWriter`): writes the status file at start,
  every 60 s, checks it every 5 s in between, and writes "stopped" when stopped (first thing of
  every clean exit); see "Status file".
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
- **Link colours** (2026-10-19, user request L3): the addon sends the colours of a TRANSLATE's
  link names in a COLOURS message right before it; the scene keeps the last 8 by (session, reqId)
  and writes them into the TRANSLATE's marks (`marks::with_colours`, `docs\BRIDGE.md` section 3b),
  so the engine gets `0x02 0x04 RRGGBB name 0x03` spans. A translation (and a streamed partial,
  and a reply card's text) holds each link name with a colour as that coloured mark
  (`tp_bridge::marks`, `docs\BRIDGE.md` "Marked spans").
  `fit` parses the marks first: wrapping, the shrink to 80% and the "..." cut measure the
  visible text only, and each fitted line keeps its coloured byte runs (`fit::Run`, a name
  wrapped over two lines is coloured on both; the dots are in the text colour). The overlay turns
  each run into pixel columns (`ColorSpan`, from the advance of the text before it,
  `Fonts::advance_px`, rounded to the nearest pixel) and draws those glyph pixels in the link's
  colour; coverage, shadow (black), mask, card alpha and pixel counts are the same as for the
  plain line. The line cache keys on the marked text, the coverage cache on the visible text. The
  clipboard and the reply card's cut text use `clip::one_line_255`, which removes the marks and
  colour codes first (`marks::plain`), so a pasted reply never holds a colour code.
- Display affinity: `WDA_NONE` under WGC unless `hideFromRecordings`; `WDA_EXCLUDEFROMCAPTURE`
  under GDI.
- `--diag` draws 1 px outlines around tags (yellow) and text rectangles (cyan) and logs at debug
  level. Without `--diag` no outline is drawn (render tests check both).

## Conversation window (Phase B, 2026-10-19)

The addon's conversation window (design A.4 and A.6, `docs\BRIDGE.md` section 8c) shows its
bubbles and its reply card in VIEW id 200 (`scene::ViewKind::Window`; ids 200..255 are addon
windows, 1..199 chat frames).

- **Bubbles:** opaque `BUBBLE_RGB` fill, rows touching at the row pitch. The VIEW rectangle is the
  bubble column, so the native-look samples (`backdrop.rs`, unchanged) land in the fill and the
  painted tag and plate merge into the bubble. In a window view a line starts at the tag's left
  edge + 2 px and may use the tag's pixels as well as the visible plate pixels
  (`overlay::window_card`), since the tag is painted over. "Not received" reads "Not received -
  click the bubble", and a failure "Translation failed - Shift-click the bubble".
- **Reply card** (TRANSLATE with the reply bit, slot 126, rows 2..6): row 0 is the Helper's status,
  rows 1..K-1 the translation (`overlay::Content::Reply`). The status is in the player's language
  (the latest chat request's target, else the Windows UI language: a reply's target is the other
  player's language, so replies never set it):

  | Colour | When | Text (en) |
  |---|---|---|
  | grey 0.6 (153) | waiting | "Waiting (N ahead)...", "Starting the translator...", "Translating..." (also while a partial result streams; partial text is shown, never copied) |
  | green, `GREEN_FONT_COLOR` (25, 255, 25) | the clipboard holds it | "Ready - Ctrl+V to paste", "... (shortened to 255 bytes)", "Already in German - Ctrl+V to paste", "Nothing to translate - Ctrl+V to paste" |
  | amber, `NORMAL_FONT_COLOR` (255, 209, 0) | not copied | "Ready (not copied)" (no consent), "Ready (not copied - off in the Translate Companion menu)" (tray switch off or `--no-clipboard`), "Not copied - you copied something else. Click Translate again", "Not copied - clipboard busy. Click Translate again" |
  | red, `RED_FONT_COLOR` (255, 25, 25) | failed | "Translation failed - click Translate again", "Not received - click Translate again" (an unknown slot 126), low memory, translator files not found |

  Green is painted only after the clipboard write succeeded. The text rows show what went to the
  clipboard (one line, cut at a word boundary to 255 wire bytes, `|` counted as 2); "Already in"
  and "Nothing to translate" copy and show the player's own text.
- **Clipboard** (`clip.rs`, `reply.rs`, decisions D2, D6, D9): only a reply request with the copy
  bit (the player allowed it in the game's consent dialog) while the tray item "Copy translated
  replies to the clipboard" is on (default on) and `--no-clipboard` is not given:
  1. the request arrives (`Action::ReplyStart`, handled on the UI thread before any engine update
     of the job): the clipboard is emptied at once, so a Ctrl+V pressed too early pastes nothing;
  2. the final result: written as `CF_UNICODETEXT` plus `CanIncludeInClipboardHistory` = 0,
     `CanUploadToCloudClipboard` = 0 and `ExcludeClipboardContentFromMonitorProcessing` (kept out
     of the Win+V history and the cloud clipboard), with `TPHelperMain` as the owner, only when the
     clipboard sequence number is still the one of that empty (or the one before it when the empty
     failed); 3 tries 30 ms apart to open it;
  3. CANCEL, a new reply on the slot, CLEAR, an addon session change and a graceful exit release
     it: the clipboard is emptied only when it still holds the Helper's own write.
  The Helper never reads the clipboard's content (sequence numbers only). Tray item "Sound when a
  reply is copied" (default off): `MessageBeep(MB_OK)` after a write.
- **No hint lane** (since addon build 6, 2026-10-19, user request R1): the reply card and the
  bubbles end with their last row (the reply card's lane only said "No status above? Start
  Translate Companion.", the not-running hint the popup replaces); a bubble's links bar may sit a
  few px under its last row, and nothing under a card's last row is ever painted (only its
  fractional edge rows, as for every card). A bubble may reserve more rows than a short
  translation needs: the text is drawn from row 0 down and the unused rows are painted with the
  bubble fill like the plate (`overlay::tests::a_short_translation_in_a_tall_bubble_is_top_aligned_and_the_other_rows_are_fill`).
- **Engine:** a reply is an urgent job (`Job.urgent`): it waits ahead of the chat requests
  (`docs\ENGINE.md` section 5).
- **Capture:** one region per VIEW rectangle with its own tag search and card cache; the window
  view comes first, and a tag the chat frame's view under the window reads again (same slot, row,
  x0 and top) is dropped (`app::dedup_hits`), so the window and a chat frame never show one card
  twice. A capture whose strip brings a different VIEW skips its tag scan (its regions were
  planned with the previous VIEW, so a window's new rows, shown together with the VIEW that
  covers them, would be read against the chat view under them for one capture); the next
  capture scans with the new VIEW, and the previous cards stay meanwhile.

## Status file (user request R1, 2026-10-19)

The addon can never receive anything from the Helper while the game runs, but the client reads the
files of the addon's folder at login and at every `/reload`. So the addon ships
`CompanionStatus.lua`, listed in its TOC before `Core.lua`, holding `TranslateProbe_CompanionStatus
= nil`, and the Helper overwrites the **deployed** copy of that one file (`status_file.rs`):

```lua
-- Written by Translate Companion while it runs (every 60 s). Do not edit.
TranslateProbe_CompanionStatus = { v = 1, version = "0.6.0", build = 6, state = "running", heartbeat = 1792400000 }
```

- **What**: a table of numbers and short ASCII strings only (`version` keeps letters, digits, `.`,
  `-` and `+`, at most 16); `build` is `scene::ADDON_BUILD`; `heartbeat` is Unix seconds. The addon
  counts the app as running when `state` is "running" and the heartbeat is within 150 s of its own
  `time()`; "installed" when the table is there otherwise; "absent" when the global is nil.
- **When**: at start, every 60 s (`HEARTBEAT`), and "stopped" on every clean exit: the tray's Exit,
  `WM_CLOSE` from any of its windows (`taskkill` without /F), `--exit-after`, and a session end
  (`WM_ENDSESSION`, synchronously before it returns). Every 5 s in between (`CHECK`) discovery runs
  again and each folder's file is read (its first 512 bytes): one that is not this Helper's
  "running" file (the addon's nil file after a first deploy, a zip update or a new install) is
  written at once, so a `/reload` 5 s after a deploy reads "running" (integration review: up to 60 s
  before). A check that wrote nothing logs nothing. `stop_workers` writes it first, before the
  capture thread and the engine stop. A second instance (exit code 2), `--help`, `--version`,
  `--selftest` and `--overlay-probe` never write; a test run writes only with `--status-root`.
- **Where**: settings.json `wowRoot` when set (a "World of Warcraft" folder or one flavour folder),
  else every path ending in "World of Warcraft" in `%ProgramData%\Battle.net\Agent\product.db`
  (the Battle.net agent's protobuf database: a drive-letter path is taken when the 1- or 2-byte
  varint right before it is its exact length; no protobuf library). Under each root: the root
  itself when it is a flavour folder, the known flavour folders (`_retail_`, `_classic_`,
  `_classic_era_`, `_classic_beta_`, `_ptr_`, `_xptr_`, `_beta_`) and any other folder starting with
  `_` (WoW Forever runs from `_classic_beta_` on the dev PC). A folder is written to only when its
  `Interface\AddOns\TranslateProbe\TranslateProbe.toc` lists `CompanionStatus.lua` (a line of its
  own, any case), so an older addon never gets a file it does not load. Discovery runs again at
  every write and check (an addon deployed later is found within 5 s). The WoW process is never
  opened.
- **How**: `CompanionStatus.lua.tmp` in the same folder, always a new file (an existing one is
  removed first and the new one opened with `create_new`, so a hard link at that name loses only
  that name and its other names keep their content), written and flushed (`sync_all`), then
  `MoveFileExW(tmp, CompanionStatus.lua, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`: the
  client never reads half a file. A replace refused because the client holds the file open (it
  reads it at a `/reload`) is tried 3 more times 50 ms apart, then the temporary file is removed and
  the next check writes again; the final "stopped" is tried again every 100 ms for 2.5 s
  (`STOP_RETRY`, within the 3 s `stop` waits). Nothing else of the folder is ever written.
- **Links**: an addon folder that is a reparse point (a junction or a symbolic link, for example to
  a repo checkout), or whose `CompanionStatus.lua` or `.tmp` is one, is skipped and counted, never
  written through (integration review: a junction to the repo made the Helper rewrite the repo's
  tracked file, and a hard link at the `.tmp` name was truncated). `scripts\dev.ps1` copies files,
  so a normal deploy is never skipped.
- **Log**: `StatusFile { state, roots, targets, written, failed, skipped }` (counts only, never a
  path; paths hold user names): at Info when it differs from the previous round or a check wrote
  something, else at Debug. Measured on the
  dev PC (read-only): the agent's product.db (3,573 bytes) names one "World of Warcraft" folder,
  three times; its `_classic_beta_` folder holds the deployed addon.
- `scripts\dev.ps1` keeps the deployed status file on a deploy (robocopy `/XF` when the destination
  has one), so a deploy never resets what the running app wrote.

## Presence card (slot 125, user request R1, 2026-10-19)

A [T] click without a confirmed Translate Companion opens only the addon's popup (centred, "Start it
on your computer, then click Continue"). The popup holds a small status card: one row of slot 125
(`scene::PRESENCE_SLOT`, the addon's `Bridge.PRESENCE_SLOT`: a tag and a plate over an opaque
`BUBBLE_RGB` fill, never a TRANSLATE), with the addon's own grey caption under it ("Once
Translate Companion runs, it says so on the bar above."). Its VIEW, id 201, is the card's fill,
sent when the popup opens and again every 2 s while it shows (`docs\BRIDGE.md` section 8d). The
fill left of the tag is wider than the discovery scan's left sample (0.75 of the band height,
`backdrop::DISCOVERY_LEFT`), so a card found by discovery is painted over with the card's own fill
(integration review: with 6 units of fill the sample was the popup's background, a darker strip
until the next VIEW 201; `tests\presence_card.rs` now draws the addon's metrics at 1080p and fails
with the old fill).

- **Scene**: a card of slot 125 with no request is `CardKind::Presence` at once (no 1.5 s wait, never
  "Not received") when the addon's build is 6 or later or not known yet (an app started while the
  popup is open has decoded no strip frame). An addon of build 2 to 5 used slot 125 for a bubble:
  its unknown slot 125 stays pending, then missing (and that addon gets the "addon is older"
  toast); a TRANSLATE on slot 125 makes it a request whatever the build
  (`SceneMachine::presence_slot`).
- **Drawn**: "Translate Companion detected" (`presence.detected`; "Translate Companion détecté" in
  French), in green (`overlay::PRESENCE_RGB` = GREEN_FONT_COLOR, as the reply's "Ready"), in the
  player's language (the latest request's target, else the Windows UI language), from the tag's
  left edge like a window card in any view (VIEW 201, or the discovery view 0, a "chat" view); the
  tag and the plate are painted with the background sampled around them, as every card. Not
  animated, so no 10 Hz timer.
- **Found without a VIEW**: an app started after the popup opened has no VIEW (the strip with the
  popup's VIEW went by before it ran) and scans by discovery, but `tags::scan` looks for a tag's
  first block only in the left max(200 px, 35%) of an area: the chat frames, not a centred popup
  (its tag is about 40% of the width in). Discovery therefore adds areas
  (`app::discovery_areas`): each starts 16 px left of where the previous one's search range ends
  and reaches the client's right edge (a plate is never cut), until the search ranges cover 65% of
  the width (`PRESENCE_REACH`): 3 areas at 1920 px, the first being the whole client as before.
  Only slot 125 is kept from the extra areas (`app::keep_discovery_hit`), so chat cards are found
  exactly as before (late start). Cost: the candidate-row pass reads about 2.2 times the columns
  of before (672 + 459 + 320 px at 1920), twice a second while no VIEW is known.
- **Leaves at once**: while the last discovery scan found the presence card, discovery runs at
  10 Hz instead of 2 Hz (`PRESENCE_SCAN_MS`), so the green text leaves within about 100 ms of the
  popup (Continue, Cancel, Escape); with VIEW 201 the card's region is scanned at the active rate
  (30 Hz). The addon's VIEW after a Continue or a Cancel leaves 201 out.
- An older app (0.5.0) with this addon takes slot 125 for an unknown bubble: the tag and the plate
  painted over at once, then "Not received - click the bubble" in grey after 1.5 s, plus its "older
  than the addon" toast (its code, unchanged). An addon of build 5 or older never shows the popup.

## Hint lanes (removed 2026-10-19, user request R1)

From 2026-10-19 (user request U1) to addon build 5 the addon drew a gold "No translation above?
Start Translate Companion. [Download]" lane under every card that waited for the Helper, and the
Helper searched and painted every lane over (`lane.rs`: window lanes, chat lanes, orphan lanes at
the top of a chat; `tests\hint_lane.rs`). In game the lane cover failed under a bubble with a links
bar, so the hint stayed visible under a painted translation, and the lane row left empty space in
every bubble. Since addon build 6 the not-running message is the popup alone (above), and the
Helper has no lane code: `lane.rs` and `tests\hint_lane.rs` are deleted, `SceneCard::lane`,
`SceneView::lanes`, `scene::LANES_BUILD`, `SceneMachine::lanes_for`, the lane covering test, the
`lane` and `lanes` fields of the `cards` diag record and `lanePx` of the `overlay` record are gone.
The reply card's lane went too: it only carried the not-running hint ("No status above?").

## Game window in the background (decision D12)

Before: the overlay was topmost and a card under any other window was dropped, so after an
Alt-Tab to a window over part of the chat the translations vanished and the raw tags and the
dark plate showed. Now:

- With the game window in front the overlay windows are topmost, as before, and a card under any
  other window is dropped (a topmost overlay would draw over it).
- With another window in front the overlay windows sit right above the game window in the
  z-order (`SetWindowPos` after the first window above the game window that is not ours,
  `SWP_NOACTIVATE`; `HWND_NOTOPMOST` when that window is topmost), which also takes the topmost
  status away: every window above the game window hides the overlay pixel for pixel, so a card is
  never drawn over another application, and the parts of the chat that stay visible keep their
  translations. A card is dropped only when every point of its covering grid is another window
  (`wow::rect_fully_covered`). The capture runs at 5 Hz, and at 20 Hz while the strip is active;
  the strip is read in the background too (review fix, deviation 38): the conversation window
  keeps sending TRANSLATE and CANCEL while the game is not in front (a followed whisper, a slot
  given back), and a missed message left a bubble "Not received", or painted another request's
  text on a reused slot.
- Hidden: the game window minimized or gone; every card fully covered; the UI hidden (Alt-Z: no
  tags, no cards).
- The z-order follows the foreground at once: the UI thread reads it live on every render and on
  every WinEvent (foreground change, minimize start and end; a minimized game window hides the
  overlay at once); the capture thread's scene also carries the state (`Scene::foreground`, part
  of the fingerprint), so a focus change republishes the scene. A window is shown and placed in
  one `SetWindowPos(.., SWP_SHOWWINDOW)` call, so it never appears topmost first.
- Tests: `scene::tests::background_keeps_cards_while_the_game_is_visible` (unfocused but visible
  keeps the cards; fully covered hides; minimized hides; back in front is topmost again) and
  `overlay::window::tests::z_order_right_above_the_game_window` (hidden windows of the test only:
  nothing is shown and the focus never changes).

## `--diag-json` (JSON Lines, `t` = Unix epoch ms, never any text)

Design D.6 records, with extra fields and events:

```
{"t":..,"ev":"frame","ok":true,"mode":"dense","margin":42,"session":1234,"seq":7,"chunks":2}
{"t":..,"ev":"frame","ok":false,"why":"torn|rejected"}
{"t":..,"ev":"msg","kind":"translate","req":5,"slot":3,"rows":2,"bytes":48,"pairs":1,"noCache":false,"plate":true,"reply":false,"copy":false}
{"t":..,"ev":"msg","kind":"cancel","req":5,"slot":3} / {"kind":"clear"} / {"kind":"test","nonce":9}
{"t":..,"ev":"msg","kind":"view","n":1,"views":[[id,x,y,w,h,fontPx,rowPx]]}
{"t":..,"ev":"tags","view":1,"hits":[{"slot":3,"row":0,"x":20,"y":1200,"w":46,"h":14,"alpha":1.0,"block":6.0,"plate":true}]}
{"t":..,"ev":"cards","hidden":null|"no_wow"|"minimized"|"stale","cards":[{"slot":3,"req":5,"view":1,
   "kind":"request|test|missing|pending|presence","rows":[0,1],"bands":[[x,y,w,h],..],"tags":[[x,y,w,h],..],
   "gaps":[[x,y,w,h]],"maskPx":1234,"masks":[{"rect":[x,y,w,h],"runs":[[y,x0,x1],..]}|null,..],
   "alpha":1.0,"plate":true,"textWidth":297,"state":"sent|queued|loading|translating|partial|done|same|nothing|failed|lowmem|notinstalled|test|missing|pending|presence","covered":false,
   "reply":false,"clip":null|"copied|copied_cut|changed|busy|not_allowed|off","bg":[[[r,g,b]|null,[r,g,b]|null],..]}],"fg":true}
   (kind "presence" since 0.6.0: slot 125 of the Translate Companion popup; no "lane" or "lanes" since 0.6.0)
{"t":..,"ev":"overlay","view":1,"shown":true,"rect":[x,y,w,h],"window":false,"px":13240,"textPx":1812,"tagPx":1536,"fillPx":0}
{"t":..,"ev":"clip","req":904,"what":"begin|finish|release","outcome":"emptied|busy|not_allowed|copied|copied_cut|changed|off","cut":false,"excluded":true}
{"t":..,"ev":"z","mode":"topmost|above","moved":true}
{"t":..,"ev":"engine","req":5,"session":1234,"status":"done","ms":200}
{"t":..,"ev":"capture","fresh":true,"method":"wgc","fg":true,"strip":true,"scan":true,"viewSkip":false}
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
capture tick, after the strip decode: `scan` is true only when the tag scan really ran on that
capture, and `viewSkip` true when it was skipped because the strip brought a different VIEW.

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

- `%APPDATA%\TranslateProbe\settings.json` (design D.6 fields, plus `replyClipboard` (true) and
  `replySound` (false), the two tray items of the conversation window, and `wowRoot` ("", since
  0.6.0: the WoW folder of the status file; empty: the ones the Battle.net agent's product.db
  names)). Invalid values fall back
  to their defaults (each fix is logged by field name); an unreadable file is left untouched and
  the defaults are used. A tray switch is saved to the file at once (not when the file was
  unreadable: the switch then lasts until the Helper exits).
- Replies (their text, translation and clipboard state) live in memory only, like the
  translations; the log and diag records of the clipboard flow are names and numbers.
- `%LOCALAPPDATA%\TranslateProbe\logs\helper.log` (or `--log-dir`): lifecycle, capture method,
  per-minute capture/decode/tag counters, message kinds with reqId/slot/byte counts, engine events.
- The logger only accepts `AppEvent` and `tp_engine::EngineEvent` (numbers, enums, fixed names).
  Chat text travels in `ChatText` (no Debug, no Display). A panic logs its line number only.
- Translations stay in memory (last 512 jobs) and are never written anywhere.

## Tests

| Command | What |
|---|---|
| `cargo test --offline` (helper\app) | unit tests: CLI (incl. the hidden `--instance-name`), settings round trip, validation and the old default colour, i18n completeness, fit (shrink to the largest size down to 80%, then "...", 6 rows), render (canvas and DIB: mask, alpha, full-opacity shadow, gap fill, tag cover, no-plate, background gradient over tag, plate, edges and gaps, no outline without `--diag`, dirty-rectangle reuse), background sampling (gradient, text-like pixels, borrowed sides, blended edges only at partial tag rows and at a consistent end column, no right sample for a card cut on the right, discovery spans, a borderless panel next to a plate, sample hysteresis), the 80% shrink with real fonts, the font cell cap (a French line with accents and descenders loses at most 1.5% of its ink at bands of 12-22 px), scene state machine (missing after 1.5 s, covered, stale 500 ms, minimized, session change, messages, weak signal, row counts, background in the fingerprint), diag records, candidate rows (SSE2 against scalar), GDI double buffers, WM_CLOSE on a hidden layered window quits the UI loop; `tests\sentinel.rs`; `tests\tagscan_equiv.rs` (the tag search finds exactly what `tags::scan` finds on 400 synthetic panels, and kept cards equal a new `cards::build`); `tests\native_look.rs` (plates sized as the addon sizes them; alpha 1.0 over a 70-level gradient, alpha 1.0 and 0.6 over a 35-level one: every pixel of the reserved rows within 6 levels of the ground, no plate colour; a tooltip over a plate is never painted, its outer rows included; a frame over the right end of every row of a 1- or 2-row card, parchment or tooltip coloured, leaves the visible plate within 6 levels of the chat; discovery with the game world on both sides of the chat: within 6 levels; a borderless panel in WoW's tooltip colour right under a plate is never painted; an ignored timing test); `tests\graceful_exit.rs` (a spawned Helper, own mutex, echo, no tray, no game window, hidden windows only: `taskkill` without /F through the hidden toast window and, with `--no-overlay`, through the main window; exit code 0 within 8 s and the log's Exit record) |
| `cargo test --offline` (2026-10-19 additions) | clipboard flow on a fake clipboard (empty on receipt, copy when unchanged, Changed, Busy after 3 tries, a busy empty still copies when nothing changed, release only when own, no consent and switch off touch nothing, `one_line_255` with UTF-8, word boundaries and pipes); replies (start, finish only on a final result, Same and Nothing copy the player's text, failures write nothing, exit releases); the lane (clean, under a tooltip, absent; removed in 0.6.0); scene (reply start, replace and end, urgent, never the language; build 2 and 3 without a toast; the lane tests removed in 0.6.0; the background rule of D12); overlay (row 0 colours and texts per outcome, window wording, status on row 0 and text below in pixels, window text origin, lane painted on passing columns only (removed in 0.6.0)); z-order with hidden windows; region planning and duplicate tags; settings, CLI, tray ids; `tests\tagscan_equiv.rs` overlapping views (each tag once, the window's reading equal to a full scan); `tests\sentinel.rs` reply flow |
| `cargo test --offline` (integration review fixes) | the capture period in and out of front (`app::tests::the_strip_is_read_at_its_own_rate_in_the_background_too`), the hidden `--assume-background` flag, every exclusion format tried after a failure (`clip::tests::every_exclusion_format_is_tried_even_after_a_failure`), `ReplyClip::own_write`, clipprobe's skip rules (`unreadable_text_is_never_touched`) and its parent-side comparison of a child's reading |
| `cargo test --offline` (2026-10-19, link colours L3) | fit: the marks and colour codes are never measured, the runs follow wrapping (a name over two lines), UTF-8 and the "..." cut (dots in the text colour), the same lines and size as the plain text; render: a colour span changes the glyph colour only (alpha, black shadow, pixel counts unchanged); overlay: a coloured link name drawn in its colour between text-coloured words, the same coverage as the plain line; fonts: the advance of "Vends " lands within 1 px of the first ink of the next glyph; clipboard and replies: a colour code never reaches the clipboard (`one_line_255`, a coloured Done, Same and Nothing); scene: build 5 served, builds 2 to 4 without a toast, build 6 told to update the app; COLOURS (integration review): written into the next TRANSLATE of its reqId and session only, used once, the newest 8 kept, dropped on CLEAR and on a session change (`scene::tests::colours_go_into_the_next_translate_of_their_request`) |
| `cargo test --offline --release --lib -- --ignored --nocapture render_cost candidate_cost` | render and candidate-row timings |
| `cargo test --offline --release --test native_look -- --ignored --nocapture` | background sampling cost of a 6-row card |
| `cargo test --offline` (2026-10-19, integration review and hint lanes) | removed with the hint lanes in 0.6.0 (`lane.rs`, `tests\hint_lane.rs`); the scene's build tests remain (builds 2 to 6 served) |
| `cargo test --offline --test window_pipeline -- --ignored` | real exe against a 400 x 260 window at the bottom of the z-order: strip, VIEW, TRANSLATE, echo translation, card, in-memory rendering; and a late start (discovery, "missing"); the focus never moves to a window of the test or of the Helper (since 0.6.0: other applications may take it meanwhile) |
| `cargo test --offline --release --test presence_card -- --ignored --nocapture` | discovery scan cost with and without the presence areas |
| `target\release\capprobe.exe` | WGC and GDI read a known pattern exactly (popup and captioned windows), plus WGC on an off-screen window and on a window at the bottom of the z-order (informational) |
| `WoWTranslateCompanion.exe --overlay-probe --diag --exit-after 5 --no-tray` | click-through and focus check |
| `clipprobe.exe live` (by hand, once, decision D9) | the real clipboard path: skipped when the clipboard holds anything but plain text, when its text is listed but cannot be read, or when something is copied between the save and the empty; else saves the text in memory (never printed, never on a command line), runs `ReplyClip` on `WinClip` with a synthetic sentence, has a child process read it back and check the history exclusion formats (the child prints a length and a hash on its stdout pipe; the parent compares), then puts the saved text back (kept out of the history) only over the probe's own last change, and has a child check it; prints verdicts only, never clipboard text |
| `WoWTranslateCompanion.exe --selftest` | discovery JSON |

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

- **Conversation window E2E (2026-10-19, `tests\e2e` scenario `conv`, echo, WGC, fakewow off
  screen, `--no-overlay --no-clipboard`; a WoW client and the user's own Helper were running):**
  every functional check passes: latency P99 107 ms; bands 0 failures in 3009 checks (window rows
  included); hidden, masks, missing, done 4/4; every background sample of a window card within 6
  levels of `BUBBLE_RGB` (136 samples) and the tags painted; the reply card done with the outcome
  "off"; the lane found where the truth has it with 528 of 528 columns passing and painted; the
  half-out bubble's hidden row never got a band and its visible row did; no slot shown twice. The
  only window-slot card in the chat view was one capture where the strip decoded the VIEW in the
  same tick (before the VIEW skip above); with the skip that tick scans nothing.
- **CPU, same conditions:** conv 4.8 to 5.9% of one core over the whole run, 2.75 to 2.93% after
  the first 2 s (the merged window and chat region is 1.5 times the chat area and holds 4 cards of
  9 rows; scan 10.0 and cards 6.9 ms per second against 5.0 and 4.1 for basic). The 9 earlier
  scenarios: 2.6 to 4.9% whole, 1.5 to 2.1% after 2 s, functional checks all passing (cover failed 2
  of 3959 band checks in one capture of a first run, the plates uncovered 33 ms before a capture
  whose WGC frame was older; two reruns 0 of 3954). A/B on basic against the previous build
  (1,225,216 bytes, 2026-10-18), interleaved, exact cycle counts after the first 2 s: 58.4 and
  60.3 Mcycles/s (this build) against 59.1 and 50.5 (previous): no difference beyond the
  run-to-run spread. The CPU rule (3%) and the conv check "within 10% of DESIGN.md I.2" fail on
  this machine state for both builds (the previous build: 2.94 and 3.92% on basic).
- **Integration run after the review fixes (2026-10-19, this build, no WoW client):** the 10 echo
  scenarios pass every functional check under WGC; whole-run CPU 2.24 to 3.54% (only basic over
  3%), 1.36 to 1.83% after the first 2 s in the 7 runs made before the display went to sleep.
  From then on (the 60-minute idle timeout) WGC delivered about 4 fresh frames a second and GDI ran
  2 to 4 ticks a second, for this build and for the previous one alike (A/B on basic), so the
  later figures and the GDI runs are not comparable (`docs\DESIGN.md` K.6). With
  `--assume-background` the conv scenario decoded all 4 TRANSLATE with the game treated as not in
  front (done 4/4, no "Not received").
- **`window_pipeline` (`--ignored`), 2026-10-19:** passed in 3 of 5 runs; each failure was the
  test's own "focus never moves" check (the foreground window changed between two windows of other
  applications during the 6 s run, while the PC was in use); the Helper runs there with
  `--no-overlay` and owns no visible window.

- **Hint lanes and the rename (2026-10-19, `WoWTranslateCompanion.exe` 0.4.0, release build with
  no warning: 1,297,408 bytes; the runs below used the build just before it, 1,300,992 bytes, which
  differs only in how the `--help` text is assembled; a WoW client and the user's own Helper
  running, so every E2E run used `--helper-arg=--instance-name=e2e`, fakewow off screen and
  `--no-overlay --no-clipboard`):**
  the 10 echo scenarios under WGC pass every functional check, the new `lanes` check included
  (every lane under a shown card found within 2 px with at least 60% of its columns passing, none
  where the truth has none, painted, and found before the addon revealed its hint):

  | Scenario | TRANSLATE P99 ms | band failures / checks | lanes found / checks | false lanes | lanes revealed | CPU whole run % | CPU after 2 s % |
  |---|---|---|---|---|---|---|---|
  | basic | 161.1 | 0 / 3509 | 1213 / 1213 | 0 | 3 | 3.63 | 1.89 |
  | scroll | 109.5 | 0 / 3181 | 1965 / 1965 | 0 | 3 | 2.96 | 1.76 |
  | fade | 81.2 | 0 / 1459 | 972 / 972 | 0 | 2 | 3.54 | 2.41 |
  | cover | 109.6 | 0 / 3963 | 1025 / 1025 | 0 | 2 | 2.80 | 1.65 |
  | tabs | 126.3 | 0 / 1984 | 1333 / 1333 | 0 | 4 | 3.92 | 2.30 |
  | altz | 80.5 | 0 / 2754 | 801 / 801 | 0 | 2 | 3.81 | 1.93 |
  | dropped | 153.1 | 0 / 1363 | 927 / 927 | 0 | 2 | 2.94 | 1.53 |
  | late_start | 65.3 | 0 / 1668 | 834 / 834 | 0 | 3 | 4.20 | 1.79 |
  | gamma22 | 96.7 | 0 / 3441 | 1191 / 1191 | 0 | 3 | 3.86 | 2.09 |
  | conv | 72.9 | 0 / 3021 | 1466 / 1466 | 0 | 4 | 4.64 | 2.75 |

  - The CPU rule (3% of one core over the whole run) fails in 7 of 10, as it did for the previous
    build on this machine state. A/B on basic, 3 interleaved pairs, the process cycle count after
    the first 2 s: the previous build (`TranslateProbeHelper.exe` of HEAD, the same exe file the user
    runs, started as a second instance with its own mutex) 53.4, 51.7, 50.5 Mcycles/s, this build
    48.0, 55.1, 50.5: no difference beyond the run-to-run spread. On conv: 73.1 against 73.5.
  - The previous build fails `lanes` in basic and conv (it covers only the reply card's lane): the
    check tells a Helper that leaves the hints visible from one that covers them.
  - A first run of all 10 found gamma22's lanes never covered: the lane search got the card's
    background samples after their conversion back to display colours while it searched the
    linearized capture. The search now runs in the linear capture with the linear samples, and
    the lane's fill goes back to display colours with the card's (`app.rs`, `scan_img`).
  - One `conv` run failed `bands` (3 rows) and `lanes` in one capture: the check expected the
    reply card as soon as VIEW 200 was known, before the grown VIEW that covers it was decoded
    (`tests\e2e\README.md`, deviation 8; check.py fixed, no Helper change).
  - Log sentinel grep (the scenarios' chat words and sender names, "ZZSENTINELZZ") over the 50
    diag, truth, report, fakewow and log files of the final run: 0 hits.
- **Integration review fixes (2026-10-19, the release build of `scripts\helper.ps1 -Build`,
  1,340,416 bytes):** the 10 echo scenarios under WGC pass every functional check, `lanes` included
  (1,923 lane checks and 61 orphan lanes in `scroll`, 0 false lanes anywhere); the CPU rule fails
  in 8 of 10 (3.08 to 4.91%, after 2 s 1.64 to 2.96%), within the spread of identical runs (conv:
  4.91, 3.95, 4.21%). Lane search in release: chat 152 us, over a busy world 240 to 270 us, window
  19 to 24 us; a chat lane under rows that did not move is searched at most every 100 ms.
  `docs\DESIGN.md` section L.3 has the tables.

- **Status file, presence card, no hint lanes (2026-10-19, user request R1; `WoWTranslateCompanion.exe`
  0.6.0, release 1,333,248 bytes, own target folder; the WoW client running and the user playing,
  the user's Translate Companion 0.5.0 left running, so every E2E run used
  `--helper-arg=--instance-name=e2e`, fakewow off screen and `--no-overlay --no-clipboard`):**
  - `cargo test --offline` (helper\app): lib 156 (2 ignored), clipprobe 4, graceful_exit 3,
    native_look 5 (1 ignored), presence_card 2 (1 ignored), sentinel 2, tagscan_equiv 5 passed;
    `window_pipeline --ignored` 2 passed (twice, after the focus check change; before it, the
    late-start test failed 3 times out of 3 on its strict focus check alone: the foreground went
    from a launcher to a chat app and to the game, windows of other processes); release:
    `render_cost` 0.637 ms, `candidate_cost` 0.131 ms per area; `cargo clippy --all-targets`: no
    new warning in the changed code.
  - The 12 echo scenarios under WGC pass every functional check, the new `presence` check
    included; the CPU rule (3% of one core over the whole run) fails in 10 of 12, as at M.3 (7 of
    10) on this machine state; conv also fails `cpu_vs_i2`:

  | Scenario | TRANSLATE P99 ms | band failures / checks | missing | done | presence | CPU whole run % | CPU after 2 s % |
  |---|---|---|---|---|---|---|---|
  | basic | 113.1 | 0 / 3502 | pass (0) | 3/3 | - | 4.09 | 2.08 |
  | scroll | 64.5 | 0 / 3323 | pass (0) | 3/3 | - | 2.36 | 1.53 |
  | fade | 137.2 | 0 / 1322 | pass (0) | 2/2 | - | 2.69 | 1.69 |
  | cover | 189.9 | 0 / 3945 | pass (0) | 2/2 | - | 3.31 | 1.98 |
  | tabs | 182.1 | 0 / 1986 | pass (0) | 2/2 | - | 3.24 | 1.98 |
  | altz | 144.3 | 0 / 2773 | pass (0) | 2/2 | - | 4.20 | 2.12 |
  | dropped | 131.4 | 0 / 1355 | pass (1) | 2/2 | - | 4.30 | 2.26 |
  | late_start | 64.4 | 0 / 1662 | pass (3) | 1/1 | - | 4.47 | 1.78 |
  | gamma22 | 73.7 | 0 / 3524 | pass (0) | 3/3 | - | 3.54 | 1.99 |
  | conv | 98.2 | 0 / 3002 | pass (0) | 4/4 | - | 4.20 | 2.34 |
  | presence | 93.1 | 0 / 593 | pass (0) | 1/1 | detected 143.6 ms after the popup (VIEW 201), gone after it closed | 3.89 | 1.67 |
  | presence_late | 71.3 | 0 / 505 | pass (0) | 1/1 | detected 107.6 ms after the Helper's first capture, by discovery (view 0) | 4.32 | 1.65 |

  - In `presence_late` the background samples of the card found by discovery were the popup's own
    background (18, 16, 14), not the card's fill (36, 30, 24): the discovery left span (0.75 x the
    row height, 15 px) reaches past the card's 6 px padding. The card is then a darker strip with
    the green text until the addon's next VIEW (every 2 s), whose samples are the fill (`presence`:
    no sample off in VIEW 201). Reported by check.py, not judged.
  - Discovery scan cost (release, a 1920 x 1064 noisy world with no tag): 0.696 ms with the
    whole-client area alone, 1.563 ms with the 3 presence areas; at 2 Hz while no VIEW is known,
    +1.7 ms of CPU a second (+0.17% of a core).
  - `check.py --selftest` on the basic, cover, scroll, late_start, conv, presence and presence_late
    truths: the perfect diag passes, the shifted one fails (bands), the broken window fails
    window_fill (conv), and a Helper that takes slot 125 for an unknown slot fails presence.
  - Log sentinel grep (the scenarios' chat words and sender names, the popup's caption,
    "Translate Companion detected", "ZZSENTINELZZ") over the 60 diag, truth, report, fakewow and
    log files: 0 hits. No process of the runs was left (run.py, tasklist); the user's Translate
    Companion kept running.
  - Status file (read only on this PC): product.db names one WoW root; the deployed addon's TOC
    (still addon 0.6.0) does not list `CompanionStatus.lua`, so a 0.6.0 app would write nothing
    there until the build-6 addon is deployed. The writer itself was run against temp folders only
    (unit tests, `graceful_exit`).

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
35. **Conversation window (2026-10-19, design B.3 H2-H10).**
    - `Action::ReplyStart` also carries the request's own text (the design lists `{ key, copy }`):
      "Already in German - Ctrl+V to paste" copies the player's text, which the UI thread does not
      otherwise have. It stays in `ChatText` and in memory.
    - `addonBuild` 3, served with 2 (`ADDON_BUILD_MIN`), not 2 with 1..2: build 2 already shipped
      rows 1..6 (`docs\BRIDGE.md` section 3, WP-P deviation 1).
    - The reply statuses add "Already in {language} (not copied)", "Nothing to translate - Ctrl+V to
      paste" and "Nothing to translate (not copied)" for the Same and Nothing results, which the
      design table does not list.
    - `SetWinEventHook`, `UnhookWinEvent` and `MessageBeep` are declared directly (user32) instead
      of enabling the large `Win32_UI_Accessibility` and `Win32_System_Diagnostics_Debug` features
      of the windows crate; the clipboard uses the design's `Win32_System_DataExchange` and
      `Win32_System_Memory`.
    - The `cards` diag record gains `reply`, `clip`, `bg` and `lane`, the `overlay` record `lanePx`,
      and two records are new: `clip` and `z` (names and numbers only).
    - A capture that decodes a different VIEW skips its tag scan (above, "Capture"). Fact: in the
      first `conv` run the reply card's rows were read against the chat view for one capture.
    - `src\bin\clipprobe.rs` (the D9 live check) is new; nothing runs it automatically.
36. **Background rule (decision D12).** A card is dropped in the background only when it is fully
    covered (any cover in front, as before), and the overlay leaves the topmost band while the game
    window is not in front. Fact: with a topmost overlay and the any-cover rule, a window over part
    of the chat after an Alt-Tab dropped those cards and left their tags and plates visible.
37. **E2E rig: a window layer.** The `conv` scenario draws a synthetic conversation window
    (`tests\e2e\scenario_lib.lua`, `E2E.Window`) as a fakewow layer of opaque rectangles with the
    real tag colours (`Bridge.TagColors`), and sends its VIEW and TRANSLATE messages through the
    addon's own `Bridge.Send`. The addon's window modules are not used (they were written in
    parallel, and the rig checks the Helper against the protocol). `helper\testkit` (fakewow) gained
    the `layer` scenario event and the `layers` truth record; it was not in the work package's file
    list, but nothing else owns the rig.
38. **The strip is read in the background too (integration review fix, 2026-10-19).** Before, the
    strip was grabbed only with the game in front ("5 Hz, tags only"). Fact (review): with smart
    following (D3) the addon sends TRANSLATE for a followed whisper and CANCEL for a slot it gives
    back while the player is Alt-Tabbed, and a strip message stays on screen about 0.4-0.5 s, so
    those messages were lost: the new bubble read "Not received", or, with a window slot reused
    after its 2 s quarantine, showed the old request's translation. The strip (8 client rows) is
    now part of every capture, and the background rate goes from 5 Hz to 20 Hz while the strip is
    active (`app::capture_period_ms`). The hidden `--assume-background` flag lets the E2E rig run a
    whole scenario as if the game were in the background.
39. **The `capture` diag record is written after the decode (integration review fix).** It said
    `scan: true` for a capture whose tag scan was skipped because the strip brought a new VIEW
    (deviation 35), so check.py counted that capture and the `conv` E2E `bands` check failed
    whenever the window's new rows were already visible in it (reproduced by the review: 3 of 3014).
    `scan` now says whether the scan ran, and `viewSkip` tells the skip.
40. **The reply card's lane has its own covering test (integration review fix).** The lane is
    painted opaque under the card's last row, outside `overlay::card_bounds`, so in front a foreign
    topmost window over the lane row alone was painted over with the bubble fill. The lane's
    rectangle is now tested with the same rule as the cards (any cover in front, a full cover in the
    background, cached 100 ms) and a covered lane is dropped; the card stays.
41. **Every exclusion format is set even after a failure (integration review fix).** `Iterator::all`
    stopped at the first format it could not set, so a failed `CanIncludeInClipboardHistory` also
    left out `CanUploadToCloudClipboard` and `ExcludeClipboardContentFromMonitorProcessing`.
    `clip::set_every_exclusion` tries all three and still reports the failure.
42. **clipprobe hardening before the one live run (integration review fixes).** It skips (exit 3)
    when the clipboard lists text it cannot read (a delayed rendering whose owner does not answer:
    it used to empty it and "restore" an empty clipboard, and report a pass), and when something
    is copied between its save and its empty (the sequence number read with the clipboard open is
    checked again when `begin` opens it). It restores only over its own last change (its write, or
    its empty when nothing was written), checked with the clipboard open. The saved text's length
    and hash no longer go on the child's command line: the child prints its reading on its stdout
    pipe and the parent compares.
43. **Hint lanes under every card (user request U1, 2026-10-19).** The reply card's lane (design
    A.6.4) was the only one; the addon now draws a lane under every card that waits for the Helper,
    and the Helper covers each one while it paints the card, in any state ("Hint lanes" above).
    `lane::find_lane` became `find_window_lane` (with an unknown K: right under the last visible
    row; the lane must lie inside the view) and `find_chat_lane` is new; `Lane` holds its kind and
    a fill (`BandBg`: flat for a window, a gradient for the chat) instead of one colour; the scene
    keeps a lane for every card kind (it dropped all but the reply's); the `cards` diag record's
    `lane` gains `kind` and `rgbRight`. Facts behind the chat rules: the chat font's glyphs and
    their shadow reach the line's top and bottom rows (the window's hint is placed clear of them),
    and the addon starts the inline hint at the tag's x0 (`Reserve.LaneRow`) with `[Download]` in
    its links' blue, so the tag column test accepts the lane's own colours and the edge test
    accepts glyph edges and shadows next to them. The ground is the card's last row's background
    rather than a new sample on the lane's rows: a first version sampled each of the 5 candidate
    positions and classified and fitted every pixel, 2,139 us per card in release; reading the
    rows once, lazily, with the row's samples brings it to 39.6 us (`hint_lane::lane_search_cost`).
44. **Known limits of the chat lanes** (updated by the integration review). A tooltip's navy body
    is close to a mix of 17% link blue and black, so where it touches a `[Download]` glyph its outer
    2 px may join the lane. A cover whose top edge lies exactly at the lane's top (its body over the
    whole lane, the row above uncovered) is not told from the chat: since the local ground, of any
    colour (before, only over a ground of nearly its colour). A straight horizontal edge of the
    game world behind a transparent chat (a horizon, a UI element under the chat) inside the lane
    is taken for a cover: those columns are left, and the hint shows there. Per-pixel texture beyond
    about +-30 levels exceeds the largest tolerance (48): some columns are left. The hint's partial
    rows at a fractional line position below the lane are not covered (the lane is whole rows; the
    row above it is painted with it). An orphan lane is followed only from where the Helper saw it
    under its card (a Helper started with a lane already at the top leaves it), and no local ground
    is made for it. With the gold boss emote of an older addon no longer an issue (no chat lane is
    searched for builds before 4), that limit is gone.
45. **Rename to Translate Companion (user request U2, 2026-10-19).** The exe and every
    user-visible string changed; the crate names, the mutex, the window classes and the data
    folders did not (see the top of this file). `tp_helper::{PRODUCT_NAME, EXE_NAME, RELEASES_URL}`
    are new, the tray menu gains "Get updates" (`MenuCmd::Updates`, `tray.updates`) and the tooltip
    the version (`tray.tip` takes `{version}`); `AppEvent::TrayUpdates` is new. Helper version 0.4.0.
46. **Addon build 4, and lanes only where the addon draws them (integration review, 2026-10-19).**
    `scene::ADDON_BUILD` is 4 with the addon's `ns.ADDON_BUILD` (builds 2 to 4 served): a Helper
    0.3.x shows "older than the addon" instead of leaving every new hint visible. The capture
    searches a card's lane only per `SceneMachine::lanes_for`: every card for build 4 or a build not
    known yet, the reply card only for builds 2 and 3, never the test card (slot 127: under its rows
    the addon prints "TranslateProbe: ... Overlay OK", whose link-blue prefix passed for a lane).
47. **Window lanes: the request's last row, the card's own fill, glyph edges (integration review).**
    `find_window_lane` refuses a lane when the request's last row is not the card's last visible row
    (a bubble cut by the thread or under a tooltip: VIEW 200 reaches over the reply card, so a lane
    under a lower row painted over its draft line) and when the tag-column fill is more than 8 levels
    from the card's background sample (a tooltip's body); its edge rows accept the hint's own glyph
    edge at a small UI scale, as the chat lanes do. `find_window_lane` takes the card's `CardBg`.
48. **Chat lanes: local ground, cover edges, indent, cut rows (integration review).** See "Hint
    lanes": a column failing against the row samples' gradient is judged again against a local
    ground and texture from the lane's own plain pixels; straight horizontal cover edges inside the
    lane fail their columns; the indent left of the tag column must be free of ink; a last row cut
    by the top of the chat is measured from its bottom; the lane takes the card's last partial row.
    Before, a +-30 texture left the lane unfound (816 gold pixels visible under a painted card), a
    feature 30 to 45 levels off the gradient left its columns, and a tooltip edge in the lane's
    middle rows over a dark chat was painted over.
49. **Orphan lanes (integration review).** See "Hint lanes": a chat lane whose card's rows left the
    top of the chat is followed there from the memo of where it was found (its ink must match), and
    painted on its visible rows; `SceneView::lanes`, `FoundLane`, `SceneMachine::update_found_lanes`
    and the `lanes` list of the `cards` diag record are new.
50. **A chat lane is searched again at most every 100 ms while its card's rows stay put
    (integration review).** In between it keeps its columns and takes each capture's background
    (`app.rs`, `lane::RESEARCH_MS`): the full search, with the local ground and the cover edges,
    costs 0.15 to 0.27 ms, and a world moving behind a translucent chat changes every capture. A
    cover arriving over a lane is therefore seen within 100 ms, like the covering test.
51. **Toasts fit a small game window (integration review).** The French "older than the addon"
    toast was 1,106 px wide on its single line; every toast is now at most 800 px wide in English
    and French (`toast::tests::every_toast_fits_a_small_game_window`).
52. **Hint lanes removed (user request R1, 2026-10-19).** Deviations 43, 44 and 46 to 50 describe
    code that is gone: the addon of build 6 draws no lane, so `lane.rs`, the lane covering test,
    the orphan lanes and their diag fields went (section "Hint lanes (removed ...)"). An addon of
    build 2 to 5 gets the "addon is older" toast since the integration review (deviation 58).
53. **The status file only where the TOC lists it.** The lead's design writes into every flavour
    folder that holds `TranslateProbe.toc`; the Helper also requires that TOC to list
    `CompanionStatus.lua`. Fact: an addon older than build 6 does not load the file, so writing
    it there would only leave a stray file in a folder the Helper does not own; the build-6 TOC
    lists it, so nothing a current addon reads is lost.
54. **No status file for test runs.** Fact: the E2E rig and the window tests start the exe while
    the user's own Helper runs; a test Helper writing "running" and then "stopped" into the real
    WoW folders would tell the addon at its next `/reload` that the user's Helper is stopped.
    `--instance-name`, `--no-overlay`, `--wow-class`, `--assume-foreground`, `--assume-background`
    and `--overlay-probe` therefore turn it off unless `--status-root` names a folder of the test's
    own (`tests\graceful_exit.rs` does).
55. **Discovery searches the middle of the client for slot 125 (presence card).** Not in the
    lead's design, which relies on VIEW 201. Fact: `tags::scan` looks for a tag's first block in
    the left max(200 px, 35%) of an area, and an app started after the popup opened has no VIEW
    until the addon sends it again (every 2 s): without the extra areas the card was found only
    then (`tests\presence_card.rs`: the whole-client area alone finds nothing in the middle). With
    them the card is painted within about a second of the app's start. Only slot 125 is taken from
    the extra areas.
56. **Discovery at 10 Hz while the presence card shows.** Fact: at 2 Hz the green text could stay
    over the game for up to 0.5 s after the popup closed. The faster discovery lasts only while the
    card is found (a popup is open and no VIEW is known).
57. **`window_pipeline`: the focus check judges our windows only (2026-10-19).** Fact: with the game
    played during the run, the late-start test failed its "the foreground never changes" check 3
    times out of 3 while every functional check would have run; the foreground had gone to windows
    of other processes (a launcher, a chat app, the game). The check now records every foreground
    window seen during the run and fails when one belongs to the test or the Helper, which is what
    amendment A1 forbids.
58. **`ADDON_BUILD_MIN` 6 (integration review of 2026-10-19).** The package kept 2, so addons of
    builds 3 to 5 were served without a toast while their gold hint lanes, no longer painted over,
    stayed under every translation: the bug of the user's screenshot, with nothing saying why. Now
    every build before 6 gets the "addon is older" toast once per session
    (`scene::tests::addon_build_mismatch_toasts_once_per_session` covers builds 1 to 5).
59. **The status writer never follows links (integration review).** A probe showed that a hard link
    at `CompanionStatus.lua.tmp` was truncated and written, and that an addon folder junctioned to a
    repo checkout made the Helper rewrite the repo's tracked file. The temporary file is now removed
    and created with `create_new`, and a reparse point at the folder or either file is skipped and
    counted (`skipped` in the log). Tests in temp folders: a hard link keeps its content, a
    junctioned folder is skipped (`mklink /J`, no privilege needed).
60. **A 5 s check between heartbeats, and "stopped" retried (integration review).** After a first
    deploy or a zip update the addon's nil file stayed for up to 60 s, so a `/reload` then read
    "absent"; and a "stopped" refused by a client read left "running" with a fresh heartbeat for up
    to 150 s. The writer now reads each file every 5 s and writes over one that is not its own
    "running" file, and tries the final "stopped" again for 2.5 s. Tests: a file put back to nil is
    written within one check, a new addon folder is found by a check, and "stopped" lands after a
    reader held the file for 600 ms.
61. **`tests\presence_card.rs` uses the addon's card metrics at 1080p (integration review).** It
    drew the card on the popup's flat background with a 14 px right pad, so neither the discovery
    sample crossing the card's left fill nor the narrow right sample was exercised. It now draws the
    `BUBBLE_RGB` fill with `CompanionDialog.CardMetrics` (fractional px), sends VIEW 201 as the
    addon rounds it, and checks both runs' samples against the fill; with the old 6-unit left fill
    the discovery run fails.
