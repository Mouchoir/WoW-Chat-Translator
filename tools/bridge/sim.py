"""Offline end-to-end simulation of the pixel bridge v2 (addon strip -> helper decoder).

Phase 1 runs the addon side (proto.Encoder inside proto.Strip: shown only while a message is
queued, OnUpdate only while visible) on a game-frame clock and records what the strip shows,
from when. Phase 2 runs the helper's capture clock against that timeline: renders each capture
(transfer curve, colour mixing, noise, tearing, background), decodes it, reassembles messages and
checks everything against the truth.

Usage:
  python -I tools/bridge/sim.py <scenario> [runs] [seed]   scenarios: nominal stress20 w1366 mono_mix
  python -I tools/bridge/sim.py all                         the four scenarios with their checks
  python -I tools/bridge/sim.py rates [n]                   decode rates per distortion category
Results go to stdout and to tools/bridge/out/result_<scenario>.json (gitignored). The workers
run at below-normal priority, 3 processes (a game may be running on this PC)."""
import os
import sys
import json
import time
import bisect
from multiprocessing import Pool

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import proto as P  # noqa: E402
from curves import random_curve, random_mix, min_gap  # noqa: E402

np = P._numpy()
WORKERS = 3


def low_priority():
    """Below-normal priority on Windows (inherited by the pool workers)."""
    if os.name == "nt":
        try:
            import ctypes
            k = ctypes.windll.kernel32
            k.SetPriorityClass(k.GetCurrentProcess(), 0x4000)   # BELOW_NORMAL_PRIORITY_CLASS
        except (OSError, AttributeError):
            pass


# ---------------------------------------------------------------- payloads (synthetic)
LATIN = "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJ eeaaoo,.!?'"
CJK = [chr(c) for c in range(0x4E00, 0x4E00 + 2000)]
SUPP = [chr(c) for c in range(0x20000, 0x20000 + 500)]     # 4-byte UTF-8 characters
TARGETS = ["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"]


