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

## 使用

1. 启动服务 `wind_input`（exe 同目录要有 `data/` 词库目录）。socket 在
   `$XDG_RUNTIME_DIR/WindInput/`。
2. 在 Fcitx5 配置里把「清风输入法」加入输入法组。
3. 想用 Shift 单击切换中英：到 Fcitx5 全局配置里清空「临时在当前和第一个输入法之间切换」
   （`AltTriggerKeys`，出厂是左 Shift），否则 Shift 会先被 Fcitx5 截走。

## 测试

```bash
make -C wind_linux test      # 单测（不需要 Fcitx5）
scripts/linux/e2e.sh         # 端到端：真服务 + 真 fcitx5 + DBus 模拟应用打字
```

开发约定与架构说明见 [AGENTS.md](AGENTS.md)。
