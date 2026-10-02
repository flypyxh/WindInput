# 反查模式改造：去总开关 / 「含未启用扩展词库」改为反查专属 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

承接 `codetable-reverse-mode.md`（P1–P3 已实施，合并于 `68c75205`）。本计划落地用户拍板的两项改造（见下「已批准的设计」），
**0.125 发布前**完成；两个旧键从未随任何发布版出现过（`git show v0.124.0:wind_input/crates/wind-config/src/config.rs` 不含
`lookup_disabled_dicts` 与 `struct ReverseConfig`，已核）。

## 计划裁决（写计划时读码落定的点；已批准的设计原文不改）

1. **反查模式的候选注释不走反查索引，变体可整块删除——已核实**。依据三处：
   - 引擎 `CodeTableEngine::wildcard_query`（`codetable/engine.rs` ~677-760）给每条结果写 `c.comment = c.code.clone()`；
     `convert_reverse` 与 `convert_wildcard` 共用它。
   - 协调器 `eval_var`（`comment.rs` ~1236 起）：`code_hint` 取 `c.comment`；`code` / `code_rev` / `code_all` / `code_rev_all`
     四个反查变量**只对 `c.source == CandidateSource::Pinyin` 的候选求值**（~1262、~1282），码表来源一律空串。
   - 反查模式候选由 `wildcard_query` 统一标 `CandidateSource::CodeTable`；`handle_reverse.rs` 不碰 `reverse_index` / `word_codes*` /
     `codetable_reverse_hint`（grep 为零）。
   ⇒ 变体（`ReverseScope::WithDisabled`）的唯一消费方是「拼音来源候选（临时拼音、混输的拼音侧、拼音方案下的码表反查）」与
   `eval_text_var`（`dict.rev(format=…)`）的注释反查。删除后这些场景的编码注释只显示**已启用**词库的码——这是「专属」的必然结果，
   文档如实写（Task 8）。
2. **旧键容忍的落法**：`Config` / `Schema` / `CodeTableSpec` 全仓无 `deny_unknown_fields`（`config.rs` ~11849 的说明），未知键被 serde 静默丢弃；
   段级降级 `section_fallback.rs` ~40「骨架里没有这条路径 = 未登记键，不降级」。故读路径天然容忍、不报错、不告警。
   **裁决**：两键进 `RETIRED_KEYS`（`config.rs` ~142），由 `prune_user_config` 从用户 `config.toml` 清掉；**不做值迁移**——两键未发布，
   与 `english_code_scope` 同一先例（那条注释原文：「该键从未随任何版本发布到用户手里…能读到它的只有开发期配置」）。
   方案覆盖文件（`schema_overrides` 目录里的 `<id>.toml`）里残留的 `[engine.codetable] lookup_disabled_dicts` **没有**清理机制，
   serde 忽略、留着无害；wind-setting 方案页「重置」也不再清它（`SPEC_BEHAVIOR_FIELDS` 移除该字段）。接受，不另写清理。
3. **热重载的真实落点是协调器的 `engine_reload_needed`，不只是 `reload_from_config`**：`reload_user_config`（`coordinator.rs` ~4142）
   只在 `engine_reload_needed(old, new)` 为真时才调 `EngineManager::reload_from_config`；该判据（~842）只比 `schema` 整段与
   `input.temp_pinyin` / `input.temp_english`。键挪到 `input.reverse` 后**若不补这一项，设置页切开关要重启才生效**——
   该函数文档里点名过的同类缺陷。**裁决**：判据补 `old.input.reverse.lookup_disabled_dicts != new.input.reverse.lookup_disabled_dicts`
   （只比这一个字段；`candidate_layout` 只给协调器读，不该触发引擎重建、丢词典缓存），并配正反两条单测。
4. **全局开关怎么到 `build_engine`**：`build_engine`（`manager.rs` ~5645）是关联函数，配置全靠参数；`EngineManager` 以「镜像字段 +
   `new` / `reload_from_config` 两处写」持有全局配置（`codetable` / `temp_pinyin` / `pinyin`…）。**裁决**：新增镜像
   `reverse_lookup_disabled: std::sync::atomic::AtomicBool`，`new`（~799 一带字段初始化）与 `reload_from_config`（写在 ~3829
   `engines.clear()` **之前**）写入；`ensure_loaded`（~2652）读出后作为 `build_engine` 新参数 `lookup_disabled_dicts: bool` 传入，
   混输分支递归构建 primary（~5691）与 secondary（~5732）时原样透传。不塞进 `CodetableGlobal`：那是 `schema.codetable` 的镜像，
   挪键的目的正是让它不再属于方案级。
5. **`wildcard_query` 的参数名用 `include_disabled`**（设计稿写「如 with_disabled」）：删除清单要 grep `with_disabled` 确认变体清零，
   参数再叫这个名字会让 grep 噪音化；构造器 `with_disabled_dicts`（影子层注入，保留）是该 grep 的唯一合法残留。
   `convert_reverse` 传 `true`、`convert_wildcard` 传 `false`。传 `true` 而非「开关值」：影子层**只在开关开时才挂到引擎上**（Task 4 的
   `build_engine` 门控），`disabled_dicts: Option<_>` 就是开关的物化，`true` = 「挂了就查」，与传开关值等价且引擎不必再存一份开关。
6. **反查通配键体检原本靠 `enabled` 门控**（`wildcard.rs` ~287 `reverse_wildcard_conflicts` 首行 `reverse_mode_available()`）。去掉
   `enabled` 后若只剩「活跃方案有反查通配键」，**每个码表方案**只要通配键配在翻页等键上就会被报反查冲突，哪怕用户从没绑反查键。
   **裁决**：新增 `Coordinator::reverse_bound_anywhere()`——方案 `[key_actions]`（`engine_mgr.active_key_actions()`）∪ 全局
   `keys.key_actions` 全部条目（含组合键）∪ `z_key_action()`，任一解析为 `BoundAction::Reverse` 即真；体检门控改为
   `reverse_mode_available() && reverse_bound_anywhere()`。`is_any_mode_trigger` / `code_char_conflicts` 走 `is_reverse_trigger`
   本就只看绑定，**不改**。
7. **门卫简化的范围**：`reverse_mode_available`（`handle_reverse.rs` ~33）保留为唯一门卫，函数体改为
   `self.engine_mgr.active_reverse_key().is_some()`（「仅码表 / 五笔拼音混输方案有反查能力」由此保证）；调用点
   `enter_bound_action`（`handle_lifecycle.rs` ~607）、`commit_and_enter_bound_action`（`handle_mode.rs` ~775）、z 夺取臂
   `try_z_fallback`（`handle_temp.rs` ~262）与体检只改注释。`bound_action_yield_reason`（`handle_lifecycle.rs` ~558）的 Reverse 判据
   只看 `active_reverse_key`、**从未读 `enabled`**，不改。
8. **Task 顺序以「每次提交可编译、可测」为准**：先让协调器不再消费变体（Task 2），再在引擎删变体（Task 3）；新键先加（Task 1）、旧字段最后删
   （Task 5，届时已无生产代码读它）。被某 Task 的语义变更**直接打红**的既有用例在该 Task 内改写（Task 4 改写行内通配 3 个真实数据用例、
   反查模式 1 个、夹具集成测试与引擎单测），Task 7 只做跨面新增验收。core 的 `wind-rpc --test wind_setting_assets` 在 Task 1–5 期间
   **预期红**（新键不在 manifest、旧键仍在 manifest），Task 6 补齐。
9. **孤儿缓存文件**：开发期开过方案级开关的机器上会留 `<cache>/<key>/<key>.with_disabled.wridx` 与旁边的 `.fp`；`purge_cache_files`
   按扩展名 `wridx` 清理，「重建缓存」即清。未发布，不写自动清理。
10. **`DisabledDictLayers` 一族不动**：`disabled_dicts.rs`、`warm_disabled_dicts_async` 三个触发点（`ensure_loaded` 活跃方案、`on_active_changed`、
    `set_dict_enabled_live` 禁用方向）、`loaded_disabled_dict_engine` / `prewarm_disabled_dicts` / `disabled_dicts_load_count`、`split_codetable_dicts`、
    `disabled_extra_sources` / `disabled_dict_layers`、`mark_disabled` 全保留；变的只有挂层条件的来源（裁决 4）与读层的入口（裁决 5）。
11. **「开关关时逐位不变」的口径**：`input.reverse.lookup_disabled_dicts = false`（出厂）时引擎不挂影子层 ⇒ 反查模式、行内通配、普通打字与
    改前「全关」逐条相同。对改前**开过方案级开关**的开发期配置，行内通配不再出未启用库——这是设计变更本身，不是回归。
12. **文档版本号**：`WindInputDocs/data/releases.json` 最新 `0.124.0`，仍用 `<Since v="0.125" />`。开工时若已出现 `0.125.x`，顺延下一小版本并在报告注明。

---

## 已批准的设计（binding，摘要；原文见派单）

**改动 1：去掉 `input.reverse.enabled`。** 反查模式可用 ⇔ 绑了进入键（`key_actions` 的 `reverse` 动词或 `z_key_action = "reverse"`）且活跃方案有反查能力，
与临时拼音 / 临时英文 / 生僻字同。设置页删「启用反查模式」；`candidate_layout` 保留、去掉依赖 enabled 的 `enabled_when`，说明写清进入键怎么绑。
旧键 `input.reverse.enabled` 出现在用户配置里须被容忍。

**改动 2：「含未启用扩展词库」改为反查模式专属。** 键从方案级 `schema.codetable.lookup_disabled_dicts` 挪到全局
`input.reverse.lookup_disabled_dicts`（bool，默认 false，放进 `ReverseConfig`），旧键须被容忍；设置项挪到「反查模式」分组。
只管反查模式本身（影子层、`from_disabled_dict`、排序、后台预热沿用，挂层条件改为全局开关）；**行内通配不再查未启用库**
（`wildcard_query` 加参数，`convert_wildcard` 传 false、`convert_reverse` 传开关）；ReverseIndex「含未启用库」变体整块删除。

---

**Goal:** 上述两项改造，不留死代码与 `allow(dead_code)`；开关关时一切与改前全关逐条相同。

**Architecture:**
- 配置：`ReverseConfig { candidate_layout, lookup_disabled_dicts }`（删 `enabled`）；`CodetableGlobal` / `CodeTableSpec` 删 `lookup_disabled_dicts`；
  `RETIRED_KEYS` 收两条旧键路径。
