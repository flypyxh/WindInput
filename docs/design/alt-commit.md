# 上屏注释 / 拼音（`input.alt_commit`）

论坛 [t138](https://forum.windinput.com/topic/138)，看板 C2-6。

## 交互

| 键 | 上屏什么 |
|---|---|
| Alt+1…9 / Alt+0 | 当前页第 1…9 / 10 个候选的注释或拼音 |
| Alt+空格 | 高亮那条（与空格同一目标；出厂高亮即首选） |

配置只有一项 `input.alt_commit`，既是开关也选内容：

| 值 | 上屏内容 |
|---|---|
| `off`（出厂） | 功能关，一个 Alt 组合都不登记 |
| `pinyin` | 带调拼音，音节间空格（`nǐ hǎo`），与注释变量 `${pinyin}` 同一算法 |
| `pinyin_plain` | 不带调（`ni hao`，`ü` 保留） |
| `comment` | 候选右侧注释段原文：当前生效的注释模板（模式级 → 方案级 → 全局），不做显示截断 |

为什么是一个键而不是「bool 开关 + 内容」两个：关着时内容没有意义，两键的组合里有一半是
死格子；设置页也只要一个下拉。键不可配：可用的数字组合只剩 Alt，配置化只会引入配了不生效的取值。

## 结局

- 有文本：已确认前缀（拼音分步上屏已选的那段）＋ 注释一起上屏，会话结束（退出路径同 Esc）。
  没被该候选消费的余码丢弃，要整句的拼音就选整句那条。**不记词频**：上屏的不是这个词。
- 没有可上屏的内容（注释模板为空、查不到读音、英文候选等），或序号超出本页：**什么都不上屏**，
  吞键，组合原样保留。不回落成候选字：用户按的是「要注释」，上屏候选字是替他做了另一件事，
  而且没法撤回。

## 冲突排查

| 位置 | 现状 | 处置 |
|---|---|---|
| 出厂配置（`data/config.toml`、`Config::default`） | 没有任何 Alt 组合 | 无冲突 |
| `keys.pin_candidate` / `delete_candidate` 模板 | 值域 `ctrl+number` / `ctrl+shift+number` / `ctrl+alt+number`，都带 Ctrl；`number_template_mods` 按修饰位**相等**判 | 与纯 Alt 不相交 |
| `keys.key_actions`（组合键） | 用户可以绑 `alt+1`、`alt+space` | **让位**：编译期撞键不登记（`hotkey.rs`），分派期 `try_alt_commit` 查到 `match_key_down` 有动作也不认领 |
| 方案 `[key_actions]` / `[session_actions]` | 只收单键与修饰键，`session_key_to_vk` 拒绝 `alt+…` | 无冲突 |
| 协调器的 Ctrl/Alt 兜底分支 | 有会话时未认领的 Ctrl/Alt 组合一律清组合、键归宿主 | 本功能排在它前面（与置顶/删除同层） |
| 小键盘 Alt+数字（Windows Alt 码） | `numpad_behavior = follow_main` 会把小键盘改写成主键盘码 | 显式排除小键盘来源；TSF 侧 `RegisterHotKey` 只注册主键盘 `0x30..0x39`，本就不含小键盘 |
| TSF 吃键 | 条目只带 `SESSION` 位：`OnTestKeyDown` / `OnKeyDown` 两道闸门只在「中文 + 有会话」时吃；候选可见期间 `_RegisterCandidateHotkeys` 把它们注册成系统热键 | 无会话时 Alt+数字（宿主菜单加速键）、Alt+空格（窗口系统菜单）照常归宿主；有候选时才被抢占 |
| TSF 候选热键槽位 | `kHotkeyIdCandidateMax = 32`，超出静默 break | 出厂置顶 10 + 删除 10 + 本功能 11 = 31，装得下；测试 `session_hotkeys_fit_candidate_slots_with_alt_commit` 钉住 |
| `should_handle_key`（Android / 后续 TSF 迁移用的吃键判据） | 按同一张表的 `Session` 策略判 | 与 TSF 同一结论 |

## 未验证（需要 Windows 实机）

- Alt+空格 在候选可见时由 `RegisterHotKey` 抢占，系统菜单不应弹出；没有候选时照常弹。
- 按 Alt+数字 上屏后松开 Alt：数字键被系统热键吃掉，宿主只看到一次单独的 Alt 按下 / 松开，
  有菜单栏的程序（记事本等）可能因此激活菜单栏。若实测会激活，再考虑在 DLL 侧补一次
  假按键打断菜单激活（与部分输入法的做法相同），目前没做。
- macOS 的 Option+数字本身会出特殊字符，该平台没有验证过。
