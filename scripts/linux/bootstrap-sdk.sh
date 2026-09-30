#!/usr/bin/env bash
# 在无 root 的机器上搭 wind_linux（Fcitx5 addon）的开发 / 测试环境。幂等，可重复执行。
#
# 做法：apt-get download 取 .deb → dpkg -x 解到用户前缀，**不往 /usr 写任何东西**。
# 依赖闭包按 `apt-cache depends --recurse` 求，只下本机「未安装」的那部分。
# cmake 走 python venv + pip（发行版的 cmake 包依赖太多）。
#
# 用法：
#   scripts/linux/bootstrap-sdk.sh            # 装到默认前缀
#   WIND_LINUX_SDK=/some/dir scripts/linux/bootstrap-sdk.sh
#   eval "$(scripts/linux/bootstrap-sdk.sh env)"   # 只打印环境变量（供 shell 引入）
#
# 装好后的环境变量由 `env` 子命令打印；scripts/linux/e2e.sh 与 wind_linux 的构建会自行 source。
set -euo pipefail

SDK="${WIND_LINUX_SDK:-$HOME/.local/opt/fcitx5-sdk}"
ROOT="$SDK/root"          # dpkg -x 的解包根（内含 usr/…）
DEBS="$SDK/debs"          # 下载的 .deb 缓存
VENV="$SDK/venv"          # cmake 所在的 python venv
MULTIARCH="$(gcc -print-multiarch 2>/dev/null || echo x86_64-linux-gnu)"

# 第一阶段（输入通路）+ 第二阶段（X11 候选窗 / Xvfb 截图）要的顶层包。
SEED_PKGS=(
    fcitx5 fcitx5-modules fcitx5-data
    libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev fcitx5-modules-dev
    extra-cmake-modules
    xvfb x11-apps x11-utils xdotool xcompmgr
    libxcb1-dev libxcb-shm0-dev libxcb-render0-dev libxcb-shape0-dev
)

# 让 Xvfb-wind 找得到 xkbcomp（见下文「Xvfb 启动时」一段）。/tmp 重启即清，故每次都补。
xvfb_shim() {
    [[ -L /tmp/wlx || ! -e /tmp/wlx ]] && ln -sfn "$ROOT/usr/bin" /tmp/wlx
}

print_env() {
    local lib="$ROOT/usr/lib/$MULTIARCH"
    cat <<EOF
export WIND_LINUX_SDK="$SDK"
export PATH="$SDK/bin:$VENV/bin:$ROOT/usr/bin:\$PATH"
export PKG_CONFIG_PATH="$lib/pkgconfig:$ROOT/usr/lib/pkgconfig:$ROOT/usr/share/pkgconfig\${PKG_CONFIG_PATH:+:\$PKG_CONFIG_PATH}"
export CMAKE_PREFIX_PATH="$ROOT/usr\${CMAKE_PREFIX_PATH:+:\$CMAKE_PREFIX_PATH}"
export LD_LIBRARY_PATH="$lib:$lib/fcitx5\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
EOF
}

if [[ "${1:-}" == "env" ]]; then
    xvfb_shim
    print_env
    exit 0
fi

mkdir -p "$ROOT" "$DEBS"

# ── 1. 求依赖闭包里本机缺的包 ──────────────────────────────────────────
echo "[bootstrap] 计算依赖闭包…"
mapfile -t CLOSURE < <(
    apt-cache depends --recurse --no-recommends --no-suggests --no-conflicts \
        --no-breaks --no-replaces --no-enhances "${SEED_PKGS[@]}" 2>/dev/null |
        grep -v '^ ' | grep -v '^<' | sort -u
)
MISSING=()
for p in "${CLOSURE[@]}"; do
    # 只要本机已装（任一架构）就跳过；虚包（apt-cache 给出 <…>）已在上面滤掉
    if ! dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed'; then
        MISSING+=("$p")
    fi
done
echo "[bootstrap] 闭包 ${#CLOSURE[@]} 个包，本机缺 ${#MISSING[@]} 个"

# ── 2. 下载（已下载的跳过）──────────────────────────────────────────────
cd "$DEBS"
for p in "${MISSING[@]}"; do
    if compgen -G "${p}_*.deb" >/dev/null; then continue; fi
    # 同名多架构时 apt 会选本机架构；:i386 之类的条目不在闭包里
    if apt-get download "$p" >/dev/null 2>&1; then continue; fi
    # 没有 root 就没法 apt update：本机包索引陈旧时 candidate 版本可能已从镜像下架（404），
    # 按版本表逐个退而求其次（noble-security 那份通常还在）。
    ok=0
    for ver in $(apt-cache madison "$p" 2>/dev/null | awk -F'|' '{gsub(/ /,"",$2); print $2}'); do
        if apt-get download "$p=$ver" >/dev/null 2>&1; then ok=1; break; fi
    done
    [[ $ok == 1 ]] || echo "[bootstrap] 警告：下载失败 $p（可能是纯虚包，忽略）"
done

