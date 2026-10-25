#!/usr/bin/env bash
# 重建智能联想的「下一词」表 data/assoc/assoc_bigram.txt：拉取全部原料 → 蒸馏 → 补标点 → 查体积。
#
# 用法:
#   ./scripts/lm/build_assoc.sh            # 拉取（已有则跳过）+ 重建，产物写回 data/assoc/
#   ./scripts/lm/build_assoc.sh eval       # 另拉 Tatoeba，拿 data/assoc/ 的表跑联想评测（会复制进 build_dev）
#
# 环境变量:
#   LM_WORK       原料与中间文件目录（默认 .cache/lm）。维基全量要 ~15 GB，盘紧就指到别处。
#   LM_JOBS       并行数（默认 CPU 核数）
#   ASSOC_MAX_KB  产物体积上限（默认 2560 KB）。表整份常驻内存，PC 用户对内存敏感，超了就失败。
#
# 前置:
#   - ./scripts/dev.sh gd 生成过 build_dev/data（切词与蒸馏用输入法自己的拼音词库）
#   - pip install jieba opencc-python-reimplemented wikiextractor
#
# 每步的产物已存在就跳过；要重做哪步，删掉 $LM_WORK 下对应的文件。
# 原料与许可:
#   bgw.gram   rime-octagram-data（LGPL-3.0），词的后继；按 sha256 钉死版本
#   中文维基    CC BY-SA 4.0，只取标点的位置；dump 只留最近几期，拉到的是当期 latest，
#              计数的 sha256 写进产物头注释
#   Tatoeba    CC BY 2.0 FR，只用于评测，不进产物
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LM="$ROOT/scripts/lm"
WORK="${LM_WORK:-$ROOT/.cache/lm}"
JOBS="${LM_JOBS:-$(nproc)}"
MAX_KB="${ASSOC_MAX_KB:-2560}"
DICTS="$ROOT/build_dev/data/schemas/pinyin/cn_dicts"
OUT="$ROOT/data/assoc/assoc_bigram.txt"

BGW_URL="https://github.com/lotem/rime-octagram-data/raw/hans/zh-hans-t-essay-bgw.gram"
BGW_SHA256="d3cb2438c1fdcd6a855dd6ca8f5c1060a29273c6b64c2c2c69af67cd71b6aa7e"
WIKI_URL="https://dumps.wikimedia.org/zhwiki/latest/zhwiki-latest-pages-articles.xml.bz2"
TATOEBA_URL="https://downloads.tatoeba.org/exports/per_language/cmn/cmn_sentences.tsv.bz2"

say() { printf '\033[32m%s\033[0m\n' "$1"; }
die() { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 1; }

# 下到 .part 再改名：中断不会留下一个看着齐全的半截文件让下次跳过。-R 保留服务器时间（维基取它当期号）
fetch() {
    local url="$1" dst="$2"
    [ -s "$dst" ] && return
    say "下载 $url"
    curl -fL --retry 3 -C - -R -o "$dst.part" "$url"
    mv "$dst.part" "$dst"
}

preflight() {
    [ -f "$DICTS/base.dict.yaml" ] && [ -f "$DICTS/8105.dict.yaml" ] \
        || die "缺 $DICTS 下的词库，先跑 ./scripts/dev.sh gd"
    python3 -c 'import jieba, opencc, wikiextractor' 2>/dev/null \
        || die "缺 Python 依赖：pip install jieba opencc-python-reimplemented wikiextractor"
    mkdir -p "$WORK"
}

fetch_bgw() {
    fetch "$BGW_URL" "$WORK/bgw.gram"
    echo "$BGW_SHA256  $WORK/bgw.gram" | sha256sum -c --quiet \
        || die "bgw.gram 的 sha256 对不上（上游换了文件？）。确认后改本脚本的 BGW_SHA256"
}

