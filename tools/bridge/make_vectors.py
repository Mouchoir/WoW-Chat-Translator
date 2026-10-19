"""Writes the bridge v2 test vectors (tests/vectors/) from the Python reference.

  frames/<trace>_<mode>.json  encoder traces: the Lua encoder must compose the same frames (bytes and
                              block colours) at the same game frames; the Rust decoder must decode
                              every frame and rebuild every message
  tags.json                   every row tag (slot 0..127, row 0..3, sessBit 0..1) plus markup samples
  messages.json               message builder inputs with the expected bytes, plus "raw" byte strings
                              no builder writes (parser-only cases); each with the reference parse
  strips.json                 distorted strip captures with the reference decoder's verdict
  marks.json                  marked spans (0x02 [0x04 RRGGBB] name 0x03): MarkSpan bytes ("encode", the
                              Lua builder must match), the spans a reader finds ("decode", for Translate
                              Companion's reader) and a TRANSLATE text with its COLOURS written in
                              ("merge", what Translate Companion's scene hands the engine)

Usage: python -I tools/bridge/make_vectors.py [out_dir]   (default: tests/vectors next to tools/)
All text is synthetic. Deterministic: the same files on every run."""
import os
import sys
import json
import random

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import proto as P  # noqa: E402

ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))

# ---------------------------------------------------------------- synthetic text
LATIN = "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789 ,.!?'-:"
WORDS = ["lf", "lf2m", "lfm", "wts", "wtb", "dps", "heal", "tank", "cheap", "need", "pst", "inv", "summon",
         "ty", "np", "brb", "afk", "omw", "gz", "boss", "pull", "run", "dungeon", "quest", "gold", "pls"]
CYR = [chr(c) for c in range(0x0430, 0x0450)]
CJK = [chr(c) for c in range(0x4E00, 0x4E00 + 400)]
HANGUL = [chr(c) for c in range(0xAC00, 0xAC00 + 300)]
EMOJI = [chr(c) for c in range(0x1F600, 0x1F640)]          # 4-byte UTF-8
ITEMS = ["[Linen Cloth]", "[Hearthstone]", "[Copper Ore]", "[Thunderfury]", "{skull}", "{star}", "{x}"]


def rand_text(rng, kind):
    if kind == "short":
        parts = []
        for _ in range(rng.randint(1, 12)):
            r = rng.random()
            if r < 0.6:
                parts.append(rng.choice(WORDS))
            elif r < 0.8:
                parts.append("\x02" + rng.choice(ITEMS) + "\x03")
            elif r < 0.9:
                parts.append("[" + rng.choice(WORDS) + "]")          # typed brackets: not marked
            else:
                parts.append("".join(rng.choice(LATIN) for _ in range(rng.randint(1, 8))))
        return " ".join(parts)
    if kind == "long":
        return "".join(rng.choice(LATIN) for _ in range(rng.randint(120, 255)))
    if kind == "cyr":
        return "".join(rng.choice(CYR + [" "]) for _ in range(rng.randint(10, 255)))
    if kind == "cjk":
        return "".join(rng.choice(CJK + HANGUL) for _ in range(rng.randint(10, 255)))
    if kind == "emoji":
        return "".join(rng.choice(EMOJI + CJK) for _ in range(rng.randint(5, 80)))
    # 'max': over every cap, cut by the builder (1024 B text, never inside a mark)
    s = "".join(rng.choice(EMOJI) for _ in range(250)) + "\x02[Linen Cloth]\x03" + "x" * rng.randint(0, 40)
    return s


def rand_pairs(rng, kind):
    if kind == "max":
        return [("t" * rng.randint(20, 60), "m" * rng.randint(40, 140)) for _ in range(rng.randint(6, 11))]
    out = []
    for _ in range(rng.randint(0, 5)):
        term = rng.choice(WORDS)
        meaning = " ".join(rng.choice(WORDS) for _ in range(rng.randint(1, 6)))
        if rng.random() < 0.2:
            meaning = "".join(rng.choice(CJK) for _ in range(rng.randint(2, 30)))
        out.append((term, meaning))
    return out


