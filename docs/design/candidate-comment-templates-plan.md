# 候选注释模板化 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 候选注释改为「命名模板 + 默认/详细两态 + 四层总表分配」，Alt 单击切详细，内置中英释义。

**Architecture:**
- **配置面**：新段 `[ui.comment]` 放用户模板与用户分配。出厂模板、出厂分配、内置注释词库放
  新数据文件 `data/system.comments.toml`。
- **解析**：wind-config 新增纯逻辑模块 `comment_lib`，接收「作用范围 + 态 + 排布」，返回模板串。
  协调器只负责组出作用范围、维护两态，`comment.rs` 的求值不变。
- **Alt**：平台层只在组字中转发 Alt 的按下与松开，单击与按住的判定在协调器里做。

**Tech Stack:** Rust（wind-config / wind-coordinator / wind-cmdbar / wind-webdata）、C++17 TSF、Swift、
Fcitx5 C++、wind-setting（windui）、WindInputDocs（mdx）、Python 数据脚本。

**Spec:** `docs/design/candidate-comment-templates.md`（下称「设计稿」，§ 号指它）

## Global Constraints

- 在 worktree 分支 `feat/comment-templates` 上做（手工 `git worktree add`，`build_dev/data` 要么全拷要么不放）。
  阶段提交要连续，定版前不合 main。
- 提交只用 `git commit -F - --only -- <路径…>`；禁用 `git add -A` / `git add .` / `git commit -a` / `git stash`。
- 格式化只对自己改的文件跑 `rustfmt`，不跑 `cargo fmt --all`。
- 全量测试用 `cargo test --workspace --no-fail-fast`，隔离 `TMPDIR`（放 `/home` 下的短路径，不放 scratchpad）。
- 每个改动 wind-config 的任务，都要过 wind-config 的门禁：`registry_covers_every_config_key`、
  `data_config_toml_covers_registry`、`data_config_toml_has_no_orphan_keys`、
  `registry_types_match_default_values`、`data_config_toml_values_pass_validation`、`tests/write_back_gates.rs`。
- 值域原文：
  - 态：`default` / `detail`；
  - Alt：`off` / `session`（出厂）/ `persist` / `hold`；
  - 格值：模板 id / `none` / `inherit`；
  - 方案类型：`codetable` / `pinyin` / `shuangpin` / `mixed` / `english`；
  - 模式键：`temp_pinyin` / `temp_english` / `url` / `email` / `unicode` / `mix:<id>` / `special:<方案id>`；
  - 模板 id：`[a-z0-9_]+`。
- 方案文件不再参与注释（设计稿 §6.1）。
- 提交信息与代码注释要简短：一句结论，不写 GH 号、日期、实测过程。

## Review Focus

1. **Alt 组合键不能误触发切换**：组字中按 Alt+Tab、Alt+F4、Alt+数字（上屏注释）。Alt+数字在 Windows 上
   走 `WM_HOTKEY → DispatchHotkey`，绕过 `OnTestKeyDown`。预期：都不切换，组合键照常生效。
   由 Task 6、Task 7 钉住。
2. **没在组字时，Alt 原样交给应用**：空闲时单击 Alt，应用的菜单栏照常激活。由 Task 6、Task 7 钉住。
3. **临时翻转要随候选窗关闭而复位**：上屏后马上出联想候选，这时还没有新的按键。
   预期：联想窗显示持久态，不沿用刚才的翻转。由 Task 5 钉住。
4. **引用了已删除的模板 id**：该格按「跟随」处理，不 panic、不显示空注释，启动只 `warn` 一次。
   由 Task 2 钉住。
5. **老用户升级**：例如关了总开关、临时英文写了 `""`、全局写了自定义模板。
   预期：升级后行为不变，旧键从文件里消失。由 Task 4 钉住。

---

## 阶段一：配置与解析（core，纯 Rust）

### Task 1: `[ui.comment]` 配置类型与出厂数据文件

**Files:**
- Create: `wind_input/crates/wind-config/src/comment_lib.rs`（本任务只放类型与解析，解析逻辑在 Task 2）
- Modify: `wind-config/src/lib.rs`（导出）、`wind-config/src/config.rs`（`UiConfig` 加 `comment` 字段）、
  `wind-config/src/config_schema.rs`（登记）、`data/config.toml`
