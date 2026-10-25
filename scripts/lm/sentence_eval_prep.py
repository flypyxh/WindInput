#!/usr/bin/env python3
"""把句子语料做成拼音整句评测集（`grammar_sentence_eval.rs::sentence_eval_file` 用）。

    python3 scripts/lm/sentence_eval_prep.py --in cmn_sentences.tsv \
        --dict base.dict.yaml --dict 8105.dict.yaml --out sentence_eval.tsv

输入每行一句（或 Tatoeba 的 `id<TAB>lang<TAB>句子`，取最后一列），繁转简（OpenCC t2s）后按标点
切成分句，留 4–16 个汉字的。用**输入法自己的词库**给 jieba 当词典切词，每个词取词库里的读音
拼出拼音（「甚么」先换成「什么」：繁转简留下的旧写法，输入法给「什么」是对的）——读音不唯一、又分不出主次（次读音权重超过主读音的 1/10）的词所在分句整句丢掉，
免得期望值本身拼错（「行」hang/xing 同权）。

输出 `类别<TAB>拼音<TAB>期望`，两类：

- `full`：全拼（`jintiantianqihenhao	今天天气很好`）；
- `mixed`：同一句里挑一个多字词换成首字母简拼（`jintiantqhenhao`），只在多于一个词时出。
  挑哪个词按句子散列定，重跑结果不变。

为什么不用手写用例：192 条手写用例上 bgw 从「+1」翻成「−4」（见
`docs/design/language-model-integration.md` §8.1），样本太少时结论跟着抽样走。

只依赖 jieba（MIT）与 opencc-python-reimplemented（Apache-2.0），都只在离线工具里用。
"""
import argparse
import random
import re
import sys
import tempfile
import os
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from corpus_segment import _read_dict  # noqa: E402

HAN = re.compile(r"^[一-鿿]+$")
SPLIT = re.compile(r"[^一-鿿]+")
MIN_CHARS, MAX_CHARS = 4, 16
# 次读音权重超过主读音的这个比例，就算分不出主次
AMBIGUOUS_RATIO = 0.1


def read_readings(paths):
    """{词: [(权重, 拼音音节表), …]}，按权重降序。"""
    out = {}
    for path in paths:
        body = False
        with open(path, encoding="utf-8") as f:
            for line in f:
                if not body:
                    body = line.startswith("...")
                    continue
                p = line.rstrip("\n").split("\t")
                if len(p) < 2 or not HAN.match(p[0]):
                    continue
                w = int(p[2]) if len(p) >= 3 and p[2].strip().isdigit() else 0
                out.setdefault(p[0], []).append((w, p[1].split()))
    for v in out.values():
        v.sort(key=lambda x: -x[0])
    return out


def reading(readings, word):
    r = readings.get(word)
    if not r or len(r[0][1]) != len(word):
        return None
    if len(r) > 1 and r[1][1] != r[0][1] and r[1][0] > r[0][0] * AMBIGUOUS_RATIO:
        return None
    return r[0][1]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True)
    ap.add_argument("--dict", action="append", required=True, help="词库 dict.yaml，可多次给")
    ap.add_argument("--out", required=True)
    ap.add_argument("--max", type=int, default=3000, help="最多留多少个分句（均匀抽样，种子固定）")
    a = ap.parse_args()

    import jieba
    import opencc

    t2s = opencc.OpenCC("t2s")
    readings = read_readings(a.dict)
    vocab = {}
    for d in a.dict:
        for w, wt in _read_dict(d).items():
            if HAN.match(w) and vocab.get(w, -1) < wt:
                vocab[w] = wt
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
        for w, wt in vocab.items():
            f.write(f"{w} {max(wt, 1)}\n")
        jd = f.name
    try:
        jieba.setLogLevel(60)
        tk = jieba.Tokenizer(dictionary=jd)
        tk.initialize()
    finally:
        os.unlink(jd)

    clauses = set()
    with open(a.inp, encoding="utf-8") as f:
        for line in f:
            s = t2s.convert(line.rstrip("\n").split("\t")[-1]).replace("甚么", "什么")
            for c in SPLIT.split(s):
                if MIN_CHARS <= len(c) <= MAX_CHARS:
                    clauses.add(c)
    clauses = sorted(clauses)
    random.Random(42).shuffle(clauses)

    kept, dropped, rows = 0, 0, []
    for c in clauses:
        if kept >= a.max:
            break
        words = list(tk.cut(c, HMM=False))
        syl = [reading(readings, w) for w in words]
        if any(s is None for s in syl):
            dropped += 1
            continue
        kept += 1
        rows.append(("full", "".join("".join(s) for s in syl), c))
        multi = [i for i, s in enumerate(syl) if len(s) >= 2]
        if len(words) > 1 and multi:
            pick = multi[zlib.crc32(c.encode()) % len(multi)]
            code = "".join(
                "".join(x[0] for x in s) if i == pick else "".join(s) for i, s in enumerate(syl)
            )
            rows.append(("mixed", code, c))

    with open(a.out, "w", encoding="utf-8") as w:
        for r in rows:
            w.write("\t".join(r) + "\n")
    print(
        f"分句 {len(clauses)}，读音分不清丢 {dropped}，留 {kept} → {len(rows)} 条 → {a.out}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
