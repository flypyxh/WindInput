#!/usr/bin/env python3
"""口语语料 → 一行一句的简体文本（给 `corpus_segment.py` 切词，再交 `gram_select` 挑条目）。

    python3 scripts/lm/grammar_corpus.py --tatoeba cmn_sentences.tsv \
        --kdconv film/train.json --kdconv music/train.json --kdconv travel/train.json \
        --crosswoz train.json --cped train_split.csv --cped valid_split.csv --out colloq.txt

每种来源各取它的「话」：Tatoeba 取句子列，KdConv / CrossWOZ 取每轮对话的文本（两边都要，
系统一侧也是人写的），CPED 取 `Utterance` 列。繁转简（OpenCC t2s），「甚么」换成「什么」。

只收**可以并入 GPL 的许可**（挑出的是万象的条目，语料本身不随包，但挑选也不用受限的来源）：
Tatoeba CC BY 2.0 FR、KdConv / CrossWOZ / CPED Apache-2.0。RiSAWOZ（CC BY-NC）、
LCCC（限科研）不收。只用各数据集的训练划分：测试划分留给评测。
"""
import argparse
import csv
import json
import sys


def tatoeba(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            yield line.rstrip("\n").split("\t")[-1]


def kdconv(path):
    with open(path, encoding="utf-8") as f:
        for dialog in json.load(f):
            for m in dialog["messages"]:
                yield m["message"]


def crosswoz(path):
    with open(path, encoding="utf-8") as f:
        for dialog in json.load(f).values():
            for m in dialog["messages"]:
                yield m["content"]


def cped(path):
    with open(path, encoding="utf-8-sig") as f:
        for row in csv.DictReader(f):
            yield row["Utterance"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    for name in ("tatoeba", "kdconv", "crosswoz", "cped"):
        ap.add_argument(f"--{name}", action="append", default=[])
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    import opencc

    t2s = opencc.OpenCC("t2s")
    n = 0
    with open(a.out, "w", encoding="utf-8") as w:
        for reader in (tatoeba, kdconv, crosswoz, cped):
            for path in getattr(a, reader.__name__):
                for text in reader(path):
                    text = t2s.convert(text.replace("\n", " ").strip()).replace("甚么", "什么")
                    if text:
                        w.write(text + "\n")
                        n += 1
    print(f"{n} 句 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
