# 码表反查模式 / 通配仅单字 / 反查含未启用扩展词库 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## 计划裁决（写计划时读码发现 spec 与现实不符或需要落定的点；spec 原文未改）

1. **A「截断之前过滤」在引擎内做不到字面意义**：词库层 `DictManager::search_pattern` 自己就按 `limit` 截断
   （`wind-dict/src/composite.rs:99`、`datformat.rs:1541` 各层按两档取额），引擎拿到的已是截断后的列表。
   **裁决**：不改 wind-dict 的 `DictLayer` 接口，在引擎 `wildcard_query` 内**加倍重取**——过滤后不够 `limit`
   且词库未取尽时，`fetch` ×2 重查，直到够数 / 取尽 / 到硬上限 `WILDCARD_RESULT_LIMIT`。与生僻字模式
   「准入下推 + 不足则加大重取」同构（提交 `1eb0aab7` / `5fdb497c`）。仅单字关时只查一轮，与现状逐条相同。
2. **A 的真实数据验收串不能只用 `azz`**：五笔 `a??` 等长 318 条**全是单字**，首批 100 条本来就全是单字，
   截断先后测不出差别。**裁决**：`azz` 保留为 spec 列的冒烟用例；「先滤后截」的验收改用 `azzz`
   （`a???` 等长 4066 条，单字 1342，按权重前 100 条里只有 38 个单字——实测于 `build_dev/data`）。
3. **C 的代码命名**：本仓「shadow」已专指候选调整（`apply_shadow` / `segment_shadow` / `ShadowProvider`）。
   **裁决**：spec 的「影子层」在代码里叫 `DisabledDictLayers`（新文件 `codetable/disabled_dicts.rs`），
   候选标记叫 `Candidate::from_disabled_dict`；注释里写「影子层（spec §4.2）」对应。
4. **C「已启用排前」与「仍走 `cmp_exact_first` + `base_sort`」**：协调器 `build_candidates` 会用
   `candidate_display_order` 把引擎结果整体重排（`handle_candidate.rs:1512`），引擎内排好会被推翻。
   **裁决**：新增布尔键 `from_disabled_dict`（false 在前），同时插进引擎通配排序与 `candidate_display_order`，
   位置都在 `cmp_exact_first` **之后**：等长 / 更长两档不变，档内先启用后未启用，再按 base_sort。
   两条非未启用候选之间该键恒 `Equal` ⇒ 既有次序可证明不变（与 `is_draft` 那条同一论证）。
5. **C 的失效点**：spec 说 `set_dict_enabled_live` 清影子层；现状**禁用**走「引擎摘层 + 返回 true、不重建」
   （t107，返回 false 会触发重建、曾让「甘蓝菜」复活）。**裁决**：影子层记住本方案**全部**扩展库来源
   （`declared`）与当前启用集；`CodeTableEngine::set_dict_enabled(id, false)` 摘层后调用
   `DisabledDictLayers::mark_disabled(id)`（启用集删掉它、已加载的影子层作废），返回值语义不变；
   **启用**照旧失效重建（影子层随引擎一起重建）。方案重载 / 开关切换本就重建引擎，天然清掉。
6. **C「注释反查」的范围**：反查索引现有 6 类消费方。**裁决**：变体只用于候选注释的
   `code_rev` / `code` / `code_rev_all` / `code_all` 四个变量（`EngineManager::codetable_reverse_hint`
   与新增的 `word_codes_display_for_comment`）。加词查重（`word_codes_in`）、辅助码来源（`text_codes`）、
   悬停 `[编码]` 段（`word_codes_display`）、联想、单字全码表**一律不变**——查重必须以启用集为准，否则
   「系统词库没有、只在未启用库里有」的码+词会被判成已存在而拒绝加词。
7. **C 的变体在「无未启用库」时退化**：方案没有任何未启用扩展库时，`comment_reverse_scope` 直接给
   `ReverseScope::Enabled`，不另建一份内容相同的文件。
8. **B 的实现形态**：沿用生僻字模式的先例（`ModeKind::RareChar` 是 special 的参数变体，复用
   `special_buffer` 与 `handle_special_key`，见 `pipeline.rs:60-75` 与提交 `ed2761a4`）。但 special 族
   原本**没有**翻页扩充与检索范围过滤，而 spec §3.2 要求二者。**裁决**：候选另写
   `build_reverse_candidates`（`convert_reverse` + `mark_common` + `apply_filter`），并给
   `expand_candidates` / `try_relax_scope_on_page_end` / `expire_scope_override` 各补一条 Reverse 分支。
9. **B 的引擎入口**：`CodeTableEngine::convert_wildcard` 首行 `self.opts.wildcard?`（`engine.rs:912`），
   主开关关时直接 `None`；spec 要求模式内不受主开关约束。**裁决**：新增 `Engine::convert_reverse` 与
   `Engine::reverse_wildcard_key`，与 `convert_wildcard` 共用私有内核 `wildcard_query`（A、C 自动生效）；
   混输只代理主码表，不调拼音。
10. **B 的配置不加 `trigger_keys`**：spec 只列 `input.reverse.{enabled,candidate_layout}`。设置端按键绑定走
    现成的「全局自定义按键」编辑器（wind-setting 动词表补 `reverse`）与方案页「z 键引导功能」下拉；
    `z_key_action = "reverse"` 需要 `try_z_fallback` 补一臂（GH#146 同构：五笔出厂 `zz*` 短语让首键 z
    恒让位，不补就永远进不去）。
11. **B 直达热键的占位组合区**不新增 `input.caret.reverse_via_composition`，固定按开（与
    `rare_char_via_composition` 出厂值一致）。
12. **B 的词频与候选调整**：模式内上屏的记账码取候选**全码**（同 `main_freq_code` 口径，spec
    codetable-wildcard §3.2）；模式内不做词频重排、不吃候选调整（通配串不是任何码位，读侧本就查不中）。
13. **B 退出残留**：special 族从不写 `has_more` / `candidate_limit` / `scope_relaxed`，Reverse 会写。
    **裁决**：`exit_special_mode` 一并复位这三位（对 Special / RareChar 是空操作）。
14. **`prefix-hijack-modes.md` §3 清单与现状名称不符**：#3 `commit_by_offset` 现为 `select_page_candidate` /
    `commit_highlighted` / `select_candidate_at`；#6 `mode_badge` 现为 `mode_indicator_names`；
    #7 `mode_layout_intent` 现为 `layout.rs` 的 `intent_for`。计划按现名接线，P3 文档任务顺手更正该清单。
15. **spec §4.1「`default=false` 即扩展库」措辞**：实际判据是 `DictSpec::is_enabled()`
    （`schema.rs:704`，看 `enabled` / `default` / `default_enabled`），`wubi86_xzqy` 是
    `default_enabled = false`（`data/schemas/wubi86.schema.toml`）。计划一律以 `is_enabled()` 为准。
16. **C 样本**：`门头沟区 uuia` 只在 `wubi86_xzqy`（主库 / extra 无 `uuia` 码），用作「开关前后可见性」；
    反查模式首位通配样本 `zuia`（`?uia`，主库 6 条全是词组，xzqy 另有 门头沟区 / 巴彦淖尔市市辖区 / 二道江区）。
17. **文档版本号**：`WindInputDocs/data/releases.json` 最新为 `0.124.0`，三期新内容一律 `<Since v="0.125" />`
    （未发布前构建期自动隐藏）。Task 开工时若 releases.json 已出现 `0.125.x`，改用下一个小版本并在报告里注明。

---

**Goal:** 三期独立落地：P1 通配仅单字（`schema.codetable.wildcard_single_only`）；P2 通配 / 注释反查可查
`is_enabled()==false` 的扩展词库（`schema.codetable.lookup_disabled_dicts`，影子层 + 反查索引变体），普通
打字候选完全不受影响；P3 用户自绑键进入的反查模式（`BoundAction::Reverse` / `ModeKind::Reverse` /
`input.reverse.{enabled,candidate_layout}`），模式内通配位置任意含首位、不受 `wildcard` 主开关约束、不含拼音。

**Architecture:**
- 引擎：`CodeTableEngine::convert_wildcard` 的查询体抽成私有 `wildcard_query(input, pattern, max)`；
  P1 在其中加单字过滤与加倍重取；P2 在其中合并 `DisabledDictLayers`（懒加载的未启用扩展库层，
  `(text, code)` 去重、启用优先）；P3 新增 `Engine::convert_reverse` 走同一内核但不看主开关。
  普通 `convert` 不引用 `DisabledDictLayers`（字段只在 `wildcard_query` 被读）。
- 反查索引：`EngineManager` 引入 `ReverseScope { Enabled, WithDisabled }`；内存表键
  `schema_id` / `schema_id + "\u{1}with_disabled"`，磁盘 `<cache>/<key>/<key>.wridx` /
  `<key>.with_disabled.wridx`；只有候选注释四变量走变体。
- 协调器：P3 的 `ModeKind::Reverse` 复用 special 族（`special_buffer` / `handle_special_key` /
  `exit_special_mode`），候选由新文件 `handle_reverse.rs` 构建，接 `apply_filter` 与翻页扩充。
- 配置：`CodetableGlobal` / `CodeTableSpec` 各加两字段（三态折叠）；`InputConfig.reverse: ReverseConfig`；
  `BoundAction::Reverse`（动词 `reverse`）。

**Tech Stack:** Rust workspace（wind-candidate / wind-config / wind-engine / wind-coordinator）+ wind-setting 仓
（`settings_manifest.toml` 等，经 `scripts/dev.sh sg/st` 在 xwin+wine 下生成与测试）+ WindInputDocs 仓（fumadocs）。

**Spec:** `docs/design/codetable-reverse-mode.md`（**binding**，§2 A / §3 B / §4 C / §5 设置与文档 / §6 分期）；
背景 `docs/design/codetable-wildcard.md` §3（首位让位）、§10（混输）、§11（过滤与翻页）。

## Global Constraints

- **分期**：P1（Task 1–5）→ P2（Task 6–14）→ P3（Task 15–22）。每期最后一个 Task 全量测试全绿后该期可单独合并；
  后一期开工前前一期已提交完。P3 依赖 P1/P2 的 `wildcard_query`（裁决 9）。
- **行为契约**（逐条照做）：
  1. 三个新开关出厂全关（`wildcard_single_only = false`、`lookup_disabled_dicts = false`、`input.reverse.enabled = false`），
     全关时一切与现状逐键相同。
  2. 单字判据只用 `wind_candidate::single_markable_char`（UAX #29 字素簇）；**禁止** `chars().count() == 1`（issue #83）。
  3. 普通 `convert`（打字候选、`has_code_prefix` 活码探针、顶码、自动上屏复评）**永不**读 `DisabledDictLayers`。
  4. 影子层**首次通配 / 反查查询时**才加载；开关关或从未查询 ⇒ 零加载。文件缺失 / 解析失败 ⇒ `warn!` 跳过该库，不阻塞。
  5. 注释反查仍只用 `*_if_ready` 系列取索引，未建好 ⇒ 后台建、本次不显示（沿用防卡死约束，`coordinator.rs:4502` 一带）。
  6. 反查模式：通配键 = 方案 `wildcard_key` 合法则用它，否则 `z`（`CodetableGlobal::reverse_wildcard_char`）；
     任何位置含首位都作通配；不看 `wildcard` 主开关；不查拼音；不自动上屏；选中上屏候选文本。
  7. 混输方案：A / C 取主码表方案的开关（`MixedRole::Primary` 构建的码表子引擎用的就是主方案的 `eff`）；
     反查模式只查主码表。
- 提交纪律：禁止 `git add -A` / `git add .` / `git commit -a` / `git stash`；新文件先 `git add -- <path>`；提交一律
  `git commit -F - --only -- <自己的路径…>`；提交前 `git diff --cached --name-status` 核对暂存区、提交后
  `git show --stat HEAD` 核对文件数 / 行数与自己的改动量相符。**任何提交都不带 `wind_input/Cargo.lock`**。
  每次提交前 `git -C <仓> branch --show-current` 须为 `feat/codetable-reverse`（三仓同名分支）。
- 路径：一切改动都在 `/home/dufeng/develop/windinput/wt-rev/{WindInput,wind-setting,WindInputDocs}` 下；**绝不**碰
  `/home/dufeng/develop/windinput/{WindInput,wind-setting,WindInputDocs}`。
- 格式化：禁止 `cargo fmt` / `cargo fmt --all`；只对自己改过的文件跑 `rustfmt --edition 2024 --check <file>`，其 diff 只落在
  自己 hunk 内才去掉 `--check` 执行，否则按 diff 手工采纳自己那几处。
- 测试环境：cargo 一律在 `wind_input/` 下、带 `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev TMPDIR=/tmp/wct-rev`
  （先 `mkdir -p /tmp/wct-rev`；路径要短，别用 scratchpad）；全量跑加 `--no-fail-fast`。下文把这串前缀简写为 `$T`：
  `T="CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev TMPDIR=/tmp/wct-rev"`，命令写作 `env $T cargo test …`。
- 已知偶发红：`handle_direct_aux::tests::schema_source_not_ready_is_noop_then_warms`（并发时序）。全量里它红了单独重跑；单跑绿即记偶发。
- 集成测试依赖 `build_dev/data`（本 worktree 里是实拷贝，已核实 `schemas/wubi86/wubi86_jidian_extra_district.dict.yaml`
  等在位），缺失时静默跳过而计数照绿——以 `--test codetable_wildcard` / `--test codetable_reverse` 耗时 ≥ 1s 且输出无「跳过」为数据在位判据。
- 日志：`info!` 不得含用户输入 / 候选。
- **wind-setting 改动**：manifest 改完在 `WindInput/` 下先 `env $T cargo test -p wind-rpc --test wind_setting_assets`（core 侧对账，秒出），
  再 `./scripts/dev.sh sg`（重生成 `capabilities.snapshot.json` / `mockdata/config.json`）与 `./scripts/dev.sh st`（xwin+wine 全量）。
  `sg` 后 `git diff` 里若只剩 `appVersion` 一行，**不提交它**。新 label 带进 `src/assets/pinyin_initials.txt` 没有的字会让
  `search::pinyin::tests::table_covers_every_title_char` 红——本计划选定的 label（「通配仅出单字」「通配与反查含扩展词库」
  「启用反查模式」「候选窗布局」）已逐字核对全在表内（`未` `只` `也` 不在表内，别用）。
- **WindInputDocs 改动**：`pnpm install --frozen-lockfile && pnpm lint`；另跑 `pnpm check:config`——基线已缺 15 个键
  （含 `schema.codetable.wildcard` / `wildcard_key`），本计划新增的键**不得**出现在缺失清单里（P1 顺手补上 wildcard 两键，
  缺失数降到 13 并保持）。

## Review Focus

- **影子层泄漏进普通 `convert`**：`DisabledDictLayers` 若被挂进主 `DictManager`（而不是独立持有），或 `convert` /
  `has_longer_code` / `handle_top_code` 任一路径读了它，未启用库就会出现在打字候选、活码判据与顶码里——
  「设置里没启用却打得出来」。规则：字段只在 `wildcard_query` 读；主 `dm` 注册层集合与改动前逐一相同。
  测试：Task 8 `plain_convert_never_touches_disabled_layers`（含「普通 convert 连加载都不触发」）+ Task 13
  `lookup_disabled_dicts_leaves_plain_typing_identical`（真实数据逐键对照）。
- **单字过滤在截断之后，条数不足**：词库层已按 `limit` 截断，事后过滤只剩被截段里的单字（`a???` 前 100 条只有 38 个），
  `has_more = engine_count >= limit` 随之判假、翻页也扩不出来。规则：`wildcard_query` 不够就 ×2 重取。
  测试：Task 2 `wildcard_single_only_filters_before_truncation` + Task 4 `single_only_azzz_first_batch_is_full_of_singles`。
- **反查触发键与方案首码冲突**：字母触发键若不让位，该字母在这个方案里再也打不出编码；符号键若误让位，模式进不去；
  反查键进不了 `is_any_mode_trigger` / `code_char_conflicts`，行内通配的首位让位与启动体检都认不得它。
  规则：沿用 `bound_action_yield_reason`（字母 + 活码前缀 ⇒ 让位；非码表 / 混输 ⇒ 让位），两条 owners 链补臂。
  测试：Task 19 `reverse_letter_trigger_yields_to_live_code_prefix`（含符号键对照与冲突体检断言）。
- **含未启用索引与常规 `.wridx` 串文件**：变体若与常规索引同名同 fp，后启动的一方会复用对方那份——注释里出现未启用库的码，
  或加词查重把未启用库的词判成已存在。规则：内存键带 `\u{1}with_disabled` 后缀、磁盘文件名 `<key>.with_disabled.wridx`、
  淘汰判据按基名。测试：Task 10 `variant_and_regular_indexes_use_separate_files`（含「换 manager 重启后各读各的」）。
- **模式退出后状态残留**：Reverse 写了 `has_more` / `candidate_limit` / `scope_relaxed`，`exit_special_mode` 不复位的话，
  退出后主路径翻页会用残留的 `has_more` 调 `expand_candidates`、下一次组码直接处于放宽态。规则：`exit_special_mode`
  复位三位（裁决 13）；`cancel_session` / `rewind_hijack` / `reset_exclusive_modes` 都能走到 Reverse 的收尾。
  测试：Task 17 `enter_and_exit_reverse_mode_clears_state` + Task 21 `reverse_esc_exits_without_residue`。

---

## 文件结构

