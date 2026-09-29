# 码表通配符（万能键）设计

> **范围**（2026-09-29 拍板）：码表引擎的**行内通配**——开关打开后，组码时按通配键即代替
> 「恰好一个码元」，候选显示完整编码，供不确定字根时查字/学码用。
> **不做**专门的通配/反查模式（需要时另立设计）。
>
> **状态：P1–P4 已实施**（提交见 git log --grep 通配）。分期见 §6，实施计划见 `codetable-wildcard-plan.md`。
> 2026-09-29 读码后修正 §3.1/§3.3/§4/§5 多处（见 §9），以本版为准。

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
- 各层按「等长 / 更长」两档分别取名额，合并按 `(text, code)` 去重、等长优先——
  否则更长高权重编码会挤掉等长结果，按 text 去重会吞掉同字不同码。
- **参与**：系统词库、用户层、临时层。**不参与**：短语、Draft 层（只做精确查询）、整句、逆切分、
  五笔拼音混输。

### 3.2 上屏行为（输入含通配时）

| 行为 | 通配下 |
|---|---|
| 满码自动上屏（`decide_auto_commit` 及协调器复评） | **不触发** |
| 顶字（`handle_top_code` / `accumulate_code_char` 前置顶码） | **不触发** |
| 空码清空（`should_clear` / 清空复核） | **不触发** |
| 编码提示 `comment` | **完整编码** `c.code`，**不受 `show_code_hint` 门控**（功能本意即学码；复用 `${code_hint}`，模板不改） |
| 词频记账码 | 记候选**自身完整编码**，不记输入缓冲（否则把 `azzd` 写进词频表，永不命中） |
| 空格 / 选词键 | 照常上屏所选候选 |

### 3.3 按键裁决（核心）

| 位置 | 规则 |
|---|---|
| **首位**（`input_buffer.is_empty()`） | **符号通配键一律让位**：空缓冲下它本就产出标点，这份产物即是它绑定的功能（与关闭通配时逐键相同，如中文标点下 `?` 出「？」）。**字母通配键**已绑定任何功能/模式即让位：`key_actions` / `z_key_action` / `z_key_repeat` / 临拼·快捷输入等 `trigger_keys` / 是本方案活码前缀（`has_code_prefix`）；全无绑定才进缓冲。 |
| **非首位**（组码中） | 通配键是**组码中途功能**键（选词键、翻页键、以词定字键、音节分隔符、辅助码引导键）⇒ **让位给原功能**，仅启动告警；否则作通配进缓冲，**优先于**兜底标点顶屏与「顶字+进模式」——故组码中想打该符号须先上屏。 |
| **满码**（缓冲已达 `max_code_length`） | 通配键**按字面**，与关闭通配时逐键相同（`aaaa` + `z` 照常顶字）。作通配只会得到一条比任何码都长的死串：无候选、又因通配不顶字。按键裁决与 pattern 重算共用同一判据。 |

**首位让位即整串字面**：首字符是让位进来的通配键（如五笔 86 首位 z 作 `zz*` 短语码元）⇒ 本串后续的
通配键**一律按字面处理**，否则 `zzbd` 会变成 `z?bd`、短语全灭。首位判据用配置开关
（`z_key_repeat` 是否开启等），不用与上屏历史相关的 `z_key_repeat_text()`。

**`convert` 永远按字面**：`has_code_prefix` / `try_z_fallback` 靠 `engine_mgr.convert` 探测活码
（`handle_temp.rs:71`），若 `convert` 按内容识别通配，`zh` 会被判成 `?h` 的活码、z 进临拼失效。
故通配是否生效由协调器按位裁决，作通配的位替换成内部占位符后走独立入口（§5.2）。

**与 `input_chars` 契约方向相反，是有意为之**：码元是方案的一部分，组码中必须归码表；
通配是用户可选的辅助功能，不应废掉已配好的选词/翻页键——冲突时保留原功能、告警提示改键。

**冲突告警**：复用 `code_char_conflicts` 的体检形状，启动时逐条日志告警（首位让位不告警，
那是设计内行为；非首位让位才告警，因为它意味着通配在该方案下实际不可用）。

**通配键是方案真实码元时**：照样按通配处理（用户显式选择）；**仅当方案显式配置了 `input_chars`
且含该键**时启动告警——五笔 86 未配 `input_chars`（默认 a-z 含 z）但词库无 z 码，按字面判定会让
出厂主用例每次启动误报。

**字面符号判定须排除通配键**：符号通配键若同在 `input.buffer_symbol_chars`，会被当普通符号
清空候选（`coordinator.rs` 的 `char_is_literal_symbol`）。

### 3.4 进缓冲闸门

