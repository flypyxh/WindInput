# 覆盖的呈现规范 · 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把设置端「跟随 / 自定义」的五种写法收敛到 R8：勾选行为标准形态，跟随时显示值与来源；全局页可以反查「已被 N 个方案覆盖」。

**Architecture:** 分三层推进。
- 主仓（core）补数据：
  - 新增 RPC `theme.resolved`，给出主题解析后的值；
  - 新增 RPC `schema.overrideSummary`，给出跨方案的覆盖汇总；
  - `schema.getConfig` 增补 `followedBehavior`，给出方案对话框里各项的跟随值；
  - 角标颜色的键拆开，并做迁移。
- 设置端（wind-setting）复用现有的 `setting_row_override` / `override_control`，逐项整改。
- 最后写规则、扫一遍文案。

**Tech Stack:** Rust；主仓在 `wind_input/` workspace 下原生测试；设置端 `../wind-setting` 走 xwin + wine（`./scripts/dev.sh sk|st|sg`）；文档站在 `../WindInputDocs`（mdx）。

**Spec:** `docs/design/override-presentation.md`

## Global Constraints

- 跟随文案只用三个词：「跟随全局」「跟随主题」「跟随方案」，后接全角括号括起的值，例如 `跟随主题（18）`；禁用「默认」「自动」表示跟随（R8.3）。
- 三态的存储只用 R3 的两种写法：`Option<T>`（未设置 = 跟随），或 `""` / `0` 哨兵（仅当 0 不是合法值时才能用 `0`）；不得新增哨兵形态（R8.4）。
- 改已发布键的类型或语义，必须在 `Config::load` 链上加 Value 层迁移（`migrate_user_layer_value`），并同步 `prune_user_config` / `set_user_value` 两条路径（R4）。
- 非 config.toml 的落点写回，一律登记 `SideCommitter`，不另加保存按钮（R2）。
- 提交只用 `git commit -F - --only -- <自己的路径…>`，禁用 `git add -A` / `git add .` / `git commit -a` / `git stash`（本工作区常有并发会话）。
- 主仓测试命令，在 `wind_input/` 下执行：`CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt TMPDIR=/tmp/wct-ovr cargo test -p <crate> <filter>`，之前先 `mkdir -p /tmp/wct-ovr`。依赖 `build_dev/data` 的集成测试缺数据时会静默跳过：耗时 0.0x 秒的「通过」算假绿。
- 设置端测试：`./scripts/dev.sh st`，在 WindInput 仓根目录执行，无法按名过滤，整套约 1.6 秒。改了主仓的 REGISTRY 或 capabilities 后先跑 `./scripts/dev.sh sg` 重生成快照；重生成后 `appVersion` 那一行的 diff 不要提交。
- 格式化只对自己改的文件跑：`rustfmt --edition 2021 <file>`。不跑 `cargo fmt --all`，它会改到并发会话未提交的文件。

## Review Focus

1. **定制版隐藏了当前主题，或主题文件读盘失败**：`theme.resolved` 要与候选窗实际显示的主题一致，走 `theme_id_honoring_hide` 和回落主题，不能返回错误或空值 → Task 2 有用例钉住。
2. **旧配置里角标色值浅/深各为 8 位且 alpha 不同**：迁移后两侧的不透明度要各自保留，不能被合成一个值 → Task 4 有用例。
3. **用户已经选了「跟随全局」，之后又改了全局值**：方案对话框再次打开时，「跟随全局（值）」要显示新值，不能沿用旧种子 → Task 6 的种子每次打开都重新取 `schema.getConfig`，有用例钉住。
4. **某方案对同一个键既在 schema 文件里自带值、又在 override 层写了值**：`overrideSummary` 只统计一次这个方案 → Task 3 有用例。
5. **外观页把字号从「跟随」切到「自定义」**：初值要是主题当前的字号（例如 18），不能是 0；如果写回 0，就会悄悄又变成跟随 → Task 8 有用例。

---

### Task 1: 规则落文（R8）与设计文档修订

**Files:**
- Modify: `docs/architecture/config-design-rules.md`（在 `## R7 文档规则` 之前插入 `## R8 覆盖的呈现`；附录 checklist 第 6 步追加一句）
- Modify: `docs/design/override-presentation.md`（修订 F7 / F8 两行）

**Interfaces:**
- Produces: R8.1–R8.7 的条款编号。后续任务的提交信息和代码注释按这些编号引用。

- [ ] **Step 1: 插入 R8。** 把 `override-presentation.md` §2 的七条原文搬进来，标题改为 `## R8 覆盖的呈现（跟随 / 自定义）`，开头加一句：「存储形态归 R3；本条只管界面。专项设计见 `docs/design/override-presentation.md`。」然后把文件第 3 行的状态说明改成「§R5（受众分级）与 §R8 的整改项为已定方向、未全部实施」。

- [ ] **Step 2: checklist 第 6 步追加一句。** 原文：
  `6. 设置仓：五道闸门（R6）；manifest 项 + hint（按纪律）。`
  改为：
  `6. 设置仓：五道闸门（R6）；manifest 项 + hint（按纪律）；可跟随上一层的项按 R8 呈现（勾选行，跟随时显示值与来源）。`

- [ ] **Step 3: 修订设计文档 §4 表里的 F7 / F8 两行。** 这是调研后的更正。
  - F7 改为：「拆成颜色键（`""` = 跟随主字）+ `alpha_light` / `alpha_dark: Option<f32>`（未设置 = 跟随全局 `badge_alpha`）。浅、深两侧各有 alpha，旧 8 位色值两侧可以不同，合成一个值会丢信息。」
  - F8 改为：「核实：`reset` 只清 12 个 `SPEC_BEHAVIOR_FIELDS` + `frequency` + `english_merge`，界面隐藏的 `auto_phrase.*` 原样保留。本项按实测结论处理，见 Task 7。」

- [ ] **Step 4: 提交。**

```bash
git commit -F - --only -- docs/architecture/config-design-rules.md docs/design/override-presentation.md <<'EOF'
docs(config-rules): 新增 R8 覆盖的呈现 —— 勾选行为标准形态、跟随须带值与来源

R3 只管三态的存储，界面层五种写法各写各的；R8 把呈现收口。同步按调研更正设计文档
F7（角标透明度分浅深两键）与 F8（全部恢复跟随的实际清理范围）。
EOF
```

