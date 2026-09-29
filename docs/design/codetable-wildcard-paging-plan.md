# 码表通配 · 常用字过滤与翻页扩充 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 通配结果在「智能」检索范围下整份算一组（有常用字就先滤掉生僻字），通配首批 100 条、翻到边界按现有机制 ×2 扩充到 5000；翻页扩容的「到底」判据对所有引擎改看引擎条数。

**Architecture:** 过滤：`wind-candidate/src/filter.rs` 的智能档分组键从 `(source, code)` 改成私有枚举 `SmartGroup`——`is_wildcard` 候选归 `SmartGroup::Wildcard(source)` 一组，其余仍是 `SmartGroup::Code(source, code)`（与原键逐字节同义），通配候选不往按码分组里写任何东西（含 `merged_codes`）。协调器 `apply_filter` 不改签名。数量：新增 `wind_engine::engine::CANDIDATE_LIMIT_CAP = 5000`，码表 `WILDCARD_RESULT_LIMIT` 改为它的别名，`convert_wildcard` 按 `max_candidates.min(上限)` 查且 0 不下传；`expand_candidates` 的 `.min(5000)` 改用同一常量。协调器 `update_candidates` 在 `wildcard_pattern_of(state).is_some()` 且非混输时首批取 `WILDCARD_INITIAL_LIMIT = 100`（混输维持 300）。到底判据：`expand_candidates` 由「可见条数未增」改为 `engine_count <= prev_limit`（「引擎条数未增」的等价写法，理由见 Task 2 Step 3 注释）。

**Tech Stack:** Rust workspace（wind-candidate / wind-engine / wind-coordinator）+ WindInputDocs 仓（文档站）

**Spec:** `docs/design/codetable-wildcard.md` **§11（常用字过滤与翻页扩充，binding）**；§3.1（结果数量）、§10（混输调度）为背景。

## Global Constraints

- §11 契约（逐条照做）：
  1. **过滤**：本串有通配位时，智能档把全部通配码表结果视为**一组**——只要有常用字，生僻字即被滤出；「常用字」档照旧全滤；被滤的仍经「翻到末页放宽」（`input.scope_relax.page_end_key`）追加在末尾。非通配路径分组不变。
  2. **数量与扩充**：去掉写死的 100；引擎按协调器给的 `max_candidates` 查，**硬上限 5000，与扩充上限对齐**；通配首批 **100** 条，使 `has_more = engine_count >= limit` 自然成立；翻页沿用 `expand_candidates`（×2、整份重查）。混输下主码表通配与字面同拿 `max_candidates`，由 `merge_wildcard` 统一截断、拼音保底配额（`PINYIN_QUOTA_DIVISOR = 5`）不变。
  3. **到底判据**：`expand_candidates` 由「可见条数未增」改判「引擎条数未增」，**对所有引擎生效**。
  4. **不做**：结果缓存；把过滤下推到词库。
  5. **已知**：扩充后同组常用字进入，智能档先前放行的生僻字会被滤走、前页内容可能移位——用例钉住现状（Task 1 `smart_wildcard_group_without_common_keeps_all_until_common_arrives`）。**调用方永不向 `search_pattern` 传 0**（Composite 把 0 当不限、各层把 0 当空）。
- 关闭通配 ⇒ 一切与现状逐键相同；开通配但串里没有通配位 ⇒ 分组、首批上限与关闭时相同（到底判据的改动对所有引擎生效，是契约 3 本意）。
- 提交纪律：禁止 `git add -A` / `git add .` / `git commit -a` / `git stash`；新文件先 `git add -- <path>`；提交一律 `git commit -F - --only -- <自己的路径…>`；提交前 `git diff --cached --name-status` 核对暂存区、提交后 `git show --stat HEAD` 核对文件数/行数与自己的改动量相符。**任何提交都不带 `wind_input/Cargo.lock`**。每次提交前 `git -C <仓> branch --show-current` 须为 `feat/wildcard-paging`。
- 路径：一切改动都在 `/home/dufeng/develop/windinput/wt-wcpage/{WindInput,WindInputDocs}` 下；**绝不**碰 `/home/dufeng/develop/windinput/{WindInput,wind-setting,WindInputDocs}`。wind-setting 本计划不改（见 Task 4 Step 1 的核对）。
- 格式化：禁止 `cargo fmt` / `cargo fmt --all`；只对自己改过的文件跑 `rustfmt --edition 2024 --check <file>`，其 diff 只落在自己 hunk 内才去掉 `--check` 执行，否则按 diff 手工采纳自己那几处。
- 测试环境：cargo 一律在 `wind_input/` 下、带 `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage`（先 `mkdir -p /tmp/wct-wcpage`；路径要短，别用 scratchpad）；全量跑加 `--no-fail-fast`。
- 已知偶发红：`handle_direct_aux::tests::schema_source_not_ready_is_noop_then_warms`（并发时序，与本计划无关）。全量里它红了，单独重跑一次；单跑绿即记为偶发，不改它。
- 集成测试依赖 `build_dev/data`，缺失时静默跳过而计数照绿——以 `--test codetable_wildcard` 耗时 ≥ 1s 且输出无「跳过」为数据在位判据。
- 日志：`info!` 不得含用户输入 / 候选。

## Review Focus

- **通配组不得漏进按码分组**：若只改查表键、`build_has_common` 仍把通配常用字的 `code` / `merged_codes` 写进 `(source, code)` 组，同码位的非通配生僻字会被一条通配常用字遮蔽，「非通配路径分组不变」就破了。规则：`is_wildcard` 候选只写 `SmartGroup::Wildcard`，`merged_codes` 循环只对非通配候选跑。测试：Task 1 `wildcard_group_does_not_leak_into_code_groups`。
- **到底判据看引擎条数，且是普通引擎的回归**：新一批全被检索范围滤掉时可见条数不变，旧判据当场判到底，后面的常用字永远翻不出来——这对普通码表同样成立，不是通配专属。规则：`expand_candidates` 以 `engine_count <= prev_limit` 判到底。测试：Task 2 `expand_stop_tests::expansion_continues_past_an_all_filtered_batch`（非通配自造码表）。
- **首批上限必须与引擎条数对得上，否则 `has_more` 恒假**：码表码长 ≥ 3 的首批上限是 1000（`handle_candidate.rs:1066-1069`），通配引擎原先至多回 100，`engine_count >= limit` 永不成立。规则：纯码表有 pattern 时首批固定 `WILDCARD_INITIAL_LIMIT = 100`；混输维持原首批 300（控制者裁决）。测试：Task 2 `pure_wildcard_first_batch_is_100_and_expands`（`azz`）。
- **硬上限单一来源、0 不下传**：扩容上限与通配硬上限各写一个 5000 的话，改一处另一处静默失配（扩容要 8000、引擎只回 5000 ⇒ `has_more` 恒假）；`max_candidates = 0` 若下传，Composite 当「不限」、各层当「空」。规则：`WILDCARD_RESULT_LIMIT = crate::engine::CANDIDATE_LIMIT_CAP`，`expand_candidates` 用同一常量；`limit == 0` 不调 `search_pattern`。测试：Task 2 `wildcard_hard_cap_matches_expansion_cap_and_zero_is_empty`。
- **混输首批缩小后，等长通配仍在最前、拼音仍有席位**：混输首批从 300 降到 100（有 pattern 即 100），字面半边只剩 `merge_wildcard` 的保底配额；扩充后整份重查、重新合并。规则：显示序靠 `cmp_exact_first` + `source_tier` 档 0（§10 已落地），截断靠 `truncate_with_pinyin_quota`。测试：Task 3 `mixed_expansion_keeps_equal_length_wildcard_first`。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `wind_input/crates/wind-candidate/src/filter.rs` | Modify | 私有 `SmartGroup` 分组键；`build_has_common` / `filter_smart` 改用它；单测 |
| `wind_input/crates/wind-engine/src/engine.rs` | Modify | 新增 `pub const CANDIDATE_LIMIT_CAP: usize = 5000` |
| `wind_input/crates/wind-engine/src/codetable/engine.rs` | Modify | `WILDCARD_RESULT_LIMIT` 改为 `CANDIDATE_LIMIT_CAP` 别名；`convert_wildcard` 按 `max_candidates` 查、0 不下传；改写 / 新增单测 |
| `wind_input/crates/wind-engine/src/mixed/engine.rs` | Modify | 仅加单测（混输主码表通配跟随 `max_candidates`） |
| `wind_input/crates/wind-coordinator/src/wildcard.rs` | Modify | `pub(crate) const WILDCARD_INITIAL_LIMIT: usize = 100` |
| `wind_input/crates/wind-coordinator/src/handle_candidate.rs` | Modify | `update_candidates` 首批上限分流；`expand_candidates` 到底判据与上限常量；文件末新增 `mod expand_stop_tests` |
| `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs` | Modify | §11 真实数据用例（纯码表 / 混输 / 常用字档 / 末页放宽）；改写 `leading_wildcard_when_unbound_scans_whole_table` |
| `../WindInputDocs/content/docs/settings/schema/codetable.mdx` | Modify | 「通配输入」小节的结果数量与过滤说明 |
| `../WindInputDocs/content/docs/settings/input/index.mdx` | Modify | 检索范围「智能」一行补通配例外 |
| `../WindInputDocs/content/docs/guides/config/input.mdx` | Modify | `filter_mode` 行补通配例外 |
| `docs/architecture/engine-candidate-pipeline.md` | Modify | §3.4 引擎上限与协调器首批 / 扩容；§8.1 通配分组例外 |
| `docs/design/codetable-wildcard.md` | Modify | 头部状态行；实施中若有偏离追加 §9 |