KINDS = ["short", "long", "cyr", "cjk", "emoji", "max"]
KIND_P = [0.5, 0.15, 0.1, 0.12, 0.08, 0.05]
TARGETS = ["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"]


def rand_view(rng):
    n = rng.randint(1, 4)
    return [{"id": rng.randint(1, 10), "x": rng.randint(0, 2000), "y": rng.randint(0, 1400), "w": rng.randint(200, 900),
             "h": rng.randint(80, 600), "fontPx": rng.randint(10, 30), "rowPx": rng.randint(12, 36)} for _ in range(n)]


def rand_message(rng, req):
    r = rng.random()
    if r < 0.75:
        kind = rng.choices(KINDS, KIND_P)[0]
        return P.msg_translate(req, rng.randint(0, 126), rng.randint(1, 4), rng.choice([0, 1, 2, 3]),
                               rng.choice(TARGETS), rand_text(rng, kind), rand_pairs(rng, kind))
    if r < 0.85:
        return P.msg_view(rand_view(rng))
    if r < 0.93:
        return P.msg_cancel(req, rng.randint(0, 127))
    if r < 0.97:
        return P.msg_test(rng.randint(0, 65535))
    return P.msg_clear()


# ---------------------------------------------------------------- encoder traces
TRACES = {
    # name: screen, strip cap, duration, mean ms between clicks, burst probability, extra events
    "a": dict(w=2560, h=1440, seconds=8, click=900, burst=0.1),
    "b": dict(w=1920, h=1080, seconds=8, click=900, burst=0.1),
    "c": dict(w=1366, h=768, seconds=8, click=900, burst=0.15),
    "d": dict(w=3840, h=2160, seconds=8, click=700, burst=0.1),
    "e": dict(w=1280, h=720, seconds=8, click=1200, burst=0.1),
    "f": dict(w=2560, h=1440, seconds=6, click=600, burst=0.3, max_cols=256),
    "g": dict(w=2560, h=1440, seconds=10, click=800, burst=0.1, screens=True),
    "h": dict(w=1920, h=1080, seconds=10, click=600, burst=0.3, altz=True, hold=True, loading=True),
    "i": dict(w=2560, h=1440, seconds=4, click=900, burst=0.2, maxonly=True, modes=True),
    # narrower than the mono and robust floors of the maxCols option: the strip must stay on screen
    "j": dict(w=1024, h=768, seconds=8, click=900, burst=0.2),
}
# Block keys (0..63, level indices r * 16 + g * 4 + b) are stored one character per block:
# the key is the character's index in BLOCK_CHARS.
BLOCK_CHARS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_"


def keys_str(keys):
    return "".join(BLOCK_CHARS[k] for k in keys)


SCREENS = [(2560, 1440), (1920, 1080), (1366, 768), (3840, 2160), (1280, 720), (1024, 768)]