---

### Task 2: core · `theme.resolved` RPC

**Files:**
- Modify: `wind_input/crates/wind-coordinator/src/theme_query.rs`（新增 `ThemeFollowValues` 与 `Coordinator::theme_follow_values`）
- Modify: `wind_input/crates/wind-coordinator/src/web_host.rs`（trait 加方法 + 转发 impl）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（`theme.*` 分发处约 :593 加一个分支 + `web_theme_resolved`）
- Test: `wind_input/crates/wind-coordinator/src/theme_query.rs` 内 `#[cfg(test)]`；`wind_input/crates/wind-coordinator/tests/input_flow.rs` 追加一例
- Modify: `../wind-setting/src/rpc.rs`（`mock_reply` 加一个分支）

**Interfaces:**
- Produces（core）：
  ```rust
  // theme_query.rs
  #[derive(Debug, Clone, PartialEq, serde::Serialize)]
  pub struct ThemeFollowValues {
      pub theme_id: String,            // 实际生效的主题 id（已过 hide 与回落）
      pub font_size: i32,
      pub font_family: Option<String>, // None = 主题没给，渲染走系统字体
      pub font_weight: i32,            // 0 = 主题没给
      pub pager_bar_display: &'static str,   // "hide" | "always" | "auto"
      pub page_number_display: &'static str, // "show" | "hide"
      pub langbar_text: [Option<String>; 4], // cn_light, cn_dark, en_light, en_dark，"#RRGGBB"
  }
  impl Coordinator { pub fn theme_follow_values(&self) -> Option<ThemeFollowValues> }
  // web_host.rs
  fn theme_follow_values(&self) -> Option<crate::theme_query::ThemeFollowValues>;
  ```
- Produces（RPC 形状，camelCase）：`theme.resolved` → `{ "themeId", "fontSize", "fontFamily"|null, "fontWeight", "pagerBarDisplay", "pageNumberDisplay", "langbarText": [..4] }`；主题与回落主题都加载失败时返回 `null`。

- [ ] **Step 1: 写失败测试（纯函数部分）。** 从 `Resolved` 提取字段的逻辑做成纯函数，测试不依赖磁盘：

```rust
// theme_query.rs 末尾
#[cfg(test)]
mod follow_values_tests {
    use super::*;
    use wind_theme::resolve::{Resolved, ResolvedBehavior};

    #[test]
    fn pager_display_maps_theme_behavior() {
        let mut r = Resolved::default();
        r.behavior = ResolvedBehavior { hide_pager: true, ..Default::default() };
        assert_eq!(follow_values_of("default", &r).pager_bar_display, "hide");
        r.behavior = ResolvedBehavior { always_show_pager: true, ..Default::default() };
        assert_eq!(follow_values_of("default", &r).pager_bar_display, "always");
        r.behavior = ResolvedBehavior::default();
        assert_eq!(follow_values_of("default", &r).pager_bar_display, "auto");
    }

    #[test]
    fn page_number_and_font_fields_copied() {
        let mut r = Resolved::default();
        r.behavior.show_page_number = false;
        r.behavior.font_size = 20;
        r.views.text.font_weight = 500;
        let v = follow_values_of("x", &r);
        assert_eq!(v.page_number_display, "hide");
        assert_eq!(v.font_size, 20);
        assert_eq!(v.font_weight, 500);
        assert_eq!(v.font_family, None);
    }

    #[test]
    fn langbar_text_formats_rgb_hex_and_keeps_none() {
        let mut r = Resolved::default();
        r.langbar_text.cn_light = Some([0x11, 0x22, 0x33, 0xFF]);
        let v = follow_values_of("x", &r);
        assert_eq!(v.langbar_text[0].as_deref(), Some("#112233"));
        assert_eq!(v.langbar_text[1], None);
    }
}
```

- [ ] **Step 2: 确认测试失败。**
  运行：`cd wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt TMPDIR=/tmp/wct-ovr cargo test -p wind-coordinator follow_values_tests`
  预期：编译错误 `cannot find function follow_values_of`。

- [ ] **Step 3: 实现。**

```rust
// theme_query.rs
/// 主题对「外观可覆盖键」给出的值——设置端「跟随主题（值）」的数据源（R8.1）。
///
/// 只抽几个字段就把 `Resolved` 丢掉：它约 13 KB，按值传递在 debug 构建下曾把栈撑爆
/// （见本文件前面的说明），而 RPC 的 ctrl 线程没有另设栈大小。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFollowValues {
    pub theme_id: String,
    pub font_size: i32,
    pub font_family: Option<String>,
    pub font_weight: i32,
    pub pager_bar_display: &'static str,
    pub page_number_display: &'static str,
    pub langbar_text: [Option<String>; 4],
}

/// 与 `wind_ui::candidate_window::pager_visible` 的「跟随主题」分支同一裁决。
fn follow_values_of(theme_id: &str, r: &wind_theme::resolve::Resolved) -> ThemeFollowValues {
    let b = &r.behavior;
    let hex = |c: Option<wind_theme::palette::Rgba>| {
        c.map(|[r, g, b, _]| format!("#{r:02X}{g:02X}{b:02X}"))
    };
    let lt = &r.langbar_text;
    ThemeFollowValues {
        theme_id: theme_id.to_string(),
        font_size: b.font_size,
        font_family: r.views.text.font_family.clone(),
        font_weight: r.views.text.font_weight,
        pager_bar_display: if b.hide_pager {
            "hide"
        } else if b.always_show_pager {
            "always"
        } else {
            "auto"
        },
        page_number_display: if b.show_page_number { "show" } else { "hide" },
        langbar_text: [hex(lt.cn_light), hex(lt.cn_dark), hex(lt.en_light), hex(lt.en_dark)],
    }
}

impl Coordinator {
    /// 当前生效主题的跟随值。与桌面推送链同一裁决：先过定制版 hide，再回落
    /// `FALLBACK_THEME`（`load_theme_with_fallback`）；两级都失败返回 None。
    pub fn theme_follow_values(&self) -> Option<ThemeFollowValues> {
        let dirs = self.theme_search_dirs();
        let name = <Self as crate::web_host::WebDataHost>::current_theme_name(self);
        let is_dark = <Self as crate::web_host::WebDataHost>::current_theme_is_dark(self);
        let (id, resolved) = Self::load_theme_with_fallback(
            |n| wind_theme::load_resolved_dirs(&dirs, n, is_dark).map(Box::new),
            &name,
        )?;
        Some(follow_values_of(&id, &resolved))
    }
}
```

  `load_theme_with_fallback` 目前是 `handle_mode.rs` 里的私有 `fn`，要改成 `pub(crate) fn`。字段名以 `wind_theme::resolve` 的实际导出为准，`Rgba` 在 `wind_theme::palette`。

