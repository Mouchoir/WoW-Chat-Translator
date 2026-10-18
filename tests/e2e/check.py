"""Compares the Helper's --diag-json output with fakewow's truth file (TranslateProbe E2E, package P5).

    python -I tests/e2e/check.py --truth T --diag D --report R [--captures 2] [--px 2]
                                 [--helper-cpu-pct X] [--done-within-s 5]
    python -I tests/e2e/check.py --synth-diag OUT --truth T [--shift-px N]
    python -I tests/e2e/check.py --selftest --truth T

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
        self.first_capture = self.all_captures[0] if self.all_captures else None
        self.views = [(e["t"], {v[0] for v in e.get("views", [])}) for e in self.msgs if e.get("kind") == "view"]
        self.views_t = [v[0] for v in self.views]

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
    if cpu_pct is not None:
        rep.add("cpu", cpu_pct <= CPU_MAX_PCT, helper_pct_one_core=cpu_pct, max=CPU_MAX_PCT,
                steady_pct_one_core=steady_cpu_pct(diag, cpu_pct))
    fw = (truth.end or {}).get("summary", {})
    if fw:
        rep.add("fakewow", bool(fw.get("ownForegroundTicks", 0) == 0), foreground_ticks=fw.get("ownForegroundTicks"),
                exposed_points=fw.get("exposedPoints"), place=fw.get("place"), late_frames=fw.get("lateFrames"))
    return rep


# ------------------------------------------------------------------------------- synthetic diag
def synth_diag(truth, shift=0, hz=30.0, process_ms=12.0):
    """A diag file a perfect Helper would write for this truth (shift moves every rectangle)."""
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
            cards.append({"slot": slot, "req": req, "view": view, "kind": kind, "rows": [i["row"] for i in its],
                          "bands": bands, "tags": tags, "gaps": [], "masks": masks, "alpha": 1.0, "plate": True,
                          "state": state, "covered": False})
        key = json.dumps(cards, sort_keys=True)
        if key != last_cards:
            out.append({"t": round(t + process_ms, 1), "ev": "cards", "hidden": None, "cards": cards})
            last_cards = key
        t += period
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
