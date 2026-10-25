# 模糊音正确拼音提示 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 开模糊音且开 `hint` 后，模糊命中的候选在正文里内嵌正确全拼并着色（「苹(ping)果」），桌面三端与 Android 都支持。

**Architecture:** 引擎在候选上算出「第几个字后插哪个正确音节」（`CandidateMeta.fuzzy_hints`，纯函数对齐用户音节与词库音节）；协调器按 `hint_scope` 取舍、按显示文字换算下标，生成带角色 `fuzzy_hint` 的 `CandidateItem.text_styled`；`wind-ui` 与 Android 按分段绘制正文。

**Tech Stack:** Rust（wind-config / wind-engine / wind-candidate / wind-coordinator / wind-ui-types / wind-ui / wind-theme / wind-mobile）、Kotlin（WindInputAndroid）、wind-setting（Rust 设置端，独立仓）。

**Spec:** `docs/design/fuzzy-pinyin-hint.md`（下称「设计」）。各任务开工前读设计 §3–§5 与 `AGENTS.md`。

## Global Constraints

- 配置：`[schema.pinyin.fuzzy] hint = false`、`hint_scope = "first"`（取值 `"first"` / `"all"`）。
- 提示形式：半角括号 + 全拼小写无声调，如 `(ping)`；整段 `(ping)` 一个角色 `fuzzy_hint`。
- 显示用：上屏文字、`code`、`learn_code`、学习/调频一律不变（设计 D7）。
- `hint=false` 或 `enabled=false` 时，所有渲染与现状逐字节一致（golden 不变）。
- 宁漏不错：字数 ≠ 音节数、简繁转换后字数变化 → 整条不标。
- 提交：只用 `git commit -F - --only -- <路径…>`；不 `git add -A`、不 `cargo fmt --all`（只 fmt 自己改的文件）。
- 各仓都在 worktree 分支 `feat/fuzzy-pinyin-hint` 上做：核心在 `/home/dufeng/develop/windinput/wt-fuzzyhint/WindInput`；其他仓在同一父目录下建同名 worktree（`wt-fuzzyhint/wind-setting`、`wt-fuzzyhint/WindInputAndroid` 等，平级放置以便相对路径依赖成立）。
- 真实词库在主仓 `build_dev/data`；worktree 要么整目录拷要么不放。测试隔离 TMPDIR；全量测试加 `--no-fail-fast`。

## Review Focus

1. **用户打对了的常用词**（`xinqing`、`renmin`、`xingfu`）在 `first` 下最多 1 个候选带提示，且不是首选——吵不吵就看这里。
2. **整句里混着简拼段**（`jintiantqzhenhao` 类）：简拼段绝不标，只标真正模糊的那段。
3. **双拼**下的模糊对偶（转换器那层）与同键音节：前者标，后者（`shared_key`）不标。
4. **候选被截断**（长整句、窄候选窗）：提示要么整段在要么整段不在，不能出现 `(pi…`。
5. **简繁转换开着**、**逆切分次选只显示后段**：下标不错位，对不上就整条不标。

每条在下面的归属任务里都有对应用例。

---

