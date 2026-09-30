# wind_linux

Linux 上的 **Fcitx5 addon**（C++17），与 Windows 的 `wind_tsf/`（TSF DLL）、macOS 的
`wind_macos/`（IMKit `.app`）对位。它是薄壳：按键转发给 Rust 服务、把服务的上屏/预编辑写回
Fcitx5 的 `InputContext`。引擎/词库/候选逻辑全在服务里，这里**不做任何中英判定**。

服务以 `cargo build -p wind_service --features linux-host` 构建（在 `wind_input/` 下），
`linux-host` 让协调器走「外部宿主 forwarder」（`wind-ui/src/manager_macos.rs`，macOS 与 Linux
共用，文件名沿用）。

## 结构

| 路径 | 角色 |
|---|---|
| `include/Protocol.h` | **直接 include** `wind_tsf/include/BinaryProtocol.h`（不另抄一份）；用宏垫掉它唯一的 Win32 依赖 `GetCurrentModifiers()`。那个函数在 Linux 上**禁止调用** |
| `include/ExtProtocol.h` | 外部宿主专用 cmd（候选帧 / tooltip / 按键合成…），Windows 不用故不在 BinaryProtocol.h 里。每个值由 `tests/protocol_sync_test.cpp` 逐个对照 `protocol.rs` 源码 |
| `include/Codec.h` + `src/core/Codec.cpp` | 帧编解码。每个函数注明对位的 Swift `BinaryCodec` 函数 |
| `include/Bridge.h` + `src/core/Bridge.cpp` | UDS 请求/响应客户端（2s 超时、`MSG_NOSIGNAL`）、push 监听线程（断线每秒重连）、端点路径 |
| `include/KeyMap.h` + `src/core/KeyMap.cpp` | X11 keysym / 修饰状态 → Windows VK / 协议修饰位；修饰键单击检测 |
| `include/ResponseRouter.h` + `src/core/ResponseRouter.cpp` | 响应帧 → 宿主操作。Swift `BridgeResponseRouter` 的逐条移植（待定标点 / 定格前缀 / hold 计时器 / 数字后智能标点记账） |
| `include/Utf.h` + `src/core/Utf.cpp` | UTF-16 码元 ↔ UTF-8 字节换算（服务端光标以 UTF-16 计，Fcitx5 要字节偏移） |
| `include/SettingsLauncher.h` + `src/core/SettingsLauncher.cpp` | 下行扩展信封 `settings.open` 的 body 解析（JSON argv）与设置程序路径（`WIND_INPUT_SETTING` / `/usr/lib/windinput/wind_setting`）。只启动自己的设置程序，信封内容只进参数位 |
| `include/Menu.h` + `src/core/Menu.cpp` | 自绘菜单的纯逻辑：kind ↔ 级、候选窗右键的目标、空闲超时（`WIND_MENU_IDLE_TIMEOUT_MS`） |
| `include/ShmFrame.h` + `src/core/ShmFrame.cpp` | 候选帧 SHM 读端（对位 `SharedMemoryReader.swift`）、落位几何 `placePanel`（对位 `CandidatePanel.show` 的翻转/钳制）与浮层落位 `placeOverlay`、命中测试 |
| `src/fcitx/WindEngine.{h,cpp}` | Fcitx5 引擎（`InputMethodEngineV2`）；与 `X11Panel` 是仅有的两个依赖 Fcitx5 头文件的地方 |
| `src/fcitx/X11Panel.{h,cpp}` | X11 候选窗 + 三层光栅浮层 + 自绘菜单：自建 xcb 连接、override-redirect 窗口贴帧、候选窗的鼠标点击/悬停/滚轮/右键回传、浮层自动隐藏计时、菜单打开期间抓指针并回报 |
| `data/*.conf.in` | addon / 输入法描述文件模板（构建时生成到 `build/…/share/fcitx5/`） |
| `tests/*_test.cpp` | 纯 C++17 单测（不需要 Fcitx5），与 `wind_tsf/tests` 同风格 |

`src/core` 与 `include` 不依赖 Fcitx5，能单独编、单独测——这条边界别破。

## 构建 / 测试

本机无 root 时先搭用户前缀 SDK（幂等，只写 `~/.local/opt/fcitx5-sdk`，不碰 `/usr`）：

