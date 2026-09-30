#!/usr/bin/env bash
# 系统关联端到端：`windinput://` 协议与 `.wpkg` / `.wtheme` 包文件，从 XDG 查询一路到
# 真的拉起设置程序（wind_setting）进导入确认。
#
#   scripts/linux/e2e-assoc.sh              # 先构建设置程序再跑
#   SKIP_BUILD=1 scripts/linux/e2e-assoc.sh # 用现成产物
#   KEEP=1 scripts/linux/e2e-assoc.sh       # 跑完保留临时目录（截图、日志都在里面）
#
# 输出每条断言的 PASS/FAIL，末行「ASSOC E2E PASS」或「ASSOC E2E FAIL」，退出码随之。
# 截图在 $W/shots/*.png（跑完打印路径）：自动断言只能判「窗口不是一片空白」，
# 进没进对话框要**亲眼看**截图。
#
# 两段场景：
#   一、系统包形态：把包里的 .desktop / MIME 描述 / 图标按 deb 的布局装进一个假的系统数据目录
#      （XDG_DATA_DIRS），断言类型识别、默认处理者、xdg-open / gio open 拉起设置程序进对应导入流程、
#      已开着时再开不起第二个进程而是转给首实例、界面显示「系统安装包提供」。
#   二、便携形态：没有系统级文件。先验启动自愈（残留的用户级文件被改指本程序），再在界面上逐个点
#      「注册」「取消注册」，断言用户目录里文件的来去、默认处理者的变化、别人的关联原样不动。
#
# 隔离：HOME / XDG_{DATA,CONFIG,CACHE}_HOME / XDG_RUNTIME_DIR / XDG_DATA_DIRS / TMPDIR 全指进临时目录，
# 私有 Xvfb；设置程序走 WIND_RPC_MOCK=1（导入预览由 mock 应答，不需要输入法服务与 fcitx5）。
# 只杀本脚本拉起的进程（按 /proc/<pid>/environ 里的 XDG_RUNTIME_DIR 认），不碰并发的另一份 e2e。
#
# 依赖：
#   - xdg-utils（xdg-mime / xdg-open）、shared-mime-info（update-mime-database）、
#     libglib2.0-bin（gio）——要装在系统里（本机 Ubuntu 24.04 已有）；
#   - desktop-file-utils（update-desktop-database）：系统没有就 apt-get download + dpkg -x 到
#     /tmp/wi-linux/assoc-tools（无 root，不装进系统）；
#   - Xvfb-wind / xwd / xdotool：scripts/linux/bootstrap-sdk.sh 搭的用户前缀 SDK；
#   - python3（只用标准库：造 zip、xwd 转 PNG）。
# 不改 e2e.sh / e2e_client.py；二者互不依赖。
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SETTING_REPO="$(cd "$REPO/.." && pwd)/wind-setting"
SETTING_TARGET_DIR="${WIND_SETTING_TARGET_DIR:-$HOME/.cache/wi-tgt-setting-linux}"
SETTING_BIN="${WIND_ASSOC_SETTING:-$SETTING_TARGET_DIR/debug/wind_setting}"
PKG="$REPO/scripts/linux/pkg"
TOOLS=/tmp/wi-linux/assoc-tools
W="${WIND_ASSOC_DIR:-/tmp/wi-linux/assoc.$$}"

log() { echo "[assoc] $*"; }

# ── 构建与工具 ────────────────────────────────────────────────────────
if [[ -z "${SKIP_BUILD:-}" ]]; then
    log "构建设置程序（$SETTING_REPO）…"
    (cd "$SETTING_REPO" && CARGO_TARGET_DIR="$SETTING_TARGET_DIR" cargo build -q)
fi
[[ -x "$SETTING_BIN" ]] || { echo "找不到设置程序：$SETTING_BIN" >&2; exit 2; }
SETTING_BIN="$(readlink -f "$SETTING_BIN")"