# ── 3. 解包（按 deb 文件名记戳，已解的跳过）─────────────────────────────
mkdir -p "$SDK/.extracted"
for deb in "$DEBS"/*.deb; do
    stamp="$SDK/.extracted/$(basename "$deb").done"
    [[ -f "$stamp" ]] && continue
    dpkg -x "$deb" "$ROOT"
    touch "$stamp"
done

# ── 4. 修正 .pc 与 cmake 配置里写死的 /usr 前缀 ─────────────────────────
# pkg-config 的 prefix=/usr 会把 -I 指回系统目录；改成解包根。cmake 的 *Config.cmake
# 按自身位置推导前缀，但 Fcitx5 的 *Targets.cmake 把 INTERFACE_INCLUDE_DIRECTORIES 写成了
# 绝对的 "/usr/include/…"（cmake 配置期就报「路径不存在」），同样改指解包根。
find "$ROOT/usr" -name '*.pc' -print0 | while IFS= read -r -d '' pc; do
    sed -i "s#^prefix=/usr\$#prefix=$ROOT/usr#" "$pc"
done
find "$ROOT/usr/lib" -path '*/cmake/*' -name '*Targets.cmake' -print0 | while IFS= read -r -d '' t; do
    sed -i "s#\"/usr/include#\"$ROOT/usr/include#g" "$t"
done

# 有的 .so 开发链接是指向 /lib/... 的绝对软链（解包后悬空），改成相对。
find "$ROOT/usr/lib" -type l -name '*.so' -print0 | while IFS= read -r -d '' l; do
    tgt="$(readlink "$l")"
    if [[ "$tgt" == /* && ! -e "$tgt" && -e "$ROOT$tgt" ]]; then
        ln -sf "$ROOT$tgt" "$l"
    fi
done

# 同一个头文件目录被拆在两处：本机装了 libxcb1-dev（/usr/include/xcb/xcb.h），SDK 里解出了
# libxcb-shape0-dev（…/include/xcb/shape.h）。shape.h 用 `#include "xcb.h"` 按**自身所在目录**
# 找，找不到系统那份。把系统目录里缺的头补软链进来。
if [[ -d "$ROOT/usr/include/xcb" && -d /usr/include/xcb ]]; then
    for h in /usr/include/xcb/*.h; do
        [[ -e "$ROOT/usr/include/xcb/$(basename "$h")" ]] || ln -s "$h" "$ROOT/usr/include/xcb/"
    done
fi

# Xvfb 启动时按写死的 `/usr/bin` 去找 xkbcomp（键盘表编译器），没 root 装不到那里。
# 复制一份 Xvfb，把那个字符串原地改成同样 8 字节的 `/tmp/wlx`，再由 `xvfb_shim` 把
# /tmp/wlx 软链到解包根的 usr/bin。xkb 数据目录走 Xvfb 自带的 `-xkbdir` 参数，不用改。
if [[ -x "$ROOT/usr/bin/Xvfb" && ! -x "$SDK/bin/Xvfb-wind" ]]; then
    mkdir -p "$SDK/bin"
    python3 - "$ROOT/usr/bin/Xvfb" "$SDK/bin/Xvfb-wind" <<'PY'
import sys
src, dst = sys.argv[1], sys.argv[2]
data = open(src, "rb").read()
old, new = b"\0/usr/bin\0", b"\0/tmp/wlx\0"
assert data.count(old) >= 1, "Xvfb 里没找到 /usr/bin 字符串，版本变了？"
open(dst, "wb").write(data.replace(old, new))
PY
    chmod +x "$SDK/bin/Xvfb-wind"
fi

# ── 5. cmake + e2e 用的 DBus 客户端库（venv + pip）─────────────────────
# dbus-next 是纯 Python 的 DBus 实现：e2e 用它扮演「应用」对 Fcitx5 调 ProcessKeyEvent、
# 收 CommitString 信号，免得依赖需要编译的 dbus-python。
if [[ ! -x "$VENV/bin/cmake" ]]; then
    echo "[bootstrap] 安装 cmake 到 venv…"
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet cmake
fi
if ! "$VENV/bin/python" -c 'import dbus_next' 2>/dev/null; then
    echo "[bootstrap] 安装 dbus-next 到 venv…"
    "$VENV/bin/pip" install --quiet dbus-next
fi

# ── 6. 自检 ───────────────────────────────────────────────────────────
eval "$(print_env)"
echo "[bootstrap] cmake: $(cmake --version | head -1)"
echo "[bootstrap] Fcitx5Core (pkg-config): $(pkg-config --modversion Fcitx5Core 2>/dev/null || echo 未找到)"
if [[ -x "$ROOT/usr/bin/fcitx5" ]]; then
    missing_libs="$(ldd "$ROOT/usr/bin/fcitx5" | grep 'not found' || true)"
    if [[ -n "$missing_libs" ]]; then
        echo "[bootstrap] 警告：fcitx5 仍缺动态库："
        echo "$missing_libs"
    else
        echo "[bootstrap] fcitx5: $("$ROOT/usr/bin/fcitx5" --version 2>/dev/null || echo '可执行')"
    fi
fi
echo "[bootstrap] 完成。引入环境：eval \"\$($0 env)\""
