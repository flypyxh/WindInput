#!/usr/bin/env python3
"""给联想「下一词」表补上统计出的标点：表里的 `$`（句末记号）换成语料里真实的句读。

    python3 scripts/lm/distill_assoc_punct.py --base assoc_bigram.bgw.txt \
        --counts zhwiki.counts.tsv --dict base.dict.yaml --dict 8105.dict.yaml \
        --out data/assoc/assoc_bigram.txt

`--base` 是 `distill_assoc_bigram.py` 从 bgw 蒸馏的表：bgw 是网页语料，词的后继口语化，但
不含任何标点，只有一个 `$`（读端一律当「，」）。`--counts` 是 `ngram_count.py` 在保留了句读的
维基上数的后继。对表里每个上文：把它在维基上的后继按同一套 `_mix` 排好，取排在最前的那个
句读和它的名次，替掉 `$` 插回原表——「吗」后给「？」由此而来。
维基里没有这个上文的，行原样保留。

为什么只取维基的标点、不取它的词：评测（Tatoeba，带前文）上整份换成维基的表，标点命中
从 22% 涨到 70%，但词的 Top3 从 18.4% 掉到 8.4%——维基是书面语（「的」后面是「时区」
「市镇」）。只补标点：整词上屏 Top3 19.1% → 21.2%，逐字上屏 31.4% → 32.5%，词的命中不掉。
补两个标点 Top3 还能再涨几个点，但第二、三名常常都成了标点，挤掉词，没取。

维基是 CC BY-SA 4.0：产物头注释写明来源。
"""
import argparse
import hashlib
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from distill_assoc_bigram import (  # noqa: E402
    CHAR_WISE_MARK,
    SCORE_SCALE,
    _is_han,
    _mix,
    _read_dict,
)

PUNCT = set("，。！？、；：…")
SENTENCE_END = "$"


def _read_counts(path):
    """→ {上文: (次数, [(后继, 次数)…])}"""
    out = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            ctx, total, succ = line.rstrip("\n").split("\t")
            pairs = []
            for item in succ.split(" ") if succ else ():
                s, n = item.rsplit(":", 1)
                pairs.append((s, int(n)))
            out[ctx] = (int(total), pairs)
    return out


def _usable(ctx, s, vocab):
    """后继能不能出：词库里的汉字词，或句读；词边界切错的（「中」+「国人」）不出。"""
    if s in PUNCT:
        return True
    if not (_is_han(s) and s in vocab):
        return False
    return not any(ctx + s[:k] in vocab for k in range(1, len(s)))


def _punct_rank(ctx, counts, han, words_sorted, a, cont):
    """ctx 在维基上的后继（与表同一套排法）里，排在最前的句读与它的名次；前 `--top` 名里
    没有句读返回 (None, None)。"""
    total, pairs = counts[ctx]
    found = [
        (s, math.log(n) * SCORE_SCALE)
        for s, n in pairs
        if len(s) <= a.max_succ_chars and _usable(ctx, s, han)
    ]
    ranked = _mix(
        ctx, found, math.log(total), han, words_sorted, a.max_succ_chars, cont, blocklist=()
    )
    for i, (s, _) in enumerate(ranked[: a.top]):
        if s in PUNCT:
            return s, i
    return None, None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", required=True, help="distill_assoc_bigram.py 的产物")
    ap.add_argument("--counts", required=True, help="ngram_count.py 的输出")
    ap.add_argument("--dict", action="append", required=True, help="词库 dict.yaml，可多次给")
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-count", type=int, default=20, help="维基里上文出现次数下限")
    ap.add_argument("--top", type=int, default=12, help="与表的每行后继数一致")
    ap.add_argument("--max-succ-chars", type=int, default=4)
    ap.add_argument("--cont-weight-n", type=float, default=0.2)
    ap.add_argument("--cont-weight-charwise", type=float, default=1.0)
    ap.add_argument("--source", default="", help="写进产物头注释的语料来源说明")
    a = ap.parse_args()

    vocab = {}
    for d in a.dict:
        for w, wt in _read_dict(d).items():
            if vocab.get(w, -1) < wt:
                vocab[w] = wt
    han = {w: max(wt, 1) for w, wt in vocab.items() if _is_han(w)}
    words_sorted = sorted(han)
    counts = _read_counts(a.counts)

    header, rows = [], []
    with open(a.base, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                header.append(line)
            elif line.strip():
                ctx, succ = line.rstrip("\n").split("\t")
                rows.append((ctx, succ.split(" ")))

    changed = 0
    out = []
    for ctx, succ in rows:
        char_wise = ctx.startswith(CHAR_WISE_MARK)
        word = ctx[len(CHAR_WISE_MARK):] if char_wise else ctx
        if word in counts and counts[word][0] >= a.min_count:
            if char_wise:
                cont = a.cont_weight_charwise
            else:
                cont = 0.0 if len(word) == 1 else a.cont_weight_n
            p, i = _punct_rank(word, counts, han, words_sorted, a, cont)
            succ = [s for s in succ if s != SENTENCE_END]
            if p is not None:
                succ.insert(min(i, len(succ)), p)
            changed += 1
        out.append((ctx, succ[: a.top]))

    with open(a.counts, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.writelines(header)
        f.write("# 标点：由 scripts/lm/distill_assoc_punct.py 按保留句读的语料补入（替掉 $）\n")
        if a.source:
            f.write(f"# 标点语料：{a.source}\n")
        f.write(f"# 标点计数 sha256：{digest}\n")
        for ctx, succ in out:
            f.write(f"{ctx}\t{' '.join(succ)}\n")
    print(f"{len(out)} 行，{changed} 行按语料换了标点 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
