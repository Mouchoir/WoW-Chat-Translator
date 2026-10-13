"""Pure-Python reference of Bergamot/Marian "tiny" student inference (no numpy, no sentencepiece).

Mirrors (cited in the research notes):
  marian-dev  browsermt/marian-dev@2781d735  src/models/transformer.h, src/rnn/cells.h (SSRU),
              src/layers/generic.{h,cpp}, src/graph/node_initializers.cpp, src/common/binary.cpp,
              src/tensors/cpu/{intgemm_interface.h,integer_common.h,expression_graph_packable.h},
              src/data/shortlist.{h,cpp}, src/translator/beam_search.cpp
  sentencepiece browsermt/sentencepiece@ae41b774 (normalizer.cc, unigram_model.cc EncodeOptimized)
Everything is float64 here; Marian runs float32 + int8 GEMM, so results match to ~1e-5, not bitwise.
"""
import math
import struct

# ----------------------------------------------------------------------------------------------
# 1. Marian binary model (.bin, BINARY_FILE_VERSION 1)
# ----------------------------------------------------------------------------------------------
T_INT8, T_FLOAT32, T_INTGEMM8 = 0x0101, 0x0404, 0x4101


def read_marian_bin(buf):
    """-> {name: (type, shape, raw_bytes)}  (common/binary.cpp loadItems)."""
    ver, n = struct.unpack_from('<QQ', buf, 0)
    assert ver == 1, ver
    off = 16
    hdrs = [struct.unpack_from('<QQQQ', buf, off + 32 * i) for i in range(n)]  # nameLen,type,shapeLen,dataLen
    off += 32 * n
    names = []
    for nl, _, _, _ in hdrs:
        names.append(buf[off:off + nl - 1].decode('utf-8'))  # nameLen includes the NUL
        off += nl
    shapes = []
    for _, _, sl, _ in hdrs:
        shapes.append(list(struct.unpack_from('<%di' % sl, buf, off)))
        off += 4 * sl
    (pad,) = struct.unpack_from('<Q', buf, off)  # padding so that data starts 256-aligned
    off += 8 + pad
    items = {}
    for name, (nl, t, sl, dl), shape in zip(names, hdrs, shapes):
        items[name] = (t, shape, buf[off:off + dl])
        off += dl
    return items


def _prod(s):
    p = 1
    for v in s:
        p *= v
    return p


def item_floats(item):
    t, shape, raw = item
    assert t == T_FLOAT32
    n = _prod(shape)
    return list(struct.unpack_from('<%df' % n, raw, 0))


def item_intgemm8(item):
    """int8 tensor + trailing float32 quantMult (expression_graph_packable.h packAndSave).
    For every '..._W*' matrix of logical shape [in, out] the bytes are W^T row-major: byte[j*in + k] = Wq[k][j].
    For 'Wemb' [V, D] the bytes are NOT transposed: byte[v*D + d]."""
    t, shape, raw = item
    assert t == T_INTGEMM8
    n = _prod(shape)
    q = struct.unpack_from('<%db' % n, raw, 0)
    (qmult,) = struct.unpack_from('<f', raw, n)
    return q, qmult


def parse_model_yml(text):
    cfg = {}
    for line in text.splitlines():
        if ':' in line and not line.startswith(' ') and not line.startswith('-'):
            k, v = line.split(':', 1)
            cfg[k.strip()] = v.strip().strip('"').strip("'")
    return cfg


class Linear:
    """y = x @ W + b with Marian's int8 'shifted, precomputed alpha' semantics (intgemm_interface.h).
    int8 path : qa_k = clamp(rne(x_k * alphaA), -127, 127); y_j = (sum_k qa_k*Wq[j][k]) / (alphaA*qB) + b_j
    float path: y_j = sum_k x_k * (Wq[j][k]/qB) + b_j            (dequantized weights, float activations)"""

    def __init__(self, rows_out, qB, bias, alphaA, n_in):
        self.rows = rows_out          # list over output j of tuple(int8) length n_in
        self.qB = qB
        self.b = bias                 # list or None (SSRU _W has no bias)
        self.alpha = alphaA
        self.n_in = n_in

    def __call__(self, x, int8=True):
        if int8:
            a = self.alpha
            qa = [max(-127, min(127, round(v * a))) for v in x]  # round(): half-to-even like cvtps_epi32
            s = 1.0 / (a * self.qB)
            out = [sum(p * w for p, w in zip(qa, r)) * s for r in self.rows]
        else:
            s = 1.0 / self.qB
            out = [sum(p * w for p, w in zip(x, r)) * s for r in self.rows]
        if self.b is not None:
            out = [o + bb for o, bb in zip(out, self.b)]
        return out


