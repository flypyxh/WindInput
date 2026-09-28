# 应用独立配置增强：按应用方案 / 密码框强制英文 / 状态气泡定位

> 状态：设计已定（2026-09-28），实施中。分支 `feat/per-app-overrides`。
> 来源：看板 C0-7 / C3-3（GH#80）、A2-37 ②（t197）、C2-33（GH#148）。

应用独立配置即 `compat.toml` 的 `[[apps]]`（`wind-config/src/app_compat.rs` 的 `AppCompatRule`）。
本轮加三组能力，全部是**用户偏好**字段——一律 `Option`，`None` = 跟随全局；**不进**
`ProtocolFields`（那是宿主协议级修正的继承通道，偏好字段走整条覆盖）。

入口只有两个：右键菜单「应用独立配置」子菜单，与手写 `compat.toml`。设置页不编辑 compat 规则
（它没有这套能力，本轮不建）；只有 ④ 新增的两个**全局**键进设置页。

---

## ① ② 按应用方案

### 配置

```toml
[[apps]]
process = "code.exe"
schema = "english"      # 固定为该方案

[[apps]]
process = "weixin.exe"
schema = "@remember"    # 记住本应用上次用的方案
```

- 不写 = 跟随全局（`schema.active`）。
- `@` 前缀是保留标记空间：方案 id 来自文件名，不以 `@` 开头。以 `@` 开头的值只认 `@remember`，
  其余一律视为未配置并 warn。
- 固定的 id 不在 `schema.available` 中 ⇒ 视为未配置并 warn（启动预热只覆盖 available，
  允许任意 id 等于允许在焦点路径上同步冷构建）。
- 容错：值非法**不得**让整份 compat.toml 失效（同 `de_initial_mode` 的理由，见
  `value_domain_guard.rs`）。解析层只收字符串；「是否在 available 内」在协调器侧按当时的
  available 判，available 热重载后自然生效。

### 两套「当前方案」

| | 谁决定 | 手切时 |
|---|---|---|
| 无规则应用 | 全局 `schema.active` | 走现有入口：写盘、改全局（**行为不变**） |
| 固定应用 | 规则值 | 只改本应用的覆盖方案（临时）；离开再回来恢复规则值 |
| `@remember` 应用 | 记忆表，无记录用全局 | 只改本应用的覆盖方案，并记入记忆表 |

规则应用内的手切**不写 `schema.active`、不写盘 config**。

### 时机

在 `handle_focus_gained` 重型段、`update_active_compat` 之后，**跨进程切入**（与 initial_mode
同一判据：同进程内焦点跳转不重算，尊重用户在应用内的手切）时计算目标方案：

1. 固定 → 规则值；
2. `@remember` → 记忆表[进程名]（且仍在 available）否则全局；
3. 无规则 → 全局。

目标 ≠ 当前活动方案 ⇒ **轻量切换**：`engine_mgr.switch_schema` + 与方案绑定的运行期同步
（拆字库、辅助码表、工具栏/语言栏刷新等 `finish_user_schema_switch` 里除下列四项外的部分）。
**不做**：写 `schema.active`、强制切回中文/取消 CapsLock、`show_status` 气泡、`notify_ui_hide`
之外的额外 UI。中英状态仍由 initial_mode 那套管，两者正交。

⛔ 不得放进 `get_current_mode`（DLL 同步阻塞路径，只许锁 + HashMap）。

### 手切分流

所有手切入口（`cycle_schema` / `switch_schema_by_id` / `toggle_schema_by_id` / 菜单
`select_schema` / `cmd_set_schema` / key_actions）最终汇到 `finish_user_schema_switch`。
在那里按「当前焦点进程有无方案规则」分流：有规则 ⇒ 更新应用覆盖方案（`@remember` 另记
记忆表），跳过 `schema.active` 写盘；其余步骤照旧（手切本就要归位中文、弹气泡）。

### 冷方案

切入时目标方案未加载完（`!is_loaded`，刚开机预热未完）⇒ 不阻塞：保持当前方案，后台
`ensure_loaded`，完成时若焦点仍在该进程再做轻量切换。

### 记忆表持久化