- 引擎：`wildcard_query(input, pattern, max, include_disabled)`；`EngineManager.reverse_lookup_disabled`（镜像）→ `build_engine(…, lookup_disabled_dicts)`
  决定挂不挂 `DisabledDictLayers`；反查索引回到单一范围（`ReverseScope` 枚举及全部 `*_in` 变体 API 删除，签名还原为 `81258aba` 时的形态）。
- 协调器：`reverse_mode_available` = 活跃方案有反查通配键；体检加绑定门控；注释反查回到常规索引；`engine_reload_needed` 收新键。

**Tech Stack:** Rust workspace（wind-config / wind-engine / wind-coordinator）+ wind-setting 仓（`settings_manifest.toml`、`scripts/dev.sh sg/st`，xwin+wine）
+ WindInputDocs 仓（fumadocs）。

**参考：** 改前（上一功能合并前）的形态 = `81258aba`（`68c75205^1`）。`git diff 81258aba 68c75205 -- <file>` 即上一功能对该文件的全部改动，
Task 2/3 还原签名时以它为准（只还原变体相关 hunk，`ModeKind::Reverse`、影子层等其余 hunk 保留）。

## Global Constraints

- **路径**：一切改动只在 `/home/dufeng/develop/windinput/wt-rev2/{WindInput,wind-setting,WindInputDocs}`；**绝不**碰
  `/home/dufeng/develop/windinput/{WindInput,wind-setting,WindInputDocs}`。
- **分支**：每次提交前 `git -C <仓> branch --show-current` 须为 `feat/reverse-rework`（三仓同名）。
- **提交纪律**：禁止 `git add -A` / `git add .` / `git commit -a` / `git stash`；新文件先 `git add -- <path>`；提交一律
  `git commit -F - --only -- <自己的路径…>`（中文提交信息）；提交前 `git diff --cached --name-status` 核对暂存区，提交后
  `git show --stat HEAD` 核对文件数 / 行数与自己的改动量相符。**任何提交都不带 `wind_input/Cargo.lock`**。
- **格式化**：禁止 `cargo fmt` / `cargo fmt --all`；只对自己改过的 `.rs` 跑 `rustfmt --edition 2024 --check <file>`，diff 只落在自己 hunk 内才去掉
  `--check` 执行，否则按 diff 手工采纳自己那几处。
- **测试环境**（zsh 下不要用 `env $T …` 的写法，按下面逐个 export）：
  ```bash
  cd /home/dufeng/develop/windinput/wt-rev2/WindInput/wind_input
  export CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev2 TMPDIR=/tmp/wct-rev2
  mkdir -p /tmp/wct-rev2
  ```
  cargo 一律在 `wind_input/` 下跑；全量加 `--no-fail-fast`。下文命令默认已在这个 shell 状态里。
- **数据在位判据**：真实数据用例依赖 `WindInput/build_dev/data`，缺失时静默跳过而计数照绿——以 `--test codetable_wildcard` /
  `--test codetable_reverse` 输出里**没有**「跳过」字样为准。
- **已知红**：wind-utils `line_ending_coverage::every_line_based_parser_normalizes`（主线已有，非本计划）。已知偶发红：
  `handle_direct_aux::tests::schema_source_not_ready_is_noop_then_warms`（并发时序；全量里红了单独重跑，单跑绿即记偶发）。
- **预期红**：Task 1–5 期间 `cargo test -p wind-rpc --test wind_setting_assets`（裁决 8），Task 6 起必须绿。
- **日志**：`info!` 不得含用户输入 / 候选。
- **wind-setting**：manifest 改完先在 `WindInput/wind_input` 下 `cargo test -p wind-rpc --test wind_setting_assets`（core 侧对账，秒出），再在
  `wind-setting/` 下 `./scripts/dev.sh sg`（重生成 `src/assets/capabilities.snapshot.json` / `src/mockdata/config.json`）与 `./scripts/dev.sh st`
  （xwin+wine 全量）。`sg` 后 `git diff` 里若只多了 `appVersion` 一行，手工改回、**不提交它**。新 label 每个字都须在
  `src/assets/pinyin_initials.txt` 里，否则 `search::pinyin::tests::table_covers_every_title_char` 红——本计划选定的新 label「反查含扩展词库」
  七个字已逐字核对在表内（`未` `只` `也` 不在表内，别用）。
- **WindInputDocs**：`pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm lint:links && node scripts/check-config-coverage.mjs`；
  `check-config-coverage` 缺失数须保持基线（Step 0 记下，预期 13），新键 `input.reverse.lookup_disabled_dicts` **不得**出现在缺失清单。
- **无占位**：完成前对改过的文件 grep `TODO|FIXME|todo!|unimplemented!|allow(dead_code)|test.skip|#\[ignore`，本计划新增处一条都不许有。

## Review Focus

1. **旧键残留时不报错、不改变行为**：用户 `config.toml` 里的 `input.reverse.enabled = true` / `schema.codetable.lookup_disabled_dicts = true`、
   方案覆盖文件里的 `[engine.codetable] lookup_disabled_dicts = true`，若让段级降级误触发，同段的 `candidate_layout` / `wildcard` 会一起被回落默认；
   若被某处读到，行内通配或反查会出现未启用库。规则：只靠 serde 忽略 + `RETIRED_KEYS` 清理，不加任何读旧键的代码。
   测试：Task 5 `stale_reverse_keys_load_without_section_fallback`、`retired_reverse_keys_are_pruned`；Task 4
   `stale_schema_level_override_does_not_attach_layers`；Task 7 `stale_schema_override_leaks_nowhere_real_data`。
2. **行内通配不再看未启用库**：`convert_wildcard` 只要还经过影子层，就会继续出「门头沟区」，且开关开着时首次行内通配还会触发影子层加载。
   规则：`include_disabled = false` 时 `wildcard_query` 连 `d.search_pattern` 都不调。测试：Task 4 引擎单测
   `convert_wildcard_never_reads_disabled_layers`（含 `load_count() == 0`）+ `lookup_disabled_dicts_inline_wildcard_excludes_xzqy`（真实数据）。
3. **全局开关热重载后影子层挂 / 摘正确**：键挪进 `input` 段后 `engine_reload_needed` 不收它 ⇒ 设置页切了没反应直到重启；镜像在
   `engines.clear()` 之后才写 ⇒ `reload_from_config` 末尾那次 `ensure_loaded` 拿旧值建引擎。规则：裁决 3、4。测试：Task 4
   `reverse_lookup_switch_requires_engine_reload` / `reverse_layout_change_does_not_require_engine_reload`（协调器判据）+
   `global_switch_reload_attaches_and_detaches_layers`（引擎，经 `reload_from_config` 开→关→开）。
4. **变体删光后无残留、常规注释反查与改前一致**：漏删一处 `*_in(…, Enabled)` 包装或 `reverse_scope_cache` 清空点，要么编译不过，要么留下
   只为变体存在的死代码；删多了会动到常规索引（加词查重、辅助码、悬停、联想都靠它）。规则：签名逐个还原到 `81258aba`；Task 3 末尾 grep 门
   （`ReverseScope|WithDisabled|WITH_DISABLED|\.with_disabled\.wridx|comment_reverse_scope|reverse_scope_cache|comment_variant_pending|variant_pending|word_codes_display_for_comment|warm_comment|WARM_COMMENT|all_dict_specs|reverse_index_base|reverse_index_key|_for_test\(.*Scope` 全仓零命中）。
   测试：Task 2 `prewarm_indexes_never_builds_comment_variant`；Task 3 `comment_reverse_ignores_disabled_extra_even_when_on`、
   `no_variant_index_file_is_written`，以及既有 `reverse_index_path_is_scoped_by_schema_id_only`（路径逐字节同改前）。
5. **绑键即可进入；未绑时反查动词零副作用**：门卫若仍残留对已删字段的判断（编译会挡住），或体检不加绑定门控（裁决 6），用户没绑反查键也会
   在日志里收到反查冲突告警；未绑键时 `\` 必须照常出「、」。测试：Task 5 `reverse_enters_with_binding_alone`、
   `unbound_trigger_falls_through_as_punct`、`reverse_symbol_trigger_enters_when_bound`、`reverse_wildcard_key_on_nav_key_is_reported`
   （改写：未绑不报），既有 `z_key_action_reverse_enters_via_z_fallback` 去 enabled 后仍绿；Task 7 `z_key_action_reverse_lists_zuia_without_any_switch`。

---

## 文件结构

| 文件 | Task | 动作 | 职责 |
|---|---|---|---|
| `wind_input/crates/wind-config/src/config.rs` | 1 5 | Modify | `ReverseConfig` 加 / 删字段；`CodetableGlobal` 删字段与折叠；`RETIRED_KEYS`；测试 |
| `wind_input/crates/wind-config/src/schema.rs` | 5 | Modify | `CodeTableSpec` 删 `lookup_disabled_dicts` |
| `wind_input/crates/wind-config/src/config_schema.rs` | 1 5 | Modify | `REGISTRY` 增删键；`schema_overridden_keys` 删一行 |
| `data/config.toml` | 1 5 | Modify | `[input.reverse]` 新键与说明；删两旧键 |
| `wind_input/crates/wind-coordinator/src/comment.rs` | 2 4 | Modify | 注释反查回到常规索引；删变体测试模块；夹具换新键 |
| `wind_input/crates/wind-coordinator/src/coordinator.rs` | 2 4 | Modify | 删变体预热 / 补建 / 计数钩子；`spawn_index_warm` 还原；`engine_reload_needed` 收新键 |
| `wind_input/crates/wind-engine/src/manager.rs` | 3 4 | Modify | 删 `ReverseScope` 一族；镜像字段；`build_engine` 新参数 |
| `wind_input/crates/wind-engine/src/lib.rs` | 3 | Modify | 摘掉 `ReverseScope` 导出 |
| `wind_input/crates/wind-engine/src/codetable/engine.rs` | 4 | Modify | `wildcard_query(…, include_disabled)`；单测改写 |
| `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs` | 3 4 | Modify | 删变体用例；按新语义重写 |
| `wind_input/crates/wind-coordinator/src/{handle_reverse,wildcard,handle_lifecycle,handle_mode,handle_temp}.rs` | 5 | Modify | 门卫简化、体检绑定门控、注释；单测 |
| `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs` | 4 | Modify | 行内通配 3 用例改「不含」 |
| `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs` | 4 5 7 | Modify | 键位置、去 enabled、新增验收 |
| `../wind-setting/src/assets/settings_manifest.toml` | 6 | Modify | 删 enabled 项；挪 / 改键与文案；z 键 hint |
| `../wind-setting/src/dialogs/schema_codetable.rs` | 6 | Modify | `SPEC_BEHAVIOR_FIELDS` 删一项 |
| `../wind-setting/src/assets/capabilities.snapshot.json`、`src/mockdata/config.json` | 6 | Regenerate | `dev.sh sg` 产物 |
| `../WindInputDocs/content/docs/guides/reverse-mode.mdx` | 8 | Modify | 去总开关；含扩展词库改为本模式专属 |
| `../WindInputDocs/content/docs/settings/schema/codetable.mdx` | 8 | Modify | 通配节删「含扩展词库」；z 键表行 |
| `../WindInputDocs/content/docs/guides/config/{schema,input,keys}.mdx` | 8 | Modify | 参考表增删键 |
| `docs/design/codetable-reverse-mode.md`、`docs/architecture/engine-candidate-pipeline.md` | 8 | Modify | §9「改造」；§1/§3.1/§4.2/§5/§8.3 指路；架构一段 |

---

## 开工前：基线

- [ ] **Step 0: 记全量基线**

```bash
cd /home/dufeng/develop/windinput/wt-rev2/WindInput && git status --short --branch && git log --oneline -1
for r in wind-setting WindInputDocs; do git -C ../$r status --short --branch; done
cd wind_input && export CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-rev2 TMPDIR=/tmp/wct-rev2 && mkdir -p /tmp/wct-rev2
cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-rev2/baseline.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
grep -E "^test .* FAILED|panicked at" /tmp/wct-rev2/baseline.log | head
cd ../../WindInputDocs && node scripts/check-config-coverage.mjs 2>&1 | tail -3
```

Expected：三仓分支 `feat/reverse-rework`、工作区除本计划文件外干净（`M wind_input/Cargo.lock` 若出现，不属本计划、永不提交）；
`passed≈6072 failed=1`，唯一红是 `line_ending_coverage::every_line_based_parser_normalizes`（四位数以下说明 `build_dev/data` 不在位，先停下查）；
`check-config-coverage` 缺失数记下（预期 13）。把三个数字写进第一个 Task 的报告。

---

## Task 1: 配置新键 `input.reverse.lookup_disabled_dicts`（加法）

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`（`ReverseConfig` ~4455-4467；`mod tests` 的 `reverse_config_defaults_off_and_parses` ~13511）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（`REGISTRY` ~489-490 `input.reverse.*` 两行之后）
- Modify: `data/config.toml`（`[input.reverse]` ~1003-1006）

