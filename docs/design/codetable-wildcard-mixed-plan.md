# 码表通配 · 五笔拼音混输调度 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 五笔拼音混输方案开通配后，码长内「字面混输 ⊕ 主码表通配」两路同查合并展示；首位、整串超码长、拼音分段续转三种情形整串按字面，行为与关闭通配时逐键相同。

**Architecture:** 引擎侧 `MixedEngine::convert_wildcard` = 字面 `convert(input)` ⊕ `primary.convert_wildcard(pattern)`，在新函数 `merge_wildcard` 里按来源分口径去重、按「通配等长 → 字面 → 通配更长」的存活次序带拼音保底截断；新增 `Engine::wildcard_code_length`（混输取主码表码长，**不**代理 `max_code_length`）与 `Engine::wildcard_mixes_pinyin`。显示序：`Candidate` 新增 `is_wildcard` 标记，`source_tier` 把「通配 + 等长」的码表候选放进档 0（与码表精确同档），非通配候选的档位逐字不变。协调器：`wildcard_lead_yields` 在混输下首位一律让位、`wildcard_pattern` 混输超码长整串字面、新增 `wildcard_pattern_of(state)` 在拼音分段续转态给 `None`，`build_candidates` 把分段续转分支排到通配分支之前。

**Tech Stack:** Rust workspace（wind-candidate / wind-engine / wind-coordinator）+ wind-setting 仓（设置说明）+ WindInputDocs 仓（文档站）

**Spec:** `docs/design/codetable-wildcard.md` **§10（五笔拼音混输下的调度，binding）**；§3（行为契约）、§9（修订记录）为背景。

## Global Constraints

- §10 调度表（逐条照做）：
  - 首位（缓冲为空）通配键**一律按字面**（z 是拼音声母：`zhang` / `zai`），不再依赖 `zz*` 短语让位；首位字面 ⇒ 整串字面（§3.3「首位让位即整串字面」）。
  - 非首位、整串长度 ≤ 主码表码长（五笔 4）⇒ **两路同查**：① 字面串走原混输转换（拼音 / 码表字面 / 英文照常）；② 主码表按通配 pattern 查；合并展示。
  - 整串长度 > 主码表码长（`hanzi`、`xianzai`）⇒ 整串按字面，走原混输路径。
  - 拼音分段上屏后的剩余串（续转拼音）⇒ 一律按字面。
- 合并规则：**通配等长结果排在最前**（与码表精确同档）；随后原混输结果按现有档位（拼音精确 → 码表前缀 → 拼音其余）；更长的通配补全排在拼音之后、落在原码表前缀档内。
- 去重：通配码表候选之间按 `(text, code)`（同字不同码各留）；拼音 / 英文候选与码表候选同字 ⇒ 码表留下（沿用现有规则）。
- 上屏：§3.2「不自动上屏 / 不顶字 / 不清空」仍按「本串有通配位」（`wildcard_pattern_of(state).is_some()`）判定。
- 通配码长：新增专门取值 `wildcard_code_length`（混输 = 主码表 `max_code_length`）；**不给 `MixedEngine` 代理 `max_code_length`**（会连带改 `phrase_auto_commit_min_len`，`handle_candidate.rs:1968-1971`）。
- 记账：码表候选记全码（`main_freq_code`），拼音候选照旧。组合区显示原始缓冲，通配键按原样显示。
- 关闭通配 ⇒ 一切与现状逐键相同；开通配但串里没有通配键 ⇒ 与关闭时逐键相同。
- 提交纪律：禁止 `git add -A` / `git add .` / `git commit -a` / `git stash`；新文件先 `git add -- <path>`；提交一律 `git commit -F - --only -- <自己的路径…>`；提交前 `git diff --cached --name-status` 核对暂存区、提交后 `git show --stat HEAD` 核对文件数。**任何提交都不带 `wind_input/Cargo.lock`**。每次提交前 `git -C <仓> branch --show-current` 须为 `feat/wildcard-mixed`。
- 路径：一切改动都在 `/home/dufeng/develop/windinput/wt-wcmix/{WindInput,wind-setting,WindInputDocs}` 下；**绝不**碰 `/home/dufeng/develop/windinput/{WindInput,wind-setting,WindInputDocs}`。
- 格式化：禁止 `cargo fmt` / `cargo fmt --all`；只对自己改过的文件跑 `rustfmt --edition 2024 --check <file>`，其 diff 只落在自己 hunk 内才去掉 `--check` 执行，否则按 diff 手工采纳自己那几处。
- 测试环境：cargo 一律在 `wind_input/` 下、带 `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix`（先 `mkdir -p /tmp/wct-wcmix`；路径要短，别用 scratchpad）；全量跑加 `--no-fail-fast`。wind-setting 的 `dev.sh sg / st` 带 `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix-ws`。
- 集成测试依赖 `build_dev/data`，缺失时静默跳过而计数照绿——以 `--test codetable_wildcard` 耗时 ≥ 1s 且输出无「跳过」为数据在位判据。
- 日志：`info!` 不得含用户输入 / 候选。

## Review Focus

- **跨来源去重只能在引擎做**：协调器通配下显示层去重键是 `(text, code)`（`handle_candidate.rs:1520`），拼音「蒸」（码 `azi`）与码表「蒸」（码 `abi`）码不同，到那里两条都留。且字面结果里的拼音可能与**更长**的通配补全同字，而更长补全排在字面之后——边走边收集码表文本的写法会漏掉这一对。规则：`merge_wildcard` 先收齐全部码表文本再过滤非码表候选。测试：Task 2 `wildcard_merge_drops_pinyin_same_text_keeps_codetable_codes`。
- **通配等长在调频开启时被拼音精确档反超**：显示序里 `cmp_exact_first` 排在 `source_tier` 之前（`candidate.rs:898-899`），所以关调频时等长通配天然在前、一切看着正常；但 `freq_rerank::freq_tier`（`freq_rerank.rs:82`）以 `source_tier` 为**首要键**，通配等长（`code != input`）落档 2，拼音精确档 1 会整体压过它。规则：`source_tier` 对 `is_wildcard && is_exact_code` 的码表候选给档 0。测试：Task 1 `wildcard_equal_length_shares_tier_with_codetable_exact`。
- **通配结果挤掉拼音**：主码表通配最多 100 条，全部排在字面结果之前，`max_candidates` 小时拼音一条都进不来。规则：合并后再过一遍 `truncate_with_pinyin_quota`。测试：Task 2 `wildcard_merge_keeps_pinyin_quota`。
- **拼音分段续转被通配截走**：`build_candidates` 现在通配分支在分段续转分支之前（`handle_candidate.rs:1206-1219`），`woaizi` 选「我」后剩 `aizi`（4 码、`z` 非首位）会被当成 `ai?i` 查五笔。规则：`wildcard_pattern_of` 在续转态给 `None`，且 `build_candidates` 先判续转。测试：Task 3 `pinyin_continuation_is_literal`、Task 4 `mixed_segment_continuation_stays_literal`（注意：spec 给的 `wo` + `aizhongguo` 超码长，走的是超码长规则，**测不到**续转规则，故另加 `woaizi`）。
- **混输首位 `z` 在无 `zz*` 短语时作了通配**：混输下 `has_code_prefix("z")` 只靠短语成立——单字母够不着 `min_pinyin_length`（`mixed/engine.rs:913`），五笔主库无 `z` 码。没有短语时首位 `z` 判 Enter，`zhang` 变 `?hang`。规则：`wildcard_lead_yields` 在 `wildcard_mixes_pinyin` 时直接让位。测试：Task 3 `mixed_lead_is_always_literal`（不装短语）、Task 4 `mixed_lead_z_is_literal_with_and_without_zz_phrases`。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `wind_input/crates/wind-candidate/src/candidate.rs` | Modify | `Candidate::is_wildcard` 字段 + `Default`；`source_tier` 通配等长进档 0；新增 `mod wildcard_tier_tests` |
| `wind_input/crates/wind-engine/src/codetable/engine.rs` | Modify | `convert_wildcard` 给结果置 `is_wildcard = true`；单测 |
| `wind_input/crates/wind-engine/src/engine.rs` | Modify | `Engine::wildcard_code_length` / `Engine::wildcard_mixes_pinyin` 默认实现 |
| `wind_input/crates/wind-engine/src/mixed/engine.rs` | Modify | `MixedEngine::merge_wildcard`；`convert_wildcard` 两路合并；两个新访问器；Task 3 放开 `wildcard_key`；单测与 `FakePinyinTable` |
| `wind_input/crates/wind-engine/src/manager.rs` | Modify | `active_wildcard_code_length` / `active_wildcard_mixes_pinyin` |
| `wind_input/crates/wind-coordinator/src/wildcard.rs` | Modify | `last_seg_is_pinyin`、`wildcard_pattern_of`、`wildcard_mixed_overflow`；`wildcard_enters` / `wildcard_past_full` / `wildcard_lead_yields` / `wildcard_pattern` 改判据；单测 |
| `wind_input/crates/wind-coordinator/src/handle_candidate.rs` | Modify | `build_candidates` 续转判据复用 `last_seg_is_pinyin`、续转分支先于通配分支、改用 `wildcard_pattern_of` |
| `wind_input/crates/wind-coordinator/src/coordinator.rs` | Modify | `accumulate_code_char` 顶字短路改用 `wildcard_pattern_of` |
| `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs` | Modify | 删 `mixed_pinyin_scheme_ignores_wildcard`，加 §10 真实数据用例 |
| `../wind-setting/src/assets/settings_manifest.toml` | Modify | `schema.codetable.wildcard` 的 hint |
| `../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json` | Modify（生成，若有变化） | `dev.sh sg` 重生成 |
| `../WindInputDocs/content/docs/settings/schema/codetable.mdx` | Modify | 「通配输入」小节的混输说明 |
| `docs/architecture/engine-candidate-pipeline.md` | Modify | §3.4 混输段落 |
| `docs/design/codetable-wildcard.md` | Modify | 头部状态行；实施中若有新偏离追加 §9 |

