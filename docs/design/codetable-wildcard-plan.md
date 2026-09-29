# 码表通配符 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让码表方案在开关打开后，组码时按通配键即代替「恰好一个码元」，候选显示完整编码，且通配结果不自动上屏、不顶字、不清空。

**Architecture:** wind-dict 新增 `DictLayer::search_pattern`（DAT 多起点分支限界 / BTreeMap 字面前缀扫描 / redb 前缀扫描 + 过滤 / Composite 按 `(text, code)` 合并、等长档优先）。引擎新增**显式**的 `Engine::convert_wildcard(input, pattern, max)`，`convert` 保持字面语义不变；「缓冲里哪几位是通配」由协调器按 §3.3 裁决后以 `WILDCARD_SLOT` 占位交给引擎，三处上屏短路（自动上屏 / 清空 / 顶字）落在协调器。配置按 `CodetableGlobal` + `CodeTableSpec` 三态折叠进 `CommitOptions.wildcard`，wind-setting 暴露两项。

**Tech Stack:** Rust workspace（wind-dict / wind-engine / wind-config / wind-coordinator）+ wind-setting 仓

**Spec:** docs/design/codetable-wildcard.md

## Global Constraints

- 出厂 `wildcard = false`；关闭时一切行为与现状逐键相同（对照组：`az`、首位 `z`、`zzbd` 短语）。
- 出厂 `wildcard_key = "z"`。
- 通配键 = 单个字面 ASCII 可打印字符（与 `input_chars` 同一词表）；非法（空、多字符、非 ASCII 可打印）⇒ 告警并视为关闭。空格与数字亦非法（spec §4）。
- 一个通配键 = **恰好一个码元**，可出现多个（`azzd`）；不做 `*` 任意长度，不做末尾通配匹配更短码。
- 结果 = 等长匹配为主；未开 `single_code_input` 时追加更长编码的前缀补全，**等长优先**，同档按现有基础排序。
- 结果上限：常量 100（`WILDCARD_RESULT_LIMIT`），不开放配置。
- 参与：系统词库、用户层、临时层。不参与：短语、Draft 层、整句、逆切分、五笔拼音混输（混输只走主码表）、英文混入。
- 输入含通配时：满码自动上屏（含协调器复评）**不触发**；顶字（`handle_top_code` / `accumulate_code_char` 前置顶码）**不触发**；空码清空（`should_clear` / 清空复核）**不触发**。
- 通配候选 `comment` = 完整编码 `c.code`（复用 `${code_hint}`，模板不改）；空格 / 选词键照常上屏所选候选。
- 首位（`input_buffer.is_empty()`）：通配键已绑定任何功能 / 模式（`key_actions`、`z_key_action`、`z_key_repeat`、临拼·快捷输入等 `trigger_keys`、本方案活码前缀 `has_code_prefix`）即让位；全无绑定才作通配进缓冲。
- 非首位：通配键是组码中途功能键（选词键、翻页键、以词定字键、音节分隔符、辅助码引导键）⇒ 让位给原功能并启动告警；否则作通配进缓冲，优先于兜底标点顶屏与「顶字 + 进模式」。
- 首位字母判定必须晚于 `try_activate_mode` 与 `try_z_fallback`（`codetable-input-chars.md` §3.4 顺序铁律）。
- 通配键是方案真实码元时照样按通配处理；**仅当方案显式配置 `input_chars` 且含该键**时启动告警（spec §3.3），文案单列（意思是「该码元被通配吞掉」，与非首位让位告警相反）。
- 冲突告警复用 `code_char_conflicts` 的体检形状；首位让位不告警，非首位让位才告警。
- 提交纪律：禁止 `git add -A` / `git add .` / `git commit -a` / `git stash`；新文件先 `git add -- <path>`；提交一律 `git commit -F - --only -- <自己的路径…>`；提交前 `git diff --cached --name-status` 核对暂存区。开局快照里 `wind_input/Cargo.lock` 已是别人的改动，任何提交都不带它。
- 格式化：禁止 `cargo fmt` / `cargo fmt --all`；只对自己的文件 `rustfmt --edition 2024 <file>`（本仓 workspace edition 是 2024，见 `wind_input/Cargo.toml:39`）。共享文件先 `--check`，只采纳落在自己 hunk 内的改动。
- 测试环境：cargo 命令都在 `wind_input/` 下跑，带隔离 TMPDIR（`mkdir -p /tmp/wct-wcard && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test …`，路径要短，别用 scratchpad）；跑全量用 `--no-fail-fast`。coordinator 集成测试依赖 `build_dev/data`，缺失时静默跳过、计数照绿——以 `--test codetable_wildcard` 耗时 ≥ 1s 且输出里没有「跳过」为数据在位判据。
- 所有 cargo 命令带 `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard`（本 worktree 专用）；每次提交前 `git -C <仓> branch --show-current` 须为 `feat/codetable-wildcard`。
- 日志：`info!` 不得含用户输入 / 候选；通配键本身是配置值，可以进 `warn!`。

## Review Focus

- **首位让位后的字面 `z` 与作通配的 `z` 在缓冲里长得一样**：五笔 86 出厂 `zz*` 短语使首位 `z` 让位成字面码元，若第二个 `z` 按「非首位 ⇒ 通配」处理，`zzbd` 变成 `z?bd`，短语整批消失。规则：缓冲首字符是让位进来的通配键 ⇒ 整轮组码都不作通配。测试：Task 11 `zz_phrases_survive_when_wildcard_on`。
- **`has_code_prefix` / `try_z_fallback` 拿 `engine_mgr.convert` 当活码探针**：若 `convert` 按内容把 `z` 解释成通配，`zh` 会被当成 `?h` 判为活码，`z_key_action = temp_pinyin` 的 z 夺取在开通配后静默失效。规则：`convert` 永远字面，通配只走 `convert_wildcard`。测试：Task 11 `z_fallback_still_hijacks_when_wildcard_on`。
- **记账码**：`freq_code_with` 对码表候选恒用输入缓冲记账，通配选词会把 `azzd` 这种码写进词频表（读端永远查不中的孤儿键）。规则：通配组码下码表候选以 `c.code` 记账。测试：Task 11 `freq_code_under_wildcard_is_candidate_full_code`（crate 内单测）。
- **符号通配键同时在 `input.buffer_symbol_chars` 里**：`buffer_has_literal_symbol` 会把它认成「字面符号」，`update_candidates` 直接清空候选。规则：`char_is_literal_symbol` 排除通配键。测试：Task 11 `symbol_wildcard_listed_in_buffer_symbol_chars_still_queries`。
- **等长结果被更长的高权重编码挤出配额**：词组权重（词频）普遍高于单字（字频），若各层 / 合并层按 `better` 混排后截断，`limit` 小时等长结果整批丢失；另外 `merge_search` 按 text 去重会吞掉同字不同码。规则：DAT / BTreeMap / redb 各自两档取额，Composite 以 `(text, code)` 去重并按 `cmp_pattern` 排序。测试：Task 2 `search_pattern_equal_length_never_crowded_out`、Task 5 `pattern_merges_by_text_and_code_equal_length_first`。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `wind_input/crates/wind-dict/src/layer.rs` | Modify | `WILDCARD_SLOT`、`pattern_matches`、`literal_prefix`、`cmp_pattern`；`DictLayer::search_pattern` 默认空实现 |
| `wind_input/crates/wind-dict/src/lib.rs` | Modify | 再导出 `WILDCARD_SLOT` |
| `wind_input/crates/wind-dict/src/datformat.rs` | Modify | `WdatReader::search_pattern`；把 `search_prefix_inner` 的分支限界主循环抽成多起点的 `bnb_collect` |
| `wind_input/crates/wind-dict/src/codetable.rs` | Modify | `CodetableDict::search_pattern`（BTreeMap 字面前缀 range + 过滤） |
| `wind_input/crates/wind-dict/src/cached.rs` | Modify | `CachedDict::search_pattern` 分派 |
| `wind_input/crates/wind-dict/src/manager.rs` | Modify | `SystemDictLayer::search_pattern`；`DictManager::search_pattern` |
| `wind_input/crates/wind-dict/src/store_layer.rs` | Modify | `StoreUserLayer` / `StoreTempLayer::search_pattern`（Draft 保持默认空） |
| `wind_input/crates/wind-dict/src/composite.rs` | Modify | `Query::Pattern`、`CompositeDict::search_pattern`、`(text, code)` 去重、等长优先排序 |
| `wind_input/crates/wind-engine/src/engine.rs` | Modify | `Engine::wildcard_key` / `Engine::convert_wildcard` 默认实现 |
| `wind_input/crates/wind-engine/src/codetable/engine.rs` | Modify | `CommitOptions.wildcard`、`WILDCARD_RESULT_LIMIT`、`CodeTableEngine` 实现两方法 |
| `wind_input/crates/wind-engine/src/mixed/engine.rs` | Modify | 两方法代理主码表 |
| `wind_input/crates/wind-engine/src/manager.rs` | Modify | `active_wildcard_key`、`convert_wildcard`；`build_engine` 注入 `wildcard` |
| `wind_input/crates/wind-config/src/config.rs` | Modify | `CodetableGlobal.wildcard` / `.wildcard_key` + `Default` + `resolved()`；`parse_wildcard_key`、`CodetableGlobal::wildcard_char` |
| `wind_input/crates/wind-config/src/schema.rs` | Modify | `CodeTableSpec.wildcard` / `.wildcard_key`（`Option`） |
| `wind_input/crates/wind-config/src/config_schema.rs` | Modify | `REGISTRY` 两条、`schema_overridden_keys` 两条 |
| `data/config.toml` | Modify | `[schema.codetable]` 显式写出两项 + 说明 |
| `wind_input/crates/wind-coordinator/src/wildcard.rs` | Create | `WildcardDecision`、`wildcard_decision`、`wildcard_pattern`、`wildcard_conflicts`、`warn_wildcard_conflicts` |
| `wind_input/crates/wind-coordinator/src/lib.rs` | Modify | `pub(crate) mod wildcard;` |
| `wind_input/crates/wind-coordinator/src/handle_lifecycle.rs` | Modify | `is_any_mode_trigger` 提为 `pub(crate)` |
| `wind_input/crates/wind-coordinator/src/coordinator.rs` | Modify | `try_code_char_gate` 符号通配入口、`accumulate_code_char` 顶字短路、`char_is_literal_symbol` 排除通配键、`build` 里调 `warn_wildcard_conflicts` |
| `wind_input/crates/wind-coordinator/src/coordinator/message_handler.rs` | Modify | 字母臂通配入口 |
| `wind_input/crates/wind-coordinator/src/handle_candidate.rs` | Modify | `build_candidates` 分流 + 短语 / 自动上屏 / 清空短路；`freq_code` 通配记账 |
| `wind_input/crates/wind-coordinator/src/debug_support.rs` | Modify | `debug_input_buffer`、`debug_candidate_triples` |
| `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs` | Create | §7 协调器集成用例 |
| `../wind-setting/src/assets/settings_manifest.toml` | Modify | 两项设置 |
| `../wind-setting/src/dialogs/schema_codetable.rs` | Modify | `SPEC_BEHAVIOR_FIELDS` 两条 |
| `../wind-setting/src/assets/pinyin_initials.txt` | Modify（生成） | 补新 label 里的「通」 |
| `../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json` | Modify（生成） | `dev.sh sg` 重生成 |
| `docs/architecture/engine-candidate-pipeline.md` | Modify | §3.4 通配输入 |
| `docs/design/codetable-wildcard.md` | Modify | 状态行 + 回写本计划的偏离 |
| `../WindInputDocs/content/docs/settings/schema/codetable.mdx` | Modify | 文档站小节 |

---

## P1 — wind-dict `search_pattern`

### Task 1: 通配查询公共件与 `DictLayer::search_pattern` 默认实现

**Files:**
- Modify: `wind_input/crates/wind-dict/src/layer.rs`（文件末尾追加常量 / 函数；trait 内 `for_each_entry` 之后追加方法；末尾新增 `mod tests`）
- Modify: `wind_input/crates/wind-dict/src/lib.rs`（`pub use layer::{…}` 那行）
- Test: `wind_input/crates/wind-dict/src/layer.rs` 内联 `mod tests`

**Interfaces:**
- Consumes: `wind_candidate::{Candidate, better}`
- Produces:
  - `pub const WILDCARD_SLOT: char = '\u{1}';`
  - `pub fn pattern_matches(pattern: &str, wildcard: char, code: &str, with_prefix: bool) -> bool`
  - `pub fn literal_prefix(pattern: &str, wildcard: char) -> &str`
  - `pub fn cmp_pattern(pattern_len: usize, a: &Candidate, b: &Candidate) -> std::cmp::Ordering`
  - `DictLayer::search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<Candidate>`（默认 `Vec::new()`）
  - `wind_dict::WILDCARD_SLOT`（再导出）

- [ ] **Step 1: 写失败测试**

在 `layer.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_exact_length_and_prefix() {
        let w = '?';
        assert!(pattern_matches("a?c", w, "abc", false));
        assert!(!pattern_matches("a?c", w, "abd", false));
        assert!(
            !pattern_matches("a?c", w, "ab", false),
            "短于 pattern 不匹配：不做王码「末尾通配匹配更短码」"
        );
        assert!(
            !pattern_matches("a?c", w, "abcd", false),
            "更长码只在 with_prefix 时匹配"
        );
        assert!(pattern_matches("a?c", w, "abcd", true));
        assert!(pattern_matches("??", w, "zz", false), "首位通配");
        assert!(!pattern_matches("??", w, "z", true), "with_prefix 也不放行更短码");
    }

    #[test]
    fn literal_prefix_stops_at_first_wildcard() {
        let w = '?';
        assert_eq!(literal_prefix("ab?d", w), "ab");
        assert_eq!(literal_prefix("?b", w), "", "首位通配 ⇒ 字面前缀为空（退化全表扫描）");
        assert_eq!(literal_prefix("abc", w), "abc");
    }

    #[test]
    fn cmp_pattern_puts_equal_length_first_then_better() {
        let c = |code: &str, w: i32| Candidate {
            code: code.into(),
            weight: w,
            ..Default::default()
        };
        let mut v = vec![c("abcd", 9999), c("ab", 10), c("ac", 20)];
        v.sort_by(|a, b| cmp_pattern(2, a, b));
        let codes: Vec<&str> = v.iter().map(|x| x.code.as_str()).collect();
        assert_eq!(codes, ["ac", "ab", "abcd"], "等长档恒在前，档内按 better（权重降序）");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && mkdir -p /tmp/wct-wcard && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib layer::tests`
Expected: 编译失败，`cannot find function pattern_matches` / `literal_prefix` / `cmp_pattern`。

- [ ] **Step 3: 最小实现**

`layer.rs` 顶部 `use` 改为：

```rust
use std::cmp::Ordering;
use wind_candidate::{Candidate, better};
```

trait `DictLayer` 内、`for_each_entry` 之后追加：

```rust
    /// **通配查询**（`docs/design/codetable-wildcard.md` §5.1）：`pattern` 中等于 `wildcard`
    /// 的位匹配**恰好一个**任意码元，其余位字面匹配。`with_prefix` 时追加更长编码
    /// （前 `pattern` 位匹配即可）的前缀补全。
    ///
    /// 结果**等长档优先**（[`cmp_pattern`]），各层自己先按两档取额再返回：更长编码的权重
    /// 常高于等长单字（词频 vs 字频），混排后截断会把等长结果挤出配额。
    ///
    /// 默认返回空——与 [`Self::search_abbrev`] 同一取舍：不支持的层（草稿层即刻意不支持，
    /// 见 `StoreDraftLayer` 文档）召不回，而不是静默全表扫。
    fn search_pattern(
        &self,
        _pattern: &str,
        _wildcard: char,
        _limit: usize,
        _with_prefix: bool,
    ) -> Vec<Candidate> {
        Vec::new()
    }
```

文件末尾（`MutableLayer` 之后、`mod tests` 之前）追加：