def layer_norm(x, g, b, eps=1e-6):  # generic.h layerNorm(prefix) -> eps 1e-6; tensor_operators.cpp
    n = len(x)
    m = sum(x) / n
    var = sum((v - m) ** 2 for v in x) / n
    sd = math.sqrt(var + eps)
    return [gg * ((v - m) / sd) + bb for v, gg, bb in zip(x, g, b)]


def pos_emb(p, D):  # node_initializers.cpp sinusoidalPositionEmbeddings
    n = D // 2
    inc = math.log(10000.0) / (n - 1)
    ang = [p * math.exp(-i * inc) for i in range(n)]
    return [math.sin(a) for a in ang] + [math.cos(a) for a in ang]


def softmax(z):
    m = max(z)
    e = [math.exp(v - m) for v in z]
    s = sum(e)
    return [v / s for v in e]


def relu(v):
    return [x if x > 0 else 0.0 for x in v]


def sigmoid(x):
    return 1.0 / (1.0 + math.exp(-x))


class Model:
    def __init__(self, items):
        self.items = items
        self.cfg = {}
        if 'special:model.yml' in items:
            raw = items['special:model.yml'][2]
            self.cfg = parse_model_yml(raw.split(b'\0')[0].decode('utf-8'))
        q, qm = item_intgemm8(items['Wemb'])
        self.V, self.D = items['Wemb'][1]
        D = self.D
        self.Wemb_q = [q[v * D:(v + 1) * D] for v in range(self.V)]   # int8 rows
        self.Wemb_qm = qm
        inv = 1.0 / qm                                                  # integer_common.h unquantizeWemb
        self.E = lambda v: [w * inv for w in self.Wemb_q[v]]
        self.out_b = item_floats(items['decoder_ff_logit_out_b'])
        self.out_alpha = None
        if 'Wemb_QuantMultA' in items:
            it = items['Wemb_QuantMultA']
            if it[0] == T_INTGEMM8:      # 2022 models pack this scalar like a matrix: value = q / qB
                qa, qam = item_intgemm8(it)
                self.out_alpha = qa[0] / qam
            else:
                self.out_alpha = item_floats(it)[0]
        self.H = int(self.cfg.get('transformer-heads', 8))
        self.enc_depth = sum(1 for k in items if k.startswith('encoder_l') and k.endswith('_self_Wq'))
        self.dec_depth = sum(1 for k in items if k.startswith('decoder_l') and k.endswith('_rnn_W'))

    def lin(self, wname, bname):
        it = self.items
        shape = it[wname][1]
        n_in, n_out = shape[0], shape[1]
        q, qB = item_intgemm8(it[wname])
        rows = [q[j * n_in:(j + 1) * n_in] for j in range(n_out)]
        b = item_floats(it[bname]) if bname else None
        aname = wname + '_QuantMultA'
        alpha = item_floats(it[aname])[0] if aname in it else None
        return Linear(rows, qB, b, alpha, n_in)

    def ln(self, prefix):
        return item_floats(self.items[prefix + '_ln_scale']), item_floats(self.items[prefix + '_ln_bias'])

    # -- attention (transformer.h MultiHead/Attention), heads = contiguous slices of dk columns
    def mha(self, Q, K, V):
        D, H = self.D, self.H
        dk = D // H
        scale = 1.0 / math.sqrt(dk)
        out = []
        for qi in Q:
            row = []
            for h in range(H):
                s0 = h * dk
                qh = qi[s0:s0 + dk]
                z = [scale * sum(a * b for a, b in zip(qh, kj[s0:s0 + dk])) for kj in K]
                w = softmax(z)               # single sentence: mask is all ones -> +0
                row += [sum(w[j] * V[j][s0 + d] for j in range(len(V))) for d in range(dk)]
            out.append(row)
        return out

    def encode(self, ids, int8=True):
        D = self.D
        x = [[e * math.sqrt(D) + p for e, p in zip(self.E(t), pos_emb(i, D))] for i, t in enumerate(ids)]
        for l in range(1, self.enc_depth + 1):
            pf = 'encoder_l%d' % l
            Wq, Wk, Wv, Wo = (self.lin(pf + '_self_W' + c, pf + '_self_b' + c) for c in 'qkvo')
            Q = [Wq(v, int8) for v in x]
            K = [Wk(v, int8) for v in x]
            Vv = [Wv(v, int8) for v in x]
            att = self.mha(Q, K, Vv)
            g, b = self.ln(pf + '_self_Wo')
            x = [layer_norm([a + r for a, r in zip(Wo(o, int8), xi)], g, b) for o, xi in zip(att, x)]
            W1 = self.lin(pf + '_ffn_W1', pf + '_ffn_b1')
            W2 = self.lin(pf + '_ffn_W2', pf + '_ffn_b2')
            g, b = self.ln(pf + '_ffn_ffn')
            x = [layer_norm([a + r for a, r in zip(W2(relu(W1(xi, int8)), int8), xi)], g, b) for xi in x]
        return x  # transformer-postprocess-top is "" -> no final norm

    def decoder_init(self, enc, int8=True):
        st = {'c': [[0.0] * self.D for _ in range(self.dec_depth)], 't': 0, 'KV': []}
        for l in range(1, self.dec_depth + 1):
            pf = 'decoder_l%d_context' % l
            Wk, Wv = self.lin(pf + '_Wk', pf + '_bk'), self.lin(pf + '_Wv', pf + '_bv')
            st['KV'].append(([Wk(v, int8) for v in enc], [Wv(v, int8) for v in enc]))
        return st

    def decoder_step(self, st, prev_id, shortlist, int8=True):
        D = self.D
        t = st['t']
        e = [0.0] * D if prev_id is None else self.E(prev_id)    # decoder.h: first step embeds zeros, no BOS
        q = [v * math.sqrt(D) + p for v, p in zip(e, pos_emb(t, D))]
        for l in range(1, self.dec_depth + 1):
            pf = 'decoder_l%d' % l
            # SSRU (rnn/cells.h): c = sig(f)*c_prev + (1-sig(f))*x ; h = relu(c); f = q Wf + bf ; x = q W
            xw = self.lin(pf + '_rnn_W', None)(q, int8)
            f = self.lin(pf + '_rnn_Wf', pf + '_rnn_bf')(q, int8)
            c = [sigmoid(fi) * cp + (1.0 - sigmoid(fi)) * xi for fi, cp, xi in zip(f, st['c'][l - 1], xw)]
            st['c'][l - 1] = c
            g, b = self.ln(pf + '_rnn_ffn')                     # postprocess "dan" with prefix+"_ffn"
            q = layer_norm([h + r for h, r in zip(relu(c), q)], g, b)
            # cross attention
            K, Vv = st['KV'][l - 1]
            Wq = self.lin(pf + '_context_Wq', pf + '_context_bq')
            Wo = self.lin(pf + '_context_Wo', pf + '_context_bo')
            o = Wo(self.mha([Wq(q, int8)], K, Vv)[0], int8)
            g, b = self.ln(pf + '_context_Wo')
            q = layer_norm([a + r for a, r in zip(o, q)], g, b)
            # FFN
            W1 = self.lin(pf + '_ffn_W1', pf + '_ffn_b1')
            W2 = self.lin(pf + '_ffn_W2', pf + '_ffn_b2')
            g, b = self.ln(pf + '_ffn_ffn')
            q = layer_norm([a + r for a, r in zip(W2(relu(W1(q, int8)), int8), q)], g, b)
        st['t'] = t + 1
        # output layer: tied Wemb (transposed) + decoder_ff_logit_out_b, restricted to shortlist
        if int8 and self.out_alpha is not None:
            a = self.out_alpha
            qa = [max(-127, min(127, round(v * a))) for v in q]
            s = 1.0 / (a * self.Wemb_qm)
            return [(j, sum(x * w for x, w in zip(qa, self.Wemb_q[j])) * s + self.out_b[j]) for j in shortlist]
        s = 1.0 / self.Wemb_qm
        return [(j, sum(x * w for x, w in zip(q, self.Wemb_q[j])) * s + self.out_b[j]) for j in shortlist]

    def translate_ids(self, src_ids, shortlist, eos_id=0, unk_id=1, max_len_factor=2.0, int8=True):
        """src_ids must already end with eos_id. Greedy (beam-size 1), unk suppressed (allow-unk false)."""
        enc = self.encode(src_ids, int8)
        st = self.decoder_init(enc, int8)
        out, prev = [], None
        while True:
            scored = self.decoder_step(st, prev, shortlist, int8)
            best_j, best_s = None, -float('inf')
            for j, s in scored:
                if j == unk_id:
                    continue
                if s > best_s:
                    best_j, best_s = j, s
            if best_j == eos_id:
                break
            out.append(best_j)
            prev = best_j
            if len(out) >= max_len_factor * len(src_ids):  # beam_search.cpp: history size >= factor*srcLen
                break
        return out