---

## 开工前：基线

- [ ] **Step 0: 记全量基线**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput && git status --short --branch && git log --oneline -1
cd wind_input && mkdir -p /tmp/wct-wcmix && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-wcmix/baseline.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: 分支 `feat/wildcard-mixed`、工作区干净；记下 `passed=` / `failed=` 两个数（Task 6 对照用）。`failed` 非 0 时先把失败名单记进账本，它们不属于本计划。

---

## Task 1: `Candidate::is_wildcard` 与 `source_tier` 通配等长进档 0

**Files:**
- Modify: `wind_input/crates/wind-candidate/src/candidate.rs`（字段加在 `is_direct_aux` ~448 之后；`impl Default for Candidate` ~587 `is_direct_aux: false,` 之后；`source_tier` ~1089 `CodeTable if c.code == input => 0,` 之后；文件末尾加测试模块）
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`convert_wildcard` 的 `.map` 闭包 ~921-927；`mod tests` 末尾加用例）
- Test: 上述两文件内联测试

**Interfaces:**
- Consumes: `CodeTableEngine::convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>`（已有）
- Produces:
  - `pub is_wildcard: bool`（`Candidate` 字段，`#[serde(skip)]`，默认 `false`）
  - `source_tier(c: &Candidate, input: &str) -> u8`：新增一条 `CodeTable if c.is_wildcard && c.is_exact_code => 0`，其余分支不变

- [ ] **Step 1: 写失败测试**

`candidate.rs` 文件末尾追加：

```rust
#[cfg(test)]
mod wildcard_tier_tests {
    use super::*;

    fn ct(code: &str, exact: bool, wildcard: bool) -> Candidate {
        Candidate {
            text: "蒸".into(),
            code: code.into(),
            is_exact_code: exact,
            is_wildcard: wildcard,
            is_common: true,
            source: CandidateSource::CodeTable,
            ..Default::default()
        }
    }

    /// spec §10：通配等长结果与码表精确同档。`freq_tier` 以本函数为首要键，
    /// 档 2 的话开调频时拼音精确档（1）会整体压过它。
    /// 对照：非通配候选只认 `code == input`，`is_exact_code` 不单独提档。
    #[test]
    fn wildcard_equal_length_shares_tier_with_codetable_exact() {
        assert_eq!(source_tier(&ct("abi", true, true), "azi"), 0, "通配等长 = 档 0");
        assert_eq!(source_tier(&ct("abic", false, true), "azi"), 2, "通配更长 = 码表前缀档");
        assert_eq!(source_tier(&ct("azi", true, false), "azi"), 0, "对照：字面精确");
        assert_eq!(
            source_tier(&ct("abi", true, false), "azi"),
            2,
            "对照：非通配候选不因 is_exact_code 提档"
        );
    }

    /// 锁（实现前即绿）：混输显示序「通配等长 → 拼音精确 → 通配更长」。
    /// 等长靠 `cmp_exact_first` 领先，更长补全靠 `source_tier` 落在拼音精确之后。
    #[test]
    fn mixed_display_order_wildcard_equal_then_pinyin_exact_then_longer() {
        let equal = Candidate {
            weight: 1,
            ..ct("abi", true, true)
        };
        let longer = Candidate {
            text: "蒸笼".into(),
            weight: 9000,
            ..ct("abic", false, true)
        };
        let py = Candidate {
            text: "阿紫".into(),
            code: "azi".into(),
            weight: 169,
            is_common: true,
            consumed_length: 3,
            source: CandidateSource::Pinyin,
            ..Default::default()
        };
        let mut v = vec![longer, py, equal];
        v.sort_by(|a, b| candidate_display_order(a, b, false, true, "azi"));
        let order: Vec<&str> = v.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(order, ["蒸", "阿紫", "蒸笼"]);
    }
}
```

`codetable/engine.rs` 的 `mod tests` 末尾追加：

```rust
    /// `is_wildcard` 只由通配入口置位；字面 `convert` 的结果恒不带（`source_tier` 靠它区分）。
    #[test]
    fn wildcard_results_carry_is_wildcard_and_literal_does_not() {
        let e = engine_opts(
            &[("ab", "甲", 10), ("abcd", "丙", 5)],
            wildcard_opts(CommitOptions::default()),
        );
        let r = e.convert_wildcard("az", &slot_pattern("a?"), 50).unwrap();
        assert!(!r.candidates.is_empty(), "前置：通配有命中");
        assert!(r.candidates.iter().all(|c| c.is_wildcard));
        let lit = e.convert("ab", 50).unwrap();
        assert!(!lit.candidates.is_empty(), "前置：字面有命中");
        assert!(lit.candidates.iter().all(|c| !c.is_wildcard), "字面结果不带通配标记");
    }
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input && mkdir -p /tmp/wct-wcmix
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-candidate wildcard_tier_tests
```

Expected: 编译失败，`error[E0560]: struct \`Candidate\` has no field named \`is_wildcard\``。

- [ ] **Step 3: 最小实现**

`candidate.rs`，`pub is_direct_aux: bool,` 之后加：

```rust
    /// 该候选来自码表**通配查询**（`CodeTableEngine::convert_wildcard`）。
    ///
    /// 只服务显示档位：通配下 `is_exact_code` 的语义是「与 pattern 等长」，`code` 必然 ≠ 输入
    /// （输入里带着通配键），[`source_tier`] 的 `code == input` 认不出它。见
    /// `docs/design/codetable-wildcard.md` §10「通配等长结果与码表精确同档」。
    /// 引擎内部用，不推送 UI。
    #[serde(skip)]
    pub is_wildcard: bool,
```

`impl Default for Candidate` 里 `is_direct_aux: false,` 之后加 `is_wildcard: false,`。

`source_tier` 的 `CodeTable if c.code == input => 0, // 码表精确全码（如五笔 cang→駏）` 之后加：

```rust
        // 通配等长结果与码表精确同档（spec §10：非首位按通配键是明确意图）。只认
        // `is_wildcard`：非通配路径上 `is_exact_code` 与 `code == input` 同义，不另开口子。
        CodeTable if c.is_wildcard && c.is_exact_code => 0,
```

`codetable/engine.rs` 的 `convert_wildcard` 里 `.map(|mut c| { … })` 闭包，`c.comment = c.code.clone();` 之后加 `c.is_wildcard = true;`。

- [ ] **Step 4: 跑，确认绿 + 全 workspace 编译**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-candidate wildcard_tier_tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-engine --lib wildcard_results_carry_is_wildcard
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo check --workspace --tests
```

Expected: 3 个用例通过；`cargo check` 无错误。核对点：若 `check` 报某处 `Candidate { … }` 字面量缺字段（没用 `..Default::default()` 的穷举构造），在那处补 `is_wildcard: false,`，并把该文件加进本任务的提交路径。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-candidate/src/candidate.rs wind_input/crates/wind-engine/src/codetable/engine.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-candidate/src/candidate.rs wind_input/crates/wind-engine/src/codetable/engine.rs <<'EOF'
feat(candidate): 通配等长结果在来源档位里与码表精确同档

Candidate 新增 is_wildcard（只由码表 convert_wildcard 置位），source_tier 对
「通配 + 等长」的码表候选给档 0。显示序里 cmp_exact_first 先于 source_tier，关调频时
看不出差别；但 freq_tier 以 source_tier 为首要键，不改的话混输开调频后拼音精确档会
压过通配等长结果（spec codetable-wildcard §10）。非通配候选档位逐字不变。
EOF
git show --stat HEAD
```

---

## Task 2: 引擎——通配码长访问器与混输两路合并

**Files:**
- Modify: `wind_input/crates/wind-engine/src/engine.rs`（trait `Engine` 的 `convert_wildcard` 默认实现 ~394-401 之后）
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（`impl MixedEngine` 里 `truncate_with_pinyin_quota` 之后加 `merge_wildcard`；`impl Engine for MixedEngine` ~837-850；`mod tests` 里改写 `wildcard_off_when_pinyin_present` ~2434-2469，新增夹具与用例）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`active_wildcard_key` ~2607-2609 之后）