eval "$("$REPO/scripts/linux/bootstrap-sdk.sh" env | grep '^export WIND_LINUX_SDK=')"
"$REPO/scripts/linux/bootstrap-sdk.sh" env >/dev/null   # 顺带补 Xvfb-wind 的 /tmp/wlx 垫片
SDK_BIN="$WIND_LINUX_SDK/bin"
SDK_USR="$WIND_LINUX_SDK/root/usr"
SDK_LIB="$SDK_USR/lib/$(gcc -print-multiarch 2>/dev/null || echo x86_64-linux-gnu)"
# SDK 里的 X 工具要它自己的库；只对这几个命令设 LD_LIBRARY_PATH，免得系统 gio 串到 SDK 的 glib。
xdo() { LD_LIBRARY_PATH="$SDK_LIB" "$SDK_USR/bin/xdotool" "$@"; }
xwdump() { LD_LIBRARY_PATH="$SDK_LIB" "$SDK_USR/bin/xwd" "$@"; }

for t in xdg-mime xdg-open update-mime-database; do
    command -v "$t" >/dev/null || { echo "缺 $t（xdg-utils / shared-mime-info）" >&2; exit 2; }
done
[[ -x /usr/bin/gio ]] || { echo "缺 /usr/bin/gio（libglib2.0-bin）" >&2; exit 2; }
if ! command -v update-desktop-database >/dev/null && [[ ! -x "$TOOLS/root/usr/bin/update-desktop-database" ]]; then
    log "下载 desktop-file-utils 到 $TOOLS（不装进系统）…"
    mkdir -p "$TOOLS"
    (cd "$TOOLS" && apt-get download desktop-file-utils >/dev/null && dpkg -x desktop-file-utils_*.deb root)
fi

# ── 隔离环境 ──────────────────────────────────────────────────────────
rm -rf "$W"
mkdir -p "$W"/{home,rt,tmp,shots,files,sys/share,sys/lib/windinput,base/share/mime/packages}
chmod 700 "$W/rt"
# 换了 HOME 后 fontconfig 找不到用户装的字体（本机的中文字体就在 ~/.fonts），界面全成豆腐块，
# 截图没法看：把用户字体目录软链进来（只读用）。
for d in .fonts .local/share/fonts; do
    [[ -d "$HOME/$d" ]] && mkdir -p "$(dirname "$W/home/$d")" && ln -s "$HOME/$d" "$W/home/$d"
done
export HOME="$W/home"
export XDG_DATA_HOME="$W/home/.local/share" XDG_CONFIG_HOME="$W/home/.config" XDG_CACHE_HOME="$W/home/.cache"
export XDG_RUNTIME_DIR="$W/rt" TMPDIR="$W/tmp"
# xdg-utils 按桌面分派：GNOME 下 filetype 走 `gio info`（认 shared-mime-info 的 glob），
# xdg-open 走 `gio open`。generic 分支的 filetype 用 `file`，根本不看 MIME 库，测不出东西。
export XDG_CURRENT_DESKTOP=GNOME
unset DESKTOP_SESSION KDE_FULL_SESSION KDE_SESSION_VERSION GNOME_DESKTOP_SESSION_ID XDG_MENU_PREFIX DISPLAY WAYLAND_DISPLAY
export WIND_RPC_MOCK=1
export PATH="$TOOLS/root/usr/bin:/usr/bin:/bin"
BASE="$W/base/share"    # 通用 MIME 库（freedesktop.org.xml），两段都在
SYS="$W/sys/share"      # 假系统目录（deb 的 /usr/share）
set_system() {          # $1=1 带系统级文件；0 不带
    if [[ "$1" == 1 ]]; then export XDG_DATA_DIRS="$SYS:$BASE"; else export XDG_DATA_DIRS="$BASE"; fi
}
cp /usr/share/mime/packages/freedesktop.org.xml "$BASE/mime/packages/"
update-mime-database "$BASE/mime" >/dev/null 2>&1

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "PASS  $*"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL  $*"; }
expect_eq() {  # 名称 期望 实际
    if [[ "$2" == "$3" ]]; then ok "$1（$3）"; else bad "$1：期望「$2」，实际「$3」"; fi
}

XVFB=""
cleanup() {
    kill_ours
    [[ -n "$XVFB" ]] && kill "$XVFB" 2>/dev/null || true
    if [[ -z "${KEEP:-}" && "$FAIL" == 0 ]]; then rm -rf "$W"; else log "保留临时目录：$W"; fi
}
trap cleanup EXIT