### Task 1: 配置项 `hint` / `hint_scope` 贯通到引擎

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`（`PinyinFuzzy`）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（约 :322-335 注册新字段）
- Modify: `data/config.toml`（约 :343，加两行带注释的默认值）
- Modify: `wind_input/crates/wind-engine/src/pinyin/fuzzy.rs`（`FuzzyConfig` 加 `hint: bool`）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`PinyinFuzzy` → `FuzzyConfig` 转换处）

**Interfaces:**
- Produces: `wind_config::FuzzyHintScope { First, All }`（serde `rename_all = "lowercase"`，`Default = First`）；`PinyinFuzzy { hint: bool, hint_scope: FuzzyHintScope, .. }`；`FuzzyConfig.hint: bool`（= `enabled && hint`）。`hint_scope` 由协调器从配置直接读，不进引擎。

- [ ] **Step 1: 写失败测试** —— `wind-config`：`pinyin_fuzzy_hint_defaults`（缺省反序列化 → `hint == false`、`hint_scope == FuzzyHintScope::First`）；`pinyin_fuzzy_hint_scope_parses_all`（`hint_scope = "all"` → `All`）。`wind-engine`/`manager`：`fuzzy_hint_requires_enabled`（`enabled=false, hint=true` → `FuzzyConfig.hint == false`）。
- [ ] **Step 2: 跑测试确认失败**：`cargo test -p wind-config pinyin_fuzzy_hint` → FAIL（字段不存在）。
- [ ] **Step 3: 实现字段、schema 注册、`data/config.toml` 注释、转换。** 注释写清「显示正确拼音，仅模糊音开启时生效；范围 first=只标第一个、all=全部」。
- [ ] **Step 4: 跑测试** `cargo test -p wind-config` 与 `cargo test -p wind-engine fuzzy_hint` → PASS；再跑 wind-config 的写回/资源门禁（`write_back_gates` 等，见 memory「子 agent 测试要带 wind-config 门禁」），新字段若需登记就登记。
- [ ] **Step 5: 提交** `feat(config): 模糊音提示开关 hint / hint_scope`

---

### Task 2: 纯函数——对齐用户音节与词库音节，产出提示

**Files:**
- Create: `wind_input/crates/wind-engine/src/pinyin/fuzzy_hint.rs`（`pinyin/mod.rs` 里 `mod fuzzy_hint;`）
- Modify: `wind_input/crates/wind-candidate/src/candidate.rs`（`CandidateMeta` 加字段）

**Interfaces:**
- Produces:
  - `wind_candidate::FuzzyHint { pub char_index: u16, pub correct: String }` —— 候选**引擎文字**第 `char_index` 个字（0 起）对应音节读错，正确音节为 `correct`。
  - `CandidateMeta.fuzzy_hints: Vec<FuzzyHint>`（`#[serde(skip)]`，空 = 无提示）。
  - `pub(crate) fn align_hints(typed: &str, dict_syllables: &[&str], cfg: &FuzzyConfig, input_ends_here: bool) -> Vec<FuzzyHint>`
    - `typed`：该候选消费的用户全拼串（全拼方案即候选 `code`；双拼见 Task 3）。
    - `dict_syllables`：词库读音逐音节（由 `learn_code` 的码与边界切出）。
    - `input_ends_here`：`typed` 的末尾就是用户当前输入的末尾（D10 末音节例外只在此时适用）。
    - 返回按 `char_index` 升序；对齐失败返回空。

算法（签名定不下来，需写明）：对 `dict_syllables` 做 DP，把 `typed` 切成同样多段，每段 `u_i` 对 `c_i` **按以下顺序判类**（先命中先得，判类与切法无关）：
1. `Equal`：`u == c`（代价 0，不标）；
2. `Fuzzy`：`u ∈ fuzzy_variants(c, cfg.without_shared_key())` 的拼写，含非法错音串（代价 1，**标**）——**先于 Prefix 判**：`pin` 对 `ping` 既是前缀又是模糊变体，算 `Fuzzy`，否则 `pinguo` 永远标不出；
3. `Prefix`：`u` 是 `c` 的真前缀（简拼/半截，代价 0，不标）；
4. `FuzzyPrefix`：`u` 是某模糊变体的真前缀（代价 1，不标）；
5. 都不是 → 此切法不成立。

取总代价最小的切法；平局取 `Fuzzy` 段更少者。**D10**：若 `input_ends_here` 且最后一段判为 `Fuzzy` 且 `u` 是 `c` 的真前缀（`pin`/`ping`），该段改判 `Prefix`。`cfg.hint == false` 时直接返回空（零分配）。