**Interfaces:**
- Consumes: Task 1 的 `Candidate::is_wildcard`（仅测试断言）；`MixedEngine::truncate_with_pinyin_quota(cands: &mut Vec<Candidate>, max_candidates: usize)`（已有）
- Produces:
  - `Engine::wildcard_code_length(&self) -> usize`（默认 `self.max_code_length()`；混输 `self.max_code_len`）
  - `Engine::wildcard_mixes_pinyin(&self) -> bool`（默认 `false`；混输 `self.secondary.is_some()`）
  - `MixedEngine::merge_wildcard(wildcard: Vec<Candidate>, literal: Vec<Candidate>, max_candidates: usize) -> Vec<Candidate>`（私有关联函数）
  - `MixedEngine::convert_wildcard`：有拼音子引擎时返回两路合并结果（`should_commit` / `should_clear` 恒 false，`preedit_display = input`，`preedit_pinyin` 取字面结果）
  - `EngineManager::active_wildcard_code_length(&self) -> usize`、`EngineManager::active_wildcard_mixes_pinyin(&self) -> bool`
- ⚠️ 本任务**不动** `MixedEngine::wildcard_key()`（仍在有拼音子引擎时返回 `None`）：协调器规则在 Task 3 才落地，提前放开会让 `hanzi` 在两任务之间变成 `han?i`、现有集成用例 `mixed_pinyin_scheme_ignores_wildcard` 转红。

- [ ] **Step 1: 写失败测试**

`mixed/engine.rs` 的 `mod tests` 里，把整个 `wildcard_off_when_pinyin_present`（含 doc 注释）替换为下面这一段：

```rust
    /// 带通配键 `z` 的内存码表（码长 4）。
    fn ct_wildcard(entries: &[(&str, &str, i32)]) -> Box<dyn Engine> {
        let mut d = CodetableDict::empty();
        for (i, (code, text, w)) in entries.iter().enumerate() {
            d.merge_single(code.to_string(), text.to_string(), *w, i as i32);
        }
        let dm = DictManager::new();
        dm.register_layer(Box::new(SystemDictLayer::new(CachedDict::Memory(d), "sys")));
        Box::new(CodeTableEngine::new(
            4,
            CommitOptions {
                wildcard: Some('z'),
                ..Default::default()
            },
            Arc::new(dm),
        ))
    }

    /// 按输入查表的假拼音（`FakePinyin` 不看输入、恒出同一条，测不了「字面串走拼音」）。
    struct FakePinyinTable {
        entries: Vec<(&'static str, &'static str)>,
    }
    impl Engine for FakePinyinTable {
        fn convert(&self, input: &str, _max: usize) -> anyhow::Result<ConvertResult> {
            let candidates = self
                .entries
                .iter()
                .filter(|(k, _)| *k == input)
                .enumerate()
                .map(|(i, (_, t))| Candidate {
                    text: t.to_string(),
                    code: input.to_string(),
                    weight: 1000 - i as i32,
                    consumed_length: input.len(),
                    source: CandidateSource::Pinyin,
                    ..Default::default()
                })
                .collect();
            Ok(ConvertResult {
                candidates,
                ..Default::default()
            })
        }
        fn reset(&self) {}
        fn engine_type(&self) -> EngineType {
            EngineType::Pinyin
        }
    }

    fn mixed_wc(ct: &[(&str, &str, i32)], py: Vec<(&'static str, &'static str)>) -> MixedEngine {
        MixedEngine::new(
            ct_wildcard(ct),
            Some(Box::new(FakePinyinTable { entries: py })),
            None,
            MixConfig::default(),
        )
    }

    fn slot(p: &str) -> String {
        p.replace('?', &wind_dict::WILDCARD_SLOT.to_string())
    }

    /// spec §10：字面混输 ⊕ 主码表通配。引擎次序 = 存活优先级：通配等长 → 字面 → 通配更长。
    /// 通配结果不给上屏 / 清空意向；组合区是原始缓冲。
    #[test]
    fn wildcard_merges_literal_mixed_with_primary_pattern() {
        let e = mixed_wc(
            &[("abi", "蒸", 9000), ("adi", "藉", 8000), ("abic", "蒸笼", 500)],
            vec![("azi", "阿紫")],
        );
        let r = e
            .convert_wildcard("azi", &slot("a?i"), 50)
            .expect("有拼音子引擎时也接通配请求");
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["蒸", "藉", "阿紫", "蒸笼"]);
        assert!(!r.should_commit && r.commit_text.is_empty() && !r.should_clear);
        assert_eq!(r.preedit_display, "azi");
        assert!(
            r.candidates
                .iter()
                .filter(|c| c.source == CandidateSource::CodeTable)
                .all(|c| c.is_wildcard),
            "码表侧全部来自通配"
        );
    }

    /// ★ Review Focus 1：码表之间按 (text, code) 去重（同字不同码各留），拼音与**任一**
    /// 码表候选同字即丢——包括与排在字面之后的通配**更长**补全同字。
    #[test]
    fn wildcard_merge_drops_pinyin_same_text_keeps_codetable_codes() {
        let e = mixed_wc(
            &[("abi", "蒸", 9000), ("aci", "蒸", 100), ("abic", "蒸笼", 500)],
            vec![("azi", "蒸"), ("azi", "蒸笼"), ("azi", "阿紫")],
        );
        let r = e.convert_wildcard("azi", &slot("a?i"), 50).unwrap();
        let pairs: Vec<(&str, &str, CandidateSource)> = r
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.code.as_str(), c.source))
            .collect();
        let zheng: Vec<&str> = pairs
            .iter()
            .filter(|p| p.0 == "蒸")
            .map(|p| p.1)
            .collect();
        assert_eq!(zheng, ["abi", "aci"], "同字不同码各留，拼音「蒸」被丢");
        assert!(
            pairs
                .iter()
                .filter(|p| p.0 == "蒸笼")
                .all(|p| p.2 == CandidateSource::CodeTable),
            "与通配更长补全同字的拼音也被丢：{pairs:?}"
        );
        assert!(pairs.iter().any(|p| p.0 == "阿紫" && p.2 == CandidateSource::Pinyin));
    }

    /// ★ Review Focus 3：通配结果全排在字面之前，截断后拼音仍有保底席位。
    #[test]
    fn wildcard_merge_keeps_pinyin_quota() {
        let e = mixed_wc(
            &[
                ("aai", "甲", 900),
                ("abi", "乙", 800),
                ("aci", "丙", 700),
                ("adi", "丁", 600),
                ("aei", "戊", 500),
                ("afi", "己", 400),
            ],
            vec![("azi", "阿紫"), ("azi", "阿姊")],
        );
        let r = e.convert_wildcard("azi", &slot("a?i"), 5).unwrap();
        assert_eq!(r.candidates.len(), 5);
        let py = r
            .candidates
            .iter()
            .filter(|c| c.source == CandidateSource::Pinyin)
            .count();
        assert!(py >= 1, "5 / PINYIN_QUOTA_DIVISOR = 1 席保底，实际 {py}");
    }

    /// 通配码长取主码表的，但 `max_code_length` 不代理（它还决定短语自动上屏门槛）。
    /// 对照：无拼音子引擎时只代理主码表、不混拼音。
    #[test]
    fn wildcard_code_length_is_primary_max_without_proxying_max_code_length() {
        let mixed = mixed_wc(&[("ab", "甲", 10)], vec![]);
        assert_eq!(mixed.wildcard_code_length(), 4);
        assert_eq!(Engine::max_code_length(&mixed), 0, "混输 max_code_length 维持 0");
        assert!(mixed.wildcard_mixes_pinyin());
        assert_eq!(
            mixed.wildcard_key(),
            None,
            "协调器规则落地前（Task 3）仍关闭"
        );

        let solo = MixedEngine::new(ct_wildcard(&[("ab", "甲", 10)]), None, None, MixConfig::default());
        assert!(!solo.wildcard_mixes_pinyin());
        assert_eq!(solo.wildcard_code_length(), 4);
        assert_eq!(solo.wildcard_key(), Some('z'), "对照：无拼音时代理主码表");
        let r = solo
            .convert_wildcard("az", &slot("a?"), 10)
            .expect("主码表开了通配");
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["甲"]);
    }
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-engine --lib mixed::engine::tests::wildcard
```

Expected: 编译失败，`error[E0599]: no method named \`wildcard_code_length\``（及 `wildcard_mixes_pinyin`）。

- [ ] **Step 3: 最小实现**

`engine.rs`，trait `Engine` 里 `convert_wildcard` 默认实现之后加：