def rand_text(rng, kind):
    if kind == "short":
        n = int(rng.integers(5, 121))
        t = "".join(rng.choice(list(LATIN), n))
        if rng.random() < 0.3:
            t = t[:n // 2] + "\x02[Linen Cloth]\x03" + t[n // 2:]
        return t
    if kind == "long":
        return "".join(rng.choice(list(LATIN), int(rng.integers(120, 256))))
    if kind == "cjk":
        return "".join(rng.choice(CJK, int(rng.integers(40, 256))))
    return "".join(rng.choice(SUPP, 300))                    # 'max': cut to 1024 bytes


def rand_pairs(rng, kind):
    n = {"short": rng.integers(0, 4), "long": rng.integers(0, 6), "cjk": rng.integers(0, 9), "max": 8}[kind]
    out = []
    for _ in range(int(n)):
        if kind == "max":
            out.append(("t" * 48, "m" * 120))
        else:
            out.append(("".join(rng.choice(list(LATIN[:26]), int(rng.integers(2, 13)))),
                        "".join(rng.choice(list(LATIN), int(rng.integers(8, 47))))))
    return out


KINDS = ["short", "long", "cjk", "max"]
KIND_P = [0.55, 0.25, 0.15, 0.05]


def rand_view(rng):
    n = int(rng.integers(1, 3))
    return [{"id": i + 1, "x": 8 + 900 * i, "y": 700, "w": 620, "h": 320, "fontPx": 14, "rowPx": 16} for i in range(n)]


# ---------------------------------------------------------------- scenarios
BASE = dict(duration=60000.0, width=2560, height=1440, mode="auto", max_cols=None,
            fps=[30, 60, 144, 240], refresh=[60, 144], hitch_rate=0.05,
            cap_hz=[20, 30], cap_jitter=5.0, cap_dur=(4.0, 6.0), drop_p=0.0, stall_p=0.0,
            curve={"identity": 1.0}, mix=False, noise=6, noise_kind="pixel", tear_cols=0.3,
            click_mean=1500.0, burst_p=0.1, view_p=0.05, cancel_p=0.03, hide_p=0.5, gap_auto=16.0)

SCEN = {
    "nominal": dict(),
    "stress20": dict(curve={"identity": 0.3, "envelope": 0.7}, noise_kind="block", drop_p=0.02, stall_p=0.002,
                     hitch_rate=0.2, cap_hz=[20]),
    "w1366": dict(width=1366, height=768, curve={"identity": 0.3, "envelope": 0.7}, noise_kind="block",
                  drop_p=0.02, stall_p=0.002, hitch_rate=0.2, cap_hz=[20, 30]),
    # colourblind filter on: auto picks mono; random grey-preserving mixing plus the colour curves
    "mono_mix": dict(mode=2, mix=True, curve={"identity": 0.3, "envelope": 0.7}, noise_kind="block", drop_p=0.02,
                     stall_p=0.002, hitch_rate=0.2, cap_hz=[20, 30]),
}


def pick(rng, d):
    keys = list(d)
    return keys[int(rng.choice(len(keys), p=np.array([d[k] for k in keys]) / sum(d.values())))]


def run(args):
    scen, seed = args
    low_priority()
    cfg = dict(BASE)
    cfg.update(SCEN[scen])
    rng = np.random.default_rng(seed)
    W, H, T = cfg["width"], cfg["height"], cfg["duration"]
    fps = float(rng.choice(cfg["fps"]))
    refresh = float(rng.choice(cfg["refresh"]))
    cap_hz = float(rng.choice(cfg["cap_hz"]))
    ckind = pick(rng, cfg["curve"])
    curve = random_curve(rng, ckind)
    mix = random_mix(rng) if cfg["mix"] else None
    gap = min_gap(curve)
    mode = cfg["mode"]
    if mode == "auto":   # the helper's weak-signal hint makes the user pick robust below 16
        mode = P.MODE_DENSE if gap >= cfg["gap_auto"] else P.MODE_ROBUST
    session = int(rng.integers(1, 65536))
    enc = P.Encoder(session, W, H, mode=mode, max_cols=cfg["max_cols"], addon_build=1)
    strip = P.Strip(enc)

    hides = []
    if rng.random() < cfg["hide_p"]:
        h0 = rng.uniform(5000, T - 10000)
        hides.append((h0, h0 + rng.uniform(1000, 4000)))
    clicks = []
    t = 200.0
    while t < T - 3000:
        clicks.append(t)
        if rng.random() < cfg["burst_p"]:
            clicks += [t + rng.uniform(20, 300) for _ in range(2)]
        t += rng.exponential(cfg["click_mean"])
    # no [T] click while the UI is hidden (Alt-Z), nor in the 300 ms before it
    clicks = sorted(x for x in clicks if not any(a - 300 <= x < b for a, b in hides))

    # phase 1: addon on the game clock
    requests = {}            # msgId -> dict(t, kind, bytes)
    timeline = []            # (visible_from, frame index or -1 when nothing is shown)
    frames = []
    ci = 0
    t = 0.0
    last_vis = 0.0
    shown_idx = -1
    hitches = []
    req = 0
    while t < T:
        dt = 1000.0 / fps * rng.uniform(0.75, 1.25)
        if rng.random() < cfg["hitch_rate"] * dt / 1000.0:
            dt = rng.uniform(80, 250)
            hitches.append((t, t + dt))
        strip.set_parent(not any(a <= t < b for a, b in hides))
        while ci < len(clicks) and clicks[ci] <= t:
            kind = KINDS[int(rng.choice(4, p=KIND_P))]
            if rng.random() < cfg["view_p"]:
                strip.send(P.msg_view(rand_view(rng)))          # sent before the TRANSLATE
            req = req % 65535 + 1
            m = P.msg_translate(req, int(rng.integers(0, 127)), int(rng.integers(1, 5)), 2,
                                str(rng.choice(TARGETS)), rand_text(rng, kind), rand_pairs(rng, kind))
            mid = strip.send(m)
            requests[mid] = {"t": clicks[ci], "kind": kind, "bytes": m}
            if rng.random() < cfg["cancel_p"]:
                strip.send(P.msg_cancel(req, 3))
            ci += 1
        did = strip.tick((t / 1000.0) * 1000.0)
        if did:
            frames.append({"bytes": enc.frame, "syms": list(enc.symbols)})
        idx = len(frames) - 1 if strip.visible() else -1
        if did or idx != shown_idx:
            # on screen from the next display refresh after this game frame
            vis = max(last_vis, t + dt + rng.uniform(0, 1000.0 / refresh))
            timeline.append((vis, idx))
            last_vis = vis
            shown_idx = idx
        t += dt
    tl_t = [x[0] for x in timeline]
    truth = {f["bytes"]: i for i, f in enumerate(frames)}

    # phase 2: helper on the capture clock
    period = 1000.0 / cap_hz
    curve_f = curve.astype(np.float32)
    cache = {}

    def drawn(fi):
        if fi in cache:
            return cache[fi]
        if len(cache) > 8:
            cache.clear()
        f = frames[fi]
        keys = P.block_keys(f["syms"], mode)
        blocks = P.render_blocks(keys)
        cache[fi] = (blocks, 2 * -(-len(blocks) // 2))
        return cache[fi]

    def content_at(x):
        k = bisect.bisect_right(tl_t, x) - 1
        return timeline[k][1] if k >= 0 else -1

    reasm = P.Reassembler()
    stalls = []
    stats = {"captures": 0, "ok": 0, "absent": 0, "torn": 0, "crc": 0, "nomagic": 0, "toolong": 0, "badver": 0,
             "malformed": 0, "undetected_frame": 0, "undetected_msg": 0, "torn_true": 0, "dropped": 0,
             "absent_while_shown": 0, "gap": gap, "mode": mode, "curve": ckind, "fps": fps, "cap_hz": cap_hz,
             "frames": len(frames)}
    delivered = {}
    j = 0
    while True:
        c = j * period + rng.uniform(-cfg["cap_jitter"], cfg["cap_jitter"]) + 50.0
        j += 1
        if c > T + 2000:
            break
        if rng.random() < cfg["stall_p"]:
            sl = rng.uniform(100, 250)
            stalls.append((c, c + sl))
            j += int(sl / period)
            continue
        if rng.random() < cfg["drop_p"]:
            stats["dropped"] += 1
            continue
        d = rng.uniform(*cfg["cap_dur"])
        a, b = content_at(c), content_at(c + d)
        stats["captures"] += 1
        base = rng.integers(0, 256, 3)
        bg = np.clip(base[None, None, :] + rng.integers(-40, 41, (4, W, 3)), 0, 255).astype(np.uint8)

        def compose(fi):
            if fi < 0:
                return bg.copy()
            blocks, _ = drawn(fi)
            return P.blocks_to_image(blocks, W, bg)

        pa = compose(a)
        if a != b:
            stats["torn_true"] += 1
            pb = compose(b)
            if rng.random() < cfg["tear_cols"]:
                x = int(rng.integers(0, W))
                pa[:, x:] = pb[:, x:]
                if 0 < x < W and rng.random() < 0.5:
                    pa[:, x - 1] = ((pa[:, x - 1].astype(int) + pb[:, x - 1]) // 2).astype(np.uint8)
            else:
                r = int(rng.integers(1, 4))
                pa[r:] = pb[r:]
                if rng.random() < 0.5:
                    pa[r - 1] = ((pa[r - 1].astype(int) + pb[r - 1]) // 2).astype(np.uint8)
        img = pa.astype(np.float32)
        if mix is not None:
            img = np.einsum("ij,hwj->hwi", mix, img)
        iv = np.clip(np.rint(img), 0, 255).astype(np.int64)
        cap = np.stack([curve_f[ch][iv[:, :, ch]] for ch in range(3)], axis=2)
        nz = cfg["noise"]
        if cfg["noise_kind"] == "pixel":
            cap += rng.integers(-nz, nz + 1, cap.shape)
        else:
            blk = rng.integers(-nz, nz + 1, (2, W // 2, 3))
            cap += np.repeat(np.repeat(blk, 2, axis=0), 2, axis=1)
        cap = np.clip(np.rint(cap), 0, 255).astype(np.uint8)
        st, fb, info = P.decode_capture(cap)
        done_t = c + d + 1.0
        if st != "ok":
            stats[st] = stats.get(st, 0) + 1
            if st == "absent" and a >= 0 and a == b:
                stats["absent_while_shown"] += 1
            continue
        stats["ok"] += 1
        if fb not in truth:
            stats["undetected_frame"] += 1
            continue
        try:
            hdr, chunks = P.parse_frame(fb)
        except ValueError:
            stats["malformed"] += 1
            continue
        for mid, msg in reasm.feed(hdr, chunks, done_t):
            P.parse_message(msg)
            if mid in requests:
                if requests[mid]["bytes"] != msg:
                    stats["undetected_msg"] += 1
                    continue
                delivered.setdefault(mid, done_t)
    lat = []
    for mid, rq in requests.items():
        l = (delivered[mid] - rq["t"]) if mid in delivered else None
        end = rq["t"] + (l if l is not None else 5000)
        # a game hitch or a helper capture stall inside the request's window is reported apart
        kind = rq["kind"] + ("~hitch" if any(x < end and y > rq["t"] for x, y in hitches + stalls) else "")
        lat.append((kind, l))
    stats["lat"] = lat
    stats["reasm"] = dict(reasm.stats)
    return stats


def pct(a, q):
    return float(np.percentile(a, q)) if len(a) else float("nan")


def block(v):
    got = np.array([l for l in v if l is not None])
    n = len(v)
    return {"n": n, "delivered_pct": round(100.0 * len(got) / n, 4) if n else None,
            "le300_pct": round(100.0 * float((got <= 300).sum()) / n, 4) if n else None,
            "le600_pct": round(100.0 * float((got <= 600).sum()) / n, 4) if n else None,
            "p50": round(pct(got, 50), 1), "p90": round(pct(got, 90), 1), "p99": round(pct(got, 99), 1),
            "p999": round(pct(got, 99.9), 1), "max": round(float(got.max()) if len(got) else -1, 1)}


def summarize(scen, results, wall):
    keys = ["captures", "ok", "absent", "torn", "crc", "nomagic", "toolong", "badver", "malformed",
            "undetected_frame", "undetected_msg", "torn_true", "dropped", "absent_while_shown", "frames"]
    agg = {k: int(sum(r.get(k, 0) for r in results)) for k in keys}
    out = {"scenario": scen, "runs": len(results), "wall_s": round(wall, 1), "counts": agg}
    out["modes"] = {P.MODE_NAMES[m]: sum(1 for r in results if r["mode"] == m) for m in P.MODES}
    lat = [x for r in results for x in r["lat"]]
    per = {}
    for kind in KINDS:
        v = [l for k, l in lat if k == kind]
        if v:
            per[kind] = block(v)
    per["all"] = block([l for k, l in lat if not k.endswith("~hitch")])
    hv = [l for k, l in lat if k.endswith("~hitch")]
    if hv:
        per["hitch_or_stall_in_window"] = block(hv)
    per["everything"] = block([l for k, l in lat])
    out["request_latency_ms"] = per
    for m in P.MODES:
        v = [l for r in results if r["mode"] == m for k, l in r["lat"] if not k.endswith("~hitch")]
        if v:
            out["latency_" + P.MODE_NAMES[m]] = block(v)
            out["latency_" + P.MODE_NAMES[m] + "_everything"] = block([l for r in results if r["mode"] == m
                                                                       for k, l in r["lat"]])
    ra = {}
    for r in results:
        for k, v in r["reasm"].items():
            ra[k] = ra.get(k, 0) + v
    out["reassembler"] = ra
    return out


CHECKS = {
    # scenario: (latency block, delivered %, (limit ms, share %))
    "nominal": ("latency_dense_everything", 100.0, (300, 99.9)),
    "stress20": ("latency_dense", 100.0, (300, 99.9)),
    "w1366": ("latency_dense", 100.0, (300, 99.9)),
    "mono_mix": ("latency_mono_everything", 100.0, (600, 99.0)),
}


def check(s):
    key, deliv, (lim, share) = CHECKS[s["scenario"]]
    b = s.get(key)
    if b is None:
        return False, "no requests in %s" % key
    got = b["le%d_pct" % lim]
    ok = b["delivered_pct"] >= deliv and got >= share and s["counts"]["undetected_frame"] == 0 \
        and s["counts"]["undetected_msg"] == 0
    return ok, "%s: n %d delivered %.4f%% (need %.1f), <= %d ms %.4f%% (need %.1f), P50/P99/P99.9 %.0f/%.0f/%.0f ms, " \
        "undetected frames %d messages %d" % (key, b["n"], b["delivered_pct"], deliv, lim, got, share, b["p50"], b["p99"],
                                              b["p999"], s["counts"]["undetected_frame"], s["counts"]["undetected_msg"])


def simulate(scen, runs, seed):
    t0 = time.time()
    with Pool(WORKERS) as pool:
        res = pool.map(run, [(scen, seed * 100000 + i) for i in range(runs)])
    s = summarize(scen, res, time.time() - t0)
    os.makedirs(os.path.join(HERE, "out"), exist_ok=True)
    with open(os.path.join(HERE, "out", "result_%s.json" % scen), "w") as f:
        json.dump(s, f, indent=1)
    return s


# ---------------------------------------------------------------- decode rates per category
def rate_capture(rng, mode, curve, mix, noise_kind, offset, tear):
    """One distorted capture of a random frame (same categories as helper/bridge/tests/distort.rs).
    Returns (decoded, decoded_wrong)."""
    def rand_frame():
        enc = P.Encoder(int(rng.integers(1, 65536)), 2560, 1440, mode=mode, addon_build=1)
        for _ in range(int(rng.integers(1, 3))):
            kind = KINDS[int(rng.choice(3, p=[0.6, 0.25, 0.15]))]
            m = P.msg_translate(int(rng.integers(1, 65536)), 1, 1, 2, "fr", rand_text(rng, kind)[:200],
                                rand_pairs(rng, kind)[:2])
            enc.queue(m)
        enc.update(0.0)
        return enc
    a = rand_frame()
    keys_a = P.block_keys(a.symbols, mode)
    b = rand_frame() if tear else None
    n = max(len(keys_a), len(P.block_keys(b.symbols, mode)) if b else 0)
    w = -(-n // 2) * 2 + 40
    base = rng.integers(0, 256, 3)
    bg = np.clip(base[None, None, :] + rng.integers(-40, 41, (4, w, 3)), 0, 255).astype(np.uint8)
    img = P.blocks_to_image(P.render_blocks(keys_a), w, bg).astype(np.float32)
    if b is not None:
        ib = P.blocks_to_image(P.render_blocks(P.block_keys(b.symbols, mode)), w, bg).astype(np.float32)
        if rng.random() < 0.3:
            r = int(rng.integers(1, 4))
            img[r:] = ib[r:]
        else:
            x = int(rng.integers(0, w))
            img[:, x:] = ib[:, x:]
            if x > 0 and rng.random() < 0.5:
                img[:, x - 1] = (img[:, x - 1] + ib[:, x - 1]) / 2
    if offset:
        sh = np.concatenate([bg[:, :1].astype(np.float32), img[:, :-1]], axis=1)
        img = (1 - offset) * img + offset * sh
    if mix is not None:
        img = np.einsum("ij,hwj->hwi", mix, img)
    iv = np.clip(np.rint(img), 0, 255).astype(np.int64)
    cap = np.stack([curve[ch][iv[:, :, ch]] for ch in range(3)], axis=2)
    if noise_kind == "pixel":
        cap += rng.integers(-6, 7, cap.shape)
    else:
        blk = rng.integers(-6, 7, (2, w // 2, 3))
        cap += np.repeat(np.repeat(blk, 2, axis=0), 2, axis=1)
    cap = np.clip(np.rint(cap), 0, 255).astype(np.uint8)
    st, fb, _ = P.decode_capture(cap)
    if st != "ok":
        return False, False
    return True, fb not in (a.frame, b.frame if b else None)


def curve_gap(rng, kind, lo):
    while True:
        c = random_curve(rng, kind)
        if min_gap(c) >= lo:
            return c


RATE_CATS = {
    "dense, identity, noise +-6": lambda rng: (0, random_curve(rng, "identity"), None, "pixel", 0.0, False),
    "dense, envelope curves (gap >= 16)": lambda rng: (0, curve_gap(rng, "envelope", 16.0), None, "block", 0.0, False),
    "robust, envelope + harsh curves": lambda rng: (1, random_curve(rng, "envelope" if rng.random() < 0.5 else "harsh"),
                                                    None, "block", 0.0, False),
    "mono, colour mixing + envelope": lambda rng: (2, random_curve(rng, "envelope"), random_mix(rng), "block", 0.0, False),
    "dense, 0.5 px offset": lambda rng: (0, random_curve(rng, "identity") if rng.random() < 0.5
                                         else curve_gap(rng, "envelope", 16.0), None, "pixel", 0.5, False),
    "torn, dense": lambda rng: (0, random_curve(rng, "identity"), None, "pixel", 0.0, True),
    "torn, robust": lambda rng: (1, random_curve(rng, "identity"), None, "pixel", 0.0, True),
    "torn, mono": lambda rng: (2, random_curve(rng, "identity"), None, "pixel", 0.0, True),
}


def rate_worker(args):
    name, n, seed = args
    low_priority()
    rng = np.random.default_rng(seed)
    ok = wrong = 0
    for _ in range(n):
        d, w = rate_capture(rng, *RATE_CATS[name](rng))
        ok += int(d and not w)
        wrong += int(w)
    return name, n, ok, wrong


def rates(n):
    with Pool(WORKERS) as pool:
        res = pool.map(rate_worker, [(name, n, 900 + i) for i, name in enumerate(RATE_CATS)])
    out = {}
    for name, cnt, ok, wrong in res:
        out[name] = {"captures": cnt, "decoded_pct": round(100.0 * ok / cnt, 3), "wrong_frames": wrong}
        print("%-36s %6d captures, decoded %8.3f%%, wrong frames %d" % (name, cnt, 100.0 * ok / cnt, wrong))
    return out


def main():
    low_priority()
    arg = sys.argv[1] if len(sys.argv) > 1 else "all"
    if arg == "rates":
        n = int(sys.argv[2]) if len(sys.argv) > 2 else 4000
        r = rates(n)
        os.makedirs(os.path.join(HERE, "out"), exist_ok=True)
        with open(os.path.join(HERE, "out", "rates.json"), "w") as f:
            json.dump(r, f, indent=1)
        return 0
    scens = list(SCEN) if arg == "all" else [arg]
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 150
    seed = int(sys.argv[3]) if len(sys.argv) > 3 else 7
    failed = 0
    for scen in scens:
        s = simulate(scen, runs, seed)
        ok, line = check(s)
        failed += 0 if ok else 1
        print("%-9s %s  %s  (%d runs, %.0f s, modes %s)" % (scen, "PASS" if ok else "FAIL", line, s["runs"], s["wall_s"],
                                                           s["modes"]))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