```rust
/// 协调器交给引擎的通配占位符。
///
/// 通配键本身（`z`、`?`）不能直接当 pattern 的通配符：首位让位后进缓冲的 `z` 是**字面**
/// 码元，与后续作通配的 `z` 同形。协调器按裁决把「作通配的那几位」替换成本字符，
/// 它不可能出现在任何码表编码里（码元来自物理按键，`\u{1}` 按不出来）。
pub const WILDCARD_SLOT: char = '\u{1}';

/// `code` 是否匹配 `pattern`：逐位比较（`wildcard` 位任意），等长即匹配；
/// `with_prefix` 时更长的 `code` 只要前 `pattern.len()` 位匹配也算。更短的 `code` 恒不匹配。
pub fn pattern_matches(pattern: &str, wildcard: char, code: &str, with_prefix: bool) -> bool {
    let mut cs = code.chars();
    for pc in pattern.chars() {
        match cs.next() {
            Some(cc) if pc == wildcard || pc == cc => {}
            _ => return false,
        }
    }
    with_prefix || cs.next().is_none()
}

/// 首个通配位之前的字面前缀（有序结构据此做 range 扫描）。首位即通配时为空串。
pub fn literal_prefix(pattern: &str, wildcard: char) -> &str {
    match pattern.find(wildcard) {
        Some(i) => &pattern[..i],
        None => pattern,
    }
}

/// 通配结果的排序：等长（`code` 字符数 == `pattern_len`）档恒在前，档内按 [`better`]。
pub fn cmp_pattern(pattern_len: usize, a: &Candidate, b: &Candidate) -> Ordering {
    let ea = a.code.chars().count() == pattern_len;
    let eb = b.code.chars().count() == pattern_len;
    eb.cmp(&ea).then_with(|| better(a, b))
}
```

`lib.rs` 的再导出改为：

```rust
pub use layer::{DictLayer, LayerType, MutableLayer, WILDCARD_SLOT};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib layer::tests`
Expected: `test result: ok. 3 passed`。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-dict/src/layer.rs wind_input/crates/wind-dict/src/lib.rs
git diff --cached --name-status   # 应为空；非空说明有别人的暂存，靠下面的 --only 隔离
git commit -F - --only -- wind_input/crates/wind-dict/src/layer.rs wind_input/crates/wind-dict/src/lib.rs <<'EOF'
feat(dict): 通配查询公共件与 DictLayer::search_pattern 默认实现