- [ ] **Step 4: 窄面 + 分发。**

```rust
// web_host.rs trait 内，current_theme_is_dark 之后
/// 主题对外观可覆盖键给出的值（设置端「跟随主题（值）」），见 `theme_query.rs`。
fn theme_follow_values(&self) -> Option<crate::theme_query::ThemeFollowValues>;
// 转发 impl 内
fn theme_follow_values(&self) -> Option<crate::theme_query::ThemeFollowValues> {
    Coordinator::theme_follow_values(self)
}
```

```rust
// wind-webdata/src/lib.rs，theme.* 分支内
"theme.resolved" => Ok(serde_json::to_value(self.theme_follow_values())?),
```

- [ ] **Step 5: 集成用例（真数据）。** 追加到 `wind-coordinator/tests/input_flow.rs`，照 `test_web_theme_preview_real` 的写法：

```rust
#[test]
fn test_web_theme_resolved_real() {
    if !has_schemas() { eprintln!("skip: no build_dev/data"); return; }
    let coord = Coordinator::new_headless(config_with("pinyin"), Some(&data_dir()));
    let v = coord.web_data_rpc("theme.resolved", &json!({})).unwrap();
    assert!(v["fontSize"].as_i64().unwrap() > 0, "{v}");
    assert!(["hide", "always", "auto"].contains(&v["pagerBarDisplay"].as_str().unwrap()));
    assert_eq!(v["langbarText"].as_array().unwrap().len(), 4);
}
```

  再补一例覆盖 Review Focus 1：先把 `ui.theme.name` 设为一个不存在的 id，再调 `theme.resolved`，断言返回的 `themeId` 等于回落主题 id（在 `handle_mode.rs` 里查 `FALLBACK_THEME` 的值），并且 `fontSize > 0`。

- [ ] **Step 6: 运行并确认全绿。** 执行 `cargo test -p wind-coordinator follow_values_tests test_web_theme_resolved`，输出里要看到这两个用例真的跑了，不是 skip。

- [ ] **Step 7: 设置端 mock。** 在 `../wind-setting/src/rpc.rs` 的 `mock_reply` 里加一个分支：

```rust
"theme.resolved" => json!({
    "themeId": "default", "fontSize": 18, "fontFamily": null, "fontWeight": 0,
    "pagerBarDisplay": "auto", "pageNumberDisplay": "show",
    "langbarText": ["#000000", "#FFFFFF", "#000000", "#FFFFFF"],
}),
```

- [ ] **Step 8: 格式化并提交。** 两个仓分开提交，主仓提交信息写 `feat(rpc): theme.resolved —— 设置端「跟随主题（值）」的数据源（R8.1）`；wind-setting 提交信息写 `test(mock): theme.resolved`。

---

### Task 3: core · `schema.overrideSummary` 与 `followedBehavior`

**Files:**
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`：分发处加 `"schema.overrideSummary"`；在 `web_schema_get_config`（约 :1409）里增补 `followedBehavior`，并登记 `READONLY_SIDECAR_FIELDS`（约 :4087）
- Modify: `wind_input/crates/wind-config/src/config_schema.rs`：新增纯函数 `schema_overridden_keys(schema: &Schema) -> Vec<&'static str>`
- Test: `config_schema.rs` 内单测；`wind-coordinator/tests/input_flow.rs` 追加一例
- Modify: `../wind-setting/src/rpc.rs` mock

**Interfaces:**
- Produces：
  - `pub fn schema_overridden_keys(schema: &crate::schema::Schema) -> Vec<&'static str>`：返回该方案**合并后**（schema 文件 ⊕ override 层）覆盖了哪些**全局 config 键**，按 `SCHEMA_OVERRIDES` 的映射。键名直接取 `CodeTableSpec` 同名字段，例如 `schema.codetable.top_code_commit`；`[candidate].layout` 非 Follow → `ui.candidate.layout`；`[punct].mode` 非 Follow → `input.punct` 段对应键（以 `SCHEMA_OVERRIDES` 登记为准）；`aux_code.enabled` / `max_phrase_len` → 对应的 `schema.pinyin.aux_code.*` 键。
  - `schema.overrideSummary` → `{ "<config_key>": ["<schema_id>", ...] }`：只统计 `available_schemas()`（已排除 hidden），每个方案对每个键最多计一次，数组按方案列表顺序排列。
  - `schema.getConfig` 增补 `followedBehavior`：`{ "layout": "horizontal"|"vertical", "fontFamily": "<全局 ui.font.family，空则取 theme.resolved 的 fontFamily，再空则 \"\">", "auxEnabled": bool, "auxMaxPhraseLen": u64 }`。

- [ ] **Step 1: 写失败测试（纯函数）。**

```rust
// config_schema.rs tests
#[test]
fn overridden_keys_lists_codetable_option_fields_once() {
    let s: crate::schema::Schema = toml::from_str(r#"
        [schema]
        id = "t"
        [engine]
        type = "codetable"
        [engine.codetable]
        top_code_commit = true
        [candidate]
        layout = "vertical"
    "#).unwrap();
    let keys = schema_overridden_keys(&s);
    assert!(keys.contains(&"schema.codetable.top_code_commit"), "{keys:?}");
    assert!(keys.contains(&"ui.candidate.layout"), "{keys:?}");
    assert_eq!(keys.iter().filter(|k| **k == "schema.codetable.top_code_commit").count(), 1);
}

#[test]
fn overridden_keys_ignores_follow_and_none() {
    let s: crate::schema::Schema = toml::from_str(r#"
        [schema]
        id = "t"
        [engine]
        type = "codetable"
        [candidate]
        layout = "follow"
    "#).unwrap();
    assert!(schema_overridden_keys(&s).is_empty());
}

/// 每个被 SCHEMA_OVERRIDES 登记为「可被方案覆盖」的键，都必须能被本函数识别，
/// 否则全局页的「已被 N 个方案覆盖」会对它恒显示 0（R8.6），这是护栏。
#[test]
fn overridden_keys_covers_every_schema_override_entry() { /* 为每个登记键构造一份只设该键的最小 Schema，断言它出现在结果中 */ }
```

  第三个用例的函数体要写全：遍历 `SCHEMA_OVERRIDES`，前缀型条目按 `CodeTableSpec` 字段名展开，每个键各构造一份 TOML。**先验证它能红**：临时从实现里删掉一个字段，确认测试失败后再恢复。（本仓记忆「护栏要先验可观测性」：下游对照可能和上游闸门是同一个不等式，恒空过。）

