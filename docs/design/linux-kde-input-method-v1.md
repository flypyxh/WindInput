# Linux：KDE Plasma（input-method-v1）候选窗适配

> 状态：**已实施并真机验证**（2026-10-12，分支 `feat/kde-im-v1`）。验证环境：PVE 虚拟机
> Kubuntu 26.04.1 / KWin 6.6.6 / Fcitx5 5.1.19（`admin@192.168.5.51`）。
> 实施中追加：Wayland 下候选窗右键菜单改开在屏幕坐标系（v1 / v2 共用），见 §5。
> 起因：GH#192——KDE Plasma 6 / Wayland 下 Wayland 原生应用（Konsole、Kate、KRunner、启动器）
> 没有候选窗。KWin 只实现 input-method-v1，`WaylandCandidatePanel` 只接了 v2。
> 关联：`wind_linux/src/fcitx/WaylandPanel.{h,cpp}`、`wind_linux/src/fcitx/WindEngine.cpp`、
> `wind_linux/AGENTS.md` 差距表、文档站 `content/docs/reference/linux.mdx`「已知限制」。

## 0. 目标与非目标

**目标**：fcitx5 前端为 `wayland`（input-method-v1）的输入上下文，候选窗及跟随光标的浮层
由 KWin 按文本光标摆位，交互能力与 v2（treeland）一致。

**非目标**：
- 设了 `QT_IM_MODULE` / `GTK_IM_MODULE` 而走 D-Bus 前端的 Wayland 原生应用。它们不走
  text-input，KWin 没有光标矩形，面板会落在左上角，协议层面无解，只靠文档引导不要设。
- GNOME（不实现任何 input-method 协议）。

## 1. KWin 侧的事实（读 KWin master 源码，2026-10-12）

| 事实 | 出处 | 影响 |
|---|---|---|
| `zwp_input_panel_v1` / `zwp_input_method_v1` 只对 KWin 拉起的输入法连接（`WAYLAND_SOCKET`）可见 | `src/wayland_server.cpp` 全局过滤 | 我们借的是 fcitx5 默认连接，正是这条；前提是用户在「系统设置 → 键盘 → 虚拟键盘」选了 Fcitx 5 |
| overlay 面板由 KWin 摆位：光标下方，放不下翻到上方，夹进 `PlacementArea` | `src/inputpanelv1window.cpp` `resetPosition` | 与 v2 相同，本端不碰坐标；会避开任务栏 |
| 重新映射、光标矩形变化、尺寸变化都会重新摆位 | `handleMapped` → `maybeShow`；`cursorRectangleChanged` / `sizeChanged` → `reposition` | 摘 buffer 再贴回去就能按新光标位置出现（不像 treeland 必须销毁角色） |
| **全局只认一个面板**：新面板 `setPanel` 时 `destroyWindow()` 掉旧的，不通知客户端 | `src/inputmethod.cpp` `setPanel` | fcitx5 classicui 显示自己的窗口（默认开的「切换输入法时提示」等）会顶掉我们的面板 |
| `zwp_input_panel_surface_v1` 没有 destroy 请求 | `input-method-unstable-v1.xml` | 角色对象无法拆除；要重新拿角色只能换一个新的 `wl_surface` |
| 没有 `text_input_rectangle` 之类的回传 | 同上 | 本端只拿它打诊断日志，不影响功能 |

v1 不需要 v2 那套 `rawProxyOf`（从 fcitx5 私有包装对象反推 C 代理）：面板全局直接从本端的
registry 绑定。

## 2. 方案

### 2.1 协议接入
- 新增 `wind_linux/protocols/input-method-unstable-v1.xml`，CMake 里与其它协议一样经
  wayland-scanner 生成客户端代码。
- `onGlobal` 绑定 `zwp_input_panel_v1`（v1）。

### 2.2 分流
- `WaylandCandidatePanel::handles` 由静态函数改成成员函数：
  - `wayland_v2` → v2 后端；
  - `wayland` 且已绑定 `zwp_input_panel_v1` → v1 后端；
  - 其余 → X11 窗口（不变）。