# 维基 → 句子 → 切词 → 每词后继计数
wiki_counts() {
    local w="$WORK/zhwiki"
    mkdir -p "$w"
    fetch "$WIKI_URL" "$w/pages-articles.xml.bz2"
    if [ ! -s "$w/sentences.txt" ]; then
        if [ ! -d "$w/extracted" ]; then
            say "抽正文（WikiExtractor，约半小时）"
            python3 -m wikiextractor.WikiExtractor --no-templates --processes "$JOBS" -b 100M \
                -o "$w/extracted.part" "$w/pages-articles.xml.bz2" > "$w/extract.log" 2>&1
            mv "$w/extracted.part" "$w/extracted"
        fi
        say "清洗成句"
        python3 "$LM/corpus_clean_wiki.py" --in "$w/extracted" --out "$w/sentences.txt.part" --jobs "$JOBS"
        mv "$w/sentences.txt.part" "$w/sentences.txt"
    fi
    if [ ! -s "$w/seg.txt" ]; then
        say "切词"
        python3 "$LM/corpus_segment.py" --in "$w/sentences.txt" \
            --dict "$DICTS/base.dict.yaml" --dict "$DICTS/8105.dict.yaml" \
            --out "$w/seg.txt.part" --jobs "$JOBS"
        mv "$w/seg.txt.part" "$w/seg.txt"
    fi
    if [ ! -s "$w/counts.tsv" ]; then
        say "计数"
        python3 "$LM/ngram_count.py" --in "$w/seg.txt" --out "$w/counts.tsv.part" --shards "$JOBS"
        mv "$w/counts.tsv.part" "$w/counts.tsv"
    fi
}

build() {
    preflight
    fetch_bgw
    wiki_counts
    say "蒸馏下一词（bgw）"
    python3 "$LM/distill_assoc_bigram.py" --gram "$WORK/bgw.gram" \
        --dict "$DICTS/base.dict.yaml" --dict "$DICTS/8105.dict.yaml" --jobs "$JOBS" \
        --source "rime-octagram-data zh-hans-t-essay-bgw.gram（LGPL-3.0，https://github.com/lotem/rime-octagram-data）" \
        --out "$WORK/assoc_bigram.bgw.txt"
    say "补标点（维基）"
    local month
    month="$(date -r "$WORK/zhwiki/pages-articles.xml.bz2" +%Y-%m)"
    python3 "$LM/distill_assoc_punct.py" --base "$WORK/assoc_bigram.bgw.txt" \
        --counts "$WORK/zhwiki/counts.tsv" \
        --dict "$DICTS/base.dict.yaml" --dict "$DICTS/8105.dict.yaml" \
        --source "中文维基百科 $month 全量（CC BY-SA 4.0），切词用输入法词库" \
        --out "$WORK/assoc_bigram.txt"
    local kb=$(( $(stat -c %s "$WORK/assoc_bigram.txt") / 1024 ))
    [ "$kb" -le "$MAX_KB" ] \
        || die "产物 ${kb} KB 超过上限 ${MAX_KB} KB（整份常驻内存）。调小 --contexts / --top，或确有必要再放宽 ASSOC_MAX_KB"
    cp "$WORK/assoc_bigram.txt" "$OUT"
    say "→ $OUT（${kb} KB）。git diff 看变化；安卓仓 app/src/main/assets/data/assoc/ 需同步"
}

evaluate() {
    preflight
    local t="$WORK/tatoeba"
    mkdir -p "$t"
    fetch "$TATOEBA_URL" "$t/cmn_sentences.tsv.bz2"
    [ -s "$t/cmn_sentences.tsv" ] || bzip2 -dk "$t/cmn_sentences.tsv.bz2"
    for u in word char; do
        [ -s "$t/events_$u.tsv" ] \
            || python3 "$LM/assoc_eval_prep.py" --in "$t/cmn_sentences.tsv" --out "$t/events_$u.tsv" --unit "$u"
    done
    # 评测读 build_dev/data（与 gd 组装的运行时数据同源），先把刚出的表放过去
    cp "$OUT" "$ROOT/build_dev/data/assoc/assoc_bigram.txt"
    cd "$ROOT/wind_input"
    WIND_ASSOC_NEXT_EVAL="$t/events_word.tsv,$t/events_char.tsv" \
        cargo test -p wind-coordinator --test assoc_eval assoc_next_eval -- --ignored --nocapture
}

case "${1:-build}" in
    build) build ;;
    eval) evaluate ;;
    *) die "用法: $0 [build|eval]" ;;
esac