- [ ] **Step 2: 确认测试失败。** 执行 `cargo test -p wind-config overridden_keys`，预期结果是找不到该函数（编译失败）。

- [ ] **Step 3: 实现 `schema_overridden_keys`。** 逐字段比对 `Option::is_some()`、`LayoutIntent::Follow`、`PunctIntent::Follow`，映射表写在函数旁边。字段清单以 `schema.rs:338-478`（CodeTableSpec）、`:300`（AuxCodeSpec）、`:714`（PunctSpec）、`:782`（CandidateSpec）为准。`input_chars` / `leading_chars` 以空串表示未设置。

- [ ] **Step 4: 实现 RPC。**

```rust
"schema.overrideSummary" => self.web_schema_override_summary(),

fn web_schema_override_summary(&self) -> anyhow::Result<Value> {
    let mgr = self.engine_mgr();
    let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
    for id in mgr.available_schemas() {          // 已排除定制版 hide
        let Some(s) = mgr.schema_merged(&id) else { continue };
        for key in wind_config::config_schema::schema_overridden_keys(&s) {
            out.entry(key).or_insert_with(|| json!([]))
               .as_array_mut().unwrap().push(json!(id));
        }
    }
    Ok(Value::Object(out))
}
```

  `available_schemas()` / `schema_merged()` 的返回类型以 `manager.rs:2288` / `:2623` 为准。

- [ ] **Step 5: 在 `web_schema_get_config` 里增补 `followedBehavior`。** 全局值从 `Config::load` 结果里读：`ui.candidate.layout`、`ui.font.family`、`schema.pinyin.aux_code.enabled` / `max_phrase_len`。`fontFamily` 为空时取 `self.theme_follow_values().and_then(|t| t.font_family)`。把 `"followedBehavior"` 登记进 `READONLY_SIDECAR_FIELDS`，否则 `schema.saveConfig` 会把它当作覆盖值写回。

- [ ] **Step 6: 集成用例。** 覆盖 Review Focus 4：用真实数据构造一个码表方案，schema 文件自带 `top_code_commit`，override 层再写一次同一个键；断言 `overrideSummary["schema.codetable.top_code_commit"]` 里这个方案 id 只出现一次。再断言 `schema.getConfig` 返回的 `followedBehavior.layout` 是 `"horizontal"` 或 `"vertical"`。

- [ ] **Step 7: 运行并确认全绿。** 执行 `cargo test -p wind-config overridden_keys && cargo test -p wind-coordinator override_summary`。

- [ ] **Step 8: 设置端 mock。** `mock_reply` 新增 `"schema.overrideSummary" => json!({"ui.candidate.layout": ["wubi86"]})`；`"schema.getConfig"` 的 mock 分支（约 :1646）补上 `followedBehavior` 字段。

- [ ] **Step 9: 格式化并按仓分别提交。** 主仓提交信息写 `feat(rpc): schema.overrideSummary + getConfig.followedBehavior —— 反查与跟随值（R8.2/R8.6）`。

---

### Task 4: core · 角标颜色拆键与迁移（F7）

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs`：`LangBarBadge`（约 :5457）加字段 `alpha_light` / `alpha_dark`；`default_langbar_color_auto` 返回 `""`；`migrate_user_layer_value`（约 :7518）挂新的迁移函数；另加迁移函数与单测
- Modify: `wind_input/crates/wind-coordinator/src/coordinator/langbar_icon.rs`：把 `parse_color`（:254-273）提取成不带 cfg 的纯函数 `badge_color_of(color: &str, alpha: Option<f32>) -> BadgeColor`，外层调用处改用它
- Modify: `wind_input/data/config.toml`：`[ui.langbar]` 段的注释（:1413-1417）和三条 `[[ui.langbar.badges]]`（:1419-1441）
- Modify: `../WindInputDocs/content/docs/guides/config/ui.mdx`（:456-468、:484-511）与 `../WindInputDocs/content/docs/settings/appearance/index.mdx`（:294-334）

**Interfaces:**
- Produces：
  ```rust
  pub struct LangBarBadge { /* 原有字段 */ 
      /// "" = 与主字同色；"#RRGGBB"。
      pub color_light: String, pub color_dark: String,
      /// None = 跟随全局 badge_alpha；Some(1.0) 切挖空档（语义同旧 8 位 FF）。
      #[serde(default, skip_serializing_if = "Option::is_none")] pub alpha_light: Option<f32>,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub alpha_dark: Option<f32>,
  }
  fn migrate_langbar_badge_colors_value(layer: &mut toml::Value)
  ```
  （coordinator 内）`pub(crate) fn badge_color_of(color: &str, alpha: Option<f32>) -> wind_ui::langbar_icon::BadgeColor`

- [ ] **Step 1: 写失败测试（迁移）。** 照 `font_size_after_migration` 的真实链路写：

```rust
fn badges_after_migration(user_toml: &str) -> Vec<LangBarBadge> {
    let mut user: toml::Value = toml::from_str(user_toml).unwrap();
    Config::migrate_user_layer_value(&mut user);
    let mut merged = toml::Value::try_from(Config::default()).unwrap();
    merge_value(&mut merged, user);
    let cfg: Config = merged.try_into().expect("反序列化");
    cfg.ui.langbar.badges
}

#[test]
fn migrate_badge_auto_becomes_empty() {
    let b = badges_after_migration(
        "[[ui.langbar.badges]]\nstate = \"punct_cn\"\ncolor_light = \"auto\"\ncolor_dark = \"auto\"\n");
    assert_eq!(b[0].color_light, "");
    assert_eq!(b[0].alpha_light, None);
}