```rust
    /// 通配用的码长（spec §10）：协调器据此判「满码后通配键按字面」与混输「超码长整串字面」。
    /// 默认即 [`Self::max_code_length`]；混输返回主码表的码长。
    ///
    /// ⚠️ 与 `max_code_length` 分开是有意的：混输的 `max_code_length` 刻意为 0，它还决定
    /// 协调器的短语自动上屏门槛（`phrase_auto_commit_min_len`），代理过去会连带改变那条行为。
    fn wildcard_code_length(&self) -> usize {
        self.max_code_length()
    }

    /// 通配与拼音共用这串输入（五笔拼音混输且有拼音子引擎）。协调器据此把首位、整串超码长、
    /// 拼音分段续转三种情形判为字面（spec §10）。
    fn wildcard_mixes_pinyin(&self) -> bool {
        false
    }
```

`mixed/engine.rs`，`impl MixedEngine` 里 `truncate_with_pinyin_quota` 之后加：

```rust
    /// 通配合并（spec §10）：主码表通配结果 ⊕ 字面混输结果。
    ///
    /// 次序即**存活优先级**（截断时谁先活；显示序由协调器 `candidate_display_order` 重排）：
    /// 通配等长 → 字面混输结果（其内部已按 `truncation_tier` 排好）→ 通配更长补全。
    ///
    /// 去重分两种口径：码表候选之间按 `(text, code)`（同字不同码各留，学码用，同 spec §3.1）；
    /// 拼音 / 英文候选与**任一**码表候选同字即丢（沿用 `sort_dedup_truncate`「码表留下」）。
    /// ⚠️ 跨来源去重只能在这里做：协调器通配下按 `(text, code)` 去重，拼音「蒸」与码表「蒸」
    /// 码不同，到那里两条都留。码表文本须**先收齐**再过滤：通配更长补全排在字面之后，
    /// 边走边收会漏掉与它同字的拼音。
    fn merge_wildcard(
        wildcard: Vec<Candidate>,
        literal: Vec<Candidate>,
        max_candidates: usize,
    ) -> Vec<Candidate> {
        let (equal, longer): (Vec<Candidate>, Vec<Candidate>) =
            wildcard.into_iter().partition(|c| c.is_exact_code);
        let ct_texts: std::collections::HashSet<String> = equal
            .iter()
            .chain(&literal)
            .chain(&longer)
            .filter(|c| c.source == CandidateSource::CodeTable)
            .map(|c| c.text.clone())
            .collect();
        let mut ct_keys: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut merged: Vec<Candidate> = Vec::with_capacity(equal.len() + literal.len() + longer.len());
        for c in equal.into_iter().chain(literal).chain(longer) {
            let keep = if c.source == CandidateSource::CodeTable {
                ct_keys.insert((c.text.clone(), c.code.clone()))
            } else {
                !ct_texts.contains(&c.text)
            };
            if keep {
                merged.push(c);
            }
        }
        Self::truncate_with_pinyin_quota(&mut merged, max_candidates);
        merged
    }
```

`impl Engine for MixedEngine` 里，`wildcard_key` 保持原样；把 `convert_wildcard` 整个替换为下面这段，并在其后加两个访问器：

```rust
    /// 通配转换（spec §10）。无拼音子引擎 ⇒ 代理主码表；有 ⇒ 字面 `convert(input)`
    /// ⊕ 主码表通配，见 [`Self::merge_wildcard`]。
    ///
    /// 前提：协调器只在整串 ≤ 主码表码长时给 pattern（超码长整串字面），故这里的字面
    /// `convert` 恒走码长内分支。上屏 / 清空意向一律不给（§3.2）；组合区显示原始缓冲，
    /// 拼音拆分形态沿用字面结果，供高亮拼音候选时的「高亮跟随」。
    fn convert_wildcard(
        &self,
        input: &str,
        pattern: &str,
        max_candidates: usize,
    ) -> Option<ConvertResult> {
        let wc = self
            .primary
            .convert_wildcard(input, pattern, max_candidates)?;
        if self.secondary.is_none() {
            return Some(wc);
        }
        let lit = self.convert(input, max_candidates).unwrap_or_default();
        let candidates = Self::merge_wildcard(wc.candidates, lit.candidates, max_candidates);
        let is_empty = candidates.is_empty();
        Some(ConvertResult {
            candidates,
            preedit_pinyin: lit.preedit_pinyin,
            preedit_display: input.to_string(),
            is_empty,
            ..Default::default()
        })
    }

    fn wildcard_code_length(&self) -> usize {
        self.max_code_len
    }

    fn wildcard_mixes_pinyin(&self) -> bool {
        self.secondary.is_some()
    }
```

`manager.rs`，`active_wildcard_key` 之后加：

```rust
    /// 活跃方案的通配码长（[`Engine::wildcard_code_length`]；混输取主码表的）。
    /// 协调器判「满码后通配键按字面」只从这里取，**不**用 [`Self::active_max_code_length`]
    /// ——混输那个值刻意为 0。
    pub fn active_wildcard_code_length(&self) -> usize {
        self.active_engine()
            .map(|e| e.wildcard_code_length())
            .unwrap_or(0)
    }

    /// 活跃方案的通配是否与拼音共用输入（五笔拼音混输且有拼音子引擎）。
    pub fn active_wildcard_mixes_pinyin(&self) -> bool {
        self.active_engine()
            .is_some_and(|e| e.wildcard_mixes_pinyin())
    }
```

核对点：`MixedEngine::max_code_len` 在 `new` 里取自 `primary.max_code_length()`（`mixed/engine.rs` 的 `new`），与主码表码长同值；`ConvertResult` 有 `preedit_pinyin` / `preedit_display` / `is_empty` / `should_commit` / `commit_text` / `should_clear` 字段（`convert` 末尾的构造即是）。

- [ ] **Step 4: 跑，确认绿 + 混输模块回归**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-engine --lib mixed::
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-coordinator --test codetable_wildcard
```

Expected: `mixed::` 全绿（含 4 条新用例）；`codetable_wildcard` 全绿（`wildcard_key` 未放开，协调器行为不变）、耗时 ≥ 1s。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): 混输通配两路合并与通配专用码长

MixedEngine::convert_wildcard 在有拼音子引擎时 = 字面 convert ⊕ 主码表通配，
merge_wildcard 按「通配等长 → 字面 → 通配更长」的存活次序合并：码表间按
(text, code) 去重，拼音 / 英文与任一码表候选同字即丢，再带拼音保底截断。
新增 Engine::wildcard_code_length（混输取主码表码长）与 wildcard_mixes_pinyin；
不给混输代理 max_code_length（它还决定短语自动上屏门槛）。

wildcard_key 仍在有拼音时关闭，协调器规则落地后再放开（spec codetable-wildcard §10）。
EOF
git show --stat HEAD
```

---

