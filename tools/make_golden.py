"""Write reference translations of test lines with tools/marian_ref.py.

Usage: python -I -X utf8 tools/make_golden.py <models_dir> <pair> <out.json> "line 1" "line 2" ...
Each entry: text, src_ids (with end mark), shortlist size, out_ids, translation. The Lua engine
must reproduce src_ids and out_ids exactly (tests/native/run_engine.py).
"""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import marian_ref as mr  # noqa: E402


def main():
    models_dir, pair, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    code = pair.replace('-', '')
    base = os.path.join(models_dir, pair)
    with open(os.path.join(base, 'model.%s.intgemm.alphas.bin' % code), 'rb') as f:
        model = mr.Model(mr.read_marian_bin(f.read()))
    with open(os.path.join(base, 'vocab.%s.spm' % code), 'rb') as f:
        spm = mr.SentencePiece(f.read())
    with open(os.path.join(base, 'lex.50.50.%s.s2t.bin' % code), 'rb') as f:
        sl = mr.read_shortlist_bin(f.read())
    entries = []
    for text in sys.argv[4:]:
        t0 = time.time()
        ids = spm.encode(text) + [spm.eos_id]
        short = mr.shortlist_generate(sl, ids, model.V)
        out = model.translate_ids(ids, short, eos_id=spm.eos_id, unk_id=spm.unk_id)
        entry = {'text': text, 'src_ids': ids, 'shortlist': len(short), 'out_ids': out,
                 'translation': spm.decode(out)}
        entries.append(entry)
        print('%5.1fs %s -> %s' % (time.time() - t0, text, entry['translation']), flush=True)
    with open(out_path, 'w', encoding='utf-8') as f:
        json.dump({'pair': pair, 'entries': entries}, f, ensure_ascii=False, indent=1)


if __name__ == '__main__':
    main()
