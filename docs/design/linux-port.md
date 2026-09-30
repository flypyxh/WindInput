# Linux 移植设计

> 状态：进行中（分支 `feat/linux-port`）。目标：在主流 Linux 桌面上「能正常输入」，再逐步补齐体验。

## 1. 架构：复用 macOS 的「薄壳 + 服务进程」

```
 宿主应用 ──(IME 框架)──► Fcitx5 addon (wind_linux/, C++17)
                              │  UDS 请求/响应 + push 推送
                              ▼
                        wind_input 服务 (Rust, --features linux-host)
                              │  tiny-skia 光栅化 → POSIX SHM 帧
                              ▼
                        addon 把帧画成候选窗（X11 先行，Wayland 后补）
```

与 `wind_tsf/`（Windows）、`wind_macos/`（macOS）同构：**引擎、词库、候选逻辑全在 Rust 服务里**，
addon 只做按键转发、上屏/预编辑写回、候选窗呈现。宿主选 **Fcitx5**（中文社区主流）；IBus 之后再议。

不走 Fcitx5 原生候选窗：自定义主题、tooltip、注释上方条等特色都靠自绘，原生窗全丢。

## 2. 精简范围（与 macOS 一致）

| 不做 | 理由 |
|---|---|
| 按应用独立配置（compat） | 主要为解决 Windows 宿主兼容性问题；Linux 上无同类需求，Wayland 下也拿不到前台进程名 |
| 工具栏 | 后续再考虑类似 macOS 的状态图标 |
| 软键盘、输入诊断 HUD | Wayland 无法自由定位窗口；HUD 依赖 Windows 专有诊断数据 |
| 全局热键 | Wayland 无标准方案 |
| 主菜单 / 候选右键菜单 | 第一阶段不做；之后由**服务光栅化菜单帧自绘**（不走框架菜单） |

## 3. 平台分层：`ext_presenter` 与 `mock_text`

此前协调器/桥接/UI 里 30 余处 `not(target_os = "macos")` 的真实语义是「Windows 桌面形态」，
Linux 会被误归进去（拼 `.exe` 设置路径、弹进程内菜单并吞键——没有窗口时菜单永不关闭，输入被永久卡死）。

`wind-ui` / `wind-bridge` / `wind-coordinator` 各有一份 `build.rs` 产出两个 cfg 别名
（改动须三处同步）：

| 别名 | 为真的条件 | 语义 |
|---|---|---|
| `ext_presenter` | macOS；或 Linux 且开 `linux-host` feature | 呈现层由**外部宿主进程**承担：服务只光栅化，窗口、菜单、设置入口、按键合成归宿主 |
| `mock_text` | 非 Windows、非 macOS，且未开 `linux-host` | 文本后端是 mock（等宽近似）；含文本数值断言的测试据此门控 |

**`linux-host` 默认关。** 关闭时 Linux 与原先完全一致——开发机 `cargo test` 仍走 mock 文本，
布局/golden 测试的数值不变。发行的 Linux 服务用 `cargo build -p wind_service --features linux-host`。

判据：真正的 macOS API（CoreText、Carbon 热键、AppKit 软键盘面板、`activate_ime`）仍留在
`target_os = "macos"`；其余「宿主承担」的语义一律用 `ext_presenter`。

## 4. 端点与目录

- socket：`$WIND_INPUT_RUNTIME_DIR` → `$XDG_RUNTIME_DIR/WindInput[Dev]/{bridge,bridge_push}.sock`
  → `/tmp/wind_input{_dev}`。socket 是运行时状态，不落盘。
- 配置：`~/.config/WindInput[Dev]`（`dirs::config_dir`）；缓存/日志：`~/.local/share/WindInput[Dev]`。
- data：服务可执行文件同目录的 `data/`（`variant::install_root`）；FHS 打包时再议。
- 已知：SHM 名 `/WindInput_SHM` 是全局的，多用户同机会撞；单用户桌面暂不处理。

## 5. 系统能力（`wind-ui/src/linux_host.rs`）

服务不链接任何桌面库，与 macOS 的 `pbcopy`/`open` 同一路数走外部命令：

| 能力 | Wayland | X11 |
|---|---|---|
| 剪贴板读写 | `wl-copy` / `wl-paste` | `xclip`，其次 `xsel` |
| 图片剪贴板 | `wl-copy --type image/png` | `xclip -t image/png`（`xsel` 不支持） |
| 打开路径/URL | `xdg-open` | `xdg-open` |

缺工具时如实报错，不返回假成功。⚠️ 由 systemd 用户单元拉起的服务可能没有 `WAYLAND_DISPLAY`/`DISPLAY`，
需要 `import-environment` 或由 addon 拉起服务以继承会话环境。

## 6. 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| M0 | `ext_presenter`/`mock_text` 别名、服务端 `linux-host` 形态、端点路径、系统能力 | 完成 |
| M1a | Linux 真实文字后端（`wind-ui/src/text/linux`，ttf-parser + ab_glyph_rasterizer + dlopen fontconfig） | 进行中 |
| M1b | Fcitx5 addon：输入通路（按键/上屏/预编辑/焦点/自愈），DBus 集成测试 | 进行中 |
| M1c | addon 的 X11 候选窗呈现、鼠标回传 | 待做 |
| M2 | Wayland：Fcitx5 UI addon + input popup surface；先做 spike 验证 popup 表面能否收鼠标事件 | 待做 |
| M3 | 自绘菜单、设置端适配（`wind-setting` 的 `windui` 已支持 X11/Wayland，缺托盘）、打包 | 待做 |

## 7. 待验证的风险

1. Wayland input popup 表面是否收得到鼠标事件（决定候选点选/悬停/自绘菜单是否成立），GNOME 与 KDE 可能不同。
2. 自绘菜单在 Wayland 下只能画进候选窗自己的表面；菜单外点击不可观测，
   `menu_open` 期间协调器会吞方向键/回车/Esc，**必须有 Esc、焦点丢失、候选清空、超时四条兜底关闭**，
   否则输入永久卡死（macOS 已踩过）。
3. `wind-coordinator` 在 macOS 目标上依赖 C 库，本机无法交叉 check，改动靠 CI 的 macOS job 兜底。
