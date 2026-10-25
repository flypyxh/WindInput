#!/usr/bin/env python3
"""训练语料切词：一行一句 → 一行一句、词间空格，供 `ngram_count` 统计。

    python3 scripts/lm/corpus_segment.py --in zhwiki.sentences.txt \
        --dict base.dict.yaml --dict 8105.dict.yaml --out zhwiki.seg.txt

用**输入法自己的词库**给 jieba 当词典（不用 jieba 自带的）：模型里的词与候选里的词一一对应，
联想才查得到、给得出。权重为 0 的词按 1 算（jieba 要正频次）。

标点当词留着（「吗 ？」是联想要学的），只留中文句读 `，。！？、；：…`；其余（数字、拉丁、
引号括号等）一律换成 `|`——断开 n-gram，统计端不跨它计数，免得「《宝典》残篇」被算成
「宝典 → 残篇」。连续的 `|` 合并成一个。
"""
import argparse
import os
import sys
import tempfile
from multiprocessing import Pool

PUNCT = set("，。！？、；：…")
BREAK = "|"
CHUNK = 20000

_cut = None


def _han(s):
    return all("一" <= c <= "鿿" for c in s)


def _read_dict(path):
    """rime dict.yaml：`...` 之后每行 `词<TAB>编码[<TAB>权重]`。返回 {词: 最大权重}。"""
    words = {}
    body = False
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not body:
                body = line.startswith("...")
                continue
            if not line.strip() or line.startswith("#"):
                continue
            p = line.rstrip("\n").split("\t")
            w = int(p[2]) if len(p) >= 3 and p[2].strip().isdigit() else 0
            if words.get(p[0], -1) < w:
                words[p[0]] = w
    return words


def _init(jieba_dict):
    global _cut
    import jieba

    jieba.setLogLevel(60)
    tk = jieba.Tokenizer(dictionary=jieba_dict)
    tk.initialize()
    _cut = lambda s: tk.cut(s, HMM=False)  # noqa: E731


def _seg(lines):
    out = []
    for s in lines:
        toks = []
        for t in _cut(s.strip()):
            if t == "……":
                t = "…"
            if not (_han(t) or t in PUNCT):
                t = BREAK
            if t == BREAK and toks and toks[-1] == BREAK:
                continue
            toks.append(t)
        while toks and toks[-1] == BREAK:
            toks.pop()
        while toks and toks[0] == BREAK:
            toks.pop(0)
        if toks:
            out.append(" ".join(toks))
    return out


def _chunks(path):
    buf = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            buf.append(line)
            if len(buf) >= CHUNK:
                yield buf
                buf = []
    if buf:
        yield buf


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True)
    ap.add_argument("--dict", action="append", required=True, help="词库 dict.yaml，可多次给")
    ap.add_argument("--out", required=True)
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    a = ap.parse_args()

    vocab = {}
    for d in a.dict:
        for w, wt in _read_dict(d).items():
            if _han(w) and vocab.get(w, -1) < wt:
                vocab[w] = wt
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
        for w, wt in vocab.items():
            f.write(f"{w} {max(wt, 1)}\n")
        jd = f.name
    print(f"词库 {len(vocab)} 词", file=sys.stderr)

    n = 0
    try:
        with Pool(a.jobs, initializer=_init, initargs=(jd,)) as pool, open(
            a.out, "w", encoding="utf-8"
        ) as w:
            for i, lines in enumerate(pool.imap(_seg, _chunks(a.inp))):
                for s in lines:
                    w.write(s + "\n")
                n += len(lines)
                if i % 100 == 0:
                    print(f"  {n} 句", file=sys.stderr)
    finally:
        os.unlink(jd)
    print(f"{n} 句 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