- [ ] **Step 1: 写失败测试**（`fuzzy_hint.rs` 内 `#[cfg(test)]`；cfg 用 `in_ing + n_l + sh_s` 开、`hint=true`）：
  - `pinguo_marks_ping`：`align_hints("pinguo", &["ping","guo"], cfg, true) == [FuzzyHint{char_index:0, correct:"ping"}]`
  - `exact_marks_nothing`：`("pingguo", ["ping","guo"])` → `[]`
  - `abbrev_marks_nothing`：`("pg", ["ping","guo"])` → `[]`
  - `trailing_prefix_not_marked`：`("pin", ["ping"], true)` → `[]`；同输入 `input_ends_here=false` → `[{0,"ping"}]`
  - `lan_nan_marked`：`("lan", ["nan"], true)` → `[{0,"nan"}]`
  - `half_last_syllable`：`("pinggu", ["ping","guo"], true)` → `[]`
  - `sentence_mixed_abbrev`：`("jintiantqzhenhao", ["jin","tian","tian","qi","zheng","hao"], true)`，cfg 开 `en_eng` → `[{4,"zheng"}]`
  - `shared_key_not_marked`：cfg.shared_key 含 `diu→[dui]`，`("diu", ["dui"])` → `[]`
  - `illegal_spelling_marked`：cfg 开 `in_ing`，`("din", ["ding"], false)` → `[{0,"ding"}]`
  - `hint_off_returns_empty`：`cfg.hint=false`，`pinguo` → `[]`
  - `misaligned_returns_empty`：`("pinguo", ["ping"])` → `[]`
- [ ] **Step 2: 跑测试** `cargo test -p wind-engine fuzzy_hint` → FAIL。
- [ ] **Step 3: 实现** `align_hints` 与 `FuzzyHint`/字段。
- [ ] **Step 4: 跑测试** → PASS。
- [ ] **Step 5: 提交** `feat(engine): 模糊音提示的音节对齐`

---

### Task 3: 引擎各路径填 `fuzzy_hints`

**Files:**
- Modify: `wind_input/crates/wind-engine/src/pinyin/mod.rs`（系统词模糊查询 `lookup_with_fuzzy_in` 产候选处约 :1753/:1775；`search_store_fuzzy` :2299；整句 :1988/:3020/:3174/:3312；同文合并继承 `learn_code` 处 :2863/:3350）
- Modify: `wind_input/crates/wind-engine/src/pinyin/shuangpin.rs`（`SylSpan` 加用户本义音节）
- Test: `wind_input/crates/wind-engine/src/pinyin/mod.rs` 的 tests 模块（夹具词典照现有 `shuangpin_shared_key_*` 用例的写法）

**Interfaces:**
- Consumes: `align_hints`、`FuzzyHint`、`FuzzyConfig.hint`（Task 1/2）。
- Produces: 每个带 `learn_code` 的拼音候选在 `hint` 开时 `meta.fuzzy_hints` 已填好；同文合并时 `fuzzy_hints` 跟随 `learn_code` 一起继承/丢弃（两者必须来自同一条）。

做法：凡设置 `meta.learn_code = Some((dict_code, dict_boundary))` 的地方，在候选成形后统一调用一个 helper `fn fill_fuzzy_hints(&self, c: &mut Candidate, query_full: &str)`：用 `dict_boundary` 切出 `dict_syllables`，`typed` = 候选 `code`（全拼），`input_ends_here = c.code.len() == query_full.len()`。
**双拼**：候选 `code` 是转换后的全拼，转换器若取了模糊对偶（`convert_pair` 结果来自 `fuzzy_initial_partners` 或「错音串垫底」），用户本义音节就丢了。在 `SylSpan` 加 `literal: Option<String>`（该键对按布局**不开模糊**时的读音；与 `pinyin` 相同或本对无合法读音时为 `None`），helper 在双拼下用逐音节 `literal.unwrap_or(pinyin)` 拼出 `typed`，并让这类候选 `is_fuzzy = true`。键对本无合法读音（错音串垫底）时 `literal` 取「声母+布局韵母」的字面拼写（如自然码 `dn` → `din`）。
**混输**：`MixedEngine.secondary` 是同一个 `PinyinEngine`，不需改；码表侧候选没有 `learn_code`，自然为空。

