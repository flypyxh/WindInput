#!/usr/bin/env bash
# windinput-setup 的离线测试：临时 HOME / XDG_CONFIG_HOME，假 pgrep / fcitx5 / fcitx5-remote，
# 不碰本机真实配置、不起真 fcitx5。逐个场景断言改动结果，以及不破坏别人的内容、重复运行幂等。
#
#   scripts/linux/test-setup.sh
#   SETUP=/path/to/windinput-setup scripts/linux/test-setup.sh   # 测别的版本
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SETUP="${SETUP:-$REPO/scripts/linux/pkg/windinput-setup}"
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

PASS=0
FAIL=0
CASE=""
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); echo "FAIL [$CASE] $*" >&2; }

# 新场景：干净的 HOME 与配置目录，默认 fcitx5 未运行。
new_case() {
    CASE="$1"
    T="$ROOT/$1"
    mkdir -p "$T/home" "$T/cfg/fcitx5" "$T/cfg/WindInput" "$T/bin"
    F="$T/cfg/fcitx5"
    W="$T/cfg/WindInput"
    # 假 pgrep：state 文件存在即「fcitx5 在跑」。
    cat >"$T/bin/pgrep" <<EOF
#!/bin/sh
[ -e "$T/fcitx_running" ]
EOF
    # 假 fcitx5-remote -e：模拟真 fcitx5 退出时把内存里的旧 profile 写回，然后退出。
    cat >"$T/bin/fcitx5-remote" <<EOF
#!/bin/sh
echo "remote \$*" >>"$T/calls"
if [ "\$1" = "-e" ] && [ -e "$T/fcitx_running" ]; then
    cp "$T/fcitx_memory_profile" "$F/profile"
    rm -f "$T/fcitx_running"
fi
EOF
    cat >"$T/bin/fcitx5" <<EOF
#!/bin/sh
echo "fcitx5 \$* profile_has_windinput=\$(grep -c '^Name=windinput' "$F/profile")" >>"$T/calls"
EOF
    chmod +x "$T/bin/"*
}

run_setup() {
    env -i HOME="$T/home" XDG_CONFIG_HOME="$T/cfg" PATH="$T/bin:/usr/bin:/bin" \
        ${DISPLAY_FOR_TEST:+DISPLAY=$DISPLAY_FOR_TEST} ${PYTHONPATH_FOR_TEST:+PYTHONPATH=$PYTHONPATH_FOR_TEST} \
        bash "$SETUP" --no-im-config "$@" >"$T/out" 2>&1 || { bad "退出码非 0：$(cat "$T/out")"; return 1; }
}

# 文件内容逐字节等于期望（期望从 stdin 读，printf 风格转义已展开）。
expect_file() {
    local f="$1" want
    want="$(cat; echo x)"
    want="${want%x}"
    if [[ "$(cat "$f"; echo x)" == "${want}x" ]]; then ok; else
        bad "$f 内容不符"
        diff <(printf '%s' "$want") "$f" >&2 || true
    fi
}
expect_grep() { if grep -qE -- "$2" "$1"; then ok; else bad "$1 缺少 /$2/"; fi; }
expect_count() {
    local n
    n=$(grep -cE -- "$2" "$1" || true)
    if [[ "$n" == "$3" ]]; then ok; else bad "$1 里 /$2/ 出现 $n 次，应为 $3"; fi
}
# 重复运行：所有文件逐字节不变。
expect_idempotent() {
    local before
    before="$(cd "$T/cfg" && find . -type f -exec md5sum {} + | sort)"
    run_setup "$@" || return
    if [[ "$(cd "$T/cfg" && find . -type f -exec md5sum {} + | sort)" == "$before" ]]; then ok; else
        bad "第二次运行改动了文件"
    fi
}

# ── 1. 全新：三份文件都按默认写出 ──
new_case fresh
rm -rf "$F" "$W"
run_setup
expect_grep "$F/profile" '^Name=windinput$'
expect_grep "$F/profile" '^Name=keyboard-us$'
expect_grep "$F/profile" '^DefaultIM=windinput$'
expect_file "$F/config" <<'EOF'
[Hotkey/AltTriggerKeys]

EOF
expect_file "$W/config.toml" <<'EOF'
[schema]
active = "pinyin"
available = ["pinyin", "wubi86"]
EOF
expect_idempotent
[[ ! -e "$T/calls" ]] && ok || bad "fcitx5 未运行时不该碰它"

# ── 2. 已有 fcitx5 写出的 profile（带注释）：只追加一项，原内容一字不动 ──
new_case existing
cat >"$F/profile" <<'EOF'
[Groups/0]
# Group Name
Name=Default
# Layout
Default Layout=us
# Default Input Method
DefaultIM=pinyin