| 文件 | 期 | 动作 | 职责 |
|---|---|---|---|
| `wind_input/crates/wind-config/src/config.rs` | P1 P2 P3 | Modify | `CodetableGlobal` 两字段 + `resolved` + `reverse_wildcard_char`；`ReverseConfig`；`BoundAction::Reverse`；测试 |
| `wind_input/crates/wind-config/src/schema.rs` | P1 P2 | Modify | `CodeTableSpec` 两个 `Option<bool>` |
| `wind_input/crates/wind-config/src/config_schema.rs` | P1 P2 P3 | Modify | `REGISTRY` 五键；`schema_overridden_keys` 两键 |
| `wind_input/crates/wind-config/src/hotkey.rs` | P3 | Modify | `hotkey_policy_for` 穷举臂 |
| `data/config.toml` | P1 P2 P3 | Modify | 显式列出新键 + 说明 |
| `wind_input/crates/wind-candidate/src/candidate.rs` | P2 | Modify | `Candidate::from_disabled_dict`；`candidate_display_order` 插键；测试 |
| `wind_input/crates/wind-engine/src/engine.rs` | P3 | Modify | `Engine::convert_reverse` / `reverse_wildcard_key` 默认实现 |
| `wind_input/crates/wind-engine/src/codetable/engine.rs` | P1 P2 P3 | Modify | `CommitOptions` 新字段；`wildcard_query` 内核；`with_disabled_dicts`；`set_dict_enabled` 同步；测试 |
| `wind_input/crates/wind-engine/src/codetable/disabled_dicts.rs` | P2 | Create | `DisabledDictLayers` / `DisabledDictSource`（影子层）；测试 |
| `wind_input/crates/wind-engine/src/codetable/mod.rs` | P2 | Modify | 导出 |
| `wind_input/crates/wind-engine/src/mixed/engine.rs` | P1 P3 | Modify | 单字只作用于通配侧的测试；`convert_reverse` / `reverse_wildcard_key` 代理主码表 |
| `wind_input/crates/wind-engine/src/manager.rs` | P1 P2 P3 | Modify | `build_engine` 注入；`disabled_extra_sources`；`ReverseScope` 与索引变体；`active_reverse_key` / `convert_reverse`；测试 |
| `wind_input/crates/wind-engine/src/lib.rs` | P2 | Modify | `pub use manager::ReverseScope` |
| `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs` | P2 | Create | 自造夹具：影子层接线、索引变体、失效 |
| `wind_input/crates/wind-coordinator/src/comment.rs` | P2 P3 | Modify | 注释反查走变体 + 按需后台建；`template_for` 补 Reverse |
| `wind_input/crates/wind-coordinator/src/coordinator.rs` | P2 P3 | Modify | `prewarm_indexes` / `spawn_index_warm_in`；`code_char_conflicts`、`cancel_session`、`hotkey_session_now` 补臂 |
| `wind_input/crates/wind-coordinator/src/pipeline.rs` | P3 | Modify | `ModeKind::Reverse` |
| `wind_input/crates/wind-coordinator/src/handle_reverse.rs` | P3 | Create | 进入 / 候选构建 / 扩充 / 放宽；`reverse_pattern`；单测 |
| `wind_input/crates/wind-coordinator/src/lib.rs` | P3 | Modify | `mod handle_reverse;` |
| `wind_input/crates/wind-coordinator/src/{handle_special,handle_lifecycle,handle_mode,handle_temp,handle_url,handle_candidate,candidate_nav,layout,debug_support,handle_common_chars}.rs`、`coordinator/message_handler.rs` | P3 | Modify | 按 `prefix-hijack-modes.md` §3 清单接线（Task 17/18/19 逐处列出） |
| `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs` | P1 P2 | Modify | 真实数据验收（单字、影子层） |
| `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs` | P3 | Create | 反查模式真实数据验收 |
| `../wind-setting/src/assets/settings_manifest.toml` | P1 P2 P3 | Modify | 三期设置项 |
| `../wind-setting/src/dialogs/schema_codetable.rs` | P1 P2 | Modify | `SPEC_BEHAVIOR_FIELDS` |
| `../wind-setting/src/dialogs/{schema_manager,key_binding}.rs`、`src/state.rs` | P3 | Modify | 动词 `reverse` 进按键编辑器 |
| `../wind-setting/src/assets/capabilities.snapshot.json`、`src/mockdata/config.json` | P1 P2 P3 | Regenerate | `dev.sh sg` 产物 |
| `../WindInputDocs/content/docs/settings/schema/codetable.mdx` | P1 P2 P3 | Modify | 通配输入节补 A / C；反查模式指路 |
| `../WindInputDocs/content/docs/guides/config/{schema,input}.mdx` | P1 P2 P3 | Modify | 参考表补行 |
| `../WindInputDocs/content/docs/guides/reverse-mode.mdx`、`guides/meta.json` | P3 | Create/Modify | 反查模式专页 |
| `docs/design/{codetable-reverse-mode,codetable-wildcard,prefix-hijack-modes}.md`、`docs/architecture/engine-candidate-pipeline.md` | P1–P3 | Modify | 状态行、§8 指路、清单更正、§3.4 |

---

## 开工前：基线

- [ ] **Step 0: 记全量基线**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput && git status --short --branch && git log --oneline -1
for r in wind-setting WindInputDocs; do git -C ../$r status --short --branch; done
T="CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev TMPDIR=/tmp/wct-rev"; mkdir -p /tmp/wct-rev
cd wind_input && env $T cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-rev/baseline.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
cd ../../WindInputDocs && node scripts/check-config-coverage.mjs 2>&1 | tail -1
```

Expected: 三仓分支均为 `feat/codetable-reverse`，WindInput 工作区除本计划文件外干净（`M wind_input/Cargo.lock` 不属于本计划、永不提交）；
记下 `passed=` / `failed=`（worktree 带实拷贝 `build_dev/data`，量级应 ~4600，四位数以下说明数据没在位）；
`check-config-coverage` 末行「缺失 15」。`failed` 非 0 时先记失败名单（`grep -E "^test .* FAILED|panicked" /tmp/wct-rev/baseline.log`）。

---

# P1 — A：通配仅单字

## Task 1: 配置字段 `schema.codetable.wildcard_single_only`

**Files:**
- Modify: `wind_input/crates/wind-config/src/schema.rs`（`CodeTableSpec` 的 `wildcard_key` 之后，~490）
- Modify: `wind_input/crates/wind-config/src/config.rs`（`CodetableGlobal` 字段 ~2133 之后；`Default` ~2237；`resolved` ~2303；`mod tests` 的 `codetable_wildcard_folds_from_schema_and_validates_key` 之后）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（`REGISTRY` ~241；`schema_overridden_keys` ~918）
- Modify: `data/config.toml`（`wildcard_key = "z"` 之后，~90）

**Interfaces:**
- Produces: `CodetableGlobal::wildcard_single_only: bool`（`#[serde(default)]`，零值 false）；`CodeTableSpec::wildcard_single_only: Option<bool>`；
  注册键 `schema.codetable.wildcard_single_only`（`Bool`）。

- [ ] **Step 1: 写失败测试**（config.rs `mod tests`）

```rust
    /// reverse-mode spec §2：仅单字是方案级开关，三态折叠同 `wildcard`，与主开关正交。
    #[test]
    fn codetable_wildcard_single_only_folds_from_schema() {
        let g = CodetableGlobal::default();
        assert!(!g.wildcard_single_only, "出厂关闭");
        let on = crate::schema::CodeTableSpec {
            wildcard_single_only: Some(true),
            ..Default::default()
        };
        assert!(g.resolved(Some(&on)).wildcard_single_only, "方案写了 ⇒ 覆盖");
        let global_on = CodetableGlobal {
            wildcard_single_only: true,
            ..Default::default()
        };
        assert!(
            global_on
                .resolved(Some(&crate::schema::CodeTableSpec::default()))
                .wildcard_single_only,
            "方案没写 ⇒ 回落全局"
        );
        assert!(
            !global_on.resolved(Some(&on)).wildcard,
            "与主开关正交：开仅单字不连带开通配"
        );
    }
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput/wind_input
env $T cargo test -p wind-config --lib codetable_wildcard_single_only_folds_from_schema
```

Expected: 编译失败 `no field wildcard_single_only`。

- [ ] **Step 3: 实现**
  - `schema.rs`：`wildcard_key` 字段后加
    ```rust
    /// 通配仅单字（`None` = 跟随全局 `schema.codetable.wildcard_single_only`）。
    /// 见 `docs/design/codetable-reverse-mode.md` §2。
    #[serde(default)]
    pub wildcard_single_only: Option<bool>,
    ```
  - `config.rs`：`CodetableGlobal` 的 `wildcard_key` 之后加字段（文档：「通配结果只留单字（一个字素簇），行内通配与反查模式都受它管；
    与 `wildcard` 主开关正交。见 reverse-mode spec §2」，`#[serde(default)]`）；`Default` 加 `wildcard_single_only: false,`；
    `resolved` 在 `wildcard_key` 那段之后加 `if let Some(v) = o.wildcard_single_only { out.wildcard_single_only = v; }`。
  - `config_schema.rs`：`REGISTRY` 在 `f("schema.codetable.wildcard_key", Str),` 后加 `f("schema.codetable.wildcard_single_only", Bool),`；
    `schema_overridden_keys` 在 `wildcard_key` 那行后加 `("schema.codetable.wildcard_single_only", ct.wildcard_single_only.is_some()),`。
  - `data/config.toml`：`wildcard_key = "z"` 之后加
    ```toml
    # 通配仅单字：通配结果只留单字（行内通配与反查模式都生效），词组不出。方案级，这里是全局基线。
    wildcard_single_only = false
    ```

- [ ] **Step 4: 跑，确认绿 + 守门测试**

```bash
env $T cargo test -p wind-config --no-fail-fast
env $T cargo test -p wind-rpc --test wind_setting_assets
```

Expected: wind-config 全绿（`registry_covers_every_config_key` / `data_config_toml_covers_registry` / `data_config_toml_has_no_orphan_keys` 在内）；
`wind_setting_assets` 红在「manifest 未覆盖 `schema.codetable.wildcard_single_only`」一类——**预期**，Task 3 补齐，本任务不处理。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-config/src/{schema,config,config_schema}.rs
git -C . branch --show-current && git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-config/src/schema.rs wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml <<'EOF'
feat(config): 码表通配仅单字开关 wildcard_single_only

方案级三态折叠，与 wildcard 主开关正交，出厂关（reverse-mode spec §2）。
wind_setting_assets 跨仓守门待 wind-setting 同步后转绿。
EOF
git show --stat HEAD
```

---

## Task 2: 引擎 `wildcard_query` 内核 + 单字过滤（先滤后截）

**Files:**
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`CommitOptions` ~218；`convert_wildcard` ~907-946 拆出 `wildcard_query`；`mod tests` 末尾）
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（`mod tests`：`ct_wildcard` ~2494 改为薄包装；末尾加测试）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`build_engine` 码表分支 `CommitOptions` 字面量 ~5773）

**Interfaces:**
- Produces: `CommitOptions::wildcard_single_only: bool`；私有 `fn CodeTableEngine::wildcard_query(&self, input: &str, pattern: &str, max_candidates: usize) -> ConvertResult`
- 签名不变：`fn convert_wildcard(&self, input: &str, pattern: &str, max_candidates: usize) -> Option<ConvertResult>`（= `self.opts.wildcard?; Some(self.wildcard_query(..))`）
- Consumes: `wind_candidate::single_markable_char(&str) -> Option<&str>`

- [ ] **Step 1: 写失败测试**

`codetable/engine.rs` 的 `mod tests` 末尾：

```rust
    fn single_only_opts() -> CommitOptions {
        CommitOptions {
            wildcard: Some('z'),
            wildcard_single_only: true,
            ..Default::default()
        }
    }

    /// ★ Review Focus 2：先滤后截。`q??` 下 150 条高权重词组压着 120 条低权重单字：
    /// 词库层按 limit 截出的前 100 条全是词组，事后过滤只剩 0 条。引擎须加倍重取到凑满。
    #[test]
    fn wildcard_single_only_filters_before_truncation() {
        let mut owned: Vec<(String, String, i32)> = Vec::new();
        for i in 0..150u32 {
            let c1 = (b'a' + (i / 26) as u8) as char;
            let c2 = (b'a' + (i % 26) as u8) as char;
            owned.push((format!("q{c1}{c2}"), format!("词组{i}"), 10_000 - i as i32));
        }
        for i in 0..120u32 {
            let c1 = (b'a' + (i / 26) as u8) as char;
            let c2 = (b'a' + (i % 26) as u8) as char;
            let ch = char::from_u32(0x4E00 + i).unwrap();
            owned.push((format!("q{c1}{c2}"), ch.to_string(), 1));
        }
        let refs: Vec<(&str, &str, i32)> = owned
            .iter()
            .map(|(c, t, w)| (c.as_str(), t.as_str(), *w))
            .collect();
        let off = engine_opts(&refs, wildcard_opts(CommitOptions::default()));
        let r = off.convert_wildcard("qzz", &slot_pattern("q??"), 100).unwrap();
        assert!(
            r.candidates.iter().all(|c| c.text.starts_with("词组")),
            "前置：不过滤时前 100 条全是词组"
        );
        let on = engine_opts(&refs, single_only_opts());
        let r = on.convert_wildcard("qzz", &slot_pattern("q??"), 100).unwrap();
        assert_eq!(r.candidates.len(), 100, "过滤后仍凑满 100 条");
        assert!(
            r.candidates
                .iter()
                .all(|c| wind_candidate::single_markable_char(&c.text).is_some())
        );
        let r = on.convert_wildcard("qzz", &slot_pattern("q??"), 500).unwrap();
        assert_eq!(r.candidates.len(), 120, "词库取尽即停，不死循环");
    }

    /// 单字判据是字素簇（UAX #29），不是 `chars().count()`：ZWJ 序列、国旗算一个字（issue #83）。
    #[test]
    fn wildcard_single_only_counts_grapheme_clusters() {
        let e = engine_opts(
            &[
                ("qa", "👨‍👩‍👧", 40),
                ("qb", "🇨🇳", 30),
                ("qc", "工作", 20),
                ("qd", "工", 10),
            ],
            single_only_opts(),
        );
        let r = e.convert_wildcard("qz", &slot_pattern("q?"), 50).unwrap();
        let got: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(got, ["👨‍👩‍👧", "🇨🇳", "工"]);
    }
```

`mixed/engine.rs` 的 `mod tests`：把 `fn ct_wildcard(entries) -> Box<dyn Engine>` 的函数体搬进
`fn ct_wildcard_with(entries: &[(&str, &str, i32)], single_only: bool) -> Box<dyn Engine>`（`CommitOptions` 加
`wildcard_single_only: single_only`），`ct_wildcard` 改为 `ct_wildcard_with(entries, false)`；末尾追加：

```rust
    /// reverse-mode spec §2：混输下仅单字只作用于通配那一侧，字面混输（拼音）的多字词照出。
    #[test]
    fn wildcard_single_only_spares_literal_side() {
        let e = MixedEngine::new(
            ct_wildcard_with(&[("qa", "甲", 10), ("qb", "乙丙", 20)], true),
            Some(Box::new(FakePinyinTable {
                entries: vec![("qz", "阿紫")],
            })),
            None,
            MixConfig::default(),
        );
        let r = e.convert_wildcard("qz", &slot("q?"), 50).unwrap();
        let texts: Vec<&str> = r.candidates.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"甲"), "{texts:?}");
        assert!(!texts.contains(&"乙丙"), "通配侧的词组被滤：{texts:?}");
        assert!(texts.contains(&"阿紫"), "拼音侧多字词不受影响：{texts:?}");
    }
```

- [ ] **Step 2: 跑，确认红**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput/wind_input
env $T cargo test -p wind-engine --lib wildcard
```

Expected: 编译失败 `no field wildcard_single_only on CommitOptions`。

- [ ] **Step 3: 实现**
  - `CommitOptions` 在 `wildcard` 之后加字段（文档：「通配结果只留单字（字素簇）。行内通配与反查模式共用 `wildcard_query`，二者同受约束。
    见 reverse-mode spec §2」）。
  - `convert_wildcard` 函数体改为 `self.opts.wildcard?; Some(self.wildcard_query(input, pattern, max_candidates))`，原查询体搬进
    `fn wildcard_query`，结构如下（`limit == 0` 不下传、硬上限、`truncate(limit)` 均保留原样）：
    ```rust
    fn wildcard_query(&self, input: &str, pattern: &str, max_candidates: usize) -> ConvertResult {
        let n = pattern.chars().count();
        let with_prefix = !self.opts.single_code_input;
        let limit = max_candidates.min(WILDCARD_RESULT_LIMIT);
        let mut hits: Vec<Candidate> = Vec::new();
        if limit > 0 {
            // ★ 词库层自己按 `fetch` 截断（各层两档取额），过滤只能在它之后。仅单字时不够就 ×2 重取，
            // 直到够数 / 取尽 / 硬上限（计划裁决 1，同生僻字模式的 refill）。关着时只查一轮 ⇒ 与原实现逐条相同。
            let mut fetch = limit;
            loop {
                let got = self.dm.search_pattern(pattern, wind_dict::WILDCARD_SLOT, fetch, with_prefix);
                let exhausted = got.len() < fetch;
                hits = got;
                if !self.opts.wildcard_single_only {
                    break;
                }
                hits.retain(|c| wind_candidate::single_markable_char(&c.text).is_some());
                if hits.len() >= limit || exhausted || fetch >= WILDCARD_RESULT_LIMIT {
                    break;
                }
                fetch = fetch.saturating_mul(2).min(WILDCARD_RESULT_LIMIT);
            }
        }
        // 以下 map（source / is_exact_code / comment / is_wildcard）、sort（cmp_exact_first → base_cmp）、
        // truncate(limit)、ConvertResult 组装与原 convert_wildcard 逐行相同。
        …
    }
    ```
  - `manager.rs` `build_engine` 码表分支的 `CommitOptions` 字面量在 `wildcard: eff.wildcard_char(schema_id),` 后加
    `wildcard_single_only: eff.wildcard_single_only,`（其余 `CommitOptions { .. }` 字面量都有 `..Default::default()` 或走 `default()`，
    编译器会指出遗漏处，按 false 补）。

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
env $T cargo test -p wind-engine --lib wildcard
env $T cargo test -p wind-engine --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail -20
```

Expected: 全绿；既有 `wildcard_*` 用例（含 `wildcard_hard_cap_matches_expansion_cap_and_zero_is_empty`、
`wildcard_respects_single_code_input_and_max_candidates`、混输三条 `wildcard_merge*`）不变。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs
git -C . branch --show-current && git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): 通配仅单字——按字素簇过滤，先滤后截

词库层 search_pattern 已按 limit 截断，事后过滤只剩被截段里的单字，首批凑不满、
has_more 判假。查询体抽成 wildcard_query，仅单字时不够就加倍重取，直到够数、取尽
或到硬上限（reverse-mode spec §2）；关着时只查一轮，与原实现逐条相同。混输只作用于
主码表通配那一侧。
EOF
git show --stat HEAD
```

---

## Task 3: wind-setting 设置项「通配仅出单字」

**Files:**
- Modify: `../wind-setting/src/assets/settings_manifest.toml`（`schema.codetable.wildcard_key` 那条 `[[items]]` 之后）
- Modify: `../wind-setting/src/dialogs/schema_codetable.rs`（`SPEC_BEHAVIOR_FIELDS` ~111）
- Regenerate: `../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json`

**Interfaces:** 无代码接口。

- [ ] **Step 1: manifest 与方案对话框**

`settings_manifest.toml` 在 `key = "schema.codetable.wildcard_key"` 那个 `[[items]]` 块之后插入：

```toml
# 通配仅单字：行内通配与反查模式都受它管，故不挂 enabled_when（反查模式不看通配主开关）。
# 单字判据是字素簇（core 的 wind_candidate::single_markable_char）。
[[items]]
key = "schema.codetable.wildcard_single_only"
group = "schema"
section = "上屏行为"
subsection = "常用功能"
type = "toggle"
label = "通配仅出单字"
hint = "通配与反查模式的结果只列单字，词组不出。不影响正常打字"
```

`schema_codetable.rs` 的 `SPEC_BEHAVIOR_FIELDS` 在 `"wildcard_key",` 后加 `"wildcard_single_only",`（注释：「仅单字按方案覆盖，理由同通配两项」）。

- [ ] **Step 2: core 侧对账 + 重生成 + 全量**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput/wind_input && env $T cargo test -p wind-rpc --test wind_setting_assets
cd .. && ./scripts/dev.sh sg && ./scripts/dev.sh st 2>&1 | tail -15
git -C ../wind-setting diff --stat
```

Expected: `wind_setting_assets` 绿；`st` 全绿（含 `table_covers_every_title_char`）；diff 只含 manifest、`schema_codetable.rs`、
snapshot（新增一条 `schema.codetable.wildcard_single_only`，带 `schemaOverride`）、mockdata（`"wildcard_single_only": false`）
与可能的 `appVersion` 一行（后者不提交：`git -C ../wind-setting checkout -p -- src/mockdata/config.json` 只撤那一 hunk，
或手工改回；**不许** `git stash`）。

- [ ] **Step 3: 提交（wind-setting 仓）**

```bash
cd /home/dufeng/develop/windinput/wt-rev/wind-setting
git branch --show-current && git diff --cached --name-status
git commit -F - --only -- src/assets/settings_manifest.toml src/dialogs/schema_codetable.rs src/assets/capabilities.snapshot.json src/mockdata/config.json <<'EOF'
feat(settings): 码表「通配仅出单字」开关

