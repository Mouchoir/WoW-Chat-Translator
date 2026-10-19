"""Compares the Helper's --diag-json output with fakewow's truth file (TranslateProbe E2E, package P5).

    python -I tests/e2e/check.py --truth T --diag D --report R [--captures 2] [--px 2]
                                 [--helper-cpu-pct X] [--done-within-s 5]
    python -I tests/e2e/check.py --synth-diag OUT --truth T [--shift-px N]
    python -I tests/e2e/check.py --selftest --truth T   (a conv truth also checks the window checks:
                                                         a perfect synthetic diag passes them, a
                                                         broken one fails window_fill)

Exit code 0 when every check passes, 1 otherwise (2 on bad input).

Checks (design section E, P6 step 2):
- latency: every TRANSLATE shown on the strip while the Helper ran is decoded (diag "msg") at
  P99 <= 300 ms after fakewow painted it;
- bands: once the truth has been stable for 2 captures, every plate that should be visible
  (alpha >= 0.45, at least 8 px of its tag inside the chat) has a card band within +-2 px of the
  truth (band top/bottom = tag, left = plate + 3, right = plate right unless a cover cuts it).
  Counted captures are the fresh ones that grabbed the scan area (diag capture.scan), and a plate
  is expected only once the Helper's VIEW covers its chat frame (or it has no VIEW and scans by
  discovery): the design grabs the scan area only for known VIEW rects or every 10th tick without
  one, and the addon's VIEW watcher runs at 2 Hz, so a literal "2 fresh captures" cannot hold for
  the first click of a session, a newly shown chat frame or a Helper started late. That wait is
  reported (view_wait_ms); --strict applies the literal rule;
- hidden: at those captures no band is shown for a plate that is gone (scrolled out, faded under
  0.2, Alt-Z, chat hidden, rows removed);
- masks: no mask pixel ever lies inside a covering rectangle (at settled captures);
- missing: a request the Helper never received (truth "dropped", or sent before the Helper's
  first capture) shows a "missing" card; after it is sent again the card becomes a request;
- done: every decoded request's card reaches done/same/nothing within --done-within-s seconds
  while its plate is visible;
- cpu: the Helper used at most 3% of one core (when --helper-cpu-pct is given). The report also
  gives steady_pct_one_core, the same figure without the first 2 s (informational, not judged).

Conversation window (scenario conv; only when the truth has `layers`, design B.4 E2E): the
window's visible rows are plates of VIEW 200 in the truth, so latency, bands, hidden and done
apply to them too, and:
- window_fill: every background sample of a window card (the colour the overlay paints the tags
  and plates with) is within 6 levels of BUBBLE_RGB, every window card has at least one sample
  (a card with none is painted with the navy plate colour: the seam this check is for), and tags
  were painted in VIEW 200;
- reply_status: the reply card (reply bit) reaches a final state with the clipboard outcome
  "off" ("not copied - off in the Helper menu": every E2E run uses --no-clipboard), and the
  Helper's clip records never say it copied;
- lane: the reply card's hint lane is found where the truth has it (+-2 px) with at least 60% of
  its columns passing, and painted (lanePx > 0);
- partial_rows: while a bubble is scrolled half out, its hidden rows never get a band and its
  visible rows do;
- no_duplicate: no capture shows two cards of one slot, and no window slot shows in a chat view;
- cpu_vs_i2: the CPU figures are within 10% of the highest of DESIGN.md I.2 (3.55% whole run,
  1.96% after the first 2 s).

Neither file holds chat text: only rectangles, ids, states and times (Unix epoch ms).
"""
import argparse
import bisect
import json
import math
import sys

LATENCY_P99_MS = 300.0
# A tag carries its row number modulo 4 (requests hold 1..6 rows since 2026-10-18), and the truth
# reports the tag's row field, while the Helper numbers a card's bands 0..5: rows are compared
# modulo 4, the y position tells the rows apart.
TAG_ROWS = 4
CPU_MAX_PCT = 3.0
FINISHED = {"done", "same", "nothing"}
# Conversation window (docs\BRIDGE.md section 8c)
BUBBLE_RGB = (36, 30, 24)
PLATE_RGB = (32, 40, 64)
REPLY_SLOT = 126
WINDOW_VIEW_MIN = 200
FILL_TOL = 6
LANE_SHARE = 0.6
# DESIGN.md I.2: the highest CPU figures of the 9 echo scenarios (whole run, after the first 2 s)
I2_CPU_WHOLE = 3.55
I2_CPU_STEADY = 1.96