- `WindEngine.cpp` 的 6 处调用改为成员调用。`sendHostDisplay` 的 `caretFree` 与缩放随之对 v1
  生效——服务端不再因等光标坐标而一帧不发（GH#192 的「候选窗不出现」即此）。

### 2.3 surface 角色的两个后端
- `ensurePopup` 抽成两路：v2 维持现状；v1 为 `get_input_panel_surface` + `set_overlay_panel`。
- **v1 每个显示周期（收起 → 再显示）换一个新 `wl_surface` 并重新拿角色**，以从「被 classicui
  顶掉」中恢复（协议上无从得知被顶掉）。classicui 自己也是每次收起都销毁窗口。
- **旧 surface 延迟销毁**：摘 buffer 取消映射 → 等一次 `wl_display_sync` 往返 → 指针不在其上
  时才 `wl_surface_destroy`。理由见 `hidePopup` 注释：映射着的 surface 被销毁后，迟到的
  `wl_pointer.enter` 会带崩 classicui。占位 user_data 照旧挂。
- 「退役 → 可销毁」的判定做成不依赖 Wayland 的小状态机，放进 `windcore`，单测覆盖：
  未同步不销毁、指针在上不销毁、同步且离开后销毁、连接断开时全部丢弃不发请求。
- 残留：打字中途被 classicui 顶掉时，要到下一个显示周期才恢复。

### 2.4 其余浮层
- tooltip、跟随光标的状态气泡、候选右键菜单已合成在同一张位图里，不改。
- Toast、屏幕锚定气泡、屏幕菜单底板走 layer-shell；KWin 实现了 `zwlr_layer_shell_v1`，预计
  直接复用，需实测。

## 3. 验证（Plasma 6 虚拟机）

前提：系统设置 → 键盘 → 虚拟键盘选「Fcitx 5」；不设 `QT_IM_MODULE` / `GTK_IM_MODULE`。

- [ ] Konsole、Kate、KRunner、Plasma 启动器：候选窗跟随光标；光标在屏幕底部时翻到上方
- [ ] 鼠标：点选、滚轮翻页、悬停、右键菜单（**待确认 KWin 是否把指针事件发给面板 surface**）
- [ ] tooltip、跟随光标的状态气泡、屏幕锚定气泡、Toast
- [ ] 先切输入法触发 classicui 提示，再打字：候选窗恢复
- [ ] 连续打字、反复上屏若干分钟：surface 数量不增长（日志里打退役 / 销毁计数）
- [ ] 125% / 150% 分数缩放；多显示器
- [ ] 回归：XWayland 应用（Chrome / VS Code）走 X11 不变；treeland（192.168.5.50）v2 不变；
      既有 e2e 全过

## 4. 真机结果（2026-10-12）

- 修复前（main 打的包）：Konsole 候选窗钉在左上角、约 1 秒延迟（服务端等光标坐标超时）。
- 修复后：Konsole / Kate / KRunner / 启动器候选窗跟随光标、无延迟；屏幕底部翻到上方；鼠标点选、
  滚轮、悬停正常；反复 Ctrl+Space 触发 classicui 提示后候选窗照常出现；退役 15 个 surface 全部
  销毁，待销毁峰值 1。
- 新应用默认不是清风：Fcitx5 默认 `ShareInputState=No` / `ActiveByDefault=False`（Deepin 出厂
  `ShareInputState=All` 所以看不出）。定为 Fcitx5 行为，只写文档。

## 5. 追加：候选窗右键菜单开在屏幕坐标系

原来候选右键菜单合成进候选窗同一张位图：菜单一大位图跟着变大，合成器重摆整块，候选窗被挤走
（KWin、treeland 同此）。改为复用状态气泡菜单的屏幕菜单——全屏透明 layer 底板，落点取指针进入
底板时的输出坐标，候选窗不动，并补上「候选菜单点外部不收」。没有 layer-shell 时退回旧路径。

## 6. 文档与收尾
- `wind_linux/AGENTS.md` 差距表、`wind_linux/README.md`「已知限制」、文档站 linux 页：去掉
  KDE 限制，补上「虚拟键盘选 Fcitx 5」的配置步骤。
- 回复 GH#192，请反馈者用测试分支复测。