```bash
scripts/linux/bootstrap-sdk.sh                  # apt-get download + dpkg -x fcitx5 全家、Xvfb、cmake(pip)、dbus-next(pip)
eval "$(scripts/linux/bootstrap-sdk.sh env)"    # PATH / PKG_CONFIG_PATH / CMAKE_PREFIX_PATH / LD_LIBRARY_PATH
```

```bash
make -C wind_linux test                         # 单测：不需要 cmake / Fcitx5，g++ 直接编
cmake -S wind_linux -B wind_linux/build/cmake -G Ninja && cmake --build wind_linux/build/cmake
ctest --test-dir wind_linux/build/cmake         # 同一批单测经 CMake 跑
scripts/linux/e2e.sh                            # 端到端：真服务 + 真 fcitx5 + DBus 客户端打字
```

`e2e.sh` 的做法（改它之前先读脚本头注释）：`dbus-run-session` 起私有会话总线；服务二进制拷进
临时目录、`data` 软链到 `build_dev/data`，配置/数据/缓存全用 `XDG_*_HOME` 隔离，socket 走
`WIND_INPUT_RUNTIME_DIR`；fcitx5 以 `--disable=all --enable=keyboard,dbus,dbusfrontend,windinput`
起，`FCITX_ADDON_DIRS` / `FCITX_DATA_DIRS` 指到构建目录；`e2e_client.py`（dbus-next）经
`org.fcitx.Fcitx.InputContext1.ProcessKeyEvent` 送键，收 `CommitString` / `UpdateFormattedPreedit`
信号断言。临时目录必须短（socket 路径 108 字节上限），默认 `/tmp/wi-linux/e2e.<pid>`。

候选窗与浮层（X11）用例在私有 Xvfb 里跑（`WIND_E2E_X11=0` 关掉）：打字后按 `WM_CLASS` 实例名
`wind-candidate` 找窗（`xdotool search --classname`，**不是** `--class`——后者匹配第二段
`WindInput`），断言已映射、落在光标下方、截图里有内容、上屏/Esc/鼠标选词后隐藏、屏幕底边时
翻到光标上方；鼠标选词因命中矩形只有服务端知道，沿窗口中线逐点点击直到有上屏。浮层用例：
Shift 切中英 / Ctrl+Shift+E 切方案后等 `wind-status` 出现再等它自己消失；Toast 用
`wind_input ui toast`（控制 RPC 在 `$XDG_RUNTIME_DIR` 下，e2e 已把它指进临时目录）；tooltip 沿
候选窗中线 `xdotool mousemove` 直到出现。截图存在 `$W/shots/`（`KEEP=1` 保留）。`WIND_E2E_COMPOSITOR=1` 另起 xcompmgr 走 ARGB 路径。

设置程序用例：e2e 经包装脚本（`$W/svc/wind_setting`，记下 argv 再 exec 真程序，真程序默认取
`~/.cache/wi-tgt-setting-linux/debug/wind_setting`，`WIND_E2E_SETTING` 可覆盖，缺它 e2e 直接失败）
设 `WIND_INPUT_SETTING`，按 `Ctrl+Shift+]` 验「热键 → 信封 → 拉起 → 连上服务 → 再按一次转交首实例
→ 退出」。设置窗口按 WM_CLASS 实例名 `wind_setting` 找；映射早于首帧，截图要轮询到有内容。

e2e 的坑，都踩过：

- **服务 socket 起得比引擎早**。词库加载（首次还要建 `.wdat` 缓存，十几秒）完成前按键一律
  PassThrough。客户端先用真实通路探活（按 `a` 直到被吃再 `Esc`），不要用 sleep 猜。
- **`dbus` 模块依赖 `keyboard`**：`--enable` 里少了 keyboard，dbus 模块根本不加载，
  `org.fcitx.Fcitx5` 这个名字永远没人持有。
- **SDK 里的 Xvfb 在写死的 `/usr/bin` 找 xkbcomp**，没 root 装不进去，键盘初始化失败即退出。
  bootstrap 复制一份 `Xvfb-wind`、把那个字符串原地改成同长的 `/tmp/wlx` 并软链到 SDK 的
  `usr/bin`；xkb 数据走 `-xkbdir /usr/share/X11/xkb`。
- **xcompmgr + Xvfb 下 `xwd -root` 截到的合成结果不可信**（候选窗整体错位、文字出彩边），
  而 `xwd -id <候选窗>` 取到的窗口自身像素是对的。ARGB 路径因此只验到「窗口像素正确」，
  屏幕上的合成效果要在真桌面上看。