#[test]
fn migrate_badge_rgb6_keeps_color_and_follows_global_alpha() {
    let b = badges_after_migration(
        "[[ui.langbar.badges]]\nstate = \"punct_cn\"\ncolor_light = \"#2288E0\"\ncolor_dark = \"#2288E0\"\n");
    assert_eq!(b[0].color_light, "#2288E0");
    assert_eq!(b[0].alpha_light, None);
}

/// Review Focus 2：浅、深两侧 8 位色值的 alpha 不同，迁移后各自保留。
#[test]
fn migrate_badge_rgba8_splits_alpha_per_side() {
    let b = badges_after_migration(
        "[[ui.langbar.badges]]\nstate = \"punct_cn\"\ncolor_light = \"#2288E080\"\ncolor_dark = \"#2288E0FF\"\n");
    assert_eq!(b[0].color_light, "#2288E0");
    assert_eq!(b[0].color_dark, "#2288E0");
    assert!((b[0].alpha_light.unwrap() - 128.0 / 255.0).abs() < 1e-3);
    assert_eq!(b[0].alpha_dark, Some(1.0));
}

#[test]
fn factory_badges_have_no_auto_and_no_alpha() {
    for b in Config::default().ui.langbar.badges {
        assert!(b.color_light != "auto" && b.color_dark != "auto");
        assert_eq!((b.alpha_light, b.alpha_dark), (None, None));
    }
}
```

- [ ] **Step 2: 确认测试失败。** 执行 `cargo test -p wind-config migrate_badge factory_badges`，预期结果是找不到字段 `alpha_light`。

- [ ] **Step 3: 实现。**
  - 结构体：加两个 `Option<f32>` 字段。重写 `color_light` 的文档注释：去掉「末两位 AA」那段，改成「`""` 与主字同色；透明度见 `alpha_light`」，同时保留「FF 切挖空档」的说明，把它挪到 alpha 字段上。
  - `default_langbar_color_auto()` 改名为 `default_langbar_color_follow()`，返回 `String::new()`。
  - 迁移函数：遍历 `ui.langbar.badges` 数组里的每个表，对 `color_light` / `color_dark` 分别处理：
    - 值为 `auto` 时写 `""`；
    - 值为 `#RRGGBBAA` 时截成 `#RRGGBB`，并写 `alpha_x = AA/255`（用 f64 写入 toml）；
    - 其余值不动。
    
    在 `migrate_user_layer_value` 里挂上这个函数。`prune_user_config` / `set_user_value` 两条路径都调 `migrate_user_layer_value`（参照 :7760 / :8779 已有的调用），确认新函数被覆盖到。
  - `langbar_icon.rs`：`badge_color_of` 的逻辑是：空串得到 `BadgeColor::AUTO` 并带上 alpha；6 位 hex 经 `parse_hex` 得到 rgb 并带上 alpha。**为兼容定制层没迁移的旧写法，`auto` 和 8 位值仍要能读**（8 位时它自带的 alpha 优先于 `alpha` 参数），但只读不写。不带 cfg，写两例单测：`badge_color_of("", None)` 应得到 AUTO；`badge_color_of("#112233", Some(0.5))` 的 alpha 应为 0.5。

- [ ] **Step 4: 运行测试。**
  执行：`cargo test -p wind-config migrate_badge factory_badges langbar && cargo test -p wind-coordinator badge_color_of && cargo test -p wind-config data_config_toml`。
  预期全部通过，含 L1 与 L2 同源的守门测试。

- [ ] **Step 5: 更新 `data/config.toml`。** 三条出厂规则保持现有色值；注释从 `auto` / 8 位的说明改为 `""` / `alpha_light` / `alpha_dark` 的说明。

- [ ] **Step 6: 更新文档站两个 mdx。**
  - `ui.mdx` 表格：`color_light/dark` 的可选值改为 `""`（与主字同色）和 `#RRGGBB`；新增 `alpha_light/dark` 两行，写明「未设置 = 跟随 `badge_alpha`，1.0 = 挖空档」。
  - `appearance/index.mdx`：:310-311 那一行同样改掉。
  - 两处都加一句「旧写法 `auto` / 8 位色值仍可读取，会自动迁移」。

- [ ] **Step 7: 提交。** 分两笔：
  - 主仓：`feat(langbar)!: 角标颜色与透明度拆成两键 —— auto/8 位哨兵迁为 ""/alpha_light|dark（R8.4）`，正文写明迁移规则，以及「定制层的旧写法仍可读」。
  - 文档站：`docs(config): 角标 alpha_light/alpha_dark`。

---

### Task 5: 设置端 · 辅助码改为勾选行（F1）

**Files:**
- Modify: `../wind-setting/src/dialogs/schema_manager.rs`：删除分段控件相关的 `AUX_CODE_MODES`（:231）、`aux_code_idx` / `aux_code_value`（:251-265），`SettingsSection::AuxCode` 的 UI（:4154-4170）改用两行 `setting_row_override`；种子（:2109-2120）与保存（:2371-2380）同步改
- Test: 同一文件 `mod tests`（:6077）；改写 :5742 / :5757 / :5777 三个现有用例

**Interfaces:**
- Consumes：`schema.getConfig` 返回的 `followedBehavior.auxEnabled` / `auxMaxPhraseLen`（Task 3）；`widgets::setting_row_override(over, label, sub, follow_source, follow_role, control)`、`widgets::override_control(over, gate, editor, follow_text, reason, width)`
- Produces：状态 signal `settings_aux_on_over: Signal<bool>`、`settings_aux_on: Signal<bool>`、`settings_aux_len_over: Signal<bool>`、`settings_aux_len: Signal<i32>`；纯函数：
  ```rust
  fn aux_write(on_over: bool, on: bool, len_over: bool, len: i32) -> (Value, Value) // (enabled, max_phrase_len)，未勾为 Value::Null
  fn follow_text_bool(v: bool) -> String  // "开" / "关"
  ```

- [ ] **Step 1: 改写现有的三个用例，并新增用例（先红）。**

```rust
#[test]
fn aux_unchecked_rows_write_null() {
    assert_eq!(aux_write(false, true, false, 4), (Value::Null, Value::Null));
}
#[test]
fn aux_checked_rows_write_values() {
    assert_eq!(aux_write(true, false, true, 3), (json!(false), json!(3)));
}
#[test]
fn aux_follow_source_text_carries_global_value() {
    assert_eq!(format!("跟随全局（{}）", follow_text_bool(true)), "跟随全局（开）");
}
```

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，预期编译失败。**