方案级覆盖同通配两项；capability 快照与 mockdata 重生成。
EOF
git show --stat HEAD
```

---

## Task 4: P1 真实数据验收

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（文件末尾新增一节）

**Interfaces:**
- Consumes（文件既有）: `data_dir()`、`dict_ready()`、`mixed_ready()`、`wubi(bool, &str) -> Config`、`wubi_pinyin(bool) -> Config`、
  `wubi_hit(&str, &str, bool) -> bool`、`press`、`press_vk`、`VK_NEXT`、`page_until_more_than(&Coordinator, usize) -> usize`、`no_relax(Config) -> Config`

本任务用例在 Task 1/2 之后**首跑即绿**，是验收与锁；每条注明缺哪一任务时会红。

- [ ] **Step 1: 追加用例**

```rust
// ─────────────────── 通配仅单字（reverse-mode spec §2） ───────────────────

fn single_only(mut cfg: Config) -> Config {
    cfg.schema.codetable.wildcard_single_only = true;
    cfg
}

fn is_single(t: &str) -> bool {
    wind_candidate::single_markable_char(t).is_some()
}

/// spec §5 列的冒烟串：`azz` 开仅单字后全是单字（`a??` 等长本就全是单字，首批不足以测截断先后）。
#[test]
fn single_only_azz_smoke() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(single_only(wubi(true, "z")), Some(&data_dir()));
    press(&coord, "azz");
    let texts = coord.debug_all_candidate_texts();
    assert!(!texts.is_empty());
    assert!(texts.iter().all(|t| is_single(t)), "{texts:?}");
}

/// ★ Review Focus 2（真实数据）：`azzz`（`a???`）按权重前 100 条只有 ~38 个单字；
/// 先滤后截才能让首批凑满 100 个单字、`has_more` 为真、翻页可扩。缺 Task 2 时红在条数。
#[test]
fn single_only_azzz_first_batch_is_full_of_singles() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let base = Coordinator::new_headless(no_relax(wubi(true, "z")), Some(&data_dir()));
    press(&base, "azzz");
    let base_texts = base.debug_all_candidate_texts();
    assert!(
        base_texts.iter().filter(|t| is_single(t)).count() < 100,
        "前置：不过滤时首批里单字不足 100"
    );
    let coord = Coordinator::new_headless(
        no_relax(single_only(wubi(true, "z"))),
        Some(&data_dir()),
    );
    press(&coord, "azzz");
    let texts = coord.debug_all_candidate_texts();
    assert!(texts.iter().all(|t| is_single(t)), "{texts:?}");
    assert!(coord.debug_has_more(), "a??? 单字 1342 个，首批应回满");
    let first = coord.debug_candidate_count();
    let grown = page_until_more_than(&coord, first);
    assert!(grown > first, "翻到边界应扩充：{first} -> {grown}");
    assert!(coord.debug_all_candidate_texts().iter().all(|t| is_single(t)));
}

/// spec §2 混输：只作用于通配那一侧，拼音「汉字」照出。
#[test]
fn mixed_single_only_keeps_pinyin_words() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let coord = Coordinator::new_headless(single_only(wubi_pinyin(true)), Some(&data_dir()));
    press(&coord, "hanz");
    let tri = coord.debug_candidate_triples();
    assert!(
        tri.iter()
            .filter(|(_, c, _)| wubi_hit("hanz", c, true) || wubi_hit("hanz", c, false))
            .all(|(t, _, _)| is_single(t)),
        "通配侧全是单字：{tri:?}"
    );
    assert!(tri.iter().any(|(t, _, _)| t == "汉字"), "拼音侧词组照出：{tri:?}");
}
```

若 `wind_candidate` 不在 wind-coordinator 的 dev-dependencies 可见范围：它是 wind-coordinator 的普通依赖（`Cargo.toml` 已有），集成测试可直接用；否则改走
`coord.debug_*` 返回的文本自行 `unicode_segmentation` 判定——**不许**退回 `chars().count()`。

- [ ] **Step 2: 跑**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput/wind_input
time env $T cargo test -p wind-coordinator --test codetable_wildcard --no-fail-fast 2>&1 | tail -20
```

Expected: 全绿、耗时 ≥ 1s、无「跳过」。核对点：`single_only_azzz_first_batch_is_full_of_singles` 若红在「前置」，说明检索范围把
`a???` 首批的词组滤掉了一截——给两个 coordinator 都加 `cfg.input.filter_mode = "gb18030".into()` 再判；仍红则回报，不改断言方向。

- [ ] **Step 3: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs
git -C . branch --show-current && git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
test(wildcard): 通配仅单字的真实数据验收

azz 冒烟；azzz 钉住先滤后截（按权重前 100 条只有 38 个单字，过滤后仍须回满并可翻页扩充）；
混输只作用于通配侧、拼音词组照出（reverse-mode spec §2）。
EOF
git show --stat HEAD
```

---

## Task 5: P1 文档 + 全量测试 + 回归核对

**Files:**
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`（`### 通配输入` 小节，~103）
- Modify: `../WindInputDocs/content/docs/guides/config/schema.mdx`（`[schema.codetable]` 代码块与参考表，~36-66）
- Modify: `docs/design/codetable-reverse-mode.md`（头部状态行）、`docs/architecture/engine-candidate-pipeline.md`（§3.4 通配「引擎」一条）

- [ ] **Step 1: 文档站**
  - `codetable.mdx` `### 通配输入` 的列表里，在「结果先列一批…」那条之后插入：
    ```mdx
    - 只想查单字：打开**通配仅出单字** <Since v="0.125" />，通配结果只列单字（emoji、国旗这类显示为一个字的也算），词组不出；条数与翻页照旧。它同样管[反查模式](/docs/guides/reverse-mode)，不影响正常打字。
    ```
    （`/docs/guides/reverse-mode` 在 P3 才建：P1 这条链接先写成 `[反查模式]` 纯文字不加链接，P3 Task 22 再补链接——`lint:links` 不许坏链。）
  - `guides/config/schema.mdx`：代码块在 `leading_chars` 行后加三行
    ```toml
    wildcard = false              # 通配输入：组码时通配键代替一个码元
    wildcard_key = "z"            # 通配键：单个字母或 ASCII 符号
    wildcard_single_only = false  # 通配仅出单字（行内通配与反查模式）
    ```
    参考表在 `leading_chars` 行后加三行：
    ```mdx
    | `wildcard` <Since v="0.124" /> | 布尔 | — | `false` | [通配输入](/docs/settings/schema/codetable#wildcard)：组码时通配键代替恰好一个码元，候选显示完整编码 |
    | `wildcard_key` <Since v="0.124" /> | 字符串 | 单个字母或 ASCII 符号 | `"z"` | 通配键。数字与空格不可用，非法则视为关闭 |
    | `wildcard_single_only` <Since v="0.125" /> | 布尔 | — | `false` | 通配结果只列单字（按显示上的一个字判断）。行内通配与反查模式都生效，与 `wildcard` 正交 |
    ```

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInputDocs && pnpm install --frozen-lockfile && pnpm lint && node scripts/check-config-coverage.mjs 2>&1 | tail -1
```

Expected: lint 通过；缺失数 **13**（wildcard 两键补上、`wildcard_single_only` 不在清单）。

- [ ] **Step 2: 设计与架构文档**
  - `codetable-reverse-mode.md` 头部状态行改为 `> **状态：P1（A 通配仅单字）已实施；P2、P3 待实施。** 实施计划见 `codetable-reverse-mode-plan.md`。`
    ——保留原「承接…」一句。
  - `engine-candidate-pipeline.md` §3.4「**引擎**」一条末尾追加：「`wildcard_single_only` 开时在 `wildcard_query` 内按字素簇过滤、
    不够则 ×2 重取（先滤后截，reverse-mode spec §2）。」

- [ ] **Step 3: 全量测试与编译门**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput/wind_input
time env $T cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-rev/p1.log | grep -E "^test result|FAILED|panicked" | tail -40
grep -E "^test result" /tmp/wct-rev/p1.log | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
env $T cargo check-headless && cd .. && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev ./scripts/dev.sh k
```

Expected: `failed` 同基线；`passed` = 基线 + **7**（Task 1：1；Task 2：码表 2 + 混输 1；Task 4：3）。两道编译门通过。

- [ ] **Step 4: 回归与提交范围自查、两仓提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
base=$(git merge-base HEAD main)
git log --oneline "$base"..HEAD && git diff "$base"..HEAD --stat
git diff "$base"..HEAD | grep -nE "^\+.*(TODO|todo!|unimplemented!|#\[ignore\]|\.only\(|chars\(\)\.count\(\) == 1)" || echo clean
git diff "$base"..HEAD --name-only | grep -x "wind_input/Cargo.lock" && echo "!! Cargo.lock 被提交" || echo "lock ok"
git commit -F - --only -- docs/design/codetable-reverse-mode.md docs/architecture/engine-candidate-pipeline.md <<'EOF'
docs(design): 通配仅单字落地——设计稿状态与架构文档 §3.4
EOF
cd ../WindInputDocs && git branch --show-current && git diff --cached --name-status
git commit -F - --only -- content/docs/settings/schema/codetable.mdx content/docs/guides/config/schema.mdx <<'EOF'
docs(settings): 通配仅出单字；配置参考补 wildcard 三键
EOF
git show --stat HEAD
```

Expected: `clean`、`lock ok`。回归核对：`wildcard_off_by_default_changes_nothing`、`mixed_overlength_is_identical_to_off` 等既有通配用例仍绿（已在全量内）。

---

# P2 — C：通配 / 注释反查含未启用扩展词库

## Task 6: `Candidate::from_disabled_dict` 与显示序键

**Files:**
- Modify: `wind_input/crates/wind-candidate/src/candidate.rs`（`is_wildcard` 字段 ~456 之后；`Default` ~596；`candidate_display_order` ~970 `.then_with(|| cmp_exact_first(a, b))` 之后；`mod tests` 末尾）

**Interfaces:**
- Produces: `Candidate::from_disabled_dict: bool`（`#[serde(skip)]`，默认 false）

- [ ] **Step 1: 写失败测试**（`candidate.rs` `mod tests` 末尾）

```rust
    fn dd(text: &str, exact: bool, weight: i32, disabled: bool) -> Candidate {
        Candidate {
            text: text.into(),
            code: "uuia".into(),
            source: CandidateSource::CodeTable,
            is_exact_code: exact,
            is_wildcard: true,
            weight,
            from_disabled_dict: disabled,
            ..Default::default()
        }
    }

    /// reverse-mode spec §4.2 / 计划裁决 4：同一精确档内已启用的排前，未启用库的排后——
    /// 不论权重；但不跨档（未启用的等长仍先于已启用的更长补全）。
    #[test]
    fn disabled_dict_sorts_after_enabled_within_exact_tier() {
        let mut v = vec![
            dd("影等长", true, 9999, true),
            dd("启更长", false, 9999, false),
            dd("启等长", true, 1, false),
        ];
        v.sort_by(|a, b| candidate_display_order(a, b, false, false, "uuiz"));
        let got: Vec<&str> = v.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(got, ["启等长", "影等长", "启更长"]);
    }

    /// 两条都不是未启用库候选时该键恒 Equal ⇒ 既有次序逐条不变。
    #[test]
    fn disabled_flag_never_reorders_enabled_candidates() {
        let a = dd("甲", true, 10, false);
        let b = dd("乙", true, 20, false);
        assert_eq!(
            a.from_disabled_dict.cmp(&b.from_disabled_dict),
            std::cmp::Ordering::Equal
        );
        let mut v = vec![a, b];
        v.sort_by(|x, y| candidate_display_order(x, y, false, false, "uuiz"));
        assert_eq!(v[0].text, "乙", "仍按权重");
    }
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-candidate --lib disabled_` → 编译失败 `no field from_disabled_dict`。

- [ ] **Step 3: 实现**
  - 字段（紧随 `is_wildcard`）：文档「该候选来自**未启用的扩展词库**（影子层，`DisabledDictLayers`，reverse-mode spec §4.2）。
    只由码表通配 / 反查查询置位；显示序在同一精确档内把它排在已启用候选之后。引擎内部用，不推送 UI。」`#[serde(skip)] pub from_disabled_dict: bool,`
  - `Default`：`from_disabled_dict: false,`
  - `candidate_display_order`：`.then_with(|| cmp_exact_first(a, b))` 之后插 `.then(a.from_disabled_dict.cmp(&b.from_disabled_dict))`，
    上方注释写明「全序：按布尔分两区，两条非未启用候选间恒 Equal ⇒ 不改任何既有次序（同 `is_draft` 论证）；放在 `cmp_exact_first` 之后
    ⇒ 不跨等长 / 更长两档」。

- [ ] **Step 4: 跑，确认绿**：`env $T cargo test -p wind-candidate --no-fail-fast` 全绿。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
rustfmt --edition 2024 --check wind_input/crates/wind-candidate/src/candidate.rs
git -C . branch --show-current && git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-candidate/src/candidate.rs <<'EOF'
feat(candidate): 未启用扩展库候选标记 from_disabled_dict，同档内排在已启用之后

显示序会整体重排引擎结果，「已启用排前」只能落在比较键上（reverse-mode spec §4.2）。
键在 cmp_exact_first 之后，不跨等长 / 更长两档；两条非未启用候选间恒 Equal，既有次序不变。
EOF
git show --stat HEAD
```

---

## Task 7: 配置字段 `schema.codetable.lookup_disabled_dicts`

**Files:** 同 Task 1 的四个文件（位置紧跟 Task 1 加的 `wildcard_single_only`）。

**Interfaces:**
- Produces: `CodetableGlobal::lookup_disabled_dicts: bool`、`CodeTableSpec::lookup_disabled_dicts: Option<bool>`、注册键 `schema.codetable.lookup_disabled_dicts`（`Bool`）

- [ ] **Step 1: 写失败测试**（config.rs `mod tests`）

```rust
    /// reverse-mode spec §4.2：一个方案级开关，三态折叠，出厂关。
    #[test]
    fn codetable_lookup_disabled_dicts_folds_from_schema() {
        let g = CodetableGlobal::default();
        assert!(!g.lookup_disabled_dicts, "出厂关闭");
        let on = crate::schema::CodeTableSpec {
            lookup_disabled_dicts: Some(true),
            ..Default::default()
        };
        assert!(g.resolved(Some(&on)).lookup_disabled_dicts);
        let global_on = CodetableGlobal {
            lookup_disabled_dicts: true,
            ..Default::default()
        };
        let off = crate::schema::CodeTableSpec {
            lookup_disabled_dicts: Some(false),
            ..Default::default()
        };
        assert!(!global_on.resolved(Some(&off)).lookup_disabled_dicts, "方案显式关压过全局");
    }
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-config --lib codetable_lookup_disabled_dicts` → 编译失败。

- [ ] **Step 3: 实现**：照 Task 1 Step 3 的四处各加一份（字段文档：「通配、反查模式与候选注释反查可查到**未启用**的扩展词库
  （`DictSpec::is_enabled() == false`）；普通打字候选不受影响。见 reverse-mode spec §4」）。`data/config.toml`：
  ```toml
  # 通配与反查含扩展词库：通配、反查模式、候选注释里的编码反查也查「未启用」的扩展词库（如行政区域），
  # 已启用的排前。普通打字候选不受影响。方案级，这里是全局基线。
  lookup_disabled_dicts = false
  ```

- [ ] **Step 4: 跑**：`env $T cargo test -p wind-config --no-fail-fast` 全绿（`wind_setting_assets` 预期红，Task 13 前补齐）。

- [ ] **Step 5: 格式与提交**（同 Task 1 的四个路径）

```bash
git commit -F - --only -- wind_input/crates/wind-config/src/schema.rs wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml <<'EOF'
feat(config): 通配与反查含未启用扩展词库开关 lookup_disabled_dicts

方案级三态折叠，出厂关（reverse-mode spec §4.2）。
EOF
```

---

## Task 8: 影子层 `DisabledDictLayers` 与引擎合并

**Files:**
- Create: `wind_input/crates/wind-engine/src/codetable/disabled_dicts.rs`
- Modify: `wind_input/crates/wind-engine/src/codetable/mod.rs`（`pub mod disabled_dicts; pub use disabled_dicts::{DisabledDictLayers, DisabledDictSource};`）
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`CodeTableEngine` 字段 + `with_disabled_dicts` + `disabled_dicts()`；`wildcard_query` 合并；`set_dict_enabled` 禁用分支；`mod tests`）

**Interfaces:**
- Produces:
  ```rust
  pub type DictLoader = Arc<dyn Fn() -> anyhow::Result<CachedDict> + Send + Sync>;
  pub struct DisabledDictSource { pub id: String, pub base_order: i32, pub default_weight: Option<i32>, pub load: DictLoader }
  pub struct DisabledDictLayers { /* declared, enabled: Mutex<HashSet<String>>, wnorm, loaded: Mutex<Option<Arc<DictManager>>>, loads: AtomicUsize */ }
  impl DisabledDictLayers {
      pub fn new(declared: Vec<DisabledDictSource>, enabled: impl IntoIterator<Item = String>, wnorm: Option<wind_dict::WeightNorm>) -> Self;
      pub fn search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<Candidate>;
      pub fn mark_disabled(&self, id: &str);
      pub fn load_count(&self) -> usize;
  }
  impl CodeTableEngine {
      pub fn with_disabled_dicts(self, d: DisabledDictLayers) -> Self;
      pub fn disabled_dicts(&self) -> Option<&DisabledDictLayers>;
  }
  ```
- 行为（数据结构 / 加载时机 / 失效 / 去重排序 / 缺失处理，spec §4.2 的落地）：
  - **结构**：`declared` = 本方案**全部**扩展库来源（不含主库，含已启用的）；`enabled` = 当前挂在主 `DictManager` 上的扩展库 id；
    影子集合 = `declared − enabled`。已加载时是一个**独立的** `DictManager`（层名 `codetable-disabled-<id>`，
    `SystemDictLayer::with_enabled(dict, name, true).with_base_order(..).with_default_weight(..).with_weight_norm(wnorm)`），
    **从不**注册进引擎的主 `dm`。
  - **加载时机**：`search_pattern` 首次被调用时在 `loaded` 锁内逐源调用 `load()`；成功的注册、失败的 `warn!("未启用扩展词库 {id} 加载失败，跳过：{e}")`
    后跳过（不阻塞、不重试到下次失效前）；`loads` 计每次「建一遍影子 DictManager」+1。
  - **失效**：`mark_disabled(id)` ⇒ `enabled.remove(id)`、`loaded = None`；启用方向不经这里（引擎失效重建）。
  - **引擎合并**（`wildcard_query` 的每一轮 fetch 内）：`enabled_hits = dm.search_pattern(..fetch..)`，
    `disabled_hits = disabled.search_pattern(..fetch..)`（字段为 `None` 时空）；`exhausted = enabled_hits.len() < fetch && disabled_hits.len() < fetch`；
    以 `(text, code)` 为键先收 `enabled_hits`，再追加键未出现过的 `disabled_hits`（置 `from_disabled_dict = true`）；之后单字过滤、
    `sort_by(cmp_exact_first → from_disabled_dict → base_cmp)`、`truncate(limit)`。
  - **普通 `convert` 不读此字段**（契约 3）。

- [ ] **Step 1: 写失败测试**

`disabled_dicts.rs` 自带 `#[cfg(test)] mod tests`：

