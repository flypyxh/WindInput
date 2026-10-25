#!/usr/bin/env bash
# 靶机按键延迟探针 —— Linux 端驱动 (靶机端脚本: key-latency-probe.ps1)
#
# 做什么: 在靶机上用 PowerShell 直连输入法服务的 bridge 管道, 按 TSF DLL 的帧格式逐键
#         发 KEY_EVENT (每个编码逐字母 keydown, 再发 Esc 撤销组合), 计每键往返耗时。
#         不上屏、不进词频。可在每轮前把服务工作集清掉 (trim) 或连系统备用页一起清 (purge)
#         来造"冷"启动场景, 对比 warm / cold 的尾延迟。
#
# 用法:
#   ./scripts/perf/key-latency.sh codes [N] [日志文件...]
#       从服务日志提取编码清单 (`key_event: code=0x20 ... buf='xxx'` 里空格上屏前的 2-4 位
#       小写字母), 去重随机取 N 个 (默认 150), 写 .remote/perf/codes-<时间戳>.txt 并打印路径。
#       不给日志文件则从靶机拉 $WIND_LOCAL_DIR/logs/wind_input.*.log。
#   ./scripts/perf/key-latency.sh run [--variant dev|release] [--codes 清单]
#                                     [--phases restart,warm,trim,purge,warm]
#       上传探针脚本+清单, 按 phases 顺序各跑一轮, 结果拉回 .remote/perf/<时间戳>/。
#       phase 与冷热: restart/warm = 不造冷 (restart 只是标签, 不会真重启服务, 想测重启后
#       首轮请自己先重启再跑); trim = 清服务工作集; purge = trim + 清系统备用页(需靶机管理员
#       会话, 否则该轮报错)。同名 phase 重复出现时, 第二次起标签追加 -2、-3...
#       不给 --codes 则用 .remote/perf/ 下最新的 codes-*.txt。
#   ./scripts/perf/key-latency.sh report <result.csv> [--top N]
#       汇总每轮 n/中位/p90/p99/max/>=15ms 个数, 并列出每轮最慢的 N 个键 (默认 5)。
#
# 配置: 读 scripts/deploy.local (同 dev.sh): WIND_REMOTE (SSH 目标)、WIND_LOCAL_DIR
#       (取日志用)。探针在靶机上的工作目录默认 C:/kp, 可用 WIND_PROBE_DIR 覆盖。
#
# ⚠️ 使用须知:
#   - 会在靶机屏幕上闪候选窗, 并改变服务的"当前客户端"状态 —— 不要在有人正在靶机上打字时跑。
#   - Esc 撤销每个编码, 不上屏、不进词频, 但会占用服务的组合状态, 跑的期间别同时人工输入。
#   - 靶机端脚本保持纯 ASCII 入库, 上传时由本脚本加 UTF-8 BOM
#     (靶机 Windows PowerShell 5 读无 BOM 文件会按 GBK 解码)。
#   - 目前只支持 a-z (其余字符跳过, meta 里记数); VK 映射在 ps1 的 Get-VkForChar 里。
#   - SSH 输出中文可能乱码 (GBK), 以拉回的文件为准。
#   - 结果在 .remote/perf/ (已 gitignore)。

set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PRODUCT_ROOT="$(cd "$SCRIPTS_ROOT/.." && pwd)"
PERF_DIR="$PRODUCT_ROOT/.remote/perf"
PROBE_PS1="$SCRIPT_DIR/key-latency-probe.ps1"

[ -f "$SCRIPTS_ROOT/deploy.local" ] && . "$SCRIPTS_ROOT/deploy.local"
WIND_REMOTE="${WIND_REMOTE:-}"
WIND_LOCAL_DIR="${WIND_LOCAL_DIR:-}"
PROBE_DIR="${WIND_PROBE_DIR:-C:/kp}"
PROBE_DIR="${PROBE_DIR%/}"

say() { printf '\033[32m%b\033[0m\n' "$1"; }
err() { printf '\033[31m%b\033[0m\n' "$1" >&2; }