**Interfaces:**
- Produces: `ReverseConfig::lookup_disabled_dicts: bool`（`#[serde(default)]`，出厂 false）；注册键 `input.reverse.lookup_disabled_dicts`（`Bool`）。
- 本 Task **不删**任何旧字段（裁决 8）。

- [ ] **Step 1: 写失败测试**（config.rs `mod tests`，紧跟 `reverse_config_defaults_off_and_parses`）

```rust
    /// reverse-rework：含未启用扩展词库是反查模式专属的全局开关，出厂关。
    #[test]
    fn reverse_lookup_disabled_dicts_defaults_off_and_parses() {
        assert!(!Config::default().input.reverse.lookup_disabled_dicts, "出厂关");
        let t: ReverseConfig = toml::from_str("lookup_disabled_dicts = true\n").unwrap();
        assert!(t.lookup_disabled_dicts);
        assert_eq!(t.candidate_layout, LayoutIntent::Follow, "缺省的布局不受影响");
    }
```

- [ ] **Step 2: 跑，确认红**：`cargo test -p wind-config --lib reverse_lookup_disabled_dicts` → 编译失败（字段不存在）。

- [ ] **Step 3: 实现**
  - `ReverseConfig` 末尾加：
    ```rust
    /// 反查模式里也查**未启用**的扩展词库（`DictSpec::is_enabled() == false`，如五笔「行政区域」），
    /// 结果排在已启用词库之后。只管反查模式本身：行内通配、普通打字候选与候选注释里的编码反查都不受影响。
    /// 出厂关。见 docs/design/codetable-reverse-mode.md §9。
    #[serde(default)]
    pub lookup_disabled_dicts: bool,
    ```
  - `REGISTRY`：`f("input.reverse.candidate_layout", …)` 之后加 `f("input.reverse.lookup_disabled_dicts", Bool),`。
  - `data/config.toml` 的 `[input.reverse]` 段 `candidate_layout` 之后加：
    ```toml
    # 反查含扩展词库：反查模式里也查「未启用」的扩展词库（如五笔的行政区域），排在已启用词库之后。
    # 只管反查模式：行内通配、普通打字候选、候选注释里的编码反查都不受影响。
    lookup_disabled_dicts = false
    ```

- [ ] **Step 4: 跑**：`cargo test -p wind-config --no-fail-fast` 全绿（含 `registry_covers_every_config_key` / `registry_types_match_default_values`）。
  `cargo test -p wind-rpc --test wind_setting_assets` 预期红（新键不在 manifest），记下失败信息、不处理。

- [ ] **Step 5: 格式与提交**

```bash
cd /home/dufeng/develop/windinput/wt-rev2/WindInput && git branch --show-current
rustfmt --edition 2024 --check wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/config_schema.rs
git commit -F - --only -- wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml <<'EOF'
feat(config): 反查模式专属开关 input.reverse.lookup_disabled_dicts

「含未启用扩展词库」从方案级挪成反查模式的全局开关，出厂关。旧的方案级键在后续提交里退役。
EOF
git show --stat HEAD
```

---

## Task 2: 协调器——注释反查回到常规索引（删变体消费方）

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/comment.rs`（辅助函数 ~1073-1096；`eval_text_var` 的 `code_rev|code` ~1121-1124 与 `code_rev_all|code_all` ~1134-1137；
  `eval_var` 的同名两臂 ~1262-1295；测试模块 `comment_reverse_scope_tests` ~3232 至文件尾）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`WARM_COMMENT_CALLS` ~70-75；`prewarm_indexes` 的变体块 ~4449-4461；
  `warm_comment_reverse_index` ~4522-4533；`spawn_index_warm` / `spawn_index_warm_in` ~4535-4640）

**Interfaces:**
- Consumes：`EngineManager::codetable_reverse_hint`、`word_codes_display`、`reverse_index_if_ready`、`reverse_index_skipped`、`is_building_reverse_index`、
  `prewarm_reverse_index`（均为无 scope 的既有签名）。
- Removes：`Coordinator::comment_reverse_hint`、`comment_reverse_codes_all`、`warm_comment_reverse_index`、`spawn_index_warm_in`、`WARM_COMMENT_CALLS`。
- 本 Task 结束时协调器**不再引用** `wind_engine::ReverseScope` 与 `comment_reverse_scope` / `*_variant_pending` / `word_codes_display_for_comment` /
  `*_in(…)` 任一 API（引擎侧那批 pub 函数留到 Task 3 删，此刻无人调用但仍是 pub，不产生 dead_code 警告）。

- [ ] **Step 1: 写失败测试**——把 `comment_reverse_scope_tests` 整个模块替换为下面这个模块（夹具 `coord()` 原样保留，仍写 `cfg.schema.codetable.lookup_disabled_dicts = on`，
  Task 4 统一换键）：

```rust
#[cfg(test)]
mod comment_reverse_regular_tests {
    //! 候选注释的编码反查只用常规索引（已启用词库）；「含未启用扩展词库」只给反查模式（reverse-rework）。
    // （Cleanup / coord 两个辅助原样搬过来，主库 `a 工`、未启用扩展 `_xz` 里 `uuia 门头沟区`）

