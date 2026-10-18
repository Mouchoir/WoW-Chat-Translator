"""Monte Carlo of the strip frame integrity check (bridge v2), at the symbol level, in all three
modes. Real frames from the reference encoder (proto.Encoder fed with synthetic requests and
VIEWs) are corrupted the way the pixel channel corrupts them, then decoded exactly like the helper
does (magic, version, length, CRC-32, then the strict record parse). A frame that passes every
check but differs from every frame the encoder composed is an undetected corruption.

Corruptions (one per trial):
  adjacent  1-20 symbol misreads: dense = one channel one level up or down, robust = one channel
            bit flipped (0 <-> 255), mono = the grey one level up or down;
  splice    two frames of the same session joined at a random symbol (a torn capture), the symbol
            at the tear sometimes random (a blended column);
  burst     1-40 consecutive random symbols.
The capture always holds 64 random background symbols after the strip.

Usage: python -I tools/bridge/crc_mc.py [trials] [seed]     (default 2000000 trials, seed 1)
Runs 3 worker processes at below-normal priority (a game may be running on this PC). Prints one
JSON line per mode and a total; exits 1 when any corruption went undetected."""
import os
import sys
import json
import time
from multiprocessing import Pool

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import proto as P  # noqa: E402
import sim  # noqa: E402  (synthetic payload generators)

np = P._numpy()
WORKERS = 3
SESSIONS = 24          # encoder sessions per worker and mode
FRAMES = 40            # frames per session


def frames_for_session(rng, mode):
    enc = P.Encoder(int(rng.integers(1, 65536)), 2560, 1440, mode=mode, addon_build=1)
    out, t, req = [], 0.0, 0
    while len(out) < FRAMES:
        t += 51.0
        if rng.random() < 0.5:
            kind = sim.KINDS[int(rng.choice(4, p=sim.KIND_P))]
            req = req % 65535 + 1
            enc.queue(P.msg_translate(req, int(rng.integers(0, 127)), int(rng.integers(1, 5)), 2,
                                      str(rng.choice(sim.TARGETS)), sim.rand_text(rng, kind),
                                      sim.rand_pairs(rng, kind)))
        if rng.random() < 0.2:
            enc.queue(P.msg_view(sim.rand_view(rng)))
        if rng.random() < 0.05:
            enc.queue(P.msg_cancel(req, 3))
        if enc.update(t):
            out.append(enc.frame)
    return out


def decode(syms, mode):
    """Symbols -> (status, frame bytes): the helper's checks after classification."""
    bps = P.BITS[mode]
    sh = np.arange(bps - 1, -1, -1, dtype=np.uint8)
    bits = ((syms[:, None] >> sh) & 1).astype(np.uint8).ravel()
    b = np.packbits(bits[:len(bits) // 8 * 8]).tobytes()
    if len(b) < P.HDR_LEN + P.CRC_LEN or b[0] != P.MAGIC[mode]:
        return "nomagic", None
    if b[1] != P.VERSION:
        return "badver", None
    total = P.HDR_LEN + ((b[6] << 8) | b[7]) + P.CRC_LEN
    if total > len(b):
        return "toolong", None
    fb = b[:total]
    if P.crc32(fb[:-4]) != int.from_bytes(fb[-4:], "big"):
        return "crc", None
    try:
        P.parse_frame(fb)
    except ValueError:
        return "malformed", None
    return "ok", fb


def corrupt(rng, mode, sa, frames, syms_of):
    """Returns (kind, corrupted symbols)."""
    n = len(sa)
    r = rng.random()
    if r < 0.5:
        s = sa.copy()
        for p in rng.integers(0, n, int(rng.integers(1, 21))):
            v = int(s[p])
            if mode == P.MODE_DENSE:
                sh = 2 * int(rng.integers(0, 3))
                lv = (v >> sh) & 3
                nl = 1 if lv == 0 else 2 if lv == 3 else lv + int(rng.choice([-1, 1]))
                s[p] = (v & ~(3 << sh)) | (nl << sh)
            elif mode == P.MODE_ROBUST:
                s[p] = v ^ (1 << int(rng.integers(0, 3)))
            else:
                s[p] = 1 if v == 0 else 2 if v == 3 else v + int(rng.choice([-1, 1]))
        return "adjacent", s
    if r < 0.8:
        sb = syms_of(frames[int(rng.integers(len(frames)))])
        m = max(n, len(sb))
        s = np.zeros(m, dtype=np.uint8)
        s[:n] = sa
        cut = int(rng.integers(0, m))
        s[cut:len(sb)] = sb[cut:]
        if rng.random() < 0.5 and cut < m:
            s[cut] = rng.integers(0, 1 << P.BITS[mode])
        return "splice", s
    s = sa.copy()
    p = int(rng.integers(0, n))
    ln = int(rng.integers(1, 41))
    s[p:p + ln] = rng.integers(0, 1 << P.BITS[mode], len(s[p:p + ln]))
    return "burst", s


def work(args):
    mode, trials, seed = args
    sim.low_priority()
    rng = np.random.default_rng(seed)
    sessions = [frames_for_session(rng, mode) for _ in range(SESSIONS)]
    cache = {}

    def syms_of(fb):
        if fb not in cache:
            cache[fb] = np.array(P.to_symbols(fb, mode), dtype=np.uint8)
        return cache[fb]

    st = {"trials": 0, "adjacent": 0, "splice": 0, "burst": 0, "nomagic": 0, "badver": 0, "toolong": 0,
          "crc": 0, "malformed": 0, "ok_true": 0, "undetected": 0}
    top = 1 << P.BITS[mode]
    for _ in range(trials):
        frames = sessions[int(rng.integers(len(sessions)))]
        a = frames[int(rng.integers(len(frames)))]
        kind, s = corrupt(rng, mode, syms_of(a), frames, syms_of)
        st[kind] += 1
        st["trials"] += 1
        s = np.concatenate([s, rng.integers(0, top, 64).astype(np.uint8)])
        status, fb = decode(s, mode)
        if status != "ok":
            st[status] += 1
        elif fb in frames:            # every frame of this session is a true frame
            st["ok_true"] += 1
        else:
            st["undetected"] += 1
    return mode, st


def main():
    sim.low_priority()
    trials = int(sys.argv[1]) if len(sys.argv) > 1 else 2000000
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    t0 = time.time()
    per_mode = [trials // 3 + (1 if m < trials % 3 else 0) for m in P.MODES]
    jobs = []
    for mode in P.MODES:
        n, k = per_mode[mode], 8
        for j in range(k):
            jobs.append((mode, n // k + (1 if j < n % k else 0), seed * 1000 + mode * 100 + j))
    total = {}
    by_mode = {m: {} for m in P.MODES}
    with Pool(WORKERS) as pool:
        for mode, st in pool.imap_unordered(work, jobs):
            for k, v in st.items():
                by_mode[mode][k] = by_mode[mode].get(k, 0) + v
                total[k] = total.get(k, 0) + v
    for m in P.MODES:
        print(json.dumps({"mode": P.MODE_NAMES[m], **by_mode[m]}))
    print(json.dumps({"mode": "all", "seconds": round(time.time() - t0, 1), **total}))
    return 1 if total.get("undetected", 0) else 0


if __name__ == "__main__":
    sys.exit(main())