- [ ] **Step 1: 写失败测试**（夹具词典含 苹果/ping guo、拼过/pin guo、心情/xin qing、性情/xing qing、南/nan、蓝/lan；cfg `in_ing+n_l`，`hint=true`）：
  - `hint_fills_fuzzy_word`：convert `pinguo` → 「苹果」`fuzzy_hints == [{0,"ping"}]`，「拼过」为空。
  - `hint_off_leaves_empty`：`hint=false` 同输入 → 所有候选 `fuzzy_hints` 为空。
  - `hint_skips_trailing_prefix`：convert `pin` → 无候选带提示。
  - `hint_fills_sentence`：夹具能组出整句时，句中模糊段带提示、精确段不带（照 `sentence_*` 现有用例构造）。
  - `hint_fills_user_store_word`：用户词 苹果 存 `pingguo`，输入 `pinguo` → 带 `{0,"ping"}`。
  - `shuangpin_partner_marked`：小鹤布局 + `sh_s`，输入一个只在对偶声母下合法的键对 → 候选 `is_fuzzy` 且带提示，`correct` 为全拼。
  - `shuangpin_shared_key_not_marked`：沿用 `shuangpin_shared_key_finals_recall_both_readings` 的布局 → 同键另一读音不带提示。
- [ ] **Step 2: 跑测试** `cargo test -p wind-engine hint_` → FAIL。
- [ ] **Step 3: 实现 helper 与各调用点、`SylSpan.literal`。**
- [ ] **Step 4: 跑** `cargo test -p wind-engine --no-fail-fast` 全绿（含原有双拼/整句用例）。
- [ ] **Step 5: 提交** `feat(engine): 拼音候选带模糊音提示（全拼/双拼/整句/用户词）`

---

### Task 4: 协调器生成 `text_styled`（范围、下标换算）

**Files:**
- Modify: `wind_input/crates/wind-ui-types/src/candidate.rs`（`CandidateItem.text_styled: StyledText`，空 = 用 `text`）
- Modify: `wind_input/crates/wind-ui-types/src/styled.rs`（`TEXT_ROLES` 结构角色区加 `"fuzzy_hint"`）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（约 :7440-7560 组装 `CandidateItem` 处）
- Modify: `wind_input/crates/wind-coordinator/src/handle_mode.rs`（`cand_display_text` 附近加下标换算）
- 其他构造 `CandidateItem` 的地方（`tooltip.rs`、`handle_addword.rs`、测试）补 `text_styled: StyledText::default()`。

**Interfaces:**
- Consumes: `CandidateMeta.fuzzy_hints`（Task 3）、`PinyinFuzzy.hint_scope`（Task 1）。
- Produces: `CandidateItem.text_styled`；纯函数 `fn hint_styled_text(display: &str, hints: &[FuzzyHint]) -> StyledText`（插入 `(correct)` 段，角色 `fuzzy_hint`，其余段无角色）；`fn remap_hints(engine_text: &str, display: &str, hints: &[FuzzyHint]) -> Option<Vec<FuzzyHint>>`（逆切分只显示后段 → 扣前段字数；字数变了 → `None`）。

`hint_scope=First`：按**最终 UI 顺序**（全部候选，不只本页）只给第一个 `fuzzy_hints` 非空的候选生成 `text_styled`，其余留空；翻页/展开网格沿用同一判定（第一个之后的候选永不带提示）。`All`：每个都生成。

- [ ] **Step 1: 写失败测试**：
  - `hint_styled_text_inserts_after_char`：`("苹果", [{0,"ping"}])` → 段为 `苹` / `(ping)`[fuzzy_hint] / `果`，`as_str() == "苹(ping)果"`。
  - `remap_hints_simplified_to_traditional_same_len`：`("苹果","蘋果")` → 原样。
  - `remap_hints_len_change_drops`：字数变化 → `None`。
  - `remap_hints_suffix_display`：引擎文字「拼过苹果」、显示「苹果」、hint `{2,"ping"}` → `{0,"ping"}`；hint 落在被隐藏前段 → 丢弃该条。
  - 协调器集成（照 `tests/english_head_keeps_dict_word.rs` 构造 Coordinator，夹具同 Task 3）：`scope_first_marks_only_first_fuzzy`（`pinguo`：「苹果」`text_styled` 非空，其后模糊候选为空，首选「拼过」为空）；`scope_all_marks_every_fuzzy`；`hint_off_text_styled_empty`；`commit_text_unchanged`（选「苹果」上屏文字为「苹果」）。
  - `every_text_role_is_evaluable_somewhere` 等两条守卫保持绿（`fuzzy_hint` 归入结构角色的豁免）。