## Task 3: 协调器——混输三种字面情形、续转先于通配、放开混输通配键

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/wildcard.rs`（`wildcard_enters` ~45-49、`wildcard_past_full` ~79-82、`wildcard_lead_yields` ~89 函数体开头、`wildcard_pattern` ~160-163；新增 `last_seg_is_pinyin`、`wildcard_mixed_overflow`、`wildcard_pattern_of`；`mod tests` 追加）
- Modify: `wind_input/crates/wind-coordinator/src/handle_candidate.rs`（`build_candidates` ~1181-1219）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`accumulate_code_char` 顶字短路 ~5128）
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（`wildcard_key` ~837-842；Task 2 用例里那条「仍关闭」断言）
- Test: `wildcard.rs` 内联 `mod tests`

**Interfaces:**
- Consumes: Task 2 的 `EngineManager::active_wildcard_code_length() -> usize`、`EngineManager::active_wildcard_mixes_pinyin() -> bool`、`MixedEngine::convert_wildcard`
- Produces:
  - `pub(crate) fn last_seg_is_pinyin(state: &State) -> bool`（`wildcard.rs` 模块级自由函数）
  - `Coordinator::wildcard_pattern_of(&self, state: &State) -> Option<String>`（`pub(crate)`）
  - `Coordinator::wildcard_mixed_overflow(&self, buffer: &str) -> bool`（私有）
  - `Coordinator::wildcard_pattern(&self, buffer: &str) -> Option<String>` 签名不变，多一条混输超码长判据
  - `MixedEngine::wildcard_key()` 改为无条件代理主码表

- [ ] **Step 1: 写失败测试**

`wildcard.rs` 的 `mod tests` 末尾追加：

```rust
    /// 五笔拼音混输 + 通配键 `z`；缺数据返回 `None`。
    fn wubi_pinyin_z(tweak: impl FnOnce(&mut Config)) -> Option<Arc<Coordinator>> {
        if !data_dir().join("schemas/wubi86_pinyin.schema.toml").exists() {
            eprintln!("跳过：混输方案不存在");
            return None;
        }
        wubi_z(|cfg| {
            cfg.schema.available = vec!["wubi86_pinyin".into(), "wubi86".into(), "pinyin".into()];
            cfg.schema.active = "wubi86_pinyin".into();
            tweak(cfg);
        })
    }

    /// ★ Review Focus 5：混输首位一律字面（z 是拼音声母），不靠 `zz*` 短语——这里**不装**
    /// 短语，混输下 `has_code_prefix("z")` 为假，旧规则会判 Enter。首位字面 ⇒ 整串字面。
    #[test]
    fn mixed_lead_is_always_literal() {
        let Some(c) = wubi_pinyin_z(|_| {}) else { return };
        assert_eq!(c.wildcard_decision("", 'z'), Yield, "混输首位 z 是拼音声母");
        assert_eq!(c.wildcard_decision("z", 'z'), Yield, "首位字面 ⇒ 整轮字面");
        assert_eq!(c.wildcard_pattern("zhan"), None);
        assert_eq!(c.wildcard_pattern("zaz"), None);
        assert_eq!(c.wildcard_decision("a", 'z'), Enter, "对照：非首位码长内照常通配");
    }

    /// 混输：整串 > 主码表码长 ⇒ 整串字面（连前段通配位一起）；码长取主码表的 4，
    /// 不是混输 `max_code_length` 的 0。对照纯五笔 `azaaz` → `a?aaz`（`wildcard_past_full_length_is_literal`）。
    #[test]
    fn mixed_overlength_buffer_is_literal() {
        let Some(c) = wubi_pinyin_z(|_| {}) else { return };
        assert_eq!(c.wildcard_pattern("hanz"), Some(format!("han{S}")));
        assert_eq!(c.wildcard_pattern("gz"), Some(format!("g{S}")));
        assert_eq!(c.wildcard_pattern("hanzi"), None, "超码长整串字面");
        assert_eq!(c.wildcard_pattern("azaaz"), None, "前段通配位也随整串字面");
        assert_eq!(c.wildcard_decision("xian", 'z'), Yield, "码长取主码表的 4");
        assert_eq!(c.wildcard_decision("han", 'z'), Enter);
    }

    /// ★ Review Focus 4：拼音分段续转态（最后一段是拼音选词）剩余串一律字面，按键也不作通配。
    #[test]
    fn pinyin_continuation_is_literal() {
        let Some(c) = wubi_pinyin_z(|_| {}) else { return };
        let mut st = crate::coordinator::State::default();
        st.input_buffer = "aizi".into();
        assert_eq!(
            c.wildcard_pattern_of(&st),
            Some(format!("ai{S}i")),
            "前置：无分段时码长内照常通配"
        );
        st.committed_segs.push(crate::coordinator::CommittedSeg {
            raw_code: "wo".into(),
            code: "wo".into(),
            text: "我".into(),
            source: wind_candidate::CandidateSource::Pinyin,
            boundary: 0,
            learn: None,
        });
        assert_eq!(c.wildcard_pattern_of(&st), None, "拼音分段续转 ⇒ 字面");
        st.input_buffer = "ai".into();
        assert!(!c.wildcard_enters(&st, 'z'), "续转态按键也不作通配");
        st.committed_segs.clear();
        assert!(c.wildcard_enters(&st, 'z'), "对照：无分段时作通配");
    }
```

`mixed/engine.rs` 的 `wildcard_code_length_is_primary_max_without_proxying_max_code_length` 里，把

```rust
        assert_eq!(
            mixed.wildcard_key(),
            None,
            "协调器规则落地前（Task 3）仍关闭"
        );
```

改为

```rust
        assert_eq!(mixed.wildcard_key(), Some('z'), "混输代理主码表的通配键（spec §10）");
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-coordinator --lib wildcard::tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-engine --lib wildcard_code_length_is_primary_max
```

Expected: coordinator 编译失败 `error[E0599]: no method named \`wildcard_pattern_of\``；engine 那条断言失败（`left: None, right: Some('z')`）。

- [ ] **Step 3: 最小实现**

`mixed/engine.rs`，`wildcard_key` 改为：

```rust
    /// 混输代理主码表的通配键（spec §10：码长内两路同查）。哪几位作通配、何时整串字面
    /// 由协调器裁决（首位 / 超码长 / 拼音分段续转，见 `wind-coordinator/src/wildcard.rs`）。
    fn wildcard_key(&self) -> Option<char> {
        self.primary.wildcard_key()
    }
```

`wildcard.rs`：

1. `impl Coordinator` 之前（`wildcard_key_vk` 之后）加：

```rust
/// 拼音分段续转态：最后一段是拼音选词，剩余编码按混输的拼音子方案转换（`build_candidates`）。
/// 该态下通配一律字面（spec §10）——剩余串是拼音的后半截，不是码表码。
///
/// ★ 与 `build_candidates` 选拼音子方案用的是同一个函数：两处各写一份，改一处另一处就会
/// 出现「续转走拼音、却又按通配查码表」。本判据看会话状态（已上屏段），退格回退段时随之
/// 失效，仍满足模块文档「同一状态前后两次重算结论相同」。
pub(crate) fn last_seg_is_pinyin(state: &State) -> bool {
    state
        .committed_segs
        .last()
        .is_some_and(|s| s.source == CandidateSource::Pinyin)
}
```

2. `wildcard_enters` 函数体改为：

```rust
        state.active.is_none()
            && !last_seg_is_pinyin(state)
            && self.engine_mgr.active_wildcard_key() == Some(ch)
            && self.wildcard_decision(&state.input_buffer, ch) == WildcardDecision::Enter
```

3. `wildcard_past_full` 里 `let max = self.engine_mgr.active_max_code_length();` 改为 `let max = self.engine_mgr.active_wildcard_code_length();`，并把 doc 里「`max_code_length` 为 0（无满码概念）时恒 `false`」改为「通配码长（`wildcard_code_length`，混输取主码表的）为 0 时恒 `false`」。

4. `wildcard_lead_yields` 函数体最前面加：

```rust
        // 五笔拼音混输：首位一律字面（spec §10）。z 是拼音声母（`zhang` / `zai`），而混输下
        // `has_code_prefix("z")` 只在装了 `zz*` 短语时成立（单字母够不着 `min_pinyin_length`，
        // 五笔主库无 z 码），靠它让位的话没装短语的用户打 `zhang` 会变成 `?hang`。
        if self.engine_mgr.active_wildcard_mixes_pinyin() {
            return true;
        }
```

5. `wildcard_pattern` 开头的判断改为：

```rust
        let key = self.engine_mgr.active_wildcard_key()?;
        if !buffer.contains(key)
            || self.wildcard_lead_literal(buffer, key)
            || self.wildcard_mixed_overflow(buffer)
        {
            return None;
        }
```

并在 doc 注释「落在满码之后的通配键一律字面」一句后补：「混输下整串超过主码表码长 ⇒ 整串字面（[`Self::wildcard_mixed_overflow`]）；拼音分段续转态见 [`Self::wildcard_pattern_of`]。」

6. `wildcard_pattern` 之后加：

```rust
    /// 混输：整串长度超过主码表码长 ⇒ 码表不可能配上，整串归拼音、按字面（spec §10）。
    /// 纯码表不走这条：那里按位判满码（`azaaz` 前段仍是通配，见 [`Self::wildcard_past_full`]）。
    fn wildcard_mixed_overflow(&self, buffer: &str) -> bool {
        let max = self.engine_mgr.active_wildcard_code_length();
        self.engine_mgr.active_wildcard_mixes_pinyin() && max > 0 && buffer.chars().count() > max
    }

    /// [`Self::wildcard_pattern`] 再加一条**看会话状态**的判据：拼音分段续转态一律字面
    /// （spec §10，[`last_seg_is_pinyin`]）。手里有 `State` 的调用点一律走这里。
    ///
    /// `main_freq_code` 仍按缓冲调 `wildcard_pattern`：续转态的转换走纯拼音子方案，不产出
    /// 码表候选，而 `main_freq_code` 只对码表候选改记全码，两者结论一致。
    pub(crate) fn wildcard_pattern_of(&self, state: &State) -> Option<String> {
        if last_seg_is_pinyin(state) {
            return None;
        }
        self.wildcard_pattern(&state.input_buffer)
    }
```

`handle_candidate.rs` 的 `build_candidates`：

1. 把

```rust
        let last_seg_is_pinyin = state
            .committed_segs
            .last()
            .is_some_and(|s| s.source == CandidateSource::Pinyin);
```

改为 `let last_seg_is_pinyin = crate::wildcard::last_seg_is_pinyin(state);`（上方那段长注释保留）。

2. 把从 `// 通配组码（spec §5.3）：只查码表，…` 开始、到 `let result = if let Some(p) = &wildcard_pattern { … } else { match pinyin_schema { … } };` 结束的整段替换为：