---

## 开工前：基线

- [ ] **Step 0: 记全量基线**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput && git status --short --branch && git log --oneline -1
cd wind_input && mkdir -p /tmp/wct-wcpage && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-wcpage/baseline.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: 分支 `feat/wildcard-paging`，工作区除本计划文件外干净（若出现 `M wind_input/Cargo.lock`，不属于本计划、永不提交）；记下 `passed=` / `failed=` 两个数（Task 5 对照用）。`failed` 非 0 时先把失败名单记下（`grep -E "^test .* FAILED|panicked" /tmp/wct-wcpage/baseline.log`），它们不属于本计划；已知偶发项见 Global Constraints。

---

## Task 1: 智能档通配结果整份一组

**Files:**
- Modify: `wind_input/crates/wind-candidate/src/filter.rs`（`build_has_common` ~210-240、`filter_smart` ~242-277、`mod tests` 末尾）
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（文件末尾新增一节）
- Test: 上述两文件

**Interfaces:**
- Consumes: `Candidate::is_wildcard: bool`（已有，只由 `CodeTableEngine::convert_wildcard` 置位，`wind-engine/src/codetable/engine.rs:927`）
- Produces（私有，仅本文件）:
  - `enum SmartGroup { Code(CandidateSource, String), Wildcard(CandidateSource) }`，`#[derive(Clone, PartialEq, Eq, Hash)]`
  - `fn SmartGroup::of(c: &Candidate) -> SmartGroup`
  - `fn build_has_common(candidates: &[Candidate]) -> HashMap<SmartGroup, bool>`（返回类型由 `(CandidateSource, String)` 键改为 `SmartGroup` 键）
- 公共签名不变：`pub fn filter_candidates(candidates: Vec<Candidate>, mode: FilterMode, phrase: RarePhrasePolicy) -> FilterOutcome`

**为什么用 `is_wildcard` 而不是把 pattern 传进 `apply_filter`**：一次 `build_candidates` 只有一个 pattern，「按 pattern 分组」≡「全部通配候选一组」；`is_wildcard` 已在候选上、只由通配入口置位，非通配路径上恒 false ⇒ 非通配候选的键 `Code(source, code)` 与原 `(source, code)` 一一对应，分组逐字节不变。传 pattern 要改 `filter_candidates` 公共签名与协调器 `apply_filter`，还得在混输里区分「哪些候选属于 pattern」——那正是 `is_wildcard` 已经回答的问题。

- [ ] **Step 1: 写失败测试**

`filter.rs` 的 `mod tests` 末尾（最后一个 `}` 之前）追加：

```rust
    fn wc(text: &str, code: &str, is_common: bool) -> Candidate {
        Candidate {
            is_wildcard: true,
            ..cand(text, code, CandidateSource::CodeTable, is_common)
        }
    }

    fn wc_texts(v: &[Candidate]) -> Vec<&str> {
        v.iter().map(|c| c.text.as_str()).collect()
    }

    /// spec codetable-wildcard §11：通配结果整份一组——有常用字就滤生僻字，不因散在不同码位
    /// 而各自当孤儿放行。现场：混输 `hanz` 首页全是 `han?` 下只有生僻字的码位。
    #[test]
    fn smart_treats_all_wildcard_results_as_one_group() {
        let out = filter_candidates(
            vec![
                wc("虑", "hand", true),
                wc("眓", "hanf", false),
                wc("虙", "hanm", false),
            ],
            FilterMode::Smart,
        );
        assert_eq!(wc_texts(&out.kept), ["虑"]);
        assert_eq!(
            wc_texts(&out.filtered),
            ["眓", "虙"],
            "被滤的仍进 filtered——末页放宽靠它追加回来"
        );
    }

    /// 通配组里一个常用字都没有 ⇒ 整组保底放行（与按码分组的孤儿码位同一条保底）。
    /// 这也是 spec §11「扩充后前页可能移位」的来源：后一批带进常用字，这些生僻字就被滤走。
    /// 本用例钉住现状（实现前即绿的那半是锁）。
    #[test]
    fn smart_wildcard_group_without_common_keeps_all_until_common_arrives() {
        let rare = vec![wc("眓", "hanf", false), wc("虙", "hanm", false)];
        assert_eq!(kept_of(rare.clone(), FilterMode::Smart).len(), 2, "无常用字：整组保底");
        let mut more = rare;
        more.push(wc("虑", "hand", true));
        assert_eq!(wc_texts(&kept_of(more, FilterMode::Smart)), ["虑"]);
    }

    /// ★ Review Focus 1：通配组与按码分组互不串。通配常用字的 `merged_codes` 不得往
    /// `(source, code)` 组里写，否则同码位的非通配生僻字（孤儿码位）会被它遮蔽；
    /// 拼音孤儿码位同样不受通配常用字影响。
    #[test]
    fn wildcard_group_does_not_leak_into_code_groups() {
        let mut common = wc("虑", "hand", true);
        common.merged_codes = vec!["hanf".into()];
        let out = kept_of(
            vec![
                common,
                cand("佢", "hanf", CandidateSource::CodeTable, false),
                cand("尪", "hanz", CandidateSource::Pinyin, false),
            ],
            FilterMode::Smart,
        );
        assert_eq!(wc_texts(&out), ["虑", "佢", "尪"]);
    }
```

`codetable_wildcard.rs` 文件末尾追加：

```rust
// ─────────────────────── 常用字过滤与翻页扩充（spec §11） ───────────────────────

/// 显式钉住检索范围档与词豁免档（`Config::default()` 的出厂档将来变了，语义会静默换掉）。
fn with_filter(mut cfg: Config, mode: &str) -> Config {
    cfg.input.filter_mode = mode.into();
    cfg.input.rare_phrase = "keep".into();
    cfg
}

fn triples_with(cfg: Config, keys: &str) -> Vec<(String, String, String)> {
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, keys);
    coord.debug_candidate_triples()
}

/// spec §11 契约 1：智能档把通配结果当一组——有常用字就滤掉生僻字，与「常用字」档结果相同。
/// 旧实现按 `(来源, 码)` 分组，`han?` / `a??` 下只含生僻字的码位当孤儿放行。
/// 对照：「全部字符」档条数更多，证明这些输入下首批确有生僻字可滤。
#[test]
fn pure_wildcard_smart_filters_like_general() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    for keys in ["hanz", "azz"] {
        let all = triples_with(with_filter(wubi(true, "z"), "gb18030"), keys);
        let general = triples_with(with_filter(wubi(true, "z"), "general"), keys);
        let smart = triples_with(with_filter(wubi(true, "z"), "smart"), keys);
        assert!(
            all.len() > general.len(),
            "前置：{keys} 首批应有生僻字（全部 {} / 常用 {}）",
            all.len(),
            general.len()
        );
        assert_eq!(smart, general, "{keys}: 智能档的通配结果应与常用字档相同");
    }
}
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input && mkdir -p /tmp/wct-wcpage
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-candidate --lib filter::tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_wildcard pure_wildcard_smart_filters_like_general
```

Expected: `smart_treats_all_wildcard_results_as_one_group` 红（kept 是 `["虑","眓","虙"]`）、`wildcard_group_does_not_leak_into_code_groups` 红（「佢」被 `merged_codes` 遮蔽）、`smart_wildcard_group_without_common_keeps_all_until_common_arrives` 红在第二个断言；`pure_wildcard_smart_filters_like_general` 红在 `assert_eq!(smart, general)`（`hanz` 智能 9 条 vs 常用 4 条量级）。