`RuntimeState`（`state.toml`）新增 `app_schemas: HashMap<String, String>`（进程名小写 →
方案 id）。写入走 `StateWriter`（1s 防抖、串行、关机 flush），不在按键路径上写盘。
读取时记录的 id 已不在 available ⇒ 当没记过（不主动清理，方案可能被重新启用）。

### 菜单

「应用独立配置 → 方案」：跟随全局 / 记住上次 / ── / 各可用方案（单选，选中即固定）。
设定后当场对当前焦点生效一次（同 `set_initial_state_rule` 的四步模板）。

### 已知待核实

- ~~切换后 `rt().config.schema.active` 在内存里是否刷新~~ **已核实：不刷新**（2026-09-28）。
  手切只经 `Config::set_user_string` 写磁盘，没有文件监视，内存 config 只在
  `reload_user_config` 时整份重读。故「全局方案」另存于协调器 `AppSchemaState::global`
  （`coordinator/app_schema.rs`）：启动取引擎活跃方案；无规则应用手切、设置页「设为当前」
  （`schema.setActive`）、热重载重建方案集时更新。

### 实施补注（与上文的差异与细化）

- **作用域外不重算**：切入判据是 `crossed && !out_of_scope`，与 initial_mode 完全同源。
  任务栏这类过渡窗口不推进 `mode_scope`，若在那里切了方案，回到原应用时不算跨进程、
  方案就回不来了。
- **热重载**：`reload_from_config` 会把活跃方案重置为磁盘上的 `schema.active`。重建之后
  更新全局方案，并把当前焦点应用对齐回它的目标方案——否则焦点在固定应用里时，设置页
  保存一次方案段就把它冲回全局。自动切换不写盘，不会触发重载。
- **往返热键**：自动切换让 `schema_generation` +1，`toggle_schema` 的去程记录随之失效
  （在 A 按往返键去英文、切进固定五笔的应用再回 A，再按没反应）。自动切走时挂起仍有效的
  记录，之后自动切换落回同一方案时按新代际恢复；任何手切丢弃挂起的记录。
- **冷方案的「焦点仍在」**：比对的是「意图代际」（每次切入重算与每次手切都 +1），不是
  pid / 进程名——加载期间用户在该应用里手切过，延后切换同样要作废。
- **菜单「记住上次」**：记忆表里还没有该应用时，先把**当前方案**记进去再生效，否则刚点完
  就被切到全局方案。
- **并发**：切入（焦点线程）、冷加载收尾（后台线程）、菜单、设置页 RPC 会并发走到轻量切换。
  「比较/检查 + 切换」整段持 `AppSchemaState::switch_lock`（最外层锁）；往返记录的两把锁
  （`schema_toggle_origin` / `parked_toggle`）是叶子锁、永不嵌套。锁序写在
  `coordinator/app_schema.rs` 模块文档。
- **编码处置**：焦点切入触发的轻量切换**只丢弃**缓冲（那一刻 active token 已是新宿主，按
  `commit_on_switch` 上屏会把旧应用的码打进新应用）；上屏语义只留给冷方案加载完成那条路。
- **compat 重载**：所有「写盘 → 整表重载」都经 `reload_app_compat`，它顺带重推密码框门控与
  英文配对配置——重载拿到的是整份文件，用户手写的 `password_force_english` 会随任何一次
  菜单/拖动生效，不重推就打破 core.suppress ⊆ C++.suppress。
- 已知遗留：`set_shuangpin_layout`（选双拼布局）用内存里陈旧的 config 调
  `reload_from_config`，会把活跃方案重置为**启动时**的 `schema.active`——全局场景下本就
  如此（先于本功能存在），规则应用里同样会被冲掉，未随本轮修。

---

## ③ 密码框强制英文按应用

```toml
[[apps]]
process = "someapp.exe"
password_force_english = false
```

- `None` = 跟随全局 `input.password_force_english`。
- 服务端 `apply_input_diag` 的 `enabled` 改为「规则优先，否则全局」。
- `push_password_suppress_config` 从广播改为逐客户端（照 `push_english_pair_config` +
  `auto_pair_allowed_for_pid` 的 `push_per_client`）。握手 / 菜单翻转 / 热重载三处调用都走它。
- ★ 服务端判定与推给 DLL 的值必须出自**同一个**函数，维持 core.suppress ⊆ C++.suppress
  （`coordinator.rs` 注释的不变量；违反即密码框丢键）。
