#!/usr/bin/env bash
# 生成 Linux 版的系统层应用兼容规则 compat.toml。
#
#   scripts/linux/gen-compat.sh <Windows 版 data/compat.toml> <输出文件>
#
# 为什么不能直接随包带 data/compat.toml：里面的规则全是 Windows 宿主的兼容修正（进程名
# `xxx.exe`、TSF 组合帧、Band 层级、HostRender 代理渲染、资源管理器的初始状态……），策略本身就
# 是 Windows 专属的，换了平台既匹配不上、语义也不成立。但**字段说明**（文件头的注释）是这套机制的
# 文档，设置端的「应用兼容性」页与用户手写 compat.toml 都要参照它，且随核心新增字段而演进——
# 所以不手抄一份，而是打包时从 Windows 版截取，避免两份文档漂移。
#
# 产物 = Linux 说明头 + 原文件第一条规则之前的全部内容（字段说明）+ 零条内置规则。
set -euo pipefail

SRC="${1:?用法: gen-compat.sh <Windows 版 compat.toml> <输出文件>}"
OUT="${2:?用法: gen-compat.sh <Windows 版 compat.toml> <输出文件>}"
[[ -f "$SRC" ]] || { echo "找不到 $SRC" >&2; exit 1; }

# 第一条规则表（[[apps]] / [[initial_mode_scope]] / [[commit_newline]] …）之前是纯文档。
if ! grep -q '^\[\[' "$SRC"; then
    echo "$SRC 里没有规则表，格式与预期不符，拒绝生成" >&2
    exit 1
fi

{
    cat <<'HEAD'
# 应用兼容性规则 —— Linux 版系统预置层
#
# 本文件由打包脚本 scripts/linux/gen-compat.sh 生成，**内置规则为空**。
# Windows 版随附的内置规则（Weixin.exe、Code.exe、WINWORD.EXE、SearchHost.exe……）针对的是
# Windows 宿主的行为（进程名、TSF 组合帧、Band 层级、HostRender 代理渲染），对 Linux 没有意义，
# 因此不随包分发；各平台的兼容策略不共用。
#
# Linux 上 `process` 匹配的是 Fcitx5 上报的程序名（InputContext::program()，通常是可执行文件名，
# 小写，如 "firefox"、"code"），不带 .exe。用户规则写在 ~/.config/WindInput/compat.toml，
# 也可以在设置里的「应用兼容性」页添加。
#
# ⚠ 以下字段说明沿用 Windows 版。其中依赖 TSF 上报的项（组合起点 / caret 探测 / Band /
#   HostRender 相关）在 Linux 上没有对应的数据来源，写了也不会生效。
# ─────────────────────────────────────────────────────────────────────────────

HEAD
    awk '/^\[\[/ { exit } { print }' "$SRC"
} >"$OUT"

# 自检：产物里不得残留任何规则表，否则等于把 Windows 规则夹带进了 Linux 包。
if grep -q '^\[\[' "$OUT"; then
    echo "生成结果里残留了规则表，拒绝" >&2
    rm -f "$OUT"
    exit 1
fi