- Create: `data/system.comments.toml`
- Test: `wind-config/src/comment_lib.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Produces（全部 `pub`，serde 取小写 / snake_case）：
  ```rust
  pub enum CommentState { Default, Detail }          // 出厂 Default
  pub enum AltSwitch { Off, Session, Persist, Hold } // 出厂 Session
  pub struct CommentTemplateDef { pub id: String, pub name: String, pub text: String }
  pub struct CommentCells {                          // 四格，None = 不写
      pub default_vertical: Option<String>, pub default_horizontal: Option<String>,
      pub detail_vertical: Option<String>,  pub detail_horizontal: Option<String>,
  }
  pub struct CommentAssign {
      pub global: CommentCells,
      #[serde(rename = "type")] pub by_type: BTreeMap<String, CommentCells>,
      pub schema: BTreeMap<String, CommentCells>,
      pub mode: BTreeMap<String, CommentCells>,
  }
  pub struct UiCommentConfig {                       // config.toml [ui.comment]
      pub enabled: bool, pub state: CommentState, pub alt_switch: AltSwitch,
      pub templates: Vec<CommentTemplateDef>, pub assign: CommentAssign,
  }
  pub struct BuiltinCommentDict { pub id: String, pub label: String, pub path: String }
  pub struct SystemComments {                        // data/system.comments.toml
      pub templates: Vec<CommentTemplateDef>, pub assign: CommentAssign,
      pub dicts: Vec<BuiltinCommentDict>,
  }
  impl SystemComments { pub fn parse(text: &str) -> anyhow::Result<Self>; }
  ```
  - `Config.ui.comment: UiCommentConfig`。
  - `UiCandidateConfig` 里的 `comment_enabled`、`comment_template_*` **本任务先不删**（Task 4 删）。

**我方决定：出厂分配放 `system.comments.toml`，不放 `config.toml`。** `config.toml` 的表是逐键深合并的，
出厂写进去的行，用户在设置里删掉后，下次加载又会被合并回来（`[keys] softkeyboard` 注释里记过这个坑）。
所以解析时每个作用范围是两层：用户格优先，其次出厂格。用户想让出厂行失效，写 `inherit`。
`config.toml` 只出厂 `enabled` / `state` / `alt_switch` 三项。

- [ ] **Step 1：写失败测试**
  - `system_comments_parses_shipped_file`：`SystemComments::parse(include_str!("../../../../data/system.comments.toml"))` 成功。
    断言：
    - 模板 id 集合包含 `code_hint`、`pinyin`、`chaizi`、`chaizi_code`、`chaizi_pinyin`、`en_meaning`、
      `zh_english`、`pinyin_english`、`detail_full`；
    - `assign.global.default_vertical == Some("code_hint")`，`assign.global.detail_vertical == Some("detail_full")`；
    - `dicts` 的 id 是 `["en_zh", "zh_en"]`。
  - `template_ids_are_valid`：每个 id 匹配 `^[a-z0-9_]+$`，并且不重复。
  - `ui_comment_defaults`：`UiCommentConfig::default()` 的取值是 `enabled == true`、`state == Default`、
    `alt_switch == Session`，`templates` 与 `assign` 为空。
- [ ] **Step 2：运行测试，确认失败**
  `cargo test -p wind-config comment_lib`，预期编译失败：类型未定义。
- [ ] **Step 3：实现类型，写 `data/system.comments.toml`**
  - 模板串照设计稿 §7。`code_hint` 的模板串必须与现行 `default_comment_template()` 逐字一致。
  - 新增 `pinyin_english = "${pinyin}{ · ${dict:zh_en}}"`。
  - 出厂分配：

    | 作用范围 | 默认 | 详细 |
    |---|---|---|
    | 全局 | `code_hint` | `detail_full` |
    | 类型 `codetable` | — | `chaizi_pinyin` |
    | 类型 `pinyin`、`shuangpin` | — | `pinyin_english` |
    | 类型 `english` | `none` | `en_meaning` |
    | 模式 `temp_english` | `none` | `en_meaning` |
    | 模式 `temp_pinyin` | — | `chaizi_code` |

  - 内置词库：`[[dicts]]` 两条，`en_zh` 的路径是 `comments/en_zh.txt`，`zh_en` 的路径是 `comments/zh_en.txt`。
  - `config_schema.rs` 登记 `ui.comment.enabled`（Bool）、`ui.comment.state`（Enum）、`ui.comment.alt_switch`（Enum）。
    值域写成 const 切片，仿照 `ALT_COMMIT_VALUES`。
  - `templates` 与 `assign` 是用户结构化内容，加进 `ALLOWED_UNREGISTERED_KEYS` 的前缀放行，或登记为 Map/StructList，
    看门禁 `data_config_toml_has_no_orphan_keys` 接受哪种。
- [ ] **Step 4：跑测试与门禁，确认通过**
  `cargo test -p wind-config --no-fail-fast`，预期全绿。
- [ ] **Step 5：提交**
  `feat(config): 新增 [ui.comment] 与出厂 system.comments.toml`

### Task 2: 解析链 `CommentLibrary`

**Files:**
- Modify: `wind-config/src/comment_lib.rs`
- Test: 同文件 `#[cfg(test)]`

**Interfaces:**
- Consumes：Task 1 的类型。
- Produces：
  ```rust
  pub enum SchemaType { CodeTable, Pinyin, Shuangpin, Mixed, English }
  impl SchemaType { pub fn key(self) -> &'static str; }       // 小写值域原文
  pub struct CommentScope<'a> { pub mode: Option<&'a str>, pub schema: &'a str, pub schema_type: SchemaType }
  pub struct CommentLibrary { /* 合并后的模板表 + 两层分配 */ }
  impl CommentLibrary {
      pub fn build(system: &SystemComments, user: &UiCommentConfig) -> (Self, Vec<String> /*warnings*/);
      pub fn template_text(&self, scope: &CommentScope, state: CommentState, vertical: bool) -> &str; // "" = 不显示
      pub fn referenced_texts(&self) -> Vec<&str>; // 所有被任一格引用的模板串 + 兜底，供 data_needs
      pub fn templates(&self) -> impl Iterator<Item = (&CommentTemplateDef, TemplateOrigin)>;
  }
  pub enum TemplateOrigin { Builtin, User, UserOverridesBuiltin }
  ```

**解析算法**（实现时直接照写）：
- **模板表**：先放内置模板，再按 id 用用户模板覆盖。
- **合并每个作用范围的四格**：`merged.x = user.x.or(system.x)`。
- **按作用范围依次找**：模式（`scope.mode` 为 `Some` 时才找）→ `schema.<id>` → `type.<key>` → `global`。
  每一格先取当前态，再按排布取值。
  - 竖排：值是模板 id 或 `none` 就采用；值是 `None` 或 `inherit`，转下一个作用范围。
  - 横排：值是模板 id 或 `none` 就采用；值是 `inherit`，转下一个作用范围的横排；值是 `None` 时：
    - 同一作用范围的竖排是模板 id 或 `none`，就采用竖排的值；
    - 否则转下一个作用范围。
  - 模板 id 不在模板表里：当作 `None`（跟随）。`build` 为每个这样的 id 产出一条 warning，同一个 id 只报一次。
- **兜底**：四层都没有值时，默认态取 `code_hint`，详细态取 `detail_full`（取内置模板）。
  内置模板也找不到时返回 `""`。

