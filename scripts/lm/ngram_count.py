#!/usr/bin/env python3
"""切好词的语料 → 每个词的后继计数（供 `distill_assoc_punct.py` 给联想表补标点）。

    python3 scripts/lm/ngram_count.py --in zhwiki.seg.txt --out zhwiki.counts.tsv

输出每行

    上文<TAB>上文出现次数<TAB>后继1:次数 后继2:次数 …

后继按次数降序，只留前 `--top` 个、次数 ≥ `--min-pair` 的；上文出现次数是它后面**接任何东西**
的总次数（含 `|` 与句末），蒸馏端拿它归一成条件概率。

不落盘中间 n-gram：按上文的散列分片，每个进程扫一遍全量语料、只数落在自己片里的上文，
数完直接按上文分组截前 K 个。标点与 `|` 不当上文（标点后的联想交给标点源）。

试过把相邻两个词也当上文（三元），在 Tatoeba 评测上只多出零点几个百分点、全是标点带来的，
词的命中没变（维基是书面语，口语的「前两个词 → 下一词」学不到），故没留。
"""
import argparse
import os
import sys
from collections import Counter, defaultdict
from multiprocessing import Pool
from zlib import crc32

PUNCT = set("，。！？、；：…")
BREAK = "|"

_a = None


def _is_ctx(t):
    return t != BREAK and t not in PUNCT


def _count(shard):
    a = _a
    c1 = Counter()
    with open(a.inp, encoding="utf-8") as f:
        for line in f:
            t = line.split()
            if not t:
                continue
            if t[-1] not in PUNCT:
                t.append(BREAK)
            for w, s in zip(t, t[1:]):
                if _is_ctx(w) and crc32(w.encode()) % a.shards == shard:
                    c1[(w, s)] += 1
    groups = defaultdict(list)
    totals = Counter()
    for (w, s), n in c1.items():
        totals[w] += n
        if n >= a.min_pair:
            groups[w].append((n, s))
    rows = []
    for w, total in totals.items():
        if total < a.min_ctx:
            continue
        succ = sorted(groups.get(w, ()), reverse=True)[: a.top]
        rows.append(f"{w}\t{total}\t{' '.join(f'{s}:{n}' for n, s in succ)}\n")
    return rows


def _init(a):
    global _a
    _a = a


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True, help="corpus_segment.py 的输出")
    ap.add_argument("--out", required=True)
    ap.add_argument("--shards", type=int, default=os.cpu_count())
    ap.add_argument("--top", type=int, default=40)
    ap.add_argument("--min-pair", type=int, default=3, help="后继次数下限")
    ap.add_argument("--min-ctx", type=int, default=20, help="上文出现次数下限")
    a = ap.parse_args()

    n = 0
    # 分片用 crc32 而不是 hash()：后者每进程随机加盐，各片会对同一上文给出不同的片号
    with Pool(a.shards, initializer=_init, initargs=(a,)) as pool, open(
        a.out, "w", encoding="utf-8"
    ) as w:
        for rows in pool.imap_unordered(_count, range(a.shards)):
            w.writelines(rows)
            n += len(rows)
            print(f"  {n} 个上文", file=sys.stderr)
    print(f"{n} 个上文 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