def make_trace(name, cfg, mode, seed):
    rng = random.Random(seed)
    session = rng.randint(1, 65535)
    w, h = cfg["w"], cfg["h"]
    enc = P.Encoder(session, w, h, mode=mode, max_cols=cfg.get("max_cols"), addon_build=1)
    strip = P.Strip(enc)
    enc.set_screen(w, h)                         # CreateStrip's Rescale()
    ticks, events, frames, shown, messages = [], [], [], [], []
    t = 0.0
    fps = rng.choice([30.0, 60.0, 144.0])
    next_click = 150.0
    req = 0
    end = cfg["seconds"] * 1000.0

    while t < end:
        dt = 1000.0 / fps * rng.uniform(0.75, 1.25)
        if rng.random() < 0.01:
            dt = rng.uniform(80.0, 250.0)        # game hitch
        t += dt
        i = len(ticks)
        ticks.append(t)
        evs = []
        if t >= next_click:
            n = 1 + (rng.randint(1, 3) if rng.random() < cfg["burst"] else 0)
            for _ in range(n):
                req = req % 65535 + 1
                if cfg.get("maxonly"):
                    m = P.msg_translate(req, rng.randint(0, 126), 4, 3, "zh-TW", rand_text(rng, "max"),
                                        rand_pairs(rng, "max"))
                else:
                    m = rand_message(rng, req)
                evs.append({"queue": m.hex()})
            next_click = t + rng.expovariate(1.0 / cfg["click"])
        if cfg.get("screens") and rng.random() < 0.004:
            sw, sh = rng.choice(SCREENS)
            evs.append({"screen": [sw, sh]})
        if cfg.get("modes") and rng.random() < 0.004:
            evs.append({"mode": rng.choice(P.MODES)})
        if cfg.get("altz") and rng.random() < 0.006:
            evs.append({"parent": not strip.parent})
        if cfg.get("hold") and rng.random() < 0.004:
            evs.append({"hold": not strip.held})
        if cfg.get("loading") and rng.random() < 0.003:
            evs.append({"loading": not strip.loading})
        for ev in evs:
            if "queue" in ev:
                b = bytes.fromhex(ev["queue"])
                strip.send(b)
                messages.append(ev["queue"])
            elif "screen" in ev:
                enc.set_screen(*ev["screen"])
            elif "mode" in ev:
                enc.set_mode(ev["mode"])
            elif "parent" in ev:
                strip.set_parent(ev["parent"])
            elif "hold" in ev:
                strip.held = ev["hold"]
                strip.refresh()
            elif "loading" in ev:
                strip.loading = ev["loading"]
                strip.refresh()
            ev = dict(ev)
            ev["t_ms"] = t
            ev["tick"] = i
            events.append(ev)
        # OnUpdate gets GetTime() * 1000 with GetTime() = t / 1000 (same double arithmetic in Lua)
        now = (t / 1000.0) * 1000.0
        if strip.tick(now):
            frames.append({"t_ms": t, "tick": i, "physW": enc.phys_w, "blocks": keys_str(enc.block_keys()),
                           "bytes": enc.frame.hex()})
        shown.append("1" if strip.on else "0")
    # leave the queue drained at the end: keep ticking (no new events) until idle
    while not enc.is_idle() and strip.visible():
        t += 1000.0 / fps
        i = len(ticks)
        ticks.append(t)
        if strip.tick((t / 1000.0) * 1000.0):
            frames.append({"t_ms": t, "tick": i, "physW": enc.phys_w, "blocks": keys_str(enc.block_keys()),
                           "bytes": enc.frame.hex()})
        shown.append("1" if strip.on else "0")

    # what the helper must rebuild: every frame parsed, fed to the reference reassembler
    reasm = P.Reassembler()
    delivered = []
    for fr in frames:
        hdr, chunks = P.parse_frame(bytes.fromhex(fr["bytes"]))
        for _, m in reasm.feed(hdr, chunks, fr["t_ms"]):
            P.parse_message(m)
            delivered.append(m.hex())
    return {"trace": name, "mode": mode, "modeName": P.MODE_NAMES[mode], "session": session, "physW": w,
            "physH": h, "maxCols": cfg.get("max_cols"), "addonBuild": 1, "ticks": ticks, "events": events,
            "frames": frames, "shown": "".join(shown), "messages": messages,
            "delivered": delivered}


# ---------------------------------------------------------------- tags
def make_tags():
    tags = []
    for slot in range(128):
        for row in range(4):
            for sb in range(2):
                tags.append({"slot": slot, "row": row, "sessBit": sb, "payload": P.tag_payload(slot, row, sb),
                             "crc5": P.crc5(P.tag_payload(slot, row, sb)), "v": P.tag_value(slot, row, sb),
                             "digits": P.tag_digits(slot, row, sb), "colors": P.tag_colors(slot, row, sb)})
    markup = []
    rng = random.Random(77)
    for slot, row, sb, w, h in [(0, 0, 0, 2, 6), (5, 2, 0, 3, 14), (127, 0, 0, 2, 12), (126, 3, 1, 5, 20),
                                (64, 1, 1, 0, 0), (12, 2, 1, 5000, 7.6)]:
        markup.append({"slot": slot, "row": row, "sessBit": sb, "w": w, "h": h,
                       "markup": P.tag_markup(slot, row, sb, w, h)})
    for _ in range(10):
        slot, row, sb, w, h = rng.randint(0, 127), rng.randint(0, 3), rng.randint(0, 1), rng.randint(2, 6), rng.randint(6, 24)
        markup.append({"slot": slot, "row": row, "sessBit": sb, "w": w, "h": h,
                       "markup": P.tag_markup(slot, row, sb, w, h)})
    plates = [{"w": w, "h": h, "markup": P.plate_markup(w, h)} for w, h in [(300, 14), (40, 6), (0, 0), (9999, 12.7)]]
    return {"check": [{"slot": 0, "row": 0, "sessBit": 0, "crc5": 25, "v": 25, "digits": [0, 0, 0, 0, 4, 1]},
                      {"slot": 5, "row": 2, "sessBit": 0, "crc5": 17, "v": 1425, "digits": [0, 1, 0, 3, 3, 3]},
                      {"slot": 127, "row": 0, "sessBit": 0, "crc5": 23, "v": 32535, "digits": [4, 1, 0, 3, 4, 3]}],
            "tags": tags, "markup": markup, "plates": plates}