- [ ] **Step 1：写失败测试**（构造最小的 system / user，独立给出期望值，不从实现里推）
  - `global_only`：只有 global 的竖排两格。
    - Default 竖排得 code_hint 的模板串；Detail 竖排得 detail_full 的模板串。
    - 横排同竖排。
  - `mode_beats_schema_beats_type_beats_global`：四层都给详细态的竖排。依次去掉一层，结果依次换成下一层的值。
  - `none_is_an_opinion`：模式层写 `none`，下层都有值，结果是 `""`。
  - `user_inherit_skips_system_cell`：system 的 `mode.temp_english.default_vertical = "none"`，
    user 同格写 `inherit`，结果是 global 的值。
  - `horizontal_same_as_vertical_when_unset`：schema 层只写竖排，横排得 schema 竖排的值。
  - `horizontal_inherit_goes_down`：schema 层横排写 `inherit`，global 横排有值，横排得 global 横排的值。
  - `horizontal_unset_and_vertical_unset_follows_down`：schema 层两格都不写，global 横竖不同，横排得 global 横排的值。
  - `unknown_id_follows_and_warns_once`：两格引用同一个不存在的 id `gone`。
    - 两格都得下一层的值；
    - `build` 返回的 warnings 长度是 1，且包含 `gone`。
  - `user_template_overrides_builtin`：用户定义 `code_hint`，text 为 `X`。
    - 结果得 `X`；
    - `templates()` 里它的 origin 是 `UserOverridesBuiltin`。
  - `fallback_when_global_empty`：system 与 user 的 global 都为空，Default 得内置 code_hint 的模板串。
  - `referenced_texts_covers_detail`：`referenced_texts()` 包含 detail_full 的模板串。
- [ ] **Step 2：运行测试，确认失败**
  `cargo test -p wind-config comment_lib`，预期 FAIL。
- [ ] **Step 3：按上面的算法实现**
- [ ] **Step 4：运行测试，确认通过**
- [ ] **Step 5：提交**
  `feat(config): 注释模板按模式→方案→类型→全局逐层解析`

### Task 3: 协调器改走 `CommentLibrary`

**Files:**
- Modify: `wind-coordinator/src/comment.rs`
  - `template_for`（:924）改为 `mode_key_of`；
  - 删除 `schema_template_of`（:971）和 `resolve_template`（:993）；
  - 改写 `comment_template_for`（:1115）；
  - `alt_commit_text`（:1181）。
- Modify: `wind-coordinator/src/coordinator.rs`
  - 新字段持有 `Arc<CommentLibrary>`；
  - 在加载、`reload_user_config` 时重建，`build` 产出的每条 warning 打一次 `warn`；
  - 读 `system.comments.toml` 走 `Config::resolve_data_file(data_dir, "system.comments.toml")`，读失败时用空的 `SystemComments`，并 `warn`。
- Modify: `wind-coordinator/src/data_needs.rs`
  - `derive` 改为扫 `lib.referenced_texts()` 与悬停段；
  - 删除 `SchemaFacts.schema_templates` / `overlay_templates`。
- Modify: `wind-engine/src/manager.rs:5686`：`schema_data_facts` 删掉模板收集。
- Modify: `wind-coordinator/src/candidate_pull.rs:215`：改用新入口（仍取横排）。
- Test：`wind-coordinator/src/comment.rs` 测试段；改写 coordinator.rs:11169-11458 一带已有的分层测试。

**Interfaces:**
- Consumes：Task 2 的 `CommentLibrary`、`CommentScope`、`SchemaType`。
- Produces：
  ```rust
  fn mode_key_of(cfg: &Config, active: Option<ModeKind>) -> Option<String>;
  // Mix(i) → "mix:<mix_modes[i].id>"；Special(id) → "special:<id>"；TempPinyin / TempEnglish / Url / Email / Unicode 各对应其键；
  // RareChar / Reverse / AuxCode / None → None
  fn schema_type_of(mgr: &EngineManager, schema_id: &str) -> SchemaType;
  // 由 schema_engine_type 与 pinyin_is_shuangpin_of 推出；未知类型按 CodeTable 处理并 warn
  pub(crate) fn comment_template_for<'a>(&'a self, cfg: &Config, state: &State, vertical: bool) -> &'a str;
  // 签名去掉 behavior；enabled 关时返回 ""；态取 effective_comment_state(state)（Task 5 实现，本任务先返回持久态）
  ```

- [ ] **Step 1：改写 / 新增测试**
  - `temp_english_uses_mode_cell`：出厂数据下进入临时英文。Default 得 `""`（none）；Detail 得 en_meaning 的模板串。
  - `wubi_detail_falls_to_codetable_type`：码表方案 Detail 得 chaizi_pinyin 的模板串。
  - `shuangpin_is_own_type`：双拼方案 Detail 得 pinyin_english 的模板串；user 给 `type.shuangpin` 单独设值后，只影响双拼。
  - `mix_mode_key_uses_instance_id`：`Mix(i)` 读的是 `mode."mix:quick_mix"`。
  - `disabled_master_switch_blanks_everything`：`ui.comment.enabled = false` 时两态都得 `""`。
  - `data_needs_loads_chaizi_for_detail_only_template`：只有 Detail 引用 `${chaizi}`，`derive` 的结果是 `chaizi == true`。
  - 把旧的三层测试（`schema_layer_wins_when_mode_absent` 等）改写成新语义，**不删测试**。
- [ ] **Step 2：运行测试，确认失败**
  `cargo test -p wind-coordinator comment --no-fail-fast`
- [ ] **Step 3：实现**
  - `notify_ui_update`（coordinator.rs:7470）改调新签名。
  - `forces_code_hint` / `comment_hint_source` 不动。
- [ ] **Step 4：跑全量测试**
  `cargo test --workspace --no-fail-fast`。预期：除 Task 4 才清的旧字段引用外全绿。如果有红，要对照基线逐条说明。
- [ ] **Step 5：提交**
  `refactor(comment): 协调器改由 CommentLibrary 解析注释模板`

### Task 4: 旧键迁移与退役

**迁移评估（设计稿 §10 要求先评估）：** 复杂度低，建议做自动迁移。
- 已有同形先例 `migrate_comment_max_chars_value`（config.rs:8578），在 `migrate_user_layer_value` 里按单层跑；
  `prune_user_config` 负责落盘。
