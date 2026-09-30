#!/usr/bin/env bash
# 由 package-deb.sh 在 Ubuntu 22.04 容器里调用，不要直接在宿主上运行。
# 挂载：/src 仓库（只读）、/data 词库、/work 构建缓存、/out 产物。
set -euxo pipefail

export PATH=/opt/cargo/bin:$PATH
: "${DEB_VERSION:?}"

# ── 服务 ──
cd /src/wind_input
cargo build --release --locked -p wind_service --features linux-host

# ── addon ──
cmake -S /src/wind_linux -B /work/cmake -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
cmake --build /work/cmake
S=/work/stage/pkg
rm -rf "$S"
DESTDIR="$S" cmake --install /work/cmake

# ── 组装 ──
install -Dm755 /work/target/release/wind_input "$S/usr/lib/windinput/wind_input"
mkdir -p "$S/usr/lib/windinput/data"
cp -a /data/. "$S/usr/lib/windinput/data/"
# addon / 输入法描述：cmake 只装了库，描述文件（构建目录里 configure_file 出来的）补进去。
install -Dm644 /work/cmake/share/fcitx5/addon/windinput.conf "$S/usr/share/fcitx5/addon/windinput.conf"
install -Dm644 /work/cmake/share/fcitx5/inputmethod/windinput.conf "$S/usr/share/fcitx5/inputmethod/windinput.conf"
install -Dm755 /src/scripts/linux/pkg/windinput-setup "$S/usr/bin/windinput-setup"
install -Dm644 /src/wind_linux/README.md "$S/usr/share/doc/windinput/README.md"

mkdir -p "$S/DEBIAN"
SIZE=$(du -sk --apparent-size "$S" | cut -f1)
cat >"$S/DEBIAN/control" <<CONTROL
Package: windinput
Version: $DEB_VERSION
Architecture: amd64
Maintainer: WindInput <noreply@windinput.com>
Section: utils
Priority: optional
Installed-Size: $SIZE
Depends: libc6 (>= 2.35), fcitx5 (>= 5.0.14), libxcb1, libxcb-shape0, fonts-noto-cjk | fonts-wqy-microhei | fonts-wqy-zenhei
Recommends: fcitx5-frontend-gtk3, fcitx5-frontend-qt5 | fcitx5-frontend-qt6, im-config, xclip | wl-clipboard, python3
Homepage: https://windinput.com
Description: 清风输入法 (WindInput) —— Fcitx5 输入法引擎
 清风输入法的 Linux 版（测试版）：Rust 服务负责输入逻辑与候选窗渲染，
 Fcitx5 addon 负责与应用对接。安装后运行 windinput-setup 配置当前用户。
CONTROL
cat >"$S/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
# 升级时旧服务还在跑旧二进制：结束它，addon 在下次连不上时会自动拉起新版。
# （已加载进 fcitx5 的旧 addon 需重启 fcitx5 或重新登录才换新。）
pkill -x wind_input 2>/dev/null || true
echo "清风输入法已安装。请对每个使用者运行一次： windinput-setup   然后注销并重新登录（升级则重启 fcitx5：fcitx5 -rd）。"
exit 0
POSTINST
chmod 755 "$S/DEBIAN/postinst"

dpkg-deb --root-owner-group --build "$S" "/out/windinput_${DEB_VERSION}_amd64.deb"