# ---------------------------------------------------------------- messages
def S(s):
    """A Lua string argument (bytes, exact)."""
    return {"s": (s.encode("utf-8") if isinstance(s, str) else s).hex()}


def make_messages():
    out = []

    def tr(note, req, slot, rows, flags, target, text, pairs):
        args = [req, slot, rows, flags, S(target) if target is not None else None, S(text) if text is not None else None,
                None if pairs is None else [[S(a) if isinstance(a, (str, bytes)) else a for a in p] for p in pairs]]
        b = P.msg_translate(req, slot, rows, flags, target, text, pairs)
        out.append({"fn": "translate", "note": note, "args": args, "hex": b.hex()})

    tr("marks and pairs", 9, 4, 2, 2, "fr", "lf2m dps \x02[Linen Cloth]\x03 cheap \x02{skull}\x03",
       [("lf2m", "looking for 2 more"), ("cheap", "pas cher")])
    tr("utf8 and limits", 65535, 126, 4, 1, "zh-TW", "été 你好", [])
    tr("no pairs table", 1, 0, 1, 0, "en", "hello", None)
    tr("nil text", 2, 1, 1, 2, "en", None, [])
    tr("flags masked", 3, 2, 3, 255, "de", "x", [])
    tr("clamped ints", 70000, 300, -2, 2.9, "en", "x", [])
    tr("float ints floored", 12.7, 3.99, 2.5, 3, "es", "y", [])
    tr("target cut to 8", 4, 5, 1, 2, "abcdefghijk", "z", [])
    tr("text over 1024 cut on utf8", 5, 6, 1, 2, "ru", "ж" * 600, [])
    tr("text cut before an open mark", 6, 7, 2, 2, "fr", "a" * 1015 + "\x02[Linen Cloth]\x03", [])
    tr("text cut after a closed mark", 7, 8, 2, 2, "fr", "a" * 1000 + "\x02[ab]\x03" + "b" * 40, [])
    tr("emoji cut", 8, 9, 3, 2, "zh", "\U0001F600" * 300, [])
    tr("pairs cut 48/120", 9, 10, 1, 2, "fr", "t", [("t" * 60, "m" * 150), ("é" * 30, "一" * 50)])
    tr("pairs stop at 8", 10, 11, 1, 2, "fr", "t", [("a%d" % i, "b%d" % i) for i in range(12)])
    tr("pairs stop before 512 B", 11, 12, 1, 2, "fr", "t", [("t" * 40, "m" * 100) for _ in range(6)])
    tr("bad pairs skipped", 12, 13, 1, 2, "fr", "t", [("ok", "fine"), ("x", 5), ("y",), ("ok2", "fine2")])
    tr("empty target", 13, 14, 1, 2, "", "t", [])
    tr("bytes kept as given", 14, 15, 1, 2, "en", b"\xff\xfe raw", [(b"\xc3", b"\xa9")])
    tr("1024 B exactly", 15, 16, 4, 3, "it", "q" * 1024, [("a", "b")])
    tr("worst case", 16, 17, 4, 3, "zh-TW", "\U0001F600" * 256, [("t" * 48, "m" * 120) for _ in range(8)])
    tr("6 rows (the most since 2026-10-18)", 17, 18, 6, 2, "fr", "lange Zeile", [])
    tr("7 rows refused", 18, 19, 7, 2, "fr", "x", [])
    # reply and copy (2026-10-19, STATUS.addonBuild 3): copy needs reply; reply needs the plate,
    # 2..6 rows and an explicit language code. The builder only drops copy without reply.
    tr("reply (flags 6)", 257, 126, 2, 6, "de", "on part ?", [])
    tr("reply with copy (flags 14), the BRIDGE.md example", 258, 126, 3, 14, "de", "on part dans 5 min ?", [])
    tr("reply with copy and noCache on 6 rows (flags 15)", 259, 126, 6, 15, "zh-TW", "lf heal", [])
    tr("reply without the plate refused (flags 4)", 260, 126, 3, 4, "de", "x", [])
    tr("copy alone dropped by the builder (flags 8 -> 0)", 261, 20, 1, 8, "fr", "x", [])
    tr("copy and plate without reply dropped to plate (flags 10 -> 2)", 262, 21, 2, 10, "fr", "x", [])
    tr("reply and copy without the plate refused (flags 12)", 263, 126, 3, 12, "de", "x", [])
    tr("reply on 1 row refused (flags 6, rows 1)", 264, 126, 1, 6, "de", "x", [])
    tr("reply on 7 rows refused", 265, 126, 7, 14, "de", "x", [])
    tr("reply to an undecided language refused (target ?)", 266, 126, 3, 14, "?", "x", [])
    tr("reply target with a digit refused", 267, 126, 3, 14, "de1", "x", [])
    tr("reply on another slot (any 0..126 is valid)", 268, 0, 2, 6, "fr", "x", [("lf", "cherche")])
    # coloured marks (2026-10-19, STATUS.addonBuild 5): the addon's text holds 0x02 0x04 RRGGBB
    # name 0x03 for a link name (MarkSpan); the TRANSLATE carries it without the colour (no 0x04
    # on the wire) and COLOURS, sent right before it, carries (span index, RGB)
    rare = P.mark_span("[Lattice Leggings]", "0070DD").decode("utf-8")
    spell = P.mark_span("[Fireball]", "71D5FF").decode("utf-8")
    skull = P.mark_span("{skull}").decode("utf-8")

    def co(note, req, text):
        args = [req, S(text) if text is not None else None]
        out.append({"fn": "colours", "note": note, "args": args, "hex": P.msg_colours(req, text).hex()})

    text = "WTS " + rare + " and " + spell + " " + skull
    tr("coloured marks (addon build 5): the TRANSLATE has no 0x04", 269, 22, 2, 2, "fr", text, [("wts", "vend")])
    co("coloured marks: COLOURS of spans 0 and 1, not the {skull} group", 269, text)
    tr("text cut before an open coloured mark", 270, 23, 2, 2, "fr", "a" * 1010 + rare, [])
    co("the cut span's colour is not sent: no COLOURS", 270, "a" * 1010 + rare)
    tr("coloured mark ending at 1024 B without its colour prefix", 271, 24, 1, 2, "fr", "a" * (1024 - len(rare) + 7) + rare, [])
    co("its colour is sent", 271, "a" * (1024 - len(rare) + 7) + rare)
    long_text = "a" * 990 + skull + rare + " " + spell + "b" * 30
    tr("a cut between two coloured marks", 273, 25, 1, 2, "fr", long_text, [])
    co("only the kept span's colour (index 1)", 273, long_text)
    tr("coloured mark in a reply (flags 14)", 272, 126, 3, 14, "de", "kaufe " + rare, [])
    co("a reply's COLOURS", 272, "kaufe " + rare)
    co("no coloured span: no COLOURS", 274, "WTS " + P.mark_span("[Plain]").decode("utf-8") + " " + skull)
    co("nil text: no COLOURS", 275, None)
    co("a 0x04 not followed by 6 hex digits: dropped, no colour", 276, "\x02\x04FF00[x]\x03 \x02\x04" + "0070dd[y]\x03")
    co("a stray 0x04 outside a span: deleted", 277, "a\x04b " + spell)
    co("clamped reqId, a CJK name", 70000, "\x02\x04FF8000[霜之哀伤]\x03")
    many = "".join(P.mark_span("x", "%06X" % (i * 4099)).decode("utf-8") for i in range(300))
    co("300 coloured spans of 3 B kept by the cut: the first 255 (index 0..254)", 278, many)

    def raw(note, b):
        out.append({"fn": "raw", "note": note, "hex": b.hex()})

    # bytes no builder writes: the parsers must refuse them
    base = lambda flags, rows=3: bytes([0x10, 0, 1, 126, rows, flags, 2]) + b"de" + bytes([0, 1]) + b"x" + bytes([0])
    raw("raw: copy alone refused (flags 8)", base(8))
    raw("raw: copy and plate without reply refused (flags 10)", base(10))
    raw("raw: bit 4 refused (flags 16)", base(16))
    raw("raw: bit 7 refused (flags 128)", base(128))
    raw("raw: reply with copy accepted (flags 14)", base(14))
    raw("raw: reply on 2 rows accepted (flags 6, rows 2)", base(6, 2))
    raw("raw: COLOURS with no span refused", bytes([0x15, 1, 44, 0]))
    raw("raw: COLOURS with span indexes not increasing refused", bytes([0x15, 1, 44, 2, 1, 0, 0x70, 0xDD, 1, 0x71, 0xD5, 0xFF]))
    raw("raw: COLOURS with a trailing byte refused", bytes([0x15, 1, 44, 1, 0, 0, 0x70, 0xDD, 0]))
    raw("raw: COLOURS span 255 accepted", bytes([0x15, 0, 1, 1, 255, 0xFF, 0xD1, 0]))
    out.append({"fn": "cancel", "args": [513, 126], "hex": P.msg_cancel(513, 126).hex()})
    out.append({"fn": "cancel", "args": [70000, 300], "hex": P.msg_cancel(70000, 300).hex()})
    out.append({"fn": "clear", "args": [], "hex": P.msg_clear().hex()})
    out.append({"fn": "test", "args": [40000], "hex": P.msg_test(40000).hex()})
    out.append({"fn": "test", "args": [-5], "hex": P.msg_test(-5).hex()})
    views = [{"id": 1, "x": 10, "y": 900, "w": 430, "h": 180, "fontPx": 18, "rowPx": 20},
             {"id": 3, "x": 1600, "y": 1000, "w": 500, "h": 200, "fontPx": 14, "rowPx": 16}]
    out.append({"fn": "view", "args": [views], "hex": P.msg_view(views).hex()})
    out.append({"fn": "view", "args": [[]], "hex": P.msg_view([]).hex()})
    big = [{"id": 300, "x": -4, "y": 70000, "w": 12.9, "h": 5, "fontPx": 256, "rowPx": 7}] * 6
    out.append({"fn": "view", "args": [big], "hex": P.msg_view(big).hex()})
    missing = [{"id": 2, "x": 5}]
    out.append({"fn": "view", "args": [missing], "hex": P.msg_view(missing).hex()})
    out.append({"fn": "utf8cut", "args": [S("été"), 2], "hex": P.utf8_cut("été".encode(), 2).hex()})
    out.append({"fn": "utf8cut", "args": [S("\U0001F600a"), 3], "hex": P.utf8_cut("\U0001F600a".encode(), 3).hex()})
    out.append({"fn": "utf8cut", "args": [S("abc"), 5], "hex": b"abc".hex()})
    return out