# 本脚本拉起的 wind_setting：环境里 XDG_RUNTIME_DIR 指向本临时目录的那些。
our_pids() {
    local p
    for p in $(pgrep -x wind_setting 2>/dev/null || true); do
        if tr '\0' '\n' <"/proc/$p/environ" 2>/dev/null | grep -qx "XDG_RUNTIME_DIR=$W/rt"; then echo "$p"; fi
    done
}
kill_ours() {
    local pids
    pids="$(our_pids)"
    [[ -z "$pids" ]] && return 0
    kill $pids 2>/dev/null || true
    for _ in $(seq 1 50); do [[ -z "$(our_pids)" ]] && break; sleep 0.1; done
    rm -f "$W"/rt/*_si.sock
}
wait_window() {  # 等设置程序主窗出现，打印窗口 id
    local id=""
    for _ in $(seq 1 150); do
        id="$(xdo search --onlyvisible --classname wind_setting 2>/dev/null | head -1 || true)"
        [[ -n "$id" ]] && { echo "$id"; return 0; }
        sleep 0.1
    done
    return 1
}
setting_log() { cat "$(find "$XDG_DATA_HOME" -name 'wind_setting*.1.log' 2>/dev/null | head -1)" 2>/dev/null || true; }
wait_log() {  # 等日志里出现某串
    for _ in $(seq 1 100); do setting_log | grep -qF -- "$1" && return 0; sleep 0.1; done
    return 1
}
# 截窗口 → PNG，打印「不同颜色数」：一片空白的窗只有一两种颜色。
shot() {  # 名称 窗口id
    local out="$W/shots/$1.png"
    xwdump -id "$2" -silent | python3 -c '
import struct, sys, zlib
d = sys.stdin.buffer.read()
h = struct.unpack(">25I", d[:100])
hs, w, ht, order, bpl, nc = h[0], h[4], h[5], h[7], h[12], h[19]
px = d[hs + nc * 12:]
raw, seen = bytearray(), set()
for y in range(ht):
    raw.append(0)
    row = px[y * bpl:y * bpl + w * 4]
    for x in range(0, w * 4, 4):
        c = (row[x + 2], row[x + 1], row[x]) if order == 0 else (row[x + 1], row[x + 2], row[x + 3])
        raw.extend(c)
        seen.add(c)
ck = lambda t, b: struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + ck(b"IHDR", struct.pack(">IIBBBBB", w, ht, 8, 2, 0, 0, 0))
    + ck(b"IDAT", zlib.compress(bytes(raw), 6)) + ck(b"IEND", b""))
print(len(seen))
' "$out"
}
expect_rendered() {  # 名称 窗口id
    local n
    n="$(shot "$1" "$2")"
    if [[ "$n" -gt 50 ]]; then ok "截图 $1 有内容（$n 色）→ $W/shots/$1.png"; else bad "截图 $1 像是空窗（$n 色）"; fi
}
# windui 的 Linux 后端在主线程收下转来的 argv，要等下一次输入事件才处理（见 wind-setting README
# 「已知限制」）：给窗口一个鼠标移动，模拟用户把鼠标移过去。
nudge() { xdo mousemove --window "$1" 5 5 >/dev/null; sleep 0.3; xdo mousemove --window "$1" 8 8 >/dev/null; }
click() { xdo mousemove --window "$1" "$2" "$3" click 1 >/dev/null; sleep 0.8; }
default_of() { xdg-mime query default "$1" 2>/dev/null | tr -d '\n'; }

# 两种包各造一个（内容是 zip：与真包一样会被魔数认成 zip，考的是 glob 能不能压过它）
python3 - "$W/files" <<'PY'
import sys, zipfile, os
for n in ("demo.wpkg", "demo.wtheme"):
    with zipfile.ZipFile(os.path.join(sys.argv[1], n), "w") as z:
        z.writestr("manifest.toml", "name = \"e2e\"\n")
PY
# 覆盖确认框「继续」按钮的窗口坐标（截图 h_overwrite_confirm 可对照）
CONFIRM_OK_X=600 CONFIRM_OK_Y=397
URL='windinput://import/theme?url=https%3A%2F%2Fexample.invalid%2Fdark.wtheme&name=E2E'

# ── 私有 Xvfb ─────────────────────────────────────────────────────────
XN=$(( 120 + $$ % 60 ))
while [[ -e /tmp/.X11-unix/X$XN ]]; do XN=$((XN + 1)); done
LD_LIBRARY_PATH="$SDK_LIB" "$SDK_BIN/Xvfb-wind" ":$XN" -screen 0 1280x800x24 -nolisten tcp \
    -xkbdir /usr/share/X11/xkb >"$W/xvfb.log" 2>&1 &
XVFB=$!
for _ in $(seq 1 50); do [[ -e /tmp/.X11-unix/X$XN ]] && break; sleep 0.1; done
export DISPLAY=":$XN"
log "Xvfb 就绪 DISPLAY=$DISPLAY，临时目录 $W"

# ═════ 一、系统包形态 ═════════════════════════════════════════════════
log "── 一、系统包形态（假 /usr/share 里装包内文件，Exec 指向本次构建的设置程序）"
ln -s "$SETTING_BIN" "$W/sys/lib/windinput/wind_setting"
mkdir -p "$SYS/applications" "$SYS/mime/packages" "$SYS/icons"
for f in windinput-setting.desktop windinput-import.desktop; do
    sed "s#/usr/lib/windinput/wind_setting#$W/sys/lib/windinput/wind_setting#" "$PKG/$f" >"$SYS/applications/$f"
done
cp "$PKG/windinput-mime.xml" "$SYS/mime/packages/windinput.xml"
cp -r "$REPO/wind_linux/data/icons/hicolor" "$SYS/icons/"
update-mime-database "$SYS/mime" >/dev/null 2>&1
update-desktop-database "$SYS/applications"
set_system 1

# a) 类型识别 + 图标名
expect_eq "a1 .wpkg 类型" application/x-windinput-package "$(xdg-mime query filetype "$W/files/demo.wpkg" | tr -d '\n')"
expect_eq "a2 .wtheme 类型" application/x-windinput-theme "$(xdg-mime query filetype "$W/files/demo.wtheme" | tr -d '\n')"
icon="$(/usr/bin/gio info -a standard::icon "$W/files/demo.wpkg" | sed -n 's/.*standard::icon: //p')"
if [[ "$icon" == application-x-windinput-package,* ]] \
    && [[ -f "$SYS/icons/hicolor/48x48/mimetypes/application-x-windinput-package.png" ]]; then
    ok "a3 .wpkg 的首选图标名是本包专属图标，且 hicolor 里有该文件（$icon）"
else
    bad "a3 .wpkg 图标名：$icon"
fi
icon="$(/usr/bin/gio info -a standard::icon "$W/files/demo.wtheme" | sed -n 's/.*standard::icon: //p')"
[[ "$icon" == application-x-windinput-theme,* ]] && ok "a4 .wtheme 图标名（${icon%%,*}）" || bad "a4 .wtheme 图标名：$icon"

# b) 默认处理者（xdg-mime 与 gio 各查一遍：xdg-open 在别的桌面走前者，GNOME 走后者）
expect_eq "b1 协议默认" windinput-setting.desktop "$(default_of x-scheme-handler/windinput)"
expect_eq "b2 方案包默认" windinput-import.desktop "$(default_of application/x-windinput-package)"
expect_eq "b3 主题包默认" windinput-import.desktop "$(default_of application/x-windinput-theme)"
/usr/bin/gio mime x-scheme-handler/windinput 2>/dev/null | grep -q 'windinput-setting.desktop' \
    && ok "b4 gio 也认协议处理者" || bad "b4 gio mime 查不到协议处理者"

# c) 协议链接冷启动 → 外观页 + 主题导入确认
xdg-open "$URL" >"$W/xdg-open.log" 2>&1 || true
if WID="$(wait_window)"; then
    ok "c1 xdg-open windinput://… 拉起了设置程序"
    wait_log "协议导入请求: kind=theme" && ok "c2 设置程序收到协议 URL 并进主题导入" || bad "c2 日志里没有协议导入记录"
    sleep 1.5
    expect_rendered c_protocol_theme "$WID"
else
    bad "c1 xdg-open 没拉起设置程序（$(cat "$W/xdg-open.log")）"
fi

kill_ours

# d) 已开着时：xdg-open 双击 .wpkg → 不开第二个进程，参数转给首实例，进方案包导入确认。
#    首实例以空参数起（不带着上一步的主题对话框），截图里只该有方案包那一个确认框。
"$SETTING_BIN" >/dev/null 2>&1 &
WID="$(wait_window)" || bad "d0 设置程序没起来"
sleep 1.5
FIRST="$(our_pids)"
xdg-open "$W/files/demo.wpkg" >>"$W/xdg-open.log" 2>&1 || true
sleep 2
expect_eq "d1 仍只有一个设置程序进程" "$FIRST" "$(our_pids)"
wait_log "二次实例 argv" && setting_log | grep "二次实例 argv" | grep -qF "demo.wpkg" \
    && ok "d2 .wpkg 路径被转给首实例" || bad "d2 首实例没收到 .wpkg 路径"
if [[ -n "${WID:-}" ]]; then nudge "$WID"; sleep 1.5; expect_rendered d_second_open_wpkg "$WID"; fi
kill_ours

# h) 系统包形态下的高级页：应显示「系统安装包提供，在此不能取消」、取消注册灰掉。
#    再让 .wpkg 的默认被别的程序抢走：那一行应显示占用者；点「注册」→ 确认覆盖 → 默认回到我们，
#    且因为系统包那份就指向本程序，用户目录里不该多出会遮住它的 .desktop。
cat >"$SYS/applications/ark.desktop" <<'DESK'
[Desktop Entry]
Type=Application
Name=Ark
Exec=/usr/bin/ark %f
MimeType=application/x-windinput-package;
DESK
update-desktop-database "$SYS/applications"
xdg-mime default ark.desktop application/x-windinput-package
expect_eq "h0 方案包默认被抢走" ark.desktop "$(default_of application/x-windinput-package)"
"$SETTING_BIN" --page advanced >/dev/null 2>&1 &
if WID="$(wait_window)"; then
    sleep 2
    expect_rendered h_advanced_system_managed "$WID"
    click "$WID" 805 184                     # .wpkg 行「注册」→ 覆盖确认框
    sleep 0.5
    expect_rendered h_overwrite_confirm "$WID"
    click "$WID" "$CONFIRM_OK_X" "$CONFIRM_OK_Y"
    sleep 1
    expect_rendered h_after_overwrite "$WID"
fi
expect_eq "h1 覆盖后方案包默认回到我们" windinput-import.desktop "$(default_of application/x-windinput-package)"
[[ ! -e "$XDG_DATA_HOME/applications/windinput-import.desktop" ]] \
    && ok "h2 系统包形态下注册不写用户级 .desktop" || bad "h2 多写了用户级 .desktop"
kill_ours
rm -f "$SYS/applications/ark.desktop" "$XDG_CONFIG_HOME/mimeapps.list"
update-desktop-database "$SYS/applications"

# c') .wtheme 冷启动（gio open，就是 GNOME 文件管理器双击走的那条）→ 主题包导入确认
/usr/bin/gio open "$W/files/demo.wtheme" >>"$W/xdg-open.log" 2>&1 || true
if WID="$(wait_window)"; then
    ok "c3 gio open .wtheme 拉起了设置程序"
    sleep 2
    expect_rendered c_gio_open_wtheme "$WID"
else
    bad "c3 gio open .wtheme 没拉起设置程序"
fi
kill_ours

# ═════ 二、便携形态（无系统级文件） ═══════════════════════════════════
log "── 二、便携形态（XDG_DATA_DIRS 里没有我们的文件）"
set_system 0
rm -rf "$XDG_CONFIG_HOME"
find "$XDG_DATA_HOME" -mindepth 1 -maxdepth 1 ! -name fonts -exec rm -rf {} +   # 字体软链留着
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME/applications"
UAPPS="$XDG_DATA_HOME/applications"
UXML="$XDG_DATA_HOME/mime/packages/windinput.xml"
expect_eq "e0 起点：协议无人处理" "" "$(default_of x-scheme-handler/windinput)"

# g) 启动自愈：用户目录里有本程序写过、指向已消失路径的 .desktop → 启动时改指本程序
{
    echo "[Desktop Entry]"; echo "Type=Application"; echo "Name=清风输入法设置"
    echo "Exec=\"$W/gone/wind_setting\" %u"; echo "MimeType=x-scheme-handler/windinput;"
    echo "X-WindInput-UserInstall=true"
} >"$UAPPS/windinput-setting.desktop"
"$SETTING_BIN" --page advanced >/dev/null 2>&1 &
WID="$(wait_window)" || true
if grep -qF "Exec=\"$SETTING_BIN\" %u" "$UAPPS/windinput-setting.desktop"; then
    ok "g1 自愈把残留的用户级 .desktop 改指本程序"
else
    bad "g1 自愈没修：$(grep Exec= "$UAPPS/windinput-setting.desktop")"
fi
kill_ours
rm -f "$UAPPS/windinput-setting.desktop"
update-desktop-database "$UAPPS"   # 手工删的，缓存要跟着刷，否则 xdg-mime 仍报出这个名字

# e) 界面上逐个注册 → 断言 → 逐个取消 → 断言
# 预置别人的关联：取消注册后必须原样还在。
printf '[Default Applications]\ntext/html=firefox.desktop\napplication/zip=org.gnome.FileRoller.desktop\n' \
    >"$XDG_CONFIG_HOME/mimeapps.list"
cp "$XDG_CONFIG_HOME/mimeapps.list" "$W/mimeapps.before"
"$SETTING_BIN" --page advanced >/dev/null 2>&1 &
if WID="$(wait_window)"; then
    sleep 2
    expect_rendered e_before_register "$WID"
    # 「系统集成」卡片三行的按钮位置（窗口坐标，高级页顶部；截图 e_before_register 可对照）
    for y in 129 184 239; do click "$WID" 805 "$y"; done
    sleep 1
    expect_rendered e_after_register "$WID"
else
    bad "e 设置程序没起来"
fi
[[ -f "$UAPPS/windinput-setting.desktop" && -f "$UAPPS/windinput-import.desktop" && -f "$UXML" ]] \
    && ok "e1 注册写入了用户级 .desktop ×2 与 MIME 描述" || bad "e1 用户级文件缺：$(ls "$UAPPS" "$(dirname "$UXML")" 2>&1 | tr '\n' ' ')"
grep -qF "Exec=\"$SETTING_BIN\" %u" "$UAPPS/windinput-setting.desktop" 2>/dev/null \
    && ok "e2 用户级 .desktop 的 Exec 是本程序绝对路径" || bad "e2 Exec 不对"
expect_eq "e3 协议默认" windinput-setting.desktop "$(default_of x-scheme-handler/windinput)"
expect_eq "e4 方案包默认" windinput-import.desktop "$(default_of application/x-windinput-package)"
expect_eq "e5 主题包默认" windinput-import.desktop "$(default_of application/x-windinput-theme)"
expect_eq "e6 用户 MIME 库认 .wpkg" application/x-windinput-package "$(xdg-mime query filetype "$W/files/demo.wpkg" | tr -d '\n')"
# 注册后的协议链接：首实例开着，应转过去
xdg-open "$URL" >>"$W/xdg-open.log" 2>&1 || true
sleep 2
[[ -n "${WID:-}" ]] && nudge "$WID"
setting_log | grep "二次实例 argv" | grep -qF "windinput://import/theme" \
    && ok "e7 用户级注册后 xdg-open 链接转到了首实例" || bad "e7 首实例没收到协议 URL"
[[ -n "${WID:-}" ]] && { sleep 1; expect_rendered e_protocol_after_user_register "$WID"; }
kill_ours

"$SETTING_BIN" --page advanced >/dev/null 2>&1 &
if WID="$(wait_window)"; then
    sleep 2
    for y in 129 184 239; do click "$WID" 880 "$y"; done
    sleep 1
    expect_rendered e_after_unregister "$WID"
fi
kill_ours

# f) 取消后：我们的文件不留、默认处理者没了、别人的关联一字不差
left="$(ls "$UAPPS"/windinput-*.desktop "$UXML" 2>/dev/null || true)"
[[ -z "$left" ]] && ok "f1 用户级文件已全部删除" || bad "f1 残留：$left"
expect_eq "f2 协议无人处理" "" "$(default_of x-scheme-handler/windinput)"
expect_eq "f3 方案包无人处理" "" "$(default_of application/x-windinput-package)"
if diff -q "$W/mimeapps.before" "$XDG_CONFIG_HOME/mimeapps.list" >/dev/null; then
    ok "f4 mimeapps.list 与注册前逐字节一致（别人的关联没动）"
else
    bad "f4 mimeapps.list 变了：$(diff "$W/mimeapps.before" "$XDG_CONFIG_HOME/mimeapps.list" | tr '\n' ' ')"
fi

echo
log "截图：$W/shots/"
if [[ "$FAIL" == 0 ]]; then echo "ASSOC E2E PASS（$PASS 条）"; else echo "ASSOC E2E FAIL（$FAIL 失败 / $((PASS + FAIL)) 条）"; exit 1; fi