    /// ★ Review Focus 4：开关开着，预热后注释仍查不到未启用库的码；启用库照常。
    #[test]
    fn prewarm_indexes_never_builds_comment_variant() {
        let (c, _g) = coord("on", true);
        c.prewarm_indexes();
        assert_eq!(c.engine_mgr.codetable_reverse_hint("工").as_deref(), Some("a"));
        assert_eq!(
            c.engine_mgr.codetable_reverse_hint("门头沟区").as_deref(),
            Some(""),
            "未启用库不进注释反查"
        );
    }
}
```

- [ ] **Step 2: 跑，确认红**：`cargo test -p wind-coordinator --lib comment_reverse_regular_tests` → 红在第二条断言（`left: Some("uuia")`：改前
  `prewarm_indexes` 会建变体）。

- [ ] **Step 3: 实现**（以 `git diff 81258aba 68c75205 -- wind_input/crates/wind-coordinator/src/comment.rs wind_input/crates/wind-coordinator/src/coordinator.rs`
  为对照，只还原变体相关 hunk）
  - comment.rs：删 `comment_reverse_hint` / `comment_reverse_codes_all`；四个调用点还原为改前写法——`code_rev|code` 臂
    `self.engine_mgr.codetable_reverse_hint(text).unwrap_or_default()`（`eval_var` 里是 `&c.text`），`code_rev_all|code_all` 臂
    `self.engine_mgr.word_codes_display(&sid, text).unwrap_or_default()`；删掉上一功能加的「注释范围那份索引 / 变体」字样的注释，恢复改前注释
    （「空串的理由同上面的 `code_rev`。」）。`template_for` 里的 `Some(ModeKind::Reverse)` 臂**保留**。
  - coordinator.rs：删 `WARM_COMMENT_CALLS` 整个 `thread_local!`；`prewarm_indexes` 删「候选注释反查的『含未启用扩展库』变体」那段；
    删 `warm_comment_reverse_index`；`spawn_index_warm` 恢复为改前的完整函数体（`git show 81258aba:wind_input/crates/wind-coordinator/src/coordinator.rs`
    ~4502 起：`reverse_index_if_ready` / `reverse_index_skipped` / `is_building_reverse_index` / 构建线程里 `prewarm_reverse_index`），删
    `spawn_index_warm_in` 及其文档。

- [ ] **Step 4: 跑**
  - `cargo test -p wind-coordinator --lib comment` 全绿。
  - `cargo test -p wind-coordinator --no-fail-fast` 全绿（`--test codetable_reverse` / `codetable_wildcard` 照常）。
  - `grep -rn "ReverseScope\|comment_reverse_scope\|variant_pending\|word_codes_display_for_comment\|warm_comment\|WARM_COMMENT\|spawn_index_warm_in\|_in(&" wind_input/crates/wind-coordinator/src`
    → 零命中（最后一个模式若命中与反查无关的 `_in(&`，逐条确认不是 `*_reverse_index_*_in` 即可）。
  - `cargo clippy -p wind-coordinator --all-targets` 零警告。

- [ ] **Step 5: 格式与提交**

```bash
git commit -F - --only -- wind_input/crates/wind-coordinator/src/comment.rs wind_input/crates/wind-coordinator/src/coordinator.rs <<'EOF'
refactor(coordinator): 候选注释反查回到常规索引，不再建含未启用库的变体

含未启用扩展词库改为反查模式专属；反查模式的注释是引擎带出的全码，不走反查索引，
变体的消费方只剩拼音来源候选的注释，按新语义一并去掉。
EOF
```

---

## Task 3: 引擎——删除 ReverseIndex「含未启用库」变体整块

**Files:**
- Modify: `wind_input/crates/wind-engine/src/manager.rs`
  - 字段 `reverse_scope_cache`（~578-581）与初始化（~836）；三处清空（`set_dict_enabled_live` ~3482、`invalidate_schema` ~3657、`reload_from_config` ~3795-3800，连同那段注释）
  - `codetable_reverse_hint`（~1163-1180）、`text_codes` / `text_codes_in`（~1270-1296）、`word_codes_display` / `word_codes_display_for_comment` /
    `word_codes_display_scoped`（~1314-1345）、`comment_variant_pending` / `codetable_reverse_hint_variant_pending`（~1347-1366）、`comment_reverse_scope`（~1368-1415）
  - `reverse_index_if_ready(_in)`（~1635-1650）、`reverse_index_for`（~1666-1703）、`prewarm_reverse_index(_in)`（~1761-1774）、
    `reverse_index_skipped(_in)` / `mark_reverse_index_skipped_for_test` / `reverse_scope_cached_for_test`（~1777-1806）、`is_building_reverse_index(_in)`（~1812-1825）
  - `index_build_lock_for`（~1909）、`reverse_index_cache_path(_in)`（~1946-1969）、`build_reverse_index_for`（~2126）、码位扫描处 `load_dicts_individually`（~2097）、
    单字全码表处（~2283-2287）、`all_dict_specs`（~6439）、`load_dicts_individually`（~6465）
  - `ReverseScope` / `WITH_DISABLED_KEY_SUFFIX` / `reverse_index_key` / `reverse_index_base`（~7212-7238）；`reverse_index_for` 里的 `retain`
  - `mod tests`：`reverse_index_path_is_scoped_by_schema_id_only` / `single_char_codes_cache_sits_next_to_the_reverse_index` 的实参（~7336-7397）、
    删 `reverse_index_variant_path_differs` / `reverse_index_keeps_variant_with_its_base`、~9632-9640 处调用
- Modify: `wind_input/crates/wind-engine/src/lib.rs`（~25-28 `pub use manager::{…}` 摘掉 `ReverseScope`）
- Modify: `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs`（删 `use wind_engine::ReverseScope;` 与 9 个变体用例；加 2 个守护用例）

**Interfaces（还原为 `81258aba` 的签名）：**
```rust
pub fn reverse_index_if_ready(&self, schema_id: &str) -> Option<Arc<ReverseIndex>>;
fn reverse_index_for(&self, schema_id: &str) -> Option<Arc<ReverseIndex>>;
pub fn prewarm_reverse_index(&self, schema_id: &str) -> bool;
pub fn reverse_index_skipped(&self, schema_id: &str) -> bool;
pub fn is_building_reverse_index(&self, schema_id: &str) -> bool;
fn index_build_lock_for(&self, schema_id: &str) -> Arc<Mutex<()>>;
fn reverse_index_cache_path(schema_id: &str) -> Option<PathBuf>;
fn reverse_index_cache_path_in(cache_root: &Path, schema_id: &str) -> Option<PathBuf>;   // <root>/<key>/<key>.wridx，与改前逐字节同
fn build_reverse_index_for(&self, schema_id: &str) -> Option<ReverseIndex>;
fn load_dicts_individually(schema: &Schema, schemas_dir: &Path) -> Vec<CachedDict>;       // 只取 enabled_dict_specs
pub fn text_codes(&self, schema_id: &str) -> TextCodeView;                                  // 系统层 reverse_index_if_ready，无回退分支
pub fn word_codes_display(&self, schema_id: &str, text: &str) -> Option<String>;
pub fn codetable_reverse_hint(&self, text: &str) -> Option<String>;                         // 用 text_codes(&primary)
```
`reverse_index_for` 的淘汰还原为 `guard.retain(|k, _| reverse_index_keeps(k, schema_id, &primary, &pins))`。
**保留**：`loaded_disabled_dict_engine`、`prewarm_disabled_dicts`、`warm_disabled_dicts_async`、`disabled_dicts_load_count`（影子层预热用）；
`loaded_engine_type`（改前就有）。

- [ ] **Step 1: 写守护测试**（`tests/lookup_disabled_dicts.rs`；此时 `manager()` 仍按旧键开开关）

```rust
/// ★ Review Focus 4：开关开着，注释 / 悬停 / 查重一律只认已启用词库；不存在能带出未启用库编码的路径。
#[test]
fn comment_reverse_ignores_disabled_extra_even_when_on() {
    let (m, id, _g) = setup("cmt", true, true, true);
    assert!(m.prewarm_reverse_index(&id));
    assert_eq!(m.codetable_reverse_hint("门头沟区").as_deref(), Some(""));
    assert_eq!(m.word_codes_display(&id, "门头沟区").as_deref(), Some(""));
    assert_eq!(m.word_codes_in(&id, "门头沟区").as_deref(), Some(""));
    assert_eq!(m.codetable_reverse_hint("立法").as_deref(), Some("uuif"));
}

/// 缓存目录里只有常规索引，没有 `*.with_disabled.wridx`。
#[test]
fn no_variant_index_file_is_written() {
    let (m, id, _g) = setup("nofile", true, true, true);
    assert!(m.prewarm_reverse_index(&id));
    let _ = m.codetable_reverse_hint("立法");
    if let Some(dir) = Config::cache_dir().map(|c| c.join(&id)) {
        let names: Vec<String> = std::fs::read_dir(&dir)
            .map(|it| it.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        assert!(names.iter().all(|n| !n.contains("with_disabled")), "{names:?}");
    }
}
```

- [ ] **Step 2: 跑**：`cargo test -p wind-engine --test lookup_disabled_dicts comment_reverse_ignores no_variant_index` → 预期**绿**（Task 2 后变体已无人建；
  这两条是删除后的守护，红绿证据在 Step 4 的 grep 门与编译）。

- [ ] **Step 3: 实现**：按 Files 清单逐处删除 / 还原；doc 注释里提到「变体」「`ReverseScope`」「`with_disabled`」的句子一并删掉，恢复改前文档
  （`reverse_index_cache_path` 文档删末段「『含未启用扩展库』变体…」；`load_dicts_individually` 文档删末段 `scope = WithDisabled`；`index_build_lock_for`
  文档还原）。`tests/lookup_disabled_dicts.rs` 删：`comment_reverse_variant_includes_disabled_extra`、`variant_and_regular_indexes_use_separate_files`、
  `scope_cache_invalidated_on_dict_toggle`、`scope_collapses_to_enabled`、`variant_invalidated_on_dict_toggle`、`scope_short_circuits_when_switch_off`、
  `scope_follows_schema_override_when_global_off`、`comment_reverse_falls_back_to_regular_while_variant_missing`、`comment_reverse_falls_back_when_variant_skipped`，
  以及孤立的那段「spec §4.2：注释反查变体含未启用库…」文档注释。

- [ ] **Step 4: 跑与 grep 门**
  - `cargo test -p wind-engine --no-fail-fast` 全绿；`cargo test -p wind-coordinator --no-fail-fast` 全绿；`cargo clippy -p wind-engine -p wind-coordinator --all-targets` 零警告。
  - grep 门（**全仓零命中**，`wind_input/` 下执行）：
    ```bash
    grep -rnE "ReverseScope|WithDisabled|WITH_DISABLED|with_disabled\.wridx|comment_reverse_scope|reverse_scope_cache|reverse_scope_cached|variant_pending|word_codes_display_for_comment|word_codes_display_scoped|text_codes_in|warm_comment|WARM_COMMENT|all_dict_specs|reverse_index_base|reverse_index_key|reverse_index_if_ready_in|prewarm_reverse_index_in|reverse_index_skipped_in|is_building_reverse_index_in|mark_reverse_index_skipped_for_test" --include='*.rs' crates
    grep -rn "with_disabled" --include='*.rs' crates | grep -v "with_disabled_dicts\|candidates_with_disabled_in_between"
    ```
    第二条只允许 `with_disabled_dicts`（影子层构造器）与 wind-quick-input 那个无关测试名。

- [ ] **Step 5: 格式与提交**

```bash
git commit -F - --only -- wind_input/crates/wind-engine/src/manager.rs wind_input/crates/wind-engine/src/lib.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs <<'EOF'
refactor(engine): 删除反查索引「含未启用扩展库」变体

注释反查只用常规索引；ReverseScope 与按范围的索引 API、变体缓存键 / 文件名、范围缓存一并删除，
签名还原为改前形态（常规索引路径逐字节不变）。
EOF
```

---

## Task 4: 引擎——`wildcard_query` 参数化 + 影子层启用来源改全局 + 热重载

**Files:**
- Modify: `wind_input/crates/wind-engine/src/codetable/engine.rs`（字段文档 ~261-266；`wildcard_query` ~677；`convert_wildcard` ~1026；`convert_reverse` ~1046；
  `mod tests` ~2770-2930 四个影子层用例）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（新字段与 `new` 初始化 ~799；`ensure_loaded` ~2666-2722；`reload_from_config` ~3829 之前；
  `build_engine` 签名 ~5645 与两处递归调用 ~5691 / ~5732；挂层门控 ~6139-6146；`disabled_extra_sources` / `warm_disabled_dicts_async` 文档）
- Modify: `wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs`（`manager()` 换键；按新语义重写）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`engine_reload_needed` ~822-846 与 `engine_reload_needed_tests` ~9926）
- Modify: `wind_input/crates/wind-coordinator/src/comment.rs`（Task 2 夹具 `coord()` 换键）
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs`（~1272-1345）、`tests/codetable_reverse.rs`（`reverse_mode_sees_disabled_dicts_when_on` ~436-455）

**Interfaces:**
```rust
// codetable/engine.rs
fn wildcard_query(&self, input: &str, pattern: &str, max_candidates: usize, include_disabled: bool) -> ConvertResult;
//   include_disabled == false ⇒ 不调 d.search_pattern、disabled_hits 恒空（与无影子层时逐条相同）
// convert_wildcard → wildcard_query(.., false)；convert_reverse → wildcard_query(.., true)（裁决 5）

// manager.rs
reverse_lookup_disabled: std::sync::atomic::AtomicBool,      // 镜像 config.input.reverse.lookup_disabled_dicts
fn build_engine(schema_id, data_dir, store, codetable_cfg, mix_cfg, override_dir, pinyin_cfg,
                phrase_seg_anywhere: bool, lookup_disabled_dicts: bool,   // ← 新参数，紧跟 phrase_seg_anywhere
                mixed_role, english_provider) -> Option<Box<dyn Engine>>;
//   挂层：if lookup_disabled_dicts && let Some(d) = Self::disabled_dict_layers(..) { engine = engine.with_disabled_dicts(d); }

// coordinator.rs
pub(crate) fn engine_reload_needed(old: &Config, new: &Config) -> bool;
//   … || old.input.reverse.lookup_disabled_dicts != new.input.reverse.lookup_disabled_dicts
```
`eff.lookup_disabled_dicts` 在 manager.rs 中**零引用**（`CodetableGlobal` 字段本身留到 Task 5 删）。

- [ ] **Step 1: 写失败测试**

`codetable/engine.rs` `mod tests`（`plain_convert_never_touches_disabled_layers` 之后）：

```rust
    /// ★ Review Focus 2：行内通配不读影子层，连加载都不触发；反查才读。
    #[test]
    fn convert_wildcard_never_reads_disabled_layers() {
        let e = with_xz(engine_opts(
            &[("uuif", "立法", 10)],
            CommitOptions { reverse_key: Some('z'), ..wildcard_opts(CommitOptions::default()) },
        ));
        let r = e.convert_wildcard("uuiz", &slot_pattern("uui?"), 50).unwrap();
        assert!(r.candidates.iter().all(|c| c.text != "门头沟区" && !c.from_disabled_dict));
        assert_eq!(e.disabled_dicts().unwrap().load_count(), 0, "行内通配不触发加载");
        let r = e.convert_reverse("uuiz", &slot_pattern("uui?"), 50).unwrap();
        assert!(r.candidates.iter().any(|c| c.text == "门头沟区" && c.from_disabled_dict));
        assert_eq!(e.disabled_dicts().unwrap().load_count(), 1);
    }
```

同模块四个既有用例改走 `convert_reverse`（opts 补 `reverse_key: Some('z')`，断言不变）：`plain_convert_never_touches_disabled_layers`（末段那次
`convert_wildcard` 换成 `convert_reverse`）、`wildcard_single_only_filters_disabled_hits_and_exhausts_on_both_sides`、
`wildcard_merges_disabled_after_enabled_dedup_by_text_code`、`set_dict_enabled_disable_moves_dict_into_disabled_layers`；名字里的 `wildcard_` 改为 `reverse_`。

`tests/lookup_disabled_dicts.rs`：`manager()` 改为 `cfg.input.reverse.lookup_disabled_dicts = on;`（删旧键那行）；新增 `rev_texts`（同 `wc_texts`，调
`m.convert_reverse`）；改写与新增：

```rust
/// 反查模式：开关开才查未启用扩展库，排在已启用之后。
#[test]
fn reverse_sees_disabled_extra_only_when_switch_on() {
    let (on, _id, _g) = setup("on", true, true, true);
    assert_eq!(rev_texts(&on, "uuiz", "uui?"), ["立法", "门头沟区"]);
    let (off, _id2, _g2) = setup("off", false, true, true);
    assert_eq!(rev_texts(&off, "uuiz", "uui?"), ["立法"]);
}

/// ★ Review Focus 2（接线层）：开关开着，行内通配照旧看不到未启用库。
#[test]
fn inline_wildcard_never_sees_disabled_extra() {
    let (m, _id, _g) = setup("inline", true, true, true);
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法"]);
}

/// ★ Review Focus 3：全局开关经 reload_from_config 切换后，影子层随引擎重建挂上 / 摘掉。
#[test]
fn global_switch_reload_attaches_and_detaches_layers() {
    let (m, id, g) = setup("reload", false, true, true);
    assert!(m.prewarm_schema(&id));
    assert_eq!(m.disabled_dicts_load_count(&id), None, "关：不挂");
    let cfg_with = |on: bool| {
        let mut cfg = Config::default();
        cfg.schema.available = vec![id.clone()];
        cfg.schema.active = id.clone();
        cfg.schema.codetable.wildcard = true;
        cfg.input.reverse.lookup_disabled_dicts = on;
        cfg
    };
    m.reload_from_config(&cfg_with(true));
    assert_eq!(wait_disabled_loaded(&m, &id), Some(1), "开：挂上并后台预热");
    assert_eq!(rev_texts(&m, "uuiz", "uui?"), ["立法", "门头沟区"]);
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法"]);
    m.reload_from_config(&cfg_with(false));
    assert_eq!(m.disabled_dicts_load_count(&id), None, "关：摘掉");
    assert_eq!(rev_texts(&m, "uuiz", "uui?"), ["立法"]);
    drop(g);
}

/// ★ Review Focus 1：方案覆盖文件里残留旧的方案级键，不再挂影子层。
#[test]
fn stale_schema_level_override_does_not_attach_layers() {
    let (m, id, _g) = setup("stale_ov", false, true, true);
    let ov: toml::Value = toml::from_str("[engine.codetable]\nlookup_disabled_dicts = true\n").unwrap();
    m.write_schema_override(&id, &ov).unwrap();
    assert!(m.prewarm_schema(&id));
    assert_eq!(m.disabled_dicts_load_count(&id), None);
    assert_eq!(rev_texts(&m, "uuiz", "uui?"), ["立法"]);
}
```
既有的 `live_disable_moves_extra_into_lookup`（末断言改 `rev_texts` 含「甘蓝菜」，另加 `wc_texts` 不含）、`missing_disabled_file_is_skipped`、
`only_disabled_extra_hits_carry_the_flag`（`convert_wildcard` → `convert_reverse`）、`disabled_layers_warm_in_background_when_switch_on` /
`switch_off_never_warms_disabled_layers`（`wc_texts` → `rev_texts`）照新语义改；`wildcard_sees_disabled_extra_only_when_switch_on` 由上面
`reverse_sees_…` 取代。文件头注释改为「未启用扩展词库只给反查模式（reverse-rework），自造夹具」。

`coordinator.rs` `engine_reload_needed_tests`：

```rust
    /// ★ Review Focus 3：反查专属的「含扩展词库」开关决定引擎挂不挂影子层，必须触发重建。
    #[test]
    fn reverse_lookup_switch_requires_engine_reload() {
        let old = Config::default();
        let mut new = old.clone();
        new.input.reverse.lookup_disabled_dicts = !old.input.reverse.lookup_disabled_dicts;
        assert!(engine_reload_needed(&old, &new));
    }

    /// 反向对照：反查模式的候选布局只给协调器读，不该丢词典缓存。
    #[test]
    fn reverse_layout_change_does_not_require_engine_reload() {
        let old = Config::default();
        let mut new = old.clone();
        new.input.reverse.candidate_layout = wind_config::LayoutIntent::Vertical;
        assert!(!engine_reload_needed(&old, &new));
    }
```
（`LayoutIntent` 的导入路径照 `coordinator.rs` 现有 `use` 取；若未导入就写全路径 `wind_config::config::LayoutIntent`，以编译为准。）

真实数据（`tests/codetable_wildcard.rs`）：辅助 `lookup_disabled()` 改写 `cfg.input.reverse.lookup_disabled_dicts = true`；三个用例改名改断言：
- `lookup_disabled_dicts_inline_wildcard_sees_xzqy` → `lookup_disabled_dicts_inline_wildcard_excludes_xzqy`：开关开、`uuiz` 行内通配的三元组与开关关**逐条相等**，且不含「门头沟区」。
- `lookup_disabled_dicts_leaves_plain_typing_identical`：键改了即可（断言不变）。
- `mixed_lookup_disabled_dicts_follows_primary` → `mixed_inline_wildcard_excludes_xzqy`：混输开关开，`uuiz` 不含「门头沟区」。
`tests/codetable_reverse.rs` `reverse_mode_sees_disabled_dicts_when_on`：`cfg.schema.codetable.lookup_disabled_dicts = true` → `cfg.input.reverse.lookup_disabled_dicts = true`。
`comment.rs` 测试夹具 `coord()`：同样换键。

- [ ] **Step 2: 跑，确认红**：
  - `cargo test -p wind-engine --lib convert_wildcard_never_reads` → 编译失败或红在第一条断言（改前行内通配会出门头沟区）。
  - `cargo test -p wind-coordinator --lib reverse_lookup_switch_requires` → 红（`engine_reload_needed` 不收新键）。
  - `cargo test -p wind-engine --test lookup_disabled_dicts` → `reverse_sees_…` / `global_switch_reload_…` 红（manager 尚不读新键）。

- [ ] **Step 3: 实现**
  - `wildcard_query` 加参数；`disabled_hits` 的 `self.disabled_dicts.as_ref().map(…)` 前加 `.filter(|_| include_disabled)`（或 `if include_disabled`），
    使关时连 `search_pattern` 都不调；`exhausted` 判据不变。字段文档改为「只在 `wildcard_query` 且 `include_disabled` 时读——即只有反查模式
    （`convert_reverse`）读它；行内通配与普通 `convert` 都不读」。`convert_wildcard` / `convert_reverse` 文档同步。
  - manager.rs：加字段 `reverse_lookup_disabled`（文档：「全局 `input.reverse.lookup_disabled_dicts` 的镜像；只在 `new` 与 `reload_from_config` 写，
    `ensure_loaded` 读出交给 `build_engine` 决定挂不挂影子层」）；`new` 初始化 `AtomicBool::new(config.input.reverse.lookup_disabled_dicts)`；
    `reload_from_config` 在 `self.engines…clear()` 之前 `store(config.input.reverse.lookup_disabled_dicts, Relaxed)`；`ensure_loaded` 读
    `load(Relaxed)` 传入；`build_engine` 加参数并在两处递归调用透传；挂层门控 `eff.lookup_disabled_dicts` → `lookup_disabled_dicts`，
    注释改为「开关取全局 `input.reverse.lookup_disabled_dicts`（反查模式专属，裁决 4）；混输的主码表子引擎同样由它决定」。
    `disabled_extra_sources` / `warm_disabled_dicts_async` / `disabled_dict_layers` 文档里「通配 / 反查」改「反查模式」。
  - coordinator.rs `engine_reload_needed`：加一项；函数文档「收什么」补一条
    「`input.reverse.lookup_disabled_dicts`：引擎据它挂不挂影子层，镜像在 `EngineManager`；同段 `candidate_layout` 不收」。

- [ ] **Step 4: 跑**
  - `cargo test -p wind-engine --no-fail-fast`、`cargo test -p wind-coordinator --no-fail-fast` 全绿；`--test codetable_wildcard` / `--test codetable_reverse` 输出无「跳过」。
  - `grep -rn "eff.lookup_disabled_dicts\|codetable.lookup_disabled_dicts" --include='*.rs' wind_input/crates/wind-engine wind_input/crates/wind-coordinator` → 零命中。
  - clippy 两个 crate `--all-targets` 零警告。

- [ ] **Step 5: 格式与提交**

```bash
git commit -F - --only -- wind_input/crates/wind-engine/src/codetable/engine.rs wind_input/crates/wind-engine/src/manager.rs wind_input/crates/wind-engine/tests/lookup_disabled_dicts.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-coordinator/src/comment.rs wind_input/crates/wind-coordinator/tests/codetable_wildcard.rs wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
feat(engine): 未启用扩展词库只给反查模式，开关改读 input.reverse.lookup_disabled_dicts

wildcard_query 加 include_disabled：行内通配传 false（不再查、也不触发加载），反查模式传 true。
影子层挂不挂由全局开关决定，经 EngineManager 镜像传给 build_engine；engine_reload_needed 收这个键，
设置页切换后热重建即生效。
EOF
```

---

## Task 5: 配置与协调器——去 `input.reverse.enabled`、删方案级旧键、旧键容忍、门卫简化

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`（`ReverseConfig` 删 `enabled` 并改类型文档；`CodetableGlobal` 字段 ~2144-2147、默认 ~2253、`resolved` 折叠 ~2325-2327；
  删测试 `codetable_lookup_disabled_dicts_folds_from_schema` ~9703-9730；改 `reverse_config_defaults_off_and_parses` ~13511；`RETIRED_KEYS` ~142；
  新增两条容忍测试，放在 `merged_user_value`（~11837）所在测试模块与 `prune_retired_removes_dead_keys_only`（~11216）旁）
- Modify: `wind_input/crates/wind-config/src/schema.rs`（`CodeTableSpec::lookup_disabled_dicts` ~495-498）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`（REGISTRY 删 `schema.codetable.lookup_disabled_dicts` ~243 与 `input.reverse.enabled` ~489；
  `schema_overridden_keys` 删 ~928-931）
- Modify: `data/config.toml`（删 `[schema.codetable]` 的 `lookup_disabled_dicts` 及其两行注释 ~93-95；`[input.reverse]` 删 `enabled = false`，段首注释改写 ~997-1006）
- Modify: `wind_input/crates/wind-coordinator/src/handle_reverse.rs`（~28-46；`mod tests` 的 `coord()` ~211 与 `reverse_disabled_does_not_enter` ~333-342）
- Modify: `wind_input/crates/wind-coordinator/src/wildcard.rs`（`reverse_wildcard_conflicts` ~277-290）
- Modify: `wind_input/crates/wind-coordinator/src/{handle_lifecycle.rs ~605,handle_mode.rs ~774,handle_temp.rs ~259}`（只改注释）
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs`（`wubi_rev` ~56；`reverse_symbol_trigger_enters_only_when_enabled` ~231；
  `reverse_wildcard_key_on_nav_key_is_reported` ~336-350）

**Interfaces:**
```rust
pub struct ReverseConfig { pub candidate_layout: LayoutIntent, pub lookup_disabled_dicts: bool }   // 无 enabled
const RETIRED_KEYS += [&["input", "reverse", "enabled"], &["schema", "codetable", "lookup_disabled_dicts"]];
pub(crate) fn reverse_mode_available(&self) -> bool { self.engine_mgr.active_reverse_key().is_some() }
pub(crate) fn reverse_bound_anywhere(&self) -> bool;   // 方案 key_actions ∪ 全局 keys.key_actions（全部条目）∪ z_key_action
// reverse_wildcard_conflicts 门控：!(reverse_mode_available() && reverse_bound_anywhere()) ⇒ 空
```

- [ ] **Step 1: 写失败测试**

config.rs（`merged_user_value` 所在模块）：

```rust
    /// ★ Review Focus 1：未发布即退役的两个键残留在用户配置里——不报错、不触发段级降级，同段的活键逐个完好。
    #[test]
    fn stale_reverse_keys_load_without_section_fallback() {
        let merged = merged_user_value(
            "[input.reverse]\nenabled = true\ncandidate_layout = \"vertical\"\nlookup_disabled_dicts = true\n\
             [schema.codetable]\nlookup_disabled_dicts = true\nwildcard = true\n",
        );
        let cfg = Config::deserialize_with_section_fallback(merged);
        assert_eq!(cfg.input.reverse.candidate_layout, LayoutIntent::Vertical);
        assert!(cfg.input.reverse.lookup_disabled_dicts);
        assert!(cfg.schema.codetable.wildcard, "同段活键不受旧键牵连");
    }
```

config.rs（`prune_retired_removes_dead_keys_only` 之后）：

```rust
    /// 两个旧键进退役清单：清掉它们，同段活键不动；幂等。
    #[test]
    fn retired_reverse_keys_are_pruned() {
        let mut root = tv(r#"
[input.reverse]
enabled = true
candidate_layout = "vertical"
lookup_disabled_dicts = true

[schema.codetable]
lookup_disabled_dicts = true
wildcard = true
"#);
        assert_eq!(prune_retired(&mut root), 2);
        assert!(get_nested(&root, &["input", "reverse", "enabled"]).is_none());
        assert!(get_nested(&root, &["schema", "codetable", "lookup_disabled_dicts"]).is_none());
        assert!(get_nested(&root, &["input", "reverse", "lookup_disabled_dicts"]).is_some(), "新键不得误删");
        assert!(get_nested(&root, &["input", "reverse", "candidate_layout"]).is_some());
        assert!(get_nested(&root, &["schema", "codetable", "wildcard"]).is_some());
        assert_eq!(prune_retired(&mut root), 0);
    }
```
（若 `prune_retired_removes_dead_keys_only` 里的 `assert_eq!(prune_retired(&mut root), 2, …)` 因清单变长受影响——它的输入不含新条目，计数不变；核对即可。）

`reverse_config_defaults_off_and_parses` 改为：删 `assert!(!c.input.reverse.enabled…)`；解析串改 `"enabled = true\ncandidate_layout = \"vertical\"\n"`
保留（证明旧字段被忽略），断言 `t.candidate_layout == Vertical`、`!t.lookup_disabled_dicts`。

handle_reverse.rs `mod tests`：`coord()` 删 `cfg.input.reverse.enabled = true;`；`reverse_disabled_does_not_enter` 替换为：

```rust
    /// ★ Review Focus 5：没有总开关——绑了键就能进。
    #[test]
    fn reverse_enters_with_binding_alone() {
        let (c, _g) = coord("bound", |_| {});
        key(&c, VK_BACKSLASH, false);
        assert_eq!(c.debug_active_mode(), Some("reverse"));
    }

    /// ★ Review Focus 5：没绑键时反查动词不存在——`\` 照常走中文标点，体检也不报。
    #[test]
    fn unbound_trigger_falls_through_as_punct() {
        let (c, _g) = coord("unbound", |cfg| {
            cfg.keys.key_actions.remove("backslash");
            cfg.schema.codetable.wildcard_key = "-".into();
        });
        let act = press(&c, VK_BACKSLASH);
        assert_eq!(c.debug_active_mode(), None);
        assert_eq!(inserted(&act), Some("、"));
        assert!(c.reverse_wildcard_conflicts().is_empty(), "没绑反查，不报反查通配键冲突");
    }
```

`tests/codetable_reverse.rs`：`wubi_rev()` 删 enabled 行；`reverse_symbol_trigger_enters_only_when_enabled` 改名
`reverse_symbol_trigger_enters_when_bound`，对照段改为「从 `cfg.keys.key_actions` 移除 `backslash` ⇒ 不进、`debug_active_mode() == None`」；
`reverse_wildcard_key_on_nav_key_is_reported` 的 `with_key(k, enabled)` 改为 `with_key(k, bound)`：`bound == false` 时
`cfg.keys.key_actions.remove("backslash")`，断言 `with_key("-", false).is_empty()`（消息「对照：没绑反查键不报」），文档注释同步。

- [ ] **Step 2: 跑，确认红**：`cargo test -p wind-config --lib retired_reverse_keys stale_reverse_keys` → `retired_reverse_keys_are_pruned` 红
  （清单尚无两键，`left: 0 right: 2`）；`stale_reverse_keys_load_without_section_fallback` 可能已绿（serde 本就忽略），它是守护。
  `cargo test -p wind-coordinator --lib unbound_trigger_falls_through` → 编译失败（`reverse_bound_anywhere` 尚无；或红在体检断言）。

- [ ] **Step 3: 实现**
  - config.rs：`ReverseConfig` 删 `enabled`，类型文档改为「进入方式只由 `keys.key_actions` / 方案 `[key_actions]` / `z_key_action`（动词 `reverse`）承载，
    出厂不绑任何键 ⇒ 默认不可用；**没有**总开关，与生僻字（`RareCharConfig`）同」；`CodetableGlobal` 删字段、默认值、`resolved` 折叠；删测试
    `codetable_lookup_disabled_dicts_folds_from_schema`；`RETIRED_KEYS` 末尾（`comment_max_chars` 那段说明之前）加：
    ```rust
    // 反查模式改造（docs/design/codetable-reverse-mode.md §9）：总开关去掉、「含未启用扩展词库」挪成
    // `input.reverse.lookup_disabled_dicts`。**不做值迁移**：两键只存在于 0.124.0 之后、0.125 之前的开发期，
    // 从未随发布版到用户手里（判据同上面 english_code_scope）。
    &["input", "reverse", "enabled"],
    &["schema", "codetable", "lookup_disabled_dicts"],
    ```
  - schema.rs / config_schema.rs：按 Files 删。
  - data/config.toml：删两处；`[input.reverse]` 段首注释改为（照生僻字段口径）：
    ```toml
    # 反查模式：按绑定的键进入，用本方案编码查字（通配键在任何位置都代替一个码元，首位也行），
    # 候选显示完整编码，选中上屏该字；不查拼音。只在码表与五笔拼音混输方案里可用。
    # 本段刻意没有 enabled —— 不绑任何键就等于关闭，绑了就是开。进入键在 [keys.key_actions] 里绑，
    # 如 backslash = "reverse"（符号键：编码为空时直接进入；正在打字时先上屏高亮候选再进入）
    # 或 "ctrl+shift+f" = "reverse"（组合键：随时进入，同样先上屏正在打的高亮候选）；
    # 字母 z 可在方案的 z_key_action 里选 "reverse"。
    ```
  - handle_reverse.rs：`reverse_mode_available` 体与文档按裁决 7 改；`is_reverse_trigger` 文档删「`input.reverse.enabled`」；新增
    ```rust
    /// 反查动词在任一层绑了键：方案 `[key_actions]`、全局 `keys.key_actions`（单键与组合键）、`z_key_action`。
    /// 只给启动体检门控用（没绑就谈不上「模式内冲突」）；按键路径的门卫仍是 [`Self::reverse_mode_available`]。
    pub(crate) fn reverse_bound_anywhere(&self) -> bool {
        use wind_config::BoundAction;
        let is_rev = |v: &str| matches!(BoundAction::parse(v), BoundAction::Reverse);
        self.engine_mgr.active_key_actions().values().any(|v| is_rev(v))
            || self.rt().config.keys.key_actions.values().any(|v| is_rev(v))
            || matches!(self.z_key_action(), BoundAction::Reverse)
    }
    ```
  - wildcard.rs：`reverse_wildcard_conflicts` 首个门控改 `if !(self.reverse_mode_available() && self.reverse_bound_anywhere())`，文档「反查模式不可用」改「反查模式不可用或没绑键」。
  - handle_lifecycle.rs / handle_mode.rs / handle_temp.rs：三处注释里的「总开关」「`input.reverse.enabled`」删掉，改为「门卫：活跃方案有反查通配键（码表 / 混输主码表）」。

- [ ] **Step 4: 跑与 grep 门**
  - `cargo test -p wind-config --no-fail-fast`、`cargo test -p wind-coordinator --no-fail-fast`、`cargo test -p wind-engine --no-fail-fast` 全绿。
  - `grep -rnE "reverse\.enabled|lookup_disabled_dicts" --include='*.rs' wind_input/crates data/config.toml` → 只剩：`ReverseConfig::lookup_disabled_dicts` 及其读写点、
    `RETIRED_KEYS` 两行、本 Task 的两条容忍测试与 `reverse_config_defaults_off_and_parses` 的解析串、Task 4 的 `stale_schema_level_override_does_not_attach_layers`。逐条列进报告。
  - clippy `-p wind-config -p wind-coordinator --all-targets` 零警告。

- [ ] **Step 5: 格式与提交**

```bash
git commit -F - --only -- wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/schema.rs wind_input/crates/wind-config/src/config_schema.rs data/config.toml wind_input/crates/wind-coordinator/src/handle_reverse.rs wind_input/crates/wind-coordinator/src/wildcard.rs wind_input/crates/wind-coordinator/src/handle_lifecycle.rs wind_input/crates/wind-coordinator/src/handle_mode.rs wind_input/crates/wind-coordinator/src/handle_temp.rs wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
feat(reverse): 去掉反查模式总开关，绑键即可用；方案级「含扩展词库」键退役

input.reverse.enabled 与 schema.codetable.lookup_disabled_dicts 进退役清单（未发布，不做值迁移），
残留在配置里时被忽略并在启动清理时删除。反查通配键体检改为「绑了反查键才报」。
EOF
```

---

## Task 6: wind-setting——去启用开关、挪「含扩展词库」到反查模式分组

**Files:**
- Modify: `../wind-setting/src/assets/settings_manifest.toml`（z 键 hint ~174；删 `schema.codetable.lookup_disabled_dicts` 项及其注释 ~227-236；反查模式分组 ~1856-1879）
- Modify: `../wind-setting/src/dialogs/schema_codetable.rs`（`SPEC_BEHAVIOR_FIELDS` ~87-115 删 `"lookup_disabled_dicts"`）
- Regenerate: `../wind-setting/src/assets/capabilities.snapshot.json`、`../wind-setting/src/mockdata/config.json`

- [ ] **Step 1: 对账红**：`cargo test -p wind-rpc --test wind_setting_assets`（`WindInput/wind_input` 下）→ 红（Task 1–5 的预期红：新键缺项、两旧键多出）。

- [ ] **Step 2: 改 manifest**
  - z 键引导功能 hint 删末句「选『反查模式』需先在 输入 → 反查模式 里启用」。
  - 删 `schema.codetable.lookup_disabled_dicts` 整项（含上方两行注释）。
  - 反查模式分组：删 `input.reverse.enabled` 项；分组注释改为
    `# 进入方式不在这里：绑键走「全局自定义按键」（动词 reverse）或方案页「z 键引导功能」。没有总开关——不绑键就是不启用。`；
    `candidate_layout` 删 `enabled_when`，hint 改为
    「反查模式期间候选窗的排列方向；退出后自动恢复。反查模式要先绑进入键才能用：在「按键 → 全局自定义按键」里给一个键选「反查模式」，或在方案页「z 键引导功能」选它」；
    其后新增：
    ```toml
    # 反查模式专属：模式内也查未启用的扩展词库（core 的 DisabledDictLayers，开关开时后台预热）。
    # 行内通配、普通候选、候选注释里的编码反查都不受影响（reverse-rework）。
    [[items]]
    key = "input.reverse.lookup_disabled_dicts"
    group = "input"
    section = "反查模式"
    type = "toggle"
    label = "反查含扩展词库"
    hint = "仅反查模式里查没启用的扩展词库（如五笔的行政区域），排在已启用词库之后；不影响行内通配与普通候选"
    ```
  - `SPEC_BEHAVIOR_FIELDS` 删 `"lookup_disabled_dicts"`；同文件里描述字段个数的注释（如「十二个扁平字段」~1217）按实际个数核对更正。

- [ ] **Step 3: 跑**
  - core：`cargo test -p wind-rpc --test wind_setting_assets` 绿。
  - `cd /home/dufeng/develop/windinput/wt-rev2/wind-setting && ./scripts/dev.sh sg && git diff --stat`：快照与 mockdata 只应出现
    `input.reverse.enabled` 消失、`input.reverse.lookup_disabled_dicts` 出现、`schema.codetable.lookup_disabled_dicts` 消失三类变化；多出的 `appVersion` 行手工改回。
  - `./scripts/dev.sh st` 全绿（记 passed / failed / ignored；上次基线 1276 / 0 / 5，数量随删一项可能变动，以零失败为准）。

- [ ] **Step 4: 提交**（wind-setting 仓）

```bash
cd /home/dufeng/develop/windinput/wt-rev2/wind-setting && git branch --show-current && git diff --cached --name-status
git commit -F - --only -- src/assets/settings_manifest.toml src/dialogs/schema_codetable.rs src/assets/capabilities.snapshot.json src/mockdata/config.json <<'EOF'
feat(setting): 反查模式去掉启用开关；「含扩展词库」挪进反查模式分组

绑了进入键即可用，候选窗布局说明写清怎么绑；input.reverse.lookup_disabled_dicts 只管反查模式，
方案页不再有通配与反查含扩展词库那一项。
EOF
git show --stat HEAD
```
（`mockdata/config.json` 若 `sg` 后无变化就不列进 `--only`。）

---

## Task 7: 真实数据验收（跨面新增）

**Files:**
- Modify: `wind_input/crates/wind-coordinator/tests/codetable_reverse.rs`

**样本**（`build_dev/data`，上一功能已实测）：「门头沟区 uuia」只在未启用的 `wubi86_xzqy`；反查 `zuia`（`?uia`）主库 6 条全是词组，
xzqy 另有门头沟区 / 巴彦淖尔市市辖区 / 二道江区；`平江 guia` 在主库。

- [ ] **Step 1: 写用例**（全部以 `if !dict_ready() { eprintln!("跳过…"); return; }` 开头；混输用 `mixed_ready()`）

```rust
/// ★ Review Focus 2 + 设计 §9：同一开关下，反查模式出门头沟区（排在已启用之后），行内通配不出。
#[test]
fn reverse_only_lookup_zuia_vs_inline_uuiz() {
    let mut cfg = wubi_rev();
    cfg.input.reverse.lookup_disabled_dicts = true;
    cfg.schema.codetable.wildcard = true;
    let rev = rev_triples(cfg.clone(), "zuia");
    let off = rev_triples(wubi_rev(), "zuia");
    let pos = rev.iter().position(|(t, c, _)| t == "门头沟区" && c == "uuia")
        .unwrap_or_else(|| panic!("{rev:?}"));
    assert!(pos >= off.len(), "已启用的 {} 条全在前：{rev:?}", off.len());
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    letters(&c, "uuiz");
    assert!(c.debug_candidate_triples().iter().all(|(t, _, _)| t != "门头沟区"), "行内通配不查未启用库");
}

/// 混输：反查只查主码表且含未启用库；行内通配（码长内）不含。
#[test]
fn mixed_reverse_sees_xzqy_inline_does_not() {
    if !mixed_ready() {
        eprintln!("跳过：五笔 / 混输方案数据不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.available = vec!["wubi86_pinyin".into(), "wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86_pinyin".into();
    cfg.schema.codetable.wildcard = true;
    cfg.input.reverse.lookup_disabled_dicts = true;
    let rev = rev_triples(cfg.clone(), "zuia");
    assert!(rev.iter().any(|(t, c, _)| t == "门头沟区" && c == "uuia"), "{rev:?}");
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    letters(&c, "uuiz");
    assert!(
        c.debug_all_candidate_texts().iter().all(|t| t != "门头沟区"),
        "混输行内通配不查未启用库"
    );
}

/// ★ Review Focus 1（真实数据）：方案覆盖文件里残留旧的方案级键，反查与行内通配都不查未启用库。
#[test]
fn stale_schema_override_leaks_nowhere_real_data() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let ov = std::env::temp_dir().join(format!("wind_rev_stale_ov_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ov);
    std::fs::create_dir_all(&ov).unwrap();
    // 覆盖文件按 `<override_dir>/<方案 id>.toml` 命名（`EngineManager::persist_schema_override`）。
    std::fs::write(
        ov.join("wubi86.toml"),
        "[engine.codetable]\nlookup_disabled_dicts = true\nwildcard = true\n",
    )
    .unwrap();
    let mk = || Coordinator::new_headless_with_override(wubi_rev(), Some(&data_dir()), Some(ov.clone()));
    let c = mk();
    key(&c, VK_BACKSLASH, false);
    letters(&c, "zuia");
    let rev = c.debug_candidate_triples();
    assert!(!rev.is_empty(), "前置：反查有结果");
    assert!(rev.iter().all(|(t, _, _)| t != "门头沟区"), "{rev:?}");
    let c = mk();
    letters(&c, "uuiz");
    let inline = c.debug_candidate_triples();
    assert!(inline.iter().any(|(_, _, m)| !m.is_empty()), "前置：覆盖文件里的 wildcard = true 生效，行内通配有结果");
    assert!(inline.iter().all(|(t, _, _)| t != "门头沟区"), "{inline:?}");
    let _ = std::fs::remove_dir_all(&ov);
}

/// ★ Review Focus 5：没有任何开关——`z_key_action = "reverse"` 经 z 夺取进入，首位通配 `zuia` 照常出结果。
#[test]
fn z_key_action_reverse_lists_zuia_without_any_switch() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.keys.key_actions.remove("backslash");
    cfg.schema.codetable.z_key_action = "reverse".into();
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    c.debug_install_phrases(zz_phrases());
    letters(&c, "zuia");
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    let tri = c.debug_candidate_triples();
    assert!(tri.iter().any(|(t, c, _)| t == "平江" && c == "guia"), "{tri:?}");
    assert!(tri.iter().all(|(_, c, _)| c.len() == 4 && &c[1..] == "uia"), "{tri:?}");
}
```
（`stale_schema_override_leaks_nowhere_real_data` 的第二个前置断言若因五笔通配结果注释形态不同而不成立，改为断言 `inline` 中存在
`c.len() == 4 && c.starts_with("uui")` 的条目——以「行内通配确实生效」为准，不许删掉前置。`new_headless_with_override` 的签名见
`construct.rs` ~283。）

- [ ] **Step 2: 跑**：`cargo test -p wind-coordinator --test codetable_reverse --test codetable_wildcard` 全绿、输出无「跳过」、耗时 ≥ 1s。
  反事实验证（实跑，不提交）：把 `convert_wildcard` 里的 `false` 临时改成 `true`，`reverse_only_lookup_zuia_vs_inline_uuiz` 与
  `lookup_disabled_dicts_inline_wildcard_excludes_xzqy` 必须红；改回后绿。报告里贴红的那行断言输出。

- [ ] **Step 3: 提交**

```bash
git commit -F - --only -- wind_input/crates/wind-coordinator/tests/codetable_reverse.rs <<'EOF'
test(reverse): 含扩展词库只在反查模式生效的真实数据验收

zuia 反查出门头沟区且排在已启用之后、uuiz 行内通配不出；混输同口径；方案覆盖里的旧键不生效；
z 键引导进反查不需要任何开关。
EOF
```

---

## Task 8: 文档 + 全量测试 + 回归核对

**Files:**
- Modify: `../WindInputDocs/content/docs/guides/reverse-mode.mdx`、`../WindInputDocs/content/docs/settings/schema/codetable.mdx`、
  `../WindInputDocs/content/docs/guides/config/{schema,input,keys}.mdx`
- Modify: `docs/design/codetable-reverse-mode.md`、`docs/architecture/engine-candidate-pipeline.md`

- [ ] **Step 1: 文档站**
  - `reverse-mode.mdx`：description 去「出厂关闭」改「出厂不绑键」；Callout 改为「没有总开关：绑了进入键就能用，出厂一个键都没绑」；「开启」节删第 1 步与
    TOML 里的 `[input.reverse] enabled = true`；「怎么用」末三条中「通配与反查含扩展词库」改为新增一条：
    「**反查含扩展词库**（设置 → 输入设置 → 反查模式；`input.reverse.lookup_disabled_dicts`）打开后，模式里也查没启用的扩展词库（如五笔的「行政区域」），
    排在已启用词库之后，如 `zuia` 会列出「门头沟区 uuia」。只管反查模式：行内通配、普通打字、候选注释里的编码都不受影响。开着时这些词库在后台提前读入；
    在设置里禁用的扩展词库，反查模式里仍查得到。」——「通配仅出单字」那条保留。
  - `codetable.mdx`：z 键表「反查模式」行删「需先开启 `input.reverse.enabled`」；通配节删「想查没启用的扩展词库…」整条及四个子项，改一句
    「行内通配只查已启用的词库；要查没启用的扩展词库请用[反查模式](/docs/guides/reverse-mode#usage)的「反查含扩展词库」」。
  - `config/schema.mdx`：示例删 `lookup_disabled_dicts` 行；参考表删该行；`z_key_action` 说明删「`reverse` 还要打开 `input.reverse.enabled` 才生效」。
  - `config/input.mdx` `#reverse`：示例改为 `candidate_layout` + `lookup_disabled_dicts = false`；表删 `enabled` 行、加
    `| \`lookup_disabled_dicts\` <Since v="0.125" /> | 布尔 | — | \`false\` | 反查模式里也查未启用的扩展词库，排在已启用之后；不影响行内通配、普通候选与候选注释反查 |`；
    段首加「没有总开关，绑了进入键即可用」。
  - `config/keys.mdx` ~117：删「需先开启 `input.reverse.enabled`」。
  - 跑：`cd ../WindInputDocs && pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm lint:links && node scripts/check-config-coverage.mjs`；
    缺失数等于 Step 0 基线，且不含 `input.reverse.lookup_disabled_dicts`。`grep -rn "input.reverse.enabled\|通配与反查含扩展词库" content` 零命中。

- [ ] **Step 2: 设计稿与架构文档**
  - `codetable-reverse-mode.md`：状态行补「0.125 发布前改造见 §9」；§1 表 C 行、§3.1「模式开关 `input.reverse.enabled`」、§4.2 首条与「注释反查」条、§5 配置清单
    各加一句「（已改，见 §9）」；新增 **§9 改造（0.125 发布前）**，写清：(1) 去总开关、可用性 = 绑键 + 方案有反查能力，体检改为绑了才报（裁决 6）；
    (2) `input.reverse.lookup_disabled_dicts` 只管反查模式，`wildcard_query(include_disabled)`、挂层看全局镜像、`engine_reload_needed` 收键；
    (3) 变体整块删除及理由（裁决 1），后果「临时拼音等拼音来源候选的编码注释只显示已启用库的码」；(4) 旧键处理（裁决 2）；(5) §8.3 遗留里
    「变体孤儿文件」「绑了 reverse 但 enabled=false 仍注册热键槽位」「注释反查方案来源不一致（变体那部分）」三条随之消解或改写，「开关开时热禁用扩展库后
    通配会重新读入」改为「反查模式会重新读入」。
  - `engine-candidate-pipeline.md` ~197-199：改为「`input.reverse.lookup_disabled_dicts`（反查模式专属）：`DisabledDictLayers` 独立于主 `DictManager`、
    开关开时后台预热，只在 `wildcard_query(include_disabled = true)`（即 `convert_reverse`）读、按 `(text, code)` 去重合并、`from_disabled_dict` 同档沉后；
    行内通配与注释反查不看它」。

- [ ] **Step 3: 全量与回归**
  ```bash
  cd /home/dufeng/develop/windinput/wt-rev2/WindInput/wind_input
  cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/wct-rev2/final.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
  grep -E "^test .* FAILED|panicked at" /tmp/wct-rev2/final.log | head
  cargo clippy -p wind-config -p wind-engine -p wind-coordinator --all-targets 2>&1 | grep -E "^(warning|error)" | head
  ```
  Expected：`failed=1`（仅基线那条）；passed 与 Step 0 的差额 = 本计划新增用例数 − 删除用例数（报告里逐 Task 列出增删，核对对得上）。
  回归核对清单（逐条在报告里打勾，附证据行）：Task 3 grep 门复跑零命中；Task 5 grep 门复跑与报告一致；`reverse_index_path_is_scoped_by_schema_id_only` 绿；
  `plain_convert_never_touches_disabled_layers` / `lookup_disabled_dicts_leaves_plain_typing_identical` 绿；对改过的文件 grep 占位模式（Global Constraints）零命中。

- [ ] **Step 4: 提交**（两仓分别提交）

```bash
cd /home/dufeng/develop/windinput/wt-rev2/WindInputDocs && git branch --show-current
git commit -F - --only -- content/docs/guides/reverse-mode.mdx content/docs/settings/schema/codetable.mdx content/docs/guides/config/schema.mdx content/docs/guides/config/input.mdx content/docs/guides/config/keys.mdx <<'EOF'
docs(reverse): 反查模式没有总开关；「含扩展词库」改为反查模式专属

input.reverse.lookup_disabled_dicts 只管反查模式，行内通配与候选注释只查已启用词库；
删去 input.reverse.enabled 与 schema.codetable.lookup_disabled_dicts。
EOF
cd ../WindInput
git commit -F - --only -- docs/design/codetable-reverse-mode.md docs/architecture/engine-candidate-pipeline.md <<'EOF'
docs(design): 反查模式改造——去总开关、含扩展词库改为反查专属、删注释反查变体

设计稿新增 §9 记改造与裁决，§8.3 遗留随之更新；架构文档同步影子层的读入口。
EOF
```

---

## 自查

- **设计覆盖**：改动 1 —— 去 enabled（T5 配置 / 门卫 / 测试，T6 设置页，T8 文档）、`reverse_mode_available` 保留反查能力判据（T5，裁决 7）、
  旧键容忍（T5 `RETIRED_KEYS` + 两条测试，T7 真实数据）、候选窗布局 `enabled_when` 与进入键说明（T6）、体检 / 让位判据不变（裁决 6、7）。
  改动 2 —— 新键（T1）、旧键删除与容忍（T5，含方案覆盖文件 T4 / T7）、设置页挪组与文案（T6）、`wildcard_query` 参数化与混输代理（T4，混输
  `convert_wildcard` / `convert_reverse` 分别代理主码表，自动随之生效，T4 / T7 真实数据钉住）、影子层来源改全局与热重载（T4，裁决 3、4）、
  变体整块删除（T2 协调器、T3 引擎，grep 门）、「开关关时逐位不变」（裁决 11，`plain_convert_never_touches_disabled_layers` 等）、文档（T8）。
- **名称一致**：`include_disabled`、`reverse_lookup_disabled`、`lookup_disabled_dicts`（`build_engine` 参数 / `ReverseConfig` 字段）、`reverse_bound_anywhere`、
  `RETIRED_KEYS` 两条路径、测试名在 Review Focus 与各 Task 中逐一对得上。
- **每次提交可编译**：T1 加法；T2 只删协调器侧调用；T3 删引擎侧（协调器已不引用）；T4 换读源（旧字段仍在、无人读）；T5 删旧字段（T3 / T4 后已无生产读点）。