def load_jsonl(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError as e:
                raise SystemExit(f"{path}:{n}: bad JSON ({e})")
    return out


def right(r):
    return r[0] + r[2]


def bottom(r):
    return r[1] + r[3]


def intersects(a, b):
    return a[0] < right(b) and b[0] < right(a) and a[1] < bottom(b) and b[1] < bottom(a)


def percentile(values, p):
    if not values:
        return None
    s = sorted(values)
    k = (len(s) - 1) * p
    lo, hi = math.floor(k), math.ceil(k)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


# ------------------------------------------------------------------------------------------ truth
def expectation(item, covers):
    """'band' (a card band must be there), 'none' (no band may be there) or 'any'."""
    tag = item.get("tag")
    if not item.get("visible") or tag is None or item.get("alpha", 0) <= 0.2:
        return "none"
    if any(intersects(tag, c) for c in covers):
        return "any"
    if item.get("alpha", 0) >= 0.45 and tag[3] >= 8 and item.get("plate") is not None:
        return "band"
    return "any"


class Truth:
    def __init__(self, events):
        self.header = next((e for e in events if e.get("ev") == "header"), None)
        if self.header is None:
            raise SystemExit("truth: no header")
        self.states = []  # (t, items, covers, ui)
        items, covers, ui = [], [], True
        for e in events:
            ev = e.get("ev")
            if ev == "plates":
                items = e["items"]
            elif ev == "covers":
                covers = e["rects"]
            elif ev == "ui":
                ui = e["shown"]
            else:
                continue
            self.states.append((e["t"], items, covers, ui))
        self.times = [s[0] for s in self.states]
        self.sent = [e for e in events if e.get("ev") == "sent"]
        self.dropped = [e for e in events if e.get("ev") == "dropped"]
        self.layers = [e for e in events if e.get("ev") == "layers"]
        self.end = next((e for e in events if e.get("ev") == "end"), None)
        # captures are compared with the truth only while fakewow was playing
        self.until = self.end["t"] if self.end else (self.states[-1][0] if self.states else math.inf)
        # times where what the Helper must show changes (alpha inside one class does not count)
        self.changes = []
        last = None
        for t, items, covers, ui in self.states:
            sig = (ui, tuple(map(tuple, covers)), tuple(sorted(
                (i["view"], i["slot"], i["row"], expectation(i, covers), tuple(i["tag"] or ()), tuple(i["plate"] or ()))
                for i in items)))
            if sig != last:
                self.changes.append(t)
                last = sig

    def at(self, t):
        k = bisect.bisect_right(self.times, t) - 1
        if k < 0:
            return [], [], True
        _, items, covers, ui = self.states[k]
        return items, covers, ui

    def last_change(self, t):
        k = bisect.bisect_right(self.changes, t) - 1
        return self.changes[k] if k >= 0 else -math.inf


# ------------------------------------------------------------------------------------------- diag
class Diag:
    def __init__(self, events):
        self.captures = [e["t"] for e in events if e.get("ev") == "capture" and e.get("fresh")]
        # fresh captures that also grabbed the tag scan area (design D.3 step 2: the VIEW rects, or
        # the discovery area every 10th tick while no VIEW is known)
        self.scan_caps = [e["t"] for e in events if e.get("ev") == "capture" and e.get("fresh") and e.get("scan", True)]
        self.all_captures = [e["t"] for e in events if e.get("ev") == "capture"]
        self.cards_ev = [e for e in events if e.get("ev") == "cards"]
        self.cards_t = [e["t"] for e in self.cards_ev]
        self.msgs = [e for e in events if e.get("ev") == "msg"]
        self.engine = [e for e in events if e.get("ev") == "engine"]
        self.perf = [e for e in events if e.get("ev") == "perf"]
        self.overlay = [e for e in events if e.get("ev") == "overlay"]
        self.clips = [e for e in events if e.get("ev") == "clip"]
        self.first_capture = self.all_captures[0] if self.all_captures else None
        self.views = [(e["t"], {v[0] for v in e.get("views", [])}) for e in self.msgs if e.get("kind") == "view"]
        self.views_t = [v[0] for v in self.views]
        # the full VIEW rectangles: (t, [[id, x, y, w, h, fontPx, rowPx], ...])
        self.view_rects = [(e["t"], e.get("views", [])) for e in self.msgs if e.get("kind") == "view" and e.get("views")]

    def next_capture(self, t):
        k = bisect.bisect_right(self.all_captures, t)
        return self.all_captures[k] if k < len(self.all_captures) else t + 100

    def view_since(self, view, t):
        """Since when the Helper's VIEW (sticky) covers chat frame `view` at time t: -inf when it has
        no VIEW at all (its discovery scan covers every frame), None when its VIEW leaves it out."""
        k = bisect.bisect_right(self.views_t, t) - 1
        if k < 0:
            return -math.inf
        if view not in self.views[k][1]:
            return None
        while k > 0 and view in self.views[k - 1][1]:
            k -= 1
        return self.views[k][0]

    def cards_before(self, t):
        k = bisect.bisect_left(self.cards_t, t) - 1
        if k < 0:
            return [], None
        e = self.cards_ev[k]
        return e.get("cards", []), e.get("hidden")


def shown_bands(cards, hidden):
    """(card, index) of every band that is drawn: not hidden, not covered."""
    if hidden:
        return []
    out = []
    for c in cards:
        if c.get("covered"):
            continue
        for i in range(len(c.get("bands", []))):
            out.append((c, i))
    return out


def find_item(items, view, slot, row, y):
    cands = [i for i in items if i["slot"] == slot and i["row"] % TAG_ROWS == row % TAG_ROWS and i.get("tag") is not None
             and (view == 0 or i["view"] == view)]
    if not cands:
        return None
    return min(cands, key=lambda i: abs(i["tag"][1] - y))


# ----------------------------------------------------------------------------------------- checks
class Report:
    def __init__(self):
        self.checks = {}

    def add(self, name, ok, **info):
        self.checks[name] = dict(pass_=ok, **info)

    def ok(self):
        return all(c["pass_"] for c in self.checks.values())

    def json(self):
        return {"pass": self.ok(), "checks": {k: {("pass" if a == "pass_" else a): b for a, b in v.items()}
                                              for k, v in self.checks.items()}}


def check_latency(truth, diag, rep):
    start = diag.first_capture
    lat, undecoded, skipped = [], [], 0
    used = set()
    for s in truth.sent:
        if s.get("kind") != "translate":
            continue
        if s.get("dropped") or start is None or s["t"] < start:
            skipped += 1
            continue
        m = next((k for k, m in enumerate(diag.msgs) if k not in used and m.get("kind") == "translate"
                  and m.get("req") == s["req"] and m["t"] >= s["t"] - 20), None)
        if m is None:
            undecoded.append(s["req"])
            continue
        used.add(m)
        lat.append(diag.msgs[m]["t"] - s["t"])
    p99 = percentile(lat, 0.99)
    ok = not undecoded and (p99 is None or p99 <= LATENCY_P99_MS) and (lat or skipped)
    rep.add("latency", bool(ok), decoded=len(lat), undecoded=undecoded, skipped=skipped,
            p50_ms=None if not lat else round(percentile(lat, 0.5), 1),
            p99_ms=None if p99 is None else round(p99, 1), max_ms=None if not lat else round(max(lat), 1))


def count_in(caps, a, b):
    """Captures in (a, b]."""
    return bisect.bisect_right(caps, b) - bisect.bisect_right(caps, a)


def check_frames(truth, diag, rep, captures, px, strict):
    # Default: count the captures that could see the plates (they grabbed the scan area), and expect
    # a plate's band only once the Helper's VIEW covers its chat frame. --strict counts every fresh
    # capture from the truth change, as the literal P6 wording does.
    caps = diag.captures if strict else diag.scan_caps
    evaluated, band_checks = 0, 0
    band_fail, hidden_fail, mask_fail, mask_px = [], [], [], 0
    view_wait, waiting = {}, {}  # view -> longest wait (ms) for a VIEW covering it; current wait start
    for c in caps:
        ch = truth.last_change(c)
        if count_in(caps, ch, c) < captures:
            continue
        nxt = diag.next_capture(c)
        if nxt > truth.until:
            break
        cards, hidden = diag.cards_before(nxt)
        items, covers, ui = truth.at(c)
        evaluated += 1
        bands = shown_bands(cards, hidden)
        # expected bands present and in place
        pending_views = set()
        for it in items:
            if expectation(it, covers) != "band":
                continue
            if not strict:
                since = diag.view_since(it["view"], c)
                if since is None:
                    pending_views.add(it["view"])
                    continue
                if count_in(caps, max(ch, since), c) < captures:
                    continue
            band_checks += 1
            match = [(cd, i) for cd, i in bands if cd["slot"] == it["slot"] and cd["rows"][i] % TAG_ROWS == it["row"] % TAG_ROWS
                     and (cd.get("view", 0) == 0 or cd["view"] == it["view"])
                     and abs(cd["tags"][i][1] - it["tag"][1]) <= 3 * px + 2]
            if not match:
                band_fail.append({"t": c, "slot": it["slot"], "row": it["row"], "view": it["view"], "why": "no band"})
                continue
            cd, i = match[0]
            b, tag, plate = cd["bands"][i], it["tag"], it["plate"]
            errs = {"top": b[1] - tag[1], "bottom": bottom(b) - bottom(tag), "left": b[0] - (plate[0] + 3)}
            cut = any(intersects(plate, cv) for cv in covers)
            r_err = right(b) - right(plate)
            bad = {k: v for k, v in errs.items() if abs(v) > px}
            if (not cut and abs(r_err) > px) or (cut and r_err > px):
                bad["right"] = r_err
            if bad:
                band_fail.append({"t": c, "slot": it["slot"], "row": it["row"], "view": it["view"], "off": bad})
        for v in pending_views:
            waiting.setdefault(v, c)
        for v in list(waiting):
            if v not in pending_views:
                view_wait[v] = max(view_wait.get(v, 0), round(c - waiting.pop(v)))
        # nothing shown where nothing should be
        for cd, i in bands:
            it = find_item(items, cd.get("view", 0), cd["slot"], cd["rows"][i], cd["tags"][i][1])
            if it is None or expectation(it, covers) == "none":
                hidden_fail.append({"t": c, "slot": cd["slot"], "row": cd["rows"][i], "view": cd.get("view"),
                                    "why": "no such plate" if it is None else "plate not visible"})
        # masks never over a cover
        for cd in cards:
            for m in cd.get("masks", []) or []:
                if not m:
                    continue
                for y, x0, x1 in m.get("runs", []):
                    for cv in covers:
                        if cv[1] <= y < bottom(cv) and x0 < right(cv) and cv[0] < x1:
                            n = min(x1, right(cv)) - max(x0, cv[0])
                            mask_px += n
                            if len(mask_fail) < 20:
                                mask_fail.append({"t": c, "slot": cd["slot"], "y": y, "px": n})
    for v, t0 in waiting.items():
        view_wait[v] = max(view_wait.get(v, 0), round(caps[-1] - t0) if caps else 0)
    rep.add("bands", evaluated > 0 and band_checks > 0 and not band_fail, captures_evaluated=evaluated,
            band_checks=band_checks, failures=len(band_fail), first_failures=band_fail[:12],
            counted_captures="fresh" if strict else "fresh with scan area",
            view_wait_ms={str(k): v for k, v in sorted(view_wait.items())})
    rep.add("hidden", evaluated > 0 and not hidden_fail, failures=len(hidden_fail), first_failures=hidden_fail[:12])
    rep.add("masks", evaluated > 0 and mask_px == 0, pixels_over_covers=mask_px, covers_seen=any(s[2] for s in truth.states),
            first_failures=mask_fail[:12])


def visible_slots_after(truth, t0, slot):
    """True when a plate of the slot is expected to be visible at some truth state after t0."""
    for t, items, covers, ui in truth.states:
        if t < t0:
            continue
        if any(i["slot"] == slot and expectation(i, covers) == "band" for i in items):
            return True
    k = bisect.bisect_right(truth.times, t0) - 1
    if k >= 0:
        _, items, covers, _ = truth.states[k]
        return any(i["slot"] == slot and expectation(i, covers) == "band" for i in items)
    return False


def check_missing(truth, diag, rep):
    start = diag.first_capture
    expected = {}  # slot -> (req, since)
    for d in truth.dropped:
        expected[d["slot"]] = (d["req"], d["t"])
    if start is not None:
        for s in truth.sent:
            if s.get("kind") == "translate" and s["t"] < start:
                expected[s["slot"]] = (s["req"], s["t"])
    results = []
    for slot, (req, since) in sorted(expected.items()):
        shown = any(c.get("kind") == "missing" and c["slot"] == slot for e in diag.cards_ev if e["t"] >= since
                    for c in e.get("cards", []))
        resend = next((s for s in truth.sent if s.get("kind") == "translate" and s["req"] == req and s["t"] > since
                       and not s.get("dropped") and (start is None or s["t"] >= start)), None)
        repaired = None
        if resend is not None:
            repaired = any(c.get("kind") == "request" and c["slot"] == slot and c.get("req") == req
                           for e in diag.cards_ev if e["t"] >= resend["t"] for c in e.get("cards", []))
        visible = visible_slots_after(truth, since, slot)
        results.append({"slot": slot, "req": req, "missingShown": shown, "plateVisible": visible,
                        "resent": resend is not None, "repaired": repaired})
    ok = all((r["missingShown"] or not r["plateVisible"]) and r["repaired"] is not False for r in results)
    rep.add("missing", ok, expected=len(results), results=results)


def check_done(truth, diag, rep, within_s):
    results = []
    for m in diag.msgs:
        if m.get("kind") != "translate":
            continue
        req, slot, t0 = m.get("req"), m.get("slot"), m["t"]
        if not visible_slots_after(truth, t0, slot):
            continue
        t_done = next((e["t"] for e in diag.cards_ev if e["t"] >= t0
                       for c in e.get("cards", []) if c["slot"] == slot and c.get("req") == req
                       and c.get("state") in FINISHED), None)
        end = truth.end["t"] if truth.end else math.inf
        if t_done is None and t0 + within_s * 1000 > end:
            continue  # the scenario ended first
        results.append({"req": req, "slot": slot, "ms": None if t_done is None else round(t_done - t0, 1)})
    late = [r for r in results if r["ms"] is None or r["ms"] > within_s * 1000]
    rep.add("done", not late, requests=len(results), late=late)


def steady_cpu_pct(diag, cpu_pct):
    """Informational: the whole-run figure without the first 2 s (one-time start-up, mostly the
    D3D11 device), split by the Helper's exact process cycle counts from its perf records."""
    perf = diag.perf
    if len(perf) < 4 or cpu_pct is None:
        return None
    cyc = sum(e.get("cycles", 0) for e in perf)
    wall = sum(e.get("wallMs", 0) for e in perf)
    cyc_late = sum(e.get("cycles", 0) for e in perf[2:])
    wall_late = sum(e.get("wallMs", 0) for e in perf[2:])
    if cyc <= 0 or wall_late <= 0:
        return None
    return round(cpu_pct * (cyc_late / cyc) * (wall / wall_late), 2)


def window_cards(diag):
    """(t, card) of every card shown in a window view (VIEW id >= 200)."""
    return [(e["t"], c) for e in diag.cards_ev for c in e.get("cards", []) if c.get("view", 0) >= WINDOW_VIEW_MIN]


def records_in_effect(diag, t0, t1):
    """The cards records whose time in effect (until the next record) meets [t0, t1)."""
    out = []
    for k, e in enumerate(diag.cards_ev):
        nxt = diag.cards_ev[k + 1]["t"] if k + 1 < len(diag.cards_ev) else math.inf
        if e["t"] < t1 and nxt > t0:
            out.append(e)
    return out


def window_view_holds(diag, t, tag):
    """True when a VIEW with a window view holding the tag rectangle was in use for the capture
    of the cards record at t: decoded before a capture that came before t (a capture plans its
    regions before it decodes the strip, so the tick that decodes a VIEW still scans with the
    previous one)."""
    rects = None
    for vt, vs in diag.view_rects:
        if vt <= t and diag.next_capture(vt) < t:
            rects = vs
    return any(v[0] >= WINDOW_VIEW_MIN and v[1] <= tag[0] < v[1] + v[3] and v[2] <= tag[1] < v[2] + v[4] for v in rects or [])


def layer_meta(e):
    """The meta data of the window layer(s) of a truth `layers` event."""
    return [it.get("meta") or {} for it in e.get("items", [])]


def check_window(truth, diag, rep, cpu_pct):
    wcards = window_cards(diag)
    # fill: the samples the overlay paints the window's tags and plates with; a card with no
    # sample at all is painted with the plate colour, so it fails too
    samples, off, unsampled = 0, [], []
    for t, c in wcards:
        mine = 0
        for i, pair in enumerate(c.get("bg") or []):
            for s in pair or []:
                if s is None:
                    continue
                samples += 1
                mine += 1
                d = max(abs(s[k] - BUBBLE_RGB[k]) for k in range(3))
                if d > FILL_TOL and len(off) < 12:
                    off.append({"t": t, "slot": c["slot"], "band": i, "rgb": s, "levels": d})
        if mine == 0 and len(unsampled) < 12:
            unsampled.append({"t": t, "slot": c["slot"]})
    tag_px = max((o.get("tagPx", 0) for o in diag.overlay if o.get("view", 0) >= WINDOW_VIEW_MIN), default=0)
    rep.add("window_fill", samples > 0 and not off and not unsampled and tag_px > 0, samples=samples, off=len(off),
            first_off=off, unsampled_cards=unsampled, tag_px=tag_px, bubble_rgb=list(BUBBLE_RGB), tolerance=FILL_TOL)
    # reply status: final, not copied (off), and never copied
    replies = [(t, c) for t, c in wcards if c.get("reply")]
    final = [(t, c) for t, c in replies if c.get("state") in FINISHED]
    clips = sorted({c.get("clip") for _, c in final if c.get("clip")})
    copied = [e for e in diag.clips if str(e.get("outcome", "")).startswith("copied")]
    finish = [e.get("outcome") for e in diag.clips if e.get("what") == "finish"]
    rep.add("reply_status", bool(final) and clips == ["off"] and not copied, reply_cards=len(replies),
            final_states=len(final), clip_outcomes=clips, clip_records=len(diag.clips), finish=finish,
            first_final_ms=None if not final else round(final[0][0] - replies[0][0], 1))
    # lane: where the truth has it, mostly passing, painted
    want = None
    for e in truth.layers:
        for m in layer_meta(e):
            if m.get("lane"):
                want = m["lane"]
    lanes = [c["lane"] for _, c in replies if c.get("lane")]
    good = [l for l in lanes if l["cols"] > 0 and l["pass"] >= LANE_SHARE * l["cols"]
            and (want is None or abs(l["rect"][1] - want[1]) <= 2)]
    lane_px = max((o.get("lanePx", 0) for o in diag.overlay if o.get("view", 0) >= WINDOW_VIEW_MIN), default=0)
    rep.add("lane", want is not None and bool(good) and lane_px > 0, truth_lane=want, found=len(lanes), good=len(good),
            example=good[-1] if good else (lanes[-1] if lanes else None), lane_px=lane_px)
    # rows of a bubble scrolled half out: hidden ones never get a band, visible ones do
    periods = []
    for k, e in enumerate(truth.layers):
        hidden = [h for m in layer_meta(e) for h in (m.get("hidden") or [])]
        if hidden:
            end = truth.layers[k + 1]["t"] if k + 1 < len(truth.layers) else truth.until
            periods.append((e["t"] + 300, end, hidden))
    bad, seen_rows = [], set()
    for t0, t1, hidden in periods:
        hslots = {h[0] for h in hidden}
        for e in records_in_effect(diag, t0, t1):
            for c in e.get("cards", []):
                if c.get("view", 0) < WINDOW_VIEW_MIN or c["slot"] not in hslots:
                    continue
                for r in c.get("rows", []):
                    if [c["slot"], r] in hidden:
                        bad.append({"t": e["t"], "slot": c["slot"], "row": r})
                    else:
                        seen_rows.add((c["slot"], r))
    rep.add("partial_rows", bool(periods) and not bad and bool(seen_rows), periods=len(periods),
            hidden=periods[0][2] if periods else None, bands_on_hidden_rows=len(bad), first_bad=bad[:6],
            visible_rows_shown=sorted(seen_rows))
    # each tag kept once: one card per slot, window slots only in window views. A window's new rows
    # can be seen before the VIEW that covers them is decoded (both are sent together, the strip
    # takes 100-400 ms): until then they belong to the chat view under them, which is reported
    # (strip_delay) but not a failure.
    wslots = {c["slot"] for _, c in wcards}
    dup, delayed = [], []
    for e in diag.cards_ev:
        slots = [c["slot"] for c in e.get("cards", [])]
        twice = sorted({s for s in slots if slots.count(s) > 1})
        stray = [c for c in e.get("cards", []) if c.get("view", 0) < WINDOW_VIEW_MIN and c["slot"] in wslots]
        known = sorted({c["slot"] for c in stray if c.get("tags") and window_view_holds(diag, e["t"], c["tags"][0])})
        if stray and not known and len(delayed) < 12:
            delayed.append({"t": e["t"], "slots": sorted({c["slot"] for c in stray})})
        if (twice or known) and len(dup) < 12:
            dup.append({"t": e["t"], "twice": twice, "in_chat_view": known})
    rep.add("no_duplicate", bool(wcards) and not dup, cards_records=len(diag.cards_ev), failures=dup, strip_delay=delayed)
    if cpu_pct is not None:
        steady = steady_cpu_pct(diag, cpu_pct)
        ok = cpu_pct <= 1.1 * I2_CPU_WHOLE and (steady is None or steady <= 1.1 * I2_CPU_STEADY)
        rep.add("cpu_vs_i2", ok, helper_pct_one_core=cpu_pct, steady_pct_one_core=steady,
                max_whole=round(1.1 * I2_CPU_WHOLE, 2), max_steady=round(1.1 * I2_CPU_STEADY, 2))


def run_checks(truth, diag, captures, px, cpu_pct, done_within_s, strict=False):
    rep = Report()
    if not diag.all_captures:
        rep.add("captures", False, why="the diag file has no capture event (Helper never found fakewow?)")
    else:
        rep.add("captures", bool(diag.captures), total=len(diag.all_captures), fresh=len(diag.captures))
    check_latency(truth, diag, rep)
    check_frames(truth, diag, rep, captures, px, strict)
    check_missing(truth, diag, rep)
    check_done(truth, diag, rep, done_within_s)
    if any(e.get("items") for e in truth.layers):
        check_window(truth, diag, rep, cpu_pct)
    if cpu_pct is not None:
        rep.add("cpu", cpu_pct <= CPU_MAX_PCT, helper_pct_one_core=cpu_pct, max=CPU_MAX_PCT,
                steady_pct_one_core=steady_cpu_pct(diag, cpu_pct))
    fw = (truth.end or {}).get("summary", {})
    if fw:
        rep.add("fakewow", bool(fw.get("ownForegroundTicks", 0) == 0), foreground_ticks=fw.get("ownForegroundTicks"),
                exposed_points=fw.get("exposedPoints"), place=fw.get("place"), late_frames=fw.get("lateFrames"))
    return rep


# ------------------------------------------------------------------------------- synthetic diag
def synth_diag(truth, shift=0, hz=30.0, process_ms=12.0, window_broken=False):
    """A diag file a perfect Helper would write for this truth (shift moves every rectangle).
    Window cards (VIEW id >= 200) also carry what the window checks read: background samples in
    BUBBLE_RGB, the reply bit and clipboard outcome "off" on the reply slot, the truth's lane, and
    overlay and clip records. window_broken: the window cards' samples are the plate colour or
    missing (window_fill must fail)."""
    h = truth.header
    start = h["playStart"] + h.get("helperStartMs", 0)
    end = truth.end["t"] if truth.end else (truth.states[-1][0] if truth.states else start) + 1000
    if truth.states:
        start = max(start, truth.states[0][0])
    out = []
    period = 1000.0 / hz
    known = {}  # slot -> req, from decoded messages
    pending = sorted([s for s in truth.sent if s.get("kind") == "translate" and not s.get("dropped")
                      and s["t"] >= start], key=lambda s: s["t"])
    msg_times = [(s["t"] + 90.0, s) for s in pending]
    first_seen = {}
    last_cards = None
    t = start
    k = 0
    lag = period * 0.9  # the picture a capture sees is almost one capture old
    lane_want = None
    for e in truth.layers:
        for m in layer_meta(e):
            if m.get("lane"):
                lane_want = m["lane"]
    window_seen = False
    while t <= end:
        out.append({"t": round(t, 1), "ev": "capture", "fresh": True, "method": "wgc"})
        while k < len(msg_times) and msg_times[k][0] <= t:
            s = msg_times[k][1]
            out.append({"t": round(msg_times[k][0], 1), "ev": "msg", "kind": "translate", "req": s["req"],
                        "slot": s["slot"], "rows": s["rows"]})
            known[s["slot"]] = s["req"]
            k += 1
        items, covers, ui = truth.at(t - lag)
        groups = {}
        for it in items:
            if expectation(it, covers) != "band":
                continue
            groups.setdefault((it["view"], it["slot"]), []).append(it)
        cards = []
        for (view, slot), its in sorted(groups.items()):
            its.sort(key=lambda i: i["tag"][1])
            first_seen.setdefault(slot, t)
            req = known.get(slot)
            if req is not None:
                kind, state = "request", "done"
            else:
                kind = "missing" if t - first_seen[slot] >= 1500 else "pending"
                state = kind
            bands, tags, masks = [], [], []
            for it in its:
                tag, plate = it["tag"], it["plate"]
                x = plate[0] + 3
                cuts = sorted(cv[0] for cv in covers if intersects(plate, cv) and cv[0] > x)
                rgt = min([right(plate)] + cuts)
                bands.append([x + shift, tag[1] + shift, rgt - x, tag[3]])
                tags.append([tag[0] + shift, tag[1] + shift, tag[2], tag[3]])
                runs = []
                for y in range(plate[1], bottom(plate)):
                    segs = [(plate[0] + 1, right(plate))]
                    for cv in covers:
                        if cv[1] <= y < bottom(cv):
                            nxt = []
                            for a, b in segs:
                                if b <= cv[0] or a >= right(cv):
                                    nxt.append((a, b))
                                else:
                                    if a < cv[0]:
                                        nxt.append((a, cv[0]))
                                    if b > right(cv):
                                        nxt.append((right(cv), b))
                            segs = nxt
                    runs += [[y + shift, a + shift, b + shift] for a, b in segs if b > a]
                masks.append({"rect": [plate[0] + 1 + shift, plate[1] + shift, plate[2] - 1, plate[3]], "runs": runs})
            card = {"slot": slot, "req": req, "view": view, "kind": kind, "rows": [i["row"] for i in its],
                    "bands": bands, "tags": tags, "gaps": [], "masks": masks, "alpha": 1.0, "plate": True,
                    "state": state, "covered": False}
            if view >= WINDOW_VIEW_MIN:
                window_seen = True
                fill = list(PLATE_RGB) if window_broken else list(BUBBLE_RGB)
                card["bg"] = [[fill, None if window_broken else fill] for _ in bands]
                if window_broken and len(cards) % 2 == 0:
                    card["bg"] = [[None, None] for _ in bands]
                card["reply"] = slot == REPLY_SLOT
                if slot == REPLY_SLOT:
                    card["clip"] = "off" if state in FINISHED else None
                    if lane_want is not None:
                        lw = [lane_want[0] + shift, lane_want[1] + shift, lane_want[2], lane_want[3]]
                        card["lane"] = {"rect": lw, "rgb": list(BUBBLE_RGB), "cols": lane_want[2], "pass": lane_want[2]}
            cards.append(card)
        key = json.dumps(cards, sort_keys=True)
        if key != last_cards:
            out.append({"t": round(t + process_ms, 1), "ev": "cards", "hidden": None, "cards": cards})
            last_cards = key
        t += period
    if window_seen:
        out.append({"t": round(end, 1), "ev": "overlay", "view": WINDOW_VIEW_MIN, "tagPx": 1, "lanePx": 1 if lane_want else 0})
        out.append({"t": round(end, 1), "ev": "clip", "what": "finish", "outcome": "off"})
    out.sort(key=lambda e: e["t"])
    return out


def write_jsonl(path, events):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        for e in events:
            f.write(json.dumps(e, separators=(",", ":")) + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--truth", required=True)
    ap.add_argument("--diag")
    ap.add_argument("--report")
    ap.add_argument("--captures", type=int, default=2)
    ap.add_argument("--px", type=int, default=2)
    ap.add_argument("--helper-cpu-pct", type=float)
    ap.add_argument("--done-within-s", type=float, default=5.0)
    ap.add_argument("--strict", action="store_true",
                    help="count every fresh capture from the truth change, even before the Helper could scan the plate")
    ap.add_argument("--synth-diag", metavar="OUT")
    ap.add_argument("--shift-px", type=int, default=0)
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    truth = Truth(load_jsonl(a.truth))
    if a.synth_diag:
        write_jsonl(a.synth_diag, synth_diag(truth, a.shift_px))
        return 0
    if a.selftest:
        perfect = run_checks(truth, Diag(synth_diag(truth, 0)), a.captures, a.px, None, a.done_within_s)
        shifted = run_checks(truth, Diag(synth_diag(truth, 5)), a.captures, a.px, None, a.done_within_s)
        result = {"perfect": perfect.json(), "shifted5": {"pass": shifted.ok(), "failed_checks": [
            k for k, v in shifted.checks.items() if not v["pass_"]]}}
        ok = perfect.ok() and not shifted.ok()
        if any(e.get("items") for e in truth.layers):
            broken = run_checks(truth, Diag(synth_diag(truth, 0, window_broken=True)), a.captures, a.px, None,
                                a.done_within_s)
            failed = [k for k, v in broken.checks.items() if not v["pass_"]]
            result["window_broken"] = {"pass": broken.ok(), "failed_checks": failed}
            ok = ok and "window_fill" in failed
        result["selftest"] = "pass" if ok else "fail"
        print(json.dumps(result, indent=1))
        return 0 if ok else 1
    if not a.diag:
        ap.error("--diag is required (or --synth-diag / --selftest)")
    rep = run_checks(truth, Diag(load_jsonl(a.diag)), a.captures, a.px, a.helper_cpu_pct, a.done_within_s, a.strict)
    doc = rep.json()
    doc["scenario"] = truth.header.get("scenario")
    if a.report:
        with open(a.report, "w", encoding="utf-8", newline="\n") as f:
            json.dump(doc, f, indent=1)
    summary = {k: v["pass"] for k, v in doc["checks"].items()}
    print(json.dumps({"scenario": doc["scenario"], "pass": doc["pass"], "checks": summary}))
    return 0 if doc["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
