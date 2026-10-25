#!/usr/bin/env python3
"""把句子语料切成联想评测事件（「打完这段，下一个想要的是什么」）。

    python3 scripts/lm/assoc_eval_prep.py --in sentences.txt --out events.tsv [--unit word|char]

输入每行一句（或 Tatoeba 的 `id<TAB>lang<TAB>句子`，取最后一列）。繁体先转简体（OpenCC t2s），
去掉空白。按 `--unit` 模拟用户怎么上屏：

- `word`：一次上屏一个词（拼音整词打）——jieba 切词，词边界即上屏边界；
- `char`：一次上屏一个字（五笔逐字打）。

每个上屏的汉字段是一个事件，输出 `前文<TAB>上屏<TAB>后文`：前文 = 本句里它之前的部分，
后文 = 它之后的原文（最多 8 字，含标点）。评测端的判据是「后文以联想候选开头」，所以与切词
口径无关——联想给「们」还是「们的」都能判对，句末给「？」也算。后文为空（句子到头）的不出。

只依赖 jieba（MIT）与 opencc-python-reimplemented（Apache-2.0），都只在离线工具里用。
"""
import argparse
import random
import re
import sys

HAN = re.compile(r"^[一-鿿]+$")
REST_CHARS = 8


def sentences(path, t2s):
    with open(path, encoding="utf-8") as f:
        for line in f:
            s = line.rstrip("\n").split("\t")[-1]
            s = re.sub(r"\s+", "", t2s.convert(s))
            if s and any("一" <= c <= "鿿" for c in s):
                yield s


def events(s, unit, cut):
    units = cut(s) if unit == "word" else list(s)
    pos = 0
    for u in units:
        start, pos = pos, pos + len(u)
        rest = s[pos : pos + REST_CHARS]
        if HAN.match(u) and rest:
            yield s[:start], u, rest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--unit", choices=["word", "char"], default="word")
    ap.add_argument("--max", type=int, default=20000, help="最多留多少事件（均匀抽样，种子固定）")
    a = ap.parse_args()

    import jieba
    import opencc

    jieba.setLogLevel(60)
    t2s = opencc.OpenCC("t2s")
    rows = []
    for s in sentences(a.inp, t2s):
        rows.extend(events(s, a.unit, jieba.lcut))
    total = len(rows)
    if total > a.max:
        rows = random.Random(20261022).sample(rows, a.max)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        for r in rows:
            f.write("\t".join(r) + "\n")
    print(f"{total} 个事件，写出 {len(rows)} → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