```rust
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wind_dict::codetable::CodetableDict;

    fn mem_source(id: &str, entries: &[(&str, &str, i32)], calls: Arc<AtomicUsize>) -> DisabledDictSource {
        let owned: Vec<(String, String, i32)> = entries
            .iter()
            .map(|(c, t, w)| (c.to_string(), t.to_string(), *w))
            .collect();
        DisabledDictSource {
            id: id.into(),
            base_order: 3,
            default_weight: None,
            load: Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                let mut d = CodetableDict::empty();
                for (i, (c, t, w)) in owned.iter().enumerate() {
                    d.merge_single(c.clone(), t.clone(), *w, i as i32);
                }
                Ok(CachedDict::Memory(d))
            }),
        }
    }

    fn slot(p: &str) -> String {
        p.replace('?', &wind_dict::WILDCARD_SLOT.to_string())
    }

    /// spec §4.2：首次查询才加载，之后复用；启用集里的库不进影子。
    #[test]
    fn loads_lazily_once_and_skips_enabled() {
        let calls = Arc::new(AtomicUsize::new(0));
        let d = DisabledDictLayers::new(
            vec![
                mem_source("xz", &[("uuia", "门头沟区", 1)], calls.clone()),
                mem_source("ext", &[("uuib", "已启用库", 1)], calls.clone()),
            ],
            ["ext".to_string()],
            None,
        );
        assert_eq!(d.load_count(), 0, "构造不加载");
        let texts = |v: Vec<Candidate>| v.into_iter().map(|c| c.text).collect::<Vec<_>>();
        assert_eq!(texts(d.search_pattern(&slot("uui?"), wind_dict::WILDCARD_SLOT, 10, true)), ["门头沟区"]);
        d.search_pattern(&slot("uui?"), wind_dict::WILDCARD_SLOT, 10, true);
        assert_eq!(d.load_count(), 1, "第二次复用");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "只读了影子集合里的那一个库");
    }

    /// 禁用一个库 ⇒ 它进影子集合、已加载的作废重建。
    #[test]
    fn mark_disabled_extends_set_and_drops_loaded() {
        let calls = Arc::new(AtomicUsize::new(0));
        let d = DisabledDictLayers::new(
            vec![
                mem_source("xz", &[("uuia", "门头沟区", 1)], calls.clone()),
                mem_source("ext", &[("uuib", "已启用库", 1)], calls.clone()),
            ],
            ["ext".to_string()],
            None,
        );
        d.search_pattern(&slot("uui?"), wind_dict::WILDCARD_SLOT, 10, true);
        d.mark_disabled("ext");
        let got: Vec<String> = d
            .search_pattern(&slot("uui?"), wind_dict::WILDCARD_SLOT, 10, true)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert!(got.contains(&"已启用库".to_string()) && got.contains(&"门头沟区".to_string()), "{got:?}");
        assert_eq!(d.load_count(), 2);
    }

    /// 文件缺失 / 解析失败：跳过该库、warn，不影响其余库，也不 panic。
    #[test]
    fn failing_source_is_skipped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let broken = DisabledDictSource {
            id: "gone".into(),
            base_order: 3,
            default_weight: None,
            load: Arc::new(|| anyhow::bail!("no such file")),
        };
        let d = DisabledDictLayers::new(
            vec![broken, mem_source("xz", &[("uuia", "门头沟区", 1)], calls)],
            std::iter::empty(),
            None,
        );
        let got = d.search_pattern(&slot("uui?"), wind_dict::WILDCARD_SLOT, 10, true);
        assert_eq!(got.len(), 1);
    }
```

`codetable/engine.rs` 的 `mod tests` 末尾：

```rust
    fn disabled_mem(id: &str, entries: &[(&str, &str, i32)]) -> super::super::DisabledDictSource {
        let owned: Vec<(String, String, i32)> = entries
            .iter()
            .map(|(c, t, w)| (c.to_string(), t.to_string(), *w))
            .collect();
        super::super::DisabledDictSource {
            id: id.into(),
            base_order: 3,
            default_weight: None,
            load: Arc::new(move || {
                let mut d = CodetableDict::empty();
                for (i, (c, t, w)) in owned.iter().enumerate() {
                    d.merge_single(c.clone(), t.clone(), *w, i as i32);
                }
                Ok(CachedDict::Memory(d))
            }),
        }
    }

    fn with_xz(e: CodeTableEngine) -> CodeTableEngine {
        e.with_disabled_dicts(super::super::DisabledDictLayers::new(
            vec![disabled_mem("xz", &[("uuia", "门头沟区", 9999), ("uuia", "重码", 9999)])],
            std::iter::empty(),
            None,
        ))
    }

    /// ★ Review Focus 1：普通 convert（精确 / 前缀 / 活码探针同一入口）永不读影子层，连加载都不触发。
    #[test]
    fn plain_convert_never_touches_disabled_layers() {
        let e = with_xz(engine_opts(
            &[("uuif", "立法", 10)],
            wildcard_opts(CommitOptions::default()),
        ));
        for input in ["uuia", "uui", "u"] {
            let r = e.convert(input, 50).unwrap();
            assert!(r.candidates.iter().all(|c| c.text != "门头沟区"), "{input}");
        }
        assert_eq!(e.disabled_dicts().unwrap().load_count(), 0, "普通 convert 不触发加载");
        e.convert_wildcard("uuiz", &slot_pattern("uui?"), 50).unwrap();
        assert_eq!(e.disabled_dicts().unwrap().load_count(), 1);
        assert!(e.convert("uuia", 50).unwrap().candidates.iter().all(|c| c.text != "门头沟区"));
    }

    /// spec §4.2：`(text, code)` 去重（同键留已启用那条），已启用排前，档内再按 base_sort。
    #[test]
    fn wildcard_merges_disabled_after_enabled_dedup_by_text_code() {
        let e = with_xz(engine_opts(
            &[("uuif", "立法", 10), ("uuia", "重码", 1)],
            wildcard_opts(CommitOptions::default()),
        ));
        let r = e.convert_wildcard("uuiz", &slot_pattern("uui?"), 50).unwrap();
        let got: Vec<(&str, &str, bool)> = r
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.code.as_str(), c.from_disabled_dict))
            .collect();
        assert_eq!(
            got,
            [("立法", "uuif", false), ("重码", "uuia", false), ("门头沟区", "uuia", true)]
        );
    }

    /// 计划裁决 5：禁用一个已加载的扩展库 ⇒ 主 dm 摘层（普通候选消失），同时它进影子层（通配仍可见）；
    /// 返回值语义不变（true = 目标态已达成）。
    #[test]
    fn set_dict_enabled_disable_moves_dict_into_disabled_layers() {
        let build = |entries: &[(&str, &str, i32)]| {
            let mut d = CodetableDict::empty();
            for (i, (code, text, w)) in entries.iter().enumerate() {
                d.merge_single(code.to_string(), text.to_string(), *w, i as i32);
            }
            CachedDict::Memory(d)
        };
        let dm = Arc::new(DictManager::new());
        dm.register_layer(Box::new(SystemDictLayer::new(build(&[("uuif", "立法", 10)]), "codetable-system")));
        dm.register_layer(Box::new(SystemDictLayer::new(build(&[("aaae", "甘蓝菜", 50)]), "codetable-extra-ext")));
        let e = CodeTableEngine::new(4, wildcard_opts(CommitOptions::default()), dm)
            .with_own_extra_dicts(["ext".to_string()])
            .with_disabled_dicts(super::super::DisabledDictLayers::new(
                vec![disabled_mem("ext", &[("aaae", "甘蓝菜", 50)])],
                ["ext".to_string()],
                None,
            ));
        assert!(e.set_dict_enabled("ext", false));
        assert!(e.convert("aaae", 20).unwrap().candidates.iter().all(|c| c.text != "甘蓝菜"));
        let r = e.convert_wildcard("aaaz", &slot_pattern("aaa?"), 20).unwrap();
        assert!(r.candidates.iter().any(|c| c.text == "甘蓝菜" && c.from_disabled_dict));
    }
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-engine --lib disabled` → 编译失败（模块 / 方法不存在）。

- [ ] **Step 3: 实现**：按 Interfaces 的「行为」逐条实现。`CodeTableEngine` 新字段 `disabled_dicts: Option<super::DisabledDictLayers>`
  （文档写明「只在 `wildcard_query` 读——普通 `convert` 读它就是把未启用库漏进打字候选，spec §4.2 的保证全靠这一条」），`new` 里置 `None`；
  `set_dict_enabled` 在 `unregister_layer` 之后、`return true` 之前加 `if let Some(d) = &self.disabled_dicts { d.mark_disabled(dict_id); }`；
  `wildcard_query` 按 Interfaces 合并（Task 2 的重取循环体内），`map` 闭包不动 `from_disabled_dict`（合并时已置位），
  排序改为 `cmp_exact_first(a, b).then(a.from_disabled_dict.cmp(&b.from_disabled_dict)).then_with(|| base_cmp(a, b))`。

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
env $T cargo test -p wind-engine --lib disabled_dicts wildcard set_dict_enabled
env $T cargo test -p wind-engine --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail -20
```

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
git add -- wind_input/crates/wind-engine/src/codetable/disabled_dicts.rs
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/codetable/{disabled_dicts,engine,mod}.rs
git -C . branch --show-current && git diff --cached --name-status
git commit -F - --only -- wind_input/crates/wind-engine/src/codetable/disabled_dicts.rs wind_input/crates/wind-engine/src/codetable/mod.rs wind_input/crates/wind-engine/src/codetable/engine.rs <<'EOF'
feat(engine): 影子层 DisabledDictLayers——通配查询合并未启用扩展词库

独立于主 DictManager 懒加载（首次通配查询才读盘、失败 warn 跳过），按 (text, code)
去重、已启用排前（reverse-mode spec §4.2）。普通 convert 不读它。禁用扩展库时引擎
摘层后同步影子层的启用集，返回值语义不变。
EOF
git show --stat HEAD
```

---

## Task 9: `build_engine` 注入影子层 + 自造夹具集成测试

**Files:**
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`load_codetable_layers` ~5960 旁新增 `fn disabled_extra_sources`；`build_engine` 码表分支 ~5836 `CodeTableEngine::new(..)` 链上接 `.with_disabled_dicts(..)`；`mod tests` 加一条）
- Create: `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs`

**Interfaces:**
- Produces: `fn EngineManager::disabled_extra_sources(schema: &Schema, schemas_dir: &Path) -> Vec<DisabledDictSource>`
  ——与 `load_codetable_layers` 同一选库口径（`usable` = path 非空；`main_idx` = 首个 `default`，无则 0；其余全部为扩展库，**不看** `is_enabled`），
  `load` 闭包 = `CachedDict::load_at_with(&full, &cache_path(&full, "wdat"), is_english)`（`full = Self::resolve_dict_file(&e.path, schemas_dir)`）。
- `build_engine`：`eff.lookup_disabled_dicts` 为真且 `disabled_extra_sources` 非空时，
  `DisabledDictLayers::new(sources, <本次 load_codetable_layers 实际加载的扩展 id>, weight_norm_of(&schema))`；否则不挂（零开销，契约 1/4）。
  「实际加载的扩展 id」从 `layers` 的 `name.strip_prefix("codetable-extra-")` 收（在 `for l in layers` 消费前先收集）。

- [ ] **Step 1: 写失败测试**

`manager.rs` `mod tests`：

```rust
    /// 影子层来源与 load_codetable_layers 同口径：主库除外、扩展库全收（含已启用），
    /// 两边选库规则一旦漂移，就会出现「某库既不在主层也不在影子层」。
    #[test]
    fn disabled_extra_sources_cover_every_declared_extra() {
        let schema: Schema = toml::from_str(
            "[schema]\nid = \"t\"\n[engine]\ntype = \"codetable\"\n\
             [[dictionaries]]\nid = \"main\"\npath = \"t/m.dict.yaml\"\ndefault = true\n\
             [[dictionaries]]\nid = \"on\"\npath = \"t/on.dict.yaml\"\ndefault_enabled = true\n\
             [[dictionaries]]\nid = \"off\"\npath = \"t/off.dict.yaml\"\ndefault_enabled = false\n\
             [[dictionaries]]\nid = \"nopath\"\npath = \"\"\n",
        )
        .unwrap();
        let ids: Vec<String> = EngineManager::disabled_extra_sources(&schema, Path::new("/nonexistent"))
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["on", "off"]);
    }
```

`tests/lookup_disabled_dicts.rs`（新文件，夹具供 Task 10 复用）：

```rust
//! 未启用扩展词库的通配 / 注释反查（reverse-mode spec §4），自造夹具，不依赖 build_dev/data。

use std::path::{Path, PathBuf};
use wind_config::Config;
use wind_engine::EngineManager;

fn uid(tag: &str) -> String {
    format!("zz_ldd_{tag}_{}", std::process::id())
}

pub struct Cleanup {
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

/// 主库：工 a / 立法 uuif；已启用扩展 `_ext`：甘蓝菜 aaae；未启用扩展 `_xz`：门头沟区 uuia。
/// `xz_file = false` ⇒ `_xz` 的 path 指向不存在的文件（缺失处理）。`with_xz = false` ⇒ 不声明 `_xz`。
fn write_fixture(dir: &Path, id: &str, with_xz: bool, xz_file: bool) {
    let s = dir.join("schemas");
    std::fs::create_dir_all(s.join(id)).unwrap();
    let mut toml = format!(
        "[schema]\nid = \"{id}\"\nname = \"测影\"\n[engine]\ntype = \"codetable\"\n\
         [engine.codetable]\nmax_code_length = 4\n\
         [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n\
         [[dictionaries]]\nid = \"{id}_ext\"\npath = \"{id}/ext.dict.yaml\"\ntype = \"rime_codetable\"\n\
         default_enabled = true\nbase_order = 1\n"
    );
    if with_xz {
        toml += &format!(
            "[[dictionaries]]\nid = \"{id}_xz\"\npath = \"{id}/xz.dict.yaml\"\ntype = \"rime_codetable\"\n\
             default_enabled = false\nbase_order = 3\ndefault_weight = 500\n"
        );
    }
    std::fs::write(s.join(format!("{id}.schema.toml")), toml).unwrap();
    let dict = |name: &str, body: &str| {
        format!("---\nname: {name}\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n{body}")
    };
    std::fs::write(s.join(format!("{id}/main.dict.yaml")), dict("main", "a\t工\t100\nuuif\t立法\t10\n")).unwrap();
    std::fs::write(s.join(format!("{id}/ext.dict.yaml")), dict("ext", "aaae\t甘蓝菜\t50\n")).unwrap();
    if with_xz && xz_file {
        std::fs::write(s.join(format!("{id}/xz.dict.yaml")), dict("xz", "uuia\t门头沟区\t0\n")).unwrap();
    }
}

fn setup(tag: &str, on: bool, with_xz: bool, xz_file: bool) -> (EngineManager, String, Cleanup) {
    let id = uid(tag);
    let dir = std::env::temp_dir().join(format!("wind_ldd_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    let guard = Cleanup { id: id.clone(), dir: dir.clone() };
    write_fixture(&dir, &id, with_xz, xz_file);
    (manager(&dir, &id, on), id, guard)
}

fn manager(dir: &Path, id: &str, on: bool) -> EngineManager {
    let mut cfg = Config::default();
    cfg.schema.available = vec![id.into()];
    cfg.schema.active = id.into();
    cfg.schema.codetable.wildcard = true;
    cfg.schema.codetable.lookup_disabled_dicts = on;
    EngineManager::with_store_override(&cfg, Some(dir), None, Some(dir.join("ov")))
}

fn slot(p: &str) -> String {
    p.replace('?', &wind_dict::WILDCARD_SLOT.to_string())
}

fn wc_texts(m: &EngineManager, input: &str, pat: &str) -> Vec<String> {
    m.convert_wildcard(input, &slot(pat), 50)
        .unwrap_or_default()
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn wildcard_sees_disabled_extra_only_when_switch_on() {
    let (on, _id, _g) = setup("on", true, true, true);
    assert_eq!(wc_texts(&on, "uuiz", "uui?"), ["立法", "门头沟区"]);
    let (off, _id2, _g2) = setup("off", false, true, true);
    assert_eq!(wc_texts(&off, "uuiz", "uui?"), ["立法"]);
}

/// ★ Review Focus 1（接线层）：开关开着，普通 convert 照旧看不到未启用库。
#[test]
fn plain_convert_ignores_disabled_extra_with_switch_on() {
    let (m, _id, _g) = setup("plain", true, true, true);
    for input in ["uuia", "uui"] {
        assert!(m.convert(input, 50).candidates.iter().all(|c| c.text != "门头沟区"), "{input}");
    }
}

/// 计划裁决 5：热禁用已启用扩展库 ⇒ 普通候选消失、通配经影子层仍可见。
#[test]
fn live_disable_moves_extra_into_lookup() {
    let (m, id, _g) = setup("live", true, true, true);
    assert!(m.convert("aaae", 20).candidates.iter().any(|c| c.text == "甘蓝菜"));
    assert!(m.set_dict_enabled_live(&id, &format!("{id}_ext"), false));
    assert!(m.convert("aaae", 20).candidates.iter().all(|c| c.text != "甘蓝菜"));
    assert!(wc_texts(&m, "aaaz", "aaa?").contains(&"甘蓝菜".to_string()));
}

/// spec §4.3：未启用库文件缺失 ⇒ 跳过（warn），通配照常出主库结果。
#[test]
fn missing_disabled_file_is_skipped() {
    let (m, _id, _g) = setup("missing", true, true, false);
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法"]);
}
```

- [ ] **Step 2: 跑，确认红**

```bash
env $T cargo test -p wind-engine --lib disabled_extra_sources
env $T cargo test -p wind-engine --test lookup_disabled_dicts
```

Expected: 前者编译失败（函数不存在）；后者 `wildcard_sees_disabled_extra_only_when_switch_on` / `live_disable_moves_extra_into_lookup` 红（影子层未接线），其余两条绿（锁）。

- [ ] **Step 3: 实现**：按 Interfaces。`build_engine` 注入点紧跟 `.with_own_extra_dicts(Self::declared_extra_dict_ids(&schema))`（~5838），英文分支（~5557）**不接**。

- [ ] **Step 4: 跑，确认绿**：同 Step 2 两条 + `env $T cargo test -p wind-engine --test engine_manager --test invalidate_cascades_to_mixed`。

- [ ] **Step 5: 格式与提交**

```bash
git add -- wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/manager.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs
git commit -F - --only -- wind_input/crates/wind-engine/src/manager.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs <<'EOF'
feat(engine): 码表方案按 lookup_disabled_dicts 挂影子层

来源与 load_codetable_layers 同口径（主库除外、扩展库全收），启用集取本次实际加载的层；
开关关或无扩展库时不挂（reverse-mode spec §4.2）。英文分支不接。
EOF
git show --stat HEAD
```

---

## Task 10: 反查索引「含未启用库」变体

