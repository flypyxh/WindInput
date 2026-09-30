#!/usr/bin/env bash
# 由 package-deb.sh 在 Ubuntu 22.04 容器里调用，不要直接在宿主上运行。
# 挂载：/src 仓库（只读）、/data 词库、/work 构建缓存、/out 产物；
#       /ws/{WindInput,wind-setting,wind-ui-rust} 设置程序及其 path 依赖的兄弟布局（只读）。
set -euxo pipefail

export PATH=/opt/cargo/bin:$PATH
: "${DEB_VERSION:?}"

# ── 服务 ──
cd /src/wind_input
cargo build --release --locked -p wind_service --features linux-host

# ── 设置程序 ──
# 单独的 target 目录：与服务同名的 path 依赖（wind-ipc 等）在这里从 /ws/WindInput 解析，
# 和上面从 /src 解析的是两份源码路径，混用一个 target 只会互相作废缓存。
# 版本号与服务同源（docs/VERSION），不走 git——worktree 的 .git 指向宿主路径，容器里读不到。
(cd /ws/wind-setting && WIND_APP_VERSION="${APP_VERSION:?}" CARGO_TARGET_DIR=/work/target-setting \
    cargo build --release --locked)

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
# 应用兼容规则是平台专属策略：Windows 版 compat.toml 里全是 Windows 宿主的修正，不能随 Linux 包带。
# 换成「字段说明 + 零条内置规则」的 Linux 版（生成脚本自检不得残留规则表）。
bash /src/scripts/linux/gen-compat.sh /data/compat.toml "$S/usr/lib/windinput/data/compat.toml"
# addon / 输入法描述：cmake 只装了库，描述文件（构建目录里 configure_file 出来的）补进去。
install -Dm644 /work/cmake/share/fcitx5/addon/windinput.conf "$S/usr/share/fcitx5/addon/windinput.conf"
install -Dm644 /work/cmake/share/fcitx5/inputmethod/windinput.conf "$S/usr/share/fcitx5/inputmethod/windinput.conf"
install -Dm755 /src/scripts/linux/pkg/windinput-setup "$S/usr/bin/windinput-setup"
install -Dm755 /work/target-setting/release/wind_setting "$S/usr/lib/windinput/wind_setting"
install -Dm644 /src/scripts/linux/pkg/windinput-setting.desktop "$S/usr/share/applications/windinput-setting.desktop"
install -Dm644 /src/scripts/linux/pkg/windinput-import.desktop "$S/usr/share/applications/windinput-import.desktop"
install -Dm644 /src/scripts/linux/pkg/windinput-mime.xml "$S/usr/share/mime/packages/windinput.xml"
install -Dm644 /ws/wind-setting/res/wind_setting_icon.png "$S/usr/share/icons/hicolor/256x256/apps/windinput.png"
install -Dm644 /ws/wind-setting/res/wind_setting_icon_sm.png "$S/usr/share/icons/hicolor/64x64/apps/windinput.png"
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
Depends: libc6 (>= 2.35), fcitx5 (>= 5.0.14), libxcb1, libxcb-shape0, libfontconfig1, fonts-noto-cjk | fonts-wqy-microhei | fonts-wqy-zenhei
Recommends: fcitx5-frontend-gtk3, fcitx5-frontend-qt5 | fcitx5-frontend-qt6, im-config, xclip | wl-clipboard, python3, xdg-utils, xdg-desktop-portal | zenity, libxkbcommon0
Homepage: https://windinput.com
Description: 清风输入法 (WindInput) —— Fcitx5 输入法引擎
 清风输入法的 Linux 版（测试版）：Rust 服务负责输入逻辑与候选窗渲染，
 Fcitx5 addon 负责与应用对接，另带图形设置程序（应用菜单「清风输入法设置」，
 或在输入时按 Ctrl+Shift+]）。安装后运行 windinput-setup 配置当前用户。
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