核对点：若 `pure_wildcard_smart_filters_like_general` 红在「前置」断言（某个 keys 下全部 == 常用），说明该串首批恰好无生僻字——`hanz` 读词库确认有 5 个生僻单字（`build_dev/data/schemas/wubi86/wubi86_jidian*.dict.yaml` 里 `han?` 共 9 条），`azz` 是按权重估算的前 100 条里约 6 个；`azz` 不满足时换 `gzz`，仍不满足则删掉该项只留 `hanz`，并在报告里注明。

- [ ] **Step 3: 最小实现**

`filter.rs`：把 `build_has_common` 整个函数（含文档注释）替换为：

```rust
/// 智能档的分组键：按 `(来源, 码)`，通配结果除外。
///
/// **必须带来源**：混输下码表（五笔码）与拼音候选常共用同一 code 字符串（原始输入，如
/// "wang"），但属不同编码体系；若仅按 code 分组，常用的拼音候选会误使同 code 的生僻码表字
/// （如 佢）被过滤，导致混输码表主方案与纯五笔表现不一致。
///
/// **通配结果（[`Candidate::is_wildcard`]）整份一组**（spec `codetable-wildcard.md` §11）：
/// 用户没有打出那些码，按码分组时「某码下只有生僻字」会被当孤儿码位放行，首页于是全是
/// 生僻字。一次构建只有一个 pattern，故「按 pattern 分组」＝「通配结果一组」，不必把
/// pattern 传进来。`is_wildcard` 只由码表通配入口置位，非通配候选的键与原 `(source, code)`
/// 一一对应——非通配路径的分组逐字节不变。
#[derive(Clone, PartialEq, Eq, Hash)]
enum SmartGroup {
    Code(CandidateSource, String),
    Wildcard(CandidateSource),
}

impl SmartGroup {
    fn of(c: &Candidate) -> Self {
        if c.is_wildcard {
            Self::Wildcard(c.source)
        } else {
            Self::Code(c.source, c.code.clone())
        }
    }
}

/// 统计每个 [`SmartGroup`]「是否存在常用词」。
fn build_has_common(candidates: &[Candidate]) -> std::collections::HashMap<SmartGroup, bool> {
    use std::collections::HashMap;
    let mut has_common: HashMap<SmartGroup, bool> = HashMap::new();
    for c in candidates {
        let group = SmartGroup::of(c);
        // 先建组（哪怕非常用），使「该码位下无常用词」与「该码位没出现过」区分开。
        has_common.entry(group.clone()).or_insert(false);
        if !is_common_like(c) {
            continue;
        }
        has_common.insert(group, true);
        // ⚠️ 通配候选只写自己那一组：它的码位（含 merged_codes）若写进按码分组，同码位的
        // 非通配生僻字会被一条通配常用字遮蔽，「非通配分组不变」就破了。
        if c.is_wildcard {
            continue;
        }
        // 去重吃掉的同文本码位一并遮蔽（见 `Candidate::merged_codes`）：「档」以简码 siv 命中时，
        // 它在 sivg 的那条已被去重丢弃，若不还原这层归属，sivg 组只剩生僻的「桜」而当孤儿码
        // 放行 —— 同一个字打 siv 出、打全 sivg 反而不出。非常用候选无需还原：它不遮蔽任何人。
        for code in &c.merged_codes {
            has_common.insert(SmartGroup::Code(c.source, code.clone()), true);
        }
    }
    has_common
}
```

`filter_smart` 的文档行 `/// 智能过滤：同一来源+编码下有常用词则过滤非常用词` 改为
`/// 智能过滤：同一组（来源+编码；通配结果整份一组，见 [`SmartGroup`]）下有常用词则过滤非常用词`；
其中的查表：

```rust
        let common_exists = has_common
            .get(&(c.source, c.code.clone()))
            .copied()
            .unwrap_or(false);
        // 同来源同编码下存在常用词则只保留常用词；否则保留全部（孤儿编码）
```

改为：

```rust
        let common_exists = has_common
            .get(&SmartGroup::of(c))
            .copied()
            .unwrap_or(false);
        // 同组存在常用词则只保留常用词；否则保留全部（孤儿编码 / 全无常用字的通配结果）
```

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-candidate --lib filter::tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_wildcard --test codetable_filter_scope_consistency --test rare_phrase_scope --test common_chars_override --no-fail-fast
```

Expected: 全绿；`codetable_wildcard` 耗时 ≥ 1s 且无「跳过」。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-candidate/src/filter.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-candidate/src/filter.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
fix(filter): 智能档把通配结果整份算一组，有常用字就先滤生僻字

通配结果散在几十个码位上，按 (来源, 码) 分组时许多码位只有生僻字，被当孤儿码位放行，
混输打 hanz 首页全是生僻字（spec codetable-wildcard §11）。用户并没打出那些码，按码
分组对通配没有意义。新增私有分组键 SmartGroup：is_wildcard 候选归一组，其余仍按
(来源, 码)，非通配路径分组逐字节不变；通配候选也不往按码分组里写 merged_codes。
EOF
git show --stat HEAD
```

---

## Task 2: 引擎按 `max_candidates` 查、通配首批 100、到底判据改看引擎条数

> **控制者裁决（2026-09-29）**：首批 100 只作用于**纯码表**；混输维持原首批 300（`EngineManager::active_wildcard_mixes_pinyin()` 为真时走原 `initial_candidate_limit`）。混输相关断言按首批 300 写；若某条计划用例假设混输首批 100，改按 300 并在报告说明。

**Files:**
- Modify: `wind_input/crates/wind-engine/src/engine.rs`（`ConvertResult` 之前加常量）
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`WILDCARD_RESULT_LIMIT` ~90-91；`convert_wildcard` ~903-941；测试 `wildcard_respects_single_code_input_and_result_cap` ~2472-2505；`mod tests` 末尾）
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（`mod tests` 末尾，仅测试）
- Modify: `wind_input/crates/wind-coordinator/src/wildcard.rs`（`WildcardDecision` 之前加常量）
- Modify: `wind_input/crates/wind-coordinator/src/handle_candidate.rs`（`update_candidates` ~2263；`expand_candidates` ~2383-2413；文件末尾新增测试模块）
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（§11 一节追加）

**Interfaces:**
- Produces:
  - `pub const CANDIDATE_LIMIT_CAP: usize = 5000;`（`wind_engine::engine`）
  - `pub const WILDCARD_RESULT_LIMIT: usize = crate::engine::CANDIDATE_LIMIT_CAP;`（`wind_engine::codetable::engine`，值 100 → 5000）
  - `pub(crate) const WILDCARD_INITIAL_LIMIT: usize = 100;`（`wind_coordinator::wildcard`）
- 签名不变：`fn convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>`；`pub(crate) fn expand_candidates(&self, state: &mut State)`；`pub(crate) fn update_candidates(&self, state: &mut State) -> InputOutcome`
- Consumes: `Coordinator::wildcard_pattern_of(&self, state: &State) -> Option<String>`（已有，`wildcard.rs:222`）

- [ ] **Step 1: 写失败测试**

`codetable/engine.rs`：把测试 `wildcard_respects_single_code_input_and_result_cap` 的文档行 `/// §3.1：精确匹配模式下不追加更长编码；上限常量 100。` 改为 `/// §3.1 / §11：精确匹配模式下不追加更长编码；条数跟随 max_candidates（不再写死 100）。`，函数名改为 `wildcard_respects_single_code_input_and_max_candidates`，函数体末尾那段

```rust
        let e = engine_opts(&refs, wildcard_opts(CommitOptions::default()));
        let r = e
            .convert_wildcard("qzz", &slot_pattern("q??"), 1000)
            .unwrap();
        assert_eq!(
            r.candidates.len(),
            WILDCARD_RESULT_LIMIT,
            "结果上限是常量 100"
        );
```

替换为：

```rust
        let e = engine_opts(&refs, wildcard_opts(CommitOptions::default()));
        let r = e
            .convert_wildcard("qzz", &slot_pattern("q??"), 1000)
            .unwrap();
        assert_eq!(r.candidates.len(), 150, "要 1000 给全部 150 条（旧实现截在 100）");
        let r = e
            .convert_wildcard("qzz", &slot_pattern("q??"), 120)
            .unwrap();
        assert_eq!(r.candidates.len(), 120, "按协调器给的 max_candidates 截");
```

`codetable/engine.rs` 的 `mod tests` 末尾追加：