- **Fcitx5 出厂 `AltTriggerKeys=Shift_L`** 会在引擎之前截走 Shift 单击，「Shift 切中英」永远
  到不了服务。e2e 的 fcitx 配置把它清空；**真实用户也得清**（或在安装脚本里改），否则
  出厂的 Shift 切中英在 Linux 上不生效——见下方差距表。

## 协议用法（与 macOS 的异同）

- 按键：只发 **keydown**（`CMD_KEY_EVENT`），**同步**等响应再决定 `filterAndAccept`。Fcitx5 与
  IMKit 一样没有 TSF 那种「先决定吃不吃、再问服务」的前置闸门，所以「C++ 吃键集 ⊆ Rust
  出字集」天然成立——吃不吃完全由响应决定。按下被吃的键，其松开也吃掉（按硬件键码记）。
- 修饰键本身的按下不上报；**干净单击**（按下→无别的键→500ms 内松开，同 Windows
  `TOGGLE_TAP_THRESHOLD_MS`）时发一帧 `eventType=UP` 的 KeyEvent（`VK_LSHIFT` 等）。协调器
  只在 keyup 分支处理切换键。
- 焦点：Fcitx5 的 `activate` 同时承载「切到本输入法」与「焦点进入文本框」，按事件类型分开——
  `InputContextSwitchInputMethod` 才发 `CMD_IME_ACTIVATED`（服务端据此套用激活初始状态），
  普通焦点进入只发 `CMD_FOCUS_GAINED`。每次聚焦都报 IME_ACTIVATED 会让「不记忆中英状态」的
  用户每切一次窗口就被重置。`deactivate` 对称地发 `FOCUS_LOST`（+ 切走时 `IME_DEACTIVATED`）。
- `CMD_IME_ACTIVATED/DEACTIVATED` 服务端**从不回响应**，必须带 AsyncFlag 发、不读——否则
  主线程白等 2s 超时。
- `FOCUS_GAINED` 载荷发满 39 字节 + `bundleId`（= `InputContext::program()`）+ 空 windowClass。
  clientToken 高 32 位 = 程序名的 FNV-1a 散列（同 macOS：Wayland/DBus 客户端拿不到可靠 pid），
  低 32 位 = IC uuid 的散列。
- 焦点重叠：没有焦点组的前端允许 B 先 FocusIn、A 才 FocusOut。A 迟到的 `FOCUS_LOST` 会被服务端
  当「陈旧失焦」丢弃（active token 已是 B），服务端缓冲里的旧编码就接着拼进 B（实测上屏
  「你好你好」）。`activate` 发现上一个 IC 还挂着时先替它收尾。e2e 7b 钉这条。
- 预编辑带 `TextFormatFlag::DontCommit`：客户端没声明 `ClientUnfocusCommit` 时，Fcitx5 会在失焦
  时把 client preedit 当正文上屏（实测收到 `"ni'hao"`）。组合串是编码不是正文。
- 密码框：`CapabilityFlag::PasswordOrSensitive` → `INPUT_SCOPE_PASSWORD_BIT`。同一 IC 内能力位
  翻转不触发 activate，按键前比对、翻转即补报 FocusGained（同 macOS 的安全输入跟随）。
- 光标：`InputContext::cursorRect()`（X11 客户端是根窗口坐标，正是 wire 坐标系）→
  `CMD_CARET_UPDATE` 12 字节版，y 取行顶、height 取行高。宿主没报过位置（全 0）时不发。
- prevChar（数字后智能标点）：宿主支持 surrounding text 就读真实光标前字符（对位 Windows 主路径），
  否则退回本端记账（对位 macOS 唯一的备用通路）。
- 断线自愈：请求连接 2s 超时；I/O 失败即重连并**重试当前帧一次**。push 通道断开后每秒重连，
  收到第二次起的 `SERVICE_READY` 即视为服务重启：换新请求连接并对当前焦点重报 FocusGained。
- `reset`（宿主要求重置，如鼠标点击挪了光标）→ 收组合 + `CMD_COMPOSITION_TERMINATED`。

## 候选窗（X11）

与 macOS 同构：服务进程光栅化（BGRA 预乘 alpha）→ 写 POSIX SHM `/WindInput_SHM[Dev]` →
push `CMD_HOST_RENDER_FRAME`（坐标 + flags）与 `CMD_CANDIDATE_RECTS`（命中矩形，晚于帧）。
addon 在主线程（经 EventDispatcher）按名只读打开 SHM、拷出一帧，交 `X11CandidatePanel` 呈现。

