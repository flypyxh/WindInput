# macOS 提示类浮层与菜单改为服务端自绘

状态：P0–P3 已实现（分支 `feat/mac-overlay-raster`），Swift 侧未经编译，待 CI 与真机验证。目标：macOS 与 Linux 同构——状态气泡 / Toast /
悬停提示 / 候选右键菜单与主菜单，都在服务进程按主题光栅化，`.app` 只贴图、落位、计时、转发鼠标。

## 为什么

候选窗与软键盘早已是服务端自绘，唯独这四类在 `.app` 里用 AppKit 原生控件排字（`TooltipPanel` /
`StatusBubblePanel` / `ToastPanel` / `NSMenu`），服务端只发文本与 hex 颜色。结果是主题的背景图、
渐变、图层、软阴影、边框、字号偏移、悬停提示右键菜单等能力在 macOS 上拿不到，每加一个主题字段
就要在 Swift 侧再抄一遍（`[tooltip] font_size` 就是这样漏掉的）。

## 做法

复用 Linux 已有的整条链路，不新造协议：

- 渲染：`overlay_linux.rs` / `menu_linux.rs` 改名为 `overlay_host.rs` / `menu_host.rs`，条件从
  「linux + ext_presenter」放宽为 `ext_presenter`（macOS 恒真）。`Forwarder` 里 macOS 与 Linux 的
  并行分支合一。
- 协议：`CMD_OVERLAY_FRAME`（kind = tooltip / status / toast / menu 各级）+ 各层独立 SHM 段
  （`overlay_shm_name`，macOS 名长不超过 31 字节上限）。
- 落位：与 Linux 同构，由宿主（`.app`）按屏幕几何落位、计时；服务端只给首选点 / 备选点 / 锚点。
- 交互：对齐 `wind_linux/include/OverlayInput.h` 的语义（见 `linux-port.md` §5b）。
- 菜单：复用 `popup_menu` 整套，指针事件由 `.app` 转 `CMD_MENU_POINTER`。

## 保留原生的部分

菜单栏状态项（`NSStatusItem`）与系统输入源菜单（`InputController.menu`，由 IMK 在别的进程里绘制）
无法自绘，继续用 `UnifiedMenuBuilder` + 精简菜单树。

## 分期与现状

| 期 | 内容 | 现状 |
|---|---|---|
| P0 | 放宽 cfg、改名；Forwarder 合一；帧带 scale、落位量折成宿主单位 | 已做，Rust 侧本机验证（linux-host 测试 + macOS 目标交叉 clippy） |
| P1 | `.app` 的 `OverlayHost` / `OverlayPanel`，删三个原生 Panel 与旧 `CMD_*_SHOW` | 已写，未编译 |
| P2 | 鼠标交互：悬停保持与宽限、提示 / 气泡右键菜单、气泡拖动、Toast 点击关闭 | 已写，未编译 |
| P3 | 菜单自绘：每屏一个捕获窗、`CMD_MENU_POINTER`、关闭路径与自愈 | 已写，未编译 |

未做：CoreText 后端的主题字重（`font_styled` 忽略 weight，原生 Panel 也没有，不算回退）；Toast 的渲染缩放
取主屏（`dpi::scale_for_point(0, 0)`）。

## 已知风险

- **卡输入**：菜单开着而本端没有任何收口，服务端 `menu_open` 为真会吞方向键 / 回车。已有关闭路径：
  服务端点选 / Esc / 点菜单外 / 焦点丢失；本端空闲 60s、抓取上限 120s、第 0 级贴不出来、服务重启、
  push 断线。真机必须逐条试，并在每条之后立刻打字确认没被吞。
- **候选悬停哨兵**：上行 `CMD_CANDIDATE_HOVER` 的「无」在 macOS 是 `Int32.min`，`-1` / `-2` 是翻页器
  （服务端 `handle_candidate_hover` 非 Windows 分支）。Linux addon 把「无」发成 `-1`，会被服务端当成上页
  按钮——这是既有问题，本分支没动。
- **Spotlight 一类宿主**：此前候选右键用 `NSMenu.popUp` 会让它们连发 Deactivate/Activate，需要「焦点闪断」补偿
  （已随原生菜单删除）。捕获窗是 nonactivating 且不成为 key，理论上不再触发，需真机在 Spotlight 里点一次菜单确认。

## 验证条件

本机（Linux）没有 Swift 工具链：Rust 侧靠 `linux-host` 形态的单测 + `aarch64-apple-darwin` 交叉
check；Swift 侧只能靠 CI（`swift test`）与真机。Retina / 多屏 / 全屏 App 的 Space / Stage Manager
必须真机看一遍。