# ---------------------------------------------------------------- marked spans (docs/BRIDGE.md)
def make_marks():
    """encode: Bridge.MarkSpan(name, rgb) bytes; decode: the spans a reader finds in a TRANSLATE
    text (P.parse_marks), for Translate Companion's own reader. All text is synthetic."""
    enc, dec = [], []

    def e(note, name, rgb):
        enc.append({"note": note, "name": S(name) if name is not None else None, "rgb": rgb,
                    "hex": P.mark_span(name, rgb).hex()})

    def d(note, text):
        dec.append({"note": note, "text": S(text),
                    "spans": [[name.hex(), rgb] for name, rgb in P.parse_marks(P._b(text))]})

    e("a rare item's name in its blue", "[Lattice Leggings]", "0070DD")
    e("a lowercase colour is written uppercase", "[Fireball]", "71d5ff")
    e("no colour: a {...} group", "{skull}", None)
    e("a colour of 5 digits: none", "[x]", "0070D")
    e("a colour of 7 digits: none", "[x]", "0070DDD")
    e("a colour with a non-hex digit: none", "[x]", "0070DG")
    e("control bytes 0x01-0x04 removed from the name", "a\x01b\x02c\x03d\x04e", "FFD100")
    e("an empty name: nothing", "", "FFD100")
    e("a name of control bytes only: nothing", "\x02\x03\x04", "FFD100")
    e("a UTF-8 name", "[Étoffe de lin]", "FFFFFF")
    e("a CJK name", "[霜之哀伤]", "FF8000")
    e("no name (nil): nothing", None, "FFD100")

    d("two coloured links and a group", "WTS \x02\x040070DD[Lattice Leggings]\x03 and \x02\x0471D5FF[Fireball]\x03 \x02{skull}\x03")
    d("a lowercase colour is read", "\x02\x04a335ee[Sunlit Mantle]\x03")
    d("0x04 not followed by 6 hex digits: dropped, no colour", "\x02\x04XYZ[x]\x03")
    d("0x04 with 4 hex digits before 0x03: dropped, the digits are the name", "\x02\x04FFD1\x03")
    d("a colour and no name: not a span", "\x02\x04FFD100\x03")
    d("never closed: not a span", "a \x02\x04FFD100[x")
    d("a 0x03 outside a span: text", "a\x03b")
    d("a 0x02 inside an open span starts it again", "\x02a\x02\x04FFD100[b]\x03")
    d("a 0x04 inside a name is dropped", "\x02[a\x04b]\x03")
    d("no marks", "plain text 20h30")
    d("UTF-8 names", "\x02\x04FFFFFF[Étoffe de lin]\x03 \x02\x04FF8000[霜之哀伤]\x03")

    merge = []

    def mg(note, wire, colours):
        merge.append({"note": note, "wire": S(wire), "colours": [[k, rgb] for k, rgb in colours],
                      "merged": S(P.merge_colours(P._b(wire), colours))})

    two = "WTS \x02[Lattice Leggings]\x03 and \x02[Fireball]\x03 \x02{skull}\x03"
    mg("two links coloured, the group left uncoloured but marked", two, [(0, "0070DD"), (1, "71D5FF")])
    mg("no colours: the text unchanged", two, [])
    mg("an index past the last span: ignored", "a \x02[x]\x03", [(5, "010203")])
    mg("the index counts the reader's spans; marks that are not spans are dropped",
       "\x03\x02\x03\x02[a]\x03 \x02[b", [(0, "FF0000")])
    mg("a colour already in the text stays unless COLOURS has one for its span",
       "\x02\x0400FF00[a]\x03\x02\x0400FF00[b]\x03", [(1, "FF0000")])
    mg("UTF-8 names", "\x02[Étoffe de lin]\x03 \x02[霜之哀伤]\x03", [(1, "FF8000")])
    addon = "WTS " + P.mark_span("[Lattice Leggings]", "0070DD").decode("utf-8") + " " + P.mark_span("{skull}").decode("utf-8") \
        + " " + P.mark_span("[Fireball]", "71D5FF").decode("utf-8")
    wire, colours = P.split_colours(addon)
    mg("the addon's text split for the wire and merged back is the addon's text", wire.decode("utf-8"), colours)
    assert P.merge_colours(wire, colours) == addon.encode("utf-8")
    return {"encode": enc, "decode": dec, "merge": merge}


