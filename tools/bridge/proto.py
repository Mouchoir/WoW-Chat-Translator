"""Pixel bridge protocol v2, reference implementation (Python).

The addon side (Encoder, message builders, row tags) mirrors addon/TranslateProbe/Bridge.lua
byte for byte; the helper side (decode_capture, parse_frame, Reassembler, parse_message) is the
reference for helper/bridge (Rust). The normative spec is docs/BRIDGE.md.

Only the decoder needs numpy. It is imported lazily (see _numpy), so the encoder, the builders
and the vector generator also run on a bare Python."""
import sys
import zlib

# ---------------------------------------------------------------- constants (normative)
VERSION = 2
LEVELS = (0, 104, 146, 255)            # 8-bit value drawn for level index 0..3
MODE_DENSE, MODE_ROBUST, MODE_MONO = 0, 1, 2
MODES = (MODE_DENSE, MODE_ROBUST, MODE_MONO)
MODE_NAMES = {0: "dense", 1: "robust", 2: "mono"}
MAGIC = {0: 0xD5, 1: 0x2A, 2: 0x5A}
BITS = {0: 6, 1: 3, 2: 2}
BLOCK = 2                              # block edge in physical pixels
ROWS = 2                               # block rows -> strip height 4 px
CALIB = (0, 3, 3, 0, 1, 2, 2, 1)       # level index of calibration blocks 0..7 (grey)
NCAL = len(CALIB)
HDR_LEN, CRC_LEN, REC_HDR = 8, 4, 3
REC_STATUS, REC_LAYOUT_V1, REC_CHUNK = 0x01, 0x02, 0x03
STATUS_DATA = 6                        # physW u16, physH u16, addonBuild u16
STATUS_LEN = REC_HDR + STATUS_DATA
CHUNK_FIXED = 10                       # msgId u16, idx u8, count u8, total u16, msgCrc u32
MSG_TRANSLATE, MSG_CANCEL, MSG_CLEAR, MSG_TEST, MSG_VIEW = 0x10, 0x11, 0x12, 0x13, 0x14
MIN_COLS = {0: 192, 1: 384, 2: 576}    # floor for the max_cols option: 248-252 B of chunk data
MIN_SCREEN_COLS = 200                  # floor for a degenerate screen size: 64 B of chunk data in mono
CREDIT_CAP_MS = 100                    # max exposure credited per game frame (hitch guard)
HOLD_MS, ROT_HOLD_MS, EXPOSE_MS = 50, 70, 400

MAX_TEXT, MAX_TARGET, MAX_PAIRS, MAX_TERM, MAX_MEANING, MAX_PAIR_BYTES = 1024, 8, 8, 48, 120, 512
MAX_VIEWS = 4
MAX_ROWS = 6                           # TRANSLATE rows 1..6 (1..4 until 2026-10-18)
# TRANSLATE flags. reply (the conversation window's reply card: row 0 = the Helper's status,
# rows 1.. = the text) and copy (the Helper may put the final translation in the clipboard)
# were added on 2026-10-19 (STATUS.addonBuild 3).
FLAG_NOCACHE, FLAG_PLATE, FLAG_REPLY, FLAG_COPY = 1, 2, 4, 8
FLAGS_MASK = 15                        # bits 4-7 are 0
REPLY_MIN_ROWS = 2                     # a reply has the status row and at least one text row

TEST_SLOT = 127
PLATE_RGB = (32, 40, 64)
TAG_SYNC = ((255, 0, 255), (0, 255, 0))                       # magenta, green
TAG_DIGIT = ((0, 0, 0), (255, 0, 0), (0, 0, 255), (255, 255, 0), (0, 255, 255), (255, 255, 255))
TEX_TAG = "|TInterface\\Buttons\\WHITE8X8:%d:%d:0:0:8:8:0:8:0:8:%d:%d:%d|t"   # h, w, r, g, b

_np = None


def _numpy():
    """numpy, imported on first use. Under `python -I` the user site-packages (where numpy is
    installed on the dev PC) is not on sys.path, so it is appended (after the standard library)
    when the first import fails."""
    global _np
    if _np is None:
        try:
            import numpy
        except ImportError:
            import site
            sys.path.append(site.getusersitepackages())
            import numpy
        _np = numpy
    return _np