- **自建 xcb 连接**，不借 Fcitx5 的 xcb 模块：候选窗与 Fcitx5 自己的 UI 无关，xcb 模块没加载时
  照常工作。连接的 fd 挂在 Fcitx5 事件循环上，鼠标事件与按键同在主线程、不会交错，所以
  选词/悬停/滚轮直接复用按键那条请求连接。连接断了只停 IO 事件，下一帧重连。
- **override-redirect 顶层窗口**：不归窗口管理器管、不抢焦点（点候选不会让宿主失焦）。
  `WM_CLASS = wind-candidate / WindInput`，`_NET_WM_WINDOW_TYPE_POPUP_MENU`。
- **透明两条路**：有合成器（`_NET_WM_CM_S<n>` 有人持有）→ 32 位 ARGB visual，圆角与软件阴影
  真透明；没有 → 根 visual + XShape 按 alpha≥128 抠形（圆角保住，半透明阴影退化）。每帧都查
  一次合成器，合成器中途起停会自动换路（重建窗口）。
- **位图挂成窗口背景 pixmap**：被遮挡后露出由 X 服务器自己重绘，不处理 Expose。PutImage 按
  最大请求长度分块（无 BIG-REQUESTS 时 256KB）。
- **落位**：服务端给左上角建议点；addon 按 macOS 同规则水平钳制、下方放不下翻到光标上方
  （光标高估 18px）、`FLAG_ABSOLUTE_POS` 只钳制不翻转。工作区取根窗口尺寸。
- **SHM 与重启**：收到 `SERVICE_READY` 关掉旧映射（服务重启会 unlink + 重建段，旧映射成孤儿、
  候选窗卡在旧帧），下一帧按名重开。
- 悬停只报候选下标（≥0）：命中表里 -1/-2 是翻页按钮，而悬停协议里 -1 表示「无」。点击翻页
  按钮直接发负下标的 `CMD_CANDIDATE_SELECT`（服务端按 -1 上页 / -2 下页处理）。滚轮 ±120。

## 光栅浮层：状态气泡 / Toast / 悬停提示（X11）

**与 macOS 分道**：`.app` 用原生 NSPanel 排字，服务只发文本 + 配色（`CMD_STATUS_SHOW` 等）；
本 addon 不排字，这三者与候选窗同构——服务进程按主题光栅化（`wind-ui/src/overlay_linux.rs`，
真实字形走 `text/linux`），像素写进**各层自己的** SHM 段（`/WindInput_SHM[Dev]` + `_TIP` /
`_STS` / `_TST`，`overlayShmName`），再推 `CMD_OVERLAY_FRAME`（0x0513，68 字节，布局见
`Codec.h` 的 `OverlayFramePayload`）。macOS 行为不变：Linux 专属代码全在
`cfg(all(target_os = "linux", ext_presenter))` 下，macOS 仍发文本帧。

- **一层一窗**：`X11CandidatePanel` 里候选窗与三层浮层各一个 `Surface`，共用一条 xcb 连接；
  实例名 `wind-tooltip` / `wind-status` / `wind-toast`（e2e 按它找窗）。浮层**对鼠标透明**
  （XShape 输入区置空），窗口类型 `_NET_WM_WINDOW_TYPE_TOOLTIP`；透明两条路同候选窗。
- **落位归 addon**：服务拿不到屏幕几何，帧里只给规则（坐标都指**内容盒**，窗口 = 内容 − 阴影扩边）：
  `ABSOLUTE`（状态气泡固定位置）、`FLIP`（首选点 + 右溢/下溢时的备选点：状态气泡跟随光标）、
  `FOLLOW_CANDIDATE`（tooltip：坐标按候选窗**建议**落点算，addon 先平移「候选窗实际落点 −
  建议落点」再同 FLIP——候选窗被翻到光标上方时 tooltip 跟着走）、`ANCHOR`（Toast 七个位置、
  状态气泡的屏幕锚点，离边 `margin`）。纯逻辑在 `placeOverlay`，公式逐条对位 Windows 各窗口的本地定位。
  状态气泡的**窗口锚点**降级为同位置的屏幕锚点（服务端编码前就降级；Linux 拿不到前台窗口边框）。
