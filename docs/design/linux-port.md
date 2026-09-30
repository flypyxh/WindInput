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

## 5b. 提示类浮层（状态气泡 / Toast / tooltip）

与 macOS 不同：`.app` 原生排字，Linux addon 不排字——服务端光栅化（与候选窗同构），像素走各层
独立的 SHM 段，`CMD_OVERLAY_FRAME` 只带落位规则与自动隐藏时长；落位（需要屏幕几何）与计时都在
addon。细节见 `wind_linux/AGENTS.md`「光栅浮层」。

## 5c. 自绘菜单（主菜单 / 候选右键菜单，M3b）

不用 Fcitx5 / 桌面的菜单：与候选窗、浮层同构，服务进程光栅化、addon 贴图，外观与 Windows 同一份。

- **复用 Windows 的 `popup_menu` 整套**（级联状态机、视图树、主题、定位、命中测试、增量重绘）。
  `wind-ui/src/menu_linux.rs` 只换两头：出——各级像素写各自的 SHM 段（`_MN0`…），推
  `CMD_OVERLAY_FRAME`（kind = `OVERLAY_KIND_MENU + 级`，落位 `EXACT`）；进——addon 在菜单打开期间
  **抓住指针**（X11 pointer grab，`owner_events=1`），把移动 / 按下原样报上来（`CMD_MENU_POINTER`
  0x021B，16 字节），命中测试在服务端做。键盘仍走 `forward_menu_key`（不抓键盘）。
- **定位在服务端**：翻转、子菜单左右展开、钳制全是 `popup_menu` 的原逻辑（有单测），工作区由
  addon 随「打开菜单」请求（扩展信封 `menu.open`）报上来；addon 按 `EXACT` 原样摆放、不夹回——
  两边各算各的，命中就会错位。工作区取整块根窗口（不分显示器，同候选窗）。
- **入口**：候选窗右键（命中候选 → 候选菜单，空白 / 翻页按钮 → 主菜单，同 Windows）；候选菜单
  末尾的「更多…」（仅 Linux：没有工具栏 / 托盘时组字中通往主菜单的路）；**空闲时**是 Fcitx5 状态区
  动作「清风输入法菜单」——托盘菜单与 kimpanel 面板（KDE / GNOME 扩展）都会列出它，点了主菜单弹在
  光标下方。选它而不是热键：热键要新增配置字段并同步设置端（两仓联动），而状态区是 Fcitx5 上
  输入法功能的惯常位置、零配置；状态气泡对鼠标透明、只短暂出现，改成可点会牵动浮层的整套设计。
  代价是依赖 Fcitx5 的 UI 模块（classicui 托盘 / kimpanel）在场，**菜单本身**仍是自绘的。
- **按平台摘掉的主菜单项**（`build_main_menu_items`）：工具栏 / 状态图标开关（addon 不接
  `CMD_MODE_STATUS`）、软键盘（`open_softkeyboard` 在 Linux 直接拒绝）、「截图所有窗口到文件」
  （依赖宿主回应 `shot.panel` 才出结果 Toast，addon 不接）、输入诊断 HUD（同 macOS）。
- **关闭路径**：Esc / 点选 / 右键 / 点菜单外（抓指针看得见）/ 其它键（关并吞掉）走服务端
  `popup_menu`；失焦与切换输入上下文（addon `deactivate` 无条件收并报 `menu.dismiss`，服务端
  FocusLost 另有一条带 250ms 守卫的）；候选被清空 / 组合结束（候选右键菜单随候选收起，
  `notify_ui_hide`；组合被终止等直接复位的路径补发 `HideMenu`）；服务被杀（push 断线，addon 自收）、
  服务重启（`SERVICE_READY`，addon 自收并补报）；addon 断线（UDS 连接断开，服务端复位）；空闲超时
  （addon 60 秒无操作自收并报；服务端另有同时长的兜底：超时后的第一个键照常处理、不被吞）。
  两端认识错开时自愈：菜单不在屏上却收到菜单键 / 指针事件，UI 回送 `MenuClose`；菜单已关却收到
  指针事件，服务端让 UI 收菜单。e2e 逐条覆盖，每条之后立即打字验证。