- [ ] **Step 3: 实现。** UI 照搬 `schema_codetable.rs:485-535` 的 `row` 写法，两行：
  - 「本方案启用辅助码」：editor 为 `Element::switch(settings_aux_on)`，`follow_text = follow_text_bool(followed.aux_enabled)`；
  - 「辅助码最长词长」：editor 为数字框，范围 0–20，与全局 manifest `:637-647` 一致；`follow_text = followed.aux_max_len.to_string()`；gate 为「启用辅助码的生效值为真」，reason 写「未启用辅助码」。

  两行放进 `override_group_card("辅助码", summary, rows)`，summary 复用 `ct_summary` 的格式，例如「全部跟随 / 已自定义 N 项」。种子从 `cfg.pointer("/engine/aux_code/enabled")` 取值（`null` 表示未勾），跟随值从 `cfg["followedBehavior"]` 取；保存时用 `aux_write` 算出两个值，再调 `set_json_path`。

- [ ] **Step 4: 运行 `./scripts/dev.sh st`，预期全部通过。**

- [ ] **Step 5: 目视。**
  执行 `WIND_RPC_MOCK=1` 加 `--screenshot /tmp/wct-ovr/aux.png`，完整命令见 WindInput `AGENTS.md` 的 windui CLI 段；打开方案设置对话框的辅助码节。
  检查：不勾选时右侧显示「开」，来源列为灰色的「跟随全局」。

- [ ] **Step 6: 提交（wind-setting）。** `feat(schema): 辅助码改用勾选行并补 max_phrase_len —— 照 aux-code-settings-ui P3（R8.1）`

---

### Task 6: 设置端 · 下拉「跟随」档带值（F2）与候选字体改勾选行（F3）

**Files:**
- Modify: `../wind-setting/src/dialogs/schema_manager.rs`：标点下拉（:4282）、候选排列下拉（:4290）、overlay 候选排列（:4260）、候选字体（:4305-4309，以及 :328-332、:2059、:2408-2412）
- Test: 同一文件 `mod tests`

**Interfaces:**
- Consumes：`followedBehavior.layout` / `.fontFamily`（Task 3）
- Produces：
  ```rust
  fn follow_option_label(scope: &str, value_label: &str) -> String // follow_option_label("全局", "横排") == "跟随全局（横排）"
  fn layout_label(v: &str) -> &'static str   // "horizontal"→"横排", "vertical"→"竖排"
  fn font_write(over: bool, family: &str) -> Value // 未勾 → Value::Null；勾且空 → Value::Null
  ```
  - 标点的跟随值是运行时状态，没有静态值，固定写「跟随全局（沿用当前状态）」。
  - 候选字体的存储从 `""` 改为 `null`（未设置）。这是 `schema_overrides` 层，不在 REGISTRY 里，按 R3 第一种写法用 `Option`；读取时 `""` 仍视同未设置，以兼容旧文件。

- [ ] **Step 1: 写失败用例。**

```rust
#[test]
fn follow_label_carries_value() {
    assert_eq!(follow_option_label("全局", layout_label("horizontal")), "跟随全局（横排）");
}
#[test]
fn font_unchecked_writes_null_not_empty_string() {
    assert_eq!(font_write(false, "楷体"), Value::Null);
    assert_eq!(font_write(true, ""), Value::Null);
    assert_eq!(font_write(true, "楷体"), json!("楷体"));
}
/// Review Focus 3：跟随标签来自本次打开时的 getConfig，不来自上次的种子。
#[test]
fn follow_labels_reseeded_on_each_open() { /* 两次 open_settings_at_id，mock 两次返回不同的 followedBehavior.layout，断言第二次下拉首项文案随之变化 */ }
```

  第三个用例的函数体参照 :5104 的写法：`ENV_LOCK` 加 `WIND_RPC_MOCK=1`，用 `mock_reply_with`（:428-435）按调用次数返回不同的值。

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，预期失败。**

- [ ] **Step 3: 实现。**
  - 下拉的选项列表改为 `Signal<Vec<String>>`，在 `open_settings_at_id` 里按 `followedBehavior` 重新计算首项。先确认 `Element::dropdown` 是否接受 signal 选项：manifest 的 `build_select_reactive`（manifest.rs:1276）有现成写法可以照抄。
  - 候选字体改为 `setting_row_override` 加文本框，`follow_text` 取 `followedBehavior.fontFamily`，为空时显示「系统字体」。

- [ ] **Step 4: 运行 `./scripts/dev.sh st`，预期通过。目视检查本方案行为节。**

- [ ] **Step 5: 提交（wind-setting）。** `feat(schema): 跟随档带值、候选字体改勾选行（R8.1/R8.2）`

---

### Task 7: 设置端 · 「全部恢复跟随」的清理范围（F8）

**Files:**
- Modify: `../wind-setting/src/dialogs/schema_codetable.rs`（`reset` :607-617）
- Test: 同一文件的 tests

**Interfaces:**
- Produces：`fn reset_scope_keys(visible: &[&str]) -> Vec<&'static str>`，返回 reset 实际会清的键；确认框文案函数 `fn reset_confirm_text(hidden_count: usize) -> Option<String>`，`hidden_count == 0` 时返回 None。

- [ ] **Step 1: 用测试钉住现状。** 断言 `reset` 清掉的键集合等于 `SPEC_BEHAVIOR_FIELDS` ∪ `frequency.*` ∪ `english_merge.*`，并且 `auto_phrase` 原样保留（:1115-1128 已有类似用例，可复用 fixture）。

```rust
#[test]
fn reset_touches_only_keys_visible_for_this_engine() {
    let visible = visible_keys(&manifest(), /* codetable */);
    for k in reset_scope_keys(&visible) {
        assert!(visible.contains(&k), "reset 清了界面上看不见的键 {k}（R8.5）");
    }
}
```

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，看这个用例是红还是绿。**
  - **绿**：说明被清的键全部可见，F8 按「无需改动」结案。保留这个用例作为护栏，并在设计文档 §4 的 F8 行写上「实测无隐藏键被清，已加护栏」。
  - **红**：有被清却不可见的键。把这些键从 reset 里移除，或者在「全部恢复跟随」的确认框里写明「另有 N 项界面未显示的设置也将恢复跟随」，然后再跑一次测试。