[Groups/0/Items/0]
# Name
Name=keyboard-us
# Layout
Layout=

[Groups/0/Items/1]
# Name
Name=pinyin
# Layout
Layout=

[GroupOrder]
0=Default

EOF
run_setup
expect_file "$F/profile" <<'EOF'
[Groups/0]
# Group Name
Name=Default
# Layout
Default Layout=us
# Default Input Method
DefaultIM=pinyin

[Groups/0/Items/0]
# Name
Name=keyboard-us
# Layout
Layout=

[Groups/0/Items/1]
# Name
Name=pinyin
# Layout
Layout=

[Groups/0/Items/2]
Name=windinput
Layout=

[GroupOrder]
0=Default

EOF
expect_idempotent

# ── 3. 多个输入法组：加进 Groups/0，序号按 Groups/0 自己的算，别的组不动 ──
new_case multigroup
cat >"$F/profile" <<'EOF'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=keyboard-us

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/1]
Name=Other
Default Layout=de
DefaultIM=keyboard-de

[Groups/1/Items/0]
Name=keyboard-de
Layout=

[Groups/1/Items/1]
Name=mozc
Layout=

[GroupOrder]
0=Default
1=Other
EOF
run_setup
expect_count "$F/profile" '^\[Groups/0/Items/1\]$' 1
expect_count "$F/profile" '^\[Groups/1/Items/2\]$' 0
expect_count "$F/profile" '^Name=windinput$' 1
expect_grep "$F/profile" '^1=Other$'
expect_grep "$F/profile" '^Name=mozc$'
expect_idempotent

# ── 4. 注释里出现节名：不能当成节头（旧实现会认成「已有该节」而什么也不做）──
new_case comment
cat >"$F/config" <<'EOF'
# 说明：[Hotkey/AltTriggerKeys] 出厂是左 Shift
[Hotkey]
EnumerateWithTriggerKeys=True

[Hotkey/TriggerKeys]
0=Control+space
EOF
run_setup
expect_file "$F/config" <<'EOF'
# 说明：[Hotkey/AltTriggerKeys] 出厂是左 Shift
[Hotkey]
EnumerateWithTriggerKeys=True

[Hotkey/TriggerKeys]
0=Control+space
[Hotkey/AltTriggerKeys]

EOF
expect_idempotent

# ── 5. 已有该节且有值：只清这一节，前后别的节原样 ──
new_case clear
cat >"$F/config" <<'EOF'
[Hotkey/TriggerKeys]
0=Control+space

[Hotkey/AltTriggerKeys]
0=Shift_L
1=Shift_R

[Behavior]
ShareInputState=No
EOF
run_setup
expect_file "$F/config" <<'EOF'
[Hotkey/TriggerKeys]
0=Control+space

[Hotkey/AltTriggerKeys]

[Behavior]
ShareInputState=No
EOF
expect_idempotent

# ── 6. 节头带尾随空格（旧实现静默不生效）──
new_case trailing
printf '[Hotkey/AltTriggerKeys]  \n0=Shift_L\n[Behavior]\nShareInputState=No\n' >"$F/config"
run_setup
expect_count "$F/config" 'Shift_L' 0
expect_grep "$F/config" '^ShareInputState=No$'
expect_idempotent

# ── 7. CRLF：识别得出、不重复加、不混入裸 LF ──
new_case crlf
printf '[Groups/0]\r\nName=Default\r\nDefault Layout=us\r\nDefaultIM=keyboard-us\r\n\r\n[Groups/0/Items/0]\r\nName=keyboard-us\r\nLayout=\r\n\r\n[GroupOrder]\r\n0=Default\r\n' >"$F/profile"
printf '[Hotkey/AltTriggerKeys]\r\n0=Shift_L\r\n\r\n[Behavior]\r\nShareInputState=No\r\n' >"$F/config"
run_setup
expect_count "$F/profile" '^Name=windinput' 1
expect_count "$F/config" 'Shift_L' 0
expect_grep "$F/config" '^ShareInputState=No'
for f in "$F/profile" "$F/config"; do
    [[ "$(grep -c $'\r$' "$f")" == "$(wc -l <"$f")" ]] && ok || bad "$f 混进了裸 LF 行"
done
expect_idempotent