- C++ 不改（DLL 每宿主一份实例，天然按进程）。macOS 无本地门控，只改服务端即可。
- 菜单：「应用独立配置 → 密码框强制英文」：跟随全局 / 开 / 关。设定后重算当前焦点的抑制态并推送。

---

## ④ 状态气泡定位

### 定位策略统一为一个枚举

全局 `ui.status.position_mode` 与按应用 `status_position_mode` 共用取值：

| 值 | 含义 |
|---|---|
| `follow_caret` | 跟随光标（现有，默认） |
| `fixed` | 固定屏幕坐标（现有，`custom_x/y`；按应用为 `status_x/y`） |
| `screen_center` `screen_top_left` `screen_top_right` `screen_bottom_left` `screen_bottom_right` | 前台窗口所在显示器工作区的锚点 |
| `window_center` `window_bottom_left` | 前台窗口可见边框的锚点 |

### 兜底位置

`ui.status.fallback_position`（按应用 `status_fallback_position`），只在 `follow_caret` 且坐标
不可信时生效：`last`（**出厂默认 = 现状**：最近一次有效坐标）/ `hide` / 上表 7 个锚点。

触发点两处：

1. `show_tip`：`resolve_caret_for_ui` 的 `valid` 此前被丢弃。改为：原始坐标无效（`caret_is_valid`
   为假）时按兜底处理；兜底为 `last` 时维持现行回退逻辑，逐字不变。
2. 焦点气泡挂起分支（`show_focus_status_if_enabled`）：原为「等不到 TSF 坐标就不显示」。兜底为
   锚点时加约 150ms 超时，到期仍无 TSF 坐标则显示在锚点；`last` / `hide` 维持现状（不超时）。
   原注释反对超时的理由是「到期只能用不可信坐标」——锚点不是光标坐标，不在此列。

### 几何

- 「屏幕」= 前台窗口所在显示器的**工作区**；「窗口」= 前台窗口可见边框（Windows 用 DWM
  扩展边框，不含阴影）。锚点带固定边距，最后经现有的工作区夹回。
- 计算放 UI 层：`UiCommand::ShowStatusTip` 的 `fixed: bool` + `fixed_x/y` 换成定位枚举
  （光标 / 固定坐标 / 锚点）。锚点矩形 → 气泡左上角的换算是纯函数，单测覆盖。
- macOS：屏幕锚点取焦点所在屏。窗口锚点若拿不到宿主窗口 frame，降级为对应的屏幕锚点
  （`window_center` → `screen_center`，`window_bottom_left` → `screen_bottom_left`），文档注明。

### 按应用与菜单

规则字段 `status_position_mode` / `status_x` / `status_y` / `status_fallback_position`。
模式与坐标从**同一层**取（同 `rule_candidate_fixed_pos`）。

- 读取：`status_position()` 先查规则再回落全局；`show_tip` 与 `show_focus_status_if_enabled`
  两处读 `si.position_mode` 的地方都改走它（漏一处 = 「有时生效有时不生效」）。
- 写入：拖动落盘 `save_status_tip_pos`、气泡右键菜单的固定/重置，命中规则时写规则
  （模式 + 坐标一起写），否则写全局，照 `save_candidate_pos_for_app`。
- 菜单「应用独立配置 → 状态提示位置」：跟随全局 / 跟随光标 / 固定（取当前位置）/ ── /
  各锚点 / 子菜单「坐标不可用时」（跟随全局 / 上次位置 / 不显示 / 各锚点）。
- 设置页（wind-setting）：状态提示段的「位置」下拉补锚点档，新增「坐标不可用时」下拉。

### 实施补注（与上文的差异与细化）

- **协议形状**：`ShowStatusTip` 的光标坐标 `x/y/caret_height` 仍留在顶层，`placement` 枚举是
  `Caret{offset_x, offset_y}` / `Fixed{x, y}` / `Anchor(StatusTipAnchor)`。没有把光标塞进
  `Caret`：固定坐标未摆过（`(0,0)` 哨兵）要拿光标选屏，锚点在取不到前台窗口时也拿光标选屏，
  三个变体都要它。
- **值域同源**：全局两个键仍是自由字符串（写错回落出厂默认，经 `StatusIndicatorConfig::position()`
  / `fallback()` 解析）；注册表的值域直接引用 `app_compat::STATUS_POSITION_MODES` /
  `STATUS_FALLBACK_POSITIONS`，与解析函数同一份。