- 要迁的只有 config.toml 里的 14 个键：全局 2 个、5 个模式 × 2、`mix_modes[]` × 2，外加 `comment_enabled`。
- 只有方案覆盖层 `schema_overrides/*.toml` 不迁移，只告警。我方决定：那是另一种文件，跨文件迁移不值得，
  当前也没有随包方案写过这些键。

**Files:**
- Modify: `wind-config/src/config.rs`
  - 新增 `fn migrate_comment_templates_value(layer: &mut toml::Value)`，挂进 `migrate_user_layer_value`；
  - 删除 `UiCandidateConfig.comment_enabled`、`comment_template_*`、`default_comment_template`、`comment_template()`；
  - 删除 6 个模式结构体的 `comment_template_*`；
  - `AltCommit::effective` 改读 `ui.comment.enabled`。
- Modify: `wind-config/src/schema.rs`：`CandidateSpec` / `OverlaySpec` 的两组字段改为只捕获、不生效。
  非空时往 `degraded_items` 推一条提示，提示指向「设置 → 候选注释」。`SchemaBehavior` 删除这两个字段。
- Modify: `config_schema.rs`
  - 删除 :654-656 与 :885-894、:1041-1046 的模板项；
  - 删除 `comment_template_l1_matches_l2` 与 `comment_enabled_l1_matches_l2`，改为对 `ui.comment.*` 的同形测试；
  - 更新 :207-213 的注释。
- Modify: `patch.rs:53`：从 `ALLOWED_UNREGISTERED_KEYS` 删除 8 个模板键。
- Modify: `data/config.toml`：删除 :978-988 的模式模板示例、:1406 和 :1446-1447，改为 `[ui.comment]` 段，并写上注释。
- Test: `wind-config/src/config.rs` 测试段、`config_schema.rs`，以及 config.rs:11649 的 `prune_keeps_mode_comment_templates` 改写。

**迁移规则：**
- `ui.candidate.comment_enabled` 迁到 `ui.comment.enabled`。
- 旧模板串的去重：按模板串去重，依次生成 `migrated_1`、`migrated_2` ……；`name` 写「迁移自 <来源键>」。
  多个来源共用同一个模板串时只生成一个模板，`name` 取第一个来源。
- 全局 `ui.candidate.comment_template_{vertical,horizontal}`：
  - 等于旧出厂值 `${code_hint|code_rev|shuangpin}` 的直接丢弃；
  - 否则写进 `ui.comment.assign.global.default_{vertical,horizontal}`。
- 模式级 `input.<m>.comment_template_*`（m 是 temp_english / temp_pinyin / url / email / unicode）
  迁到 `ui.comment.assign.mode.<m>.default_*`。`""` 迁成 `none`。
- `schema.mix_modes[i].comment_template_*` 迁到 `ui.comment.assign.mode."mix:<该项 id>"`。缺 id 的跳过并 warn。
- 横竖相同：只写 vertical。横竖不同：两格都写。
- 只迁 `default_*`，旧版没有详细态的概念。
- 层内已有 `ui.comment` 的同名格时，不覆盖它，只删旧键（幂等）。

- [ ] **Step 1：写失败测试**
  - `migrates_global_custom_template`：层里有 `ui.candidate.comment_template_vertical = "${pinyin}"`。
    断言：
    - 迁移后 `ui.comment.templates` 是 `[{id:"migrated_1", text:"${pinyin}"}]`；
    - `assign.global.default_vertical == "migrated_1"`；
    - 旧键已删除。
  - `drops_factory_valued_global`：旧值等于旧出厂值，迁移后不生成模板，旧键已删除。
  - `empty_mode_template_becomes_none`：`input.temp_english.comment_template_vertical = ""` 迁成
    `assign.mode.temp_english.default_vertical == "none"`。
  - `dedupes_identical_texts`：两个模式用同一个模板串，只生成一个模板。
  - `mix_mode_uses_id`：`mix_modes = [{id="quick_mix", comment_template_vertical="${pinyin}"}]` 迁到
    `assign.mode."mix:quick_mix"`。
  - `comment_enabled_moves`：`comment_enabled = false` 迁成 `ui.comment.enabled == false`。
  - `migration_is_idempotent`：同一层跑两次，结果与跑一次相同。
  - `schema_file_template_degrades`：方案 toml 写 `[candidate] comment_template_vertical = "x"`，
    `behavior().degraded_items` 非空，且不影响解析结果。
- [ ] **Step 2：运行测试，确认失败**
- [ ] **Step 3：实现上面的迁移与删字段，把编译报错逐个改完**
  - 设置端仍引用旧键，那边在 Task 11 处理；本任务只改 core。
- [ ] **Step 4：跑全量测试与 wind-config 门禁，确认全绿**
  `cargo test --workspace --no-fail-fast`
- [ ] **Step 5：提交**
  `feat(config): 旧注释模板键自动迁入 [ui.comment]，方案文件注释配置作废`

## 阶段二：两态与切换

### Task 5: 持久态与临时翻转

**Files:**
- Modify: `wind-coordinator/src/coordinator.rs`
  - `State` 加字段；
  - 新增 `effective_comment_state`、`set_comment_state`；
  - `cancel_session`（:5992）与 `commit_text_ending_session`（handle_candidate.rs:5205）里清掉临时翻转。
- Modify: `wind-coordinator/src/coordinator/message_handler.rs:1019-1024`：与 `grid_keep` 同一处做懒清理。
- Modify: `comment.rs`：`comment_template_for` 与 `alt_commit_text` 都读 `effective_comment_state`。
- Test: `wind-coordinator/tests/comment_state.rs`（新建，用现有协调器测试夹具）

**Interfaces:**
- Produces：
  ```rust
  // State 新字段
  pub(crate) comment_session_flip: bool,
  pub(crate) comment_hold_flip: bool,
  pub(crate) alt_track: Option<AltTrack>,   // Task 6 使用
  pub(crate) struct AltTrack { pub down_at: Instant, pub dirty: bool }

  pub(crate) fn effective_comment_state(cfg: &Config, state: &State) -> CommentState;
  // 持久态 XOR session_flip XOR hold_flip
  pub(crate) fn set_comment_state(&self, state: &mut State, to: CommentState) -> anyhow::Result<()>;
  // 写回 ui.comment.state：仿照 cmd_toggle_layout（:7166），只改内存镜像加
  // Config::set_user_string(&["ui","comment","state"], ..)，不 reload；之后 notify_ui_update
  pub(crate) fn toggle_comment_session(&self, state: &mut State);
  // 翻转 session_flip；有候选时 notify_ui_update，不重新查询
  ```