- [ ] **Step 2: 跑** `cargo test -p wind-coordinator hint` → FAIL。
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 跑** `cargo test -p wind-coordinator --no-fail-fast` 与 `-p wind-ui-types` → PASS。
- [ ] **Step 5: 提交** `feat(coordinator): 候选正文内嵌模糊音提示（首个/全部）`

---

### Task 5: 主题角色色 `fuzzy_hint`

**Files:**
- Modify: `wind_input/crates/wind-theme/src/span.rs`（角色取色；`fuzzy_hint` 未配 → 注释色，含高亮态）
- Modify: 内置默认主题文件（`data/themes/` 下默认主题；在 `wind-theme/src/normalize.rs` 的角色清单里登记，若有）
- Test: `wind_input/crates/wind-theme/src/span.rs` tests

**Interfaces:**
- Produces: `fn candidate_span_color(theme, role: Option<&str>, state: TextState, text_fallback: Rgba) -> Rgba`（或在现有 `span_color` 上扩一个 `node="text"` 的分支——实现者看哪种贴合现有代码；名称写进本任务的提交说明供 Task 6 用）。

- [ ] **Step 1: 写失败测试**：`fuzzy_hint_falls_back_to_comment`（主题未配 → 等于该状态下的注释色）；`fuzzy_hint_theme_override`（主题配了 → 用配置值）；默认主题深浅两套 `fuzzy_hint` 与候选正文色、背景色对比度 ≥ 3:1（用现有对比度工具函数，没有就内联 WCAG 公式）。
- [ ] **Step 2: 跑** `cargo test -p wind-theme fuzzy_hint` → FAIL。
- [ ] **Step 3: 实现**；默认主题取一个与注释色可区分的色（深浅各一）。
- [ ] **Step 4: 跑** `cargo test -p wind-theme --no-fail-fast` → PASS（契约 golden 若因新角色变化，确认只多了 `fuzzy_hint` 行后更新）。
- [ ] **Step 5: 提交** `feat(theme): 模糊音提示角色色`

---

### Task 6: `wind-ui` 按分段绘制候选正文

**Files:**
- Modify: `wind_input/crates/wind-ui/src/candidate_window.rs`（候选文字测宽/绘制/截断：`cand_metrics`、`truncate_candidate_text`、`water_fill`；着色参照约 :4096 注释的 `color_runs`）
- Modify: `wind_input/crates/wind-ui/src/manager_macos.rs`（若它另有一份候选文字路径）
- Test: `wind_input/crates/wind-ui/src/candidate_window/render_golden.rs`

**Interfaces:**
- Consumes: `CandidateItem.text_styled`（Task 4）、Task 5 的取色函数。

截断规则：需要截断时先尝试完整保留所有 `fuzzy_hint` 段；放不下则**整段删去提示**后再按原逻辑截正文；不得截在提示段内部。`text_styled` 为空走原路径（逐字节不变）。

- [ ] **Step 1: 写失败测试**（golden）：`golden_fuzzy_hint_horizontal`、`golden_fuzzy_hint_vertical`、`golden_fuzzy_hint_grid`（展开网格）、`golden_fuzzy_hint_highlighted`（高亮态取色）、`golden_fuzzy_hint_truncated`（窄窗：结果里不含半截 `(`）；`golden_no_hint_unchanged`：现有 golden 全部不变。
- [ ] **Step 2: 跑** `cargo test -p wind-ui golden_fuzzy` → FAIL。
- [ ] **Step 3: 实现。** 竖排蒙古文布局（`mongolian-vertical-candidates`）若候选文字走独立路径，同样接上。
- [ ] **Step 4: 跑** `cargo test -p wind-ui --no-fail-fast` → PASS；生成的 golden 图人工看一眼（提示色、间距）。
- [ ] **Step 5: 提交** `feat(ui): 候选正文分段着色，承载模糊音提示`

---

### Task 7: 移动端门面与 Android 绘制、Android 设置