def crc32(b):
    return zlib.crc32(bytes(b)) & 0xFFFFFFFF


def u16(v):
    return [(v >> 8) & 255, v & 255]


def u32(v):
    return [(v >> 24) & 255, (v >> 16) & 255, (v >> 8) & 255, v & 255]


def clamp(v, lo, hi):
    """Lua Clamp: floor(tonumber(v) or 0), then clamped."""
    try:
        v = int(v // 1)
    except TypeError:
        v = 0
    return lo if v < lo else hi if v > hi else v


def frame_capacity(ncols, mode):
    """Bytes (header + body + crc) that fit in a strip of ncols block columns."""
    return ((ROWS * ncols - NCAL) * BITS[mode]) // 8


def max_chunk_data(ncols, mode):
    return frame_capacity(ncols, mode) - HDR_LEN - CRC_LEN - STATUS_LEN - REC_HDR - CHUNK_FIXED


# ---------------------------------------------------------------- messages (addon builders)
def utf8_cut(b, n):
    """Cuts bytes b to at most n bytes without splitting a UTF-8 sequence."""
    if len(b) <= n:
        return b
    i = n
    while i > 0 and 0x80 <= b[i] < 0xC0:
        i -= 1
    return b[:i]


def cut_marked(b, n):
    """utf8_cut that never leaves a 0x02 link mark open."""
    cut = utf8_cut(b, n)
    if len(cut) < len(b):
        o, c = cut.rfind(b"\x02"), cut.rfind(b"\x03")
        if o >= 0 and (c < 0 or o > c):
            cut = cut[:o]
    return cut


def _b(s):
    if isinstance(s, bytes):
        return s
    if isinstance(s, str):
        return s.encode("utf-8")
    return b""


def str8(s, n):
    b = utf8_cut(_b(s), n)
    return bytes([len(b)]) + b


def translate_flags(flags):
    """Bridge.TranslateMessage's flags byte: bits 0-3 kept, copy dropped unless reply."""
    f = clamp(flags, 0, 255) & FLAGS_MASK
    if f & FLAG_COPY and not f & FLAG_REPLY:
        f &= ~FLAG_COPY
    return f


def msg_translate(req_id, slot, rows, flags, target, text, pairs):
    """Bridge.TranslateMessage. pairs: list of (term, meaning); entries that are not two strings
    are skipped; at most 8, each cut to 48 / 120 bytes, stopping before 512 bytes in total."""
    tb = cut_marked(_b(text), MAX_TEXT)
    out = bytes([MSG_TRANSLATE]) + bytes(u16(clamp(req_id, 0, 65535))) + bytes([clamp(slot, 0, 255)])
    out += bytes([clamp(rows, 0, 255), translate_flags(flags)])
    out += str8(target, MAX_TARGET) + bytes(u16(len(tb))) + tb
    parts, used = [], 0
    for p in pairs or []:
        if len(parts) >= MAX_PAIRS:
            break
        if not (isinstance(p, (list, tuple)) and len(p) >= 2 and isinstance(p[0], (str, bytes))
                and isinstance(p[1], (str, bytes))):
            continue
        term, meaning = utf8_cut(_b(p[0]), MAX_TERM), utf8_cut(_b(p[1]), MAX_MEANING)
        cost = 2 + len(term) + len(meaning)
        if used + cost > MAX_PAIR_BYTES:
            break
        parts.append(bytes([len(term)]) + term + bytes([len(meaning)]) + meaning)
        used += cost
    return out + bytes([len(parts)]) + b"".join(parts)


def msg_cancel(req_id, slot):
    return bytes([MSG_CANCEL] + u16(clamp(req_id, 0, 65535)) + [clamp(slot, 0, 255)])


def msg_clear():
    return bytes([MSG_CLEAR])


def msg_test(nonce):
    return bytes([MSG_TEST] + u16(clamp(nonce, 0, 65535)))


def msg_view(views):
    """views: list of dicts {id, x, y, w, h, fontPx, rowPx}; at most 4 are sent."""
    views = views or []
    n = min(len(views), MAX_VIEWS)
    out = [MSG_VIEW, n]
    for v in views[:n]:
        out += [clamp(v.get("id"), 0, 255)] + u16(clamp(v.get("x"), 0, 65535)) + u16(clamp(v.get("y"), 0, 65535))
        out += u16(clamp(v.get("w"), 0, 65535)) + u16(clamp(v.get("h"), 0, 65535))
        out += [clamp(v.get("fontPx"), 0, 255), clamp(v.get("rowPx"), 0, 255)]
    return bytes(out)


# ---------------------------------------------------------------- row tags
def crc5(payload10):
    c = 0x1F
    for i in range(9, -1, -1):
        bit = (payload10 >> i) & 1
        top = (c >> 4) & 1
        c = (c << 1) & 0x1F
        if bit != top:
            c ^= 0x05
    return c


def tag_payload(slot, row, sess_bit):
    return (int(slot) % 128) * 8 + (int(row) % 4) * 2 + int(sess_bit) % 2


def tag_value(slot, row, sess_bit):
    p = tag_payload(slot, row, sess_bit)
    return p * 32 + crc5(p)


def tag_digits(slot, row, sess_bit):
    v = tag_value(slot, row, sess_bit)
    d = []
    for _ in range(6):
        d.append(v % 6)
        v //= 6
    return d[::-1]


def tag_colors(slot, row, sess_bit):
    return [list(TAG_SYNC[0]), list(TAG_SYNC[1])] + [list(TAG_DIGIT[d]) for d in tag_digits(slot, row, sess_bit)]


def tag_markup(slot, row, sess_bit, w, h):
    w, h = clamp(w, 1, 4096), clamp(h, 1, 4096)
    return "".join(TEX_TAG % (h, w, c[0], c[1], c[2]) for c in tag_colors(slot, row, sess_bit))


def plate_markup(w, h):
    return TEX_TAG % (clamp(h, 1, 4096), clamp(w, 1, 4096), PLATE_RGB[0], PLATE_RGB[1], PLATE_RGB[2])


# ---------------------------------------------------------------- addon-side encoder
class Encoder:
    """Mirrors the Lua encoder (Bridge.NewEncoder) step for step. Times in ms."""

    def __init__(self, session, phys_w, phys_h, mode=0, max_cols=None, addon_build=0,
                 hold_ms=HOLD_MS, rot_hold_ms=ROT_HOLD_MS, expose_ms=EXPOSE_MS):
        self.session = clamp(session, 1, 65535)
        self.mode = mode if mode in MAGIC else MODE_DENSE
        self.addon_build = clamp(addon_build, 0, 65535)
        self.hold, self.rot_hold, self.expose = hold_ms, rot_hold_ms, expose_ms
        self.max_cols_opt = max_cols
        self.msgs = []           # [{id, msg, chunks: [{rec, exp, msg, order}]}]
        self.fresh = False
        self.seq = 0
        self.next_msg = 1
        self.dirty = True
        self.rotating = False
        self.last_tick = None
        self.frame_ticks = 0
        self.frame_start = None
        self.cur = []
        self.frame = b""
        self.symbols = []
        self.max_cols = None
        self.set_screen(phys_w, phys_h)

    def _chunks(self, mid, msg):
        size = max_chunk_data(self.max_cols, self.mode)
        count = max(1, -(-len(msg) // size))
        mcrc = u32(crc32(msg))
        out = []
        for i in range(count):
            data = msg[i * size:(i + 1) * size]
            rec = [REC_CHUNK] + u16(CHUNK_FIXED + len(data)) + u16(mid) + [i, count] + u16(len(msg)) + mcrc + list(data)
            out.append({"rec": rec, "exp": 0.0, "msg": mid, "order": 0})
        return out

    def fit(self, force):
        # never wider than the screen: blocks past the client's right edge are never captured
        cols = self.phys_w // BLOCK
        if self.max_cols_opt:
            cols = min(max(self.max_cols_opt, MIN_COLS[self.mode]), cols)
        cols = max(cols, MIN_SCREEN_COLS)
        if force or cols != self.max_cols:
            self.max_cols = cols
            for m in self.msgs:
                m["chunks"] = self._chunks(m["id"], m["msg"])
        self.max_cols = cols
        self.dirty = True

    def set_screen(self, phys_w, phys_h):
        self.phys_w, self.phys_h = clamp(phys_w, 0, 65535), clamp(phys_h, 0, 65535)
        self.fit(False)

    def set_mode(self, mode):
        if mode not in MAGIC or mode == self.mode:
            return
        self.mode = mode
        self.fit(True)

    def queue(self, msg):
        mid = self.next_msg
        self.next_msg = mid % 65535 + 1
        self.msgs.append({"id": mid, "msg": bytes(msg), "chunks": self._chunks(mid, bytes(msg))})
        self.dirty = True
        self.fresh = True
        return mid

    def is_idle(self):
        return not self.msgs

    def _done(self, m):
        return all(c["exp"] >= self.expose for c in m["chunks"])

    def on_show(self):
        self.last_tick = None
        self.frame_ticks = 0
        if self.frame_start is not None:
            self.frame_start = -1e18
        self.dirty = True

    def update(self, now):
        if self.last_tick is not None and self.frame_ticks >= 1:
            credit = min(now - self.last_tick, CREDIT_CAP_MS)
            for c in self.cur:
                c["exp"] += credit
        self.frame_ticks += 1
        self.last_tick = now
        if self.frame_start is not None:
            held = now - self.frame_start
            wait = self.rot_hold if (self.rotating and not self.fresh) else self.hold
            if held < wait:
                return False
            done = any(self._done(m) for m in self.msgs)
            if not (self.dirty or self.rotating or done):
                return False
            if done:
                self.msgs = [m for m in self.msgs if not self._done(m)]
        if not self.msgs:
            self.frame_start = None
            self.rotating = False
            self.cur = []
            return False
        self.compose()
        self.frame_start = now
        self.frame_ticks = 0
        self.dirty = False
        self.fresh = False
        return True

    def compose(self):
        self.seq = (self.seq + 1) % 256
        body = [REC_STATUS] + u16(STATUS_DATA) + u16(self.phys_w) + u16(self.phys_h) + u16(self.addon_build)
        room = frame_capacity(self.max_cols, self.mode) - CRC_LEN - HDR_LEN - len(body)
        flat = []
        for m in self.msgs:
            for c in m["chunks"]:
                flat.append(c)
                c["order"] = len(flat)
        total = sum(len(c["rec"]) for c in flat)
        if total <= room:
            self.rotating = False
            chosen = flat
        else:
            # least exposed first, first fit; the frame keeps message order
            self.rotating = True
            chosen, used = [], 0
            for c in sorted(flat, key=lambda c: (c["exp"], c["order"])):
                if used + len(c["rec"]) <= room:
                    chosen.append(c)
                    used += len(c["rec"])
            chosen.sort(key=lambda c: c["order"])
        for c in chosen:
            body += c["rec"]
        self.cur = chosen
        hdr = [MAGIC[self.mode], VERSION, self.seq, 0] + u16(self.session) + u16(len(body))
        fb = hdr + body
        fb += u32(crc32(fb))
        self.frame = bytes(fb)
        self.symbols = to_symbols(self.frame, self.mode)

    def block_count(self):
        return NCAL + len(self.symbols)

    def block_keys(self):
        return block_keys(self.symbols, self.mode)


def to_symbols(fb, mode):
    bps = BITS[mode]
    acc, nacc, out = 0, 0, []
    for b in fb:
        acc = (acc << 8) | b
        nacc += 8
        while nacc >= bps:
            nacc -= bps
            out.append((acc >> nacc) & ((1 << bps) - 1))
        acc &= (1 << nacc) - 1
    if nacc:
        out.append((acc << (bps - nacc)) & ((1 << bps) - 1))
    return out


def symbol_levels(sym, mode):
    """(R, G, B) level indices of a data symbol."""
    if mode == MODE_DENSE:
        return (sym >> 4) & 3, (sym >> 2) & 3, sym & 3
    if mode == MODE_ROBUST:
        return 3 * ((sym >> 2) & 1), 3 * ((sym >> 1) & 1), 3 * (sym & 1)
    return sym, sym, sym


def key_of_levels(lv):
    return lv[0] * 16 + lv[1] * 4 + lv[2]


def block_keys(symbols, mode):
    """Colour key of every block (Encoder:BlockKey): level indices packed r * 16 + g * 4 + b."""
    return [c * 21 for c in CALIB] + [key_of_levels(symbol_levels(s, mode)) for s in symbols]


def key_rgb(key):
    return (LEVELS[(key >> 4) & 3], LEVELS[(key >> 2) & 3], LEVELS[key & 3])


# ---------------------------------------------------------------- strip on the WoW side (visibility)
class Strip:
    """The strip frame of Bridge.CreateStrip: shown only while a message is queued, OnUpdate only
    while visible (own flag and UIParent shown). Used by the vector generator and the simulation."""

    def __init__(self, enc):
        self.enc = enc
        self.on = False          # the frame's own shown flag (f.on)
        self.parent = True       # UIParent shown (Alt-Z hides it)
        self.held = False
        self.loading = False

    def visible(self):
        return self.on and self.parent

    def _set_on(self, on):
        if on != self.on:
            self.on = on
            if on and self.parent:
                self.enc.on_show()

    def refresh(self):
        self._set_on(not (self.enc.is_idle() or self.held or self.loading))

    def send(self, msg):
        mid = self.enc.queue(msg)
        self.refresh()
        return mid

    def set_parent(self, shown):
        if shown != self.parent:
            self.parent = shown
            if shown and self.on:
                self.enc.on_show()

    def tick(self, now):
        """One game frame. Returns True when a new frame was drawn."""
        if not self.visible():
            return False
        did = self.enc.update(now)
        if self.enc.is_idle():
            self._set_on(False)
        return did


# ---------------------------------------------------------------- rendering (tests, simulation)
def render_blocks(keys):
    np = _numpy()
    return np.array([key_rgb(k) for k in keys], dtype=np.uint8)


def blocks_to_image(blocks, width, background, x_off=0):
    """4 x width x 3 image of the strip (blocks: n x 3 uint8) over a background."""
    np = _numpy()
    img = background.copy()
    n = len(blocks)
    ncols = -(-n // 2)
    padded = np.zeros((ncols * 2, 3), dtype=np.uint8)
    padded[:n] = blocks
    grid = padded.reshape(ncols, 2, 3).transpose(1, 0, 2)
    big = np.repeat(np.repeat(grid, BLOCK, axis=0), BLOCK, axis=1)
    if n % 2:  # last column has only row 0: background under row 1
        big[2:4, -2:] = background[2:4, 2 * ncols - 2:2 * ncols]
    wpx = min(width, 2 * ncols)
    img[:, :wpx] = big[:, :wpx]
    return img


# ---------------------------------------------------------------- helper-side decoder
PRESENCE_MIN = 16     # white - black per channel (presence; validity comes from the CRC)
CONSIST_MAX = 20      # |K0 - K3|, |W1 - W2|, |M1a - M1b|, |M2a - M2b|
GAP_MIN_DENSE = 8     # smallest observed adjacent-level gap for dense mode
HDR_SYMS = {m: -(-HDR_LEN * 8 // BITS[m]) for m in MODES}


def _phase_means(img):
    np = _numpy()
    w = img.shape[1] // 2
    f = img[:4, :2 * w].astype(np.float32)
    out = np.empty((4, 2 * w, 3), dtype=np.float32)
    for py in range(2):
        for px in range(2):
            sub = f[py::2, px::2]
            out[py * 2 + px] = sub.transpose(1, 0, 2).reshape(2 * w, 3)
    return out


def decode_capture(img):
    """img: (>= 4) x W x 3 uint8 capture of the client's top rows.
    Returns (status, frame_bytes, info); status in ok, absent, torn, nomagic, badver, toolong, crc."""
    np = _numpy()
    ph = _phase_means(img[:, :max(16, min(img.shape[1], 64))])
    cal = ph[:, :NCAL, :]
    contrast = np.minimum.reduce([cal[:, 1] - cal[:, 0], cal[:, 2] - cal[:, 0], cal[:, 1] - cal[:, 3],
                                  cal[:, 2] - cal[:, 3]]).min(axis=1)
    best = contrast.max()
    if best < PRESENCE_MIN:
        return "absent", None, None
    use = contrast >= 0.8 * best
    means = _phase_means(img)[use].mean(axis=0)
    c = means[:NCAL]
    k, wt, m1, m2 = (c[0] + c[3]) / 2, (c[1] + c[2]) / 2, (c[4] + c[7]) / 2, (c[5] + c[6]) / 2
    cons = max(np.abs(c[0] - c[3]).max(), np.abs(c[1] - c[2]).max(), np.abs(c[4] - c[7]).max(),
               np.abs(c[5] - c[6]).max())
    if cons > CONSIST_MAX:
        return "torn", None, None
    refs = np.stack([k, m1, m2, wt])                       # 4 levels x 3 channels
    min_gap = float(np.diff(refs, axis=0).min())
    data = means[NCAL:]
    info = {"min_gap": min_gap, "black": k.tolist(), "white": wt.tolist()}
    status = "nomagic"
    for mode in MODES:
        if mode == MODE_DENSE and min_gap < GAP_MIN_DENSE:
            continue
        bps = BITS[mode]
        if len(data) < HDR_SYMS[mode]:
            continue
        sy = classify(data[:HDR_SYMS[mode]], refs, mode)
        head = from_symbols(sy, bps, HDR_LEN)
        if head is None or head[0] != MAGIC[mode]:
            continue
        if head[1] != VERSION:
            return "badver", None, info
        total = HDR_LEN + ((head[6] << 8) | head[7]) + CRC_LEN
        nsym = -(-total * 8 // bps)
        if nsym > len(data):
            return "toolong", None, info
        tries = [classify(data[:nsym], refs, mode)]
        if mode != MODE_ROBUST:
            r2 = refine(c, data[:nsym], refs, tries[0], mode)
            if r2 is not None:
                tries.insert(0, classify(data[:nsym], r2, mode))
        status = "crc"
        for sy in tries:
            fb = from_symbols(sy, bps, total)
            if fb is not None and crc32(fb[:-4]) == int.from_bytes(bytes(fb[-4:]), "big"):
                info["mode"] = mode
                return "ok", bytes(fb), info
        return status, None, info
    return status, None, info


def classify(data, refs, mode):
    """Symbols (int array) of the data blocks, from the observed reference levels."""
    np = _numpy()
    if mode == MODE_DENSE:
        th = (refs[:-1] + refs[1:]) / 2                    # 3 thresholds x 3 channels
        lv = (data[:, None, :] > th[None, :, :]).sum(axis=1)
        return (lv[:, 0] << 4) | (lv[:, 1] << 2) | lv[:, 2]
    if mode == MODE_ROBUST:
        th = (refs[0] + refs[3]) / 2
        b = (data > th[None, :]).astype(np.int64)
        return (b[:, 0] << 2) | (b[:, 1] << 1) | b[:, 2]
    g = data.mean(axis=1)                                  # mono: channel mean
    rg = refs.mean(axis=1)
    th = (rg[:-1] + rg[1:]) / 2
    return (g[:, None] > th[None, :]).sum(axis=1)


def refine(calib, data, refs, syms, mode):
    """Decision-directed refinement: re-estimate each level from every block of this frame
    (calibration + data), per channel; None when the refined levels are not increasing."""
    np = _numpy()
    if mode == MODE_DENSE:
        lv = np.stack([(syms >> 4) & 3, (syms >> 2) & 3, syms & 3], axis=1)
    else:
        lv = np.stack([syms, syms, syms], axis=1)
    lv_all = np.concatenate([np.array([[x] * 3 for x in CALIB]), lv])
    vals = np.concatenate([calib, data])
    r2 = refs.copy()
    for li in range(4):
        for ch in range(3):
            sel = lv_all[:, ch] == li
            if sel.any():
                r2[li, ch] = vals[sel, ch].mean()
    if mode == MODE_MONO:
        ok = (np.diff(r2.mean(axis=1)) > 0).all()
    else:
        ok = (np.diff(r2, axis=0) > 0).all()
    return r2 if ok else None


def from_symbols(syms, bps, nbytes):
    acc, nacc, out = 0, 0, []
    for s in syms:
        acc = (acc << bps) | int(s)
        nacc += bps
        if nacc >= 8:
            nacc -= 8
            out.append((acc >> nacc) & 255)
            acc &= (1 << nacc) - 1
            if len(out) == nbytes:
                return out
    return out if len(out) == nbytes else None


def parse_frame(fb):
    """Parses a CRC-valid frame strictly. Raises ValueError on malformed content."""
    if fb[0] not in MAGIC.values() or fb[1] != VERSION:
        raise ValueError("magic")
    session = (fb[4] << 8) | fb[5]
    if session == 0:
        raise ValueError("session")
    hdr = {"mode": [m for m in MODES if MAGIC[m] == fb[0]][0], "seq": fb[2], "session": session}
    body_len = (fb[6] << 8) | fb[7]
    if HDR_LEN + body_len + CRC_LEN != len(fb):
        raise ValueError("length")
    p, end = HDR_LEN, HDR_LEN + body_len
    status = None
    chunks = []
    first = True
    while p < end:
        if p + REC_HDR > end:
            raise ValueError("rec hdr")
        t, ln = fb[p], (fb[p + 1] << 8) | fb[p + 2]
        if p + REC_HDR + ln > end:
            raise ValueError("rec len")
        d = fb[p + REC_HDR:p + REC_HDR + ln]
        p += REC_HDR + ln
        if t == REC_STATUS:
            if ln != STATUS_DATA or not first:
                raise ValueError("status")
            status = ((d[0] << 8) | d[1], (d[2] << 8) | d[3], (d[4] << 8) | d[5])
        elif t == REC_CHUNK:
            if ln < CHUNK_FIXED:
                raise ValueError("chunk")
            mid, idx, cnt = (d[0] << 8) | d[1], d[2], d[3]
            total, mcrc = (d[4] << 8) | d[5], int.from_bytes(bytes(d[6:10]), "big")
            data = bytes(d[10:])
            if cnt == 0 or idx >= cnt or len(data) > total:
                raise ValueError("chunk fields")
            chunks.append((mid, idx, cnt, total, mcrc, data))
        # other record types (0x02 = the v1 LAYOUT) are skipped
        first = False
    if status is None:
        raise ValueError("no status")
    hdr["physW"], hdr["physH"], hdr["addonBuild"] = status
    return hdr, chunks


class Reassembler:
    """Helper side: chunk reassembly, dedup, expiry and loss counters (mirrors tp_bridge::reasm)."""

    def __init__(self, partial_ttl_ms=5000, done_keep=512, missed_grace_ms=3000):
        self.session = None
        self.partial = {}
        self.done = {}
        self.done_keep = done_keep
        self.ttl = partial_ttl_ms
        self.grace = missed_grace_ms
        self.max_mid = None
        self.missing = {}
        self.stats = {"frames": 0, "chunks": 0, "duplicates": 0, "conflicts": 0, "messages": 0, "bad_crc": 0,
                      "parse_errors": 0, "expired": 0, "missed": 0, "session_changes": 0}

    def feed(self, hdr, chunks, now):
        """Returns [(msgId, message bytes)] completed by this frame."""
        st = self.stats
        st["frames"] += 1
        if hdr["session"] != self.session:
            if self.session is not None:
                st["session_changes"] += 1
            self.session = hdr["session"]
            self.partial.clear()
            self.done.clear()
            self.missing.clear()
            self.max_mid = None
        for mid in [m for m, p in self.partial.items() if now - p["t0"] > self.ttl]:
            del self.partial[mid]
            st["expired"] += 1
        for mid in [m for m, t in self.missing.items() if now - t > self.grace]:
            del self.missing[mid]
            st["missed"] += 1
        out = []
        for mid, idx, cnt, total, mcrc, data in chunks:
            st["chunks"] += 1
            if mid in self.done:
                st["duplicates"] += 1
                continue
            p = self.partial.get(mid)
            if p is not None and (p["cnt"], p["total"], p["crc"]) != (cnt, total, mcrc):
                st["conflicts"] += 1
                p = None
            if p is None:
                p = self.partial[mid] = {"cnt": cnt, "total": total, "crc": mcrc, "parts": {}, "t0": now}
            p["parts"][idx] = data
            if len(p["parts"]) == cnt:
                msg = b"".join(p["parts"][i] for i in range(cnt))
                del self.partial[mid]
                if len(msg) != total or crc32(msg) != mcrc:
                    st["bad_crc"] += 1
                    continue
                self.done[mid] = now
                if len(self.done) > self.done_keep:
                    self.done.pop(next(iter(self.done)))
                self.missing.pop(mid, None)
                if self.max_mid is not None and 0 < mid - self.max_mid <= 64:
                    for gap in range(self.max_mid + 1, mid):
                        if gap not in self.done:
                            self.missing.setdefault(gap, now)
                if self.max_mid is None or 0 < mid - self.max_mid <= 32768 or self.max_mid - mid > 32768:
                    self.max_mid = mid
                st["messages"] += 1
                out.append((mid, msg))
        return out


def is_language_code(b):
    """An explicit language code, as a reply's target must be: ASCII letters and '-', starting
    with a letter ("de", "zh-TW"). A placeholder such as "?" for an undecided language is not."""
    def letter(c):
        return 0x41 <= c <= 0x5A or 0x61 <= c <= 0x7A

    return len(b) > 0 and letter(b[0]) and all(letter(c) or c == 0x2D for c in b)


def parse_message(m):
    """Strict message parser (helper side). Returns a dict or raises ValueError."""
    p = 0

    def take(n):
        nonlocal p
        if p + n > len(m):
            raise ValueError("short")
        r = m[p:p + n]
        p += n
        return r

    def end():
        if p != len(m):
            raise ValueError("trailing")

    if not m:
        raise ValueError("empty")
    t = take(1)[0]
    if t == MSG_TRANSLATE:
        req = int.from_bytes(take(2), "big")
        slot, rows, flags = take(1)[0], take(1)[0], take(1)[0]
        if slot > 126 or not 1 <= rows <= MAX_ROWS or flags & ~FLAGS_MASK:
            raise ValueError("range")
        reply, copy, plate = bool(flags & FLAG_REPLY), bool(flags & FLAG_COPY), bool(flags & FLAG_PLATE)
        if copy and not reply:
            raise ValueError("copy without reply")
        if reply and (not plate or rows < REPLY_MIN_ROWS):
            raise ValueError("reply rules")
        tgt = take(take(1)[0])
        if not 1 <= len(tgt) <= MAX_TARGET or not all(0x21 <= b <= 0x7E for b in tgt):
            raise ValueError("target")
        if reply and not is_language_code(tgt):
            raise ValueError("reply target")
        text = take(int.from_bytes(take(2), "big"))
        if len(text) > MAX_TEXT:
            raise ValueError("text")
        n = take(1)[0]
        if n > MAX_PAIRS:
            raise ValueError("pairs")
        pairs, used = [], 0
        for _ in range(n):
            a = take(take(1)[0])
            b = take(take(1)[0])
            used += 2 + len(a) + len(b)
            if len(a) > MAX_TERM or len(b) > MAX_MEANING or used > MAX_PAIR_BYTES:
                raise ValueError("pair")
            pairs.append((a.decode("utf-8"), b.decode("utf-8")))
        end()
        return {"type": "translate", "req": req, "slot": slot, "rows": rows, "noCache": bool(flags & FLAG_NOCACHE),
                "plate": plate, "reply": reply, "copy": copy, "target": tgt.decode("ascii"),
                "text": text.decode("utf-8"), "pairs": pairs}
    if t == MSG_CANCEL:
        req = int.from_bytes(take(2), "big")
        slot = take(1)[0]
        end()
        if slot > TEST_SLOT:
            raise ValueError("range")
        return {"type": "cancel", "req": req, "slot": slot}
    if t == MSG_CLEAR:
        end()
        return {"type": "clear"}
    if t == MSG_TEST:
        nonce = int.from_bytes(take(2), "big")
        end()
        return {"type": "test", "nonce": nonce}
    if t == MSG_VIEW:
        n = take(1)[0]
        if n > MAX_VIEWS:
            raise ValueError("range")
        views = []
        for _ in range(n):
            v = take(11)
            views.append({"id": v[0], "x": (v[1] << 8) | v[2], "y": (v[3] << 8) | v[4], "w": (v[5] << 8) | v[6],
                          "h": (v[7] << 8) | v[8], "fontPx": v[9], "rowPx": v[10]})
        end()
        return {"type": "view", "views": views}
    raise ValueError("type")