- [ ] **Step 3: 提交（wind-setting，必要时附带主仓设计文档一行）。** `test(schema): 全部恢复跟随只清可见键 —— R8.5 护栏`

---

### Task 8: 设置端 · 外观页「跟随主题（值）」（F4 / F5）

**Files:**
- Modify: `../wind-setting/src/state.rs`：`LoadedState::fetch`（:2754-2808）并行拉取 `theme.resolved`；`apply_loaded`（:2028）写入新 signal `theme_follow: Signal<Option<ThemeFollow>>`；切换主题后重新拉取（在切主题写回处调同一个刷新函数）
- Modify: `../wind-setting/src/manifest.rs`：`field_with`（:631-676）识别新的 manifest 属性 `follow = "theme"`，把行包成 `setting_row_override`
- Modify: `../wind-setting/src/assets/settings_manifest.toml`：`ui.candidate.font_size`（:2445）、`ui.font.family`（:2455）、`ui.font.weight`（:2466）加 `follow = "theme"`；翻页栏（:2724）、页码（:2739）的空值选项文案改成动态文案
- Modify: `../wind-setting/src/state.rs:2048-2058`：字体下拉首项「默认」改为不再出现（由勾选行表达跟随）
- Test: `manifest.rs` tests、`state.rs` tests

**Interfaces:**
- Consumes：`theme.resolved`（Task 2）
- Produces：
  ```rust
  pub struct ThemeFollow { pub font_size: i32, pub font_family: Option<String>, pub font_weight: i32,
                           pub pager_bar_display: String, pub page_number_display: String,
                           pub langbar_text: [Option<String>; 4] }
  fn theme_follow_text(key: &str, tf: &ThemeFollow) -> String  // "18" / "系统字体" / "常规 400"…
  fn sentinel_of(key: &str) -> Value   // font_size→0, font.family→"", font.weight→0
  fn seed_on_check(key: &str, tf: &ThemeFollow) -> Value // 勾选瞬间的初值 = 主题当前值
  ```

- [ ] **Step 1: 写失败用例。**

```rust
#[test]
fn follow_text_for_font_size_is_theme_value() {
    let tf = ThemeFollow { font_size: 18, ..mock_tf() };
    assert_eq!(theme_follow_text("ui.candidate.font_size", &tf), "18");
}
/// Review Focus 5：勾选瞬间写入主题当前值，不能写 0，否则会悄悄又变回跟随。
#[test]
fn checking_font_size_seeds_theme_value_not_zero() {
    let tf = ThemeFollow { font_size: 18, ..mock_tf() };
    assert_eq!(seed_on_check("ui.candidate.font_size", &tf), json!(18));
}
#[test]
fn unchecking_writes_r3_sentinel() {
    assert_eq!(sentinel_of("ui.candidate.font_size"), json!(0));
    assert_eq!(sentinel_of("ui.font.family"), json!(""));
}
#[test]
fn pager_empty_option_label_shows_theme_choice() {
    let tf = ThemeFollow { pager_bar_display: "hide".into(), ..mock_tf() };
    assert_eq!(pager_follow_label(&tf), "跟随主题（隐藏）");
}
#[test]
fn theme_follow_missing_shows_plain_follow() {
    // theme.resolved 返回 null（两级主题都坏）时不崩，文案退化为「跟随主题」
    assert_eq!(theme_follow_text_opt("ui.candidate.font_size", None), "跟随主题");
}
```

  - `mock_tf()` 在测试模块里构造一个默认的 `ThemeFollow`。
  - `pager_follow_label` 复用 manifest 里同一键 options 的 label 映射（hide→隐藏、auto→大于一页时显示、always→总是显示），不另写一份。

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，预期失败。**

- [ ] **Step 3: 实现。**
  - `field_with` 遇到 `follow = "theme"` 时：
    - `over` 取「当前值 ≠ sentinel」；
    - 勾选时写入 `seed_on_check`，取消勾选时写入 `sentinel_of`；
    - `follow_source` 固定为「跟随主题」（`Role::TextMuted`），`follow_text` 取 `theme_follow_text`。
  - 新增的 manifest 属性要在 manifest 解析的结构体里加字段，并在 `capabilities.rs:169 control_type_compatible` 旁边确认不影响类型兼容守门。
  - 翻页栏和页码的 `select_by_layout`：空值选项的 label 在渲染时用 `pager_follow_label` / `page_number_follow_label` 替换。
  - 字号 hint 去掉「0 = 跟随主题」这半句（勾选行已经表达了跟随）。

- [ ] **Step 4: 运行 `./scripts/dev.sh st`，预期通过。** 然后跑 `./scripts/dev.sh sg`，确认快照没有意外变化（manifest 改动不应影响 capabilities）。

- [ ] **Step 5: 目视外观页「候选字体」「候选排版」两张卡。** 在 mock 下检查三行勾选行都显示「跟随主题（18 / 系统字体 / 默认字重的实际名）」；字重为 0 时显示「主题未指定」。

- [ ] **Step 6: 提交（wind-setting）。** `feat(appearance): 字号/字体/字重改勾选行、翻页栏页码跟随档带主题值（R8.1/R8.3）`

---

### Task 9: 设置端 · 语言栏主字与角标颜色（F6 / F7 设置端）

**Files:**
- Modify: `../wind-setting/src/dialogs/langbar_text_colors_dialog.rs`：`cell_text`（:58）在 `FollowTheme` 时带上主题色值；`fallback_color`（:28）优先取 `theme_follow.langbar_text[i]`
- Modify: `../wind-setting/src/dialogs/langbar_badges_dialog.rs`：删除 `AUTO_COLOR`（:67）和按位长判断的逻辑（`badge_value_of` :492）；颜色格与透明度格分开，透明度用勾选行（未勾 = 跟随全局 `badge_alpha`）
- Test: 两个文件各自的 tests

**Interfaces:**
- Consumes：`ThemeFollow.langbar_text`（Task 8）；`LangBarBadge.alpha_light/alpha_dark`（Task 4；设置端经 `config.get` 得到 JSON）
- Produces：`fn badge_alpha_write(over: bool, v: f32) -> Value`（未勾为 `Value::Null`）