**Files:**
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（反查索引一族 ~1159-1700、~1887、`set_dict_enabled_live` ~3204、`invalidate_schema` ~3370、`reload_from_config` ~3481、`reverse_index_keeps` ~6828；`mod tests`）
- Modify: `wind_input/crates/wind-engine/src/lib.rs`（`pub use manager::ReverseScope`）
- Modify: `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs`（追加）

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
  pub enum ReverseScope { Enabled, WithDisabled }
  const WITH_DISABLED_KEY_SUFFIX: &str = "\u{1}with_disabled"; // 内存键后缀；\u{1} 不可能出现在方案 id 里
  fn reverse_index_key(schema_id: &str, scope: ReverseScope) -> String;
  fn reverse_index_base(key: &str) -> &str;               // 去后缀
  pub fn comment_reverse_scope(&self, schema_id: &str) -> ReverseScope;  // 带缓存
  pub fn reverse_index_if_ready_in(&self, schema_id: &str, scope: ReverseScope) -> Option<Arc<ReverseIndex>>;
  pub fn prewarm_reverse_index_in(&self, schema_id: &str, scope: ReverseScope) -> bool;
  pub fn reverse_index_skipped_in(&self, schema_id: &str, scope: ReverseScope) -> bool;
  pub fn is_building_reverse_index_in(&self, schema_id: &str, scope: ReverseScope) -> bool;
  pub fn word_codes_display_for_comment(&self, schema_id: &str, text: &str) -> Option<String>;
  ```
  既有无 scope 的同名函数全部改为 `…_in(schema_id, ReverseScope::Enabled)` 的薄包装——调用方零改动。
- 变体的具体规则：
  - **选库**：`WithDisabled` 用 `Self::all_dict_specs(schema)`（path 非空的全部 `[[dictionaries]]`，不看 `is_enabled`）；`Enabled` 仍 `enabled_dict_specs`。
    `load_dicts_individually` 加一个 `scope` 参数选前者或后者（加载逻辑不变）。
  - **缓存 key**：内存表 `reverse_index` / `reverse_index_skipped` / `index_build_locks` 一律用 `reverse_index_key(..)`；磁盘
    `reverse_index_cache_path_in(root, schema_id, scope)` → `<root>/<key>/<key>.wridx`（Enabled，与现状逐字节同）或
    `<root>/<key>/<key>.with_disabled.wridx`（`key` = `schema_cache_key(schema_id)`）；fp 文件随路径分开（`derived_cache_is_fresh` 按路径旁 `.fp`），
    build_guard 键随路径分开。
  - **淘汰**：`reverse_index_keeps(reverse_index_base(k), requested_base, primary, pins)`——变体与其基名同去留。
  - **范围判定** `comment_reverse_scope(sid)`：`codetable_settings_of(sid).lookup_disabled_dicts` 且方案里至少一个扩展库 `!is_enabled()`
    ⇒ `WithDisabled`，否则 `Enabled`（裁决 7）；结果缓存在新字段 `reverse_scope_cache: Mutex<HashMap<String, ReverseScope>>`，
    与 `reverse_index` **同批清空**（`set_dict_enabled_live` / `invalidate_schema` / `reload_from_config` 三处，紧跟 `reverse_index.clear()`）。
  - **消费面**：`codetable_reverse_hint(text)` 改用 `text_codes_in(&primary, self.comment_reverse_scope(&primary))`；新增
    `word_codes_display_for_comment` = `word_codes_display` 的变体版。`word_codes_in` / `text_codes` / `word_codes_display` 不变（裁决 6）。
  - **内存**：变体同样落盘后 mmap（`REVERSE_INDEX_RESIDENT_MAX` 规则不变），Private 只多出常驻小索引那一档；只在开关开且有未启用库时才建。

- [ ] **Step 1: 写失败测试**

`manager.rs` `mod tests`：

```rust
    /// ★ Review Focus 4（路径层）：变体与常规索引同目录、不同文件；常规路径与旧实现逐字节一致。
    #[test]
    fn reverse_index_variant_path_differs() {
        let root = Path::new("/c");
        let a = EngineManager::reverse_index_cache_path_in(root, "wubi86", ReverseScope::Enabled).unwrap();
        let b = EngineManager::reverse_index_cache_path_in(root, "wubi86", ReverseScope::WithDisabled).unwrap();
        assert_eq!(a, root.join("wubi86").join("wubi86.wridx"));
        assert_eq!(b, root.join("wubi86").join("wubi86.with_disabled.wridx"));
    }

    #[test]
    fn reverse_index_keeps_variant_with_its_base() {
        let k = reverse_index_key("wubi86", ReverseScope::WithDisabled);
        assert_eq!(reverse_index_base(&k), "wubi86");
        assert!(reverse_index_keeps(reverse_index_base(&k), "stroke", "wubi86", &[]));
        assert!(!reverse_index_keeps(reverse_index_base(&reverse_index_key("old", ReverseScope::WithDisabled)), "stroke", "wubi86", &[]));
    }
```

同时把既有 `reverse_index_path_is_scoped_by_schema_id_only` / `single_char_codes_cache_sits_next_to_the_reverse_index` 里的
`reverse_index_cache_path_in(root, id)` 调用补上 `ReverseScope::Enabled` 实参（断言不变）。

`tests/lookup_disabled_dicts.rs` 追加（顶部加 `use wind_engine::ReverseScope;`）：

```rust
/// spec §4.2：注释反查变体含未启用库；加词查重（word_codes_in）与悬停（word_codes_display）口径不变（裁决 6）。
#[test]
fn comment_reverse_variant_includes_disabled_extra() {
    let (m, id, _g) = setup("rev", true, true, true);
    assert_eq!(m.comment_reverse_scope(&id), ReverseScope::WithDisabled);
    assert_eq!(m.word_codes_display_for_comment(&id, "门头沟区"), None, "没就绪 ≠ 查不到");
    assert!(m.prewarm_reverse_index_in(&id, ReverseScope::WithDisabled));
    assert!(m.prewarm_reverse_index(&id));
    assert_eq!(m.word_codes_display_for_comment(&id, "门头沟区").as_deref(), Some("uuia"));
    assert_eq!(m.codetable_reverse_hint("门头沟区").as_deref(), Some("uuia"));
    assert_eq!(m.word_codes_in(&id, "门头沟区").as_deref(), Some(""), "加词查重只认启用集");
    assert_eq!(m.word_codes_display(&id, "门头沟区").as_deref(), Some(""), "悬停口径不变");
}

/// ★ Review Focus 4：两份索引分文件、内容不同；换一个 manager（重启）后各自复用自己那份，不串。
#[test]
fn variant_and_regular_indexes_use_separate_files() {
    let Some(root) = Config::cache_dir() else {
        eprintln!("跳过：无缓存根");
        return;
    };
    let (m, id, g) = setup("files", true, true, true);
    assert!(m.prewarm_reverse_index(&id));
    assert!(m.prewarm_reverse_index_in(&id, ReverseScope::WithDisabled));
    let dir = root.join(&id);
    let regular = dir.join(format!("{id}.wridx"));
    let variant = dir.join(format!("{id}.with_disabled.wridx"));
    assert!(regular.exists() && variant.exists(), "{:?}", std::fs::read_dir(&dir).map(|r| r.count()));
    assert_ne!(std::fs::read(&regular).unwrap(), std::fs::read(&variant).unwrap());
    drop(m);
    let m2 = manager(&g.dir, &id, true);
    assert!(m2.prewarm_reverse_index(&id));
    assert_eq!(m2.word_codes_in(&id, "门头沟区").as_deref(), Some(""), "常规索引复用后仍不含未启用库");
    assert!(m2.prewarm_reverse_index_in(&id, ReverseScope::WithDisabled));
    assert_eq!(m2.word_codes_display_for_comment(&id, "门头沟区").as_deref(), Some("uuia"));
}

/// 裁决 7：开关关 / 方案没有未启用库 ⇒ 退化为常规索引，不另建文件。
#[test]
fn scope_collapses_to_enabled() {
    let (off, id, _g) = setup("scope_off", false, true, true);
    assert_eq!(off.comment_reverse_scope(&id), ReverseScope::Enabled);
    let (none, id2, _g2) = setup("scope_none", true, false, true);
    assert_eq!(none.comment_reverse_scope(&id2), ReverseScope::Enabled);
}

/// 失效：启用集变了，变体与范围缓存一并作废。
#[test]
fn variant_invalidated_on_dict_toggle() {
    let (m, id, _g) = setup("inval", true, true, true);
    assert!(m.prewarm_reverse_index_in(&id, ReverseScope::WithDisabled));
    m.set_dict_enabled_live(&id, &format!("{id}_ext"), false);
    assert!(m.reverse_index_if_ready_in(&id, ReverseScope::WithDisabled).is_none());
}
```

（`Cleanup` 的 `dir` 字段在 `variant_and_regular_indexes_use_separate_files` 里被读：把 `struct Cleanup` 的两个字段改 `pub`。）

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-engine --lib reverse_index` 与 `--test lookup_disabled_dicts` → 编译失败。

- [ ] **Step 3: 实现**：按 Interfaces。`build_reverse_index_for(schema_id)` 改为 `build_reverse_index_for(schema_id, scope)`；
  `reverse_index_for` 同理；所有 `.get(schema_id)` / `.insert(schema_id…)` / `.contains(schema_id)` 改用 `reverse_index_key`。
  `info!` 日志里的 `schema_id` 保持为方案 id（不打印后缀），末尾加 `scope={:?}`。

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
env $T cargo test -p wind-engine --lib reverse_index single_char_codes
env $T cargo test -p wind-engine --test lookup_disabled_dicts --test reverse_index_cache --test text_codes --test single_char_codes_cache --test aux_code_sources
```

Expected: 全绿。`reverse_index_cache` / `text_codes` 是「常规索引行为不变」的锁。

- [ ] **Step 5: 格式与提交**

```bash
rustfmt --edition 2024 --check wind_input/crates/wind-engine/src/{manager,lib}.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs
git commit -F - --only -- wind_input/crates/wind-engine/src/manager.rs wind_input/crates/wind-engine/src/lib.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs <<'EOF'
feat(engine): 反查索引加「含未启用扩展库」变体，只供候选注释反查

内存键带 \u{1}with_disabled 后缀、磁盘 <key>.with_disabled.wridx，与常规索引分文件
分指纹，淘汰按基名同去留（reverse-mode spec §4.2）。加词查重、悬停、辅助码仍只认
启用集。方案无未启用库或开关关时退化为常规索引。三处失效点一并清范围缓存。
EOF
git show --stat HEAD
```

---

## Task 11: 协调器注释反查走变体 + 后台预热

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/comment.rs`（`eval_text_var` 的 `code_rev_all | code_all` ~1108-1112；`eval_var` 的同名臂 ~1264-1267；两处 `code_rev | code` 臂在返回 `None` 时触发预热；文件末尾新测试模块）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`prewarm_indexes` ~4429；`spawn_index_warm` ~4502 抽成 `spawn_index_warm_in`；新增 `warm_comment_reverse_index`）

**Interfaces:**
- Produces:
  - `pub(crate) fn Coordinator::spawn_index_warm_in(&self, schema_id: &str, scope: wind_engine::ReverseScope, with_single_char: bool)`
    （原 `spawn_index_warm` 体搬入，所有 `reverse_index_if_ready` / `reverse_index_skipped` / `is_building_reverse_index` / `prewarm_reverse_index`
    换成 `_in(.., scope)`；`spawn_index_warm(id, w)` = `spawn_index_warm_in(id, Enabled, w)`，调用方零改动）
  - `pub(crate) fn Coordinator::warm_comment_reverse_index(&self)`：`sid = code_source_schema()`；`scope = comment_reverse_scope(&sid)`；
    `scope == WithDisabled` 时 `spawn_index_warm_in(&sid, scope, false)`。
- `prewarm_indexes`：`ids` 循环之后，对 `code_source_schema()` 若 `comment_reverse_scope == WithDisabled` 则
  `prewarm_reverse_index_in(&sid, WithDisabled)`（阻塞版，预热线程上跑）。
- `comment.rs`：`code_rev_all | code_all` 两臂 `word_codes_display` → `word_codes_display_for_comment`；四个臂在引擎返回 `None` 时调
  `self.warm_comment_reverse_index()`（去重靠 `is_building_reverse_index_in`，契约 5）。

- [ ] **Step 1: 写失败测试**（`comment.rs` 末尾新模块；夹具同 Task 9 形状，写在模块里）

```rust
#[cfg(test)]
mod comment_reverse_scope_tests {
    //! 候选注释反查走「含未启用库」变体（reverse-mode spec §4.2）。自造夹具，不依赖 build_dev/data。
    use crate::coordinator::Coordinator;
    use std::path::PathBuf;
    use wind_config::Config;

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

    fn coord(tag: &str, on: bool) -> (std::sync::Arc<Coordinator>, Cleanup) {
        let id = format!("zz_crs_{tag}_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("wind_crs_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("schemas");
        std::fs::create_dir_all(s.join(&id)).unwrap();
        std::fs::write(
            s.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"注\"\n[engine]\ntype = \"codetable\"\n\
                 [engine.codetable]\nmax_code_length = 4\n\
                 [[dictionaries]]\nid = \"{id}_m\"\npath = \"{id}/m.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n\
                 [[dictionaries]]\nid = \"{id}_xz\"\npath = \"{id}/xz.dict.yaml\"\ntype = \"rime_codetable\"\ndefault_enabled = false\n"
            ),
        )
        .unwrap();
        let dict = |body: &str| format!("---\nname: d\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n{body}");
        std::fs::write(s.join(format!("{id}/m.dict.yaml")), dict("a\t工\t100\n")).unwrap();
        std::fs::write(s.join(format!("{id}/xz.dict.yaml")), dict("uuia\t门头沟区\t0\n")).unwrap();
        let mut cfg = Config::default();
        cfg.schema.available = vec![id.clone()];
        cfg.schema.active = id.clone();
        cfg.schema.codetable.lookup_disabled_dicts = on;
        (Coordinator::new_headless(cfg, Some(&dir)), Cleanup { id, dir })
    }

    #[test]
    fn prewarm_indexes_builds_comment_variant_when_switch_on() {
        let (c, _g) = coord("on", true);
        c.prewarm_indexes();
        assert_eq!(c.engine_mgr.codetable_reverse_hint("门头沟区").as_deref(), Some("uuia"));
        let (off, _g2) = coord("off", false);
        off.prewarm_indexes();
        assert_eq!(off.engine_mgr.codetable_reverse_hint("门头沟区").as_deref(), Some(""));
    }
}
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-coordinator --lib comment_reverse_scope_tests` → 开关开那半红（`None`：变体没被预热）。

- [ ] **Step 3: 实现**：按 Interfaces。

- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
env $T cargo test -p wind-coordinator --lib comment handle_aux_code handle_direct_aux handle_addword
env $T cargo test -p wind-coordinator --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail -30
```

- [ ] **Step 5: 格式与提交**

```bash
rustfmt --edition 2024 --check wind_input/crates/wind-coordinator/src/{comment,coordinator}.rs
git commit -F - --only -- wind_input/crates/wind-coordinator/src/comment.rs wind_input/crates/wind-coordinator/src/coordinator.rs <<'EOF'
feat(coordinator): 候选注释反查含未启用扩展库，未就绪时后台建

code_rev / code / code_rev_all / code_all 走「含未启用库」变体（reverse-mode spec §4.2）；
索引没好本次不显示、后台建，启动预热一并带上。加词查重与悬停编码段口径不变。
EOF
git show --stat HEAD
```

---

## Task 12: wind-setting 设置项「通配与反查含扩展词库」

**Files:** 同 Task 3（manifest、`schema_codetable.rs`、两份生成物）。

- [ ] **Step 1**：`settings_manifest.toml` 在 Task 3 加的 `wildcard_single_only` 块之后插入：

```toml
# 未启用的扩展词库（is_enabled()==false，如五笔「行政区域」）也参与通配、反查模式与候选注释反查；
# 普通打字候选不变。影子层首次通配才加载（core 的 DisabledDictLayers）。
[[items]]
key = "schema.codetable.lookup_disabled_dicts"
group = "schema"
section = "上屏行为"
subsection = "常用功能"
type = "toggle"
label = "通配与反查含扩展词库"
hint = "通配、反查模式与候选注释里的编码反查，也查没有启用的扩展词库（排在已启用词库之后）；仅用于通配与反查，不影响普通候选"
```

`SPEC_BEHAVIOR_FIELDS` 加 `"lookup_disabled_dicts",`。

- [ ] **Step 2 / Step 3**：同 Task 3 Step 2 / Step 3，提交信息：

```
feat(settings): 码表「通配与反查含扩展词库」开关

方案级覆盖；文案写明仅用于通配与反查、不影响普通候选。capability 快照与 mockdata 重生成。
```

---

## Task 13: P2 真实数据验收

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（末尾新增一节）

- [ ] **Step 1: 追加用例**

```rust
// ─────────────── 通配含未启用扩展词库（reverse-mode spec §4） ───────────────
// 样本：「门头沟区 uuia」只在未启用的 wubi86_xzqy（default_enabled = false），主库 / extra 无 uuia 码。

fn lookup_disabled(mut cfg: Config) -> Config {
    cfg.schema.codetable.lookup_disabled_dicts = true;
    cfg
}

fn tri_of(cfg: Config, keys: &str) -> Vec<(String, String, String)> {
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, keys);
    coord.debug_candidate_triples()
}

#[test]
fn lookup_disabled_dicts_inline_wildcard_sees_xzqy() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let off = tri_of(wubi(true, "z"), "uuiz");
    assert!(off.iter().all(|(t, _, _)| t != "门头沟区"), "对照：关时不可见");
    let on = tri_of(lookup_disabled(wubi(true, "z")), "uuiz");
    let pos = on
        .iter()
        .position(|(t, c, _)| t == "门头沟区" && c == "uuia")
        .unwrap_or_else(|| panic!("开后应可见：{on:?}"));
    let last_enabled_equal = on
        .iter()
        .rposition(|(t, c, _)| c.len() == 4 && t != "门头沟区" && off.iter().any(|(ot, oc, _)| ot == t && oc == c))
        .unwrap();
    assert!(last_enabled_equal < pos, "已启用的等长结果全在它之前：{on:?}");
}

/// ★ Review Focus 1（真实数据）：开关开着，普通打字的候选逐条不变（含前缀补全与活码字母）。
#[test]
fn lookup_disabled_dicts_leaves_plain_typing_identical() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    for keys in ["uuia", "uui", "djma", "a", "zz"] {
        assert_eq!(
            tri_of(lookup_disabled(wubi(true, "z")), keys),
            tri_of(wubi(true, "z"), keys),
            "{keys}"
        );
    }
}

/// spec §4.2：混输取主方案的开关，通配侧同样可见。
#[test]
fn mixed_lookup_disabled_dicts_follows_primary() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let on = tri_of(lookup_disabled(wubi_pinyin(true)), "uuiz");
    assert!(on.iter().any(|(t, c, _)| t == "门头沟区" && c == "uuia"), "{on:?}");
}
```

- [ ] **Step 2: 跑**：`time env $T cargo test -p wind-coordinator --test codetable_wildcard --no-fail-fast 2>&1 | tail -20` → 全绿、≥1s、无「跳过」。
  核对点：`lookup_disabled_dicts_leaves_plain_typing_identical` 若红在 `zz`，看是否是 headless 下 `zz` 无短语导致两边都空——空 == 空仍绿；若不等则是泄漏，回 Task 8/9 修。

- [ ] **Step 3: 格式与提交**

```bash
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs <<'EOF'
test(wildcard): 通配含未启用扩展库的真实数据验收

uuiz 开关前后「门头沟区」可见性、已启用等长结果全在其前；普通打字逐键不变；
混输取主方案开关（reverse-mode spec §4）。
EOF
```

---

## Task 14: P2 文档 + 全量测试 + 回归核对

**Files:**
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`、`../WindInputDocs/content/docs/guides/config/schema.mdx`
- Modify: `docs/design/codetable-reverse-mode.md`（状态行）、`docs/architecture/engine-candidate-pipeline.md`（§3.4）