- [ ] **Step 1：写失败测试**
  - `session_flip_shows_detail_then_resets_on_commit`：
    - 打 `gg`，调 `toggle_comment_session`，首条注释等于 Detail 的结果；
    - 上屏后再打 `gg`，首条注释回到 Default。
  - `session_flip_resets_before_association`：开联想。翻转后上屏，联想候选的注释按持久态（Review Focus 3）。
  - `session_flip_survives_entering_temp_pinyin`：翻转后进入临时拼音，注释取 temp_pinyin 的 Detail 格（chaizi_code）。
  - `persist_detail_then_session_flip_shows_default`：持久态为 Detail 时再翻转，显示 Default。
  - `set_comment_state_writes_back`：调用后用户层 `ui.comment.state == "detail"`；重建协调器后仍是 Detail。
  - `alt_commit_uses_effective_state`：`alt_commit = "comment"`，翻转到 Detail 后 Alt+1 上屏的是 Detail 那份注释。
  - `flip_does_not_requery`：翻转前后候选列表、顺序、高亮下标都相同。
- [ ] **Step 2：运行测试，确认失败**
- [ ] **Step 3：实现**
- [ ] **Step 4：运行测试，确认通过；再跑全量测试**
- [ ] **Step 5：提交**
  `feat(comment): 默认/详细两态，临时翻转随候选窗关闭复位`

### Task 6: 协调器侧 Alt 单击 / 按住判定

**协议（我方决定）：**
- 平台层只在**组字中**把 Alt 的按下和松开转发给服务：`key_code` 为 `0xA4`（左）或 `0xA5`（右），不带修饰位。
- 服务返回 `Consumed` 时，平台吃掉这次事件；返回 `PassThrough` 时放行。
- 判定全部在服务里做，平台不做单击计时。

**Files:**
- Modify: `wind-coordinator/src/coordinator/message_handler.rs`
  - `handle_key_event`（:1006）：keydown 入口加 Alt 分支；任何非 Alt 键都把 `alt_track.dirty` 置为 true；
  - keyup 分支（:1101）在 `handle_select_key_up` 之前加 Alt 分支。
- Create: `wind-coordinator/src/comment_alt.rs`（判定的纯函数，便于测试）
- Modify: `wind-keys/src/keymap.rs`：新增 `pub fn is_alt_vk(vk: u32) -> bool`（0xA4 / 0xA5 / 0x12）。
  **不要**扩 `is_pure_modifier_vk`，那会改变 Shift/Ctrl 现有路径的语义。
- Test: `wind-coordinator/tests/comment_alt.rs`

**Interfaces:**
- Consumes：Task 5 的 `AltTrack`、`toggle_comment_session`、`set_comment_state`。
- Produces：
  ```rust
  pub(crate) enum AltOutcome { PassThrough, Consumed }
  pub(crate) fn on_alt_down(mode: AltSwitch, has_candidates: bool, track: &mut Option<AltTrack>, now: Instant) -> (AltOutcome, AltEffect);
  pub(crate) fn on_alt_up(mode: AltSwitch, has_candidates: bool, track: &mut Option<AltTrack>, now: Instant) -> (AltOutcome, AltEffect);
  pub(crate) fn on_other_key(track: &mut Option<AltTrack>);
  pub(crate) enum AltEffect { None, ToggleSession, TogglePersist, HoldOn, HoldOff }
  pub(crate) const ALT_TAP_MAX: Duration = Duration::from_millis(500); // 与 TSF 的 TOGGLE_TAP_THRESHOLD_MS 一致
  ```

**判定规则：**
- **适用条件**：`has_candidates` 为 false，或 `ui.comment.enabled` 为 false，或模式为 `Off`，都返回 `PassThrough` / `None`。
- **`Session` / `Persist` 档**：
  - 按下时记录 track，返回 `PassThrough`；
  - 松开时，如果 track 存在、`!dirty`、且按住时长不超过 `ALT_TAP_MAX`，返回 `Consumed` 和 `ToggleSession` 或 `TogglePersist`；
    否则返回 `PassThrough`。
  - 无论哪种结果，松开后都清掉 track。
- **`Hold` 档**：
  - 按下时返回 `Consumed` 和 `HoldOn`；松开时返回 `Consumed` 和 `HoldOff`。
  - 按住期间来了别的键：先照常处理那个键，处理完再执行 `HoldOff`。所以 Alt+数字上屏的是详细态（设计稿 §4）。
  - 之后的 Alt 松开返回 `Consumed`，`HoldOff` 重复执行无害。
- **按键表让位**：用户在 `key_actions` / `session_actions` 里绑了裸 `lalt` / `ralt` 时，用户绑定优先。
  加载时 `warn` 一次「alt_switch 已让位」。

- [ ] **Step 1：写失败测试**
  - 纯函数层：
    - `tap_toggles_session`：按下后 100ms 松开，返回 `(Consumed, ToggleSession)`。
    - `long_press_is_not_tap`：600ms 后松开，返回 `PassThrough`。
    - `combo_is_not_tap`：按下，接着 `on_other_key`，再松开，返回 `PassThrough`。
    - `no_candidates_passes_through`：`has_candidates = false` 时按下、松开都是 `PassThrough`（Review Focus 2）。
    - `off_mode_passes_through`
    - `persist_mode_toggles_persist`
    - `hold_on_off`
  - 协调器集成：
    - `alt_tap_rerenders_detail`：打 `gg`，Alt 按下再松开（0xA4），首条注释变为 Detail，且松开返回 `Consumed`。
    - `alt_digit_does_not_toggle`：打 `gg`，Alt 按下，Alt+1（alt_commit），Alt 松开。断言：上屏照常，态没有翻转（Review Focus 1）。
    - `hold_alt_digit_commits_detail_comment`：`Hold` 档，Alt 按下后 Alt+1，上屏文本等于 Detail 注释；松开后态复位。
    - `user_bound_lalt_wins`：`session_actions` 里绑了 `lalt`，Alt 单击走用户绑定，不翻转。