# ----------------------------------------------------------------------------------------------
# 2. Binary lexical shortlist (data/shortlist.{h,cpp})
# ----------------------------------------------------------------------------------------------
SHORTLIST_MAGIC = 0xF11A48D5013417F5


def read_shortlist_bin(buf):
    magic, checksum, firstNum, bestNum, w2oSize, slSize = struct.unpack_from('<6Q', buf, 0)
    assert magic == SHORTLIST_MAGIC
    off = 48
    w2o = struct.unpack_from('<%dQ' % w2oSize, buf, off)
    off += 8 * w2oSize
    lists = struct.unpack_from('<%dI' % slSize, buf, off)
    return {'firstNum': firstNum, 'bestNum': bestNum, 'w2o': w2o, 'lists': lists}


def shortlist_generate(sl, src_ids, trg_vocab_size, shared_vocab=True):
    """BinaryShortlistGenerator::generate. src_ids include the trailing EOS."""
    sel = set(range(min(sl['firstNum'], trg_vocab_size)))
    for s in set(src_ids):
        if shared_vocab:          # bergamot: shared when one vocab file serves src and trg
            sel.add(s)
        sel.update(sl['lists'][sl['w2o'][s]:sl['w2o'][s + 1]])
    i = sl['firstNum']
    while len(sel) % 8 and i < trg_vocab_size:   # pad to a multiple of 8 with the next unused ids
        sel.add(i)
        i += 1
    return sorted(sel)


