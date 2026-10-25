#!/usr/bin/env python3
"""WikiExtractor 的输出 → 训练语料：简体、一行一句。

    python3 -m wikiextractor.WikiExtractor --no-templates -o extracted zhwiki-*-pages-articles.xml.bz2
    python3 scripts/lm/corpus_clean_wiki.py --in extracted --out zhwiki.sentences.txt

中文维基是 CC BY-SA 4.0，可单向并入 GPLv3（创作共用 2015 年的兼容声明），由它训练的统计
数据可随包分发，附来源说明即可。

处理：去掉 `<doc>` 标记与标题行（WikiExtractor 把小节标题写成以「.」结尾的短行），繁转简
（OpenCC t2s，维基繁简混杂），按 。！？；与换行断句，留汉字占比 ≥ 60%、6–200 字的句子。
句末标点保留：联想要学「语气词后接问号」这类事。
"""
import argparse
import os
import re
import sys
from multiprocessing import Pool

SPLIT = re.compile(r"(?<=[。！？；])")
_t2s = None


def _init():
    global _t2s
    import opencc

    _t2s = opencc.OpenCC("t2s")


def _keep(s):
    if not 6 <= len(s) <= 200:
        return False
    han = sum("一" <= c <= "鿿" for c in s)
    return han >= 0.6 * len(s)


def _clean(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("<doc") or line.startswith("</doc"):
                continue
            # 小节标题：「發展歷史.」
            if line.endswith(".") and len(line) < 30:
                continue
            for s in SPLIT.split(_t2s.convert(line)):
                s = s.strip()
                if _keep(s):
                    out.append(s)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True, help="WikiExtractor 输出目录")
    ap.add_argument("--out", required=True)
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    a = ap.parse_args()
    files = sorted(
        os.path.join(d, f) for d, _, fs in os.walk(a.inp) for f in fs if f.startswith("wiki_")
    )
    n = 0
    with Pool(a.jobs, initializer=_init) as pool, open(a.out, "w", encoding="utf-8") as w:
        for sents in pool.imap(_clean, files):
            for s in sents:
                w.write(s + "\n")
            n += len(sents)
    print(f"{len(files)} 个文件，{n} 句 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