- [ ] **Step 2：运行测试，确认失败**
- [ ] **Step 3：实现**
- [ ] **Step 4：运行测试，确认通过；再跑全量测试**
- [ ] **Step 5：提交**
  `feat(comment): 组字中单击/按住 Alt 切换注释详细态`

### Task 7: 平台层转发 Alt（TSF / macOS / Linux）

**Files:**
- Modify: `wind_tsf/src/KeyEventSink.cpp`
  - :787 一带：单独按 Alt 时，**组字中**向服务转发按下，按回包决定是否吃掉；
  - 松开路径（`OnTestKeyUp` :1721、`OnKeyUp` :1801）：组字中转发 VK_MENU 的松开，回包是 Consumed 就把 `*pfEaten` 置为 TRUE；
  - `_IsMatchingKeyUp`（:2160）补上 VK_MENU 分支；
  - `DispatchHotkey`（:2212）入口：Alt 按住期间收到热键时，照常把该组合键上报给服务（服务据此把 dirty 置为 true）。
- Create/Modify: `wind_tsf/src/AltKey.h`
  - 纯函数 `UINT MapAltVk(WPARAM wParam, LPARAM lParam)`：VK_MENU 按 lParam 的扩展位区分，扩展位为 1 映射到 0xA5，否则 0xA4。
  - 纯函数 `bool ShouldForwardAlt(bool composing)`。
- Test: `wind_tsf/tests/` 新增 `alt_key_test.cpp`，本机 g++ 直接跑。
- Modify: `wind_macos/Sources/WindInputApp/Controller/InputController.swift:349`
  - `modifierInfo(forKeyCode:)` 加 keyCode 58（左 Option，映射 0xA4）和 61（右 Option，映射 0xA5）；
  - 组字中把按下和松开都发给服务；按回包决定 `handle` 返回 true 还是 false。
- Modify: `wind_linux/src/core/KeyMap.cpp`、`wind_linux/src/fcitx/WindEngine.cpp:827`
  - 组字中，Alt_L / Alt_R 的按下和松开不再被 `isModifier()` 早退，而是上报；回包 Consumed 时 `filterAndAccept`。

**Interfaces:**
- Consumes：Task 6 的协议（0xA4/0xA5、Consumed/PassThrough）。

- [ ] **Step 1：写 TSF 纯函数测试，确认失败**
  - `MapAltVk(VK_MENU, 扩展位=1) == 0xA5`；
  - `MapAltVk(VK_MENU, 扩展位=0) == 0xA4`；
  - `ShouldForwardAlt(false) == false`。
- [ ] **Step 2：实现 TSF 改动，跑 `wind_tsf/tests`，确认通过**
- [ ] **Step 3：实现 macOS 改动**
  本机 `cargo check --target aarch64-apple-darwin` 只覆盖 Rust 部分，Swift 用例靠 CI；
  按 [[macos-cross-check-on-linux]] 的口径说明哪些没覆盖到。
- [ ] **Step 4：实现 Linux 改动**
  跑 `wind_linux` 现有单测与 e2e（KDE 靶机 .51），确认通过。
- [ ] **Step 5：靶机验证，Windows dev 变体部署（d1 + pd1）**
  - 靶机跑不了 GUI 自动化，给维护者人工判据：
    1. 记事本组字中单击 Alt，注释变详细，菜单栏不激活；
    2. 不在组字时单击 Alt，菜单栏激活；
    3. 组字中 Alt+Tab 切走，回来后态没有翻转；
    4. 组字中 Alt+1，上屏注释照常，态没有翻转。
- [ ] **Step 6：提交**
  `feat(platform): 组字中转发 Alt 供注释切换`

### Task 8: 动作命令（按键动词 + 命令栏）

**Files:**
- Modify: `wind-config/src/config.rs`
  - `BoundAction`（:1610）加 `CommentState(CommentStateAction)`；
  - `SessionAction`（:1926）加同名变体；
  - 两个 `parse`（:1810 / :2062）照 `single_char` 的先例解析；
  - `requires_candidates`（:2129）：只有 `Session` 档返回 true；
  - `Display`（:2186）。
- Modify: `wind-config/src/hotkey.rs:94`：GLOBAL 抢占分类为 false。同步测试动词表 hotkey.rs:1836/:1885、config.rs:7651。
- Modify: `wind-coordinator/src/handle_mode.rs:838` 一带（`commit_and_enter_bound_action`）、`coordinator.rs:5823` 一带（`apply_session_action`）。
- Modify: `wind-cmdbar/src/services.rs:153`：新增 `fn set_value(&self, target: &str, value: &str) -> anyhow::Result<()>`，默认 bail。
- Modify: `wind-cmdbar/src/funcs/dict_ime.rs`
  - `fn_ime_set`（:96）：target 为 `comment` 时绕过 `parse_switch`，调 `set_value`；
  - `specs()` 的说明文案加上 `comment`。
- Modify: `wind-coordinator/src/handle_cmdbar.rs:309`（`CoordIme`）与 `coordinator.rs:7019`（`cmd_ime_toggle` 加 `"comment"` 臂）。
- Modify: `data/system.phrases.toml`：在 :588 之后加 `$CC("切注释", ime.toggle("comment"))`，格式照邻近条目。
- Test: `wind-config` 解析测试、`wind-cmdbar` 的 dict_ime 测试（RecIme :235）、`wind-coordinator/tests/comment_state.rs` 追加用例。

