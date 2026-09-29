# 码表通配符（万能键）设计

> **范围**（2026-09-29 拍板）：码表引擎的**行内通配**——开关打开后，组码时按通配键即代替
> 「恰好一个码元」，候选显示完整编码，供不确定字根时查字/学码用。
> **不做**专门的通配/反查模式（需要时另立设计）。
>
> **状态：设计已定，未实施。** 分期见 §6。

---

## 1. 调研结论（外部）

| 输入法 | 通配键 | 语义 | 首位 z 冲突 | 候选 | 默认 |
|---|---|---|---|---|---|
| 王码原始定义 | Z | 一个 Z = 一个码元；末尾 Z 也匹配更短码 | 早期任意位；现代多数禁首位 | 显示全码 | 开 |
| 小小输入法（源码） | 码表头 `wildcard=`，可配 | **恰好一个码元，总长须等** | `dwf=1`：首位不作通配，首位 z 走拼音辅助码表 | 全码提示；**唯一候选也不自动上屏** | wbx 码表开 |
| 可可五笔 Rime 版（lua） | z | 一个码元，≤3 个，只查单字 | 不区分，与 z 拼音混排 | `[全码]`，等长优先再按字频档 | **关**（F4 开；理由内存 12M→36M） |
| Rime 行列 30 | `?` | 一个码元，≤3 个 | 非字母键，无冲突 | 反查注释出编码 | 方案自带 |
| Rime wubi86 官方 | 无 | — | z 为拼音反查前缀 | — | — |
| 搜狗五笔 | Z，勾选项 | 未公开 | 未公开 | 未公开 | 需勾选 |
| macOS 五笔 | Z | 一个码元，可多个 | 拼音免引导直接混输 | — | 开 |

**共识**：一个通配 = 一个码元、可多个；**候选必须显示完整编码**（功能的学习价值所在）；
**通配结果不自动上屏/不顶屏**。**分歧**集中在首位：让位（小小/现代多数）、混排（可可）、
换非字母键（行列/百度专用键）。本设计取「让位」，并支持符号键以覆盖 26 码元方案。

