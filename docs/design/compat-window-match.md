# 应用兼容规则：窗口类名 / 标题匹配

> 状态：设计定稿（2026-10-08，维护者确认）；P1～P4 已实施（标题变化检测未做）。来源：论坛 t270（AutoHotkey 同进程多窗口）、
> GH#175（想用 `class = "Chrome_WidgetWin_*"` 覆盖所有 Chromium / Electron 宿主）。

## 定稿决策

| 项 | 决定 |
|---|---|
| 匹配字段 | `[[apps]]` 新增 `class`（顶层窗口类名）、`title`（顶层窗口标题），可与 `process` 组合 |
| 语法 | 通配符 `*` `?`，不区分大小写，自写匹配器，不引依赖 |
| 类名来源 | **只匹配顶层窗口类名**（FocusGained 已上报，`protocol.rs:937`）；子控件类名以后若需要另立 `focus_class` |
| 多条命中 | 按具体程度逐字段叠加，越具体越后叠（后叠的赢） |
| `process = "*"` | 纳入统一叠加后**对所有字段生效**（此前只对 host_drawn_candidates / composition_placeholder 生效） |
| 标题变化 | 第一期只在焦点变化时判定；标题变化不重算初始中英态。开始组合时检测标题变化留作第四期 |

## 叠加顺序（同一层内，从先到后）

- T0：仅 `*`
- T1：仅窗口条件（仅类名 < 仅标题 < 类名+标题）
- T2：仅进程名
- T3：进程名 + 窗口条件（内部同 T1）
- 同级：模式里非通配字符数少的先叠；再按层（出厂 < data_custom < 用户）与文件行序稳定排序。

必须在**原始键值**上 `compose` 再反序列化（结构体上叠分不开裸 bool 的「没写」与 `false`，见
`compat_overlay.rs:13`）。disabled 行不参与；行内 `unset` 跨级生效（= 本窗口回到全局默认）。

## 规则身份

跨层认「同一条」的键：`(process 或 "*", class 模式, title 模式)`，trim + 小写比较；不写 process 视为 `*`；
三者全空的行作废。旧文件无 class/title 时退化为现在的进程名键，行为不变。

需替换按进程名认同一条的地方：`compat_overlay.rs` coalesce / overlay / sanitize / apply_edits，
`app_compat.rs` 右键菜单写回（只写纯进程键），`compat_admin.rs` 各接口。`class` / `title` 进 META_KEYS，
不可 unset。`[[initial_mode_scope]]` / `[[commit_newline]]` 不加窗口条件（写了当未知键保留并告警）。

## 服务端判定

- 缓存键 pid → (pid, 顶层类名小写, 标题哈希)，在 focus_gained 判定；ime_activated 沿用该 client_token
  最近一次 focus_gained 的窗口上下文。
- `AppCompat::resolve` 缓存两级（P3）：一级按 (进程, 类名) 存「进程 + 类名都命中」的候选拆成的基底与
  待比标题的子集（LRU，容量 64）；二级按命中的标题规则集合存合成结果。标题频繁变化只在子集里比标题；
  没有标题规则时不折叠标题。表里没有标题规则时服务端也不留标题（`focus_window_of`）。
- 持续类字段（定位、首显、配对、占位…）随判定结果即时刷新；进入类（initial_mode / punct / schema）
  只在「pid 变了，或命中的带窗口条件的初始规则集合变了」时重算，仅标题变化永不重算。
- `host_render` 只认具体进程、无窗口条件的规则；按事件源 pid 直查（`push_config.rs:157`）在无窗口
  上下文时不匹配窗口规则；类名为空一律不命中。

## 标题上报（Windows）

- FocusGained 变长段末尾追加 `titleLen:u32 + title`（旧 DLL 解出空串），DLL 用 `InternalGetWindowText`
  （不发 WM_GETTEXT，宿主卡住也不阻塞），截断 256 字符。
- 仅当服务端推送「本进程可能命中标题规则」（`CONFIG_KEY_COMPAT_TITLE_MATCH`，按客户端进程算：不限进程
  的标题规则或本进程的标题规则；握手推、compat 重载 / pid 名纠正后逐客户端重推）时才采集；DLL 在推送
  通道重连（CMD_SERVICE_READY）时清零。纯判据在 `wind_tsf/include/WindowTitlePolicy.h`。
- 新进程首焦常早于开关到达：DLL 在开关 0→1、仍有焦点、本焦点会话发出的 focus_gained 没带标题时补发一次
  （带类名与标题）。服务端把「上一次推进模式归属的焦点看不见标题、这次同 pid 同类名带标题且命中写进入类
  字段的标题规则」也算切进来（`focus_crossed` / `ModeScope::title_blind`），其余仅标题变化照旧不重算。
- 窗口身份（token 窗口表比较、`ActiveCompat` 缓存键）比「类名 + 命中的标题规则集合」，不比标题原文。
- 日志：标题按用户数据处理，任何级别只记长度与命中规则键；`WindLogHostProcessInfo`（`compat.*.foreground_host`
  等）只留 `title_len`，`WindHostProcessInfo` 不再存标题原文。

## 分期

| 期 | 内容 | 测试点 |
|---|---|---|
| P1 | wind-config：身份键、匹配器、按级合成 | 匹配器；排序；跨层同键叠加；unset/disabled 跨级；旧文件不变；`*` 全字段 |
| P2 | 协调器按窗口上下文解析、改造各 get_rule 调用点与跨越判定 | AHK 两窗口；Chromium 类名通配；标题变不重算初始模式 |
| P3 | 标题段 C++ + 协议 + 推送开关 + 日志脱敏 | 编解码往返；新旧 DLL 互通；未推开关不采集 |
| P4 | admin RPC（含 rename）、设置端兼容窗口、文档；可选标题变化检测 | rename / 校验 / 导入导出往返；设置端 mock |

GH#175 出厂规则在本特性落地后由进程名改为 `class = "Chrome_WidgetWin_*"` + `MozillaWindowClass`。
