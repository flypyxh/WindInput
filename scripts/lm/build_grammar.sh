#!/usr/bin/env bash
# 重建拼音整句的小语法模型：从万象 .gram 里挑出真实输入用得到的条目。
#
# 用法:
#   ./scripts/lm/build_grammar.sh          # 拉取（已有则跳过）+ 挑选 + 查体积，产物放进 build_dev 试用
#
# 环境变量:
#   LM_WORK          原料与中间文件目录（默认 .cache/lm，与 build_assoc.sh 共用）
#   LM_JOBS          并行数（默认 CPU 核数）
#   GRAM_WIKI_HITS   维基里至少被用到几次才留（默认 200）；口语语料里用到一次就留
#   GRAM_MAX_KB      产物体积上限（默认 8192 KB）
#
# 前置:
#   - ./scripts/dev.sh gd 生成过 build_dev/data（切词用输入法自己的拼音词库）
#   - ./scripts/lm/build_assoc.sh 跑过一次（复用它切好的维基：$LM_WORK/zhwiki/seg.txt）
#   - pip install jieba opencc-python-reimplemented；g++（C++17）
#
# 挑法与实测见 docs/design/language-model-integration.md §10–§11。原料与许可：
#   万象 .gram   amzxyz/rime-wanxiang-data LTS（CC-BY-4.0），产物是它的子集，随包须署名；按 sha256 钉版本
#   挑选用语料    Tatoeba（CC BY 2.0 FR）、KdConv / CrossWOZ / CPED 训练划分（Apache-2.0）、中文维基
#                （CC BY-SA 4.0）——只用来决定留哪些条目，本身不进产物
#   darts-clone  BSD-2-Clause，只在构建时编译工具用
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LM="$ROOT/scripts/lm"
WORK="${LM_WORK:-$ROOT/.cache/lm}"
JOBS="${LM_JOBS:-$(nproc)}"
WIKI_HITS="${GRAM_WIKI_HITS:-200}"
MAX_KB="${GRAM_MAX_KB:-8192}"
DICTS="$ROOT/build_dev/data/schemas/pinyin/cn_dicts"
OUT_NAME="wanxiang-mini.gram"

WX_URL="https://github.com/amzxyz/rime-wanxiang-data/releases/download/LTS/wanxiang-lts-zh-hans.gram"
WX_SHA256="9eedac1d836f3fdafb61b162035c8937253aedc106be40b2b957d9ab64e9ed89"
DARTS_URL="https://raw.githubusercontent.com/s-yata/darts-clone/87b71afd6cf784953e3c08f24c64203397f3b724/include/darts.h"
DARTS_SHA256="acfb9cb7522d4c6a6f6099b981f7f12edb17a9b77fe33b3d38da9c20cd77301a"
TATOEBA_URL="https://downloads.tatoeba.org/exports/per_language/cmn/cmn_sentences.tsv.bz2"
KDCONV="https://raw.githubusercontent.com/thu-coai/KdConv/653db76432de09a004ba708a68f8bbd5500e6bec/data"
CROSSWOZ="https://raw.githubusercontent.com/thu-coai/CrossWOZ/df82c9fdff91b9b130f2d6b89110d3870ba6260e/data/crosswoz"
CPED="https://raw.githubusercontent.com/scutcyr/CPED/1e4b81c28a123f22387e06664f37e5dc9322380f/data/CPED"

say() { printf '\033[32m%s\033[0m\n' "$1"; }
die() { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 1; }

# 下到 .part 再改名：中断不会留下一个看着齐全的半截文件让下次跳过
fetch() {
    local url="$1" dst="$2"
    [ -s "$dst" ] && return
    say "下载 $url"
    curl -fL --retry 3 -C - -o "$dst.part" "$url"
    mv "$dst.part" "$dst"
}

check_sha() {
    echo "$2  $1" | sha256sum -c --quiet || die "$1 的 sha256 对不上（上游换了文件？）。确认后改本脚本里的值"
}

