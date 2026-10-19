# Pixel bridge v2

The addon talks to TranslateProbe Helper through pixels only: a strip of coloured blocks at the
client's top-left corner carries messages (addon to helper), and row tags printed in the chat
identify the reserved rows the helper draws on. This file is the normative spec of both, the
record of how they were validated, and the list of places where the implementation differs from
the design (`docs\DESIGN.md`; the original plan's sections C.4 and D.2) and why.

| Part | Where |
|---|---|
| Encoder, message builders, tag and plate markup (Lua) | `addon\TranslateProbe\Bridge.lua` |
| Reference implementation (Python) | `tools\bridge\proto.py` |
| Test vectors and their generator | `tests\vectors\`, `tools\bridge\make_vectors.py` |
| Lua against the vectors | `node tests\bridge\run.js` (`tests\bridge\bridge_test.lua`) |
| Simulation, colour curves, CRC Monte Carlo | `tools\bridge\sim.py`, `curves.py`, `crc_mc.py` |
| Helper side (Rust crate `tp_bridge`) | `helper\bridge\src\{crc,image,strip,msg,reasm,tags,cards,tone,synth}.rs` |
| Rust tests | `helper\bridge\tests\{vectors,distort,tags,cards,tone,corrupt,perf}.rs` |

## 1. Strip pixels

- The strip sits at client (0,0), 4 physical px tall, made of 2x2 blocks. Block k is at
  `x = 2 * floor(k / 2)`, `y = 2 * (k % 2)`: columns are filled top then bottom, left to right.
- Levels `L = {0, 104, 146, 255}`. Blocks 0-7 are calibration greys with level indices
  `0, 3, 3, 0, 1, 2, 2, 1` (black, white, white, black, grey1, grey2, grey2, grey1).
- Then the frame bytes, most significant bit first, cut into symbols; the last symbol is zero
  padded.

| Mode | Magic | Bits per block | Colour of symbol s |
|---|---|---|---|
| 0 dense | 0xD5 | 6 | `(L[s>>4&3], L[s>>2&3], L[s&3])` |
| 1 robust | 0x2A | 3 | `(L[3*(s>>2&1)], L[3*(s>>1&1)], L[3*(s&1)])` |
| 2 mono | 0x5A | 2 | `(L[s], L[s], L[s])` |

- Mode `auto` = mono when `GetCVar("colorblindSimulator")` is set and not `"0"`, else dense.
- The strip frame is a child of UIParent (Alt-Z hides it), strata TOOLTIP, level 9000,
  `SetIgnoreParentScale(true)`, `SetScale(768/physH)` (1 unit = 1 physical px), 2x2 textures with
  `SetSnapToPixelGrid(false)` and `SetTexelSnappingBias(0)`. Only changed blocks are recoloured.
- It is **hidden while the queue is empty** (no OnUpdate cost when idle), hidden on
  LOADING_SCREEN_ENABLED and shown again on LOADING_SCREEN_DISABLED if busy; `Bridge.Hold(true)`
  keeps it hidden with the queue intact.

## 2. Frame bytes (big-endian)

```
0 magic u8 | 1 version u8 = 2 | 2 seq u8 | 3 flags u8 = 0 (reserved, ignored) | 4 session u16 (1..65535) | 6 L u16
8 records (L bytes): STATUS, then CHUNK x n | 8+L CRC-32 (zlib / ISO-HDLC) of bytes 0..8+L-1
record = type u8, len u16, data; unknown types are skipped (0x02 = the v1 LAYOUT)
0x01 STATUS (len 6): physW u16, physH u16, addonBuild u16
0x03 CHUNK: msgId u16, idx u8, count u8, total u16, msgCrc u32 (CRC-32 of the whole message), data
```

- Frame capacity = `floor((2 * cols - 8) * bits / 8)` with `cols = physW / 2`; chunk data per
  frame = capacity - 8 - 4 - 9 - 13.

| Screen | dense | robust | mono |
|---|---|---|---|
| 2560 px | 1880 B | 923 B | 604 B |
| 1920 px | 1400 B | 683 B | 444 B |
| 1366 px | 984 B | 475 B | 305 B |

- Strip width: `cols = physW / 2`, never more (blocks past the client's right edge are never
  captured). The `maxCols` encoder option can only narrow it, down to MIN_COLS = {192, 384, 576}
  block columns (dense, robust, mono: 248-252 B of chunk data); a degenerate screen narrower
  than 400 px still gets 200 columns (64 B of chunk data in mono).
- Timing (addon, ms from GetTime): HOLD 50 between strip changes, ROT_HOLD 70 per rotation step,
  EXPOSE 400 of screen time per chunk before its message is dropped; screen time is credited from
  the second game frame after a change, at most 100 per game frame, and not while hidden. When
  the chunks do not all fit, the least exposed go first. Messages are cut into chunks again on
  DISPLAY_SIZE_CHANGED / UI_SCALE_CHANGED. No keepalive. When the queue drains, no further frame
  is composed; the next queued message is composed at once on the next update.

## 3. Messages

`str8` = u8 length + bytes, `str16` = u16 length + bytes.

```
0x10 TRANSLATE  reqId u16 | slot u8 (0..126) | rows u8 (1..6)
                | flags u8 (bit0 noCache, bit1 plate, bit2 reply, bit3 copy, bits 4-7 = 0)
                | target str8 (printable ASCII, 1..8) | text str16 (UTF-8 <= 1024 B, may contain 0x02/0x03 marks)
                | nPairs u8 (<= 8) | pairs: term str8 (<= 48 B), meaning str8 (<= 120 B); sum(2+#t+#m) <= 512
                rules: copy => reply; reply => plate and rows 2..6 (row 0 = the Helper's status, rows 1.. =
                the text); a reply's target is a language code (ASCII letters and '-', starting with a
                letter); a reply may use any slot 0..126 (the addon uses 126)
0x11 CANCEL     reqId u16 | slot u8 (0..127)
0x12 CLEAR      (empty)
0x13 TEST       nonce u16
0x14 VIEW       n u8 (0..4) | n x { id u8, x u16, y u16, w u16, h u16, fontPx u8, rowPx u8 }  (physical px, client origin)
                ids 1..199: chat frames (GetID() % 256); ids 200..255: addon windows (200 = the conversation window)
0x01 STATUS     addonBuild 3 since 2026-10-19 (TRANSLATE reply and copy)
```

- 2026-10-18: TRANSLATE rows went from 1..4 to 1..6 (long German-to-French lines were cut); same
  version 2, and a tag still carries the row modulo 4 (its row field has 2 bits). Every parser
  follows: `msg.rs` (Rust), `tools\bridge\proto.py` (the Python reference, with a 6-row
  vector and a refused 7-row one in `tests\vectors\messages.json`). Because a Helper
  built for 1..4 rows drops a longer request as a parse error, STATUS.addonBuild went from 1 to
  2 at the same time: either side then shows its "update" toast instead of failing silently.
- Since 2026-10-18 the addon sends a VIEW before every TRANSLATE (changed or not), so a Helper
  started after the last VIEW leaves its discovery scan at the next click.
- 2026-10-19: the conversation window's reply card (docs\DESIGN.md, conversation window). Two
  TRANSLATE flags are new, same version 2:
  - **reply** (bit 2): the request is the user's own reply. Row 0 of its card carries the Helper's
    status ("Translating...", "Ready - Ctrl+V to paste", ...), rows 1 to rows-1 the translation.
    So a reply needs the plate and 2..6 rows. Its target must be an explicit language code: the
    addon resolves an undecided language with its language list before it sends (a placeholder
    such as "?" is refused).
  - **copy** (bit 3): the user allowed the clipboard for this reply (the in-game consent). The
    Helper may then empty the clipboard at once and put the final translation in it. It is only
    valid with reply: the Helper never copies a message the user received.
  - Builders mask the flags to bits 0-3 and drop copy when reply is not set
    (`Bridge.TranslateMessage`, `proto.msg_translate`); any other combination is written as given
    and refused by the parsers (`msg.rs`, `proto.parse_message`). Of the 16 values, 0-3, 6, 7, 14 and
    15 are valid.
  - `STATUS.addonBuild` went from 2 to 3 (`ns.ADDON_BUILD`): a Helper of build 2 refuses a reply
    (its flags check) and shows its "update" toast. The Helper of build 3 accepts builds 2 and 3
    without a toast (a build-2 addon never sends a reply).
  - Example (reply with copy: reqId 258, slot 126, rows 3, "de", "on part dans 5 min ?", 0 pairs),
    also a builder vector and a Rust test:
    ```
    10 | 01 02 | 7E | 03 | 0E | 02 64 65 | 00 14 | 6F 6E 20 70 61 72 74 20 64 61 6E 73 20 35 20 6D 69 6E 20 3F | 00
    ```
  - Compatibility: a new addon with an old Helper keeps chat and window translations (window
    bubbles are plain TRANSLATEs with the plate); its replies are dropped as parse errors ("Not
    received", the lane hint and the "this Helper is older" toast show). An old addon with a new
    Helper: no change.
  - Deviation from the conversation design (B.2), backed by a fact: the design set addonBuild 2 for
    this change because it read build 1 in the code, but build 2 already shipped with rows 1..6
    on 2026-10-18 (`Core.lua`, `scene.rs`). Keeping 2 would leave a build-2 Helper dropping replies
    without a toast, so the build is 3 and the Helper accepts 2..3.
- Parsers are strict: a length that does not match exactly, a field out of range or invalid
  UTF-8 drops the message (`msg::ParseError`) and a counter is incremented.
- Builders (`Bridge.TranslateMessage` etc.) write integers as given, clamped to the field width;
  the helper refuses out-of-range values rather than the addon remapping them. The text is cut at
  1024 B on a UTF-8 boundary and never inside an open 0x02 mark.

## 4. Helper side: strip decoder (`strip::decode_strip`)

1. The capture covers client rows 0..8 from x = 0. For each of the 4 sub-pixel phases of a 2x2
   block, the calibration contrast is the worst channel of the white/black differences; phases
   within 0.8 of the best are kept. Best under 16: **Absent**.
2. Block colours are averaged over the kept phases (only as many blocks as the frame needs).
3. The calibration copies (0/3, 1/2, 4/7, 5/6) must agree within 20 on every channel, else
   **Torn**. The four reference levels and their smallest gap (`Calib.min_gap`, the signal
   margin) come from them.
4. Modes are tried in order dense (only when the smallest gap is at least 8), robust, mono: the
   header symbols are classified at the mid thresholds; the magic must match; version 2; the
   length must fit in the capture. Dense and mono re-estimate the four levels from all blocks of
   the frame (decision directed) and classify again; the CRC-32 is checked on the refined reading,
   then on the first. Then the records are parsed strictly (STATUS first, CHUNK fields
   consistent). Any failure: **Rejected**.

## 5. Reassembly (`reasm::Reassembler`)

Messages are keyed by (session, msgId). Chunks of one message must agree on count, total and
msgCrc (a disagreement restarts the message). A complete message must match its total length and
CRC-32, then `msg::parse_message`. A done-set of 512 ids drops repeats; partial messages expire
after 5 s; a msgId still missing 3 s after a later one completed counts as missed (gaps over 64 are
not tracked: an addon reload restarts at 1); a new session clears everything and is reported once
by `take_session_change()`.

## 6. Row tags (encoding)

- 8 inline textures, each
  `"|TInterface\\Buttons\\WHITE8X8:" .. H .. ":" .. W .. ":0:0:8:8:0:8:0:8:" .. R .. ":" .. G .. ":" .. B .. "|t"`.
- B0 = (255,0,255) magenta and B1 = (0,255,0) green form the sync pair (complementary: every
  channel gets a high and a low reference).
- B2..B7 are 6 base-6 digits: 0 black, 1 red (255,0,0), 2 blue (0,0,255), 3 yellow (255,255,0),
  4 cyan (0,255,255), 5 white. Magenta and green never appear as data.
- `payload10 = slot << 3 | row << 1 | sessBit` (sessBit = sessionId % 2); CRC-5 with polynomial
  0x05, init 0x1F, payload bits most significant first (`c = 0x1F; for i = 9..0: bit = p>>i & 1;
  top = c>>4 & 1; c = (c<<1) & 0x1F; if bit ~= top then c = c ~ 0x05`); `v = payload10 << 5 | crc5`
  (< 32768) written in base 6, most significant digit first. Check values: (0,0,0) gives digits
  0,0,0,0,4,1; (5,2,0) 0,1,0,3,3,3; (127,0,0) 4,1,0,3,4,3. CRC-5 detects every single-bit error.
- The plate follows B7: `"|TInterface\\Buttons\\WHITE8X8:" .. H .. ":" .. plateW .. ":0:0:8:8:0:8:0:8:32:40:64|t"`.
- `TEST_SLOT = 127` never carries a TRANSLATE.

## 7. Tag scan (`tags::scan`)

1. Rows y step 2 over the area; B0 must start in the left `min(w, max(200, 0.35 w))` px.
2. Sync pair: a pixel p with a pixel q at most 4 px to its right, inside the scan area, such that
   p is above q by 40 or more on red and blue and below it by 40 or more on green. Every candidate is tried, left to
   right. B0 and B1 are bounded by colour similarity (24); B1 must start within `max(2, bw/2)` px
   after B0 (+2 for blended edge pixels).
3. Vertical extent of B0 and B1 by similarity walks, cut where any pixel of the tag's span changes
   (two rows of one card can touch); h >= 4.
4. On the row profile averaged over the tag's rows, the block edges are measured to a fraction
   of a pixel from two-colour blends. The pitch is searched within +-1.5 px of the sync pair's
   (0.05 px steps) for the most uniform block interiors; each range of equally good pitches is
   then fitted to the blocks' full extents (B0's edge free by +-0.5 px). When no range decodes,
   the block width joins that fit (+-0.5 px) and the ranges are read again. Each block is sampled
   in a box of half-size `(max(1, bw/4), max(1, h/4))` inside its fully covered pixels:
   per-channel max-min <= 28 (uniformity); every channel of a data block within 0.25 of the
   contrast from the sync pair's low or high reference (alphabet fit); no third block colour
   between two blocks; just past B7 comes the plate or the background, not more of B7; digits at
   the mid thresholds; a magenta or green digit, v >= 32768 or a bad CRC-5 rejects the tag. Every
   range that decodes must give the same v.
5. The scan continues right of the tag and skips the rows it covers. A refused geometry is not
   read again on later rows.
6. `alpha = mean_c((hi_c - lo_c) / (white_c - black_c))` with the last strip calibration
   (default 255 / 0), clamped to 0..1.

## 8. Cards (`cards::build`)

1. Hits with alpha under 0.3, or of the other session bit, are dropped. Hits group by (view,
   slot) and are ordered by y. A tag carries its row modulo 4 and a request has up to 6 rows,
   so rows are numbered on from the first visible one (`cards::build_with_rows`, with each
   known slot's TRANSLATE row count K; 6 when unknown):
   - the next tag continues the card as row `prev + step`, where step = 1..3 is how far its tag
     row is (modulo 4), when it lies within `(step + 0.6)` row pitches (the pitch is
     `max(rowPx, tag height + 1)`), or as row `prev + step + 4` when it lies within 0.6 pitches
     of that distance (the rows between are hidden by a tooltip; step 0 can only be that);
   - the row must stay below K; anything else starts another card (a copy);
   - a run of a 5- or 6-row request that stops before its last row while its first tag is at
     the top of the view (within 1.5 pitches) is the request's last rows, the first ones being
     scrolled away (a chat clips lines only at its top): it is renumbered + 4.
   Without K (TRANSLATE not received) tag rows 0 and 1 at the top of a view read as rows 0 and
   1. `cards::build` is `build_with_rows` with no row count known.
2. **Plate start.** `Pobs` = per-channel median of `[x1+2, x1+2+max(3, bw)] x [cy - h/4, cy + h/4]`,
   with its spread (75th percentile of the distances to Pobs). It is a plate when the spread is at
   most 16 and either
   - **the prediction holds**: the tag's blocks are read again; per channel, `lo` is the block
     nearest the plate drawn 0 on that channel and `d = hi - lo` the sync pair's contrast. A
     chat line faded to alpha over a ground g reads `(1 - alpha) g` where drawn 0 and
     `alpha 255 + (1 - alpha) g` where drawn 255, so the plate reads `lo + d * PLATE_RGB / 255`
     over any ground. Tolerance per channel: `5 + min(spread, 8) + 0.5 |lo_near - lo_sync| +
     8 (1 - alpha)`;
   - **or the hue rule holds**: B > G > R, B - R >= 10 alpha, G - R >= 3 alpha, every channel
     <= 150 (kept for display curves that bend the prediction).
3. **Covers.** A plate start that passes is still refused when
   - a column of the sample window (after its first) differs from Pobs by more than
     `10 + min(spread, 6)` (an edge or a border inside the window), or
   - the rows 2 and 3 px above, or below, the row (where the card has no other row) have the
     colour of Pobs: within `6 + min(spread, 6)` of it and at most half as far from it as from
     the ground behind the plate start (the tag's `lo_near / (1 - alpha)`, or the indent's ground
     left of the tag when alpha > 0.85). A plate is one row tall; a cover is not. A plate-like run
     that reaches the view edge within 1.6 rows is a clipped row of the card (never above row 0).
     Inconclusive when Pobs and that ground are closer than `max(8, tolerance)`;
   - or the row sits next to a covered or non-plate row of the card (gap <= 6 px) with the same
     Pobs (the middle rows of a cover over several rows have no free side to test).
   Such rows are dropped, like a plate expected but not found (never paint over unknown pixels).
4. **Mask** over yTop..yBot from x1+1 to the view's right edge, tolerance
   `min(14 + round(10 (1 - alpha)), 6 + 2 spread)`. Per column, the plate colour is followed along
   the row (median of the 3 middle rows; it moves 1/4 of the way toward columns that are clearly
   plate, at most the tolerance away from Pobs) because the ground's texture shows through a
   faded plate. A pixel is set when it is within the tolerance of its column's plate colour and,
   when the ground behind it can be told apart (at least `max(8, 3 spread)` away), nearer to the
   plate than to that ground (`(P - d PLATE / 255) / (1 - alpha)`, or the indent's ground when
   alpha > 0.85). Holes of 2 px or less are closed when their pixels are within twice the
   tolerance.
5. **Text rectangle**: from x1+3 to where the plate run that starts within 6 px of x1 ends, on the
   per-column classification: the run ends at the first column from which 3/4 of the next 8
   columns are not plate; a stretch of up to 32 ground-like columns after which the plate
   resumes is texture, not the end, unless it holds a column that is neither plate nor ground (a
   cover or a border).
6. **Gaps** of 4 px or less between consecutive rows are filled where both neighbouring masks are
   set (on either of each band's two rows next to the gap).
7. **No plate expected** (`plate_expected(slot) == Some(false)`): text from x1+3 to view.right-4, no
   mask, filled with the median of a 6 x h/2 patch at view.right-10.
8. `RowBand.tag_rect` is the area that hides the tag: columns x0 - 1 ..= x1 (x0 and x1 are rounded
   sub-pixel edges, so these columns can hold part of B0 and B7) and rows yTop..=yBot, plus the row
   just above or below when it is a partial row of the tag (at every block centre the pixel is
   ground + f (block - ground) with the same f >= 0.08, the ground read one row further out). The
   overlay paints it opaque with the observed plate colour (which already includes the fade);
   every reported tag passed the uniformity check. Gaps between rows are measured on the tag
   rows (`text_rect`), not on `tag_rect`.
9. `text_width_px` is the widest band; `StickyWidth` keeps the 2 s maximum per (view, slot) (the
   Helper applies it to every capture's cards).

## 8b. Display curves (`tone`)

The tag reader and the plate rules assume a linear capture. A display curve between the game and
the capture (the game's Gamma setting, contrast, brightness) bends every channel, so blended edge
pixels leave the blend line and a faded plate leaves its prediction. The strip measures the
curve: `strip::Calib::levels` holds the four reference levels per channel (drawn 0, 104, 146,
255). `tone::Tone::from_levels` fits `out = black + (white - black) (in / 255)^e` per channel to
the two middle levels; a calibration with every exponent within 0.12 of 1 and black and white
within 12 of 0 and 255 is linear and changes nothing. Otherwise the Helper copies the scan area
through the inverse curve (`Tone::apply`) before `tags::scan` and `cards::build`, scans with the
nominal 0 / 255 calibration, and converts each card's `plate_rgb` back to display colours
(`Tone::display`) for the overlay. The curve is kept from the last strip seen (like the
calibration) and reset when the game window is lost; an unchanged capture reuses the previous
linear copy.

## 8c. Window cards (the conversation window, 2026-10-19)

The conversation window draws its tags and plates with textures (`Bridge.ApplyTag` colours the 8
tag textures; the plate is a `PLATE_RGB` colour texture), inside bubbles of its own, not inline in a
chat line. Rules the addon keeps and the Helper relies on:

- The bubble and reply card interior is opaque `BUBBLE_RGB` = (36, 30, 24). It fails the plate
  prediction and the hue rule (B 24 is under G and far under the plate's 64), so the Helper never
  takes the fill for a plate.
- At least 3 physical px of fill above the first row, below the last row and left of the tag. Rows
  touch: row tops sit at exact multiples of `rowPx` physical px, so the row pitch equals VIEW
  `rowPx`.
- One VIEW entry, id 200, for the whole window: its x-range is the bubble fill column (so the
  native-look backdrop samples land in the fill on both sides), from the thread top to the reply
  card's fill bottom; `fontPx` and `rowPx` from the window's text size and scale.
- A row not entirely inside the visible thread has its tag and plate hidden (fill only): the
  Helper never sees a partly clipped window row.
- The reply lane is the band `[row0.yTop + K * rowPx, + rowPx]` from the tag's x0 to the plate's
  end: flat fill with the addon's hint text, clear of its top and bottom 2 px, which the Helper
  covers when it paints the card.
- Slots: 0..99 chat, 100..125 window bubbles, 126 the reply, 127 TEST.

## 9. Validation

Commands (from the repo root; numbers measured on 2026-10-17 on the dev PC, re-run on 2026-10-18 and 2026-10-19 at integration with the same results where shown):

| Command | Result |
|---|---|
| `python -I tools\bridge\make_vectors.py` | 30 traces (10 x 3 modes), 1066 frames, 387 queued messages, 1024 tags, 52 message vectors (46 builder vectors and 6 parser-only byte strings; 34 before the reply and copy flags, 2026-10-19), 36 strips; byte-identical on re-run, and the traces, tags and strips unchanged by the flags |
| `node tests\bridge\run.js` | ALL BRIDGE TESTS PASSED: 30 traces, 16892 ticks, 1066 frames, 1268038 blocks, 1024 tags (each also through `ApplyTag`, at its row and row + 4), 20 markup, 46 builder vectors (+6 parser-only), 29 API checks |
| `python -I tools\bridge\sim.py all` (150 runs each) | 4/4 PASS, see below |
| `python -I tools\bridge\crc_mc.py` (2 M) | 0 undetected, see below |
| `cargo test --offline` in `helper\bridge` | 62 passed, 0 failed (2 ignored: the release suites; 2026-10-19, with the reply and copy flags: every flag byte, reply rows, slot and target, the documented example) |
| `cargo test --offline --release -- --ignored` | 2 passed: corruption suite, per-call cost |
| `luacheck addon\TranslateProbe\Bridge.lua` | 0 warnings |

**Simulation** (`sim.py`: the addon's encoder and strip on a game-frame clock, the helper's
captures with jitter, drops, stalls, tearing, noise and colour curves; latency from click to
delivery):

| Scenario | Requests | Delivered | Within the limit | P50 / P99 / P99.9 |
|---|---|---|---|---|
| nominal (dense) | 6939 | 100% | 300 ms: 100% | 55 / 126 / 211 ms |
| stress20 (dense; hitch or stall windows apart) | 6228 | 100% | 300 ms: 100% | 60 / 135 / 177 ms |
| w1366 (dense) | 5780 | 100% | 300 ms: 99.965% | 57 / 170 / 228 ms |
| mono_mix (grey-preserving mixing + curves) | 6813 | 100% | 600 ms: 99.927% | 63 / 298 / 531 ms |

No undetected frame or message in any scenario. stress20 including the windows with a game hitch
or helper stall: 6578 requests, 100% delivered, 99.848% within 300 ms.

**CRC Monte Carlo** (`crc_mc.py 2000000 1`, 77 s): 666,667 dense, 666,667 robust and 666,666
mono frames corrupted (1,000,327 adjacent misreads, 599,043 splices, 400,630 bursts):
14,789 refused by the magic, 14,363 by the version, 15,078 by the length, 1,883,513 by the
CRC-32, 0 by the record parse; 72,257 decoded as a true frame; **0 undetected**.

**Rust suites** (`helper\bridge\tests`; the synthetic captures come from `src\synth.rs`, all
text is random glyph shapes):

| Suite | Result |
|---|---|
| `vectors`: every trace frame drawn as pixels and decoded | 1066 frames decoded byte-identical, 339 messages rebuilt exactly as the reference; all 1024 tags drawn and scanned back; 52 message vectors (with reply and copy) and 36 distorted strips agree with the reference |
| `distort`: decode rate per category, 4000 captures each (Python reference in brackets) | dense identity +-6 noise 100% (100%); dense envelope curves, gap >= 16, block noise 99.750% (99.725%); robust envelope + harsh curves 99.825% (99.825%); mono with colour mixing 99.900% (99.850%); dense 0.5 px offset 100% (100%); torn captures decoded 21.6 / 19.9 / 19.6% (20.6 / 18.3 / 17.3%) in dense / robust / mono; **0 wrong frames** anywhere; colour mixing: dense 0/300, mono 300/300 |
| `tags`: recall on 3000 synthetic chat panels (dark 25-100% over a random world, antialiased text in chat, class and item colours incl. (255,128,255) and (0,255,246), raid icons, blocks 4-12 px, gaps 0-2 px, alpha 0.3-1.0, offset +-2 px, top clipping, two frames) | 26601 / 26601 tags (100%), 0 wrong identity, 0 unexpected hits; every alpha bucket 100% |
| `tags`: tag-free panels (text, item links, raid icon runs, tooltips) | 0 hits over 10000 panels |
| `tags`: 1 M random digit mutations (uniform blocks) | 74081 drawn valid code words, all found with the drawn identity; 925919 invalid, all rejected; **0 misdecodes** |
| `tags`: 1 M random-colour substitutions (residual risk, not an acceptance item) | 855 decoded as another tag (8.55e-4 per substitution) |
| `cards`: covers over part of the plates (tooltips and random menus, 2000 cards) | mask IoU against the truth 0.9999 aggregate, worst card 0.9867 (hard cover edges); 0.9958 with soft edges; 0 covered plate pixels in the mask (hard edges) |
| `cards`: fade over coloured grounds (green, brown, grey, dark, bright, dark blue; textured, +-6 noise, half under a 30% black panel; alpha 0.3-1.0; 1-3 rows) | 3555 / 3555 rows with alpha >= 0.33 shown; where the plate end stands out by 12 or more (3154 rows): text end within 2 px on all but 11, mask IoU 0.9911; alpha >= 0.45 and step >= 16: 0 of 2394 off; text cut short: 1 of 3555; where the end does not stand out (401 rows), the text runs on by a median 37 px |
| `cards`: covers whose edge lies at the plate start, over random grounds and alphas (2706 covered rows) | 11 taken for a plate: tooltip 1/538, E2E-rig tooltip 5/558, near-black menu 0/522, random colour 0/535, dark-blue colour 5/553 (the design's rule: 184, 178, 7, 11, 238) |
| `cards`: gap fill, no-plate mode, grouping, session bit, fade-out, dropped rows | pass |
| `cards`: rows 1..6 (tag rows modulo 4): 1-6 rows with and without K, rows hidden in the middle (0,1,3,4,5 / 0,4,5 / 0,1,5 / 1,5 / 0,2,3,5), a copy 4 rows on for K = 4, the last rows of 5- and 6-row requests at the top of the view | pass |
| `cards`: tag cover over blended edges (fractional block edges and heights, 75 cases) | largest unpainted colour step 9.5 off the ground-to-plate line |
| `tone`: 3-row cards with fractional, antialiased edges through display curves (40 cards per curve) | rows shown, raw / curve undone: Gamma 2.2 60/120 / 120/120, Gamma 1.6 120/120 / 120/120, darkening 0.4545 0/120 / 120/120 |
| `tags`: a narrow scan area inside a wider capture, tags across its right edge (48 cases) | no panic, nothing read outside the area |
| `corrupt` (release, ignored): 200000 corrupted strips (99710 adjacent misreads, 60374 splices, 39916 bursts) | 176857 rejected, 23143 decoded as a drawn frame, **0 undetected** |

**Per-call cost** (`cargo test --offline --release --test perf -- --ignored --nocapture`, release
profile `opt-level = "s"` as shipped, best of 5 batches; a synthetic frame with the strip of a
typical TRANSLATE, a chat frame at the bottom left and two cards of 3 rows). "Before" is the code
at the start of the P2 completion run, built and timed the same way:

| Call | 1920x1080 before | after | 2560x1440 before | after |
|---|---|---|---|---|
| `decode_strip`, strip shown | 44.4 us | 11.0 us | 58.6 us | 15.3 us |
| `decode_strip`, strip absent | 0.21 us | 0.19 us | 0.21 us | 0.19 us |
| `tags::scan`, the VIEW area with 6 tags | 527 us | 268 us | 704 us | 359 us |
| `tags::scan`, same area without tags | 385 us | 109 us | 504 us | 148 us |
| `tags::scan`, discovery (whole client below row 16) | 7653 us | 3245 us | 13344 us | 5677 us |
| `cards::build`, 6 rows | 231 us | 237 us | 401 us | 354 us |

`cards::build` now also runs the plate prediction, the cover checks and the two-way mask (section
8); it costs about what the design's single-colour mask did. Where the time went: the scan reads
row bytes directly with a cheap pre-test, the pitch search avoids libm rounding calls and only
widens to the block width when it must, and the strip decoder stops at the frame's length. A
helper capturing at 30 Hz spends about 15 ms per second (1.5% of one core) on scan and build for
this frame at 1080p, 21 ms at 1440p.

## 10. Deviations from the design

Each is backed by a measured fact; the tests named reproduce it.

1. **Sync pair test** (tag scan step 2). The design's "first magenta-like run (R-G >= 40 and
   B-G >= 40)" misses faded tags (a magenta block at alpha 0.3 over a green ground is not
   magenta-like) and merges B0 with a magenta-like ground (`magenta_background_does_not_stretch_the_sync_block`).
   The scan uses the relation between B0 and B1 instead and bounds blocks by colour similarity.
2. **Block geometry** (step 4). Blocks of 4-12 px at fractional positions, with gaps of 0-2 px,
   are not read reliably at the centre of a run of equal digits from the sync pair's pitch alone;
   the pitch search, full-extent fit, alphabet fit, between-blocks and tag-end checks are what
   give 100% recall and 0 misdecodes on the suites below. Measured on 1 M digit mutations: with
   the block width fixed at the sync pair's measurement, 3 valid tags with hard-edged blocks of
   4.2-4.3 px were missed (the pair reads 5 + 4 px, so 4.5 px); the width now joins the fit when
   nothing decodes, and 0 are missed.
3. **Plate rule** (step 7). Over coloured grounds the faded plate does not keep B > G > R: on the
   fade suite the design's rule shows 2483 of 3478 rows with alpha >= 0.33 (green 301/540, brown
   196/603, bright 332/579) and the E2E rig dropped rows from alpha 0.5. The prediction from the
   tag's own references is added (the hue rule stays as an alternative). G - R >= 3 alpha is added
   to the hue rule: the default tooltip backdrop (23,23,48) has G = R.
4. **Cover checks** (new). With the design's rule, a tooltip, menu or dark-blue cover whose edge
   lies at the plate start is taken for a plate in 618 of 2706 covered rows of the cover suite
   (tooltips 184/538, E2E-rig tooltips 178/558); the edge, neighbour-row and propagation checks
   bring this to 11 of 2706.
5. **Mask and text end** (step 7). The design's mask (within a tolerance of one Pobs) leaks into a
   ground of nearly the plate's faded colour: the E2E rig measured text rectangles 4 to 96 px past
   the plate end at alpha 0.5-0.6. The two-way test against the ground behind the plate, the
   followed plate colour and the look-ahead end rule fix this where the end is visible (section
   9). The mask tolerance is also capped at `6 + 2 spread` (a solid plate captures exactly; the
   tooltip backdrop is only 16 from the plate).
6. **Strip decode** computes block colours only as far as the frame needs them (same results,
   about 4 times faster).
7. **The strip is never wider than the screen** (review fix). The design's
   `cols = max(physW / 2, MIN_COLS[mode])` drew mono strips 1152 px wide on any client narrower
   than that (robust below 768 px, dense below 384 px); the blocks past the client edge were never
   captured, so the Helper rejected those frames (TooLong) and the TRANSLATE never arrived.
   Measured with the real encoder at 1024 x 768 and a first click: mono used 576 columns and only
   the VIEW was delivered. MIN_COLS is now only a floor for the `maxCols` option, the width is
   capped at `physW / 2` (Lua and Python alike), and trace `j` (1024 x 768, three modes) was added
   to the vectors. 512 mono columns (220 B per chunk) still carry the 1554 B worst case in 8
   chunks.
8. **Tag cover** (review fix, amendment A2). The first `tag_rect` was the rounded tag only and the
   overlay painted it at the card alpha: with fractional block edges the columns x0 - 1 and x1 and
   the blended rows above and below stayed visible (colour steps up to 82), and a faded tag showed
   through the translucent cover (up to 55 per channel at alpha 0.5). `tag_rect` now covers the
   blended edges (section 8) and is painted opaque (`tag_rect_hides_the_blended_tag_edges`: 75
   sub-pixel cases, largest unpainted colour step 9.5 off the ground-to-plate line).
9. **B1 bounded by the scan area** (review fix). The B1 candidate was bounded by the image's right
   edge, not the area's: with one capture covering several VIEW rectangles and one scan per view,
   a sync pair straddling a narrow view's right edge sent `read_tag` past the end of its profile
   (index panic, which aborts the release Helper; 146 panics in 100 000 fuzzed scans). Both
   `scan` and `diagnose` now stop at the area's edge
   (`narrow_area_inside_a_wider_capture_never_panics`).
10. **Display curves undone before the scan** (new, section 8b). Fact: the E2E scenario gamma22
    (the rig's Gamma 2.2) lost 4 of 8 tags after the first scroll (blended edge pixels off the
    blend line: "between blocks" rejects) and showed no plate at all; with the exact inverse curve
    all 8 tags and all plates came back. On synthetic 3-row cards with fractional, antialiased
    edges (`tests\tone.rs`, 40 cards per curve), rows shown on the raw capture / with the fitted
    curve undone: Gamma 2.2 60/120 / 120/120, Gamma 1.6 120/120 / 120/120, the darkening 0.4545
    curve 0/120 / 120/120. `strip::Calib` gains `levels`; the new module `tone` is additive.

## 11. Known limits

- **Display curves** are undone with a power-law fit of the strip's levels (section 8b). A curve
  far from a power law (strong contrast with clipping, an S curve, HDR tone mapping) is only
  approximated; such a curve, and a curve applied before the strip was ever seen in the session,
  can still lose tags or plates. Dark levels of a strong darkening curve are coarsely quantized by
  the capture.
- **Covers of the plate's exact faded colour** that end within the card's own rows cannot be told
  from a plate by colour or extent (11 of 2706 covered rows in the suite, mostly dark-blue covers
  over dark-blue grounds).
- **Plate ends under alpha 0.45** over a textured ground can be found a few pixels off or carried
  into ground that looks like the plate (rates in section 9); the helper's 2 s sticky text width
  absorbs short-lived misreads.
- **Random colours** in data blocks (not digits): about 8.5e-4 per substitution decode as another
  tag (a colour near another digit's colour is that digit; CRC-5 misses 1 in 32 substitutions).
  The slot must also be known in the session for the helper to draw anything.

## 12. M0 decisions and deviations

**Rust API (`tp_bridge`)**
- `image::Rect` derives `Debug` as well as `Clone, Copy`. Reason: `Mask`, `RowBand` and `Card`
  derive `Debug` and contain a `Rect`, so the design's API does not compile without it.
- `cards::RowBand` has a fourth field, `tag_rect: image::Rect`, the area the overlay paints over
  to hide the tag once read (section 8, item 8).
- `strip::Calib` has a fourth field, `levels: [[f32; 3]; 4]`, the measured reference levels
  (section 8b).
- `reasm::ReasmStats` was referenced but not defined. It is a `Clone, Copy, Debug, Default,
  PartialEq, Eq` struct of `u64` counters: `frames`, `chunks`, `duplicates`, `conflicts`,
  `messages`, `bad_crc`, `parse_errors`, `expired`, `missed`, `session_changes`.
- `Reassembler::take_session_change()` returns the previous session id, once, after the session
  changed from one id to another (None on the first session), so the caller can call
  `engine.clear_session(old)`.
- `msg::parse_message` is strict: TRANSLATE needs slot 0..126, rows 1..6 (1..4 until
  2026-10-18), flags with only bits 0-3 (0-1 until 2026-10-19) where copy needs reply and reply
  needs plate, rows 2..6 and a language-code target, a target of 1-8 printable ASCII bytes,
  text <= 1024 B, <= 8 pairs with term <= 48 B,
  meaning <= 120 B and sum(2 + #term + #meaning) <= 512, valid UTF-8 everywhere. CANCEL accepts
  slot 0..127. VIEW accepts 0..4 views. Any other length is `Length`.
- `msg::Translate` has `reply` and `copy` (2026-10-19); `msg::{FLAG_NO_CACHE, FLAG_PLATE,
  FLAG_REPLY, FLAG_COPY, FLAGS_MASK, REPLY_MIN_ROWS}` are public.
- Extra public items beyond D.2 (additive): `strip::{decode_strip_detail, DecodeDetail, Reject,
  Mode::{ALL, magic, bits, name, index}, frame_capacity, max_chunk_data, parse_frame_bytes}` and
  the format constants, `tags::diagnose`, `cards::{Mask::get, Mask::count, StickyWidth}` and the
  tuning constants, `reasm::Reassembler::pending_missing`.
- Feature `synth` compiles `src\synth.rs` (synthetic captures for the tests) outside unit tests;
  the crate's own dev-dependency on itself turns it on for `tests\*.rs`.

**Lua (`Bridge.lua`)**
- Builders write integers as given, clamped to the field width (u8 0..255, u16 0..65535); they do
  not remap values into the valid range, so a wrong slot or row count is refused by the helper
  instead of addressing other rows. Flags are masked to bits 0-3 (0-1 until 2026-10-19) and the
  copy bit is dropped unless the reply bit is set.
- Window helpers (2026-10-19): `Bridge.FLAG_NOCACHE`, `FLAG_PLATE`, `FLAG_REPLY`, `FLAG_COPY` (1, 2,
  4, 8), `Bridge.FLAGS_MASK` (15), `Bridge.BUBBLE_RGB` ({36, 30, 24}) and
  `Bridge.ApplyTag(textures, slot, row, sessBit)`, which colours 8 Textures with the tag of
  (slot, row % 4, sessBit), opaque (`SetColorTexture(r / 255, g / 255, b / 255, 1)`, no table
  allocated). The protocol colours carry an `art-ok: protocol colour` comment.
- The TRANSLATE text is cut at 1024 bytes on a UTF-8 boundary; when a cut happens and the last
  0x02 mark is after the last 0x03, the text is cut before that 0x02 (same rule as
  `Glossary.PromptText`). Pairs that are not `{string, string}` are skipped; pairs stop at the
  first one that would pass 512 bytes.
- When the queue drains, the encoder composes no further frame: `Update` returns false,
  `IsIdle()` turns true, the strip hides, and the next queued message is composed on the next
  `Update` without waiting for HOLD (the previous frame was fully exposed). The Python reference
  mirrors this for byte identity.
- `Bridge.SetMode(name)`: an unknown name means `auto`. It does not write `ns.db.mode` (the
  caller does).
- `Bridge.Init()` falls back to 1920x1080 when `GetPhysicalScreenSize` is missing and to a
  random session when `ns.sessionId` is missing (tests only).
- Extra public helpers kept from bridge-sim for tests: `Bridge.Crc32`, `Bridge.FrameCapacity`,
  `Bridge.MaxChunkData`, `Bridge.LEVELS`, `Bridge.MODES`, `Bridge.KEY_R/G/B`, `enc:Fit(force)`.
  The strip frame has `frame.Refresh()`, `frame.held`, `frame.loading` and `frame.Draw`.
- Mode `auto` is resolved at `Init` and at `SetMode`, and again on `CVAR_UPDATE` for
  `colorblindSimulator` while the requested mode is `auto` (review fix: the filter switched in the
  game settings now takes effect at once).