**Interfaces:**
- Produces：
  ```rust
  pub enum CommentStateAction { Toggle, Set(CommentState), Session }
  impl CommentStateAction { pub fn parse_payload(rest: &str) -> Option<Self>; pub fn as_payload(&self) -> &'static str; }
  // 载荷："" → Toggle；"default" / "detail" → Set；"session" → Session
  ```
  - 动词名 `comment_state`，写法如 `comment_state`、`comment_state:detail`、`comment_state:session`。
  - `BoundAction::parse("comment_state:session")` 判为无效（无会话时没有意义），`is_enabled` 为 false，加载时 warn。
- Consumes：Task 5 的 `set_comment_state`、`toggle_comment_session`。

- [ ] **Step 1：写失败测试**
  - `parses_comment_state_verbs`：
    - 两张表都能解析 `comment_state` 和 `comment_state:detail`；
    - 只有 SessionAction 能解析 `comment_state:session`；
    - `Display` 往返一致。
  - `ime_set_comment_accepts_state_values`：`ime.set("comment","detail")` 调到 `set_value("comment","detail")`；
    `ime.set("comment","on")` 报错；`ime.set("dict:x","on")` 行为不变。
  - `ime_toggle_comment_flips_persist`：命令栏 `ime.toggle("comment")` 之后，持久态翻转并写回。
  - `session_verb_toggles_flip_only`：`session_actions` 绑定 `comment_state:session`，触发后翻转但持久态不变。
- [ ] **Step 2：运行测试，确认失败**
- [ ] **Step 3：实现**
- [ ] **Step 4：跑全量测试**
- [ ] **Step 5：提交**
  `feat(comment): 注释两态的按键动词与 ime.toggle/set("comment")`

## 阶段三：内置中英释义

### Task 9: 内置注释词库挂载

**Files:**
- Modify: `wind-coordinator/src/coordinator.rs`
  - 在 `merge_comment_dicts`（:18789）里加入 `SystemComments.dicts`。规则与 auto 空壳相同：用户 `[[ui.comment_dicts]]` 里同 id 的条目只记 `enabled` 和位置；未认领的内置项默认启用，排在用户项之后、auto 项之前；
  - `sync_comment_dicts`（:4640）路径解析不变，仍走 `resolve_schema_resource`。
- Modify: `wind-config/src/config.rs:5883`：`CommentDictSpec` 加 `#[serde(default)] pub builtin: bool`，作为设置端认领内置项的空壳标记，与 `auto` 同形。
- Modify: `wind-webdata/src/lib.rs:414`：`comment.sources` 回包里内置项带 `origin: "builtin"`。
- Create: `data/schemas/comments/en_zh.txt`、`data/schemas/comments/zh_en.txt`，各放 3 行占位夹具，正式数据由 Task 10 生成后替换。
- Test：coordinator 的 merge 测试段。

- [ ] **Step 1：写失败测试**
  - `builtin_dicts_enabled_by_default`：用户没写任何条目时，有效列表含 `en_zh` 与 `zh_en`，二者均为 enabled。
  - `user_can_disable_builtin`：用户写 `{id="zh_en", builtin=true, enabled=false}`，`zh_en` 变为 disabled，位置不变。
  - `dict_var_reads_builtin`：临时英文 Detail 下打 `apple`，注释等于夹具里的释义。
- [ ] **Step 2：运行测试，确认失败**
- [ ] **Step 3：实现**
- [ ] **Step 4：运行测试，确认通过**
- [ ] **Step 5：提交**
  `feat(comment): 随包内置 en_zh / zh_en 注释词库`

### Task 10: 释义数据生成管线

**我方决定：**
- 上游的 ECDICT、CC-CEDICT 在构建时下载，固定提交 hash，与 rime-frost 的做法相同；
- AI 生成的部分作为源数据入库：`data/comments-src/zh_en_ai.tsv`，带表头注释写明来源与生成日期；
- 合并产物 `comments/*.txt` 在 `assemble_data` 里由脚本生成，不入库；
- Task 9 的占位夹具改为只在测试里使用。

**Files:**
- Create: `scripts/comment_dicts/build.py`
  - 子命令 `en-zh`：用 ECDICT 按 `en.dict.yaml` 与 `en_ext.dict.yaml` 的词表裁剪，每词输出 `词\t词性. 基础义` 一行，只取第一个词性、前两个义项；
  - 子命令 `zh-en`：用 CC-CEDICT 打底，加上 `zh_en_ai.tsv`，按系统词库（base / ext / idiom / wubi86_jidian）的词表裁剪，去重时 CEDICT 优先；
  - 子命令 `missing`：列出系统词库里两者都没有覆盖的词，作为 AI 生成的输入；
  - 子命令 `report`：输出条目数、覆盖率、文件字节数、gzip 后字节数。
- Create: `scripts/comment_dicts/ai_fill.py`：分批读 `missing` 的输出，调 Claude API（模型 `claude-sonnet-5-5`，批量 200 词/次，输出 `词\t词性. 英文`），可断点续跑，追加写进 `zh_en_ai.tsv`。没有可靠释义的词输出空，不硬凑。
- Modify: `scripts/dev.sh`
  - `download_dicts`（:437）加两个上游下载；
  - `assemble_data`（:609）调 `build.py en-zh` 与 `build.py zh-en`。
- Modify: 第三方声明文件，加 ECDICT（MIT）与 CC-CEDICT（CC BY-SA 4.0）的署名。路径照仓里现有 rime-frost 署名的位置。
- Test: `scripts/comment_dicts/test_build.py`（pytest，用小夹具）

- [ ] **Step 1：写失败测试**
  - `test_en_zh_keeps_first_pos_two_senses`
  - `test_zh_en_cedict_wins_over_ai`
  - `test_missing_lists_uncovered_only`
  - `test_ai_tsv_blank_rows_skipped`
- [ ] **Step 2：实现 `build.py`，运行测试确认通过**
- [ ] **Step 3：跑 `build.py report`，把英→中、中→英（仅 CEDICT 部分）的条目数、覆盖率、体积报给维护者**
- [ ] **Step 4：`ai_fill.py` 先跑 2000 词，抽 100 条样本给维护者看（设计稿 §11 第 3 项），等维护者认可后再跑全量**
- [ ] **Step 5：全量跑完后再出 `report`，报最终体积和 mmap 映射量。由维护者决定内置还是按需下载（§11 第 2 项）**
- [ ] **Step 6：提交**
  - 先提交脚本：`build(data): 中英释义注释词库生成脚本`；
  - 再单独提交 `zh_en_ai.tsv`：`data(comment): AI 补齐中→英释义`。