# ----------------------------------------------------------------------------------------------
# 3. SentencePiece .spm (ModelProto) without the library
# ----------------------------------------------------------------------------------------------
def _varint(b, o):
    r = s = 0
    while True:
        c = b[o]
        o += 1
        r |= (c & 0x7F) << s
        if c < 0x80:
            return r, o
        s += 7


def pb_fields(b):
    o, n = 0, len(b)
    while o < n:
        k, o = _varint(b, o)
        fn, wt = k >> 3, k & 7
        if wt == 0:
            v, o = _varint(b, o)
        elif wt == 1:
            v = b[o:o + 8]
            o += 8
        elif wt == 2:
            ln, o = _varint(b, o)
            v = b[o:o + ln]
            o += ln
        elif wt == 5:
            v = b[o:o + 4]
            o += 4
        else:
            raise ValueError('wire type %d' % wt)
        yield fn, wt, v


NORMAL, UNKNOWN, CONTROL, USER_DEFINED, UNUSED, BYTE = 1, 2, 3, 4, 5, 6  # ModelProto.SentencePiece.Type


def parse_spm(buf):
    pieces = []  # (piece_bytes, score, type)
    trainer = {}
    norm = {'add_dummy_prefix': True, 'remove_extra_whitespaces': True, 'escape_whitespaces': True,
            'precompiled_charsmap': b'', 'name': ''}
    for fn, wt, v in pb_fields(buf):
        if fn == 1:  # repeated SentencePiece
            p, sc, ty = b'', 0.0, NORMAL
            for f2, w2, v2 in pb_fields(v):
                if f2 == 1:
                    p = bytes(v2)
                elif f2 == 2:
                    (sc,) = struct.unpack('<f', v2)
                elif f2 == 3:
                    ty = v2
            pieces.append((p, sc, ty))
        elif fn == 2:  # TrainerSpec
            for f2, w2, v2 in pb_fields(v):
                if f2 in (3, 4, 24, 35, 40, 41, 42, 43):
                    if f2 >= 40 and v2 >= 1 << 63:
                        v2 -= 1 << 64                     # int32 -1 is encoded as 10-byte varint
                    trainer[{3: 'model_type', 4: 'vocab_size', 24: 'treat_whitespace_as_suffix',
                             35: 'byte_fallback', 40: 'unk_id', 41: 'bos_id', 42: 'eos_id', 43: 'pad_id'}[f2]] = v2
                elif f2 == 44:
                    trainer['unk_surface'] = bytes(v2)
        elif fn == 3:  # NormalizerSpec
            for f2, w2, v2 in pb_fields(v):
                if f2 == 1:
                    norm['name'] = bytes(v2).decode()
                elif f2 == 2:
                    norm['precompiled_charsmap'] = bytes(v2)
                elif f2 == 3:
                    norm['add_dummy_prefix'] = bool(v2)
                elif f2 == 4:
                    norm['remove_extra_whitespaces'] = bool(v2)
                elif f2 == 5:
                    norm['escape_whitespaces'] = bool(v2)
                elif f2 in (7, 8):                        # browsermt fork: encode_case / decode_case
                    norm[{7: 'encode_case', 8: 'decode_case'}[f2]] = bool(v2)
    return pieces, trainer, norm