- **兜底判据是原始坐标**：`hide` / 锚点兜底看的是本次原始坐标 `caret_is_valid`，不是
  `resolve_caret_for_ui` 回退后的 `valid`（后者有最近一次有效坐标就报 true）。`last` 下仍调
  resolve，行为逐字不变（`last_fallback_keeps_previous_behavior_for_invalid_caret` 钉住）。
- **锚点边距**：16dp（Windows 按目标屏 DPI 缩放；macOS 16pt），居中两档不用边距；最后夹回
  前台窗口所在屏的工作区。前台窗口最小化视为无边框，窗口锚点降级为屏幕锚点。
- **焦点气泡超时**：共享定时器（原 `FirstShowTimer`，改名 `OneShotTimer`）另起一个实例，
  并改为**按协调器分槽**——全进程一个槽时，同进程的多个协调器（并行测试）会互相顶掉待办。
  作废点：挂起位被清（TSF 坐标到达 / 失焦 / 下次焦点直接显示）或代际变化（下次焦点重新挂起），
  到期回调两者都校验，并按**此刻**的定位重算（兜底已不是锚点就放弃）。TSF 坐标与超时的
  消费都用 `compare_exchange`，只让一方显示。
- **落盘分流**：拖动落盘、气泡右键菜单的「固定位置」「恢复默认位置」在当前应用配了定位方式
  规则时都写规则（方式 + 坐标一起写，非 fixed 坐标清零），否则写全局；勾选态看生效值。
- **菜单**：`MenuCmd::StatusPositionRule` 占 16000+（0 跟随全局 / 1 跟随光标 / 2 固定 /
  3+i 锚点），`StatusFallbackRule` 占 17000+（0 跟随全局 / 1 上次位置 / 2 不显示 / 3+i 锚点），
  锚点下标按 `StatusAnchor::ALL`。「固定（取当前位置）」经 `ReportStatusTipPos` 取气泡当前
  位置；气泡此刻不可见时 UI 不回报，坐标留在 `(0,0)` 哨兵（落到光标所在屏），拖一次即定。
- **macOS**：`CMD_STATUS_SHOW` 尾部追加 `anchor:i32`（旧 `.app` 忽略，互容）。`.app` 的
  「焦点所在屏」取**鼠标所在屏**，其次光标点所在屏：`NSScreen.main` 问的是 `.app` 自己的
  key window（输入法没有），而锚点兜底场景下光标点恰恰不可信。窗口锚点降级为屏幕锚点。
- 未做：文档站（WindInputDocs）的 `guides/compat.mdx` / `settings/menu.mdx` / `guides/config`
  补注，与设置页（wind-setting）两个下拉，留给后续阶段。

---

## 共同约定

- 每个新 `AppCompatRule` 字段补进 `value_domain_guard.rs` 的 `app_compat_seed`；枚举/字符串
  字段走容错反序列化。
- 菜单项：`MenuCmd` 新变体 + 新千位号段 + `per_app_children` 挂载 + 处理函数套模板
  （`set_user_*` → 重载 `app_compat` → 刷新 `active_compat` → 按需推 DLL → 状态提示）。
- 文档站：`guides/compat.mdx` 补字段，`settings/menu.mdx` 补菜单；`guides/config` 里状态提示段补
  新取值与 `fallback_position`。顺手更正 `compat.mdx` 开头「整条覆盖」的过时说明（现有
  `ProtocolFields` 继承与定制层）。

## 测试

- 纯函数：解析容错（`@remember`、非法 `@x`、非 available id、锚点字符串拼错）、`set_*`、seed 元测试、
  锚点几何（边距、夹回、多屏选屏）。
- 协调器：固定 ↔ 无规则往返；`@remember` 手切—切走—切回；规则应用内手切不改 `schema.active`；
  冷方案延后切换；密码抑制按 pid 取值、推送逐客户端；气泡无效坐标走兜底、`last` 与现状一致、
  焦点挂起超时显示锚点、按应用覆盖优先级、落盘分流。
- `state.toml`：记忆表写入与重载恢复。
- 实机：微信 `@remember`、VS Code 固定英文、密码框、Illustrator 类拿不到坐标的宿主。