[ -f "$DICTS/base.dict.yaml" ] && [ -f "$DICTS/8105.dict.yaml" ] \
    || die "缺 $DICTS 下的词库，先跑 ./scripts/dev.sh gd"
[ -s "$WORK/zhwiki/seg.txt" ] || die "缺 $WORK/zhwiki/seg.txt，先跑 ./scripts/lm/build_assoc.sh"
python3 -c 'import jieba, opencc' 2>/dev/null \
    || die "缺 Python 依赖：pip install jieba opencc-python-reimplemented"
G="$WORK/grammar"
mkdir -p "$G/raw"

fetch "$WX_URL" "$G/wanxiang.gram"
check_sha "$G/wanxiang.gram" "$WX_SHA256"
fetch "$DARTS_URL" "$G/darts.h"
check_sha "$G/darts.h" "$DARTS_SHA256"
fetch "$TATOEBA_URL" "$G/raw/cmn_sentences.tsv.bz2"
[ -s "$G/raw/cmn_sentences.tsv" ] || bzip2 -dk "$G/raw/cmn_sentences.tsv.bz2"
for d in film music travel; do fetch "$KDCONV/$d/train.json" "$G/raw/kdconv_$d.json"; done
fetch "$CROSSWOZ/train.json.zip" "$G/raw/crosswoz_train.json.zip"
[ -s "$G/raw/crosswoz_train.json" ] || unzip -p "$G/raw/crosswoz_train.json.zip" > "$G/raw/crosswoz_train.json"
for s in train valid; do fetch "$CPED/${s}_split.csv" "$G/raw/cped_$s.csv"; done

if [ ! -s "$G/colloq.seg" ]; then
    say "整理口语语料并切词"
    python3 "$LM/grammar_corpus.py" --out "$G/colloq.txt" \
        --tatoeba "$G/raw/cmn_sentences.tsv" \
        --kdconv "$G/raw/kdconv_film.json" --kdconv "$G/raw/kdconv_music.json" \
        --kdconv "$G/raw/kdconv_travel.json" --crosswoz "$G/raw/crosswoz_train.json" \
        --cped "$G/raw/cped_train.csv" --cped "$G/raw/cped_valid.csv"
    python3 "$LM/corpus_segment.py" --in "$G/colloq.txt" \
        --dict "$DICTS/base.dict.yaml" --dict "$DICTS/8105.dict.yaml" \
        --out "$G/colloq.seg.part" --jobs "$JOBS"
    mv "$G/colloq.seg.part" "$G/colloq.seg"
fi

say "编译 gram_select"
g++ -O2 -std=c++17 -I"$G" -o "$G/gram_select" "$LM/gram_select.cc"

# 口语里被用到一次就留（权重 = 门槛），维基里要被用到 WIKI_HITS 次
say "挑条目（口语全收，维基 ≥ $WIKI_HITS 次）"
"$G/gram_select" "$G/wanxiang.gram" "$G/$OUT_NAME" "$WIKI_HITS" \
    "$G/colloq.seg:$WIKI_HITS" "$WORK/zhwiki/seg.txt:1"

kb=$(( $(stat -c %s "$G/$OUT_NAME") / 1024 ))
[ "$kb" -le "$MAX_KB" ] \
    || die "产物 ${kb} KB 超过上限 ${MAX_KB} KB。调大 GRAM_WIKI_HITS，或确有必要再放宽 GRAM_MAX_KB"
dst="$ROOT/build_dev/data/schemas/pinyin/grammar"
mkdir -p "$dst"
cp "$G/$OUT_NAME" "$dst/$OUT_NAME"
say "→ $dst/$OUT_NAME（${kb} KB，sha256 $(sha256sum "$G/$OUT_NAME" | cut -c1-16)…）"
say "评测满意后 cp 到 data/schemas/pinyin/grammar/ 入库；安卓仓 assets 同路径需同步"