```rust
        // 通配组码（spec §5.3 / §10）：纯码表只查码表；五笔拼音混输码长内是「字面混输 ⊕
        // 主码表通配」（合并在 `MixedEngine::convert_wildcard`）。两者都不走短语 / 整句 / 逆切分。
        // pattern 由协调器按 §3.3 / §10 重算：首位字面、混输超码长、拼音分段续转都不给 pattern。
        // 它同时是本函数下面各处短路（短语、自动上屏复评、清空复核、短语自动上屏、
        // 显示层去重口径、出简让全）的唯一判据。
        let wildcard_pattern = self.wildcard_pattern_of(state);
        // ★ 分段续转排在通配之前（spec §10）：续转态的剩余串是拼音后半截。
        // `wildcard_pattern_of` 在续转态本就给 `None`，这里的先后是第二道保险。
        let result = match pinyin_schema {
            Some(ps) if self.engine_mgr.ensure_schema(&ps) => {
                self.engine_mgr
                    .convert_with(&ps, &state.input_buffer, limit)
            }
            _ => match &wildcard_pattern {
                Some(p) => self
                    .engine_mgr
                    .convert_wildcard(&state.input_buffer, p, limit)
                    .unwrap_or_default(),
                None => self.engine_mgr.convert(&state.input_buffer, limit),
            },
        };
```

核对点：替换后 `CandidateSource` 在 `handle_candidate.rs` 里仍有别处使用（该文件大量引用），不会出现未使用导入告警；`via_mixed_pinyin` 在替换段之前已由 `pinyin_schema.is_some()` 求出，`match pinyin_schema` 按值消费不影响它。

`coordinator.rs` 的 `accumulate_code_char`：`let top_code = if self.wildcard_pattern(&state.input_buffer).is_some() {` 改为 `let top_code = if self.wildcard_pattern_of(state).is_some() {`。

- [ ] **Step 4: 跑，确认绿 + 协调器回归**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-coordinator --lib wildcard::tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-engine --lib mixed::
time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-coordinator --no-fail-fast --test codetable_wildcard --test input_flow --test mix_repeat_no_freq --test commit_highlighted_key --test top_commit_learn 2>&1 | grep -E "^test result|FAILED|panicked|跳过"
```

Expected: 全绿；输出无「跳过」。`mixed_pinyin_scheme_ignores_wildcard` 此时仍绿（`hanzi` 超码长整串字面，与关闭时相同），由 Task 4 替换。若 `input_flow` 的混输用例转红，说明某处混输行为在「串里没有 z」时也变了——回查 `wildcard_lead_yields` 的提前返回是否误伤了非通配键路径（它只在 `wildcard_decision` 里被调用）。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-engine/src/mixed/engine.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-engine/src/mixed/engine.rs <<'EOF'
feat(coordinator): 五笔拼音混输下开通配——码长内两路同查，三种情形整串字面

混输首位一律字面（z 是拼音声母，不再靠 zz* 短语让位）；整串超过主码表码长整串字面；
拼音分段续转态字面（wildcard_pattern_of，与 build_candidates 的续转判据同源），且
build_candidates 先判续转再判通配。满码判据改用通配专用码长（混输取主码表的 4）。
MixedEngine::wildcard_key 放开为代理主码表（spec codetable-wildcard §10）。
EOF
git show --stat HEAD
```

---

## Task 4: 真实数据集成用例（wubi86_pinyin）

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（删除 `mixed_pinyin_scheme_ignores_wildcard` ~332-365 整条含 doc；在 `// ─── 候选管线（Task 11）` 分隔行之前插入新的一节）

**Interfaces:**
- Consumes: `Coordinator::new_headless`、`debug_all_candidate_texts`、`debug_candidate_triples`、`debug_input_buffer`、`debug_install_phrases`、`select_candidate(index)`（绝对下标，`candidate_pull.rs:95`）；文件内已有 `data_dir` / `dict_ready` / `press` / `wubi` / `zz_phrases`
- Produces: 无新接口

> 这一组是验收用例：Task 1-3 已把行为落地，预期**首跑即绿**。任一条红都是 Task 1-3 的缺陷，回到对应任务修，**不许**放宽断言。数据前提（2026-09-29 核对 `build_dev/data`）：五笔主库无含 `z` 的码；`ga 开` / `gb 屯` 等二码字；`hana 戱` / `hang 虛` / `hani 虑`；`abi 蒸` / `adi 藉` / `afi 蒜`；拼音 `base.dict.yaml` 有 `阿紫 a zi 169`、`阿姊 a zi 13`，`8105.dict.yaml` 有 `张 zhang`。

- [ ] **Step 1: 写用例**

删除 `mixed_pinyin_scheme_ignores_wildcard`，插入：

```rust
// ─────────────────────────── 五笔拼音混输（spec §10） ───────────────────────────

fn mixed_ready() -> bool {
    dict_ready()
        && data_dir()
            .join("schemas/wubi86_pinyin.schema.toml")
            .exists()
}

fn wubi_pinyin(wildcard: bool) -> Config {
    let mut cfg = wubi(wildcard, "z");
    cfg.schema.available = vec!["wubi86_pinyin".into(), "wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86_pinyin".into();
    cfg
}

/// `code` 是否为 `pattern`（`z` 作通配位）的五笔命中。五笔词库没有含 `z` 的码，
/// 故带 `z` 的码只可能是拼音 / 字面串——借此从 `(text, code, comment)` 里认出来源。
fn wubi_hit(pattern: &str, code: &str, equal: bool) -> bool {
    let len_ok = if equal {
        code.len() == pattern.len()
    } else {
        code.len() > pattern.len()
    };
    len_ok
        && !code.contains('z')
        && pattern
            .chars()
            .zip(code.chars())
            .all(|(p, c)| p == 'z' || p == c)
}

fn mixed_triples(on: bool, keys: &str) -> Vec<(String, String, String)> {
    let coord = Coordinator::new_headless(wubi_pinyin(on), Some(&data_dir()));
    press(&coord, keys);
    coord.debug_candidate_triples()
}

/// `gz`：通配出 `g` 开头的二码字，首位是等长结果，注释是完整编码。
/// 对照：关闭时没有任何 `g?` 五笔命中。
#[test]
fn mixed_gz_lists_two_code_chars() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let on = mixed_triples(true, "gz");
    let (text, code, comment) = on.first().expect("gz 应有候选");
    assert!(wubi_hit("gz", code, true), "首选应是 g? 等长命中，实际 {text} {code}");
    assert_eq!(comment, code, "注释是完整编码");
    let off = mixed_triples(false, "gz");
    assert!(
        !off.iter().any(|(_, c, _)| wubi_hit("gz", c, true)),
        "对照：关闭时无通配命中，实际 {off:?}"
    );
}

/// `hanz`：通配（`han?` 的五笔字）与拼音（「汉字」这类补全）并存，等长通配排在拼音之前。
#[test]
fn mixed_hanz_merges_wildcard_and_pinyin() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let off = mixed_triples(false, "hanz");
    assert!(
        off.iter().any(|(t, _, _)| t == "汉字"),
        "前置：关闭时 hanz 应有拼音补全「汉字」，实际 {off:?}"
    );
    let on = mixed_triples(true, "hanz");
    let hanzi = on
        .iter()
        .position(|(t, _, _)| t == "汉字")
        .unwrap_or_else(|| panic!("开启后拼音「汉字」仍在，实际 {on:?}"));
    let last_equal = on
        .iter()
        .rposition(|(_, c, _)| wubi_hit("hanz", c, true))
        .unwrap_or_else(|| panic!("应有 han? 五笔命中，实际 {on:?}"));
    assert!(wubi_hit("hanz", &on[0].1, true), "首选是等长通配，实际 {:?}", on[0]);
    assert!(last_equal < hanzi, "等长通配全部排在拼音之前：{on:?}");
}

/// `hanzi` / `xianzai`：超码长整串字面，与关闭时逐条相同。
#[test]
fn mixed_overlength_is_identical_to_off() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    for (keys, word) in [("hanzi", "汉字"), ("xianzai", "现在")] {
        let on = mixed_triples(true, keys);
        assert!(on.iter().any(|(t, _, _)| t == word), "{keys} 应出「{word}」，实际 {on:?}");
        assert_eq!(on, mixed_triples(false, keys), "{keys} 与关闭时相同");
    }
}

/// ★ Review Focus 5：`zhang` 首位字面，装不装 `zz*` 短语都与关闭时相同。
#[test]
fn mixed_lead_z_is_literal_with_and_without_zz_phrases() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    for phrases in [false, true] {
        let run = |on: bool| {
            let coord = Coordinator::new_headless(wubi_pinyin(on), Some(&data_dir()));
            if phrases {
                coord.debug_install_phrases(zz_phrases());
            }
            press(&coord, "zhang");
            (coord.debug_input_buffer(), coord.debug_candidate_triples())
        };
        let on = run(true);
        assert_eq!(on.0, "zhang");
        assert!(
            on.1.iter().any(|(t, _, _)| t == "张"),
            "短语 {phrases}：zhang 应出「张」，实际 {:?}",
            on.1
        );
        assert_eq!(on, run(false), "短语 {phrases}：与关闭时相同");
    }
}

/// `azi` 撞车串：通配等长（`a?i` 的五笔字）在前，拼音精确「阿紫」随后，通配更长补全在其后。
#[test]
fn mixed_azi_wildcard_first_then_pinyin() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let off = mixed_triples(false, "azi");
    assert!(
        off.iter().any(|(t, _, _)| t == "阿紫"),
        "前置：关闭时 azi 应出拼音「阿紫」，实际 {off:?}"
    );
    let on = mixed_triples(true, "azi");
    let azi = on
        .iter()
        .position(|(t, _, _)| t == "阿紫")
        .unwrap_or_else(|| panic!("开启后「阿紫」仍在，实际 {on:?}"));
    assert_eq!(
        on.iter().filter(|(t, _, _)| t == "阿紫").count(),
        1,
        "拼音不重复"
    );
    let last_equal = on
        .iter()
        .rposition(|(_, c, _)| wubi_hit("azi", c, true))
        .unwrap_or_else(|| panic!("应有 a?i 五笔命中，实际 {on:?}"));
    assert!(last_equal < azi, "等长通配全部排在「阿紫」之前：{on:?}");
    if let Some(first_longer) = on.iter().position(|(_, c, _)| wubi_hit("azi", c, false)) {
        assert!(azi < first_longer, "通配更长补全排在拼音精确之后：{on:?}");
    }
}

/// ★ Review Focus 4：拼音分段续转的剩余串按字面续转拼音，与关闭时逐条相同。
/// `woaizi` 选「我」后剩 `aizi`（4 码、z 非首位）——这条才测得到续转规则；
/// spec 给的 `woaizhongguo` 剩余串超码长，走的是超码长规则，一并保留。
#[test]
fn mixed_segment_continuation_stays_literal() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    for (keys, rest) in [("woaizi", "aizi"), ("woaizhongguo", "aizhongguo")] {
        let run = |on: bool| {
            let coord = Coordinator::new_headless(wubi_pinyin(on), Some(&data_dir()));
            press(&coord, keys);
            let texts = coord.debug_all_candidate_texts();
            let wo = texts
                .iter()
                .position(|t| t == "我")
                .unwrap_or_else(|| panic!("前置：{keys} 应有部分候选「我」，实际 {texts:?}"));
            let _ = coord.select_candidate(wo);
            (coord.debug_input_buffer(), coord.debug_candidate_triples())
        };
        let on = run(true);
        assert_eq!(on.0, rest, "选「我」后剩余 {rest}");
        assert!(
            !on.1.iter().any(|(_, c, _)| wubi_hit(rest, c, true)),
            "续转态不出五笔通配命中：{:?}",
            on.1
        );
        assert_eq!(on, run(false), "{keys}：续转与关闭时相同");
    }
}

/// 开着通配、但串里没有通配键 ⇒ 与关闭时逐条相同（引擎放开 `wildcard_key` 不得波及字面路径）。
#[test]
fn mixed_without_wildcard_key_is_identical_to_off() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    for keys in ["wo", "xian", "aawt", "yijga"] {
        assert_eq!(
            mixed_triples(true, keys),
            mixed_triples(false, keys),
            "{keys}：无通配键时与关闭相同"
        );
    }
}
```