```rust
    /// ★ Review Focus 4：通配硬上限与翻页扩容上限是**同一个**常量；`max_candidates = 0` 回空，
    /// 且不把 0 传给 `search_pattern`（Composite 把 0 当不限、各层把 0 当空）。
    #[test]
    fn wildcard_hard_cap_matches_expansion_cap_and_zero_is_empty() {
        assert_eq!(WILDCARD_RESULT_LIMIT, crate::engine::CANDIDATE_LIMIT_CAP);
        let n = WILDCARD_RESULT_LIMIT + 100;
        let many: Vec<(String, String, i32)> = (0..n as u32)
            .map(|i| {
                let c1 = (b'a' + (i / 676) as u8) as char;
                let c2 = (b'a' + (i / 26 % 26) as u8) as char;
                let c3 = (b'a' + (i % 26) as u8) as char;
                (format!("{c1}{c2}{c3}"), format!("字{i}"), 1)
            })
            .collect();
        let refs: Vec<(&str, &str, i32)> = many
            .iter()
            .map(|(c, t, w)| (c.as_str(), t.as_str(), *w))
            .collect();
        let e = engine_opts(&refs, wildcard_opts(CommitOptions::default()));
        let r = e
            .convert_wildcard("zzz", &slot_pattern("???"), usize::MAX)
            .unwrap();
        assert_eq!(r.candidates.len(), WILDCARD_RESULT_LIMIT, "硬上限兜底");
        let r0 = e
            .convert_wildcard("zzz", &slot_pattern("???"), 0)
            .unwrap();
        assert!(r0.candidates.is_empty() && r0.is_empty, "max 0 ⇒ 空结果");
    }
```

`mixed/engine.rs` 的 `mod tests` 末尾追加：

```rust
    /// spec §11：混输下主码表通配按 `max_candidates` 查，不再被主码表内部的固定 100 截断。
    #[test]
    fn wildcard_primary_follows_max_candidates() {
        let many: Vec<(String, String, i32)> = (0..150u32)
            .map(|i| {
                let c1 = (b'a' + (i / 26) as u8) as char;
                let c2 = (b'a' + (i % 26) as u8) as char;
                (format!("q{c1}{c2}"), format!("字{i}"), 1)
            })
            .collect();
        let refs: Vec<(&str, &str, i32)> = many
            .iter()
            .map(|(c, t, w)| (c.as_str(), t.as_str(), *w))
            .collect();
        let e = mixed_wc(&refs, vec![("qzz", "阿紫")]);
        let r = e.convert_wildcard("qzz", &slot("q??"), 300).unwrap();
        let ct = r
            .candidates
            .iter()
            .filter(|c| c.source == CandidateSource::CodeTable)
            .count();
        assert_eq!(ct, 150, "主码表通配全部进来（旧实现截在 100）");
        assert!(r.candidates.iter().any(|c| c.text == "阿紫"), "字面半边照常合并");
    }
```

`handle_candidate.rs` 文件末尾追加：

```rust
#[cfg(test)]
mod expand_stop_tests {
    //! 翻页扩容的「到底」判据（spec codetable-wildcard §11 契约 3）：看**引擎条数**，不看可见
    //! 条数。自造码表、**非通配**——这条判据对所有引擎生效，通配只是更容易撞上。
    use crate::charset_test_support::copy_factory_charsets;
    use crate::coordinator::Coordinator;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use wind_bridge::handler::{KeyEventData, MessageHandler};
    use wind_config::Config;
    use wind_ipc::protocol::EVENT_KEY_DOWN;

    const VK_A: u32 = 0x41;
    const VK_NEXT: u32 = 0x22; // PageDown，出厂翻页键组 "pageupdown"

    /// 清掉夹具目录与码表方案在共享缓存根下的产物（同 `handle_aux_code` 的 `Cleanup`）。
    struct Cleanup {
        id: String,
        dir: PathBuf,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(cache) = Config::cache_dir() {
                let _ = std::fs::remove_dir_all(cache.join(&self.id));
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 常用 / 生僻各取 `n` 个真实汉字：常用取出厂 `common_han.yaml` 名单里 CJK 基本区的头部，
    /// 生僻取 U+4E00 起**不在名单里**的字（该类 `outside: rare`：汉字域内名单外即生僻）。
    fn common_and_rare(dir: &Path, n: usize) -> (Vec<char>, Vec<char>) {
        let yaml = std::fs::read_to_string(dir.join("charsets/common_han.yaml")).unwrap();
        let body = yaml.split("\n...\n").nth(1).expect("common_han.yaml 缺文档体");
        let listed: Vec<char> = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
            .filter_map(|l| {
                let mut cs = l.chars();
                let c = cs.next()?;
                cs.next().is_none().then_some(c)
            })
            .collect();
        let set: HashSet<char> = listed.iter().copied().collect();
        let common: Vec<char> = listed
            .into_iter()
            .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
            .take(n)
            .collect();
        let rare: Vec<char> = (0x4E00u32..=0x9FFF)
            .filter_map(char::from_u32)
            .filter(|c| !set.contains(c))
            .take(n)
            .collect();
        assert_eq!((common.len(), rare.len()), (n, n), "前置：字表够取");
        (common, rare)
    }

    /// 码表 `a` 前缀下三段（按权重降序）：
    /// - `ab` 99 个常用字 + `ac` 1 个常用字 → 首批 100 条全可见；
    /// - `ac` 150 个生僻字 → 与上面那个常用字同组，智能档全滤（第 2 批引擎 +100、可见 +0）；
    /// - `ad` 50 个常用字 → 第 3 批才出现。
    fn coord(tag: &str) -> (Arc<Coordinator>, Cleanup) {
        let id = format!("zz_expand_{tag}_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!(
            "wind_expand_stop_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let guard = Cleanup {
            id: id.clone(),
            dir: dir.clone(),
        };
        copy_factory_charsets(&dir);
        let (common, rare) = common_and_rare(&dir, 150);
        let mut dict = String::new();
        for (i, ch) in common[..99].iter().enumerate() {
            dict += &format!("ab\t{ch}\t{}\n", 10_000 - i);
        }
        dict += &format!("ac\t{}\t9800\n", common[99]);
        for (i, ch) in rare.iter().enumerate() {
            dict += &format!("ac\t{ch}\t{}\n", 5_000 - i);
        }
        for (i, ch) in common[100..150].iter().enumerate() {
            dict += &format!("ad\t{ch}\t{}\n", 100 - i);
        }
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(schemas.join(&id)).unwrap();
        std::fs::write(
            schemas.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"扩容\"\n[engine]\ntype = \"codetable\"\n\
                 [engine.codetable]\nmax_code_length = 4\n\
                 [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/{id}.dict.yaml\"\n\
                 type = \"rime_codetable\"\ndefault = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            schemas.join(format!("{id}/{id}.dict.yaml")),
            format!(
                "---\nname: {id}\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n{dict}"
            ),
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.available = vec![id.clone()];
        cfg.schema.active = id;
        cfg.input.default.chinese_mode = true;
        cfg.input.filter_mode = "smart".into();
        cfg.input.rare_phrase = "keep".into();
        // 关掉末页放宽：本用例只看扩容，不许「翻到底再按一次」把被滤的生僻字追加回来。
        cfg.input.scope_relax.page_end_key = false;
        (Coordinator::new_headless(cfg, Some(&dir)), guard)
    }

    fn press(c: &Coordinator, vk: u32) {
        c.handle_key_event(&KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: 0,
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        });
    }

    /// 一路往后翻到「末页且不再有更多」为止（不多按：末页多按一下就是放宽意图）。
    fn page_to_last(c: &Coordinator) {
        for _ in 0..200 {
            let (cur, _, total) = c.debug_page_info();
            if cur + 1 >= total && !c.debug_has_more() {
                return;
            }
            press(c, VK_NEXT);
        }
        panic!("翻页未收敛");
    }

    /// ★ Review Focus 2：第 2 批 100 条全被检索范围滤掉（可见条数不变），扩容不得就此判到底——
    /// 引擎条数涨了，第 3 批还带着常用字。旧判据「可见条数未增」在这里停在 100 条。
    #[test]
    fn expansion_continues_past_an_all_filtered_batch() {
        let (c, _guard) = coord("allfiltered");
        press(&c, VK_A);
        assert_eq!(c.debug_candidate_count(), 100, "前置：首批 100 条全是常用字");
        assert!(c.debug_has_more(), "前置：引擎回满首批上限（码表单码 100）");
        page_to_last(&c);
        assert_eq!(
            c.debug_candidate_count(),
            150,
            "第 2 批全被滤，第 3 批带出 ad 下 50 个常用字"
        );
        assert!(!c.debug_has_more(), "引擎 300 条 < 上限 400，到底");
    }
}
```

`codetable_wildcard.rs` 的 §11 一节追加：