- [ ] **Step 1: 文档站**
  - `codetable.mdx` 通配输入列表里 Task 5 那条之后插入：
    ```mdx
    - 想查没启用的扩展词库：打开**通配与反查含扩展词库** <Since v="0.125" />，没启用的扩展词库（如五笔的「行政区域」）也参与通配、反查模式和候选注释里的编码反查，排在已启用词库的结果之后，例如 `uuiz` 会列出「门头沟区 uuia」。普通打字的候选不受影响；第一次通配时才读这些词库，注释里的编码要等后台建好索引才出现。
    ```
  - `guides/config/schema.mdx`：代码块加 `lookup_disabled_dicts = false  # 通配 / 反查 / 注释反查也查未启用的扩展词库`；
    参考表加 `| \`lookup_disabled_dicts\` <Since v="0.125" /> | 布尔 | — | \`false\` | 通配、反查模式与候选注释反查也查未启用（\`default_enabled = false\` 且未手动启用）的扩展词库，已启用的排前；普通打字候选不变 |`

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInputDocs && pnpm lint && node scripts/check-config-coverage.mjs 2>&1 | tail -1
```

Expected: lint 通过；缺失 13。

- [ ] **Step 2: 设计与架构文档**：状态行改为「P1、P2（C 影子层 + 反查索引变体）已实施；P3 待实施」；
  `engine-candidate-pipeline.md` §3.4「**引擎**」追加一条「`lookup_disabled_dicts`：`DisabledDictLayers`（影子层）独立于主 `DictManager`、
  首次通配懒加载，`wildcard_query` 按 `(text, code)` 去重合并、`from_disabled_dict` 同档沉后；反查索引变体 `ReverseScope::WithDisabled`
  只供候选注释四变量」。

- [ ] **Step 3: 全量测试与编译门**（命令同 Task 5 Step 3，日志 `/tmp/wct-rev/p2.log`）

Expected: `failed` 同基线；`passed` = P1 结束时 + **24**（Task 6：2；Task 7：1；Task 8：`disabled_dicts` 3 + 引擎 3；Task 9：单测 1 + 集成 4；
Task 10：单测 2 + 集成 4；Task 11：1；Task 13：3）。不符时列出多出 / 缺少的测试名。

- [ ] **Step 4: 回归与提交范围自查、两仓提交**：同 Task 5 Step 4（grep 模式加 `from_disabled_dict` 之外无新增 `pub` 泄漏检查：
  `git diff "$base"..HEAD -- wind_input/crates/wind-engine/src/codetable/engine.rs | grep -n "disabled_dicts" ` 只应出现在
  字段声明、`new`、`with_disabled_dicts` / `disabled_dicts()`、`set_dict_enabled`、`wildcard_query` 与测试里——出现在 `fn convert` 体内即 Review Focus 1 破了）。
  提交信息：WindInput `docs(design): 通配与反查含未启用扩展库落地——设计稿状态与架构文档 §3.4`；
  WindInputDocs `docs(settings): 通配与反查含扩展词库`。

---

# P3 — B：反查模式

## Task 15: 配置 `BoundAction::Reverse` 与 `input.reverse`

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`（`BoundAction` 枚举 ~1514 `RareChar` 之后；值域文档 ~1492；`only_in_chinese_mode` ~1670；`parse` ~1762；`InputConfig` ~3799 `rare_char` 之后；`Default` ~3917；`RareCharConfig` 之后新增 `ReverseConfig`；`CodetableGlobal` 新方法 `reverse_wildcard_char`；测试 ~13280 与 ~6983）
- Modify: `wind_input/crates/wind-config/src/hotkey.rs`（`hotkey_policy_for` ~84）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（`REGISTRY` ~485 之后）
- Modify: `data/config.toml`（`[input.rare_char]` 段 ~974 之后）

**Interfaces:**
- Produces:
  - `BoundAction::Reverse`（动词 `"reverse"`；`only_in_chinese_mode() == true`；热键策略 `GLOBAL`）
  - `pub struct ReverseConfig { pub enabled: bool, pub candidate_layout: LayoutIntent }`（`#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]`，
    `candidate_layout` 带 `#[serde(default, deserialize_with = "crate::tolerant_de::tolerant")]`）；`InputConfig::reverse: ReverseConfig`
  - `pub fn CodetableGlobal::reverse_wildcard_char(&self) -> char` = `parse_wildcard_key(&self.wildcard_key).unwrap_or('z')`（不告警：合法性告警归 `wildcard_char`）
  - 注册键 `input.reverse.enabled`（`Bool`）、`input.reverse.candidate_layout`（`Enum(LAYOUT_INTENT_VALUES)`）

- [ ] **Step 1: 写失败测试**（config.rs `mod tests`；并把 `"reverse"` 加进 `chinese_only_matches_action_family` 的第一组数组）

```rust
    #[test]
    fn reverse_bound_action_parses() {
        assert_eq!(BoundAction::parse("reverse"), BoundAction::Reverse);
        assert_eq!(BoundAction::parse(" Reverse "), BoundAction::Reverse);
        assert_eq!(BoundAction::parse("reversex"), BoundAction::None, "未知动词仍回落 None");
    }

    #[test]
    fn reverse_config_defaults_off_and_parses() {
        let c = Config::default();
        assert!(!c.input.reverse.enabled, "出厂关（reverse-mode spec §3.1）");
        assert_eq!(c.input.reverse.candidate_layout, LayoutIntent::Follow);
        let t: ReverseConfig =
            toml::from_str("enabled = true\ncandidate_layout = \"vertical\"\n").unwrap();
        assert!(t.enabled);
        assert_eq!(t.candidate_layout, LayoutIntent::Vertical);
        let bad: ReverseConfig = toml::from_str("candidate_layout = \"diagonal\"\n").unwrap();
        assert_eq!(bad.candidate_layout, LayoutIntent::Follow, "非法值容错回落（tolerant_de）");
    }

    /// spec §3.2：模式内通配键取方案键；键非法（含通配关时的出厂 "z"）一律回落 z。
    #[test]
    fn reverse_wildcard_char_falls_back_to_z() {
        let g = CodetableGlobal::default();
        assert_eq!(g.reverse_wildcard_char(), 'z');
        let q = CodetableGlobal { wildcard_key: "?".into(), ..Default::default() };
        assert_eq!(q.reverse_wildcard_char(), '?', "不看主开关");
        let bad = CodetableGlobal { wildcard_key: "12".into(), ..Default::default() };
        assert_eq!(bad.reverse_wildcard_char(), 'z');
    }
```

`hotkey.rs` 的测试模块加：

```rust
    #[test]
    fn reverse_hotkey_policy_is_global_chinese_only() {
        let p = hotkey_policy_for(&crate::config::BoundAction::Reverse).unwrap();
        assert_eq!(p & HOTKEY_POLICY_CHINESE_ONLY, HOTKEY_POLICY_CHINESE_ONLY);
        assert_eq!(p & HOTKEY_POLICY_GLOBAL, HOTKEY_POLICY_GLOBAL);
    }
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-config --lib reverse` → 编译失败。

- [ ] **Step 3: 实现**
  - `BoundAction::Reverse` 文档：「进反查模式：输入**本方案编码**（含通配，任何位置都可通配），只查主码表（reverse-mode spec §3）。无载荷，单例。」
    值域文档加 `- "reverse"：进反查模式（当前方案的编码，通配可在首位）`。`parse` 的 `match lower` 加 `"reverse" => Self::Reverse,`；
    `only_in_chinese_mode` 把 `Self::Reverse` 并入 overlay 一族；`hotkey_policy_for` 的 `needs_global` 真值臂加 `| BA::Reverse`。
  - `ReverseConfig` 文档写明「进入方式只由 `keys.key_actions` / 方案 `[key_actions]` / `z_key_action` 承载（动词 `reverse`），出厂不绑任何键；
    `enabled` 是 spec §3.1 要求的总开关，关着时绑了键也不进（门卫在协调器 `reverse_mode_available`）」。
  - `data/config.toml` 在 `[input.rare_char]` 段末尾之后加：
    ```toml
    # 反查模式：按绑定的键进入，用本方案编码查字（通配键在任何位置都代替一个码元，首位也行），
    # 候选显示完整编码，选中上屏该字；不查拼音。出厂关，且不绑任何键——在 [keys.key_actions] 里绑，
    # 如 backslash = "reverse"（符号键：编码为空时按下进入）或 "ctrl+shift+f" = "reverse"（组合键：随时进入）。
    [input.reverse]
    enabled = false
    # 反查模式期间的候选窗布局（follow / vertical / horizontal，见「模式级候选布局」）。
    candidate_layout = "follow"
    ```

- [ ] **Step 4: 跑**：`env $T cargo test -p wind-config --no-fail-fast` 全绿；`env $T cargo check -p wind-coordinator -p wind-service 2>&1 | grep -E "^error" | head`
  ——预期 wind-coordinator 在 `bound_action_mode_kind` / `is_lock_free_bound` 等穷举 `match` 上报「non-exhaustive」，**本任务只在那些
  `match` 上补最小臂让它编译**：`is_lock_free_bound` 并入 `false` 组；`bound_action_mode_kind` 暂并入 `return None` 组；`enter_bound_action`
  与 `handle_mode.rs` 的 commit-and-enter 暂返回 `None`（注释 `// 反查模式在 Task 17 接线`）；Task 17 替换。

- [ ] **Step 5: 格式与提交**

```bash
rustfmt --edition 2024 --check wind_input/crates/wind-config/src/{config,hotkey,config_schema}.rs wind_input/crates/wind-coordinator/src/{handle_lifecycle,handle_mode}.rs
git commit -F - --only -- wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/hotkey.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/handle_mode.rs <<'EOF'
feat(config): 反查模式动词 reverse 与 [input.reverse]（enabled / candidate_layout）

出厂关、不绑任何键（reverse-mode spec §3.1）；模式内通配键取方案 wildcard_key，
非法回落 z。协调器穷举 match 暂补不进入的臂，接线在后续任务。
EOF
```

---

## Task 16: 引擎 `convert_reverse` / `reverse_wildcard_key`

**Files:**
- Modify: `wind_input/crates/wind-engine/src/engine.rs`（trait `convert_wildcard` ~400 之后）
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（`CommitOptions::reverse_key`；`Engine` impl；测试）
- Modify: `wind_input/crates/wind-engine/src/mixed/engine.rs`（代理 primary；测试）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（`build_engine` 注入 `reverse_key`；`active_reverse_key` / `convert_reverse` 紧跟 `convert_wildcard` ~2627）

**Interfaces:**
- Produces:
  - trait 默认：`fn reverse_wildcard_key(&self) -> Option<char> { None }`、`fn convert_reverse(&self, _input: &str, _pattern: &str, _max: usize) -> Option<ConvertResult> { None }`
    （文档：「反查模式（reverse-mode spec §3）：与 `convert_wildcard` 同一查询内核，但**不看** `wildcard` 主开关；`pattern` 由协调器把每个通配键位替换成 `WILDCARD_SLOT`」）
  - `CommitOptions::reverse_key: Option<char>`；码表：`reverse_wildcard_key() = self.opts.reverse_key`、`convert_reverse = self.opts.reverse_key.map(|_| self.wildcard_query(..))`
  - 混输：两者代理 `self.primary`（不调拼音 / 英文）
  - `EngineManager::active_reverse_key(&self) -> Option<char>`、`EngineManager::convert_reverse(&self, input, pattern, max) -> Option<ConvertResult>`（均取 `active_engine()`）
  - `build_engine` 码表分支：`reverse_key: Some(eff.reverse_wildcard_char())`；英文分支保持 `CommitOptions::default()`（`None`）

- [ ] **Step 1: 写失败测试**

`codetable/engine.rs`：

```rust
    /// spec §3.2：反查不受 wildcard 主开关约束；仅单字照样生效（同一内核）。
    #[test]
    fn convert_reverse_ignores_wildcard_switch() {
        let opts = CommitOptions {
            reverse_key: Some('z'),
            wildcard_single_only: true,
            ..Default::default()
        };
        let e = engine_opts(&[("ab", "甲", 10), ("ac", "乙丙", 20), ("bb", "丁", 5)], opts);
        assert!(e.convert_wildcard("zb", &slot_pattern("?b"), 50).is_none(), "前置：主开关关");
        let r = e.convert_reverse("zb", &slot_pattern("?b"), 50).unwrap();
        let got: Vec<(&str, &str, &str)> = r
            .candidates
            .iter()
            .map(|c| (c.text.as_str(), c.code.as_str(), c.comment.as_str()))
            .collect();
        assert_eq!(got, [("甲", "ab", "ab"), ("丁", "bb", "bb")], "首位通配、全码注释");
        assert!(!r.should_commit && !r.should_clear);
    }

    #[test]
    fn convert_reverse_absent_without_reverse_key() {
        let e = engine_opts(&[("ab", "甲", 10)], CommitOptions::default());
        assert!(e.convert_reverse("ab", "ab", 50).is_none());
    }
```

`mixed/engine.rs`：

```rust
    /// spec §3.2：反查模式在混输下只查主码表，拼音不参与。
    #[test]
    fn mixed_convert_reverse_is_primary_only() {
        let e = mixed_wc(&[("qa", "甲", 10)], vec![("qz", "阿紫")]);
        assert_eq!(e.reverse_wildcard_key(), Some('z'));
        let r = e.convert_reverse("qz", &slot("q?"), 50).unwrap();
        assert!(r.candidates.iter().all(|c| c.source == CandidateSource::CodeTable), "{:?}", r.candidates);
        assert!(r.candidates.iter().any(|c| c.text == "甲"));
    }
```

（`ct_wildcard_with` 的 `CommitOptions` 加 `reverse_key: Some('z')`。）

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-engine --lib reverse` → 编译失败。
- [ ] **Step 3: 实现**：按 Interfaces。
- [ ] **Step 4: 跑**：`env $T cargo test -p wind-engine --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail`
- [ ] **Step 5: 提交**

```bash
git commit -F - --only -- wind_input/crates/wind-engine/src/engine.rs wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/mixed/engine.rs wind_input/crates/wind-engine/src/manager.rs <<'EOF'
feat(engine): 反查模式入口 convert_reverse——同通配内核、不看主开关

与 convert_wildcard 共用 wildcard_query（仅单字、影子层自动生效）；混输只代理主码表，
拼音不参与（reverse-mode spec §3.2）。通配键取方案 wildcard_key，非法回落 z。
EOF
```

---

## Task 17: 协调器 `ModeKind::Reverse` 骨架（进入 / 候选 / 按键 / 退出）

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/pipeline.rs`（`ModeKind::Reverse`，文档仿 `RareChar`）
- Create: `wind_input/crates/wind-coordinator/src/handle_reverse.rs`；Modify: `src/lib.rs`（`mod handle_reverse;`）
- Modify（`prefix-hijack-modes.md` §3 清单逐处，现名见裁决 14）：
  1. `debug_support.rs` `debug_active_mode` ~108：`Some(ModeKind::Reverse) => Some("reverse")`；新增 `pub fn debug_scope_relaxed(&self) -> bool`
  2. `handle_candidate.rs` `select_page_candidate` ~2470、`commit_highlighted` ~2539、`select_candidate_at` ~5049：并入 `Special | RareChar` 臂 → `commit_special_candidate`
  3. `handle_candidate.rs` `overlay_buf_edit` ~2850、`overlay_caret_parts` ~2886、`handle_candidate_nav` 的 `include_printable` ~3079、`command_input_code` ~4905、`candidate_op_scope` ~3236（同 RareChar：active 方案 + `special_buffer`）：并入
  4. `handle_mode.rs` `mode_indicator_names` ~1537：`ModeKind::Reverse => Some(("反查".to_string(), "反".to_string()))`；`overlay_engine_schema` ~230：`Some(ModeKind::Reverse) => Some(self.engine_mgr.active_schema_id())`
  5. `layout.rs` `intent_for` ~56：`Some(ModeKind::Reverse) => Some(cfg.input.reverse.candidate_layout)`；测试 `cfg_with` 加 `c.input.reverse.candidate_layout = intent;`、`MODES` 加 `ModeKind::Reverse`
  6. `comment.rs` `template_for` ~756：并入 `Special | RareChar` 臂（`overlay` 恒 None ⇒ 跟随全局，注释仍是引擎给的全码）
  7. `handle_url.rs` `active_hijack_buffer` ~173：`Some(ModeKind::Reverse) => Some(&state.special_buffer)`；`rewind_hijack` ~209：`Some(ModeKind::Reverse) => self.exit_special_mode(state)`（两处成对）
  8. `coordinator.rs` `cancel_session` ~5026、`hotkey_session_now` ~7112：并入；`coordinator/message_handler.rs` 模式分派 ~1180：并入 `handle_special_key`；新增 `pub(crate) fn reverse_entry_composition(&self, key_code: u32, display: String) -> KeyAction { self.hotkey_entry_composition(key_code, display, true) }`（裁决 11）
  9. `handle_common_chars.rs` `toggle_common_char` ~429：并入
  10. `handle_lifecycle.rs`：`enter_bound_action` 的 Reverse 臂 `if !self.reverse_mode_available() { return None; } Some(self.enter_reverse_mode(state, key_code))`；`bound_action_mode_kind` → `ModeKind::Reverse`
  11. `handle_mode.rs` commit-and-enter ~843：`BoundAction::Reverse => { if !self.reverse_mode_available() { return None; } Some(self.commit_and_enter_reverse_mode(state, key_code)) }`
  12. `handle_special.rs`：`update_special_candidates` 开头 `if matches!(state.active, Some(ModeKind::Reverse)) { self.update_reverse_candidates(state); return None; }`；
      `record_special_selection` 的 Reverse 分支用 `cand.code`（非空时）作记账码、归属 `active` 方案（裁决 12）；
      `handle_special_key` 的 `_ =>` 臂**最前**：Reverse 且 `punct_char(vk, shift) == self.engine_mgr.active_reverse_key()` ⇒ 光标处插入该字符、刷新候选、返回 `UpdateComposition`（符号通配键进缓冲，先于选词键与标点顶屏）；
      `exit_special_mode` 追加 `state.has_more = false; state.candidate_limit = 0; state.scope_relaxed = false;`（裁决 13，注释写明对 Special / RareChar 是空操作）
  13. `handle_lifecycle.rs` `reset_exclusive_modes`：已逐字段清 `special_buffer` 等；追加 `state.has_more = false; state.candidate_limit = 0;`（注释：Reverse 会写这两位）

**Interfaces:**
- Produces（`handle_reverse.rs`）:
  ```rust
  pub(crate) fn reverse_pattern(buffer: &str, key: char) -> String;          // 每个 key 位 → WILDCARD_SLOT
  impl Coordinator {
      pub(crate) fn reverse_mode_available(&self) -> bool;                  // input.reverse.enabled && active_reverse_key().is_some()
      pub(crate) fn is_reverse_trigger(&self, key_code: u32) -> bool;       // bound_action_for == Some(Reverse)
      pub(crate) fn enter_reverse_mode(&self, state: &mut State, key_code: u32) -> KeyAction;
      pub(crate) fn commit_and_enter_reverse_mode(&self, state: &mut State, key_code: u32) -> KeyAction; // 同 commit_and_enter_rare_char_mode
      pub(crate) fn update_reverse_candidates(&self, state: &mut State);    // reset_candidate_view + build(WILDCARD_INITIAL_LIMIT)
      pub(crate) fn build_reverse_candidates(&self, state: &mut State, limit: usize) -> usize; // 不动页码；返回引擎条数
  }
  ```
  `enter_reverse_mode` 字段组同 `enter_rare_char_mode`（`handle_special.rs:144`）：`active = Some(Reverse)`、`special_id = 0`、`overlay_spec = None`、
  清 `special_buffer` / `cursor`、`special_prefix` = 触发键字符（`vk_to_prefix_char_with_letters`）、`scope_relaxed = false`；然后
  `update_reverse_candidates` → `notify_ui_update` → `reverse_entry_composition(key_code, preedit)`。
  `build_reverse_candidates`：清候选；`preedit = special_prefix + special_buffer`；`candidate_limit = limit`；缓冲空或 `active_reverse_key()` 为 `None` ⇒
  `has_more = false`、返回 0；否则 `convert_reverse(buffer, reverse_pattern(buffer, key), limit)`，`engine_count = len`，`mark_common` →
  `apply_filter(state, ..)`（§11 的通配整组分组靠 `is_wildcard`，自动成立）→ 写回；`has_more = engine_count >= limit`。不重排、不吃候选调整与词频（裁决 12）。