来源：小小 [dgod/yong](https://github.com/dgod/yong)、可可 [Rime-KeKeWubi](https://github.com/KeKeWubi/Rime-KeKeWubi)、
[rime-array](https://github.com/rime/rime-array/blob/master/array30_query.schema.yaml)、
[rime/home#473](https://github.com/rime/home/issues/473)、
[维基教科书·五笔特殊键](https://zh.wikibooks.org/zh-hans/%E4%BA%94%E7%AD%86%E5%AD%97%E5%9E%8B%E8%BC%B8%E5%85%A5%E6%B3%95/%E7%89%B9%E6%AE%8A%E9%8D%B5)。

---

## 2. 现状（本仓）

- **无任何通配查询**。`DictLayer`（`wind-dict/src/layer.rs:32-97`）只有精确 / 前缀 / 简拼。
- **首位 z 在五笔 86 已有四重身份**：
  出厂 `system.phrases.toml` 的 `zz*` 短语使 z 恒为活码前缀（`has_code_prefix`，`handle_temp.rs:71`）；
  `z_key_repeat`（出厂开）；`z_key_action`；z 夺取 `try_z_fallback`（`handle_temp.rs:175`）。
- **五笔 86 未配 `input_chars`** ⇒ 码元集为默认 a-z，z 能进缓冲；但 wubi86 词库里 z 开头的码为 0。
- **符号码元已有闸门**：`try_code_char_gate`（`coordinator.rs`）在标点流水线之前截住码元符号；
  `code_char_conflicts` 做启动时冲突体检（见 `codetable-input-chars.md` §3.3/§3.5）。
- **两处判定遇通配会误判**：`has_longer_code` / `has_full_input_match`（`engine.rs:603-610`）在 DAT
  `walk` 遇未知字节即失败 ⇒ 超码长时**顶字误触发**、`clear_on_empty_max` 可能**误清空**。
  编码提示按位截取剩余码（`engine.rs:824-833`），通配下语义错误。

---

## 3. 行为契约

### 3.1 匹配语义

- 一个通配键 = **恰好一个码元**，可出现多个（`azzd`）。
- 结果 = **等长匹配**为主；未开 `single_code_input` 时追加更长编码的前缀补全（与现有逐码提示
  一致），**等长优先**，同档按现有基础排序。
- 结果上限：常量 100（不开放配置，YAGNI）。
- **参与**：系统词库、用户层、临时层。**不参与**：短语、Draft 层（只做精确查询）、整句、逆切分、
  五笔拼音混输。

### 3.2 上屏行为（输入含通配时）

| 行为 | 通配下 |
|---|---|
| 满码自动上屏（`decide_auto_commit` 及协调器复评） | **不触发** |
| 顶字（`handle_top_code` / `accumulate_code_char` 前置顶码） | **不触发** |
| 空码清空（`should_clear` / 清空复核） | **不触发** |
| 编码提示 `comment` | **完整编码** `c.code`（复用 `${code_hint}`，模板不改） |
| 空格 / 选词键 | 照常上屏所选候选 |

### 3.3 按键裁决（核心）

| 位置 | 规则 |
|---|---|
| **首位**（`input_buffer.is_empty()`） | 通配键**已绑定任何功能/模式**即让位、不作通配：`key_actions` / `z_key_action` / `z_key_repeat` / 临拼·快捷输入等 `trigger_keys` / 是本方案活码前缀（`has_code_prefix`）。全无绑定才进缓冲。 |
| **非首位**（组码中） | 通配键是**组码中途功能**键（选词键、翻页键、以词定字键、音节分隔符、辅助码引导键）⇒ **让位给原功能**，仅启动告警；否则作通配进缓冲，**优先于**兜底标点顶屏与「顶字+进模式」。 |

**与 `input_chars` 契约方向相反，是有意为之**：码元是方案的一部分，组码中必须归码表；
通配是用户可选的辅助功能，不应废掉已配好的选词/翻页键——冲突时保留原功能、告警提示改键。

**冲突告警**：复用 `code_char_conflicts` 的体检形状，启动时逐条日志告警（首位让位不告警，
那是设计内行为；非首位让位才告警，因为它意味着通配在该方案下实际不可用）。

**通配键是方案真实码元时**：照样按通配处理（用户显式选择），启动告警一次。

### 3.4 进缓冲闸门

通配键不在 `input_chars` 中（五笔配 `a-y`、或符号键）时，`can_enter_buffer` / `try_code_char_gate`
在满足 §3.3 的前提下放行它；符号通配键在标点流水线**之前**截住，接线点与码元闸门同处。

---

## 4. 配置

方案级，按 `schema-config-layering.md` 的三态折叠：

```toml
[schema.codetable]
# 通配输入：组码时通配键代替恰好一个码元，候选显示完整编码；通配结果不自动上屏、不顶字。
# 首位：通配键若已绑定其它功能/模式（z_key_action、z_key_repeat、引导键、活码前缀）则让位。
# 非首位：通配键若是选词/翻页/分隔/辅助码键则让位（启动告警）。
wildcard = false
# 通配键：单个字面字符（与 input_chars 同一词表），字母或 ASCII 符号，如 "z"、"?"、"`"。
# 26 码元方案请选符号键；选了真实码元则该码元被通配吞掉。
wildcard_key = "z"
```

同步点（`wind-config/AGENTS.md`「新增配置字段」）：`CodetableGlobal` 字段 + `Default`；
`CodeTableSpec` 的 `Option` 字段；`resolved()` 折叠；`REGISTRY`；`schema_overridden_keys`；
`data/config.toml` 显式写出；`CommitOptions` 带入引擎（`build_engine` 与 `codetable_settings()` 两处
折叠点判据一致）。`wildcard_key` 非法（空、多字符、非 ASCII 可打印）⇒ 告警并视为关闭。

**wind-setting**：`settings_manifest.toml` 两项（文案写明首位/非首位让位规则）、
`SPEC_BEHAVIOR_FIELDS`、`capabilities.snapshot.json`、`mockdata/config.json`。

---

## 5. 实现

### 5.1 词库层（wind-dict）

- `DictLayer::search_pattern(pattern: &str, wildcard: char, limit: usize, with_prefix: bool)`，
  默认实现返回空；`CompositeDict` 的 `Query` 加 `Pattern` 变体，去重按精确查询语义（同 text 同 code 合并）。
- **DAT**（`datformat.rs`）：DFS 逐位走，通配位遍历该状态全部有效转移（`1..=max_code`，稀疏）；
  到达末位取 `terminal_leaf`；`with_prefix` 时以所有末位状态为多起点压入现有分支限界 `pq`
  （需改 `build_code` 的单前缀假设，`datformat.rs:1321`）。
- **BTreeMap**（`codetable.rs`）：以首个通配前的字面前缀 range 扫描 + 逐条过滤。
- **redb**（`store_layer.rs`）：同上；沿用已知的「limit 先截断后排序」局限，不在本期修。
- 首位即通配 ⇒ 字面前缀为空、退化全表扫描；仅首位无冲突时可达，靠 `limit` 兜底。

### 5.2 引擎（`codetable/engine.rs`）

- `CommitOptions.wildcard: Option<char>`（关闭时 `None`）。
- `convert`：输入含通配 ⇒ 走 `search_pattern` 分支，跳过整句/逆切分；`comment = c.code`；
  排序 `cmp_exact_first` 的「精确」改判「等长」。
- `decide_auto_commit` / `should_clear` / `handle_top_code`：输入含通配即短路返回不触发。

### 5.3 协调器

- 新增 `Coordinator::wildcard_decision(key, buffer_empty) -> Yield | Enter`，集中实现 §3.3，
  调用点：`try_code_char_gate`（符号）、字母分支 `can_enter_buffer` 前（字母）、
  缓冲非空的「顶字+进模式」入口（`message_handler.rs` ~1810）前。
- **首位字母判定必须晚于** `try_activate_mode` 与 `try_z_fallback`（沿用 `codetable-input-chars.md`
  §3.4 的顺序铁律）——让位本就是它们先赢。
- 缓冲含通配时：`accumulate_code_char` 跳过顶码；`build_candidates` 跳过自动上屏复评与清空复核；
  跳过五笔拼音混输。
- 启动体检：`wildcard_conflicts()`，与 `code_char_conflicts` 并列输出。

---

## 6. 分期

| 期 | 内容 | 完成判据 |
|---|---|---|
| P1 | wind-dict `search_pattern`（DAT / BTreeMap / redb / Composite） | 三种层与全遍历对拍一致（多通配、首位通配、`with_prefix`） |
| P2 | 引擎通配分支 + 三处短路 | 引擎单测：不自动上屏 / 不顶字 / 不清空 / 全码注释 / 等长优先 |
| P3 | 配置字段全链路 + 协调器裁决 + 冲突体检 | 配置一致性测试绿；协调器集成测试（§7）绿 |
| P4 | wind-setting 设置项 + 文档站 + `engine-candidate-pipeline.md` | 设置端测试与生成器（`dev.sh sk/st/sg`）绿 |

---

## 7. 测试

- **wind-dict**：`search_pattern` 与 `for_each_entry` 全遍历过滤结果对拍（仿 `bnb_matches_full_scan`）。
- **引擎单测**（`engine.rs` 内联，`engine_with` 夹具）：§3.2 表逐行；超码长含通配不顶字（回归 §2 的误判）。
- **协调器集成**（`wind-coordinator/tests/`，五笔 86 真实数据；注意 `build_dev/data` 缺失时静默跳过，
  以耗时判断数据在位）：
  - **默认关闭 ⇒ 行为完全不变**（对照组：`az`、首位 z、`zzbd` 短语）；
  - 开启：`az` 出候选且注释为全码；`azzd` 多通配；
  - 首位 z 让位给 `zz*` 短语 / `z_key_action` / `z_key_repeat`；
  - 26 码元方案配符号通配键能出候选、组码中不触发标点顶屏；
  - 非首位通配键与选词键冲突 ⇒ 让位且有告警；
  - 通配键不在 `input_chars`（`a-y`）时组码中仍能进缓冲。

---

## 8. 明确不做

- 专门的通配 / 反查模式（首位也想通配时的出口），留待后续。
- `*` 任意长度通配；末尾通配匹配更短码（王码原义）。
- 结果上限可配置；短语参与通配。