```rust
const VK_NEXT: u32 = 0x22; // PageDown，出厂翻页键组 "pageupdown"

/// 关掉末页放宽：翻页类用例只看扩容，末页多按一下的放宽会把被滤的字追加回来、把条数弄乱。
fn no_relax(mut cfg: Config) -> Config {
    cfg.input.scope_relax.page_end_key = false;
    cfg
}

/// 往后翻，直到候选总数超过 `than`，或已在末页且不再有更多（此时不再按，免得触发放宽）。
fn page_until_more_than(coord: &Coordinator, than: usize) -> usize {
    for _ in 0..400 {
        let n = coord.debug_candidate_count();
        if n > than {
            return n;
        }
        let (cur, _, total) = coord.debug_page_info();
        if cur + 1 >= total && !coord.debug_has_more() {
            return n;
        }
        press_vk(coord, VK_NEXT, false);
    }
    coord.debug_candidate_count()
}

/// ★ Review Focus 3：通配首批至多 100 条且 `has_more` 为真，翻到边界按 ×2 扩充。
/// 旧实现码长 ≥ 3 首批上限 1000、引擎却至多回 100 ⇒ `has_more` 恒假，永不扩充。
#[test]
fn pure_wildcard_first_batch_is_100_and_expands() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(no_relax(wubi(true, "z")), Some(&data_dir()));
    press(&coord, "azz");
    let first = coord.debug_candidate_count();
    assert!(first > 0 && first <= 100, "首批至多 100 条：{first}");
    assert!(coord.debug_has_more(), "a?? 远超 100 条，首批应标记还有更多");
    let grown = page_until_more_than(&coord, first);
    assert!(grown > first, "翻到边界应扩充：{first} -> {grown}");
}
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-engine --lib wildcard
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --lib expand_stop_tests
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_wildcard pure_wildcard_first_batch_is_100_and_expands
```

Expected: wind-engine 编译失败 `error[E0425]: cannot find value \`CANDIDATE_LIMIT_CAP\` in module \`crate::engine\``（先把新测试里这一行注释掉可看到另外两条红：`wildcard_respects_single_code_input_and_max_candidates` 得 100 ≠ 150、`wildcard_primary_follows_max_candidates` 得 100 ≠ 150；看完恢复）；`expansion_continues_past_an_all_filtered_batch` 红在 `left: 100, right: 150`；`pure_wildcard_first_batch_is_100_and_expands` 红在 `has_more`。

核对点（夹具）：若 `expansion_continues_past_an_all_filtered_batch` 红在**前置**断言（首批 ≠ 100），说明自造码表的前缀查询没按权重取前 100，或 `copy_factory_charsets` 后常用字判定未生效（全判常用 ⇒ 第 2 批不被滤、旧实现也会绿）。排查：临时在用例里打印 `c.debug_all_candidate_texts()` 的前 5 条与 `c.debug_common_char_mark(0)`；前者应是 `common[0..]`，后者应为 `Some((_, true))`。不改判据去迁就夹具。

- [ ] **Step 3: 实现**

`wind-engine/src/engine.rs`，`/// 引擎转换结果` 那行之前加：

```rust
/// 协调器向引擎要候选的**上限的上限**：翻页扩容（`expand_candidates`）翻倍到此为止，
/// 码表通配的硬上限（`codetable::engine::WILDCARD_RESULT_LIMIT`）与之是同一个值
/// （spec codetable-wildcard §11）。两处各写一个 5000 的话，改一处另一处静默失配：
/// 扩容要 8000、引擎只回 5000 ⇒ `has_more` 恒假。
pub const CANDIDATE_LIMIT_CAP: usize = 5000;
```

`codetable/engine.rs`：

```rust
/// 通配结果上限（spec §3.1：常量，不开放配置）。首位即通配会退化成全表扫描，靠它兜底。
pub const WILDCARD_RESULT_LIMIT: usize = 100;
```

替换为：

```rust
/// 通配结果**硬上限**（spec §3.1 / §11：常量、不开放配置，与翻页扩容上限是同一个值）。
/// 平常由协调器给的 `max_candidates` 决定条数（首批 100，翻页 ×2）；首位即通配退化成
/// 全表扫描时靠它兜底。
pub const WILDCARD_RESULT_LIMIT: usize = crate::engine::CANDIDATE_LIMIT_CAP;
```

`convert_wildcard` 里从 `let mut candidates: Vec<Candidate> = self` 到 `.collect();`（含 `.dm.search_pattern(…WILDCARD_RESULT_LIMIT…)` 与 `.map` 闭包）整段替换为：

```rust
        // 按协调器给的上限查（spec §11），硬上限兜底。⚠️ 永不向 `search_pattern` 传 0：
        // Composite 把 0 当「不限」、各层把 0 当「空」，两边语义不一致。
        let limit = max_candidates.min(WILDCARD_RESULT_LIMIT);
        let hits = if limit == 0 {
            Vec::new()
        } else {
            self.dm
                .search_pattern(pattern, wind_dict::WILDCARD_SLOT, limit, with_prefix)
        };
        let mut candidates: Vec<Candidate> = hits
            .into_iter()
            .map(|mut c| {
                c.source = CandidateSource::CodeTable;
                c.is_exact_code = c.code.chars().count() == n;
                c.comment = c.code.clone();
                c.is_wildcard = true;
                c
            })
            .collect();
```

同函数的 `candidates.truncate(max_candidates.min(WILDCARD_RESULT_LIMIT));` 改为 `candidates.truncate(limit);`。

`wind-coordinator/src/wildcard.rs`，`/// 通配键此刻的去向。` 之前加：

```rust
/// 通配组码的首批候选上限（spec §11）。不走 `initial_candidate_limit` 的码长分级
/// （码表 1/2/≥3 码 → 100/300/1000）：那套是给前缀补全配的，通配首位即退化成全表扫描；
/// 首批小、翻到边界再由 `expand_candidates` ×2 扩，`has_more = engine_count >= limit`
/// 才自然成立（旧实现首批 1000、引擎至多回 100 ⇒ `has_more` 恒假）。
pub(crate) const WILDCARD_INITIAL_LIMIT: usize = 100;
```

`handle_candidate.rs` 的 `update_candidates`：

```rust
        let limit = self.initial_candidate_limit(&state.input_buffer);
        let (engine_count, outcome) = self.build_candidates(state, limit);
```

改为：

```rust
        // 纯码表通配组码首批固定 100 条（spec codetable-wildcard §11），见 `WILDCARD_INITIAL_LIMIT`；
        // 混输维持原首批（控制者裁决：缩到 100 会让拼音保底只剩 20 席）。
        let limit = if self.wildcard_pattern_of(state).is_some()
            && !self.engine_mgr.active_wildcard_mixes_pinyin()
        {
            crate::wildcard::WILDCARD_INITIAL_LIMIT
        } else {
            self.initial_candidate_limit(&state.input_buffer)
        };
        let (engine_count, outcome) = self.build_candidates(state, limit);
```

`expand_candidates`：文档行 `/// 扩展候选（翻页/下移到边界时调用）：上限翻倍（≤5000）重新加载，保持当前页/高亮。` 改为
`/// 扩展候选（翻页/下移到边界时调用）：上限翻倍（≤ [`wind_engine::engine::CANDIDATE_LIMIT_CAP`]）重新加载，保持当前页/高亮。`；
函数体从 `let new_limit = …` 到 `state.has_more = engine_count >= new_limit;` 替换为：

```rust
        let new_limit = (state.candidate_limit.saturating_mul(2))
            .min(wind_engine::engine::CANDIDATE_LIMIT_CAP);
        if new_limit <= state.candidate_limit {
            state.has_more = false;
            return;
        }
        let prev_limit = state.candidate_limit;
        // 翻页扩展不消费全码自动上屏（仅正向输入字母时才上屏）。
        let (engine_count, _) = self.build_candidates(state, new_limit);
        // 重建后立刻重新展开变体（列表整份重建，变体须随之重展）。
        self.expand_s2t_variants(state);
        // ★ 到底判据看**引擎条数**，不看可见条数（spec codetable-wildcard §11 契约 3，对所有
        // 引擎生效）：新增的一批若全被检索范围滤掉，可见条数不变，而引擎后面可能还有常用字
        // ——按可见条数判会在这里误判到底，后面的字永远翻不出来。
        // 「引擎条数未增」写成 `engine_count <= prev_limit`：`has_more` 只在
        // `engine_count >= limit` 时置真，故上一批引擎条数 ≥ prev_limit；这次没超过它，
        // 引擎就没给出新东西。不另存上一批条数，免得多一个要跟着各处复位的状态位。
        if engine_count <= prev_limit {
            state.has_more = false;
            return;
        }
        state.candidate_limit = new_limit;
        state.has_more = engine_count >= new_limit;
```