- [ ] **Step 1: 写失败用例。**

```rust
#[test]
fn follow_theme_cell_text_carries_theme_color() {
    assert_eq!(cell_text_with("", Some("#000000")), "跟随主题（#000000）");
    assert_eq!(cell_text_with("", None), "跟随主题");
}
#[test]
fn badge_color_follow_is_empty_string() {
    assert_eq!(badge_color_value(/* 与主字同色 */ true, "#112233"), json!(""));
}
#[test]
fn badge_alpha_unchecked_writes_null() {
    assert_eq!(badge_alpha_write(false, 0.5), Value::Null);
    assert_eq!(badge_alpha_write(true, 0.5), json!(0.5));
}
```

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，预期失败。**
- [ ] **Step 3: 实现。**
  - 按钮「与主字同色」保留，写入 `""`，作为颜色这一维的「跟随」；
  - 按钮「跟随全局透明度」删掉，改成透明度勾选行：行里放滑块或数字框，`follow_text` 为 `跟随全局（{badge_alpha}）`。
- [ ] **Step 4: 运行 `./scripts/dev.sh st`，预期通过；目视两个对话框。**
- [ ] **Step 5: 提交（wind-setting）。** `feat(langbar): 主字色跟随档带主题色、角标透明度独立勾选行（R8.1/R8.4）`

---

### Task 10: 设置端 · 全局页「已被 N 个方案覆盖」（F9）

**Files:**
- Modify: `../wind-setting/src/state.rs`：`LoadedState::fetch` 并行拉取 `schema.overrideSummary`，`apply_loaded` 写入 `override_summary: Signal<HashMap<String, Vec<String>>>`；方案对话框保存后刷新一次（在 `SideCommitter` 提交回调处）
- Modify: `../wind-setting/src/widgets.rs`：`setting_row_schema_hint`（:341）/ `schema_hint_badge`（:301）改为接受 `Signal<String>` 提示，走 `info_badge(tip: Signal<String>, on_click)`（:188-268）
- Modify: `../wind-setting/src/manifest.rs`：`field_with`（:647-668）传入该键的动态文案；`schema_hint_header_note`（:2224）的顶部说明同样加上已覆盖计数
- Test: `manifest.rs` tests

**Interfaces:**
- Consumes：`schema.overrideSummary`（Task 3）
- Produces：`fn override_badge_text(static_note: &str, ids: &[String], names: &HashMap<String, String>) -> String`
  - `ids` 为空时返回 `static_note`（维持现状）；
  - 否则返回 `"已被 {n} 个方案覆盖：{名1}、{名2}"`，最多列 3 个名字，超出部分写「等」。

- [ ] **Step 1: 写失败用例。**

```rust
#[test]
fn badge_text_keeps_static_note_when_not_overridden() {
    assert_eq!(override_badge_text("可被方案覆盖", &[], &HashMap::new()), "可被方案覆盖");
}
#[test]
fn badge_text_lists_overriding_schemas() {
    let names = HashMap::from([("wubi86".to_string(), "五笔86".to_string())]);
    assert_eq!(override_badge_text("可被方案覆盖", &["wubi86".into()], &names),
               "已被 1 个方案覆盖：五笔86");
}
#[test]
fn badge_text_truncates_after_three() {
    let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
    let names: HashMap<String, String> = ids.iter().map(|i| (i.clone(), i.to_uppercase())).collect();
    assert_eq!(override_badge_text("可被方案覆盖", &ids, &names), "已被 4 个方案覆盖：A、B、C 等");
}
```

- [ ] **Step 2: 运行 `./scripts/dev.sh st`，预期失败。**
- [ ] **Step 3: 实现。**
  - 页面树是在数据加载完成之前构建的，所以文案必须走 signal，不能取快照；做法照 `stats_mgr.schema_names`（state.rs:2039）。
  - 点击角标时调用 `schema_mgr.open_settings_at_id(ids[0])`。
  - 注意 widgets.rs:282-285 关于命中测试的警告。
- [ ] **Step 4: 运行 `./scripts/dev.sh st`，预期通过；在 mock 下目视全局页的「候选排列」。**
- [ ] **Step 5: 提交（wind-setting）。** `feat(settings): 全局页角标反查「已被 N 个方案覆盖」（R8.6）`

---

### Task 11: 文案清扫与整体验证

**Files:**
- Modify: 按 grep 结果定位，只改「表示跟随」的「默认」
- Modify: `docs/design/override-presentation.md`：状态行改为「已实施」，F1–F9 每行补上提交号

- [ ] **Step 1: 扫描。**
  `cd ../wind-setting && grep -n '默认（主题\|label = "默认"\|"默认"' src/assets/settings_manifest.toml src/state.rs src/dialogs/*.rs`
  逐条判断语义，对照调研清单：
  - 以下几处**不要改**：`默认（水平）`（出厂行为）；`（默认：全拼）` / `（自动）`（有测试钉住）；hint 里「默认 72 小时」一类（指内置值）。
  - 字重下拉里的 `{value="0",label="默认"}` 在 Task 8 之后应已不再显示（跟随改由勾选行表达）。如果仍然作为选项存在，就删掉这一项。

- [ ] **Step 2: 全量测试。**
  - 主仓：`cd wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt TMPDIR=/tmp/wct-ovr cargo test -p wind-config -p wind-coordinator -p wind-webdata -p wind-rpc`。与开工前的通过数对比，失败数必须为 0。
  - 设置端：`./scripts/dev.sh st && ./scripts/dev.sh sk`。
  - 两个仓都跑 clippy：主仓 `cargo clippy -p wind-config -p wind-coordinator -p wind-webdata -- -D warnings`。

- [ ] **Step 3: 靶机验证的判据（交给人工）。** 靶机没有 GUI 自动化，需要人工按以下几条验证：
  1. 外观页字号切到「自定义」时，数字框显示主题字号而不是 0；
  2. 在一个码表方案里自定义候选排列后，全局页「候选排列」出现「已被 1 个方案覆盖」；
  3. 旧配置里的 8 位角标色升级后，颜色和透明度都与升级前一致。

- [ ] **Step 4: 回填设计文档并提交（主仓）。** `docs(design): 覆盖的呈现规范 —— 标记实施完成并回填提交号`