WILDCARD_SLOT 占位、逐位匹配、字面前缀、等长优先比较器；默认实现返回空，
不支持的层召不回而不是静默全表扫。见 docs/design/codetable-wildcard.md §5.1。
EOF
git show --stat HEAD   # 文件数应为 2
```

---

### Task 2: DAT `WdatReader::search_pattern`（多起点分支限界）

**Files:**
- Modify: `wind_input/crates/wind-dict/src/datformat.rs`（`search_prefix_inner` ~1396-1510 拆出 `bnb_collect`；`search_prefix_scan` ~1519 之前新增 `search_pattern`；`mod tests` 追加两条用例与 `reference_pattern`）
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: `crate::layer::pattern_matches`；既有私有件 `walk` / `terminal_leaf` / `read_leaf_entries` / `build_code` / `RankKey` / `Ranked` / `Pending` / `PathNode` / `NO_MAXW`
- Produces:
  - `pub fn search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<DictEntry>`
  - 私有 `fn bnb_collect(&self, v: &DatView, pq: BinaryHeap<Pending>, arena: &mut Vec<PathNode>, prefix: &str, limit: usize, filter: Option<(u32, usize)>, stats: &mut PrefixSearchStats) -> Vec<DictEntry>`

- [ ] **Step 1: 写失败测试**

在 `datformat.rs` 的 `mod tests` 内（`reference_prefix` 之后）追加：

```rust
    /// 通配查询的**全遍历参考实现**：全树 DFS 收集全部条目 → 按 pattern 过滤 →
    /// 等长 / 更长两档各自按 (weight 降, order 升) 稳定排序 → 等长先取满 `limit`，余额给更长档。
    /// DFS 序即叶号序（构建期按 code 字典序分配），故稳定排序的 tie-break 与 `RankKey` 一致。
    fn reference_pattern(
        r: &WdatReader,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<DictEntry> {
        let v = &r.main;
        let n = pattern.len();
        let mut exact: Vec<DictEntry> = Vec::new();
        let mut longer: Vec<DictEntry> = Vec::new();
        let mut path: Vec<u8> = Vec::new();
        r.for_each_leaf(v, 0, &mut path, &mut |code, leaf| {
            if !crate::layer::pattern_matches(pattern, wildcard, code, with_prefix) {
                return;
            }
            let bucket = if code.len() == n { &mut exact } else { &mut longer };
            r.read_leaf_entries(v, leaf, &mut |text, weight, order, boundary| {
                bucket.push(DictEntry {
                    code: code.to_string(),
                    text: text.to_string(),
                    weight,
                    order,
                    boundary,
                });
            });
        });
        let key = |a: &DictEntry, b: &DictEntry| b.weight.cmp(&a.weight).then(a.order.cmp(&b.order));
        exact.sort_by(key);
        longer.sort_by(key);
        exact.truncate(limit);
        let rest = limit - exact.len();
        longer.truncate(rest);
        exact.extend(longer);
        exact
    }

    /// 通配查询 == 全遍历参考实现，逐条相同（含顺序）。覆盖多通配、首位通配、`with_prefix`、
    /// 转移空洞（`i % 5` 挖掉的码）与大量等权 tie-break。
    #[test]
    fn search_pattern_matches_full_scan() {
        let alphabet = ['a', 'b', 'c'];
        let mut data: Vec<(String, Vec<(String, i32)>)> = Vec::new();
        let mut i = 0u32;
        for len in 1..=4u32 {
            for k in 0..3usize.pow(len) {
                let mut code = String::new();
                let mut x = k;
                for _ in 0..len {
                    code.push(alphabet[x % 3]);
                    x /= 3;
                }
                i += 1;
                if i % 5 == 0 {
                    continue; // 挖空洞：逼出「该位无此转移」的分支
                }
                let w = ((i * 7919) % 7) as i32 * 100; // 权重大量重复，逼出 tie-break
                let mut ents = vec![(format!("甲{i}"), w)];
                if i % 4 == 0 {
                    ents.push((format!("乙{i}"), w)); // 一码多词
                }
                data.push((code, ents));
            }
        }
        let p = build_owned("wdat_pattern_full_scan.wdat", &data);
        let r = WdatReader::open(&p).unwrap();
        for pat in ["?", "a?", "?b", "??", "a?c", "??c?", "????", "c??a", "?a"] {
            for with_prefix in [false, true] {
                for limit in [1usize, 3, 10, 1000] {
                    assert_same(
                        &r.search_pattern(pat, '?', limit, with_prefix),
                        &reference_pattern(&r, pat, '?', limit, with_prefix),
                        &format!("pat={pat} with_prefix={with_prefix} limit={limit}"),
                    );
                }
            }
        }
        let _ = std::fs::remove_file(&p);
    }

    /// ★ 等长档不得被更长的高权重编码挤出配额（Review Focus 第 5 条）。
    #[test]
    fn search_pattern_equal_length_never_crowded_out() {
        let p = build(
            "wdat_pattern_quota.wdat",
            &[
                ("ab", &[("等长", 1)]),
                ("abcd", &[("长码", 9999)]),
                ("ac", &[("等长二", 2)]),
            ],
        );
        let r = WdatReader::open(&p).unwrap();
        let texts = |v: Vec<DictEntry>| v.into_iter().map(|e| e.text).collect::<Vec<_>>();
        assert_eq!(
            texts(r.search_pattern("a?", '?', 1, true)),
            ["等长二"],
            "limit=1 时名额先给等长档"
        );
        assert_eq!(
            texts(r.search_pattern("a?", '?', 3, true)),
            ["等长二", "等长", "长码"]
        );
        assert_eq!(
            texts(r.search_pattern("a?", '?', 3, false)),
            ["等长二", "等长"],
            "不带前缀补全时只出等长"
        );
        assert!(r.search_pattern("x?", '?', 3, true).is_empty(), "字面位无转移 ⇒ 空");
        let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib datformat::tests::search_pattern`
Expected: 编译失败，`no method named search_pattern found for struct WdatReader`。

- [ ] **Step 3: 重构 `search_prefix_inner` 抽出 `bnb_collect`（行为不变）**

把 `search_prefix_inner` 改写为只做起点准备，主循环移入新函数：

```rust
    fn search_prefix_inner(
        &self,
        prefix: &str,
        limit: usize,
        filter: Option<(u32, usize)>,
    ) -> (Vec<DictEntry>, PrefixSearchStats) {
        let mut stats = PrefixSearchStats::default();
        let v = &self.main;
        if v.dat_size == 0 || limit == 0 {
            return (Vec::new(), stats);
        }
        let Some(start) = self.walk(v, prefix) else {
            return (Vec::new(), stats);
        };
        let root_bound = self.maxw(v, start);
        if root_bound == NO_MAXW {
            return (Vec::new(), stats); // 子树无任何条目
        }
        let mut pq: BinaryHeap<Pending> = BinaryHeap::new();
        pq.push(Pending {
            bound: root_bound,
            state: start,
            path: u32::MAX,
        });
        let mut arena: Vec<PathNode> = Vec::new();
        let out = self.bnb_collect(v, pq, &mut arena, prefix, limit, filter, &mut stats);
        (out, stats)
    }

    /// 分支限界主循环：从 `pq` 里的起点（可以不止一个）出发按上界降序展开，取前 `limit` 条。
    ///
    /// 各起点的完整 code = `prefix` + 其 `path` 在 `arena` 上回溯出的字节。单起点
    /// （`search_prefix`）传查询前缀、`path = u32::MAX`；多起点（`search_pattern`）传
    /// `prefix = ""`，并把各起点自身的码预先铺进 `arena`——`build_code` 的「单前缀」假设
    /// 由此解除，而无需改它的签名。剪枝判据与正确性论证不变，见 `search_prefix` 文档。
    #[allow(clippy::too_many_arguments)]
    fn bnb_collect(
        &self,
        v: &DatView,
        mut pq: BinaryHeap<Pending>,
        arena: &mut Vec<PathNode>,
        prefix: &str,
        limit: usize,
        filter: Option<(u32, usize)>,
        stats: &mut PrefixSearchStats,
    ) -> Vec<DictEntry> {
        let mut heap: BinaryHeap<Ranked> = BinaryHeap::with_capacity(limit + 1);
        while let Some(node) = pq.pop() {
            // ……此处原样搬入 `search_prefix_inner` 旧版 `while let Some(node) = pq.pop() { … }`
            // 的**完整循环体**，只做两处机械替换：
            //   `Self::build_code(prefix, &arena, node.path)` → `Self::build_code(prefix, &arena[..], node.path)`
            //   `let entries_read = &mut stats.entries_read;` 保持不变（stats 已是 &mut）
        }
        heap.into_sorted_vec()
            .into_iter()
            .map(|r| r.entry)
            .collect()
    }
```

> 核对点：搬运循环体时 `arena.push(...)` / `arena.len()` 对 `&mut Vec` 照写即可；`stats.states_visited += 1` 不变。搬完先单独跑一次既有用例确认零行为变化：
> `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib datformat::tests` —— 除两条新用例（仍缺 `search_pattern`，编译不过就先注释掉那两条再跑）外全绿，尤其 `topn_prefix_*`、`bnb_actually_prunes_when_weights_differ`、`pruning_bound_must_be_strictly_less`。

- [ ] **Step 4: 实现 `search_pattern`**

在 `search_prefix_scan` 之前插入：

```rust
    /// 通配查询（`docs/design/codetable-wildcard.md` §5.1）：`pattern` 中等于 `wildcard`
    /// 的字节位匹配**恰好一个**任意码元，其余位字面匹配。
    ///
    /// 1. **逐位推进前沿**：通配位遍历该状态全部有效转移（`1..=max_code`，稀疏，`NO_MAXW`
    ///    子树直接跳过），字面位走一次转移；
    /// 2. **等长档**：前沿各状态的终止叶按 `RankKey` 取前 `limit`；
    /// 3. **更长档**（`with_prefix`）：余额 `limit - 等长条数` 交给 [`Self::bnb_collect`]，
    ///    以全部前沿状态的子节点为多起点入队。
    ///
    /// 两档分开取额的理由见 [`crate::layer::DictLayer::search_pattern`]。首位即通配时
    /// 前沿从根展开，靠 `limit` 与分支限界兜底。`pattern` / `wildcard` 须为 ASCII，否则返回空。
    pub fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<DictEntry> {
        let v = &self.main;
        if v.dat_size == 0
            || limit == 0
            || pattern.is_empty()
            || !pattern.is_ascii()
            || !wildcard.is_ascii()
        {
            return Vec::new();
        }
        let wb = wildcard as u8;
        let mut arena: Vec<PathNode> = Vec::new();
        // 前沿：(状态, 该状态完整码在 arena 上的末节点)。根的码为空，记 u32::MAX。
        let mut frontier: Vec<(i32, u32)> = vec![(0, u32::MAX)];
        for &pb in pattern.as_bytes() {
            let mut next: Vec<(i32, u32)> = Vec::new();
            for &(s, path) in &frontier {
                let bs = self.base(v, s);
                if bs < 0 {
                    continue; // 叶状态无出边
                }
                if pb == wb {
                    for c in 1..=v.max_code {
                        let t = bs + c;
                        if !Self::in_range(v, t)
                            || self.check(v, t) != s
                            || self.maxw(v, t) == NO_MAXW
                        {
                            continue;
                        }
                        arena.push(PathNode {
                            parent: path,
                            byte: v.rev_map[c as usize],
                        });
                        next.push((t, arena.len() as u32 - 1));
                    }
                } else {
                    let c = v.char_map[pb as usize];
                    if c < 0 {
                        continue; // 该字节不在本词库的码元里（同 `walk`）
                    }
                    let t = bs + c;
                    if !Self::in_range(v, t) || self.check(v, t) != s {
                        continue;
                    }
                    arena.push(PathNode {
                        parent: path,
                        byte: pb,
                    });
                    next.push((t, arena.len() as u32 - 1));
                }
            }
            if next.is_empty() {
                return Vec::new();
            }
            frontier = next;
        }

        // 等长档。
        let mut exact: BinaryHeap<Ranked> = BinaryHeap::with_capacity(limit + 1);
        for &(s, path) in &frontier {
            let Some(leaf) = self.terminal_leaf(v, s) else {
                continue;
            };
            let code = Self::build_code("", &arena[..], path);
            let mut slot: u16 = 0;
            self.read_leaf_entries(v, leaf, &mut |text, weight, order, boundary| {
                let key = RankKey {
                    weight,
                    order,
                    leaf,
                    slot,
                };
                slot += 1;
                if exact.len() >= limit {
                    match exact.peek() {
                        Some(worst) if key >= worst.key => return,
                        _ => {}
                    }
                    exact.pop();
                }
                exact.push(Ranked {
                    key,
                    entry: DictEntry {
                        code: code.clone(),
                        text: text.to_string(),
                        weight,
                        order,
                        boundary,
                    },
                });
            });
        }
        let mut out: Vec<DictEntry> = exact
            .into_sorted_vec()
            .into_iter()
            .map(|r| r.entry)
            .collect();
        let rest = limit - out.len();
        if !with_prefix || rest == 0 {
            return out;
        }

        // 更长档：前沿各状态的子节点（紧凑码 ≥1）作多起点。
        let mut pq: BinaryHeap<Pending> = BinaryHeap::new();
        for &(s, path) in &frontier {
            let bs = self.base(v, s);
            if bs < 0 {
                continue;
            }
            for c in 1..=v.max_code {
                let t = bs + c;
                if !Self::in_range(v, t) || self.check(v, t) != s {
                    continue;
                }
                let bound = self.maxw(v, t);
                if bound == NO_MAXW {
                    continue;
                }
                arena.push(PathNode {
                    parent: path,
                    byte: v.rev_map[c as usize],
                });
                pq.push(Pending {
                    bound,
                    state: t,
                    path: arena.len() as u32 - 1,
                });
            }
        }
        let mut stats = PrefixSearchStats::default();
        out.extend(self.bnb_collect(v, pq, &mut arena, "", rest, None, &mut stats));
        out
    }
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib datformat::tests`
Expected: 全绿，含 `search_pattern_matches_full_scan`、`search_pattern_equal_length_never_crowded_out` 与全部既有 `topn_prefix_*` / `bnb_*`。

- [ ] **Step 6: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-dict/src/datformat.rs   # 只采纳落在自己 hunk 内的改动
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-dict/src/datformat.rs <<'EOF'
feat(dict): wdat 通配查询 search_pattern（多起点分支限界）

分支限界主循环抽成 bnb_collect，起点可以不止一个：各起点的码预铺进 arena、
prefix 传空，build_code 的单前缀假设由此解除而签名不变。等长档先取满，
余额交给更长档，避免高权重长码挤掉等长结果。与全遍历参考实现逐条对拍。
EOF
git show --stat HEAD
```

---

### Task 3: BTreeMap 与系统层（`CodetableDict` / `CachedDict` / `SystemDictLayer`）

> **控制者裁决（预检 #15）**：本任务另加一条 `CodetableDict::search_pattern` 与「`for_each_entry` 全遍历 + `pattern_matches` 过滤 + 同口径排序截断」的对拍测试（随机/多组 pattern：单通配、多通配、首位通配、`with_prefix` 开关），与 Task 2 的 DAT 对拍同形。

**Files:**
- Modify: `wind_input/crates/wind-dict/src/codetable.rs`（`search_prefix` ~786 之后新增方法；`mod tests` ~1284 追加用例）
- Modify: `wind_input/crates/wind-dict/src/cached.rs`（`search_prefix` ~438 之后新增分派）
- Modify: `wind_input/crates/wind-dict/src/manager.rs`（`impl DictLayer for SystemDictLayer` ~212 内追加；`DictManager` ~45 追加包装）
- Test: `codetable.rs` 与 `manager.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 1 `pattern_matches` / `literal_prefix` / `cmp_pattern`；Task 2 `WdatReader::search_pattern`
- Produces:
  - `CodetableDict::search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<crate::cached::DictHit>`
  - `CachedDict::search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<DictHit>`
  - `impl DictLayer for SystemDictLayer { fn search_pattern(...) -> Vec<Candidate> }`
  - （`DictManager::search_pattern` 依赖 `CompositeDict::search_pattern`，放在 Task 5）

- [ ] **Step 1: 写失败测试**

`codetable.rs` 的 `mod tests` 追加：

```rust
    /// BTreeMap 通配：以首个通配前的字面前缀 range 扫描 + 逐条过滤；等长档先取满。
    #[test]
    fn search_pattern_scans_literal_prefix_and_tiers_equal_length_first() {
        let mut d = CodetableDict::empty();
        for (code, text, w) in [
            ("ab", "甲", 10),
            ("ac", "乙", 20),
            ("abcd", "丙", 9999),
            ("bb", "丁", 30),
            ("abd", "戊", 5),
        ] {
            d.merge_single(code.into(), text.into(), w, 0);
        }
        let got = |p: &str, lim: usize, wp: bool| -> Vec<String> {
            d.search_pattern(p, '?', lim, wp)
                .into_iter()
                .map(|h| h.text)
                .collect()
        };
        assert_eq!(got("a?", 10, false), ["乙", "甲"]);
        assert_eq!(got("a?", 10, true), ["乙", "甲", "丙", "戊"]);
        assert_eq!(got("a?", 1, true), ["乙"], "名额先给等长档");
        assert_eq!(got("?b", 10, false), ["丁", "甲"], "首位通配：字面前缀为空，全表扫");
        assert_eq!(got("??d", 10, false), ["戊"]);
        assert!(got("x?", 10, true).is_empty());
    }
```

`manager.rs`（wind-dict）的 `mod tests` 追加：

```rust
    /// 系统层把通配命中转成候选：带完整 code、更长码标 `is_prefix`、等长档在前。
    #[test]
    fn system_layer_answers_pattern_queries() {
        let mut d = CodetableDict::empty();
        d.merge_single("ab".into(), "甲".into(), 10, 0);
        d.merge_single("abcd".into(), "丙".into(), 9999, 0);
        let layer = SystemDictLayer::new(CachedDict::Memory(d), "sys");
        let r = layer.search_pattern(&format!("a{}", crate::WILDCARD_SLOT), crate::WILDCARD_SLOT, 10, true);
        let got: Vec<(&str, &str, bool)> = r
            .iter()
            .map(|c| (c.text.as_str(), c.code.as_str(), c.is_prefix))
            .collect();
        assert_eq!(got, [("甲", "ab", false), ("丙", "abcd", true)]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib search_pattern`
Expected: 编译失败，`no method named search_pattern found for struct CodetableDict`。

- [ ] **Step 3: 最小实现**

`codetable.rs`，`search_prefix` 之后：

```rust
    /// 通配查询（内存路径对应 [`crate::datformat::WdatReader::search_pattern`]）：以首个通配前的
    /// 字面前缀做 range 扫描、逐条过滤，等长档先取满 `limit`、余额给更长档。
    /// 首位即通配时字面前缀为空，退化为全表扫描（内存模式只在无 wdat 缓存时出现）。
    pub fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<crate::cached::DictHit> {
        if limit == 0 || pattern.is_empty() {
            return Vec::new();
        }
        let lit = crate::layer::literal_prefix(pattern, wildcard);
        let n = pattern.chars().count();
        let mut exact: Vec<crate::cached::DictHit> = Vec::new();
        let mut longer: Vec<crate::cached::DictHit> = Vec::new();
        for (code, entries) in self.entries.range(lit.to_string()..) {
            if !code.starts_with(lit) {
                break;
            }
            if !crate::layer::pattern_matches(pattern, wildcard, code, with_prefix) {
                continue;
            }
            let bucket = if code.chars().count() == n {
                &mut exact
            } else {
                &mut longer
            };
            for e in entries {
                bucket.push(crate::cached::DictHit {
                    code: code.clone(),
                    text: e.text.clone(),
                    weight: e.weight,
                    order: e.order,
                    boundary: e.boundary,
                });
            }
        }
        let key = |a: &crate::cached::DictHit, b: &crate::cached::DictHit| {
            b.weight
                .cmp(&a.weight)
                .then(a.order.cmp(&b.order))
                .then_with(|| a.code.cmp(&b.code))
        };
        exact.sort_by(key);
        longer.sort_by(key);
        exact.truncate(limit);
        let rest = limit - exact.len();
        longer.truncate(rest);
        exact.extend(longer);
        exact
    }
```

`cached.rs`，`search_prefix` 之后：

```rust
    /// 通配查询。两个后端语义一致（等长档先取满），见
    /// [`crate::datformat::WdatReader::search_pattern`]。
    pub fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<DictHit> {
        match self {
            Self::Mmap(reader) => reader
                .search_pattern(pattern, wildcard, limit, with_prefix)
                .into_iter()
                .map(|e| DictHit {
                    code: e.code,
                    text: e.text,
                    weight: e.weight,
                    order: e.order,
                    boundary: e.boundary,
                })
                .collect(),
            Self::Memory(dict) => dict.search_pattern(pattern, wildcard, limit, with_prefix),
        }
    }
```

`manager.rs`（wind-dict）`impl DictLayer for SystemDictLayer`，`has_longer_code` 之后：

```rust
    /// 通配查询：委托底层有序索引，权重按本层口径换算，等长档在前。
    fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        let n = pattern.chars().count();
        let mut v: Vec<Candidate> = self
            .dict
            .search_pattern(pattern, wildcard, limit, with_prefix)
            .into_iter()
            .map(|hit| {
                let is_prefix = hit.code.chars().count() > n;
                Candidate {
                    text: hit.text,
                    code: hit.code,
                    weight: self.effective_weight(hit.weight),
                    meta: CandidateMeta {
                        raw_weight: hit.weight,
                        weight_layer: Some(self.name.clone()),
                        ..Default::default()
                    },
                    natural_order: hit.order,
                    boundary: hit.boundary,
                    is_prefix,
                    source: CandidateSource::None,
                    ..Default::default()
                }
            })
            .collect();
        v.sort_by(|a, b| crate::layer::cmp_pattern(n, a, b));
        if limit > 0 {
            v.truncate(limit);
        }
        v
    }
```

本任务不碰 `DictManager`：它的包装要直通 `CompositeDict::search_pattern`，那在 Task 5 才有。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib search_pattern`
Expected: `search_pattern_scans_literal_prefix_and_tiers_equal_length_first`、`system_layer_answers_pattern_queries` 及 Task 2 两条全过。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-dict/src/codetable.rs wind_input/crates/wind-dict/src/cached.rs wind_input/crates/wind-dict/src/manager.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-dict/src/codetable.rs wind_input/crates/wind-dict/src/cached.rs wind_input/crates/wind-dict/src/manager.rs <<'EOF'
feat(dict): 内存码表与系统层接上通配查询

CodetableDict 按字面前缀 range 扫描后逐条过滤，CachedDict 两后端同语义分派，
SystemDictLayer 换算本层权重、标 is_prefix、等长档在前。
EOF
git show --stat HEAD
```

---

### Task 4: redb 用户层 / 临时层（草稿层不参与）

**Files:**
- Modify: `wind_input/crates/wind-dict/src/store_layer.rs`（`sort_trunc` ~58 旁新增 `sort_trunc_pattern`；`StoreUserLayer` / `StoreTempLayer` 的 `impl DictLayer` 各加一个方法；`mod tests` 追加用例）
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: `Store::search_user_words_prefix(schema, prefix, limit)` / `Store::search_temp_words_prefix(schema, prefix, limit)`（`limit = 0` 不限）；Task 1 公共件
- Produces: `StoreUserLayer::search_pattern` / `StoreTempLayer::search_pattern`（trait 方法）

- [ ] **Step 1: 写失败测试**

```rust
    /// 用户层 / 临时层回答通配查询：字面前缀扫描后过滤（**不带 limit 进 redb**——先截断再
    /// 过滤会把匹配项整批截掉），等长档在前；草稿层恒不参与。
    #[test]
    fn store_layers_answer_pattern_queries_and_drafts_stay_out() {
        let s = store("pattern");
        s.add_user_word("wb", "ab", "甲", 10, 0).unwrap();
        s.add_user_word("wb", "ac", "乙", 20, 0).unwrap();
        s.add_user_word("wb", "abcd", "丙", 99, 0).unwrap();
        s.learn_temp_word("wb", "bb", "丁", 30, 0).unwrap();
        s.add_drafts("wb", &[("ad".to_string(), "草".to_string())])
            .unwrap();
        let slot = crate::layer::WILDCARD_SLOT;
        let p = format!("a{slot}");

        let ul = StoreUserLayer::new(s.clone(), "wb");
        let got: Vec<(String, String, bool)> = ul
            .search_pattern(&p, slot, 10, true)
            .into_iter()
            .map(|c| (c.code, c.text, c.is_prefix))
            .collect();
        assert_eq!(
            got,
            [
                ("ac".to_string(), "乙".to_string(), false),
                ("ab".to_string(), "甲".to_string(), false),
                ("abcd".to_string(), "丙".to_string(), true),
            ]
        );
        let exact_only: Vec<String> = ul
            .search_pattern(&p, slot, 10, false)
            .into_iter()
            .map(|c| c.code)
            .collect();
        assert_eq!(exact_only, ["ac", "ab"]);
        assert_eq!(ul.search_pattern(&p, slot, 1, true).len(), 1, "limit 生效");

        let tl = StoreTempLayer::new(s.clone(), "wb");
        let lead: Vec<String> = tl
            .search_pattern(&format!("{slot}b"), slot, 10, false)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(lead, ["丁"], "首位通配：字面前缀为空，扫本方案全部临时词");

        let dl = StoreDraftLayer::new(s.clone(), "wb", 0);
        assert!(dl.search_pattern(&p, slot, 10, true).is_empty(), "草稿层不参与通配");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib store_layer::tests::store_layers_answer_pattern`
Expected: FAIL（trait 默认实现返回空，第一条 `assert_eq!` 左侧为 `[]`）。

- [ ] **Step 3: 最小实现**

`sort_trunc` 之后：

```rust
/// 通配结果的排序截断：等长档在前（[`crate::layer::cmp_pattern`]）。
fn sort_trunc_pattern(mut v: Vec<Candidate>, pattern_len: usize, limit: usize) -> Vec<Candidate> {
    v.sort_by(|a, b| crate::layer::cmp_pattern(pattern_len, a, b));
    if limit > 0 {
        v.truncate(limit);
    }
    v
}

/// 两层共用：字面前缀扫描（`limit = 0` 不截断）→ 过滤 → 排序截断。
///
/// ⚠️ **不能把 `limit` 传进 redb**：`search_*_words_prefix` 按 key 字典序数够就停，
/// 通配过滤发生在那之后，于是「数够的那一批里没有匹配项」就会整批落空——比
/// `search_prefix` 那条「先截断后排序」的已知局限（见 `StoreUserLayer::search_prefix`）
/// 严重得多。用户词 / 临时词表规模小，按字面前缀全扫可接受；首位通配时即全方案扫描。
fn pattern_from_records(
    recs: Vec<UserWordRecord>,
    pattern: &str,
    wildcard: char,
    limit: usize,
    with_prefix: bool,
    is_temp: bool,
) -> Vec<Candidate> {
    let n = pattern.chars().count();
    let cands = recs
        .into_iter()
        .filter(|r| crate::layer::pattern_matches(pattern, wildcard, &r.code, with_prefix))
        .map(|r| {
            let longer = r.code.chars().count() > n;
            record_to_candidate(r, is_temp, longer)
        })
        .collect();
    sort_trunc_pattern(cands, n, limit)
}
```

`impl DictLayer for StoreUserLayer` 内追加：

```rust
    fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        let lit = crate::layer::literal_prefix(pattern, wildcard);
        let recs = self
            .store
            .search_user_words_prefix(&self.schema_id, lit, 0)
            .unwrap_or_default();
        pattern_from_records(recs, pattern, wildcard, limit, with_prefix, false)
    }
```

`impl DictLayer for StoreTempLayer` 内追加：

```rust
    fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        let lit = crate::layer::literal_prefix(pattern, wildcard);
        let recs = self
            .store
            .search_temp_words_prefix(&self.schema_id, lit, 0)
            .unwrap_or_default();
        pattern_from_records(recs, pattern, wildcard, limit, with_prefix, true)
    }
```

`StoreDraftLayer` **不加**（走默认空实现），并在其结构体文档「只响应精确查询」一段末补一句：`通配查询（search_pattern）同样走默认空实现，理由相同。`

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib store_layer::tests`
Expected: 全绿（含既有 `drafts_never_surface_through_prefix_queries` 等）。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-dict/src/store_layer.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-dict/src/store_layer.rs <<'EOF'
feat(dict): 用户层与临时层接上通配查询，草稿层不参与

字面前缀全扫后过滤，不把 limit 传进 redb（先截断后过滤会整批落空）。
EOF
git show --stat HEAD
```

---

### Task 5: `CompositeDict` 的 `Query::Pattern` 与 `DictManager` 包装

**Files:**
- Modify: `wind_input/crates/wind-dict/src/composite.rs`（`enum Query` ~18；`search_abbrev_exact` ~92 之后新增公开方法；`merge_search` ~132-231；测试 `MockLayer` 的 `impl DictLayer` ~248 加方法；`mod tests` 追加用例）
- Modify: `wind_input/crates/wind-dict/src/manager.rs`（`DictManager::has_longer_code` ~60 之后）
- Test: `composite.rs` `mod tests`

**Interfaces:**
- Consumes: 各层 `DictLayer::search_pattern`；Task 1 `cmp_pattern`
- Produces:
  - `CompositeDict::search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<Candidate>`
  - `DictManager::search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<Candidate>`

- [ ] **Step 1: 写失败测试**

先给测试用 `MockLayer` 的 `impl DictLayer` 加：

```rust
        fn search_pattern(
            &self,
            pattern: &str,
            wildcard: char,
            _limit: usize,
            with_prefix: bool,
        ) -> Vec<Candidate> {
            self.items
                .iter()
                .filter(|c| crate::layer::pattern_matches(pattern, wildcard, &c.code, with_prefix))
                .cloned()
                .collect()
        }
```

再追加用例：

```rust
    /// 通配合并：**同 text 同 code** 才合并（取更高权重）；同字不同码各留一条——学码要看到
    /// 每个码位。排序等长档在前，高权重的更长编码也排不到等长之前。
    #[test]
    fn pattern_merges_by_text_and_code_equal_length_first() {
        let slot = crate::layer::WILDCARD_SLOT;
        let c = CompositeDict::new();
        c.register_layer(Box::new(MockLayer {
            name: "user".into(),
            ltype: LayerType::User,
            items: vec![cand("工", "aaaa", 50, 0)],
        }));
        c.register_layer(Box::new(MockLayer {
            name: "sys".into(),
            ltype: LayerType::System,
            items: vec![
                cand("工", "aaaa", 80, 0),
                cand("工", "abaa", 70, 1),
                cand("字", "abaaq", 9999, 2),
                cand("式", "aa", 10, 3),
            ],
        }));
        let p = format!("a{slot}{slot}a");
        let got: Vec<(String, String, i32)> = c
            .search_pattern(&p, slot, 10, true)
            .into_iter()
            .map(|x| (x.text, x.code, x.weight))
            .collect();
        assert_eq!(
            got,
            [
                ("工".to_string(), "aaaa".to_string(), 80),
                ("工".to_string(), "abaa".to_string(), 70),
                ("字".to_string(), "abaaq".to_string(), 9999),
            ]
        );
        assert_eq!(c.search_pattern(&p, slot, 10, false).len(), 2, "不带补全时无更长档");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --lib composite::tests::pattern_merges`
Expected: 编译失败，`no method named search_pattern found for struct CompositeDict`。

- [ ] **Step 3: 最小实现**

`enum Query` 追加变体（`Query` 仍是 `Copy`）：

```rust
    /// 通配（见 `DictLayer::search_pattern`）：按 `(text, code)` 去重、等长档优先。
    Pattern { wildcard: char, with_prefix: bool },
```

`search_abbrev_exact` 之后：

```rust
    /// 通配查询：跨层合并。与其它查询的两处不同——去重键是 `(text, code)`（同字不同码各留
    /// 一条，学码要看到每个码位；同码才合并取更高权重），排序等长档优先（`cmp_pattern`）。
    pub fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        self.merge_search(
            pattern,
            limit,
            Query::Pattern {
                wildcard,
                with_prefix,
            },
        )
    }
```

`merge_search` 内三处改动：

1. 取层结果的 `match kind` 追加臂：

```rust
                Query::Pattern {
                    wildcard,
                    with_prefix,
                } => layer.search_pattern(query, wildcard, limit, with_prefix),
```

2. 去重键：在 `let is_prefix = …;` 之后加 `let by_code = matches!(kind, Query::Pattern { .. });`，把循环里的
   `if let Some(&idx) = seen.get(&cand.text) {` 改为：

```rust
                let key = if by_code {
                    format!("{}\u{0}{}", cand.text, cand.code)
                } else {
                    cand.text.clone()
                };
                if let Some(&idx) = seen.get(&key) {
```

   并把末尾的 `seen.insert(cand.text.clone(), results.len());` 改为 `seen.insert(key, results.len());`。

3. 排序：`results.sort_by(wind_candidate::better);` 改为：

```rust
        if by_code {
            let n = query.chars().count();
            results.sort_by(|a, b| crate::layer::cmp_pattern(n, a, b));
        } else {
            results.sort_by(wind_candidate::better);
        }
```

`manager.rs`（wind-dict）`DictManager`，`has_longer_code` 之后：

```rust
    /// 通配查询（跨层，见 `DictLayer::search_pattern` / `CompositeDict::search_pattern`）。
    pub fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        self.composite
            .search_pattern(pattern, wildcard, limit, with_prefix)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-dict --no-fail-fast`
Expected: wind-dict 全绿（既有 `dedup_same_text_merges_source_flags` 等不受影响，因为非 Pattern 查询的去重键仍是 text）。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-dict/src/composite.rs wind_input/crates/wind-dict/src/manager.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-dict/src/composite.rs wind_input/crates/wind-dict/src/manager.rs <<'EOF'
feat(dict): CompositeDict 通配查询按 (text, code) 合并、等长优先

同字不同码各留一条（学码要看到每个码位），同码才合并取更高权重；
DictManager 直通。P1 完成：三种层与全遍历对拍一致。
EOF
git show --stat HEAD
```

---

## P2 — 引擎通配分支

### Task 6: `CommitOptions.wildcard` 与 `CodeTableEngine::convert_wildcard`

**Files:**
- Modify: `wind_input/crates/wind-engine/src/engine.rs`（trait `Engine` 内 `input_chars` ~377 之后）
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`CommitOptions` ~175 追加字段；文件顶部常量区追加 `WILDCARD_RESULT_LIMIT`；`impl Engine for CodeTableEngine` ~643 追加两方法；`mod tests` ~971 追加用例）
- Test: `codetable/engine.rs` 内联 `mod tests`（`engine_opts` 夹具 ~1186）

**Interfaces:**
- Consumes: `DictManager::search_pattern`、`wind_dict::WILDCARD_SLOT`、`wind_candidate::cmp_exact_first`
- Produces:
  - `CommitOptions { pub wildcard: Option<char>, .. }`（`Default` = `None`，派生 `Copy` 不变）
  - `pub const WILDCARD_RESULT_LIMIT: usize = 100;`（`wind_engine::codetable::engine::WILDCARD_RESULT_LIMIT`）
  - `Engine::wildcard_key(&self) -> Option<char>`（默认 `None`）
  - `Engine::convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>`（默认 `None`）

- [ ] **Step 1: 写失败测试**

`codetable/engine.rs` 的 `mod tests` 追加：

```rust
    fn wildcard_opts(extra: CommitOptions) -> CommitOptions {
        CommitOptions {
            wildcard: Some('z'),
            ..extra
        }
    }

    fn slot_pattern(p: &str) -> String {
        p.replace('?', &wind_dict::WILDCARD_SLOT.to_string())
    }

    /// §3.1 / §3.2：等长优先（更长码权重再高也排后）、注释是完整编码、源标码表。
    #[test]
    fn wildcard_equal_length_first_and_full_code_comment() {
        let e = engine_opts(
            &[
                ("ab", "甲", 10),
                ("ac", "乙", 20),
                ("abcd", "丙", 9999),
                ("bb", "丁", 50),
            ],
            wildcard_opts(CommitOptions::default()),
        );
        assert_eq!(e.wildcard_key(), Some('z'));
        let r = e.convert_wildcard("az", &slot_pattern("a?"), 50).unwrap();
        let got: Vec<(&str, &str, &str, bool)> = r
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.code.as_str(), c.comment.as_str(), c.is_exact_code))
            .collect();
        assert_eq!(
            got,
            [
                ("乙", "ac", "ac", true),
                ("甲", "ab", "ab", true),
                ("丙", "abcd", "abcd", false),
            ]
        );
        assert!(r.candidates.iter().all(|c| c.source == CandidateSource::CodeTable));
        assert_eq!(r.preedit_display, "az", "组合区显示用户所打的原串，不是占位串");
    }

    /// §3.2：通配下满码唯一也不自动上屏。反向对照：同一词库字面打全码照常上屏。
    #[test]
    fn wildcard_never_auto_commits() {
        let opts = CommitOptions {
            auto_commit_at_full: true,
            auto_commit_min_len: 4,
            ..Default::default()
        };
        let e = engine_opts(&[("aaaa", "工", 100)], wildcard_opts(opts));
        let r = e.convert_wildcard("azza", &slot_pattern("a??a"), 50).unwrap();
        assert_eq!(r.candidates.len(), 1, "前置：通配唯一命中");
        assert!(!r.should_commit && r.commit_text.is_empty(), "通配结果不自动上屏");
        let lit = e.convert("aaaa", 50).unwrap();
        assert!(lit.should_commit, "对照：字面全码仍自动上屏");
    }

    /// §3.2：通配满码无匹配也不请求清空。反向对照：字面满码空码照常清空。
    #[test]
    fn wildcard_never_requests_clear() {
        let opts = CommitOptions {
            clear_on_empty_max: true,
            ..Default::default()
        };
        let e = engine_opts(&[("aaaa", "工", 100)], wildcard_opts(opts));
        let r = e.convert_wildcard("bzzz", &slot_pattern("b???"), 50).unwrap();
        assert!(r.is_empty && !r.should_clear, "通配空码不清空");
        let lit = e.convert("bbbb", 50).unwrap();
        assert!(lit.is_empty && lit.should_clear, "对照：字面满码空码照常清空");
    }

    /// §3.1：精确匹配模式下不追加更长编码；上限常量 100。
    #[test]
    fn wildcard_respects_single_code_input_and_result_cap() {
        let e = engine_opts(
            &[("ab", "甲", 10), ("abcd", "丙", 9999)],
            wildcard_opts(CommitOptions {
                single_code_input: true,
                ..Default::default()
            }),
        );
        let r = e.convert_wildcard("az", &slot_pattern("a?"), 50).unwrap();
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["甲"]);

        let many: Vec<(String, String, i32)> = (0..150u32)
            .map(|i| {
                let c1 = (b'a' + (i / 26) as u8) as char;
                let c2 = (b'a' + (i % 26) as u8) as char;
                (format!("q{c1}{c2}"), format!("字{i}"), 1)
            })
            .collect();
        let refs: Vec<(&str, &str, i32)> =
            many.iter().map(|(c, t, w)| (c.as_str(), t.as_str(), *w)).collect();
        let e = engine_opts(&refs, wildcard_opts(CommitOptions::default()));
        let r = e.convert_wildcard("qzz", &slot_pattern("q??"), 1000).unwrap();
        assert_eq!(r.candidates.len(), WILDCARD_RESULT_LIMIT, "结果上限是常量 100");
    }

    /// 关闭通配时引擎不接通配请求；`convert` 永远是字面语义（它还被当活码探针用）。
    #[test]
    fn wildcard_off_means_no_wildcard_path_and_convert_stays_literal() {
        let e = engine_opts(&[("ab", "甲", 10)], CommitOptions::default());
        assert_eq!(e.wildcard_key(), None);
        assert!(e.convert_wildcard("az", &slot_pattern("a?"), 50).is_none());
        let on = engine_opts(&[("ab", "甲", 10)], wildcard_opts(CommitOptions::default()));
        assert!(
            on.convert("az", 50).unwrap().candidates.is_empty(),
            "开了通配，convert(\"az\") 仍按字面查——`z` 不是通配"
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --lib codetable::engine::tests::wildcard`
Expected: 编译失败，`struct CommitOptions has no field named wildcard` / `no method named convert_wildcard`。

- [ ] **Step 3: 最小实现**

`engine.rs`（trait `Engine`，`input_chars` 之后）：

```rust
    /// 通配键（`[engine.codetable].wildcard_key`；开关关闭或键非法时 `None`）。
    /// 只有码表引擎返回 `Some`，混输代理主码表。见 `docs/design/codetable-wildcard.md`。
    fn wildcard_key(&self) -> Option<char> {
        None
    }

    /// 通配转换。`pattern` 是把**作通配的那几位**替换成 [`wind_dict::WILDCARD_SLOT`] 后的输入，
    /// 哪几位作通配由协调器裁决（首位让位时首位是字面码元，spec §3.3）；`input` 是原始缓冲，
    /// 只用于组合区显示。不支持通配的引擎返回 `None`。
    ///
    /// ★ 与 [`Self::convert`] 分开而不是让 `convert` 按内容自判：`convert` 还被协调器当作
    /// 「活码前缀」探针（`has_code_prefix` / z 夺取），那里的 `z` 必须是字面码元——自判会把
    /// `zh` 当 `?h` 判活，`z_key_action` 的夺取在开通配后静默失效。
    fn convert_wildcard(
        &self,
        _input: &str,
        _pattern: &str,
        _max_candidates: usize,
    ) -> Option<ConvertResult> {
        None
    }
```

`codetable/engine.rs` 常量区（`SPLIT_FRONT_LEN` 之后）：

```rust
/// 通配结果上限（spec §3.1：常量，不开放配置）。首位即通配会退化成全表扫描，靠它兜底。
pub const WILDCARD_RESULT_LIMIT: usize = 100;
```

`CommitOptions` 末尾追加字段：

```rust
    /// 通配键（`[engine.codetable].wildcard` + `.wildcard_key` 折叠后；关闭或键非法为 `None`）。
    /// 引擎只拿它回答 [`Engine::wildcard_key`]；哪几位作通配由协调器定，见 `convert_wildcard`。
    pub wildcard: Option<char>,
```

`impl Engine for CodeTableEngine`，`input_chars` 之后：

```rust
    fn wildcard_key(&self) -> Option<char> {
        self.opts.wildcard
    }

    /// 通配转换（spec §5.2）：只查词库（系统 / 用户 / 临时；草稿层不参与），不走整句、
    /// 逆切分、英文混入；「精确」改判「等长」（落在 `is_exact_code` 上，协调器重排沿用）；
    /// 注释恒为完整编码（学码价值所在，不受 `show_code_hint` 门控）；
    /// `should_commit` / `should_clear` 恒 false（spec §3.2）。
    fn convert_wildcard(
        &self,
        input: &str,
        pattern: &str,
        max_candidates: usize,
    ) -> Option<ConvertResult> {
        self.opts.wildcard?;
        let n = pattern.chars().count();
        let with_prefix = !self.opts.single_code_input;
        let mut candidates: Vec<Candidate> = self
            .dm
            .search_pattern(pattern, wind_dict::WILDCARD_SLOT, WILDCARD_RESULT_LIMIT, with_prefix)
            .into_iter()
            .map(|mut c| {
                c.source = CandidateSource::CodeTable;
                c.is_exact_code = c.code.chars().count() == n;
                c.comment = c.code.clone();
                c
            })
            .collect();
        let base_cmp = self.opts.base_sort.cmp();
        candidates.sort_by(|a, b| cmp_exact_first(a, b).then_with(|| base_cmp(a, b)));
        candidates.truncate(max_candidates.min(WILDCARD_RESULT_LIMIT));
        let is_empty = candidates.is_empty();
        Some(ConvertResult {
            candidates,
            preedit_display: input.to_string(),
            is_empty,
            ..Default::default()
        })
    }
```

> 核对点：`manager.rs` 里 `CommitOptions { … }` 字面量只有 `build_engine` ~5615 一处（`grep -n "CommitOptions {" wind_input/crates/wind-engine/src/manager.rs`），它是显式列全字段的——本步编译会在那里报 `missing field wildcard`，**先临时补 `wildcard: None,`** 让本任务编过，Task 9 再改成真值。其余构造点都用 `..Default::default()`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --lib codetable::engine::tests`
Expected: 五条 `wildcard_*` 全过，既有用例全绿。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/codetable/engine.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): 码表引擎显式通配转换 convert_wildcard

convert 保持字面语义（它还是协调器的活码前缀探针），通配走独立方法：
等长优先、注释为完整编码、不自动上屏不清空、上限常量 100。
build_engine 暂以 None 占位，配置接线在后续提交。
EOF
git show --stat HEAD
```

---

### Task 7: 混输代理与 `EngineManager` 入口

**Files:**
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（`impl Engine for MixedEngine` 内 `input_chars` ~830 之后；`mod tests` ~1127 追加）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`active_is_leading_char` ~2574 之后新增两方法）
- Test: `mixed/engine.rs` 内联 `mod tests`

**Interfaces:**
- Consumes: Task 6 两个 trait 方法
- Produces:
  - `EngineManager::active_wildcard_key(&self) -> Option<char>`
  - `EngineManager::convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>`

- [ ] **Step 1: 写失败测试**

```rust
    /// 通配只走主码表：拼音子引擎不参与（spec §3.1「五笔拼音混输不参与」）。
    /// 对照：同一引擎走普通 convert 时拼音候选照常出现，证明 FakePinyin 确实有货。
    #[test]
    fn wildcard_goes_to_primary_only() {
        let mut d = CodetableDict::empty();
        d.merge_single("ab".into(), "甲".into(), 10, 0);
        let dm = DictManager::new();
        dm.register_layer(Box::new(SystemDictLayer::new(CachedDict::Memory(d), "sys")));
        let primary = Box::new(CodeTableEngine::new(
            4,
            CommitOptions {
                wildcard: Some('z'),
                ..Default::default()
            },
            Arc::new(dm),
        ));
        let e = MixedEngine::new(
            primary,
            Some(Box::new(FakePinyin {
                word: "拼音",
                syllables: 1,
            })),
            None,
            MixConfig::default(),
        );
        assert_eq!(e.wildcard_key(), Some('z'));
        let r = e
            .convert_wildcard("az", &format!("a{}", wind_dict::WILDCARD_SLOT), 10)
            .expect("主码表开了通配");
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["甲"], "拼音子引擎不得参与通配");
        let plain = e.convert("ab", 10).unwrap();
        assert!(
            plain.candidates.iter().any(|c| c.text == "拼音"),
            "对照：普通 convert 下拼音照常出现"
        );
    }
```

> 核对点：`FakePinyin` 定义在同一 `mod tests` 内（~1616，字段 `word: &'static str, syllables: usize`）；若它的 `convert` 对 `ab` 不产候选（其实现只看 `word` 是否为空），对照断言照样成立。混输下拼音是否出现还受 `min_pinyin_length` 等门槛影响，对照断言若因此不成立，改用 `MixConfig { min_pinyin_length: 1, ..Default::default() }`（字段名以 `MixConfig` 现有定义为准）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --lib mixed::engine::tests::wildcard_goes_to_primary_only`
Expected: FAIL，`assert_eq!(e.wildcard_key(), Some('z'))` 左侧为 `None`（trait 默认实现）。

- [ ] **Step 3: 最小实现**

`mixed/engine.rs`，`input_chars` 之后：

```rust
    /// 通配只走主码表（spec §3.1：五笔拼音混输不参与），拼音 / 英文子引擎一概不问。
    fn wildcard_key(&self) -> Option<char> {
        self.primary.wildcard_key()
    }

    fn convert_wildcard(
        &self,
        input: &str,
        pattern: &str,
        max_candidates: usize,
    ) -> Option<ConvertResult> {
        self.primary.convert_wildcard(input, pattern, max_candidates)
    }
```

`manager.rs`，`active_is_leading_char` 之后：

```rust
    /// 活跃方案的通配键（关闭 / 非码表 / 键非法时 `None`；混输取主码表的）。
    ///
    /// 与 [`Self::active_input_chars`] 同一归属理由：方案级引擎固定参数挂在引擎上，方案切换
    /// 自然跟着换。协调器**只从这里取**，不读 `codetable_settings()`——那份每次调用都重新折叠，
    /// 且非法键的告警只该在构建期出一次。
    pub fn active_wildcard_key(&self) -> Option<char> {
        self.active_engine().and_then(|e| e.wildcard_key())
    }

    /// 通配转换（透传活跃引擎）。**不**叠英文混入：通配是查码，不是打英文。
    pub fn convert_wildcard(
        &self,
        input: &str,
        pattern: &str,
        max_candidates: usize,
    ) -> Option<ConvertResult> {
        self.active_engine()?
            .convert_wildcard(input, pattern, max_candidates)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --lib --no-fail-fast`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): 混输通配代理主码表，EngineManager 暴露通配入口

P2 完成：引擎单测覆盖不自动上屏 / 不清空 / 全码注释 / 等长优先；
顶字短路在协调器侧（引擎的 handle_top_code 保持字面语义）。
EOF
git show --stat HEAD
```

---

## P3 — 配置全链路、协调器裁决与冲突体检

### Task 8: wind-config 字段全链路

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`（`CodetableGlobal` ~2068 追加两字段；`Default` ~2173；`resolved()` ~2210 在 `z_key_action` 折叠之后；`default_short_code_yield_level` 旁新增 `default_wildcard_key`、`parse_wildcard_key`；`impl CodetableGlobal` 内新增 `wildcard_char`；`mod tests` ~9484 追加用例）
- Modify: `wind_input/crates/wind-config/src/schema.rs`（`CodeTableSpec` ~479 `z_key_action` 之后）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（`REGISTRY` ~233 `z_key_action` 之后；`schema_overridden_keys` ~897 之后）
- Modify: `data/config.toml`（`[schema.codetable]` 的 `z_key_action = ""` ~78 之后）
- Test: `config.rs` 内联 `mod tests`；既有守门 `registry_covers_every_config_key` / `data_config_toml_covers_registry` / `data_config_toml_has_no_orphan_keys` / `overridden_keys_covers_every_schema_override_entry`

**Interfaces:**
- Produces:
  - `CodetableGlobal { pub wildcard: bool, pub wildcard_key: String, .. }`（`Default`：`false` / `"z"`）
  - `CodeTableSpec { pub wildcard: Option<bool>, pub wildcard_key: Option<String>, .. }`
  - `pub fn parse_wildcard_key(s: &str) -> Option<char>`（`wind_config::config::parse_wildcard_key`）
  - `CodetableGlobal::wildcard_char(&self, schema_id: &str) -> Option<char>`（关闭 ⇒ `None`；非法 ⇒ `warn!` + `None`）
  - 注册表键 `schema.codetable.wildcard`（Bool）、`schema.codetable.wildcard_key`（Str）

- [ ] **Step 1: 写失败测试**

`config.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn codetable_wildcard_folds_from_schema_and_validates_key() {
        let g = CodetableGlobal::default();
        assert!(!g.wildcard, "出厂关闭");
        assert_eq!(g.wildcard_key, "z");
        assert_eq!(g.wildcard_char("t"), None, "关闭时恒 None，不看键");

        let spec = crate::schema::CodeTableSpec {
            wildcard: Some(true),
            wildcard_key: Some("?".into()),
            ..Default::default()
        };
        assert_eq!(g.resolved(Some(&spec)).wildcard_char("t"), Some('?'));

        let on = CodetableGlobal {
            wildcard: true,
            ..Default::default()
        };
        assert_eq!(
            on.resolved(Some(&crate::schema::CodeTableSpec::default()))
                .wildcard_char("t"),
            Some('z'),
            "方案没写 ⇒ 回落全局"
        );

        for bad in ["", "ab", "中", " ", "1", "\t"] {
            let c = CodetableGlobal {
                wildcard: true,
                wildcard_key: bad.into(),
                ..Default::default()
            };
            assert_eq!(c.wildcard_char("t"), None, "非法键 {bad:?} 应视为关闭");
        }
        assert_eq!(parse_wildcard_key("Z"), Some('z'), "字母归一小写：缓冲恒存小写");
        assert_eq!(parse_wildcard_key("`"), Some('`'));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-config --lib codetable_wildcard`
Expected: 编译失败，`no field wildcard on type CodetableGlobal` / `cannot find function parse_wildcard_key`。

- [ ] **Step 3: 最小实现**

`config.rs` — `CodetableGlobal` 在 `leading_chars` 字段之后追加：

```rust
    /// 通配输入（万能键）：组码时通配键代替**恰好一个**码元，候选显示完整编码；
    /// 通配结果不自动上屏、不顶字、不清空。见 `docs/design/codetable-wildcard.md`。
    #[serde(default)]
    pub wildcard: bool,
    /// 通配键：单个字面字符（字母或 ASCII 符号）。合法性见 [`parse_wildcard_key`]。
    #[serde(default = "default_wildcard_key")]
    pub wildcard_key: String,
```

`default_short_code_yield_level` 之后：

```rust
fn default_wildcard_key() -> String {
    "z".to_string()
}

/// 通配键的合法性：恰好一个字符、ASCII 可打印（不含空格）、**不是数字**；字母归一小写。
///
/// 数字排除的理由：空缓冲时 C++ 不吃数字键（到不了 core），组码中数字恒是选词键（按 §3.3
/// 恒让位）——两个位置都当不了通配，配上只会「开了没反应」。
pub fn parse_wildcard_key(s: &str) -> Option<char> {
    let mut it = s.chars();
    let c = it.next()?;
    if it.next().is_some() || !c.is_ascii_graphic() || c.is_ascii_digit() {
        return None;
    }
    Some(c.to_ascii_lowercase())
}
```

`Default for CodetableGlobal` 在 `leading_chars: String::new(),` 之后：

```rust
            wildcard: false,
            wildcard_key: default_wildcard_key(),
```

`resolved()` 在 `if let Some(v) = &o.z_key_action { … }` 之后：

```rust
        if let Some(v) = o.wildcard {
            out.wildcard = v;
        }
        if let Some(v) = &o.wildcard_key {
            out.wildcard_key = v.clone();
        }
```

`impl CodetableGlobal` 内（`resolved` 之后）：

```rust
    /// 生效的通配键：关闭 ⇒ `None`；键非法 ⇒ 告警并视为关闭（spec §4）。
    ///
    /// 只在**构建引擎时**调用一次（`EngineManager::build_engine`），协调器经
    /// `EngineManager::active_wildcard_key` 取引擎上的值——按键热路径上反复折叠会反复告警。
    pub fn wildcard_char(&self, schema_id: &str) -> Option<char> {
        if !self.wildcard {
            return None;
        }
        let parsed = parse_wildcard_key(&self.wildcard_key);
        if parsed.is_none() {
            tracing::warn!(
                "方案 {} 的 wildcard_key = {:?} 非法（须为单个字母或 ASCII 符号），通配输入已关闭",
                schema_id,
                self.wildcard_key
            );
        }
        parsed
    }
```

`schema.rs` — `CodeTableSpec` 在 `z_key_action` 之后：

```rust
    /// 通配输入开关（`None` = 跟随全局 `schema.codetable.wildcard`）。
    #[serde(default)]
    pub wildcard: Option<bool>,
    /// 通配键（`None` = 跟随全局）。合法性见 `wind_config::config::parse_wildcard_key`。
    #[serde(default)]
    pub wildcard_key: Option<String>,
```

`config_schema.rs` — `REGISTRY` 在 `f("schema.codetable.z_key_action", Str),` 之后：

```rust
    // 通配输入（万能键）。键是单个字面字符，值域无法枚举故用 Str；合法性见 `parse_wildcard_key`。
    f("schema.codetable.wildcard", Bool),
    f("schema.codetable.wildcard_key", Str),
```

`schema_overridden_keys` 在 `("schema.codetable.z_key_action", ct.z_key_action.is_some()),` 之后：

```rust
        ("schema.codetable.wildcard", ct.wildcard.is_some()),
        ("schema.codetable.wildcard_key", ct.wildcard_key.is_some()),
```

`data/config.toml` 在 `z_key_action = ""` 之后插入：

```toml
# 通配输入（万能键）：组码时通配键代替恰好一个码元，候选显示完整编码；通配结果不自动上屏、不顶字。
# 首位：通配键若已绑定其它功能/模式（z_key_action、z_key_repeat、引导键、活码前缀）则让位。
# 非首位：通配键若是选词/翻页/分隔/辅助码键则让位（启动告警）。
# 这是**方案级**配置，按码表在自己的 [engine.codetable] 里开；此处是全局基线。
wildcard = false
# 通配键：单个字面字符（与 input_chars 同一词表），字母或 ASCII 符号，如 "z"、"?"、"`"。
# 26 码元方案请选符号键；选了真实码元则该码元被通配吞掉。数字与空格不可用。
wildcard_key = "z"
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-config --no-fail-fast`
Expected: 全绿，含 `codetable_wildcard_folds_from_schema_and_validates_key`、`registry_covers_every_config_key`、`data_config_toml_covers_registry`、`data_config_toml_has_no_orphan_keys`、`data_config_toml_values_pass_validation`、`overridden_keys_covers_every_schema_override_entry`。

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-rpc --test wind_setting_assets`
Expected: **预期红**（`capabilities_snapshot_matches_core` / `mock_config_has_every_preset_key` / `every_core_key_is_either_in_the_manifest_or_exempt` 报缺 `schema.codetable.wildcard*`），由 Task 12 在 wind-setting 侧转绿。记下失败条目名，Task 12 结束时逐条确认转绿。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/schema.rs wind_input/crates/wind-config/src/config_schema.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/schema.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml <<'EOF'
feat(config): 码表通配输入两项配置（wildcard / wildcard_key）

全局基线 + 方案 [engine.codetable] 三态折叠；键须为单个字母或 ASCII 符号，
数字与空格不可用，非法告警并视为关闭。注册表、方案覆盖反查、预置文件同步。
wind_setting_assets 跨仓守门待 wind-setting 同步后转绿。
EOF
git show --stat HEAD
```

---

### Task 9: `build_engine` 注入通配键

> **控制者裁决（预检 #5）**：本任务新测试在实现前即绿，接受；`build_engine` 接线的真值由 **Task 10** 的集成用例钉住（非 Task 11），说明文字按此理解。

**Files:**
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`build_engine` 内 `CommitOptions { … }` ~5615：把 Task 6 的 `wildcard: None,` 改为真值；`mod tests` ~6733 追加用例，紧挨 `overlay_schema_does_not_inherit_global_codetable`）
- Test: `manager.rs` 内联 `mod tests`

**Interfaces:**
- Consumes: Task 8 `CodetableGlobal::wildcard_char`；既有 `EngineManager::resolve_codetable(schema_id, data_dir, global, override_dir)`
- Produces: 构建出的码表引擎 `wildcard_key()` = 折叠后的键

- [ ] **Step 1: 写失败测试**

```rust
    /// 通配两项与其它码表行为同一条折叠链：方案写了覆盖、没写回落全局；overlay 方案
    /// 取内置基线，**不继承**全局开关（快符那类小符号表里 `z` 多半是正经编码）。
    #[test]
    fn wildcard_folds_from_schema_and_overlay_does_not_inherit() {
        use std::io::Write;
        let base_dir = std::env::temp_dir().join("wind_eng_wildcard_fold");
        let schemas = base_dir.join("schemas");
        let _ = std::fs::remove_dir_all(&base_dir);
        std::fs::create_dir_all(&schemas).unwrap();
        for (id, body) in [
            ("wc_inline", "[engine.codetable]\nwildcard_key = \"?\"\n"),
            ("wc_follow", ""),
            ("wc_overlay", "[overlay]\n"),
        ] {
            let mut f =
                std::fs::File::create(schemas.join(format!("{id}.schema.toml"))).unwrap();
            write!(
                f,
                "[schema]\nid = \"{id}\"\n[engine]\ntype = \"codetable\"\n{body}"
            )
            .unwrap();
        }
        let global = wind_config::CodetableGlobal {
            wildcard: true,
            ..Default::default()
        };
        let ov = std::env::temp_dir().join("wind_eng_wildcard_fold_ov");
        let _ = std::fs::remove_dir_all(&ov);
        let key = |id: &str| {
            EngineManager::resolve_codetable(id, Some(&base_dir), &global, Some(&ov))
                .wildcard_char(id)
        };
        assert_eq!(key("wc_inline"), Some('?'), "方案写了键 ⇒ 覆盖；开关回落全局 true");
        assert_eq!(key("wc_follow"), Some('z'), "都没写 ⇒ 全局");
        assert_eq!(key("wc_overlay"), None, "overlay 方案不继承全局开关");
        let _ = std::fs::remove_dir_all(&base_dir);
    }
```

- [ ] **Step 2: 跑测试确认失败 / 通过情况**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --lib wildcard_folds_from_schema`
Expected: **本条会直接通过**——它锁的是 `resolve_codetable`（即 `codetable_settings()` 那条折叠点），在 Task 8 之后已成立。它的作用是给下一步的 `build_engine` 改动立一个「两处折叠点同判据」的基准；`build_engine` 的真值接线由 Task 11 的协调器集成用例（`wildcard_off_by_default_changes_nothing` 的开启组）钉住：本步不改 `build_engine` 的话那条会红。

- [ ] **Step 3: 最小实现**

`build_engine` 里 `CommitOptions { … }` 的 `wildcard: None,` 改为：

```rust
                // 通配：与上屏行为同源于 `eff`（全局基线 + 方案折叠；overlay 方案取内置基线，
                // 默认关）。非法键在此告警一次并视为关闭，协调器经 `active_wildcard_key` 取用。
                wildcard: eff.wildcard_char(schema_id),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-engine --no-fail-fast`
Expected: 全绿。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/manager.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): build_engine 按方案折叠注入通配键

与 codetable_settings() 同一条折叠链（codetable_baseline + resolved），
overlay 方案不继承全局开关；非法键构建期告警一次。
EOF
git show --stat HEAD
```

---

### Task 10: 协调器按键裁决与冲突体检

> **控制者裁决（预检 #8/#9）**：①「通配键是方案真实码元」单列一条告警，文案表达「该码元将被通配吞掉」，**仅当方案显式配置 `input_chars` 且含该键**时报；非首位冲突告警的归属照 `code_char_conflicts` 的细分写法（同一键不重复报成「会话键」与「次选键」两条）。② `wildcard_pattern` 的定义与其单测**移到 Task 11**（本任务提交时它尚无调用者，会出 dead_code 警告）。

**Files:**
- Create: `wind_input/crates/wind-coordinator/src/wildcard.rs`
- Modify: `wind_input/crates/wind-coordinator/src/lib.rs`（模块表，按字母序插在 `pub(crate) mod preedit_cursor;` 附近：`pub(crate) mod wildcard;`）
- Modify: `wind_input/crates/wind-coordinator/src/handle_lifecycle.rs`（`fn is_any_mode_trigger` ~89 改 `pub(crate) fn`）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`try_code_char_gate` ~5244 符号通配入口；`build` ~2764 `warn_code_char_conflicts()` 之后调 `warn_wildcard_conflicts()`）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator/message_handler.rs`（字母臂 `keymap::VK_A..=keymap::VK_Z` ~1587，在 `if !self.can_enter_buffer(&state, ch)` ~1633 之前）
- Modify: `wind_input/crates/wind-coordinator/src/debug_support.rs`（追加 `debug_input_buffer`、`debug_candidate_triples`）
- Create: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`
- Test: `tests/codetable_wildcard.rs`

**Interfaces:**
- Consumes: `EngineManager::active_wildcard_key`、`EngineManager::active_input_chars`、`EngineManager::codetable_settings`；`Coordinator::{bound_action_for, session_action_for, select_key_offset, select_char_index, manual_separator_key, has_code_prefix, is_any_mode_trigger, accumulate_code_char}`；`crate::key_convert::{punct_char, punct_source_vk}`；`wind_dict::WILDCARD_SLOT`
- Produces:
  - `pub(crate) enum WildcardDecision { Yield, Enter }`
  - `pub(crate) fn wildcard_decision(&self, buffer: &str, key: char) -> WildcardDecision`
  - `pub(crate) fn wildcard_pattern(&self, buffer: &str) -> Option<String>`（Task 11 消费）
  - `pub fn wildcard_conflicts(&self) -> Vec<&'static str>`
  - `pub(crate) fn warn_wildcard_conflicts(&self)`
  - `pub fn debug_input_buffer(&self) -> String`、`pub fn debug_candidate_triples(&self) -> Vec<(String, String, String)>`

- [ ] **Step 1: 写失败测试**

新建 `tests/codetable_wildcard.rs`：

```rust
//! 码表通配输入（万能键）端到端验证（设计见 docs/design/codetable-wildcard.md §7）。
//!
//! 每条「开启通配」用例都配一条「关闭时同一操作」的对照——只测正向的话，
//! 通配整个没接线、或某键本就这么表现，用例一样会绿。
//!
//! ⚠️ `build_dev/data` 不存在时整族静默跳过而计数照绿，判据是耗时（正常 ≥1s）
//! 与输出里有没有「跳过」。

use std::path::PathBuf;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn dict_ready() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}

fn key_event(key_code: u32, modifiers: u32) -> KeyEventData {
    KeyEventData {
        key_code,
        scan_code: 0,
        modifiers,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

/// 字母键（VK = 大写 ASCII）。符号键用 [`press_vk`]。
fn press(coord: &Coordinator, s: &str) -> KeyAction {
    let mut last = KeyAction::Consumed;
    for c in s.chars() {
        debug_assert!(c.is_ascii_alphabetic(), "符号键的 VK 与字符不同，请用 press_vk");
        last = coord.handle_key_event(&key_event((c.to_ascii_uppercase() as u32) & 0xFF, 0));
    }
    last
}

fn press_vk(coord: &Coordinator, vk: u32, shift: bool) -> KeyAction {
    coord.handle_key_event(&key_event(vk, if shift { MOD_SHIFT } else { 0 }))
}

const VK_SLASH: u32 = 0xBF; // `/`，Shift 即 `?`
const VK_SEMICOLON: u32 = 0xBA; // `;`（出厂次选键）
const VK_SPACE: u32 = 0x20;

fn committed(a: &KeyAction) -> Option<&str> {
    match a {
        KeyAction::InsertText { text, .. } => Some(text.as_str()),
        _ => None,
    }
}

fn wubi(wildcard: bool, key: &str) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.codetable.wildcard = wildcard;
    cfg.schema.codetable.wildcard_key = key.into();
    cfg
}

/// 真机出厂 `zz*` 标点短语的最小替身（同 input_flow.rs 的 `zz_system_phrases`）。
fn zz_phrases() -> Vec<wind_phrase::PhraseSeed> {
    let seed = |code: &str, text: &str| wind_phrase::PhraseSeed {
        code: code.into(),
        text: text.into(),
        weight: 0,
        position: 0,
        is_system: true,
        category: String::new(),
    };
    vec![seed("zzbd", "、"), seed("zzsz", "…")]
}

// ─────────────────────────── 进缓冲裁决（Task 10） ───────────────────────────

/// 非首位字母通配键进缓冲，即使它不在 `input_chars`（`a-y`）里。
/// 对照：关闭时同一操作按非码元字母处置（顶屏高亮候选再出 `z`）。
#[test]
fn wildcard_letter_enters_buffer_outside_input_chars() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.codetable.input_chars = "a-y".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "az");
    assert_eq!(coord.debug_input_buffer(), "az");

    let mut off = wubi(false, "z");
    off.schema.codetable.input_chars = "a-y".into();
    let coord = Coordinator::new_headless(off, Some(&data_dir()));
    press(&coord, "a");
    let act = press(&coord, "z");
    assert!(committed(&act).is_some_and(|t| t.ends_with('z')), "对照：关闭时 z 是非码元，实际 {act:?}");
}

/// 26 码元方案配符号通配键：组码中 `?` 进缓冲，不走标点流水线。
/// 对照：关闭时 `?` 不进缓冲。
#[test]
fn symbol_wildcard_enters_buffer_instead_of_punct() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "?"), Some(&data_dir()));
    press(&coord, "a");
    press_vk(&coord, VK_SLASH, true);
    assert_eq!(coord.debug_input_buffer(), "a?");

    let coord = Coordinator::new_headless(wubi(false, "?"), Some(&data_dir()));
    press(&coord, "a");
    press_vk(&coord, VK_SLASH, true);
    assert_ne!(coord.debug_input_buffer(), "a?", "对照：关闭时 ? 不进缓冲");
}

/// 非首位通配键与次选键冲突 ⇒ 让位（`;` 照常选第 2 个候选）且体检报冲突。
/// 对照：通配键 `z` 在出厂键位下无冲突。
#[test]
fn mid_composition_conflict_yields_and_is_reported() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, ";"), Some(&data_dir()));
    assert!(
        coord.wildcard_conflicts().contains(&"次选键"),
        "应报次选键冲突：{:?}",
        coord.wildcard_conflicts()
    );
    press(&coord, "a");
    let second = coord
        .debug_all_candidate_texts()
        .get(1)
        .cloned()
        .expect("a 至少两个候选");
    let act = press_vk(&coord, VK_SEMICOLON, false);
    assert_eq!(committed(&act), Some(second.as_str()), "让位给次选键");

    let coord = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    assert!(coord.wildcard_conflicts().is_empty(), "对照：z 在出厂键位下无冲突");
}

/// 通配键是**显式配置**的方案码元时，体检报「方案码元」。
#[test]
fn wildcard_that_is_a_configured_code_char_is_reported() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "/");
    cfg.schema.codetable.input_chars = "a-z/".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    assert!(
        coord
            .wildcard_conflicts()
            .iter()
            .any(|o| o.starts_with("方案码元")),
        "{:?}",
        coord.wildcard_conflicts()
    );
}

/// 首位 z 已绑 `z_key_action` ⇒ 让位进模式，不作通配。
#[test]
fn leading_z_yields_to_z_key_action() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.available.push("pinyin".into());
    cfg.schema.codetable.z_key_action = "temp_pinyin".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "z");
    assert!(coord.debug_in_temp_pinyin(), "首位 z 应让位给 z_key_action");
}
```

`debug_support.rs` 的 `impl Coordinator` 内追加（它们是 Step 1 编译所需，也一并算在「写测试」里）：

```rust
    /// 当前输入缓冲（测试/诊断用）。通配用例要断言「键进没进缓冲」，组合区文本会混入已上屏前缀。
    pub fn debug_input_buffer(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .input_buffer
            .clone()
    }

    /// 全部候选的 `(text, code, comment)`（测试/诊断用）。通配要断言「注释是完整编码」。
    pub fn debug_candidate_triples(&self) -> Vec<(String, String, String)> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .candidates
            .iter()
            .map(|c| (c.text.clone(), c.code.clone(), c.comment.clone()))
            .collect()
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --test codetable_wildcard`
Expected: 编译失败，`no method named wildcard_conflicts found for struct Arc<Coordinator>`。

- [ ] **Step 3: 最小实现**

新建 `src/wildcard.rs`：

```rust
//! 码表通配输入（万能键）的按键裁决、组码判定与冲突体检。
//! 设计见 `docs/design/codetable-wildcard.md` §3.3–§3.4、§5.3。
//!
//! # 为什么「哪几位是通配」由这里定，而不是引擎按内容自判
//!
//! 缓冲本身不带标记：首位让位后进缓冲的字面 `z`（`zz*` 短语的活码）与作通配的 `z` 同形。
//! 故通配只在需要时按**同一套规则**从缓冲重算（[`Coordinator::wildcard_pattern`]），不记
//! 标志位——理由同 `buffer_has_literal_symbol`：缓冲会被退格、光标中插改写，来源标志迟早
//! 与内容对不上，而字符与规则不会。规则必须**与历史无关**（`z_key_repeat` 取配置开关而非
//! 「有没有上屏历史」），否则同一串缓冲前后两次重算会得出不同的 pattern。

use crate::coordinator::Coordinator;
use crate::key_convert::{punct_char, punct_source_vk};
use tracing::warn;
use wind_dict::WILDCARD_SLOT;
use wind_keys::keymap;

/// 通配键此刻的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WildcardDecision {
    /// 让位：按它原本的身份走（码元 / 模式 / 选词 / 翻页 / 标点），与关闭通配时逐键相同。
    Yield,
    /// 作通配进缓冲。
    Enter,
}

/// 通配键 → (主键盘 VK, 是否需要 Shift)。字母恒无 Shift（缓冲恒存小写）。
fn wildcard_key_vk(key: char) -> Option<(u32, bool)> {
    if key.is_ascii_lowercase() {
        return Some((keymap::VK_A + (key as u32 - 'a' as u32), false));
    }
    let vk = punct_source_vk(key)?;
    Some((vk, punct_char(vk, false) != Some(key)))
}

impl Coordinator {
    /// §3.3 按键裁决。`buffer` 是**按下前**的输入缓冲。
    ///
    /// ⚠️ 调用点必须晚于 `try_activate_mode` 与 `try_z_fallback`（顺序铁律）：首位让位本就是
    /// 它们先赢；这里的首位判据只处理「它们都没接手」之后的去向。
    pub(crate) fn wildcard_decision(&self, buffer: &str, key: char) -> WildcardDecision {
        let yield_ = if buffer.is_empty() {
            self.wildcard_lead_yields(key)
        } else {
            // 首字符是让位进来的字面通配键 ⇒ 整轮都是字面（否则 `zzbd` 变 `z?bd`，
            // 出厂 `zz*` 短语整批消失）。
            self.wildcard_lead_literal(buffer, key) || !self.wildcard_mid_owners(key).is_empty()
        };
        if yield_ {
            WildcardDecision::Yield
        } else {
            WildcardDecision::Enter
        }
    }

    /// 首位：通配键已绑定任何功能 / 模式即让位（spec §3.3 首位一行）。
    fn wildcard_lead_yields(&self, key: char) -> bool {
        let Some((vk, _)) = wildcard_key_vk(key) else {
            return true;
        };
        if self.bound_action_for(vk).is_some_and(|a| a.is_enabled()) {
            return true;
        }
        // 取**配置开关**而不是 `z_key_repeat_text()`（有无上屏历史）：本判据要被
        // `wildcard_pattern` 反复重算，随历史变化的判据会让同一串缓冲前后解释不一。
        if key == 'z' && self.engine_mgr.codetable_settings().z_key_repeat {
            return true;
        }
        if self.is_any_mode_trigger(vk) {
            return true;
        }
        self.has_code_prefix(&key.to_string())
    }

    /// 缓冲首字符是通配键，且首位按规则会让位 ⇒ 它是字面码元，整轮组码不作通配。
    fn wildcard_lead_literal(&self, buffer: &str, key: char) -> bool {
        buffer.starts_with(key) && self.wildcard_lead_yields(key)
    }

    /// 非首位：通配键若是组码中途的功能键，返回占用它的功能名（空 = 不冲突）。
    ///
    /// 判据反查现有函数，与 `code_char_conflicts` / `symbol_buffer_key_free` 同源——
    /// ⚠️ 那两处的 owners 链增减时这里要跟着改。
    fn wildcard_mid_owners(&self, key: char) -> Vec<&'static str> {
        let Some((vk, shift)) = wildcard_key_vk(key) else {
            return Vec::new();
        };
        let mut owners: Vec<&'static str> = Vec::new();
        if let Some(a) = self.session_action_for(vk, shift, true) {
            owners.push(match a {
                wind_config::SessionAction::AuxCode(_) => "辅助码引导键",
                _ => "翻页/高亮/上屏类会话键",
            });
        }
        if !shift && self.select_key_offset(vk).is_some() {
            owners.push("次选键");
        }
        if !shift && self.select_char_index(vk).is_some() {
            owners.push("以词定字键");
        }
        if self.manual_separator_key(vk) {
            owners.push("音节分隔符");
        }
        owners
    }

    /// 当前缓冲若是通配组码，返回交给引擎的 pattern（作通配的位替换成 `WILDCARD_SLOT`）。
    ///
    /// 与 [`Self::wildcard_decision`] 同一套规则：首位字面 ⇒ 整轮字面；非首位存在冲突 ⇒
    /// 非首位的通配键一律字面（冲突时它们本就是经让位进来的，见 spec §3.3 非首位一行）。
    pub(crate) fn wildcard_pattern(&self, buffer: &str) -> Option<String> {
        let key = self.engine_mgr.active_wildcard_key()?;
        if !buffer.contains(key) || self.wildcard_lead_literal(buffer, key) {
            return None;
        }
        let mid_ok = self.wildcard_mid_owners(key).is_empty();
        let mut any = false;
        let pattern: String = buffer
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if c == key && (i == 0 || mid_ok) {
                    any = true;
                    WILDCARD_SLOT
                } else {
                    c
                }
            })
            .collect();
        any.then_some(pattern)
    }

    /// 通配键与既有按键功能的冲突清单（只告警）。空 = 无冲突或通配关闭。
    ///
    /// 首位让位**不报**（那是设计内行为）；只报非首位让位（意味着通配在组码中实际不可用）
    /// 与「通配键是显式配置的方案码元」（该码元将被通配吞掉）。
    ///
    /// 码元一项只在 `input_chars` **显式配置**时报：五笔 86 未配 `input_chars`，默认 `a-z`
    /// 含 `z`，但词库里 `z` 开头 / 含 `z` 的码为 0——按默认集报会让出厂主用例每次启动都告警。
    pub fn wildcard_conflicts(&self) -> Vec<&'static str> {
        let Some(key) = self.engine_mgr.active_wildcard_key() else {
            return Vec::new();
        };
        let mut owners = self.wildcard_mid_owners(key);
        let charset = self.engine_mgr.active_input_chars();
        if !charset.is_default_alpha() && charset.contains(key) {
            owners.push("方案码元（组码中将被通配吞掉）");
        }
        owners
    }

    /// 启动时把 [`Self::wildcard_conflicts`] 写进日志。只告警，不改行为。
    pub(crate) fn warn_wildcard_conflicts(&self) {
        let Some(key) = self.engine_mgr.active_wildcard_key() else {
            return;
        };
        let owners = self.wildcard_conflicts();
        if !owners.is_empty() {
            warn!(
                "通配键 {:?} 同时是 {}；组码中它按原功能处理，通配在该方案下不可用——请在方案设置里换一个通配键",
                key,
                owners.join(" / ")
            );
        }
    }
}
```

> 核对点：
> - `BoundAction::is_enabled` 以 `wind-config` 现有签名为准（`handle_lifecycle.rs:397` 的 `z.is_enabled()` 即此方法）。
> - `CodeCharSet::contains` / `is_default_alpha` 以 `code_charset.rs` 现有签名为准（`code_char_conflicts` 已在用）。
> - 「方案码元」这条同时满足 `wildcard_decision` 不因它让位（spec「照样按通配处理」），只告警。

`handle_lifecycle.rs`：`fn is_any_mode_trigger(&self, key_code: u32) -> bool {` 改为 `pub(crate) fn is_any_mode_trigger(…)`。

`coordinator.rs` — `try_code_char_gate` 在 `let lower = ch.to_ascii_lowercase();` 之后、`if !self.can_enter_buffer(state, lower)` 之前插入：

```rust
        // 符号通配键（spec §3.4）：在标点流水线之前截住，接线点与码元闸门同处。
        // 让位时落回下面的码元判定，与关闭通配时逐键相同。
        if self.engine_mgr.active_wildcard_key() == Some(lower)
            && self.wildcard_decision(&state.input_buffer, lower)
                == crate::wildcard::WildcardDecision::Enter
        {
            return Some(self.accumulate_code_char(state, lower, ch));
        }
```

`coordinator.rs` — `build` 里 `coordinator.warn_code_char_conflicts();` 之后加：

```rust
        // 通配键与既有按键功能的冲突体检（只告警）。通配关闭时直接返回。
        coordinator.warn_wildcard_conflicts();
```

`message_handler.rs` — 字母臂里 `let raw = …;` 之后、`if !self.can_enter_buffer(&state, ch) {` 之前插入：

```rust
                // 字母通配键（spec §3.3）：本处已晚于 `try_activate_mode` 与 z 夺取（顺序铁律）。
                // 放在码元判定之前：五笔配 `a-y` 时 `z` 不是码元，但组码中仍要能作通配。
                // 让位时落回下面的码元判定，与关闭通配时逐键相同。
                if self.engine_mgr.active_wildcard_key() == Some(ch)
                    && self.wildcard_decision(&state.input_buffer, ch)
                        == crate::wildcard::WildcardDecision::Enter
                {
                    return self.accumulate_code_char(&mut state, ch, raw);
                }
```

`lib.rs` 模块表加 `pub(crate) mod wildcard;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --test codetable_wildcard -- --nocapture`
Expected: 5 passed；输出里**没有**「跳过：五笔词库不存在」，测试段耗时 ≥ 1s。若出现「跳过」，先按 AGENTS.md 备好 `build_dev/data` 再判定（worktree 下 `build_dev/data` 要么全拷要么别放）。

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --test input_flow --test codetable_input_chars`
Expected: 全绿、零断言修改（出厂 `wildcard = false` 零回归锁）。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
git add -- wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
rustfmt --edition 2024 wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/src/lib.rs wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/coordinator/message_handler.rs wind_input/crates/wind-coordinator/src/debug_support.rs
git diff --cached --name-status   # 只应有上面两个新文件（A）
git commit -F - --only -- wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs wind_input/crates/wind-coordinator/src/lib.rs wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/coordinator/message_handler.rs wind_input/crates/wind-coordinator/src/debug_support.rs <<'EOF'
feat(coordinator): 通配键按键裁决与冲突体检

首位已绑定功能/模式/活码即让位；首位让位进来的字面通配键使整轮组码不作通配；
非首位遇选词/翻页/分隔/辅助码键让位并启动告警。符号在标点流水线前截住，
字母在码元判定前接手（a-y 方案的 z 组码中仍可通配）。
EOF
git show --stat HEAD
```

---

### Task 11: 协调器候选管线（分流、短路、记账）

> **控制者裁决（预检 #2/#6/#7/#9）**：① 本任务定义 `wildcard_pattern`（及其单测，自 Task 10 移入）。② `build_candidates` 排序后的**显示层去重**（`handle_candidate.rs` ~1502-1514 按 text）在有通配位时改为按 `(text, code)` 去重，并补集成用例：同字不同码（如简码与全码）在通配结果里两条都在。③ 通配记账**不改 `freq_code` 本身**，只在主输入路的上屏记账点对通配组码用 `c.code`；mix / 临拼缓冲与读侧调频不动。④ 有通配位时跳过 `short_code_yield`（出简让全）；shadow 与读侧调频保持现状。

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/handle_candidate.rs`（`build_candidates` ~1162：`let result = match pinyin_schema` ~1203 分流；短语块条件 ~1262；自动上屏复评 ~1717；清空复核 ~1766；短语自动上屏 ~1778；`freq_code` ~3350）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`accumulate_code_char` ~5072 的 `let top_code = …` ~5110；`char_is_literal_symbol` ~5364）
- Create: `wind_input/crates/wind-coordinator/src/wildcard.rs` 内 `#[cfg(test)] mod tests`（记账码单测）
- Test: `tests/codetable_wildcard.rs` 追加；`src/wildcard.rs` 单测

**Interfaces:**
- Consumes: Task 10 `wildcard_pattern`；Task 7 `EngineManager::convert_wildcard`
- Produces: 通配组码下 `state.candidates` 来自 `convert_wildcard`；`InputOutcome` 恒 `Normal`；顶码不触发；`freq_code` 对码表候选返回 `c.code`

- [ ] **Step 1: 写失败测试**

`tests/codetable_wildcard.rs` 追加：

```rust
// ─────────────────────────── 候选管线（Task 11） ───────────────────────────

/// 默认关闭 ⇒ 行为不变（`az` 无候选）；开启 ⇒ `az` 出候选、注释为全码、等长在前。
#[test]
fn wildcard_off_by_default_changes_nothing() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let off = Coordinator::new_headless(wubi(false, "z"), Some(&data_dir()));
    press(&off, "az");
    assert!(off.debug_all_candidate_texts().is_empty(), "对照：关闭时 az 是空码");

    let on = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    press(&on, "az");
    let tri = on.debug_candidate_triples();
    assert!(!tri.is_empty(), "开启后 az 应出候选");
    for (text, code, comment) in &tri {
        assert!(code.starts_with('a') && code.chars().count() >= 2, "{text} 的码 {code} 不匹配 a?");
        assert_eq!(comment, code, "{text} 的注释应是完整编码");
    }
    assert_eq!(tri[0].1.chars().count(), 2, "等长档在前");
}

/// 多通配：`azzd` 只出第 1 位 a、第 4 位 d 的 4 码。
#[test]
fn multiple_wildcards_match_exactly_one_code_char_each() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    press(&coord, "azzd");
    let tri = coord.debug_candidate_triples();
    assert!(!tri.is_empty(), "azzd 应有候选");
    for (text, code, _) in tri.iter().filter(|(_, c, _)| c.chars().count() == 4) {
        let cs: Vec<char> = code.chars().collect();
        assert!(cs[0] == 'a' && cs[3] == 'd', "{text} 的码 {code} 不匹配 a??d");
    }
}

/// 首位无任何绑定时首位即可通配（退化全表扫描，靠上限兜底）。
#[test]
fn leading_wildcard_when_unbound_scans_whole_table() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    press(&coord, "z");
    let tri = coord.debug_candidate_triples();
    assert!(!tri.is_empty() && tri.len() <= 100, "首位通配应出候选且不超上限：{}", tri.len());
    assert!(tri.iter().all(|(_, c, m)| c == m));
}

/// ★ Review Focus 1：首位 z 让位给 `zz*` 短语后，整轮都是字面——`zzbd` 仍出「、」。
/// 对照：同配置下 `az` 仍是通配（证明通配确实开着）。
#[test]
fn zz_phrases_survive_when_wildcard_on() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    coord.debug_install_phrases(zz_phrases());
    press(&coord, "zzbd");
    assert_eq!(
        coord.debug_all_candidate_texts().first().map(String::as_str),
        Some("、"),
        "zzbd 短语应照常命中"
    );
    press_vk(&coord, 0x1B, false); // Esc 清空
    press(&coord, "az");
    assert!(!coord.debug_candidate_triples().is_empty(), "对照：az 仍是通配");
}

/// 首位 z 让位给 `z_key_repeat`：首选是上一次上屏内容，不是通配结果。
#[test]
fn leading_z_yields_to_z_key_repeat() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.codetable.z_key_repeat = true;
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "a");
    let first = committed(&press_vk(&coord, VK_SPACE, false))
        .expect("前置：空格上屏")
        .to_string();
    press(&coord, "z");
    assert_eq!(
        coord.debug_all_candidate_texts().first(),
        Some(&first),
        "首位 z 应让位给重复上屏"
    );
}

/// ★ Review Focus 2：开着通配，`z_key_action = temp_pinyin` 的 z 夺取仍然成立
/// （`has_code_prefix("zh")` 走字面 convert，不会被当成 `?h` 判活）。
#[test]
fn z_fallback_still_hijacks_when_wildcard_on() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.available.push("pinyin".into());
    cfg.schema.codetable.z_key_action = "temp_pinyin".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    coord.debug_install_phrases(zz_phrases()); // 首键 z 让位（活码），靠夺取进临拼
    press(&coord, "z");
    assert!(!coord.debug_in_temp_pinyin(), "前置：首键 z 让位");
    press(&coord, "h");
    assert!(coord.debug_in_temp_pinyin(), "zh 破活码前缀应被夺取进临拼");
}

/// §3.2：通配下超码长不顶字。对照：关闭时 `aaaa` + `a` 照常顶字。
#[test]
fn wildcard_never_top_commits() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut on = wubi(true, "z");
    on.schema.codetable.top_code_commit = true;
    let coord = Coordinator::new_headless(on, Some(&data_dir()));
    let act = press(&coord, "aaaza");
    assert!(committed(&act).is_none(), "通配组码不顶字，实际 {act:?}");
    assert_eq!(coord.debug_input_buffer(), "aaaza");

    let mut off = wubi(false, "z");
    off.schema.codetable.top_code_commit = true;
    let coord = Coordinator::new_headless(off, Some(&data_dir()));
    let act = press(&coord, "aaaaa");
    assert!(committed(&act).is_some(), "对照：字面超码长照常顶字");
}

/// §3.2：通配下满码唯一也不自动上屏（结果多于一条时本就不会上屏，故此处断言的是
/// 「最后一键没有上屏」这一弱形态；强形态见引擎单测 `wildcard_never_auto_commits`）。
#[test]
fn wildcard_full_length_does_not_auto_commit() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.codetable.auto_commit_at_full = true;
    cfg.schema.codetable.clear_on_empty_max = true;
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    let act = press(&coord, "aaaz");
    assert!(committed(&act).is_none(), "通配满码不自动上屏，实际 {act:?}");
    assert_eq!(coord.debug_input_buffer(), "aaaz", "也不清空");
}

/// ★ Review Focus 4：符号通配键同时在 `input.buffer_symbol_chars` 里，仍按通配查候选。
#[test]
fn symbol_wildcard_listed_in_buffer_symbol_chars_still_queries() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "?");
    cfg.input.buffer_symbol_chars = "-?".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "a");
    press_vk(&coord, VK_SLASH, true);
    assert_eq!(coord.debug_input_buffer(), "a?");
    assert!(
        !coord.debug_candidate_triples().is_empty(),
        "通配键不是「字面符号」，不得清空候选"
    );
}
```

`src/wildcard.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use wind_candidate::{Candidate, CandidateSource};

    use crate::coordinator::Coordinator;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
    }

    /// ★ Review Focus 3：通配组码下码表候选以**候选全码**记账，不以 `azzz` 这种通配串记账
    /// （读端永远查不中的孤儿键）。对照：非通配组码沿用缓冲（既有口径）。
    #[test]
    fn freq_code_under_wildcard_is_candidate_full_code() {
        let d = data_dir();
        if !d.join("schemas/wubi86/wubi86_jidian.dict.yaml").exists() {
            eprintln!("跳过：五笔词库不存在");
            return;
        }
        let mut cfg = wind_config::Config::default();
        cfg.schema.available = vec!["wubi86".into()];
        cfg.schema.active = "wubi86".into();
        cfg.schema.codetable.wildcard = true;
        let coord = Coordinator::new_headless(cfg, Some(&d));
        let cand = Candidate {
            text: "工".into(),
            code: "aaaa".into(),
            source: CandidateSource::CodeTable,
            ..Default::default()
        };
        assert_eq!(coord.freq_code("azzz", &cand), "aaaa");
        assert_eq!(coord.freq_code("aaa", &cand), "aaa", "对照：非通配沿用缓冲");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --test codetable_wildcard; CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --lib wildcard::tests`
Expected: FAIL 的有 `wildcard_off_by_default_changes_nothing`（开启组 `az` 无候选）、`multiple_wildcards_*`、`leading_wildcard_*`、`zz_phrases_survive_*`（对照组 `az` 无候选）、`wildcard_full_length_does_not_auto_commit`（`aaaz` 被满码空码清空）、`symbol_wildcard_listed_*`、`freq_code_under_wildcard_*`；`leading_z_yields_to_z_key_repeat` 与 `z_fallback_still_hijacks_*` 与 `wildcard_never_top_commits`（实现前 `aaaz` 无候选、顶码自然放弃）此时已绿（它们是护栏：锁住 Task 11 不得改坏）；Task 10 的五条仍绿。

- [ ] **Step 3: 最小实现**

`handle_candidate.rs` — `build_candidates`，把 `let result = match pinyin_schema { … };` 整段替换为：

```rust
        // 通配组码（spec §5.3）：只查码表，不走短语 / 整句 / 逆切分 / 混输拼音 / 英文混入。
        // pattern 由协调器按 §3.3 从缓冲重算（首位让位的字面通配键不在其中）。
        let wildcard_pattern = self.wildcard_pattern(&state.input_buffer);
        let result = if let Some(p) = &wildcard_pattern {
            self.engine_mgr
                .convert_wildcard(&state.input_buffer, p, limit)
                .unwrap_or_default()
        } else {
            match pinyin_schema {
                Some(ps) if self.engine_mgr.ensure_schema(&ps) => {
                    self.engine_mgr
                        .convert_with(&ps, &state.input_buffer, limit)
                }
                _ => self.engine_mgr.convert(&state.input_buffer, limit),
            }
        };
```

短语块条件 `if !phrases.is_empty() && !phrase_scope.is_closed() {` 改为：

```rust
        if wildcard_pattern.is_none() && !phrases.is_empty() && !phrase_scope.is_closed() {
```

自动上屏复评：

```rust
        // 通配结果不自动上屏（spec §3.2）：引擎意向本就为空，这里连复评一起跳过。
        let auto_commit = if wildcard_pattern.is_some() {
            None
        } else {
            auto_commit.or_else(|| {
                self.engine_mgr
                    .recheck_auto_commit(&state.input_buffer, &state.candidates)
            })
        };
```

清空复核臂 `None if should_clear` 改为 `None if should_clear && wildcard_pattern.is_none()`（后续 `&& !clear_blocked_by_candidates(…)` 保留）。

短语自动上屏：

```rust
        let outcome = match outcome {
            InputOutcome::Normal if wildcard_pattern.is_none() => self
                .phrase_auto_commit(state)
                .unwrap_or(InputOutcome::Normal),
            other => other,
        };
```

`freq_code`：

```rust
    pub(crate) fn freq_code(&self, buf: &str, cand: &Candidate) -> String {
        // 通配组码：缓冲是 `azzd` 这种查询串，不是任何词条的码位——按它记账会写出读端永远
        // 查不中的孤儿键。改记候选自己的全码（该字在正常输入时所在的码位）。
        if cand.source == CandidateSource::CodeTable
            && !cand.code.is_empty()
            && self.wildcard_pattern(buf).is_some()
        {
            return cand.code.clone();
        }
        Self::freq_code_with(
            buf,
            cand,
            self.engine_mgr.freq_settings().english_code_by_input,
        )
    }
```

`coordinator.rs` — `accumulate_code_char` 里 `let top_code = self.engine_mgr.handle_top_code(…)…;` 整个链改为包一层：

```rust
        // 通配组码不顶字（spec §3.2）。引擎的 `handle_top_code` 是字面语义（`azzda` 在它看来
        // 是「超码长 + 无匹配 + 无后继」的典型溢出），故短路必须落在这里。
        let top_code = if self.wildcard_pattern(&state.input_buffer).is_some() {
            None
        } else {
            self.engine_mgr
                .handle_top_code(&state.input_buffer)
                // ……原链上的三个 `.filter` 与一个 `.map` 原样保留……
        };
```

`coordinator.rs` — `char_is_literal_symbol` 开头加：

```rust
        // 通配键进缓冲走的是通配闸门，不是本闸门；它是查询的一部分，不是字面符号。
        if self.engine_mgr.active_wildcard_key() == Some(c) {
            return false;
        }
```

> 核对点：`build_candidates` 里 `auto_commit` 原本是 `let auto_commit = if result.should_commit …`（~1237），复评处是同名遮蔽（~1717）；按上面写法保持遮蔽即可。改完 `grep -n "wildcard_pattern" handle_candidate.rs` 应命中 5 处（定义 1 + 使用 4）。若 `state.candidates[i].comment` 在 `build_candidates` 的注释分层（`comment.rs`）里被改写导致 `wildcard_off_by_default_changes_nothing` 的注释断言失败，核对 `comment.rs:1084` 一带 `show_code_hint` 门控：通配候选的 comment 是引擎给的「编码提示」，应走 `${code_hint}`，不应被门控剥掉。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --test codetable_wildcard -- --nocapture && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --lib wildcard::tests -- --nocapture`
Expected: 集成 14 passed，单测 1 passed；输出无「跳过」，集成耗时 ≥ 1s。

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-coordinator --no-fail-fast`
Expected: 全绿（`every_record_selection_call_goes_through_freq_code` 等守门不受影响；`input_flow` 零断言修改）。

- [ ] **Step 5: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
rustfmt --edition 2024 wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/coordinator.rs
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
feat(coordinator): 通配组码走 convert_wildcard，上屏三短路与记账

短语/整句/混输拼音/英文混入不参与；自动上屏复评、满码清空复核、短语自动上屏、
顶字全部跳过；码表候选以全码记账而非通配串；通配键不算字面符号。
P3 完成：协调器集成用例覆盖 spec §7 全部条目。
EOF
git show --stat HEAD
```

---

## P4 — 设置端与文档

### Task 12: wind-setting 两项设置 + 检入产物

> **控制者裁决（预检 #11）**：manifest 注释不得写「不与出厂键位冲突」（反引号是出厂临拼引导键），改为如实说明各选项与出厂键位的关系；方案对话框遇选项外值回显的问题与 `z_key_action` 现状相同，本计划不修。

**Files:**
- Modify: `../wind-setting/src/assets/settings_manifest.toml`（`schema.codetable.z_key_action` 条目 ~167-180 之后）
- Modify: `../wind-setting/src/dialogs/schema_codetable.rs`（`SPEC_BEHAVIOR_FIELDS` ~87-108）
- Modify（生成）: `../wind-setting/src/assets/pinyin_initials.txt`、`../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json`
- Test: 邻仓 `manifest_keys_match_spec_fields`、`search::pinyin::tests::table_covers_every_title_char`、`uncovered_capability_keys_match_allowlist`；core 侧 `cargo test -p wind-rpc --test wind_setting_assets`

**Interfaces:**
- Consumes: Task 8 注册表键 `schema.codetable.wildcard` / `schema.codetable.wildcard_key`
- Produces: 设置页「上屏行为 / 常用功能」两项；方案级对话框同步覆盖

> 注：`../wind-setting` 没有 AGENTS.md，构建 / 测试命令以本仓 AGENTS.md「邻仓 wind-setting：交叉 + wine」一节为准（`dev.sh sk / st / sg`，在 WindInput 仓根执行）。方案对话框的 `make_sig`（`schema_codetable.rs:631`）只支持 `toggle / number / select / number_select`，故通配键用 `select`。

- [ ] **Step 1: 写失败测试（先让守门变红）**

在 `settings_manifest.toml` 的 `z_key_action` 条目之后加：

```toml
# 通配输入（万能键）。首位 / 非首位的让位规则由 core 裁决（wind-coordinator/src/wildcard.rs），
# 键的合法性（单个字母或 ASCII 符号，数字与空格不可用）也由 core 校验，非法值告警并视为关闭。
[[items]]
key = "schema.codetable.wildcard"
group = "schema"
section = "上屏行为"
subsection = "常用功能"
type = "toggle"
label = "通配输入"
hint = "组码时按通配键代替一个拿不准的码元，候选旁显示完整编码；通配结果不自动上屏、不顶字。首键若已有功能（z 键引导、z 键重复、活码）则让位；组码中若是选词、翻页、分隔键也让位"

# 下拉只列不与出厂键位冲突的三个；配了其它字符的用户由下拉的「保留当前配置」项兜住。
[[items]]
key = "schema.codetable.wildcard_key"
group = "schema"
section = "上屏行为"
subsection = "常用功能"
type = "select"
label = "通配键"
hint = "26 码元的方案请选符号键；选了本方案真实用到的码元，该码元会被通配吞掉"
enabled_when = "schema.codetable.wildcard == true"
options = [
  { value = "z", label = "z" },
  { value = "`", label = "` 反引号" },
  { value = "?", label = "? 问号" },
]
```

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput && ./scripts/dev.sh st`
Expected: FAIL —— `manifest_keys_match_spec_fields`（清单比 `SPEC_BEHAVIOR_FIELDS` 多两项）与 `table_covers_every_title_char`（label 里的「通」不在 `pinyin_initials.txt`）。

- [ ] **Step 2: 补 `SPEC_BEHAVIOR_FIELDS`**

在 `"single_char",` 之后加：

```rust
    // 通配输入（万能键）。按方案覆盖：26 码元方案与五笔的合适通配键不同，
    // 开不开也取决于这张码表的编码结构。
    "wildcard",
    "wildcard_key",
```

- [ ] **Step 3: 重算拼音首字母表（补「通」）**

表头写明「请勿手改」，按项目记忆的等价做法重算（本机无 pwsh）：

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/wind-setting
. ../WindInput/scripts/lib/xwin-env.sh && setup_xwin_env
cargo xwin test --target x86_64-pc-windows-msvc dump_title_chars -- --ignored --nocapture 2>&1 | grep -A1 '<<<TITLE_CHARS>>>' | tail -1 > /tmp/wct-wcard/title_chars.txt
```

再按 `../WindInput/build_dev/data/pinyin_map.txt`（`U+XXXX: pīn,yīn` 格式）每字取前两读的首字母（零声母带调首字母归一 `ā→a` 等）、按字符码位排序写表，表头三行、第三行 `# 字数: N`。
Expected diff（只应有这两处）：在 `透 t` 之后插入 `通 t`（U+901A，读音 `tōng,tòng` ⇒ `t`），`# 字数: 334` → `# 字数: 335`。多出别的行说明语料或归一化走偏，重来。

- [ ] **Step 4: 重生成检入产物并跑邻仓测试**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
./scripts/dev.sh sg
cd ../wind-setting
# sg 会顺带把 appVersion 改成 docs/VERSION 的值——与本改动无关，还原它
for f in src/assets/capabilities.snapshot.json src/mockdata/config.json; do
  old=$(git show HEAD:"$f" | grep -m1 '"appVersion"' || true)
  new=$(grep -m1 '"appVersion"' "$f" || true)
  if [ -n "$old" ] && [ "$old" != "$new" ]; then sed -i "s|$new|$old|" "$f"; fi
done
git diff --stat
cd ../WindInput && ./scripts/dev.sh st
cd wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test -p wind-rpc --test wind_setting_assets
```

Expected: `git diff --stat` 只列 5 个文件（manifest、schema_codetable.rs、pinyin_initials.txt、snapshot、mockdata）；`st` 全绿；`wind_setting_assets` 三条（Task 8 记下的）转绿。

- [ ] **Step 5: 提交（wind-setting 仓）**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/wind-setting
git diff --cached --name-status
git commit -F - --only -- src/assets/settings_manifest.toml src/dialogs/schema_codetable.rs src/assets/pinyin_initials.txt src/assets/capabilities.snapshot.json src/mockdata/config.json <<'EOF'
feat(settings): 码表通配输入两项（开关 + 通配键）

方案级对话框同步覆盖；拼音检索表补「通」；capability 快照与 mockdata 重生成。
EOF
git show --stat HEAD
```

---

### Task 13: 架构文档、spec 回写、文档站

**Files:**
- Modify: `docs/architecture/engine-candidate-pipeline.md`（§3.3 ~170 之后新增 §3.4；§12 源文件索引补 `wildcard.rs`）
- Modify: `docs/design/codetable-wildcard.md`（头部状态行；新偏离追加 §9）
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`（「常用功能」表 ~61-69 加一行；`### z 键引导功能` 小节之后新增小节）

**Interfaces:** 无代码接口。

- [ ] **Step 1: 写 `engine-candidate-pipeline.md` §3.4**

```markdown
### 3.4 通配输入（`convert_wildcard`）

- **入口分离**：`convert` 恒为字面语义（它另被 `has_code_prefix` / z 夺取当活码探针用）；
  通配走 `Engine::convert_wildcard(input, pattern, max)`，`pattern` 中作通配的位是
  `wind_dict::WILDCARD_SLOT`。哪几位作通配由协调器 `wildcard_pattern` 按
  `docs/design/codetable-wildcard.md` §3.3 从缓冲重算：首位让位进来的字面通配键使整轮不作通配。
- **词库层**：`DictLayer::search_pattern`。DAT 逐位推进前沿（通配位遍历全部有效转移），
  等长档取满后余额交给多起点分支限界（`bnb_collect`）；BTreeMap / redb 按首个通配前的字面
  前缀扫描后过滤；草稿层不参与；Composite 按 `(text, code)` 去重、`cmp_pattern` 等长优先。
- **引擎**：`is_exact_code` 改判等长，`comment = code`，`should_commit` / `should_clear` 恒 false，
  上限 `WILDCARD_RESULT_LIMIT = 100`。混输只代理主码表。
- **协调器**：`build_candidates` 通配时不查短语、跳过自动上屏复评 / 清空复核 / 短语自动上屏；
  `accumulate_code_char` 不顶字；`freq_code` 对码表候选改记 `c.code`；通配键不算字面符号。
```

- [ ] **Step 2: 回写 spec**

`docs/design/codetable-wildcard.md` 的 §5.2 / §5.3 与规则修正已于 2026-09-29 回写（见其 §9）；此步只把头部状态行改为 `> **状态：P1–P4 已实施**（提交见 git log --grep 通配）。`，若实施中又有新偏离，追加到 §9。

- [ ] **Step 3: 文档站**

`codetable.mdx`「常用功能」表在 `z 键引导功能` 行之后加：

```mdx
| 通配输入 <Since v="0.124" /> | 组码时按通配键代替一个拿不准的码元，见下 | 关 |
```

`### z 键引导功能 [#z-key-action]` 整节之后加：

```mdx
### 通配输入 <Since v="0.124" /> [#wildcard]

拆字拿不准某一码时，用通配键（出厂 `z`）顶替它：打 `azzd` 会列出第 1 码是 `a`、第 4 码是 `d` 的全部 4 码字词，候选旁显示**完整编码**，顺手就把正确的码学会了。

- 一个通配键恰好代替一个码元，可以用多个；码长不够时会顺带列出以它开头的更长编码，完整匹配的排在前面。
- 通配结果**不会**自动上屏、不会顶字，也不会因为没有匹配而被清空——用空格或选词键自己挑。
- 短语、整句、拼音混输不参与通配。

<Callout type="warn" title="通配键遇到别的功能时会让位">
- **打第一个码时**：通配键若已经有别的用途（z 键引导功能、z 键重复输入、本方案里 `z` 开头的编码或短语，如出厂的 `zz` 标点短语），按它原来的用途处理。此时这一整串都不作通配。
- **组码中间**：通配键若同时是选词、翻页、音节分隔或辅助码键，按原功能处理，通配在该方案下不可用（启动日志会提示换键）。
</Callout>

26 个字母都参与编码的方案，请在「通配键」里选反引号或问号。
```

> 版本号：`<Since>` 取 `WindInputDocs/data/releases.json` 最新版本的下一个小版本（写本计划时最新为 0.123，故 `0.124`）；实施时若已发过 0.124 就顺延。

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInputDocs && pnpm install --frozen-lockfile && pnpm lint`
Expected: 通过（`check-mdx` 不报 Since 锚点与未发布标注位置）。

- [ ] **Step 4: 提交（两个仓各自提交）**

```bash
cd /home/dufeng/develop/windinput/wt-wildcard/WindInput
git diff --cached --name-status
git commit -F - --only -- docs/architecture/engine-candidate-pipeline.md docs/design/codetable-wildcard.md <<'EOF'
docs(design): 码表通配输入落地——架构文档 §3.4 与设计稿回写偏离

convert 保持字面、通配走显式 convert_wildcard；首位字面通配键使整轮不作通配；
上屏三短路落在协调器。
EOF
cd ../WindInputDocs
git diff --cached --name-status
git commit -F - --only -- content/docs/settings/schema/codetable.mdx <<'EOF'
docs(settings): 码表方案新增「通配输入」说明
EOF
```

---

### Task 14: 全量回归与编译门

**Files:** 无改动。

- [ ] **Step 1: 全量测试**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcard TMPDIR=/tmp/wct-wcard cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-wcard/full.log | grep -E "^test result|FAILED|panicked" | tail -60`
Expected: 无 FAILED；按 `test result` 行求和，与开工前同命令的基线相比只多出本计划新增的用例数（本 worktree 已拷全 `build_dev/data`，基线取开工前实测值，见账本 Baseline 行）。`codetable_wildcard` 那一行耗时 ≥ 1s。

- [ ] **Step 2: 编译门**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput/wind_input && cargo check-headless && cd .. && ./scripts/dev.sh k`
Expected: 均通过（新增代码零平台依赖；`wildcard.rs` 不引用 wind-ui）。

- [ ] **Step 3: 占位与格式自查**

Run: `cd /home/dufeng/develop/windinput/wt-wildcard/WindInput && git diff HEAD~12 --stat && git diff HEAD~12 | grep -nE "TODO|todo!|unimplemented!|\.only\(|#\[ignore" || echo clean`
Expected: `clean`（`HEAD~12` 以实际提交数为准，范围只含本计划的提交）；`rustfmt --edition 2024 --check` 对本计划新建的两个文件无输出。

---

## 与 spec 初稿的偏离（已于 2026-09-29 回写 spec §9，此处留作溯源）

1. **引擎不按内容自判通配**（spec §5.2）。`has_code_prefix`（`handle_temp.rs:71`）与 `try_z_fallback`（`handle_temp.rs:~218`）把 `engine_mgr.convert` 当活码探针；按内容自判会让 `zh` 变 `?h` 判活、z 夺取失效。改为显式 `Engine::convert_wildcard`，`convert` 恒字面。
2. **上屏三短路落在协调器**（spec §5.2 写在引擎）。引擎看不见「首位字面」这一信息，按内容短路 `handle_top_code` 会误伤 `zz*` 短语的顶码（`accumulate_code_char` 的短语切点修正路径）。改为 `build_candidates` / `accumulate_code_char` 以 `wildcard_pattern` 为判据短路；`convert_wildcard` 自身恒不给上屏 / 清空意向。
3. **首位字面 ⇒ 整轮字面**（spec §3.3 未写）。否则与 spec §7 自己的用例「首位 z 让位给 `zz*` 短语」矛盾。
4. **`wildcard_decision` 签名** 由 `(key, buffer_empty)` 改为 `(buffer, key)`，理由同 3。
5. **Composite 去重键与排序**：`merge_search`（`composite.rs:132-231`）现按 text 去重、按 `better` 排序截断；Pattern 改为 `(text, code)` 去重 + `cmp_pattern`，各层两档取额。
6. **redb 不沿用「limit 先截断」**（spec §5.1）：先截断再过滤会整批落空，改为字面前缀全扫后过滤。
7. **键合法性另排除数字与空格**；**「真实码元」告警只在 `input_chars` 显式配置时报**。
8. **记账码、字面符号**两处 spec 未涉及，见 Review Focus 3、4。
9. **注释不受 `show_code_hint` 门控**（`engine.rs:824-833` 的既有注释受门控）：spec 称注释是功能的学习价值所在，故通配恒给。

## 自查

- **spec 覆盖**：§3.1 匹配语义 → Task 1/2/3/4/5（一通配一码元、多通配、等长优先 + 补全、上限 100、参与 / 不参与层）+ Task 6/7（整句 / 逆切分 / 混输 / 英文混入不参与）+ Task 11（短语不参与）。§3.2 上屏行为 → Task 6（引擎不给意向）+ Task 11（复评 / 清空 / 顶字 / 短语自动上屏短路、注释全码）。§3.3 按键裁决 → Task 10（首位 / 非首位 / 顺序铁律 / 冲突告警 / 真实码元告警）+ Task 11（首位字面整轮字面）。§3.4 进缓冲闸门 → Task 10（`try_code_char_gate` 符号、字母臂 `can_enter_buffer` 前）。§4 配置 → Task 8（五处同步 + 非法视为关闭）+ Task 9（`build_engine` 注入、两折叠点同判据）+ Task 12（wind-setting 四处）。§5 实现 → Task 1-11。§6 分期 → P1-P4 分组与各组末提交信息。§7 测试 → wind-dict 对拍（Task 2/3/4/5）、引擎 §3.2 逐行（Task 6）、协调器 §7 六条（Task 10/11）。§8 明确不做 → Global Constraints 与 Task 1 用例「短于 pattern 不匹配」。
- **签名一致性**：`search_pattern(pattern: &str, wildcard: char, limit: usize, with_prefix: bool)` 在 `DictLayer` / `CompositeDict` / `DictManager` 返回 `Vec<Candidate>`，`CachedDict` / `CodetableDict` 返回 `Vec<DictHit>`，`WdatReader` 返回 `Vec<DictEntry>`；`convert_wildcard(input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>` 在 trait / 码表 / 混输 / `EngineManager` 四处一致；`wildcard_key() -> Option<char>` / `active_wildcard_key() -> Option<char>` 一致；`wildcard_char(&self, schema_id: &str) -> Option<char>` 在 Task 8 定义、Task 9 消费；`wildcard_pattern(&self, buffer: &str) -> Option<String>` 在 Task 10 定义、Task 11 消费；`WildcardDecision::{Yield, Enter}` 仅 Task 10 内与两个接线点使用。
- **占位扫描**：全文无 TBD / TODO / 「类似任务 N」。依赖无法完全确认的内部细节处均写明核对点：Task 2 Step 3（循环体机械搬运的两处替换）、Task 6 Step 3（`build_engine` 的 `CommitOptions` 字面量需临时补字段）、Task 7 Step 1（`FakePinyin` / `MixConfig` 字段）、Task 10 Step 3（`BoundAction::is_enabled`、`CodeCharSet` 方法）、Task 11 Step 3（遮蔽变量与 `comment.rs` 的注释门控）、Task 13 Step 3（`<Since>` 版本号）。
- **已知取舍**：Task 9 的新增单测在实现前即通过（它锁 `resolve_codetable`，`build_engine` 真值由 Task 11 集成用例钉住），已在该步说明；Task 8 结束到 Task 12 结束之间 `wind_setting_assets` 预期红，已在两处注明。