# ── 8. config.toml 已用点号键 / 内联表 / 子表定义 schema：不追加（追加 [schema] 是重复定义）──
toml_cases() {
    local tag="$1"
    local i=0
    for content in 'schema.active = "wubi86"\n[ui]\nx = 1\n' \
                   'schema = { active = "wubi86" }\n' \
                   '"schema".active = "wubi86"\n' \
                   '[ schema ]\nactive = "wubi86"\n'; do
        i=$((i + 1))
        new_case "toml_${tag}_$i"
        printf "$content" >"$W/config.toml"
        local before
        before="$(md5sum <"$W/config.toml")"
        run_setup
        [[ "$(md5sum <"$W/config.toml")" == "$before" ]] && ok || bad "改动了已含 schema 的 config.toml：$(cat "$W/config.toml")"
    done
    # 没有 schema：补一段，原内容在前不动。
    new_case "toml_${tag}_append"
    printf '[keys]\nx = 1\n' >"$W/config.toml"
    run_setup
    expect_file "$W/config.toml" <<'EOF'
[keys]
x = 1

[schema]
active = "pinyin"
available = ["pinyin", "wubi86"]
EOF
    expect_idempotent
    # 只有 `[schema.xxx]` 子表：其后补 `[schema]` 合法，照补。
    new_case "toml_${tag}_subtable"
    printf '[schema.overrides]\nx = 1\n' >"$W/config.toml"
    run_setup
    expect_count "$W/config.toml" '^\[schema\]$' 1
    python3 -c 'import tomllib,sys; tomllib.load(open(sys.argv[1],"rb"))' "$W/config.toml" 2>/dev/null \
        && ok || { python3 -c 'import tomllib' 2>/dev/null && bad "补写后不是合法 TOML" || ok; }
    # 值里 / 别的表里出现 schema 字样不算。
    new_case "toml_${tag}_notschema"
    printf '[ui]\nschema_hint = "schema.active"\n' >"$W/config.toml"
    run_setup
    expect_count "$W/config.toml" '^\[schema\]$' 1
}
toml_cases native
# 模拟 22.04 的 Python 3.10（没有 tomllib），走按行判断那条路。
mkdir -p "$ROOT/no_tomllib"
echo 'raise ImportError("simulated python3.10")' >"$ROOT/no_tomllib/tomllib.py"
PYTHONPATH_FOR_TEST="$ROOT/no_tomllib" toml_cases py310

# ── 9. config.toml 语法有误：不动它（tomllib 可用时才分辨得出）──
if python3 -c 'import tomllib' 2>/dev/null; then
    new_case toml_broken
    printf '[keys\nx = 1\n' >"$W/config.toml"
    run_setup
    expect_file "$W/config.toml" <<'EOF'
[keys
x = 1
EOF
    expect_grep "$T/out" '语法错误'
fi

# ── 10. 符号链接（dotfiles 管理）：写到链接目标，链接保住；权限沿用 ──
new_case symlink
mkdir -p "$T/dotfiles"
printf '[Hotkey/AltTriggerKeys]\n0=Shift_L\n' >"$T/dotfiles/fcitx-config"
chmod 600 "$T/dotfiles/fcitx-config"
ln -s "$T/dotfiles/fcitx-config" "$F/config"
run_setup
[[ -L "$F/config" ]] && ok || bad "符号链接被替换成了普通文件"
expect_count "$T/dotfiles/fcitx-config" 'Shift_L' 0
[[ "$(stat -c %a "$T/dotfiles/fcitx-config")" == 600 ]] && ok || bad "权限没沿用：$(stat -c %a "$T/dotfiles/fcitx-config")"
[[ -z "$(find "$T/cfg" "$T/dotfiles" -name '.*' -type f)" ]] && ok || bad "留下了临时文件"

# ── 11. fcitx5 正在运行：先退出（它退出时写回旧 profile）再改，改完重新拉起 ──
new_case running
cat >"$T/fcitx_memory_profile" <<'EOF'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=keyboard-us

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[GroupOrder]
0=Default
EOF
cp "$T/fcitx_memory_profile" "$F/profile"
touch "$T/fcitx_running"
DISPLAY_FOR_TEST=:99 run_setup
expect_count "$F/profile" '^Name=windinput$' 1
expect_grep "$T/calls" '^remote -e$'
expect_grep "$T/calls" '^fcitx5 -d profile_has_windinput=1$'
expect_grep "$T/out" '先让它退出'

# 没有图形会话（如 SSH）或 --no-restart：只退出、不拉起，并说明。
new_case running_norestart
cp "$ROOT/running/fcitx_memory_profile" "$T/"
cp "$T/fcitx_memory_profile" "$F/profile"
touch "$T/fcitx_running"
DISPLAY_FOR_TEST=:99 run_setup --no-restart
expect_count "$F/profile" '^Name=windinput$' 1
expect_count "$T/calls" '^fcitx5 ' 0
expect_grep "$T/out" '未重新启动'

echo "test-setup: $PASS 通过, $FAIL 失败"
[[ $FAIL -eq 0 ]]
