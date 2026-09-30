# wind_linux — 清风输入法 Fcitx5 前端

清风输入法在 Linux 上以 [Fcitx5](https://fcitx-im.org) 输入法引擎的形式工作：本目录是 Fcitx5
addon（`libwindinput.so`），负责把按键交给清风输入法服务（`wind_input`，Rust）、把上屏与预编辑
写回应用。输入逻辑、词库、候选全部在服务里。

## 构建

依赖：Fcitx5 5.x 开发包（`libfcitx5core-dev` `libfcitx5utils-dev` `libfcitx5config-dev`）、
`libxcb1-dev` `libxcb-shape0-dev`（候选窗）、CMake ≥ 3.16、C++17 编译器。

```bash
cmake -S wind_linux -B wind_linux/build/cmake -G Ninja
cmake --build wind_linux/build/cmake
cmake --install wind_linux/build/cmake --prefix /usr     # 需要 root；或装到 ~/.local 并设 FCITX_ADDON_DIRS
```

服务：

```bash
cd wind_input && cargo build --release -p wind_service --features linux-host
```

没有 root 的开发机用 `scripts/linux/bootstrap-sdk.sh` 把 Fcitx5 SDK 解到用户目录。

## 安装包（Ubuntu/Debian）

```bash
scripts/linux/package-deb.sh          # 在 22.04 容器里构建，产物 dist/linux/windinput_<版本>_amd64.deb
sudo apt install ./windinput_*.deb     # 会带上 fcitx5 及 GTK/Qt 前端
windinput-setup                        # 每个使用者运行一次：配置 fcitx5、切输入法框架
# 然后注销并重新登录
```

包里带图形设置程序（`/usr/lib/windinput/wind_setting`）：应用菜单「清风输入法设置」，或输入时按
`Ctrl+Shift+]`。构建时需要兄弟仓库 `../wind-setting` 与 `../wind-ui-rust`（`SETTING_REPO` 可指定）。
设置程序的文件对话框依赖 xdg-desktop-portal 或 zenity（包已 Recommends）。

必须在 22.04 上构建：更新的发行版编出的产物带 GLIBC_2.38+ 符号和更新的 fcitx5 ABI，22.04 上加载不了。
服务由 addon 在连不上时自动拉起（`/usr/lib/windinput/wind_input`，可用 `WIND_INPUT_SERVICE` 覆盖），
服务自己持 flock 单例。

## 使用

1. 启动服务 `wind_input`（exe 同目录要有 `data/` 词库目录）。socket 在
   `$XDG_RUNTIME_DIR/WindInput/`。
2. 在 Fcitx5 配置里把「清风输入法」加入输入法组。
3. 设置：应用菜单或 Fcitx5 托盘菜单里的「清风输入法设置」、Fcitx5 输入法配置里清风的「配置」，
   或在输入时按 `Ctrl+Shift+]`（开发时设置程序路径可用
   `WIND_INPUT_SETTING` 覆盖，默认 `/usr/lib/windinput/wind_setting`）。
4. 想用 Shift 单击切换中英：到 Fcitx5 全局配置里清空「临时在当前和第一个输入法之间切换」
   （`AltTriggerKeys`，出厂是左 Shift），否则 Shift 会先被 Fcitx5 截走。

## 测试

```bash
make -C wind_linux test      # 单测（不需要 Fcitx5）
scripts/linux/e2e.sh         # 端到端：真服务 + 真 fcitx5 + DBus 模拟应用打字
```

开发约定与架构说明见 [AGENTS.md](AGENTS.md)。