- **计时归 addon**（同 macOS `.app`）：帧带 `durationMs`，>0 时本端计时器到点摘窗；0 = 常驻到
  下一帧 / 隐藏帧（常驻型状态气泡、tooltip）。服务端 forwarder 线程阻塞在命令通道上，没有到期唤醒。
- **隐藏**：服务端对某层推 `FLAG_VISIBLE` 缺席的帧；候选窗隐藏时 tooltip 两端都会藏（服务推隐藏帧，
  `hide()` 也兜底藏）。`SERVICE_READY` 时关掉三层 SHM 映射并藏掉全部浮层。

## 自绘菜单（X11）

设计与关闭路径全表见 `docs/design/linux-port.md` §5c。本端要点：

- **每级一窗**：`wind-menu-0`…`wind-menu-5`（WM_CLASS 实例名，e2e 按它找窗），`OVERLAY_PLACE_EXACT`
  原样摆放、不夹回。只有**新映射**的一级才提到最上；已显示的级只换像素 / 位置、不重排 z 序——
  否则父菜单一次高亮重绘就会盖住翻到左侧的子菜单（Windows `plan_render` 的同一教训）。
- **抓指针**：第 0 级出现时 `xcb_grab_pointer(owner_events=1)`，全部收起时放开。抓取期间本连接
  上的全部指针事件（含候选窗上的）只进菜单：移动按批合并、按下立即报（先把积着的移动报掉保序），
  松开 / 滚轮 / 离开不报。别的客户端还抓着（刚点完托盘菜单）时每 50ms 重试、最多 1 秒；抓不住菜单
  照用，只是点菜单外看不见。**不抓键盘**：键要照常经宿主到服务端的 `forward_menu_key`。
- **本端收菜单并报 `menu.dismiss`**：`deactivate`（失焦 / 换 IC / 切走输入法）、空闲超时、`SERVICE_READY`
  （补报）。push 断线（服务没了）只收不报。服务端推来的隐藏帧不报（是它关的）。
- **入口**：候选窗右键 → `menu.open`（带工作区）；主菜单经候选菜单末行「更多…」。空闲时没有
  主菜单入口：状态区动作 `windinput-settings`（「清风输入法设置」，`activate` 时挂进
  `StatusGroup::InputMethod`）直接启动设置程序，取舍见设计文档 §5c。
- e2e 的菜单用例都从组字中的「更多…」开主菜单，关掉后先 Esc 收组字再验打字；菜单用例把空闲超时
  调到 5 秒以覆盖超时那条路。客户端扮演 kimpanel（持有 `org.kde.impanel`、发 `TriggerProperty`
  点状态区动作，听 `org.kde.kimpanel.inputmethod` 的 `UpdateProperty` 取当前输入法图标），故
  fcitx5 要 `--enable` 上 `kimpanel`。

## 与 Windows / macOS 的差距