（其后 `// 保持当前页/高亮不变…` 与 `self.sync_preedit_to_highlight(state);` 原样保留；原 `let prev_len = state.candidates.len();` 与 `if state.candidates.len() <= prev_len { … }` 一并删除。）

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-engine --lib wildcard
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --lib expand_stop_tests handle_direct_aux handle_aux_code
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_wildcard --test input_flow --test codetable_filter_scope_consistency --test temp_pinyin_candidate_limit --no-fail-fast
```

Expected: 全绿（`handle_direct_aux::tests::schema_source_not_ready_is_noop_then_warms` 偶发红按 Global Constraints 处理）。重点看：`input_flow` 的 `test_dynamic_paging_expands_candidates`、`handle_direct_aux::tests::hits_survive_page_expansion`、`handle_aux_code::tests::arrow_navigation_does_not_lose_filter`（三者都走 `expand_candidates`）；`codetable_wildcard` 里 §10 的混输用例（`mixed_hanz_merges_wildcard_and_pinyin` / `mixed_azi_wildcard_first_then_pinyin` 等）在混输首批从 300 缩到 100 后仍绿。

核对点：若 §10 某条混输用例红在「拼音候选不在」，是首批缩小后字面半边只剩保底配额（100 / 5 = 20 条）所致——确认红因后回报，不改配额（契约 2：配额不变）。若任一引擎在某输入下 `engine_count > limit`（判据的等价前提被破坏），表现是扩容多跑一轮后才判到底，不会误判到底；无需处理，但在报告里注明是哪个引擎。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
fix(wildcard): 通配结果首批 100 条、翻页扩充到 5000；扩容到底改看引擎条数

引擎 search_pattern 的 limit 写死 100 且先截断后过滤，而首批上限按码长给 300/1000，
has_more 恒假、翻页永不扩充（spec codetable-wildcard §11）。改为按协调器给的
max_candidates 查，硬上限 CANDIDATE_LIMIT_CAP 与扩容上限同一常量，0 不下传；有通配
pattern 时首批固定 100。

expand_candidates 原先「可见条数未增」即判到底：新一批全被检索范围滤掉时后面的常用字
再也翻不出来。改判引擎条数（engine_count <= 上一批上限），对所有引擎生效，附非通配
码表的回归用例。
EOF
git show --stat HEAD
```

---

## Task 3: 真实数据验收（纯码表 / 混输 / 常用字档 / 末页放宽）

> **控制者裁决（2026-09-29）**：首批 100 只作用于**纯码表**；混输维持原首批 300（`EngineManager::active_wildcard_mixes_pinyin()` 为真时走原 `initial_candidate_limit`）。混输相关断言按首批 300 写；若某条计划用例假设混输首批 100，改按 300 并在报告说明。

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（改写 `leading_wildcard_when_unbound_scans_whole_table` ~596-611；§11 一节追加）

**Interfaces:**
- Consumes（Task 1/2 已加的测试助手）: `with_filter(Config, &str) -> Config`、`triples_with(Config, &str) -> Vec<(String, String, String)>`、`no_relax(Config) -> Config`、`page_until_more_than(&Coordinator, usize) -> usize`；文件既有 `wubi(bool, &str) -> Config`、`wubi_pinyin(bool) -> Config`、`wubi_hit(&str, &str, bool) -> bool`、`mixed_ready() -> bool`、`press` / `press_vk`。
- Produces（测试内）: `fn wc_hits(pattern: &str, tri: &[(String, String, String)]) -> Vec<(String, String)>`

本任务用例在 Task 1/2 之后**首跑即绿**，是验收与锁：每条都注明它在哪个实现缺失时会红。

- [ ] **Step 1: 改写首位全表扫描用例**

`leading_wildcard_when_unbound_scans_whole_table` 整个函数（含文档注释）替换为：

```rust
/// 首位无任何绑定时首位即可通配（退化全表扫描）：首批至多 100 条，翻到边界照常扩充
/// （spec §11；旧实现写死 100 条，扩容后可见条数不变即判到底）。
#[test]
fn leading_wildcard_when_unbound_scans_whole_table() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(no_relax(wubi(true, "z")), Some(&data_dir()));
    press(&coord, "z");
    let tri = coord.debug_candidate_triples();
    assert!(
        !tri.is_empty() && tri.len() <= 100,
        "首批至多 100 条：{}",
        tri.len()
    );
    assert!(tri.iter().all(|(_, c, m)| c == m));
    assert!(coord.debug_has_more(), "全表远超 100 条");
    let grown = page_until_more_than(&coord, tri.len());
    assert!(grown > tri.len(), "翻到边界应扩充：{} -> {grown}", tri.len());
}
```

- [ ] **Step 2: 追加验收用例**

§11 一节末尾追加：

```rust
/// 通配命中（等长或更长，码里没有 `z`——五笔词库无 z 码，带 z 的只可能是拼音 / 字面串）。
fn wc_hits(pattern: &str, tri: &[(String, String, String)]) -> Vec<(String, String)> {
    tri.iter()
        .filter(|(_, c, _)| wubi_hit(pattern, c, true) || wubi_hit(pattern, c, false))
        .map(|(t, c, _)| (t.clone(), c.clone()))
        .collect()
}

/// 混输同 `pure_wildcard_smart_filters_like_general`：通配那一截与「常用字」档相同，首选仍是
/// 等长通配。拼音候选按 `(拼音, 码)` 照旧分组，不在比较范围内。缺 Task 1 时红。
#[test]
fn mixed_wildcard_smart_filters_like_general() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    for keys in ["hanz", "azz"] {
        let all = wc_hits(
            keys,
            &triples_with(with_filter(wubi_pinyin(true), "gb18030"), keys),
        );
        let general = wc_hits(
            keys,
            &triples_with(with_filter(wubi_pinyin(true), "general"), keys),
        );
        let smart = triples_with(with_filter(wubi_pinyin(true), "smart"), keys);
        assert!(
            all.len() > general.len(),
            "前置：{keys} 首批通配结果应有生僻字（全部 {} / 常用 {}）",
            all.len(),
            general.len()
        );
        assert_eq!(wc_hits(keys, &smart), general, "{keys}: 通配那一截与常用字档相同");
        assert!(
            wubi_hit(keys, &smart[0].1, true),
            "{keys}: 首选是等长通配，实际 {:?}",
            smart[0]
        );
    }
}

/// ★ Review Focus 5：混输首批 100 条回满、翻页扩充后，等长通配仍全部排在最前，字面候选仍在。
/// 缺 Task 2 时红在 `has_more`（混输首批 300、合并后不足 300）。
#[test]
fn mixed_expansion_keeps_equal_length_wildcard_first() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let coord = Coordinator::new_headless(no_relax(wubi_pinyin(true)), Some(&data_dir()));
    press(&coord, "azz");
    let first = coord.debug_candidate_count();
    assert!(first <= 100, "首批至多 100 条：{first}");
    assert!(coord.debug_has_more(), "前置：混输首批回满 100 条");
    let grown = page_until_more_than(&coord, first);
    assert!(grown > first, "翻到边界应扩充：{first} -> {grown}");
    let tri = coord.debug_candidate_triples();
    let first_other = tri
        .iter()
        .position(|(_, c, _)| !wubi_hit("azz", c, true))
        .unwrap_or(tri.len());
    assert!(
        tri[first_other..]
            .iter()
            .all(|(_, c, _)| !wubi_hit("azz", c, true)),
        "扩充后等长通配仍全部在最前"
    );
    assert!(
        tri.iter()
            .any(|(_, c, _)| !wubi_hit("azz", c, true) && !wubi_hit("azz", c, false)),
        "字面（拼音）候选仍在——保底配额"
    );
}

/// 「常用字」档：首批被滤掉一部分，翻页照样扩充补足可见结果。缺 Task 2 时红在 `has_more`。
#[test]
fn general_mode_wildcard_pages_fill_up() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(
        no_relax(with_filter(wubi(true, "z"), "general")),
        Some(&data_dir()),
    );
    press(&coord, "azz");
    let first = coord.debug_candidate_count();
    assert!(first < 100, "前置：首批 100 条里有被滤的生僻字：{first}");
    assert!(coord.debug_has_more());
    let grown = page_until_more_than(&coord, first);
    assert!(grown > first, "翻页应补足可见结果：{first} -> {grown}");
}

/// 智能档被滤的通配生僻字经「翻到末页再按一次」追加在末尾，原有候选顺序不动。
/// `hanz` 纯码表只有 9 条（`han?`，4 码满码无更长补全），一页即末页。缺 Task 1 时红
/// （无被滤候选可追加）。
#[test]
fn page_end_relax_appends_filtered_wildcard_results() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let pair = |t: &(String, String, String)| (t.0.clone(), t.1.clone());
    let all: Vec<(String, String)> = triples_with(with_filter(wubi(true, "z"), "gb18030"), "hanz")
        .iter()
        .map(pair)
        .collect();
    let coord = Coordinator::new_headless(with_filter(wubi(true, "z"), "smart"), Some(&data_dir()));
    press(&coord, "hanz");
    let before: Vec<(String, String)> = coord.debug_candidate_triples().iter().map(pair).collect();
    assert!(all.len() > before.len(), "前置：hanz 有被滤的生僻字");
    assert!(!coord.debug_has_more(), "前置：9 条一批取完");
    for _ in 0..20 {
        let (cur, _, total) = coord.debug_page_info();
        if cur + 1 >= total {
            break;
        }
        press_vk(&coord, VK_NEXT, false);
    }
    press_vk(&coord, VK_NEXT, false); // 末页再按一次 = 放宽
    let after: Vec<(String, String)> = coord.debug_candidate_triples().iter().map(pair).collect();
    assert_eq!(&after[..before.len()], &before[..], "原有候选顺序不动");
    let mut appended = after[before.len()..].to_vec();
    appended.sort();
    let mut expected: Vec<(String, String)> =
        all.into_iter().filter(|p| !before.contains(p)).collect();
    expected.sort();
    assert_eq!(appended, expected, "被滤的通配生僻字全部追加在末尾");
}
```