require_remote() {
    [ -n "$WIND_REMOTE" ] || { err "未配置 WIND_REMOTE: 请在 $SCRIPTS_ROOT/deploy.local 设置 SSH 目标"; return 1; }
}

# 在靶机上跑一条 PowerShell 语句 (经 UTF-16LE+base64 传入, 避开多层引号; 同 dev.sh 的 remote_ps)
remote_ps() {
    command -v iconv >/dev/null 2>&1 || { err "需要 iconv"; return 1; }
    local script b64
    script="\$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; $1"
    b64="$(printf '%s' "$script" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
    ssh "$WIND_REMOTE" "powershell -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand $b64"
}

# ---------- codes ----------
do_codes() {
    local n=150
    if [[ "${1:-}" =~ ^[0-9]+$ ]]; then n="$1"; shift; fi
    local ts logs=("$@")
    ts="$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$PERF_DIR"
    if [ ${#logs[@]} -eq 0 ]; then
        require_remote || return 1
        [ -n "$WIND_LOCAL_DIR" ] || { err "未配置 WIND_LOCAL_DIR (deploy.local)"; return 1; }
        local logdir="$PERF_DIR/logs-$ts"
        mkdir -p "$logdir"
        say "拉取日志 <- $WIND_REMOTE:$WIND_LOCAL_DIR/logs/wind_input.*.log"
        scp -q "$WIND_REMOTE:$WIND_LOCAL_DIR/logs/wind_input.*.log" "$logdir/" \
            || { err "scp 失败 (检查 WIND_LOCAL_DIR / SSH)"; return 1; }
        logs=("$logdir"/wind_input.*.log)
    fi
    local out="$PERF_DIR/codes-$ts.txt"
    python3 -I - "$n" "$out" "${logs[@]}" <<'PY' || return 1
import random, re, sys
n, out, files = int(sys.argv[1]), sys.argv[2], sys.argv[3:]
rx = re.compile(r"key_event: code=0x20 .*buf='([a-z]{2,4})'")
seen = set()
for f in files:
    with open(f, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = rx.search(line)
            if m:
                seen.add(m.group(1))
if not seen:
    sys.exit("日志里没找到 key_event code=0x20 且 buf 为 2-4 位小写字母的行")
codes = sorted(seen)
random.shuffle(codes)
codes = codes[:n]
with open(out, "w", encoding="ascii") as fh:
    fh.write("\n".join(codes) + "\n")
print(f"候选 {len(seen)} 个, 取 {len(codes)} 个 -> {out}")
PY
}

# ---------- run ----------
do_run() {
    local variant=dev phases="restart,warm,trim,purge,warm" codes=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --variant) variant="${2:-}"; shift 2 ;;
            --phases)  phases="${2:-}";  shift 2 ;;
            --codes)   codes="${2:-}";   shift 2 ;;
            *) err "未知参数: $1"; return 1 ;;
        esac
    done
    case "$variant" in dev|release) ;; *) err "--variant 只能是 dev 或 release"; return 1 ;; esac
    require_remote || return 1
    if [ -z "$codes" ]; then
        codes="$(ls -1t "$PERF_DIR"/codes-*.txt 2>/dev/null | head -1)"
        [ -n "$codes" ] || { err "没有编码清单: 先跑 codes, 或用 --codes 指定"; return 1; }
    fi
    [ -f "$codes" ] || { err "找不到编码清单: $codes"; return 1; }

    local ts outdir tmp
    ts="$(date +%Y%m%d-%H%M%S)"
    outdir="$PERF_DIR/$ts"
    mkdir -p "$outdir"

    say "准备靶机目录 $PROBE_DIR 并上传探针 (加 UTF-8 BOM)"
    remote_ps "New-Item -ItemType Directory -Force -Path '$PROBE_DIR' | Out-Null; Remove-Item -Force -ErrorAction SilentlyContinue '$PROBE_DIR/result.csv','$PROBE_DIR/result.csv.meta'; exit 0" >/dev/null \
        || { err "靶机目录准备失败"; return 1; }
    tmp="$(mktemp)"
    { printf '\xEF\xBB\xBF'; cat "$PROBE_PS1"; } > "$tmp"
    scp -q "$tmp" "$WIND_REMOTE:$PROBE_DIR/key-latency-probe.ps1" || { rm -f "$tmp"; err "上传 ps1 失败"; return 1; }
    rm -f "$tmp"
    scp -q "$codes" "$WIND_REMOTE:$PROBE_DIR/codes.txt" || { err "上传清单失败"; return 1; }
    cp "$codes" "$outdir/codes.txt"

    local -A seen=()
    local ph label cold rc=0
    IFS=',' read -ra plist <<< "$phases"
    for ph in "${plist[@]}"; do
        seen[$ph]=$(( ${seen[$ph]:-0} + 1 ))
        label="$ph"; [ "${seen[$ph]}" -gt 1 ] && label="$ph-${seen[$ph]}"
        case "$ph" in
            restart|warm) cold=none ;;
            trim)  cold=trim ;;
            purge) cold=purge ;;
            *) err "未知 phase: $ph (可选 restart|warm|trim|purge)"; rc=1; break ;;
        esac
        say "== phase $label (cold=$cold) =="
        if ! ssh "$WIND_REMOTE" "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $PROBE_DIR/key-latency-probe.ps1 -Variant $variant -CodesFile $PROBE_DIR/codes.txt -Phase $label -Cold $cold -OutFile $PROBE_DIR/result.csv"; then
            err "phase $label 失败 (见上方 ERROR 行); 已完成的轮次仍会拉回"
            rc=1; break
        fi
    done

    say "拉回结果 -> $outdir/"
    scp -q "$WIND_REMOTE:$PROBE_DIR/result.csv" "$outdir/result.csv" || { err "拉回 result.csv 失败"; return 1; }
    scp -q "$WIND_REMOTE:$PROBE_DIR/result.csv.meta" "$outdir/result.csv.meta" 2>/dev/null || true
    do_report "$outdir/result.csv"
    return $rc
}