| 能力 | 现状 | 备注 |
|---|---|---|
| 候选窗显示（X11） | 已接 | 见上「候选窗（X11）」 |
| 候选窗显示（Wayland 原生） | 未做 | 无 DISPLAY 时候选窗不显示（日志 warn 一次）。XWayland 下走 X11 路径，但 `cursorRect()` 是窗口相对坐标，位置会错 |
| 多显示器 | 未做 | 工作区取整个根窗口，不按 RandR 显示器切分：跨屏边缘的翻转/钳制按整块虚拟屏算 |
| HiDPI（帧 `scale>1`） | 按物理像素原样贴 | X11 没有逻辑坐标，服务端在 Linux 上目前恒发 scale=1 |
| 候选窗拖动 / 固定位置回报（`pos.candidate` / `pos.candidate.query`） | 未接 | 服务端问位置时不答 = 保留旧值（macOS 不可见时也不答，语义安全） |
| 候选右键菜单 / 功能主菜单 | 已接（X11） | 见上「自绘菜单」。缺：Wayland；多显示器（工作区取整块根窗口）；点菜单外那一下被菜单吃掉（同 X11 原生菜单，Windows 会透传）；菜单开着时在候选上再右键只关菜单、不接着弹新菜单（Windows 会） |
| tooltip / 状态气泡 / toast | 已接（X11） | 见上「光栅浮层」。缺：气泡/提示的鼠标交互（Windows 可拖动状态气泡、右键菜单、悬停 tooltip 时保持显示）——浮层对鼠标透明；截图类命令（`TakeScreenshot` 的 `shot.panel`）仍只截候选窗 |
| 多显示器下的浮层锚点 | 未做 | 工作区取整个根窗口（同候选窗）：Toast / 锚点气泡落在整块虚拟屏的角上，而不是光标所在显示器 |
| 命令直通车按键合成（`CMD_KEY_TAP/SEQ/HOLD/RELEASE`） | 未接 | 可用 `InputContext::forwardKey` 实现，但只能打进当前 IC，不是系统级合成 |
| 工具栏 / 软键盘 / 输入诊断 HUD | 不做 | 产品决策：与 macOS 精简范围一致。设置端已按平台门控相应设置项（wind-setting README「按平台屏蔽的设置项」）；主菜单里的对应项也按平台摘掉 |
| 截图所有窗口到文件（主菜单「高级」） | 摘掉 | 流程按 macOS「浮层像素在宿主」写：要宿主回应 `shot.panel` 才出结果 Toast，addon 不接。「截图候选窗口到剪贴板」可用 |
| 按应用独立配置（compat） | 机制可用；**打包时不带 Windows 内置规则**（`scripts/lib/gen-compat.sh linux` 生成「字段说明 + 零规则」的 Linux 版系统层，各平台兼容策略不共用） | 服务按 `FOCUS_GAINED` 的 bundleId（= `InputContext::program()`）匹配规则，同 macOS；设置端的应用兼容性窗口保留 |
| 打开设置（`settings.open` 扩展信封） | 已接 | `WindEngine::onExt` → `fcitx::startProcess`（双 fork，不留僵尸，继承 fcitx5 的会话环境）。设置程序已开着时由它自己的单实例转发参数；但**转来的切页要等设置窗口下一次输入事件才显示**（windui Linux 后端，冷启动深链正常） |
| 全局热键 | 不做 | 设置端藏掉热键对话框的「全局」勾选；热键只在输入法激活、有焦点时经按键通路生效 |
| Shift 单击切中英 | **需清 Fcitx5 的 AltTriggerKeys** | 出厂 `Shift_L` 被 Fcitx5 截走。安装脚本应改 `~/.config/fcitx5/config`，或在 AGENTS 外的用户文档里写明 |
| 「Shift+鼠标拖选不算单击」 | 缺 | Windows 靠 ToggleTapPolicy 的四个鼠标信号；Fcitx5 引擎收不到鼠标事件 |
| CapsLock 状态通知（`VK_CAPITAL` keyup） | 不发 | 服务端靠每键 `toggles` 校准 CapsLock 镜像，功能不缺；只是按 CapsLock 本身不会即时刷新状态 |
| 英文输入统计（`CMD_INPUT_STATS`） | 不发 | 同 macOS |
| 小键盘 / 符号键映射 | keysym 走 US 布局反查 | 非 US 布局的 Shift 符号键（如德语 `§`）没有 VK，透传给宿主 |
| 彩色 emoji | 已接（服务端文字后端） | CBDT（Noto Color Emoji）、COLR v0/v1、OpenType-SVG 实测可画；肤色 / ZWJ / 国旗 / 键帽按字体 GSUB 合成一个字形。依赖系统装彩色 emoji 字体（`.deb` Recommends `fonts-noto-color-emoji`），没有时画单色字形或方框。sbix 未实测、COLR v1 扫掠渐变降级为纯色，见 `wind-ui/src/text/linux/mod.rs` 模块头 |
| 非 X11 坐标（Wayland） | 未处理 | `cursorRect()` 在 Wayland 下是窗口相对坐标，候选窗定位要等 Wayland 阶段 |

## 协议同步铁律

改 cmd id 或帧布局：`protocol.rs` + `codec.rs`（SSOT）→ `wind_tsf/include/BinaryProtocol.h` →
`wind_macos/…/{ProtocolTypes,BinaryCodec}.swift` → **本目录的 `ExtProtocol.h` 与 `Codec.cpp`**。
`protocol_sync_test` 会对 cmd id 当场报红，但**帧布局**它管不到——改布局要同时改
`tests/codec_test.cpp` 里按 Rust 解码器写死的字节偏移。

新增下行 cmd 必须在 `ResponseRouter::apply` 显式接一臂（default = 消费按键但不出字）；判断接不接
只看**谁产出**，不看名字像不像 Windows 专有（见 wind_macos/AGENTS.md 同名条目）。