- [ ] **Step 3: 跑**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input
time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_wildcard --no-fail-fast 2>&1 | tail -30
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test -p wind-coordinator --test codetable_filter_scope_consistency --test rare_phrase_scope --test common_chars_override --test single_char --no-fail-fast
```

Expected: 全绿、耗时 ≥ 1s、无「跳过」。第二条是「非通配 smart 分组与扩充行为不变」的既有锁；「关闭通配时逐键相同」由既有 `wildcard_off_by_default_changes_nothing` 覆盖。

核对点：
- `mixed_wildcard_smart_filters_like_general` 若红在「前置」（混输 `azz` 首批通配那一截无生僻字：混输首批 100 条被拼音保底挤掉 20 席），删掉 `azz` 只留 `hanz` 并在报告里注明。
- `page_end_relax_appends_filtered_wildcard_results` 若红在 `assert_eq!(appended, expected)` 且差异只在放宽候选的 `code` / 文本带前缀，检查 `input.scope_relax.prefix` 是否落在 `code` 上（预期只影响显示文本，不影响 `(text, code)`）。
- 若首跑即红而原因是 Task 1/2 的实现缺陷，回到对应任务修，不改用例。

- [ ] **Step 4: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
git -C . branch --show-current
git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
test(wildcard): 常用字过滤与翻页扩充的真实数据验收

纯码表 / 混输下智能档通配结果与常用字档一致、首选仍是等长通配；混输扩充后等长通配
仍在最前、拼音仍有保底；常用字档翻页补足可见结果；末页放宽把被滤的通配生僻字追加在
末尾；首位全表扫描改为断言可扩充（spec codetable-wildcard §11）。
EOF
git show --stat HEAD
```

---

## Task 4: 文档站、架构文档、spec 状态

**Files:**
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`（`### 通配输入` 小节 ~103）
- Modify: `../WindInputDocs/content/docs/settings/input/index.mdx`（检索范围表「智能」行 ~38）
- Modify: `../WindInputDocs/content/docs/guides/config/input.mdx`（`filter_mode` 行 ~38）
- Modify: `docs/architecture/engine-candidate-pipeline.md`（§3.4 ~192-205；§8.1 ~835-839）
- Modify: `docs/design/codetable-wildcard.md`（头部状态行 ~7）

**Interfaces:** 无代码接口。

- [ ] **Step 1: 核对设置说明不需要改**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/wind-setting
grep -n 'schema.codetable.wildcard' -A8 src/assets/settings_manifest.toml | grep -nE '100|上限|最多' || echo "hint 不提条数，wind-setting 不改"
```

Expected: `hint 不提条数，wind-setting 不改`。若有命中，停下回报（本计划未安排 wind-setting 改动与 `dev.sh sg` 重生成）。

- [ ] **Step 2: 文档站**

`codetable.mdx` `### 通配输入` 小节，把

```mdx
- 短语、整句不参与通配；临时拼音、快捷输入里也不生效。结果最多 100 条。
```

替换为

```mdx
- 短语、整句不参与通配；临时拼音、快捷输入里也不生效。
- 结果先列 100 条，翻到最后几页时自动再加载，最多 5000 条。[候选检索范围](/docs/settings/input#候选检索范围)照常生效：「智能」档下全部通配结果算一组，只要其中有常用字，生僻字就先不出；翻到末页再按一次向后翻页键，被滤掉的生僻字追加到末尾。
```

`settings/input/index.mdx` 检索范围表，把

```mdx
| 智能（默认） | 按通用规范汉字表过滤，但同一编码下没有常用候选时整组照出 |
```

替换为

```mdx
| 智能（默认） | 按通用规范汉字表过滤，但同一编码下没有常用候选时整组照出（[通配输入](/docs/settings/schema/codetable#wildcard)的结果整份算一组） |
```

`guides/config/input.mdx` 的 `filter_mode` 行里，把子串

```
`smart` = 智能过滤（同一编码下存在常用候选时过滤掉非常用的）
```

替换为

```
`smart` = 智能过滤（同一编码下存在常用候选时过滤掉非常用的；码表通配的结果整份算一组）
```

Run:

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInputDocs && pnpm install --frozen-lockfile && pnpm lint
```

Expected: 通过。

- [ ] **Step 3: 架构文档与 spec 状态行**

`engine-candidate-pipeline.md` §3.4「**引擎**」一条里，把

```markdown
  上限 `WILDCARD_RESULT_LIMIT = 100`；结果带 `is_wildcard`，`source_tier` 把「通配 + 等长」放档 0。
```

替换为

```markdown
  按协调器给的 `max_candidates` 查，硬上限 `WILDCARD_RESULT_LIMIT` = `engine::CANDIDATE_LIMIT_CAP`
  （5000，与翻页扩容上限同一常量；0 不下传 `search_pattern`）；结果带 `is_wildcard`，`source_tier`
  把「通配 + 等长」放档 0。
```

「**协调器**」一条末尾（`- 五笔拼音混输（…）` 子项之后）追加子项：

```markdown
  - 条数与过滤（设计稿 §11）：有 pattern 时首批固定 `WILDCARD_INITIAL_LIMIT = 100`（不走码长
    分级），翻到边界由 `expand_candidates` ×2 扩充；检索范围智能档把全部 `is_wildcard` 候选当
    一组（§8.1）。`expand_candidates` 的到底判据是**引擎条数未增**（`engine_count <= prev_limit`），
    对所有引擎生效——按可见条数判，一批全被滤掉时会误判到底。
```

§8.1 第一段（以「无常用则整组保留。」结尾）之后插入一段：

```markdown
**通配例外**（设计稿 `codetable-wildcard.md` §11）：`is_wildcard` 候选整份一组（私有键
`SmartGroup::Wildcard`），不按码分——用户没打出那些码，按码分组会把只含生僻字的码位当孤儿
放行。非通配候选的键仍是 `(source, code)`；通配候选也不往按码分组里写（含 `merged_codes`）。
```

`docs/design/codetable-wildcard.md` 头部状态行改为：

```markdown
> **状态：P1–P4、§10（混输调度）、§11（常用字过滤与翻页扩充）已实施**（提交见 git log --grep 通配）。分期见 §6，实施计划见 `codetable-wildcard-plan.md`、`codetable-wildcard-mixed-plan.md`、`codetable-wildcard-paging-plan.md`。
```

实施中若有与 §11 不同之处（含本计划「spec 待澄清」一节被控制者裁决的条目），追加到 §9 末尾一条「2026-09-29 §11 实施后偏离」。

- [ ] **Step 4: 两个仓各自提交**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInputDocs
git branch --show-current
git diff --cached --name-status
git commit -F - --only -- content/docs/settings/schema/codetable.mdx content/docs/settings/input/index.mdx content/docs/guides/config/input.mdx <<'EOF'
docs(settings): 通配结果可翻页扩充，智能检索范围下整份算一组
EOF
git show --stat HEAD

cd ../WindInput
git branch --show-current
git diff --cached --name-status
git commit -F - --only -- docs/architecture/engine-candidate-pipeline.md docs/design/codetable-wildcard.md <<'EOF'
docs(design): 通配 §11 落地——架构文档 §3.4 / §8.1 与设计稿状态
EOF
git show --stat HEAD
```

---

## Task 5: 全量回归与编译门