## 阶段四：设置端与文档

### Task 11: 设置端「候选注释」Tab 对话框（wind-setting 仓）

**Files（`../wind-setting`）：**
- Create: `src/dialogs/comment_settings.rs`：新对话框；
- Modify: `src/manifest.rs:3708-3834`：`build_dialog_button_comment` 改为打开新对话框；`comment_template_block` 保留，供模板库 Tab 复用；
- Modify: `src/assets/settings_manifest.toml:3297-3380`
  - 「候选标注」分区只保留「显示候选注释」开关和「候选注释…」按钮；
  - 竖排模板、`comment_above`、`max_chars` 的 `dialog_extra_for` 移进新对话框；
  - `ui.comment_dicts` 的按钮移进新对话框的 Tab。
- Modify: `src/dialogs/schema_manager.rs`：删除 `comment_tpl_row`、`build_comment_tpl_dialog`、`seed_comment_tpl` 与对应的保存逻辑（:880、:899、:1036、:1117、:1384、:2999、:3225-3230、:4433、:5747），换成一行「在 候选注释 中配置」的跳转。
- Modify: `src/custom_hide.rs:317-320`、`src/assets/capabilities.snapshot.json`、mockdata。
- Modify: `src/dialogs/key_binding.rs:593/674`（会话动词）与 `schema_manager.rs:1727/1768`（按键动词）：加上 `comment_state` 的三种写法。
- Core 侧：`wind_input/crates/wind-webdata/src/lib.rs:414` 一带新增 RPC `comment.library`。回包：
  ```json
  { "templates": [{"id","name","text","origin": "builtin"|"user"|"user_overrides_builtin"}],
    "system_assign": {...CommentAssign...},
    "effective": [{"scope": "global"|"type:codetable"|"schema:wubi86"|"mode:temp_english", "state": "default", "vertical": true, "template_id": "code_hint", "from_scope": "global"}] }
  ```
  `effective` 供表格里「跟随」格显示实际落到了哪个模板。

**对话框结构**（照设计稿 §9，Tab 用本仓惯例「`segmented` + 各页 `visible_when` 同时建好」，见 schema_manager.rs:5600-5640）：
1. **分配**：`Element::table_custom`，列为作用范围 / 默认·竖 / 默认·横 / 详细·竖 / 详细·横。
   - 每格是一个 `dropdown_items`，选项依次是：模板列表、「不显示」、「跟随」；横排格另有「同竖排」。
   - 「跟随」格用灰色斜体，后缀实际生效的模板名，如「跟随 · 编码提示」。
   - 常驻行：全局与五个类型。其余行：出厂分配里的行，加上用户已经写过的方案行、模式行。
   - 底部「＋ 添加方案覆盖…」「＋ 添加模式覆盖…」。删除行时：用户行直接删除；出厂行只能把各格设成「跟随」，即写 `inherit`。
   - 下方是选中格的实时预览。
2. **模板库**：左边列表分组（内置 / 自定义），右边用 `comment_template_block` 编辑；
   内置模板只读，提供「复制为自定义」；`UserOverridesBuiltin` 的模板标「已修改」，并提供「恢复内置」（删除用户同 id 项）。
3. **注释词库**：嵌入 `build_comment_dicts_dialog` 的内容，内置项标「内置」，不能删除，只能开关。
4. **切换**：总开关、当前持久态（单选）、Alt 四档下拉、命令短语 `切注释` 与按键动词说明；
   另含「显示」小节：`comment_max_chars_*`、`comment_above`。

- [ ] **Step 1：写 `comment.library` 回包测试，确认失败**
  在 webdata 测试段，按 `dict.systemStats` 测试的写法，断言 `templates` 含 `code_hint` 且 origin 为 builtin，`effective` 中 global/default/竖排 的 template_id 为 `code_hint`。
- [ ] **Step 2：实现 RPC，运行测试确认通过，提交 core**
  `feat(webdata): comment.library 供设置端注释总表`
- [ ] **Step 3：写设置端测试，确认失败**
  - 写回形态：选择「同竖排」时删除横排键；选择「跟随」时，用户行删键，出厂行写 `inherit`；选择「不显示」时写 `none`。
  - 动词标签：`session_verb_label("comment_state:session")` 非空。
- [ ] **Step 4：实现对话框，运行设置端测试确认通过**
  `cargo test`（wind-setting 本机原生）
- [ ] **Step 5：截图给维护者目视**
  按 [[wind-setting-linux-build]] 本机跑起来截图，内容是分配 Tab 与模板库 Tab。
- [ ] **Step 6：提交（wind-setting 仓）**
  `feat(comment): 候选注释改为 Tab 对话框，总表分配四列`

### Task 12: 文档站（WindInputDocs 仓）

**Files（`../WindInputDocs`）：**
- Modify: `content/docs/settings/appearance/candidate-comment.mdx`
  - 重写「分模式设置」（:552）及之后的配置示例，改成「模板库 + 总表 + 两态 + Alt」；
  - 模式清单补上 email / unicode；
  - 新增「升级说明」一节：旧键会自动迁移，方案文件里的注释配置已作废。
- Modify: `content/docs/guides/config/ui.mdx:28-56`、`input.mdx:421`、`special-modes.mdx:398`、
  `schemas.mdx`、`command-bar.mdx:562`（加 `ime.toggle/set("comment")`）。
- Modify: 快捷键参考 `content/docs/reference/hotkeys.mdx`：加入 Alt 单击切换与 `comment_state` 动词。

- [ ] **Step 1：改文档；拿 Task 1 的出厂数据核对示例里的 id 与值逐字一致**
- [ ] **Step 2：本地构建文档站，确认无断链**
- [ ] **Step 3：提交（文档仓），不 push**
  文档仓等发版时再推。