## 6. 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| M0 | `ext_presenter`/`mock_text` 别名、服务端 `linux-host` 形态、端点路径、系统能力 | 完成 |
| M1a | Linux 真实文字后端（`wind-ui/src/text/linux`，ttf-parser + ab_glyph_rasterizer + dlopen fontconfig）；彩色 emoji（CBDT / COLR v0·v1 / OpenType-SVG，emoji 序列经 rustybuzz 整簇整形），限制见该模块头 | 进行中 |
| M1b | Fcitx5 addon：输入通路（按键/上屏/预编辑/焦点/自愈），DBus 集成测试 | 进行中 |
| M1c | addon 的 X11 候选窗呈现、鼠标回传；状态气泡 / Toast / tooltip 光栅浮层 | 进行中 |
| M2 | Wayland：Fcitx5 UI addon + input popup surface；先做 spike 验证 popup 表面能否收鼠标事件 | 待做 |
| M3a | 设置端适配：`wind-setting` Linux 原生构建、平台门控、addon 处理 `settings.open`、`.desktop` 入口、随 `.deb` 分发 | 完成（见 §6b） |
| M3b | 自绘菜单（主菜单 / 候选右键菜单），见 §5c | 完成（X11） |

## 6b. 设置端（M3a 结论）

- **能用**：`wind_setting` 在 Linux 原生编译、单测全绿；windui 默认走 X11（Wayland 会话经
  XWayland，`WINDUI_BACKEND=wayland` 才试原生 Wayland），文字经 fontconfig，中文渲染正常。
  控制 socket、用户目录、日志目录与服务同一套规则（`$XDG_RUNTIME_DIR/wind_input[_dev]_ctrl.sock`、
  `~/.config/WindInput[Dev]`），改设置写入用户 `config.toml` 并被服务热加载（实测）。
- **入口**：应用菜单（`windinput-setting.desktop`，兼 `windinput://` 协议）与「打开设置」热键
  （出厂 `Ctrl+Shift+]`）。后者：服务 `open_settings_with` 推扩展信封 `settings.open` → addon
  （`SettingsLauncher` + `fcitx::startProcess`）启动 `/usr/lib/windinput/wind_setting`
  （`WIND_INPUT_SETTING` 可覆盖）；只启动自己的设置程序，信封内容只当参数。e2e 覆盖整条链。
- **门控**：清单 `platform` 字段加 `linux`；在 macOS 精简范围之上，Linux 另藏工具栏显示/切换、
  软键盘热键、`activate_ime`、Dota2 兼容与热键的「全局」勾选。逐项依据见 wind-setting README
  「按平台屏蔽的设置项」。应用兼容性（按应用配置）**保留**：服务按 addon 报的程序名匹配规则，
  与 macOS 同一机制——§2「不做」指的是不为 Linux 维护内置规则，不是机制不可用。
- **托盘 / 常驻（`run_resident`）缺失不影响设置端**：`wind_setting` 不用常驻模式也不用托盘；
  缺的是输入法自己的状态指示（见 §2「工具栏」）。
- **已知问题**（windui Linux 后端）：设置程序已开着时，深链转来的切页要等下一次输入事件才显示；
  文件对话框依赖 xdg-desktop-portal 或 zenity，都没有时点了无反应。

## 7. 待验证的风险

1. Wayland input popup 表面是否收得到鼠标事件（决定候选点选/悬停/自绘菜单是否成立），GNOME 与 KDE 可能不同。
2. 自绘菜单在 Wayland 下只能画进候选窗自己的表面；菜单外点击不可观测（X11 靠指针抓取）。
   `menu_open` 期间协调器会吞方向键/回车/Esc——X11 已按 §5c 补齐全部关闭路径并逐条 e2e；
   Wayland 阶段要重新验证每一条（尤其没有抓取时的「点菜单外」只能退回失焦 / 超时）。
   另：抓指针期间点菜单外，那一下被菜单吃掉、不传给下面的应用（同 X11 原生菜单，与 Windows
   「点外面照常响应」不同）。
3. `wind-coordinator` 在 macOS 目标上依赖 C 库，本机无法交叉 check，改动靠 CI 的 macOS job 兜底。