- [ ] **Step 1: 写失败测试**（`handle_reverse.rs` 自带 `#[cfg(test)] mod tests`，自造夹具）

```rust
#[cfg(test)]
mod tests {
    use super::reverse_pattern;
    use crate::coordinator::Coordinator;
    use std::path::PathBuf;
    use std::sync::Arc;
    use wind_bridge::handler::{KeyEventData, MessageHandler};
    use wind_config::Config;
    use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

    const VK_BACKSLASH: u32 = 0xDC;
    const VK_ESCAPE: u32 = 0x1B;
    const VK_SLASH: u32 = 0xBF;

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

    /// 码表 `ab 甲 / ac 乙 / bb 丁`；`\` 绑反查、模式开。
    fn coord(tag: &str, tweak: impl FnOnce(&mut Config)) -> (Arc<Coordinator>, Cleanup) {
        let id = format!("zz_rev_{tag}_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("wind_rev_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("schemas");
        std::fs::create_dir_all(s.join(&id)).unwrap();
        std::fs::write(
            s.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"反\"\n[engine]\ntype = \"codetable\"\n\
                 [engine.codetable]\nmax_code_length = 4\n\
                 [[dictionaries]]\nid = \"{id}_m\"\npath = \"{id}/m.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            s.join(format!("{id}/m.dict.yaml")),
            "---\nname: m\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\nab\t甲\t30\nac\t乙\t20\nbb\t丁\t10\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.available = vec![id.clone()];
        cfg.schema.active = id.clone();
        cfg.input.default.chinese_mode = true;
        cfg.input.reverse.enabled = true;
        cfg.keys.key_actions.insert("backslash".into(), "reverse".into());
        tweak(&mut cfg);
        (Coordinator::new_headless(cfg, Some(&dir)), Cleanup { id, dir })
    }

    fn key(c: &Coordinator, vk: u32, shift: bool) {
        c.handle_key_event(&KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: if shift { MOD_SHIFT } else { 0 },
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        });
    }

    fn letters(c: &Coordinator, s: &str) {
        for ch in s.chars() {
            key(c, ch.to_ascii_uppercase() as u32, false);
        }
    }

    #[test]
    fn reverse_pattern_marks_every_key_position() {
        let s = wind_dict::WILDCARD_SLOT;
        assert_eq!(reverse_pattern("zbz", 'z'), format!("{s}b{s}"), "含首位");
        assert_eq!(reverse_pattern("ab", 'z'), "ab", "无通配键 ⇒ 字面等长 + 前缀");
    }

    /// ★ Review Focus 5：进 → 打码出候选 → Esc，模式态与翻页 / 放宽位全部复位，主路径随后照常。
    #[test]
    fn enter_and_exit_reverse_mode_clears_state() {
        let (c, _g) = coord("exit", |_| {});
        key(&c, VK_BACKSLASH, false);
        assert_eq!(c.debug_active_mode(), Some("reverse"));
        letters(&c, "zb");
        let tri = c.debug_candidate_triples();
        assert_eq!(
            tri.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>(),
            ["甲", "丁"],
            "首位通配：?b"
        );
        assert!(tri.iter().all(|(_, code, comment)| code == comment), "注释是全码");
        key(&c, VK_ESCAPE, false);
        assert_eq!(c.debug_active_mode(), None);
        assert!(c.debug_preedit().is_empty());
        assert_eq!(c.debug_candidate_count(), 0);
        assert!(!c.debug_has_more());
        assert!(!c.debug_scope_relaxed());
        letters(&c, "a");
        assert_eq!(c.debug_input_buffer(), "a", "主路径照常组码");
    }

    /// 符号通配键：模式内先于选词 / 标点顶屏进缓冲。
    #[test]
    fn reverse_mode_symbol_wildcard_key_enters_buffer() {
        let (c, _g) = coord("sym", |cfg| cfg.schema.codetable.wildcard_key = "?".into());
        key(&c, VK_BACKSLASH, false);
        key(&c, VK_SLASH, true); // ?
        letters(&c, "b");
        assert_eq!(c.debug_preedit(), "\\?b");
        assert_eq!(c.debug_all_candidate_texts(), ["甲", "丁"]);
    }

    /// 选中上屏候选文本并退出（不上屏编码）。
    #[test]
    fn reverse_selecting_commits_candidate_and_exits() {
        let (c, _g) = coord("sel", |_| {});
        key(&c, VK_BACKSLASH, false);
        letters(&c, "zb");
        key(&c, 0x20, false); // 空格
        assert_eq!(c.debug_active_mode(), None);
    }
}
```

（`debug_preedit` 的前缀：反斜杠触发时 `special_prefix == "\\"`；若 `vk_to_prefix_char_with_letters(0xDC)` 实际给的不是 `\`，按它的返回值改断言并在报告注明。）

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-coordinator --lib handle_reverse` → 编译失败（模块不存在）；实现骨架后跑 `layout` 测试确认 `MODES` 含 Reverse。
- [ ] **Step 3: 实现**：按 Files 1–13 与 Interfaces。
- [ ] **Step 4: 跑，确认绿 + 相邻回归**

```bash
env $T cargo test -p wind-coordinator --lib handle_reverse layout handle_special handle_url debug_support comment
env $T cargo test -p wind-coordinator --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail -30
```

Expected: 全绿；`handle_special` 生僻字那族（`real_dict_*`、`engine_side_admit_matches_caller_side_filter`）不变。

- [ ] **Step 5: 提交**

```bash
git add -- wind_input/crates/wind-coordinator/src/handle_reverse.rs
cd wind_input/crates/wind-coordinator/src && rustfmt --edition 2024 --check pipeline.rs handle_reverse.rs lib.rs debug_support.rs handle_candidate.rs handle_mode.rs layout.rs comment.rs handle_url.rs coordinator.rs coordinator/message_handler.rs handle_common_chars.rs handle_lifecycle.rs handle_special.rs && cd -
git commit -F - --only -- wind_input/crates/wind-coordinator/src/pipeline.rs wind_input/crates/wind-coordinator/src/handle_reverse.rs wind_input/crates/wind-coordinator/src/lib.rs wind_input/crates/wind-coordinator/src/debug_support.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/handle_mode.rs wind_input/crates/wind-coordinator/src/layout.rs wind_input/crates/wind-coordinator/src/comment.rs wind_input/crates/wind-coordinator/src/handle_url.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/coordinator/message_handler.rs wind_input/crates/wind-coordinator/src/handle_common_chars.rs wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/handle_special.rs <<'EOF'
feat(coordinator): 反查模式 ModeKind::Reverse——复用 special 族按键处理

同生僻字模式先例：缓冲 / 光标 / 退格 / 选词复用 handle_special_key，候选另由
handle_reverse 构建（convert_reverse + 检索范围过滤），通配键任何位置含首位都作
通配、符号通配键先于选词与标点顶屏进缓冲（reverse-mode spec §3.2）。按
prefix-hijack-modes §3 清单接线；exit_special_mode 复位 has_more / 上限 / 放宽位。
EOF
```

---

## Task 18: 反查模式的翻页扩充与末页放宽

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/handle_reverse.rs`（`expand_reverse_candidates`）
- Modify: `wind_input/crates/wind-coordinator/src/handle_candidate.rs`（`expand_candidates` ~2392 开头分流）
- Modify: `wind_input/crates/wind-coordinator/src/candidate_nav.rs`（`try_relax_scope_on_page_end` ~223、`expire_scope_override` ~200）
- Create: `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs`（本任务起的真实数据用例都放这里）

**Interfaces:**
- Produces: `pub(crate) fn Coordinator::expand_reverse_candidates(&self, state: &mut State)`——与 `expand_candidates` 同判据：`!has_more` 返回；
  `new_limit = (limit×2).min(CANDIDATE_LIMIT_CAP)`；`engine_count = build_reverse_candidates(state, new_limit)`；`clamp_candidate_view`；
  `engine_count <= prev_limit` ⇒ `has_more = false`、`candidate_limit = prev_limit`。
- `expand_candidates` 开头：`if matches!(state.active, Some(ModeKind::Reverse)) { return self.expand_reverse_candidates(state); }`（在 `input_buffer` 守卫之前）。
- `try_relax_scope_on_page_end`：`has_input` 对 Reverse 看 `special_buffer`；重建走 `build_reverse_candidates(state, state.candidate_limit)`，页码 / `paged` 同临拼那支还原。
- `expire_scope_override`：`ended` 对 Reverse 看 `special_buffer.is_empty()`。

- [ ] **Step 1: 写失败测试**（`tests/codetable_reverse.rs` 新文件）

```rust
//! 反查模式端到端（docs/design/codetable-reverse-mode.md §3）。真实数据：build_dev/data 的 wubi86 / wubi86_pinyin。
//! ⚠️ 数据缺失时整族静默跳过而计数照绿，判据是耗时（≥1s）与输出里有没有「跳过」。

use std::path::PathBuf;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

const VK_BACKSLASH: u32 = 0xDC;
const VK_ESCAPE: u32 = 0x1B;
const VK_NEXT: u32 = 0x22;
const VK_SPACE: u32 = 0x20;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}
fn dict_ready() -> bool {
    data_dir().join("schemas/wubi86/wubi86_jidian.dict.yaml").exists()
}
fn mixed_ready() -> bool {
    dict_ready() && data_dir().join("schemas/wubi86_pinyin.schema.toml").exists()
}
fn key(c: &Coordinator, vk: u32, shift: bool) {
    c.handle_key_event(&KeyEventData {
        key_code: vk,
        scan_code: 0,
        modifiers: if shift { MOD_SHIFT } else { 0 },
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    });
}
fn letters(c: &Coordinator, s: &str) {
    for ch in s.chars() {
        key(c, ch.to_ascii_uppercase() as u32, false);
    }
}

/// 五笔 86 + `\` 绑反查 + 模式开。通配主开关默认关（反查不看它）。
fn wubi_rev() -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.input.reverse.enabled = true;
    cfg.keys.key_actions.insert("backslash".into(), "reverse".into());
    cfg.input.scope_relax.page_end_key = false;
    cfg
}
fn with_filter(mut cfg: Config, mode: &str) -> Config {
    cfg.input.filter_mode = mode.into();
    cfg.input.rare_phrase = "keep".into();
    cfg
}
fn rev_triples(cfg: Config, code: &str) -> Vec<(String, String, String)> {
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, code);
    c.debug_candidate_triples()
}
fn page_until_more_than(c: &Coordinator, than: usize) -> usize {
    for _ in 0..400 {
        let n = c.debug_candidate_count();
        if n > than {
            return n;
        }
        let (cur, _, total) = c.debug_page_info();
        if cur + 1 >= total && !c.debug_has_more() {
            return n;
        }
        key(c, VK_NEXT, false);
    }
    c.debug_candidate_count()
}

/// spec §3.2 + codetable-wildcard §11：首批 100、`has_more`、翻到边界 ×2 扩充。缺 Task 18 时红在扩充。
#[test]
fn reverse_azzz_first_batch_100_and_expands() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(with_filter(wubi_rev(), "gb18030"), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "azzz");
    assert_eq!(c.debug_candidate_count(), 100);
    assert!(c.debug_has_more());
    let grown = page_until_more_than(&c, 100);
    assert!(grown > 100, "应扩充：{grown}");
    assert_eq!(c.debug_active_mode(), Some("reverse"), "翻页不退模式");
}

/// §11 契约 1：智能档把反查结果整份当一组，与常用字档相同。
#[test]
fn reverse_smart_matches_general() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let all = rev_triples(with_filter(wubi_rev(), "gb18030"), "hanz");
    let general = rev_triples(with_filter(wubi_rev(), "general"), "hanz");
    let smart = rev_triples(with_filter(wubi_rev(), "smart"), "hanz");
    assert!(all.len() > general.len(), "前置：han? 有生僻字可滤");
    assert_eq!(smart, general);
}
```

- [ ] **Step 2: 跑，确认红**：`env $T cargo test -p wind-coordinator --test codetable_reverse` → `reverse_azzz_first_batch_100_and_expands` 红在扩充；`reverse_smart_matches_general` 应已绿（Task 17 已接 `apply_filter`，是锁）。
- [ ] **Step 3: 实现**：按 Interfaces。
- [ ] **Step 4: 跑**：同 Step 2 + `env $T cargo test -p wind-coordinator --lib candidate_nav expand_stop_tests` + `--test codetable_wildcard --test input_flow`。
- [ ] **Step 5: 提交**

```bash
git add -- wind_input/crates/wind-coordinator/tests/codetable_reverse.rs
git commit -F - --only -- wind_input/crates/wind-coordinator/src/handle_reverse.rs wind_input/crates/wind-coordinator/src/handle_candidate.rs wind_input/crates/wind-coordinator/src/candidate_nav.rs wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
feat(coordinator): 反查模式接翻页扩充与末页放宽

special 族原本没有扩充与检索范围放宽，而反查结果与行内通配同样首批 100、×2 扩到
5000、智能档整份一组（reverse-mode spec §3.2 / codetable-wildcard §11）。三处按
active 分流到 special_buffer。
EOF
```

---

## Task 19: 进入通路与首键冲突判据

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/handle_lifecycle.rs`（`is_any_mode_trigger` ~89；`bound_action_yield_reason` ~540）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`code_char_conflicts` ~5540 的「空缓冲类占用」块）
- Modify: `wind_input/crates/wind-coordinator/src/handle_temp.rs`（`z_fallback_accepts` ~155；`try_z_fallback` ~247 装载臂与 ~274 刷新臂）
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs`（追加）

**Interfaces（逐处改动，spec §3.1 / §3.3）:**
- `is_any_mode_trigger`：`|| self.is_reverse_trigger(key_code)`（智能符号 press2 与行内通配首位让位 `wildcard_lead_yields` 都经它认得反查键）。
- `code_char_conflicts`：在 `if charset.contains_leading(ch) { … }` 块内加 `if self.is_reverse_trigger(vk) { owners.push("反查模式触发键"); }`（上方「新增引导键类动词必须同步」注释点名 reverse）。
- `bound_action_yield_reason`：`!action.is_enabled()` 之后、字母 / 符号分流之前加
  `if matches!(action, BoundAction::Reverse) && self.engine_mgr.active_reverse_key().is_none() { return Some("反查模式仅码表 / 五笔拼音混输方案生效"); }`
  ——符号键在拼音方案里也让位；字母键在码表里仍走既有「活码前缀」判据（`has_code_prefix`），在混输里仍走既有「字母键仅码表引擎」判据。
- `has_code_prefix`：**不改**。它只在 `state.active == None` 时被调用（其文档 ⚠️ 那条），反查模式内不经过它；让位判据经 `bound_action_yield_reason` 复用。
- `z_fallback_accepts`：`BA::Reverse => ch.is_ascii_alphabetic()`（残余码是本方案编码，数字仍是选词键）；
  `try_z_fallback` 装载臂：`BoundAction::Reverse => { if !self.reverse_mode_available() { return None; } state.active = Some(ModeKind::Reverse); state.special_id = 0; state.overlay_spec = None; state.special_buffer = residual.clone(); state.special_cursor = state.special_buffer.len(); state.special_prefix = "z".to_string(); state.scope_relaxed = false; "reverse" }`；
  刷新臂：`Some(ModeKind::Reverse) => self.update_reverse_candidates(state)`（不自动上屏）。`active_hijack_buffer` / `rewind_hijack` 已在 Task 17 成对接好。

- [ ] **Step 1: 写失败测试**（`codetable_reverse.rs` 追加）

```rust
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

#[test]
fn reverse_symbol_trigger_enters_only_when_enabled() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(wubi_rev(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    let mut off = wubi_rev();
    off.input.reverse.enabled = false;
    let c = Coordinator::new_headless(off, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), None, "总开关关：绑了键也不进");
}

/// ★ Review Focus 3：字母触发键是本方案首码（活码前缀）⇒ 让位作正常码；符号键不让位；
/// 冲突体检认得反查键。
#[test]
fn reverse_letter_trigger_yields_to_live_code_prefix() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.keys.key_actions.insert("a".into(), "reverse".into());
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    letters(&c, "a");
    assert_eq!(c.debug_active_mode(), None, "a 是五笔活码，让位");
    assert_eq!(c.debug_input_buffer(), "a");
    key(&c, VK_ESCAPE, false);
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), Some("reverse"), "符号键不让位");

    let mut conflict = wubi_rev();
    conflict.schema.codetable.input_chars = "a-y\\".into();
    let c = Coordinator::new_headless(conflict, Some(&data_dir()));
    let rep = c.code_char_conflicts();
    assert!(
        rep.iter().any(|(ch, owners)| *ch == '\\' && owners.contains(&"反查模式触发键")),
        "{rep:?}"
    );
}

/// spec §3.1：非码表 / 混输方案让位。
#[test]
fn reverse_yields_in_pinyin_schema() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.available = vec!["pinyin".into(), "wubi86".into()];
    cfg.schema.active = "pinyin".into();
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), None);
}

/// GH#146 同构：`z_key_action = "reverse"` 在出厂 `zz*` 短语下经 z 夺取进入。
#[test]
fn z_key_action_reverse_enters_via_z_fallback() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.codetable.z_key_action = "reverse".into();
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    c.debug_install_phrases(zz_phrases());
    letters(&c, "z");
    assert_eq!(c.debug_active_mode(), None, "首键 z 让位给 zz* 活码");
    letters(&c, "uia");
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    assert_eq!(c.debug_preedit(), "zuia");
    assert!(!c.debug_all_candidate_texts().is_empty(), "uia 的等长 + 前缀结果");
}
```

核对点：`input_chars = "a-y\\"`（TOML 里即 `a-y\`）若被 `CodeCharSet::new`（`wind-config/src/code_charset.rs:88`）判非法而回落默认 `a-z`，
`code_char_conflicts` 会因 `is_default_alpha()` 直接返回空——换成 `input_chars = "a-y;"` 并把反查键改绑 `semicolon`（同时 `cfg.schema.mix_modes.clear()`
让出厂 quick_mix 不占 `;`），断言字符改 `';'`，并在报告注明。

- [ ] **Step 2: 跑，确认红**：`reverse_letter_trigger_yields_to_live_code_prefix` 红在冲突体检；`reverse_yields_in_pinyin_schema` 视 Task 17 门卫可能已绿（锁）；`z_key_action_reverse_enters_via_z_fallback` 红。
- [ ] **Step 3: 实现**：按 Interfaces。
- [ ] **Step 4: 跑**：`env $T cargo test -p wind-coordinator --test codetable_reverse --test input_flow --test codetable_wildcard` + `--lib handle_temp wildcard handle_lifecycle`。
- [ ] **Step 5: 提交**

```bash
git commit -F - --only -- wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/handle_temp.rs wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
feat(coordinator): 反查触发键接入首键冲突判据与 z 夺取