**Files:** 无改动。

- [ ] **Step 1: 全量测试**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input
time CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage TMPDIR=/tmp/wct-wcpage cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-wcpage/full.log | grep -E "^test result|FAILED|panicked" | tail -60
grep -E "^test result" /tmp/wct-wcpage/full.log | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: `failed` 与开工前基线相同（基线为 0 则为 0；偶发项按 Global Constraints 单跑确认）；`passed` = 基线 + 12（Task 1：filter 3 + 集成 1；Task 2：码表引擎 1（另 1 条改名不计）+ 混输 1 + `expand_stop_tests` 1 + 集成 1；Task 3：集成 4 新增，`leading_wildcard_when_unbound_scans_whole_table` 改写不计）。

- [ ] **Step 2: 编译门**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage cargo check-headless
cd .. && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-wcpage ./scripts/dev.sh k
```

Expected: 均通过。

- [ ] **Step 3: 占位与提交范围自查**

```bash
cd /home/dufeng/develop/windinput/wt-wcpage/WindInput
base=$(git merge-base HEAD main)
git log --oneline "$base"..HEAD
git diff "$base"..HEAD --stat
git diff "$base"..HEAD | grep -nE "^\+.*(TODO|todo!|unimplemented!|#\[ignore\]|\.only\()" || echo clean
git diff "$base"..HEAD --name-only | grep -x "wind_input/Cargo.lock" && echo "!! Cargo.lock 被提交" || echo "lock ok"
git -C ../WindInputDocs log --oneline -2
```

Expected: `clean`、`lock ok`；`--stat` 只含本计划文件结构表里 WindInput 仓的那些文件（加本计划文件与已在分支上的设计稿提交）。

---

## spec 待澄清（写计划时对照真实代码发现）

1. **§11 根因「首批 limit ≥ 300」不全对**：纯码表首批按码长分级，**1 码是 100**（`wind-coordinator/src/handle_candidate.rs:1065-1067`），2 码 300、≥3 码 1000，混输 / 其它 300（`:1072`）。故首位单键 `z` 的 `has_more` 本就为真，它翻不动的原因是另一条：扩容后引擎仍回 100 条、可见条数不增，`expand_candidates` 判到底（`:2403-2408`）。两条根因叠加，本计划两处都改（Task 2）。
2. **§11 契约 1「分组键用 pattern 而非 code」**：过滤器与协调器 `apply_filter`（`handle_candidate.rs:2046-2080`）都拿不到 pattern，而一次构建只有一个 pattern ⇒ 「按 pattern 分组」≡「全部 `is_wildcard` 候选一组」。本计划用 `is_wildcard` 实现（`filter.rs` 私有 `SmartGroup`），语义等价、不改公共签名。若控制者要的是字面上把 pattern 传下去，需改 `filter_candidates` 公共签名。
3. **§11 契约 3「引擎条数未增」没有现成的「上一批引擎条数」**：`State` 只存 `candidate_limit` / `has_more`（`coordinator.rs:647-649`）。本计划写成 `engine_count <= prev_limit`，前提是 `has_more` 只在 `engine_count >= limit` 时置真（`handle_candidate.rs:2286`、`:2411`）。若某引擎回的条数**超过** limit，判据会多扩一轮才停，不会误判到底（Task 2 Step 4 核对点）。
4. **§11 契约 2「混输主码表通配与字面同拿 `max_candidates`」在代码里已成立**（`mixed/engine.rs:894-901`），挡路的只是主码表内部的固定 100（`codetable/engine.rs:919`、`:933`）。但「通配首批 100 条」一旦对混输生效，**混输首批从 300 缩到 100**，字面半边（拼音）只剩 `truncate_with_pinyin_quota` 的 20 席保底（`mixed/engine.rs:524-567`，`PINYIN_QUOTA_DIVISOR = 5`）。spec 未明说首批 100 是否含混输；本计划按「本串有 pattern 即 100」处理，需控制者确认。
5. **§11 测试对照「纯码表 `hanz` 翻页 `has_more` 为真」测不到**：纯五笔 `han?` 全库只有 9 条（4 码满码无更长补全），`has_more` 必为假。本计划 `hanz` 只用于过滤与末页放宽，翻页 / `has_more` 改用 `azz`（`a??`，四千余条）。
6. **§11 契约 5「扩充后前页可能移位——用例钉住现状」**：在通配单组下，只有「首批通配结果一个常用字都没有」时才会发生（整组保底放行，后一批带进常用字后整组被滤）。真实数据里难以稳定复现，本计划钉在单测 `smart_wildcard_group_without_common_keeps_all_until_common_arrives`（Task 1），未做端到端用例。

---

## 自查

- **spec 覆盖**：§11 契约 1（过滤一组、general 照旧、放宽追加、非通配不变）→ Task 1 `SmartGroup` + 3 条单测 + `pure_wildcard_smart_filters_like_general`；Task 3 `mixed_wildcard_smart_filters_like_general`、`page_end_relax_appends_filtered_wildcard_results`、既有 `codetable_filter_scope_consistency` 等。契约 2（去掉 100、硬上限 5000 对齐、首批 100、`has_more` 成立、混输同拿 `max_candidates`、配额不变）→ Task 2 常量 / `convert_wildcard` / `WILDCARD_INITIAL_LIMIT` + `wildcard_hard_cap_matches_expansion_cap_and_zero_is_empty`、`wildcard_respects_single_code_input_and_max_candidates`、`wildcard_primary_follows_max_candidates`、`pure_wildcard_first_batch_is_100_and_expands`；Task 3 `mixed_expansion_keeps_equal_length_wildcard_first`。契约 3（到底判据、所有引擎）→ Task 2 `expand_candidates` + `expand_stop_tests::expansion_continues_past_an_all_filtered_batch`（非通配）。契约 4（不做缓存、不下推）→ 未触碰 wind-dict 与缓存。契约 5（前页移位钉现状、不传 0）→ Task 1 单测、Task 2 `limit == 0` 分支与用例。测试对照：`hanz` / `a??` 首批无生僻且常用在前 → Task 1 / Task 3 两条 `*_filters_like_general`；翻页 `has_more` 与条数增加 → Task 2/3；general 翻页补足 → `general_mode_wildcard_pages_fill_up`；混输扩充后等长在前 → Task 3；末页放宽追加 → Task 3；关闭通配逐键相同 → 既有 `wildcard_off_by_default_changes_nothing`；非通配 smart 分组与扩充不变 → 既有 filter 单测、`codetable_filter_scope_consistency`、`input_flow::test_dynamic_paging_expands_candidates`、`expand_stop_tests`。文档：设置说明核对不改（Task 4 Step 1）、文档站三处、架构 §3.4 / §8.1、spec 状态行。
- **签名一致性**：`CANDIDATE_LIMIT_CAP: usize`（`wind_engine::engine`，Task 2 定义）被 `WILDCARD_RESULT_LIMIT`（同任务）与 `expand_candidates`（同任务）消费；`WILDCARD_INITIAL_LIMIT: usize`（`crate::wildcard`，`pub(crate)`）只在 `update_candidates` 消费；`SmartGroup::of(&Candidate) -> SmartGroup` 与 `build_has_common(&[Candidate]) -> HashMap<SmartGroup, bool>` 仅 `filter.rs` 内部；`convert_wildcard(&self, &str, &str, usize) -> Option<ConvertResult>` / `filter_candidates(Vec<Candidate>, FilterMode, RarePhrasePolicy) -> FilterOutcome` / `expand_candidates(&self, &mut State)` 签名未变。测试助手 `with_filter` / `triples_with`（Task 1）、`no_relax` / `page_until_more_than` / `VK_NEXT`（Task 2）在 Task 3 消费，定义先于使用。
- **占位扫描**：全文无 TBD / TODO / 「类似任务 N」。无法完全确认的细节均写明核对点：Task 1 Step 2（`azz` 首批是否确有生僻字，按词库估算约 6 个）、Task 2 Step 2（自造码表的前缀取数与常用字判定生效）、Task 2 Step 4（混输首批缩小后 §10 用例、引擎超 limit 的情形）、Task 3 Step 3（混输 `azz` 前置、放宽前缀是否落在 `code` 上）、Task 4 Step 1（设置说明是否提条数）。
- **已知取舍**：Task 3 全部用例在 Task 1/2 之后首跑即绿，是验收与锁，已逐条注明缺哪一任务时会红；Task 1 的 `smart_wildcard_group_without_common_keeps_all_until_common_arrives` 前半在实现前即绿（锁住保底）。到底判据用 `engine_count <= prev_limit` 而非新增状态位，理由见 Task 2 Step 3 注释与「spec 待澄清」3。