**Files:**
- Modify: `wind_input/crates/wind-mobile/src/candidate_pull.rs`（`CandidateWindow` 每个候选加 `text_segments: Vec<TextSegment>`，`TextSegment { text: String, role: Option<String> }`；空 = 无分段）及 UniFFI 声明处
- Modify（WindInputAndroid worktree）：`rust/wind-android/src/lib.rs`（投影）、`app/src/main/java/com/windinput/ime/keyboard/CandidateBarView.kt`、`app/src/main/java/com/windinput/ime/panels/ExpandedCandidatesWindow.kt`、`app/src/main/assets/data/settings_manifest.toml`（模糊音区 :284 附近加两项）、`app/src/main/assets/data/config.toml`（同步核心那两行）

**Interfaces:**
- Consumes: `CandidateItem.text_styled`（Task 4）。
- Produces: Kotlin 侧 `TextSegment(text, role)` 列表；`role == "fuzzy_hint"` 用提示色（缺省 `Palette.hint`），与正文同字号。

- [ ] **Step 1: 写失败测试**：`wind-mobile` `candidates_carry_hint_segments`（夹具同 Task 3，开 hint → 「苹果」项 segments 拼起来为「苹(ping)果」且第 2 段 role 为 `fuzzy_hint`；hint 关 → segments 空）。Android：若有 JVM 单测基础设施，加 `CandidateBarView` 文字分段测宽用例；否则在提交说明里记手测步骤。`scripts/check-settings-labels.py` 通过。
- [ ] **Step 2: 跑** `cargo test -p wind-mobile hint` → FAIL。
- [ ] **Step 3: 实现**（核心与 Android 两仓分别提交；Android 主仓 main 有他人未提交改动，只在 worktree 分支里做）。
- [ ] **Step 4: 跑** `cargo test -p wind-mobile`、Android 侧 `./gradlew :app:testDebugUnitTest`（及能跑的 assemble）→ PASS。
- [ ] **Step 5: 提交** 核心：`feat(mobile): 候选正文分段透传`；Android：`feat: 模糊音提示分段绘制与设置项`

---

### Task 8: 设置端、文档站

**Files:**
- Modify（wind-setting worktree）：`src/assets/settings_manifest.toml`（模糊音区约 :784-880，加「显示正确拼音」开关与「提示范围」下拉：首个模糊候选 / 全部模糊候选，`hint` 关时禁用）；必要时 `src/manifest.rs` `build_fuzzy_row`；`src/assets/pinyin_initials.txt`（label 有新字时按 `scripts/gen-pinyin-initials.ps1` 的等价实现重算，见 memory「设置项 label 会撞拼音检索表」）
- Modify（WindInputDocs worktree）：`content/docs/settings/schema/pinyin.mdx` 模糊音章节加说明（含 `first`/`all` 与「末音节未打完不标」）。

- [ ] **Step 1: 写失败测试**：wind-setting 若有清单/搜索守卫（`table_covers_every_title_char` 等），先加条目跑红；加 `fuzzy_hint_items_registered`（清单含 `schema.pinyin.fuzzy.hint` 与 `hint_scope`）。
- [ ] **Step 2: 跑** wind-setting 测试 → FAIL。
- [ ] **Step 3: 实现**；文档按现有页面体例写。
- [ ] **Step 4: 跑** wind-setting 全量测试 → PASS；文档站 `pnpm build`（或仓内现成的检查命令）→ 通过。
- [ ] **Step 5: 提交** 两仓各一笔 `feat(settings): 模糊音提示开关与范围` / `docs: 模糊音正确拼音提示`

---

### Task 9: 全链路验证

- [ ] 核心全量 `cargo test --workspace --no-fail-fast`（对照 main 基线，main 原有的红要单独标注）；TSF C++、wind-setting 全量。
- [ ] 设计 §8 验收逐条，用 Coordinator 集成测试或 Windows 靶机实测（部署 d1+pd1，见 memory「分支可直接上靶机」），截图给维护者看提示色与间距。
- [ ] 主题编辑器仓（`WindInputThemeEditor`）需要加 `fuzzy_hint` 角色：本计划不做，在汇报里列为后续。