is_any_mode_trigger / code_char_conflicts 补 reverse（新增引导键类动词的既有规约）；
非码表 / 混输方案让位，字母触发键沿用活码前缀让位（reverse-mode spec §3.1 / §3.3）；
z_key_action = reverse 经 try_z_fallback 进入（GH#146 同构）。
EOF
```

---

## Task 20: wind-setting 反查模式设置与动词

**Files:**
- Modify: `../wind-setting/src/assets/settings_manifest.toml`（`# ── 生僻字 ──` 两项之后新增 `# ── 反查模式 ──` 两项；`schema.codetable.z_key_action` 的 `options` 加一项）
- Modify: `../wind-setting/src/dialogs/schema_manager.rs`（动词表 ~1629 `rare_char` 之后）、`src/dialogs/key_binding.rs`（`categorize_lead_verbs` ~529 的 `"rare_char"` 臂；测试数组 ~1882）、`src/state.rs`（`key_action_label` ~2821；测试 ~3605）
- Regenerate: snapshot / mockdata

- [ ] **Step 1: 改动**

manifest：

```toml
# ── 反查模式 ──
# 进入方式不在这里：绑键走「全局自定义按键」（动词 reverse）或方案页「z 键引导功能」。
# core 的 [input.reverse].enabled 是总开关，关着时绑了键也不进（reverse-mode spec §3.1）。
[[items]]
key = "input.reverse.enabled"
group = "input"
section = "反查模式"
type = "toggle"
label = "启用反查模式"
hint = "绑定按键后按下进入，用本方案编码查字，通配键在任何位置都能代替一个码元，候选显示完整编码；不查拼音。进入键在「按键 → 全局自定义按键」里选「反查模式」"

[[items]]
key = "input.reverse.candidate_layout"
group = "input"
section = "反查模式"
type = "select"
label = "候选窗布局"
hint = "反查模式期间候选窗的排列方向；退出后自动恢复"
enabled_when = "input.reverse.enabled == true"
width = 210
options = [
  { value = "follow", label = "跟随全局" },
  { value = "vertical", label = "强制竖排" },
  { value = "horizontal", label = "强制横排" },
]
```

`z_key_action` 的 `options` 末尾加 `{ value = "reverse", label = "反查模式" },`（上方注释补一句「`reverse` 同 `rare_char`：出厂 `zz*` 短语下靠 core 的 z 夺取进入」）。
`schema_manager.rs` 在 `rare_char` 那个 `Verb` 之后加 `Verb { value: "reverse".into(), label: "反查模式".into() },`（注释：「core 的 `BoundAction::Reverse`，全局与方案级共用 parse」）；
`key_binding.rs` 分类臂改 `"temp_pinyin" | "temp_english" | "aux_code" | "rare_char" | "reverse" => &mut modes,`，测试数组加 `"reverse",`；
`state.rs` `key_action_label` 加 `"reverse" => "反查模式".to_string(),`，测试加 `assert_eq!(key_action_label("reverse"), "反查模式");`。

- [ ] **Step 2 / Step 3**：同 Task 3 Step 2 / Step 3（提交路径加 `src/dialogs/schema_manager.rs src/dialogs/key_binding.rs src/state.rs`），提交信息：

```
feat(settings): 反查模式开关与候选布局；按键编辑器与 z 键引导补 reverse 动词

总开关与布局两项（reverse-mode spec §5）；进入键走全局自定义按键或 z 键引导。
capability 快照与 mockdata 重生成。
```

---

## Task 21: P3 真实数据验收

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs`（追加）

- [ ] **Step 1: 追加用例**

```rust
/// spec §3.2：首位通配（`?uia`），通配主开关关着也生效；注释是全码。
#[test]
fn reverse_leading_wildcard_zuia_ignores_inline_switch() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.codetable.wildcard = false;
    let tri = rev_triples(cfg, "zuia");
    assert!(tri.iter().any(|(t, c, _)| t == "平江" && c == "guia"), "{tri:?}");
    assert!(tri.iter().all(|(_, c, m)| c.len() == 4 && &c[1..] == "uia" && c == m), "{tri:?}");
}

/// spec §3.2 + §4：模式内同样查未启用扩展库，排在已启用之后。
#[test]
fn reverse_mode_sees_disabled_dicts_when_on() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let off = rev_triples(wubi_rev(), "zuia");
    assert!(off.iter().all(|(t, _, _)| t != "门头沟区"));
    let mut cfg = wubi_rev();
    cfg.schema.codetable.lookup_disabled_dicts = true;
    let on = rev_triples(cfg, "zuia");
    let pos = on.iter().position(|(t, _, _)| t == "门头沟区").unwrap_or_else(|| panic!("{on:?}"));
    assert!(pos >= off.len(), "已启用的 {} 条全在前：{on:?}", off.len());
}

/// spec §2 作用于反查模式。
#[test]
fn reverse_mode_single_only() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = with_filter(wubi_rev(), "gb18030");
    cfg.schema.codetable.wildcard_single_only = true;
    let tri = rev_triples(cfg, "azzz");
    assert_eq!(tri.len(), 100);
    assert!(tri.iter().all(|(t, _, _)| wind_candidate::single_markable_char(t).is_some()));
}

/// spec §3.2：混输下模式内只查主码表，没有拼音候选。
#[test]
fn reverse_mode_has_no_pinyin_in_mixed() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.available = vec!["wubi86_pinyin".into(), "wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86_pinyin".into();
    let tri = rev_triples(cfg, "hanz");
    assert!(!tri.is_empty());
    assert!(tri.iter().all(|(t, c, _)| !c.contains('z') && t != "汉字"), "{tri:?}");
}

/// ★ Review Focus 5（真实数据）：翻过页、放宽过再 Esc，残留全清；随后主路径打字与新开的 coordinator 逐条相同。
#[test]
fn reverse_esc_exits_without_residue() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = with_filter(wubi_rev(), "smart");
    cfg.input.scope_relax.page_end_key = true;
    let c = Coordinator::new_headless(cfg.clone(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "hanz");
    for _ in 0..5 {
        key(&c, VK_NEXT, false); // 翻到底再按 ⇒ 放宽
    }
    key(&c, VK_ESCAPE, false);
    assert_eq!(c.debug_active_mode(), None);
    assert!(!c.debug_has_more() && !c.debug_scope_relaxed());
    letters(&c, "hanz");
    let fresh = Coordinator::new_headless(cfg, Some(&data_dir()));
    letters(&fresh, "hanz");
    assert_eq!(c.debug_candidate_triples(), fresh.debug_candidate_triples());
}

/// spec §3.2：选中上屏候选文本（不上屏编码），并退出。
#[test]
fn reverse_space_commits_highlight() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(wubi_rev(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "zuia");
    let first = c.debug_all_candidate_texts()[0].clone();
    let act = c.handle_key_event(&KeyEventData {
        key_code: VK_SPACE,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    });
    assert!(format!("{act:?}").contains(&first), "上屏的是候选：{act:?}");
    assert_eq!(c.debug_active_mode(), None);
}
```

- [ ] **Step 2: 跑**：`time env $T cargo test -p wind-coordinator --test codetable_reverse --no-fail-fast 2>&1 | tail -30` → 全绿、≥1s、无「跳过」。
  核对点：`reverse_esc_exits_without_residue` 里 `hanz` 若只有一页，5 次 PageDown 中第 2 次起即放宽——正是要测的残留来源；若放宽没发生（无被滤字），
  改用 `azz` 并在报告注明。`reverse_space_commits_highlight` 以 `Debug` 字符串断言是为兼容 `InsertText` / `CommitThenDeferComposition` 两形态。

- [ ] **Step 3: 提交**

```bash
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
test(reverse): 反查模式真实数据验收

首位通配 zuia 且不看行内主开关；模式内含未启用扩展库、仅单字；混输不出拼音；
Esc 后翻页 / 放宽残留清空、主路径与新开实例逐条相同；空格上屏候选（reverse-mode spec §3）。
EOF
```

---

## Task 22: P3 文档 + 全量测试 + 回归核对

**Files:**
- Create: `../WindInputDocs/content/docs/guides/reverse-mode.mdx`；Modify: `../WindInputDocs/content/docs/guides/meta.json`（`"rare-char-mode"` 之后加 `"reverse-mode"`）
- Modify: `../WindInputDocs/content/docs/settings/schema/codetable.mdx`（Task 5 那条补链接；`### 通配输入` 末段加一句指路；`### z 键引导功能` 选项列表若列举动词则补「反查模式」）
- Modify: `../WindInputDocs/content/docs/guides/config/input.mdx`（`## 临时拼音（input.temp_pinyin）` 节之后新增 `## 反查模式（input.reverse） <Since v="0.125" /> [#reverse]`）、
  `../WindInputDocs/content/docs/guides/config/keys.mdx`（`key_actions` 动词表若逐条列举则补 `reverse`）
- Modify: `docs/design/codetable-reverse-mode.md`（状态行）、`docs/design/codetable-wildcard.md`（顶部范围声明 + §8 第一条）、
  `docs/design/prefix-hijack-modes.md`（§3 清单名称更正 + 当前实例补反查）、`docs/architecture/engine-candidate-pipeline.md`（§3.4）

- [ ] **Step 1: 文档站**
  - `guides/reverse-mode.mdx`（仿 `rare-char-mode.mdx` 结构）：frontmatter `title: 反查模式`、`description: 按一个键进入，用本方案编码查字：通配键在任何位置都能代替一个码元（首位也行），候选显示完整编码。出厂关闭`；
    `import { Since } from "@/components/since";` 后单独一行 `<Since v="0.125" />`；小节：
    `## 开启 [#enable]`（设置 → 输入 → 反查模式 → 启用；再到「按键 → 全局自定义按键」给一个键选「反查模式」，或 `config.toml`：`[input.reverse] enabled = true` + `[keys] key_actions = { backslash = "reverse" }`；
    字母键到方案页「z 键引导功能」选「反查模式」；表格同生僻字页：符号键只在编码为空时进入、组合键随时进入并先上屏当前高亮）、
    `## 怎么用 [#usage]`（例：进入后打 `zuia`，第一码不确定也能查，列出 `guia` 平江、`iuia` 滴灌……；通配键取方案的「通配键」，没配合法键时用 `z`；
    不需要打开「通配输入」；只查本方案码表，不查拼音；空格 / 数字选中上屏该字词，回车上屏编码原文，Esc 放弃；结果先列 100 条、翻页自动加载；
    检索范围、「通配仅出单字」「通配与反查含扩展词库」都生效）、
    `## 与行内通配的区别 [#vs-inline]`（行内通配第一码会让位给别的功能，想在第一码通配就用反查模式）、
    `## 限制 [#limits]`（只在码表与五笔拼音混输方案里可用；混输里字母键不能当进入键；反查键若同时配成方案码元会有启动告警）。
  - `codetable.mdx`：Task 5 那条里的纯文字「反查模式」改为 `[反查模式](/docs/guides/reverse-mode)`；`### 通配输入` 末段（「26 个字母都参与编码的方案…」之后）加：
    `想在第一码就通配，用[反查模式](/docs/guides/reverse-mode) <Since v="0.125" />：进入后通配键在任何位置都生效。`
  - `guides/config/input.mdx` 新节：代码块同 Task 15 的 `[input.reverse]`，参考表两行
    `| \`enabled\` | 布尔 | — | \`false\` | 反查模式总开关；关着时绑了键也不进 |`、
    `| \`candidate_layout\` | 枚举 | \`follow\` / \`vertical\` / \`horizontal\` | \`"follow"\` | 反查模式期间的候选窗排列方向，见[模式级候选布局](#模式级候选布局) |`，
    正文一句「进入键在 [`key_actions`](/docs/guides/config/keys#key-actions) 里绑动词 `reverse`，出厂不绑。详见[反查模式](/docs/guides/reverse-mode)」。

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInputDocs && pnpm lint && pnpm build && pnpm lint:links && node scripts/check-config-coverage.mjs 2>&1 | tail -1
```

Expected: 全部通过；缺失 13（`input.reverse.*` 不在清单）。

- [ ] **Step 2: 设计与架构文档**
  - `codetable-reverse-mode.md` 状态行：`> **状态：P1–P3 已实施**（提交见 git log --grep 反查 / 通配）。实施计划见 `codetable-reverse-mode-plan.md`；实施中偏离见 §8。`，
    并在文末新增 `## 8. 实施后偏离`，逐条抄录本计划「计划裁决」里改变了 spec 字面的项（1、4、5、6、8、9、10、11、12、13）与执行中控制者新增的裁决。
  - `codetable-wildcard.md`：顶部「**不做**专门的通配/反查模式」改为「专门的反查模式（首位也可通配）见 `codetable-reverse-mode.md`」；§8 第一条末尾加「——已由 `codetable-reverse-mode.md` 实施」。
  - `prefix-hijack-modes.md`：顶部「当前实例」一句补「反查模式（`input.reverse`，special 的参数变体，入口是 `key_actions` 与 z 夺取）」；§3 表 #3 / #6 / #7 改为现名（裁决 14）。
  - `engine-candidate-pipeline.md` §3.4 追加「反查模式：协调器 `handle_reverse` 把每个通配键位换成 `WILDCARD_SLOT`，经 `Engine::convert_reverse`（同 `wildcard_query`、不看主开关、混输只查主码表），
    再过 `apply_filter`；翻页扩充 / 末页放宽按 active 分流到 `special_buffer`」。

- [ ] **Step 3: 全量测试与编译门**（命令同 Task 5 Step 3，日志 `/tmp/wct-rev/p3.log`；另跑 `cd .. && ./scripts/dev.sh st`）

Expected: `failed` 同基线；`passed` = P2 结束时 + **23**（Task 15：config 3 + hotkey 1；Task 16：码表 2 + 混输 1；Task 17：4；Task 18：2；Task 19：4；Task 21：6；
`layout::every_mode_maps_intent_over_baseline`、`chinese_only_matches_action_family` 等改的是既有用例不计）。不符时列出差异。

- [ ] **Step 4: 回归与提交范围自查、三仓提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev/WindInput
base=$(git merge-base HEAD main)
git diff "$base"..HEAD | grep -nE "^\+.*(TODO|todo!|unimplemented!|#\[ignore\]|\.only\(|chars\(\)\.count\(\) == 1)" || echo clean
git diff "$base"..HEAD --name-only | grep -x "wind_input/Cargo.lock" && echo "!! Cargo.lock 被提交" || echo "lock ok"
git diff "$base"..HEAD | grep -n "Task 17 接线" && echo "!! Task 15 的临时臂没替换" || echo "stub ok"
git commit -F - --only -- docs/design/codetable-reverse-mode.md docs/design/codetable-wildcard.md docs/design/prefix-hijack-modes.md docs/architecture/engine-candidate-pipeline.md <<'EOF'
docs(design): 反查模式落地——设计稿状态与实施偏离、通配 §8 指路、加模式清单更正
EOF
cd ../WindInputDocs && git add -- content/docs/guides/reverse-mode.mdx
git commit -F - --only -- content/docs/guides/reverse-mode.mdx content/docs/guides/meta.json content/docs/settings/schema/codetable.mdx content/docs/guides/config/input.mdx content/docs/guides/config/keys.mdx <<'EOF'
docs(guides): 反查模式专页；通配输入指路、配置参考补 input.reverse
EOF
git show --stat HEAD
```

Expected: `clean`、`lock ok`、`stub ok`。回归核对（已在全量内，逐项确认名字出现在 `p3.log` 的 ok 行）：`handle_special` 生僻字族、`input_flow::test_z_key_action_enters_rare_char`、
`codetable_wildcard` 全族、`layout::every_mode_maps_intent_over_baseline`、`coordinator` 的 `hotkey_session` 用例、`handle_candidate` 的 `command_input_code` 用例
（后两者的模式数组若只枚举了既有模式，按裁决不强制加 Reverse；加了更好）。`keys.mdx` 若没有逐条动词表则不改、从提交路径里去掉。

---

## 自查

- **spec 覆盖**：
  §2 A（配置、字素簇判据、截断前过滤、上限按过滤后计、混输只作用于通配侧、先落地）→ Task 1–5（裁决 1、2）。
  §3.1 B 入口（`BoundAction::Reverse` / `ModeKind::Reverse` / §3 清单接线 / 自绑键出厂无绑定 / `input.reverse.enabled` / 仅码表与混输 / `is_any_mode_trigger` 与 `code_char_conflicts`）→ Task 15、17、19。
  §3.2 模式内行为（本方案编码、任意位置含首位、不受主开关、键取 `wildcard_key` 否则 z、`convert_wildcard` 语义、A 与 §11 过滤翻页、全码注释、不含拼音、Esc / 提交 / 清空退出、
  `candidate_layout`、选中上屏候选）→ Task 16、17、18、21。
  §3.3 首键冲突 → Task 19。
  §4 C（配置、影子层、首次加载、复用 `load_at_with` / `reader_pool` / `resolve_dict_file` / `cache_path`、`(text, code)` 去重已启用排前、注释反查变体与缓存 key、`*_if_ready` 与后台建、
  三处失效、混输取主方案、文件缺失 warn 跳过、内存 mmap）→ Task 6–14（裁决 3–7）。
  §5 设置与文档（wind-config 五键 + 动词、wind-setting 三期各自设置项且 label 过拼音表、文档站 `codetable.mdx` 与反查说明、`pnpm lint`、`codetable-wildcard.md` 顶部与 §8、
  真实数据 A `azz` / B 首位通配 Esc 冲突让位 / C `xzqy` 可见性与普通候选不变）→ Task 3/12/20、5/14/22、4/13/21。
  §6 分期 → P1 / P2 / P3 顺序与 Global Constraints「分期」。§7 已做裁决 → 全部照做（拼音不参与、不受主开关、已启用排前、不改普通候选）。
- **签名一致性**：`wildcard_query(&self, &str, &str, usize) -> ConvertResult`（Task 2 定义，Task 8 扩展，Task 16 复用）；`CommitOptions::{wildcard_single_only: bool, reverse_key: Option<char>}`；
  `DisabledDictLayers::{new, search_pattern, mark_disabled, load_count}` 与 `DisabledDictSource { id, base_order, default_weight, load }`（Task 8 定义，Task 9 构造）；
  `CodeTableEngine::{with_disabled_dicts, disabled_dicts}`；`ReverseScope::{Enabled, WithDisabled}` 与 `…_in` 系列、`comment_reverse_scope`、`word_codes_display_for_comment`（Task 10 定义，Task 11 消费）；
  `spawn_index_warm_in` / `warm_comment_reverse_index`（Task 11）；`Engine::{convert_reverse, reverse_wildcard_key}`、`EngineManager::{active_reverse_key, convert_reverse}`（Task 16 定义，Task 17 消费）；
  `reverse_pattern` / `reverse_mode_available` / `is_reverse_trigger` / `enter_reverse_mode` / `commit_and_enter_reverse_mode` / `update_reverse_candidates` / `build_reverse_candidates`（Task 17）、
  `expand_reverse_candidates`（Task 18）；`reverse_entry_composition`、`debug_scope_relaxed`（Task 17）；`CodetableGlobal::reverse_wildcard_char`、`ReverseConfig`（Task 15）。定义均先于使用。
- **占位扫描**：全文无 TBD / 「类似 Task N」。需执行时确认的细节都写成了核对点：Task 4 Step 2（检索范围是否挤掉首批词组）、Task 13 Step 2（`zz` 两边皆空）、
  Task 17 Step 1（反斜杠的显示前缀字符）、Task 21 Step 2（`hanz` 是否触发放宽）、Task 22 Step 4（`keys.mdx` 是否有逐条动词表）、裁决 17（版本号）。
- **测试计数**：P1 +7、P2 +24、P3 +23（各期最后 Task 以此核对全量）。