# ---------------------------------------------------------------- distorted strips (decoder cross-check)
def make_strips():
    np = P._numpy()
    from curves import random_curve, random_mix
    rng = np.random.default_rng(2026)
    pyr = random.Random(5)
    out = []
    for i in range(36):
        mode = P.MODES[i % 3]
        w = [480, 560, 700][i % 3]
        enc = P.Encoder(pyr.randint(1, 65535), w, 480, mode=mode, addon_build=1)
        req = i + 1
        m = rand_message(pyr, req)
        while len(m) > 110:                              # short frames: they fit the capture width
            m = rand_message(pyr, req)
        enc.queue(m)
        if i % 4 == 0:
            enc.queue(P.msg_cancel(req, 3))
        enc.update(0.0)
        keys = enc.block_keys()
        kind = ["identity", "envelope", "envelope", "harsh"][i % 4]
        curve = random_curve(rng, kind)
        bg = np.clip(rng.integers(0, 256, 3)[None, None, :] + rng.integers(-40, 41, (4, w, 3)), 0, 255).astype(np.uint8)
        img = P.blocks_to_image(P.render_blocks(keys), w, bg)
        f = img.astype(np.float64)
        if mode == P.MODE_MONO and i % 2:
            f = np.einsum("ij,hwj->hwi", random_mix(rng), f)
        if i % 6 == 5:                                   # 0.5 px horizontal offset
            sh = np.concatenate([f[:, :1], f[:, :-1]], axis=1)
            f = 0.5 * f + 0.5 * sh
        iv = np.clip(np.rint(f), 0, 255).astype(np.int64)
        cap = np.stack([curve[ch][iv[:, :, ch]] for ch in range(3)], axis=2)
        cap += rng.integers(-6, 7, cap.shape)
        if i % 8 == 7:                                   # torn: right part from another frame
            x = int(rng.integers(40, w))
            cap[:, x:] = np.clip(cap[:, x:] + rng.integers(-80, 81, (1, 1, 3)), 0, 255)
        cap = np.clip(np.rint(cap), 0, 255).astype(np.uint8)
        st, fb, info = P.decode_capture(cap)
        out.append({"w": w, "h": 4, "mode": mode, "kind": kind, "rgb": cap.tobytes().hex(), "status": st,
                    "bytes": fb.hex() if fb is not None else None, "truth": enc.frame.hex()})
    return out


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "tests", "vectors")
    os.makedirs(os.path.join(out_dir, "frames"), exist_ok=True)
    nframes = nmsgs = 0
    for k, (name, cfg) in enumerate(TRACES.items()):
        for mode in P.MODES:
            tr = make_trace(name, cfg, mode, 1000 + 10 * k + mode)
            nframes += len(tr["frames"])
            nmsgs += len(tr["messages"])
            with open(os.path.join(out_dir, "frames", "%s_%s.json" % (name, P.MODE_NAMES[mode])), "w") as f:
                json.dump(tr, f, separators=(",", ":"))
    with open(os.path.join(out_dir, "tags.json"), "w") as f:
        json.dump(make_tags(), f, separators=(",", ":"))
    marks = make_marks()
    with open(os.path.join(out_dir, "marks.json"), "w") as f:
        json.dump(marks, f, indent=0, separators=(",", ":"))
    msgs = make_messages()
    for m in msgs:
        if m["fn"] == "utf8cut":
            continue
        try:
            d = P.parse_message(bytes.fromhex(m["hex"]))
            if d["type"] == "translate":
                d["text"] = d["text"].encode("utf-8").hex()
                d["pairs"] = [[a.encode("utf-8").hex(), b.encode("utf-8").hex()] for a, b in d["pairs"]]
            m["parsed"] = d
        except ValueError:
            m["parsed"] = None
    with open(os.path.join(out_dir, "messages.json"), "w") as f:
        json.dump(msgs, f, indent=0, separators=(",", ":"))
    strips = make_strips()
    with open(os.path.join(out_dir, "strips.json"), "w") as f:
        json.dump(strips, f, separators=(",", ":"))
    ok = sum(1 for s in strips if s["status"] == "ok")
    print("traces %d, frames %d, messages %d, tags 1024, builder vectors %d, strips %d (%d decode ok), "
          "marks %d encode + %d decode + %d merge"
          % (len(TRACES) * len(P.MODES), nframes, nmsgs, len(msgs), len(strips), ok, len(marks["encode"]),
             len(marks["decode"]), len(marks["merge"])))


if __name__ == "__main__":
    main()