# ---------- report ----------
do_report() {
    local csv="${1:-}"; shift || true
    local top=5
    [ "${1:-}" = "--top" ] && top="${2:-5}"
    [ -f "$csv" ] || { err "用法: key-latency.sh report <result.csv> [--top N]"; return 1; }
    python3 -I - "$csv" "$top" <<'PY'
import csv, sys
path, top = sys.argv[1], int(sys.argv[2])
phases, order = {}, []
with open(path, newline="") as fh:
    for row in csv.reader(fh):
        if len(row) < 5:
            continue
        ph, code, idx, ms, cmd = row[:5]
        if ph not in phases:
            phases[ph] = []
            order.append(ph)
        phases[ph].append((float(ms), code, idx))

def pct(sorted_ms, p):
    # nearest-rank
    k = max(0, min(len(sorted_ms) - 1, -(-len(sorted_ms) * p // 100) - 1))
    return sorted_ms[k]

print(f"{'phase':<12}{'n':>7}{'median':>10}{'p90':>10}{'p99':>10}{'max':>10}{'>=15ms':>8}")
for ph in order:
    ms = sorted(r[0] for r in phases[ph])
    med = ms[len(ms) // 2] if len(ms) % 2 else (ms[len(ms) // 2 - 1] + ms[len(ms) // 2]) / 2
    print(f"{ph:<12}{len(ms):>7}{med:>10.2f}{pct(ms, 90):>10.2f}{pct(ms, 99):>10.2f}{ms[-1]:>10.2f}{sum(1 for x in ms if x >= 15):>8}")
print()
for ph in order:
    worst = sorted(phases[ph], reverse=True)[:top]
    desc = ", ".join(f"{c}#{i if i == 'esc' else int(i) + 1} {m:.1f}ms" for m, c, i in worst)
    print(f"{ph:<12}slowest: {desc}")
PY
}

case "${1:-}" in
    codes)  shift; do_codes "$@" ;;
    run)    shift; do_run "$@" ;;
    report) shift; do_report "$@" ;;
    *)
        sed -n '2,/^set -o/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
        [ -z "${1:-}" ] || exit 1
        ;;
esac