- [ ] **Step 2: 跑**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-coordinator --test codetable_wildcard -- --nocapture 2>&1 | grep -E "^test |test result|跳过|panicked"
```

Expected: 全绿、无「跳过」、耗时 ≥ 1s。核对点（真实数据相关，红了先查数据再查实现）：
- `mixed_azi_wildcard_first_then_pinyin` 的「更长补全在拼音精确之后」依赖「阿紫」落在拼音精确档（`is_pinyin_exact_tier` 要求 `is_common`）。若只有这一条断言红，用 `debug_candidate_triples` 打出「阿紫」前后的码核对它是否被检索范围判为非常用——是数据问题就停下回报控制者，不改断言。
- `mixed_segment_continuation_stays_literal` 的前置「我」来自超码长分支的部分候选（`pinyin_partial_candidates_overflow` 出厂 `true`）。前置红说明出厂值变了，回报控制者。

- [ ] **Step 3: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
test(coordinator): 五笔拼音混输下通配的真实数据用例

gz 出二码字；hanz 通配与拼音并存；hanzi / xianzai 与关闭时相同；zhang 首位字面
（装不装 zz* 短语各一次）；azi 通配等长在前、拼音精确随后、更长补全再后；分段续转
woaizi → aizi 与关闭时相同；无通配键的串与关闭时相同。替换原「混输不参与通配」用例
（spec codetable-wildcard §10）。
EOF
git show --stat HEAD
```

---

## Task 5: 设置说明、文档站、架构文档、spec 状态

**Files:**
- Modify: `../wind-setting/src/assets/settings_manifest.toml`（`key = "schema.codetable.wildcard"` 条目的 `hint` ~191）
- Modify（生成，若有变化）: `../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json`
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`（`### 通配输入` 小节 ~99-105 的列表）
- Modify: `docs/architecture/engine-candidate-pipeline.md`（§3.4 ~183-205）
- Modify: `docs/design/codetable-wildcard.md`（头部状态行 ~7）

**Interfaces:** 无代码接口。

- [ ] **Step 1: wind-setting hint**

把 hint 行末的 `五笔拼音混输方案不生效"` 改为：

```
五笔拼音混输方案下，首键和超出五笔码长的串按拼音处理，码长内通配结果与拼音候选一起列出"
```

（整行即：`hint = "组码时按通配键代替一个拿不准的码元，候选旁显示完整编码；通配结果不自动上屏、不顶字。首键是符号键时照常出标点，是字母键且已有功能（z 键引导、z 键重复、活码）时让位；组码中若是选词、翻页、分隔键也让位，满码后照常顶字。五笔拼音混输方案下，首键和超出五笔码长的串按拼音处理，码长内通配结果与拼音候选一起列出"`）

- [ ] **Step 2: 重生成产物、还原 appVersion、跑邻仓测试**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix-ws ./scripts/dev.sh sg
cd ../wind-setting
for f in src/assets/capabilities.snapshot.json src/mockdata/config.json; do
  old=$(git show HEAD:"$f" | grep -m1 '"appVersion"' || true)
  new=$(grep -m1 '"appVersion"' "$f" || true)
  if [ -n "$old" ] && [ "$old" != "$new" ]; then sed -i "s|$new|$old|" "$f"; fi
done
git diff --stat
cd ../WindInput && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix-ws ./scripts/dev.sh st
cd wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test -p wind-rpc --test wind_setting_assets
```

Expected: `git diff --stat` 只列 manifest，外加 snapshot / mockdata 中**因 hint 文本**产生的差异（若二者不含 hint，则只有 manifest 一个文件）；`st` 全绿；`wind_setting_assets` 全绿。label 未改，`pinyin_initials.txt` 不应变化——变了说明生成器取词范围与预期不符，停下回报。

- [ ] **Step 3: 文档站**

`codetable.mdx` `### 通配输入` 小节的列表里，把

```mdx
- 短语、整句不参与通配；五笔拼音混输方案、临时拼音、快捷输入里也不生效。结果最多 100 条。
```

替换为

```mdx
- 短语、整句不参与通配；临时拼音、快捷输入里也不生效。结果最多 100 条。
- **五笔拼音混输方案**：五笔码长以内（4 码）按通配查五笔、同时照常查拼音，两边候选一起列出，通配出的等长结果排在最前——如 `gz` 列出 `g` 开头的二码字，`hanz` 既有第 4 码任意的五笔字、也有拼音「汉字」。首键的 `z` 一律当拼音声母（`zhang`）；超出码长的串（`hanzi`、`xianzai`）和拼音分段上屏后剩下的部分都按原样当拼音，与关闭通配时相同。
```

Run:

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInputDocs && pnpm install --frozen-lockfile && pnpm lint
```

Expected: 通过。

- [ ] **Step 4: 架构文档 §3.4 与 spec 状态行**

`engine-candidate-pipeline.md` §3.4「**引擎**」一条里，把

```markdown
  上限 `WILDCARD_RESULT_LIMIT = 100`。混输有拼音子引擎时通配关闭（`wildcard_key()` 为 `None`），
  否则代理主码表。
```

（以文件实际两行为准，含「混输有拼音子引擎时通配关闭」的那句）替换为

```markdown
  上限 `WILDCARD_RESULT_LIMIT = 100`；结果带 `is_wildcard`，`source_tier` 把「通配 + 等长」放档 0。
  混输代理主码表的通配键；有拼音子引擎时 `MixedEngine::convert_wildcard` = 字面 `convert(input)`
  ⊕ 主码表通配，`merge_wildcard` 按「通配等长 → 字面 → 通配更长」合并、码表间 `(text, code)`
  去重、拼音 / 英文与码表同字即丢、带拼音保底截断。通配码长走 `Engine::wildcard_code_length`
  （混输取主码表码长；混输 `max_code_length` 仍为 0，免得改动短语自动上屏门槛）。
```

「**协调器**」一条末尾追加一个子项：

```markdown
  - 五笔拼音混输（`wildcard_mixes_pinyin`）另有三种整串字面：首位、整串超主码表码长
    （`wildcard_mixed_overflow`）、拼音分段续转（`wildcard_pattern_of`，与 `build_candidates`
    的续转判据同源 `last_seg_is_pinyin`；`build_candidates` 先判续转再判通配）。