def expand_charsmap(blob):
    """precompiled_charsmap = <uint32 trie_size><Darts double array><NUL-separated replacement pool>.
    Returns {input_bytes: replacement_bytes} by enumerating every key of the double array."""
    if not blob:
        return {}
    (tsize,) = struct.unpack_from('<I', blob, 0)
    units = struct.unpack_from('<%dI' % (tsize // 4), blob, 4)
    pool = blob[4 + tsize:]

    def offset(u):
        return (u >> 10) << ((u & (1 << 9)) >> 6)

    def label(u):
        return u & ((1 << 31) | 0xFF)

    def has_leaf(u):
        return (u >> 8) & 1

    out = {}
    stack = [(0, b'')]
    while stack:
        pos, key = stack.pop()
        base = pos ^ offset(units[pos])
        for c in range(1, 256):
            ch = base ^ c
            if ch >= len(units) or label(units[ch]) != c:
                continue
            k2 = key + bytes([c])
            u = units[ch]
            if has_leaf(u):
                val = units[ch ^ offset(u)] & 0x7FFFFFFF
                end = pool.index(b'\0', val)
                out[k2] = pool[val:end]
            stack.append((ch, k2))
    return out


def utf8_len(b, i):
    """Length of the valid UTF-8 char at b[i], or 0 if malformed."""
    c = b[i]
    if c < 0x80:
        return 1
    for n, lo, hi in ((2, 0xC2, 0xDF), (3, 0xE0, 0xEF), (4, 0xF0, 0xF4)):
        if lo <= c <= hi:
            if i + n > len(b) or any((b[i + k] & 0xC0) != 0x80 for k in range(1, n)):
                return 0
            cp = c & (0x7F >> n)
            for k in range(1, n):
                cp = (cp << 6) | (b[i + k] & 0x3F)
            if (n == 3 and (cp < 0x800 or 0xD800 <= cp <= 0xDFFF)) or (n == 4 and not 0x10000 <= cp <= 0x10FFFF):
                return 0
            return n
    return 0


class SentencePiece:
    SP = '▁'.encode()

    def __init__(self, buf):
        self.pieces, self.trainer, self.norm = parse_spm(buf)
        self.cmap = expand_charsmap(self.norm['precompiled_charsmap'])
        self.cmap_max = max((len(k) for k in self.cmap), default=0)
        self.vocab, self.prefixes = {}, set()
        self.unk_id = self.eos_id = None
        scores_normal = []
        for i, (p, sc, ty) in enumerate(self.pieces):
            if ty in (NORMAL, USER_DEFINED, UNUSED):
                self.vocab[p] = i
                for k in range(1, len(p) + 1):
                    self.prefixes.add(p[:k])
            if ty == NORMAL:
                scores_normal.append(sc)
            if ty == UNKNOWN:
                self.unk_id = i
            if ty == CONTROL and p == b'</s>':
                self.eos_id = i
        self.min_score, self.max_score = min(scores_normal), max(scores_normal)
        self.max_piece = max(len(p) for p in self.vocab)
        self.byte_ids = {}
        for i, (p, sc, ty) in enumerate(self.pieces):
            if ty == BYTE:
                self.byte_ids[int(p[3:5], 16)] = i  # "<0xAB>"

    def _norm_prefix(self, b, i):
        for L in range(min(self.cmap_max, len(b) - i), 0, -1):
            r = self.cmap.get(b[i:i + L])
            if r is not None:
                return r, L
        n = utf8_len(b, i)
        if n == 0:
            return '�'.encode(), 1
        return b[i:i + n], n

    def normalize(self, text):
        b, i, n = text.encode('utf-8'), 0, None
        rew = self.norm['remove_extra_whitespaces']
        sp = self.SP if self.norm['escape_whitespaces'] else b' '
        if rew:
            while i < len(b):
                r, n = self._norm_prefix(b, i)
                if r != b' ':
                    break
                i += n
        if i >= len(b):
            return b''
        out = bytearray()
        if self.norm['add_dummy_prefix'] and not self.trainer.get('treat_whitespace_as_suffix'):
            out += sp
        prev_space = rew
        while i < len(b):
            r, n = self._norm_prefix(b, i)
            if prev_space:
                r = r.lstrip(b' ')
            if r:
                out += r.replace(b' ', sp)
                prev_space = r.endswith(b' ')
            i += n
            if not rew:
                prev_space = False
        if rew:
            while out.endswith(sp):
                del out[-len(sp):]
        return bytes(out)

    def encode(self, text):
        s = self.normalize(text)
        size = len(s)
        unk_score = self.min_score - 10.0                     # kUnkPenalty
        best = [[None, 0.0, -1] for _ in range(size + 1)]     # id, score, starts_at
        st = 0
        while st < size:
            base = best[st][1]
            mblen = min((1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 3, 4)[s[st] >> 4], size - st)  # OneCharLen
            single = False
            for e in range(st + 1, min(size, st + self.max_piece) + 1):
                p = s[st:e]
                if p not in self.prefixes:
                    break
                pid = self.vocab.get(p)
                if pid is None or self.pieces[pid][2] == UNUSED:
                    continue
                ty = self.pieces[pid][2]
                sc = (len(p) * self.max_score - 0.1) if ty == USER_DEFINED else self.pieces[pid][1]
                node = best[e]
                if node[2] == -1 or sc + base > node[1]:
                    best[e] = [pid, sc + base, st]
                if len(p) == mblen:
                    single = True
            if not single:
                node = best[st + mblen]
                if node[2] == -1 or unk_score + base > node[1]:
                    best[st + mblen] = [self.unk_id, unk_score + base, st]
            st += mblen
        res, e = [], size
        while e > 0:
            pid, _, st = best[e]
            res.append((s[st:e], pid))
            e = st
        res.reverse()
        ids, prev_unk = [], False
        for piece, pid in res:                                 # byte fallback / merge runs of <unk>
            is_unk = pid == self.unk_id
            if is_unk and self.trainer.get('byte_fallback'):
                ids += [self.byte_ids[c] for c in piece]
            elif not (is_unk and prev_unk):
                ids.append(pid)
            prev_unk = is_unk
        return ids

    def decode(self, ids):
        out, pend, first = bytearray(), bytearray(), True

        def flush():
            i = 0
            while i < len(pend):
                n = utf8_len(pend, i)
                out.extend(pend[i:i + n] if n else '�'.encode())
                i += n or 1
            pend.clear()
        for i in ids:
            p, sc, ty = self.pieces[i]
            if ty == CONTROL:
                continue
            if ty == BYTE:
                pend.append(int(p[3:5], 16))
                first = False
                continue
            flush()
            if ty == UNKNOWN:
                out += self.trainer.get('unk_surface', ' ⁇ '.encode())
            else:
                if first and self.norm['add_dummy_prefix'] and p.startswith(self.SP):
                    p = p[len(self.SP):]
                out += p.replace(self.SP, b' ')
            first = False
        flush()
        return out.decode('utf-8', 'replace')