通配键不在 `input_chars` 中（五笔配 `a-y`、或符号键）时，`can_enter_buffer` / `try_code_char_gate`
在满足 §3.3 的前提下放行它；符号通配键在标点流水线**之前**截住，接线点与码元闸门同处。

符号通配键只在组码中生效（§3.3 首位一行），而组码中 C++ 本就把按键送到 core，故**不改** C++
透传标点集。

---

## 4. 配置

方案级，按 `schema-config-layering.md` 的三态折叠：

```toml
[schema.codetable]
# 通配输入：组码时通配键代替恰好一个码元，候选显示完整编码；通配结果不自动上屏、不顶字。
# 首位：符号通配键一律让位（照常出标点）；字母通配键若已绑定其它功能/模式
# （z_key_action、z_key_repeat、引导键、活码前缀）则让位。
# 非首位：通配键若是选词/翻页/分隔/辅助码键则让位（启动告警）；满码后按字面。
wildcard = false
# 通配键：单个字面字符（与 input_chars 同一词表），字母或 ASCII 符号，如 "z"、"?"、"`"。
# 26 码元方案请选符号键；选了真实码元则该码元被通配吞掉。
wildcard_key = "z"
```

同步点（`wind-config/AGENTS.md`「新增配置字段」）：`CodetableGlobal` 字段 + `Default`；
`CodeTableSpec` 的 `Option` 字段；`resolved()` 折叠；`REGISTRY`；`schema_overridden_keys`；
`data/config.toml` 显式写出；`CommitOptions` 带入引擎（`build_engine` 与 `codetable_settings()` 两处
折叠点判据一致）。`wildcard_key` 非法（空、多字符、非 ASCII 可打印、**数字、空格**）⇒ 告警并视为关闭。数字不可：
空缓冲时 C++ 不把数字送到 core（`codetable-input-chars.md` §2.3），组码中数字恒为选词键。

设置端方案对话框的 `make_sig` 不支持自由文本，通配键做成**下拉选择**；设置项 label 的「通」字
不在拼音检索表 `pinyin_initials.txt` 中，需重算。

**wind-setting**：`settings_manifest.toml` 两项（文案写明首位/非首位让位规则）、
`SPEC_BEHAVIOR_FIELDS`、`capabilities.snapshot.json`、`mockdata/config.json`。

---

## 5. 实现

### 5.1 词库层（wind-dict）

- `DictLayer::search_pattern(pattern: &str, wildcard: char, limit: usize, with_prefix: bool)`，
  默认实现返回空；`CompositeDict` 的 `Query` 加 `Pattern` 变体。现有 `merge_search` 只按 text 去重
  并按 `better` 截断，通配查询需独立合并：`(text, code)` 去重、等长优先。
- 系统层适配点：`CachedDict` 分派（`cached.rs:160`）与 `SystemDictLayer`（`wind-dict/src/manager.rs:212`）。
- **DAT**（`datformat.rs`）：DFS 逐位走，通配位遍历该状态全部有效转移（`1..=max_code`，稀疏）；
  到达末位取 `terminal_leaf`；`with_prefix` 时以所有末位状态为多起点压入现有分支限界 `pq`
  （需改 `build_code` 的单前缀假设，`datformat.rs:1321`）。
- **BTreeMap**（`codetable.rs`）：以首个通配前的字面前缀 range 扫描 + 逐条过滤。
- **redb**（`store_layer.rs`）：按字面前缀**全扫**（limit 传 0）再过滤——若把 limit 传给
  `search_user_words_prefix`，会在过滤前截断，匹配项可能整批丢失。
- 首位即通配 ⇒ 字面前缀为空、退化全表扫描；仅首位无冲突时可达，靠 `limit` 兜底。

### 5.2 引擎（`codetable/engine.rs`）

- `CommitOptions.wildcard: Option<char>`（关闭时 `None`）。
- **独立入口** `convert_wildcard(input, pattern, max)`：协调器把作通配的位替换成内部占位符
  `WILDCARD_SLOT = '\u{1}'` 后传入；`convert` 保持纯字面不变（见 §3.3）。走 `search_pattern`，
  跳过整句/逆切分；`comment = c.code`；排序「精确」改判「等长」。混输只走主码表。
- 三处上屏短路（自动上屏 / 顶字 / 清空）**不放在引擎**：引擎不知首位 z 是否字面码元，按内容短路
  会误伤 `zz*` 短语的顶码切点。改由协调器在 `build_candidates` 与 `accumulate_code_char` 中按
  「本串是否有通配位」短路。

### 5.3 协调器

- 新增 `wildcard.rs`：`wildcard_decision(buffer, key)`，集中实现 §3.3（需要整串以判断
  「首位让位即整串字面」），调用点：`try_code_char_gate`（符号）、字母分支 `can_enter_buffer` 前（字母）。
  符号键更早经过 `try_code_char_gate`，缓冲非空的「顶字+进模式」入口（`message_handler.rs` ~1810）无需另接。
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

- **wind-dict**：`search_pattern` 与 `for_each_entry` 全遍历过滤结果对拍（仿 `datformat.rs` 的
  `topn_prefix_matches_full_sort_*` 与 `reference_prefix`）。
- **引擎单测**（`engine.rs` 内联，`engine_with` 夹具）：§3.2 表逐行；超码长含通配不顶字（回归 §2 的误判）。
- **协调器集成**（`wind-coordinator/tests/`，五笔 86 真实数据；注意 `build_dev/data` 缺失时静默跳过，
  以耗时判断数据在位）：
  - **默认关闭 ⇒ 行为完全不变**（对照组：`az`、首位 z、`zzbd` 短语）；
  - 开启：`az` 出候选且注释为全码；`azzd` 多通配；
  - 首位 z 让位给 `zz*` 短语 / `z_key_action` / `z_key_repeat`，且 `zzbd` 整串字面、短语照出；
  - 开启后 z 进临拼（`try_z_fallback`）不受影响；通配选词后词频记在完整编码上；
  - 26 码元方案配符号通配键能出候选、组码中不触发标点顶屏；
  - 非首位通配键与选词键冲突 ⇒ 让位且有告警；
  - 通配键不在 `input_chars`（`a-y`）时组码中仍能进缓冲。

---

## 8. 明确不做

- 专门的通配 / 反查模式（首位也想通配时的出口），留待后续。
- `*` 任意长度通配；末尾通配匹配更短码（王码原义）。
- 结果上限可配置；短语参与通配。

---

## 9. 修订记录

- **2026-09-29 读码后修正**（写实施计划时发现）：补「首位让位即整串字面」与「`convert` 永远字面」
  两条规则（§3.3）；上屏短路从引擎移到协调器、引擎改独立入口 `convert_wildcard`（§5.2）；
  合并按 `(text, code)` 分档去重、redb 全扫再过滤、补系统层适配点（§5.1）；通配键禁数字/空格、
  真实码元告警限显式 `input_chars`、字面符号判定排除通配键、记账码用完整编码、注释不受
  `show_code_hint` 门控（§3–§4）；`wildcard_decision` 参数改为整串（§5.3）。

- **2026-09-29 实施后新增偏离**：
  - 首位符号类通配键（如 `/`）从 C++ 透传标点集剔除：`ConfigBundle` 的 `not_occupied` 减去
    `EngineManager::installed_wildcard_keys(global)`（跨已安装、已开启通配的码表方案取并集），
    并按即将生效的配置在热重载时重算，否则 `/` 在首位到不了 Rust。
  - 显示层去重：缓冲含通配位时 `build_candidates` 按 `(text, code)` 去重。
  - 记账：`freq_code` 不变；新增 `main_freq_code`，用在主输入上屏点，通配组码记候选完整编码。
  - `short_code_yield`（出简让全）在通配下跳过（让位与记录都不做）。
  - overlay 模式（临拼 / 快捷输入）永不作通配，门控在 `wildcard_enters`。
- **待定（需用户拍板）**：五笔拼音混输方案开通配且键为 `z` 时，非首位的 `z`（如 `hanzi`）被当通配，
  该串只查主码表、拼音混输被跳过。现按 §3.1「混输只走主码表」暂保留，未决定是否对混输另作处理。
  （已由下条终审修正裁决。）

- **2026-09-29 终审修正**（全分支终审）：
  - **混输关闭通配**：`MixedEngine` 有拼音子引擎时 `wildcard_key()` / `convert_wildcard` 返回 `None`，
    五笔拼音混输方案下通配不生效（§3.1「拼音不参与」）。非首位 `z` 吞掉 `hanzi` / `xianzai` 的拼音候选
    不可接受；混输用户要通配可切到纯码表方案。
  - **首位符号键一律让位**（§3.3）：符号键的标点产物即是它绑定的功能，对应用户原始要求「键已启动
    别的功能/模式时不做首键模糊匹配」；首位通配本属 §8 延后的专门模式。随之**撤销**上条「实施后
    新增偏离」第一项的 C++ 透传集剔除（`installed_wildcard_keys` / `SchemaKeyUnion.wildcard_keys` /
    热重载传参及其用例）：它只为让首位符号通配键到达 Rust 而存在，且会让 `/` 一类键跨方案被白吃。
  - **满码按字面**（§3.3 新增一行）：缓冲已达 `max_code_length` 时通配键按字面，`aaaa` + `z` 照常顶字。
  - **出厂笔画方案**显式 `wildcard = false`：笔画的 `z` 是「折」，跟随全局会吞掉真实码元
    （同文件 `z_key_repeat = false` 先例）。其余出厂码表方案（五笔 86）词库无 `z` 码，不改。