```

`docs/design/codetable-wildcard.md` 头部状态行改为：

```markdown
> **状态：P1–P4 已实施；§10（混输调度）已实施**（提交见 git log --grep 通配）。分期见 §6，实施计划见 `codetable-wildcard-plan.md`、`codetable-wildcard-mixed-plan.md`。
```

实施中若有与 §10 不同之处（含本计划「spec 待澄清」一节被控制者裁决的条目），追加到 §9 末尾一条「2026-09-29 §10 实施后偏离」。

- [ ] **Step 5: 三个仓各自提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/wind-setting
git branch --show-current
git diff --cached --name-status
# 路径以 Step 2 `git diff --stat` 实际列出的为准；未变化的生成文件不要列
git commit -F - --only -- src/assets/settings_manifest.toml src/assets/capabilities.snapshot.json src/mockdata/config.json <<'EOF'
docs(settings): 通配输入说明改为五笔拼音混输下的实际行为

混输方案不再是「不生效」：首键与超码长的串按拼音，码长内通配与拼音并列。
EOF
git show --stat HEAD

cd ../WindInputDocs
git branch --show-current
git diff --cached --name-status
git commit -F - --only -- content/docs/settings/schema/codetable.mdx <<'EOF'
docs(settings): 通配输入补五笔拼音混输下的行为
EOF
git show --stat HEAD

cd ../WindInput
git branch --show-current
git diff --cached --name-status
git commit -F - --only -- docs/architecture/engine-candidate-pipeline.md docs/design/codetable-wildcard.md <<'EOF'
docs(design): 通配 §10 混输调度落地——架构文档 §3.4 与设计稿状态
EOF
git show --stat HEAD
```

---

## Task 6: 全量回归与编译门

**Files:** 无改动。

- [ ] **Step 1: 全量测试**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input
time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix TMPDIR=/tmp/wct-wcmix cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-wcmix/full.log | grep -E "^test result|FAILED|panicked" | tail -60
grep -E "^test result" /tmp/wct-wcmix/full.log | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: `failed` 与开工前基线相同（基线为 0 则为 0）；`passed` = 基线 + 本计划新增用例数（Task 1：3，Task 2：4 新增 − 1 删除，Task 3：3，Task 4：7 新增 − 1 删除，合计 +15）。

- [ ] **Step 2: 编译门**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix cargo check-headless
cd .. && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcmix ./scripts/dev.sh k
```

Expected: 均通过。

- [ ] **Step 3: 占位与提交范围自查**

```bash
cd /home/dufeng/develop/windinput/wt-wcmix/WindInput
base=$(git merge-base HEAD main)
git log --oneline "$base"..HEAD
git diff "$base"..HEAD --stat
git diff "$base"..HEAD | grep -nE "^\+.*(TODO|todo!|unimplemented!|#\[ignore\]|\.only\()" || echo clean
git diff "$base"..HEAD --name-only | grep -x "wind_input/Cargo.lock" && echo "!! Cargo.lock 被提交" || echo "lock ok"
```

Expected: `clean`、`lock ok`；`--stat` 只含本计划文件结构表里 WindInput 仓的那些文件（加本计划文件本身，若控制者另行提交）。

---

## spec 待澄清（写计划时对照真实代码发现）

1. **§10 分段续转的测试对照测不到续转规则**：`wo` 上屏后剩 `aizhongguo`（10 码）已超码长，走「超码长整串字面」即可通过，续转规则删掉也照绿。本计划另加 `woaizi` → `aizi`（Task 4）。续转判据的落点：`handle_candidate.rs:1181-1184`（原内联判据）→ `wildcard.rs` `last_seg_is_pinyin`。
2. **「通配等长与码表精确同档」在显示序里本就成立，真正会破的是调频**：`candidate_display_order` 里 `cmp_exact_first` 在 `source_tier` 之前（`wind-candidate/src/candidate.rs:898-899`），等长通配 `is_exact_code = true`、拼音候选恒 false，关调频时等长通配天然在最前。`source_tier` 只在 `freq_rerank::freq_tier`（`wind-engine/src/freq_rerank.rs:82`，调频首要键）与英文领位 `leads_as_exact`（`wind-coordinator/src/handle_candidate.rs:128`）起决定作用。本计划的档 0 改动因此同样作用于**纯码表**通配：此前开调频时等长与更长补全同在档 2，used-first 可把更长补全提到等长之前（违背 §3.1「等长优先」）；改后等长在档 0。需控制者确认这一顺带修正可接受。
3. **§10「字面串走原混输转换」与短语短路冲突**：协调器在有 pattern 时整体跳过短语（`handle_candidate.rs:1273`），混输码长内含非首位 `z` 的串（如用户短语 `dazi`）开通配后短语消失，而「原混输转换」字面上包含协调器合并的短语。本计划按 §3.1「短语不参与」保留短路，未让字面半边带短语；若要带，需另议短路粒度。
4. **§10「组合区显示原始缓冲」与高亮跟随**：本计划 `preedit_display = input`，但 `preedit_pinyin` 沿用字面结果（`MixedEngine::convert_wildcard`），高亮到拼音候选时组合区显示拼音拆分（如 `a'zi`），与关闭通配时高亮同一候选一致。若 §10 的本意是任何高亮下都显示原始缓冲，把 `preedit_pinyin` 置空即可。
5. **无拼音子引擎的混输（`secondary = None`）也受通配码长影响**：该情形此前 `active_max_code_length()` 为 0、「满码后字面」从不触发；改用 `wildcard_code_length` 后与纯码表一致（`aaaa` + `z` 顶字）。§10 未提，按 §3.3 满码规则处理。

---

## 自查

- **spec 覆盖**：§10 调度表——首位字面 → Task 3 `wildcard_lead_yields` + `mixed_lead_is_always_literal`、Task 4 `mixed_lead_z_is_literal_with_and_without_zz_phrases`；码长内两路同查 → Task 2 `convert_wildcard` / `merge_wildcard` + Task 4 `gz` / `hanz` / `azi`；超码长字面 → Task 3 `wildcard_mixed_overflow` + Task 4 `hanzi` / `xianzai`；分段续转字面 → Task 3 `wildcard_pattern_of` / `build_candidates` 次序 + Task 4 `woaizi` / `woaizhongguo`。合并规则：排序 → Task 1（档 0）+ Task 2（存活次序）+ Task 4 `azi`；去重 → Task 2 `wildcard_merge_drops_pinyin_same_text_keeps_codetable_codes`；上屏短路 → Task 3 顶字改 `wildcard_pattern_of`、`build_candidates` 各短路沿用 `wildcard_pattern`（已改为 `wildcard_pattern_of` 求得）；通配码长 → Task 2 访问器 + `max_code_length` 维持 0 的断言；记账 → `main_freq_code` 不变（Task 3 文档说明续转态无码表候选）；preedit → Task 2 `preedit_display = input`。落点：引擎 `wildcard_key` 代理（Task 3）、`convert_wildcard` 合并（Task 2）；协调器 `wildcard_pattern` 按位裁决、通配分流排在续转之后（Task 3）。测试对照七项 → Task 4 全部覆盖，外加「开关开但无通配键」对照。
- **签名一致性**：`wildcard_code_length(&self) -> usize` / `wildcard_mixes_pinyin(&self) -> bool` 在 trait（Task 2）、`MixedEngine`（Task 2）一致，`EngineManager::active_wildcard_code_length() -> usize` / `active_wildcard_mixes_pinyin() -> bool`（Task 2）在 Task 3 消费；`convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>` 未改签名；`merge_wildcard(Vec<Candidate>, Vec<Candidate>, usize) -> Vec<Candidate>` 仅 Task 2 内部；`last_seg_is_pinyin(state: &State) -> bool` 在 Task 3 定义、`build_candidates` 与 `wildcard_enters` / `wildcard_pattern_of` 共用；`wildcard_pattern_of(&self, state: &State) -> Option<String>` 在 Task 3 定义，`build_candidates` 与 `accumulate_code_char` 消费；`Candidate::is_wildcard` 在 Task 1 定义、Task 1 码表置位、Task 2 测试断言。
- **占位扫描**：全文无 TBD / TODO / 「类似任务 N」。无法完全确认的内部细节均写明核对点：Task 1 Step 4（穷举 `Candidate` 字面量补字段）、Task 2 Step 3（`max_code_len` 来源、`ConvertResult` 字段）、Task 3 Step 3（替换段前后的 `CandidateSource` 使用与 `via_mixed_pinyin` 求值次序）、Task 4 Step 2（「阿紫」常用字判定、超码长部分候选出厂值）、Task 5 Step 2（生成产物是否含 hint）。
- **已知取舍**：Task 1 的显示序用例与 Task 4 全部用例在实现前 / 首跑即绿，是锁与验收，已分别注明；Task 2 刻意不放开 `wildcard_key`，保证 Task 2 → Task 3 之间每次提交全绿。
