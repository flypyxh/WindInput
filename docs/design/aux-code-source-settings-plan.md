# 辅助码来源 · 第 3 期（设置端）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 方案设置的辅助码卡片加一行「辅助码来源」：不勾显示「跟随方案（笔画）」，勾上用下拉在「输入方案」与「辅助码表」间选一个，写 `[engine.aux_code].files`。

**Architecture:** 核心 `schema.getConfig` 补一个只读旁路字段 `auxCodeBaseFiles`（方案文件自己声明的 `files`，不含 override），设置端据此区分「跟随方案」与「用户选的」；可选项来自第 1 期的 `schema.auxCodeSources`。设置端的判定与落盘写成纯函数（可测），界面行照卡片上既有两行的 R8.1 勾选行形态。

**Tech Stack:** Rust；核心 workspace `wind_input/`；设置端 wind-setting（windui，经 xwin + wine 跑测试）。

**Spec:** `docs/design/aux-code-schema-source.md` §6；R8 呈现规范 `docs/architecture/config-design-rules.md`。

**工作位置：**
- 核心：worktree `/home/dufeng/develop/windinput/wt-auxsrc/WindInput`，分支 `feat/aux-code-source`。
- 设置端：worktree `/home/dufeng/develop/windinput/wt-auxsrc/wind-setting`，分支 `feat/aux-code-source`（`Cargo.toml` 的 `../WindInput` 正好指向上面那个核心 worktree；`../wind-ui-rust` 是软链到主 checkout）。
- harness 每条命令后 cwd 会复位——**每条 Bash 都显式 `cd`**。

## Global Constraints

- 文案只用「跟随方案」（R8.3）；跟随档必须带值：`跟随方案（笔画）`；多来源用「+」连接；声明的来源不存在加「（未安装）」。
- 行形态：R8.1 勾选行（`widgets::setting_row_override` + `widgets::override_control`），放在「本方案启用辅助码」之后、「辅助码最长词长」之前。
- 落盘：不勾 → `files` 写 `null`（删键，回落方案文件）；勾上选中一项 → `files = ["schema:<id>"]` 或 `files = ["<path>"]`。与打开时的落盘结果相同就不写（同 `aux_write` 那段的做法）。
- 首版只选一个来源；override 里已是多条或含下拉里没有的条目 → 勾选态、只读显示各条名字、提示「在方案文件里编辑」，保存时原样不动。
- 「全部恢复跟随」一并清这一行；卡片摘要「已自定义 N 项」计入它。
- 下拉选项：方案在前、码表在后；每项右侧徽章区分「输入方案」/「辅助码表」。
- 核心新增的旁路字段必须登记进 `READONLY_SIDECAR_FIELDS`（否则会随 saveConfig 落进 override）。
- 核心测试：`cd <核心 worktree>/wind_input && mkdir -p /tmp/wct-auxsrc && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-webdata <filter>`。
- 设置端测试：`cd /home/dufeng/develop/windinput/wt-auxsrc/wind-setting && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-setting-auxsrc ./scripts/dev.sh st`（全量，经 wine）；检查 `... ./scripts/dev.sh sk`。
- 格式化只对改动文件 `rustfmt --edition 2024`；禁 `cargo fmt --all`。
- 提交：`git commit -F - --only -- <路径>`（新文件先 `git add -- <文件>`）；禁 `git add -A` / `.` / `commit -a` / `stash` / `reset --hard`。两个 worktree 各有一处与本任务无关的未提交 `Cargo.lock` 改动，**不要提交它**。
- 注释中文，风格对齐所在文件。

## Review Focus

1. 方案文件没写 `files`、override 却写了（用户手配）——判为「已自定义」，跟随档显示「跟随方案（无）」（Task 2 测试）。
2. override 里的来源恰好等于方案基线（手写了一份相同的）——`json_diff` 会把它当无差异删掉，界面应判为跟随（Task 2 测试）。
3. `schema.auxCodeSources` 调用失败 / 返回空 —— 行仍渲染，下拉只有当前值时不崩；跟随档名字退回原始条目文本（Task 2 测试）。
4. 打开后什么都不改就关 —— 不写 `files`（不产生脏 override）（Task 2 测试）。
5. 多条来源的 override —— 只读展示、保存不改写（Task 2 测试）。

---

### Task 1: 核心 `getConfig` 补 `auxCodeBaseFiles` 旁路字段

**Files:**
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（`web_schema_get_config` 里 `followedBehavior` 插入处附近；`READONLY_SIDECAR_FIELDS`；测试模块）

**Interfaces:**
- Produces: `schema.getConfig` 结果顶层 `"auxCodeBaseFiles": [String]`——方案文件（不含 override 层）的 `[engine.aux_code].files`，没写则为 `[]`。

- [ ] **Step 1: 失败测试**（wind-webdata 测试模块，照 `schema_overlay_section_roundtrips_through_save_config` 的夹具写法）

```rust
    /// 设置页区分「跟随方案」与「用户选的来源」要看方案文件自己的 files——合并值答不了。
    #[test]
    fn get_config_exposes_aux_code_base_files_and_strips_it_on_save() {
        let dir = std::env::temp_dir().join(format!("wind_webdata_aux_base_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(&schemas).unwrap();
        std::fs::write(
            schemas.join("zz_py.schema.toml"),
            "[schema]\nid = \"zz_py\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n\
             [engine.aux_code]\nfiles = [\"aux_code/stroke.txt\"]\n",
        )
        .unwrap();
        let ov = dir.join("overrides");
        std::fs::create_dir_all(&ov).unwrap();
        std::fs::write(ov.join("zz_py.toml"), "[engine.aux_code]\nfiles = [\"schema:wubi86\"]\n").unwrap();
        let store = std::sync::Arc::new(wind_store::Store::open(&dir.join("s.redb")).unwrap());
        let c = Coordinator::new_headless_with_store_override(
            wind_config::Config::default(), Some(&dir), store, Some(ov.clone()),
        );
        let v = c.web_data_rpc("schema.getConfig", &json!({ "id": "zz_py" })).unwrap();
        assert_eq!(v["auxCodeBaseFiles"], json!(["aux_code/stroke.txt"]), "方案文件自己的 files");
        assert_eq!(v["engine"]["aux_code"]["files"], json!(["schema:wubi86"]), "合并值是 override 的");
        // 原样回传不能把旁路字段写进 override。
        c.web_data_rpc("schema.saveConfig", &json!({ "id": "zz_py", "cfg": v })).unwrap();
        let saved = std::fs::read_to_string(ov.join("zz_py.toml")).unwrap();
        assert!(!saved.contains("auxCodeBaseFiles"), "旁路字段必须剥掉：{saved}");
        assert!(saved.contains("schema:wubi86"), "用户的选择保留：{saved}");
    }
```

- [ ] **Step 2: 跑，确认失败**（`auxCodeBaseFiles` 为 null）

`cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && mkdir -p /tmp/wct-auxsrc && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-webdata get_config_exposes_aux_code_base_files`

- [ ] **Step 3: 实现**

在插入 `followedBehavior` 的同一个 `if let Some(obj)` 块里加：

```rust
                    // 方案文件**自己**声明的辅助码来源（不含 override 层）。设置页据此区分
                    // 「跟随方案」与「用户选的」：`engine.aux_code.files` 是合并值，两种来源
                    // 给出的都是一个数组，答不了「这是谁写的」。
                    let base_files = self
                        .engine_mgr()
                        .schema_base(id)
                        .map(|s| s.engine.aux_code.files)
                        .unwrap_or_default();
                    obj.insert("auxCodeBaseFiles".to_string(), json!(base_files));
```

（`schema_base` 的返回类型以实际为准——`web_schema_save_config` 里有现成用法。）`READONLY_SIDECAR_FIELDS` 末尾加：

```rust
    // 方案文件自己的辅助码来源（设置页判「跟随方案」用）。不剥的话一份基线快照会落进
    // override，从此方案作者改了推荐码表也透不过来。
    "auxCodeBaseFiles",
```

- [ ] **Step 4: 跑该测试与 `cargo test -p wind-webdata` 全量，确认通过**

- [ ] **Step 5: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-webdata/src/lib.rs
git commit -F - --only -- crates/wind-webdata/src/lib.rs <<'EOF'
feat(webdata): schema.getConfig 附带 auxCodeBaseFiles —— 设置页区分「跟随方案」与用户选的辅助码来源

engine.aux_code.files 是方案文件 ⊕ override 的合并值，两种来源都是一个数组，设置页判不出
这一项是不是用户改的。旁路字段只读，登记进 READONLY_SIDECAR_FIELDS，saveConfig 时剥掉。
EOF
```

---

### Task 2: 设置端数据层 —— mock、可选项解析、判定与落盘纯函数、状态接线

**Files:**
- Modify: `src/rpc.rs`（mock：`schema.getConfig` 拼音族分支加 `auxCodeBaseFiles`；新增 `"schema.auxCodeSources"` 分支）
- Modify: `src/dialogs/schema_manager.rs`（纯函数与 `SchemaManagerState` 信号、打开时 seed、保存、`aux_reset_all`、卡片摘要计数；测试）

**Interfaces:**
- Consumes: 核心 `schema.getConfig` 的 `auxCodeBaseFiles`、`engine.aux_code.files`；`schema.auxCodeSources`（参数 `{ "id": 方案 id }`，返回 `{ "schemas": [{id,name}], "files": [{path,label}] }`）。
- Produces（Task 3 用）：
  - `struct AuxSourceOpts { items: Vec<AuxSourceItem> }`，`struct AuxSourceItem { entry: String, label: String, is_schema: bool }`（`entry` 即写进 `files` 的原文：`"schema:<id>"` 或路径）；`fn parse_aux_source_opts(v: &Value) -> AuxSourceOpts`；`fn label_of(&self, entry: &str) -> Option<&str>`。
  - `fn aux_src_follow_text(base: &[String], opts: &AuxSourceOpts) -> String`（不含「跟随方案」前缀，即括号里那部分：`笔画` / `五笔+小鹤` / `笔画（未安装）` / `无`）。
  - `enum AuxSrcSeed { Follow, Pick(usize), Manual(Vec<String>) }` 与 `fn seed_aux_src(merged: &[String], base: &[String], opts: &AuxSourceOpts) -> AuxSrcSeed`。
  - `fn aux_src_write(over: bool, pick: Option<&str>) -> Value`（`over=false` → `Null`；`over=true` → `json!([pick])`）。
  - 信号：`settings_aux_src_over: Signal<bool>`、`settings_aux_src_idx: Signal<usize>`、`settings_aux_src_manual: Signal<bool>`（多条/未知条目，只读态）；`settings_aux_src_opts: Rc<RefCell<AuxSourceOpts>>`；`settings_aux_src_follow: Signal<String>`（跟随档文字，已含括号内容）；打开时基线 `settings_aux_src_at_open: Rc<Cell<(bool, usize, bool)>>`。

- [ ] **Step 1: mock**

`rpc.rs` 拼音族分支：`"engine"` 同级加 `"auxCodeBaseFiles": ["aux_code/bihua.txt"]`（与已有 `aux_code.files` 相同 ⇒ 默认呈现为跟随）。新增：

```rust
        // 辅助码来源的可选项（对齐核心 `schema.auxCodeSources`）：码表方案 + aux_code/*.txt。
        // 名字给中文，下拉与跟随档文字在 `WIND_RPC_MOCK=1` 下才有得看。
        "schema.auxCodeSources" => json!({
            "schemas": [
                { "id": "wubi86", "name": "五笔 86" },
                { "id": "stroke", "name": "笔画" }
            ],
            "files": [
                { "path": "aux_code/bihua.txt", "label": "笔画" },
                { "path": "aux_code/flypy_full.txt", "label": "小鹤" },
                { "path": "aux_code/ZRM-wanxiang.txt", "label": "自然码" }
            ]
        }),
```

- [ ] **Step 2: 纯函数的失败测试**（`schema_manager.rs` 测试模块）

```rust
    fn opts() -> AuxSourceOpts {
        parse_aux_source_opts(&json!({
            "schemas": [{ "id": "wubi86", "name": "五笔 86" }],
            "files": [{ "path": "aux_code/stroke.txt", "label": "笔画" },
                      { "path": "aux_code/flypy_full.txt", "label": "小鹤" }]
        }))
    }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn aux_source_items_put_schemas_first() {
        let o = opts();
        let e: Vec<&str> = o.items.iter().map(|i| i.entry.as_str()).collect();
        assert_eq!(e, vec!["schema:wubi86", "aux_code/stroke.txt", "aux_code/flypy_full.txt"]);
        assert!(o.items[0].is_schema && !o.items[1].is_schema);
        assert_eq!(o.label_of("schema:wubi86"), Some("五笔 86"));
    }

    #[test]
    fn follow_text_names_base_sources() {
        let o = opts();
        assert_eq!(aux_src_follow_text(&s(&["aux_code/stroke.txt"]), &o), "笔画");
        assert_eq!(aux_src_follow_text(&s(&["schema:wubi86", "aux_code/flypy_full.txt"]), &o), "五笔 86+小鹤");
        assert_eq!(aux_src_follow_text(&s(&["aux_code/gone.txt"]), &o), "aux_code/gone.txt（未安装）");
        assert_eq!(aux_src_follow_text(&[], &o), "无");
        // 可选项拿不到（RPC 失败）时退回原文，不崩、不报未安装。
        assert_eq!(aux_src_follow_text(&s(&["aux_code/stroke.txt"]), &AuxSourceOpts::default()), "aux_code/stroke.txt");
    }

    #[test]
    fn seed_distinguishes_follow_pick_manual() {
        let o = opts();
        let base = s(&["aux_code/stroke.txt"]);
        assert!(matches!(seed_aux_src(&base, &base, &o), AuxSrcSeed::Follow), "合并值 = 基线 ⇒ 跟随");
        assert!(matches!(seed_aux_src(&s(&["schema:wubi86"]), &base, &o), AuxSrcSeed::Pick(0)));
        assert!(matches!(seed_aux_src(&s(&["aux_code/flypy_full.txt"]), &[], &o), AuxSrcSeed::Pick(2)), "方案没写、用户配了");
        assert!(matches!(seed_aux_src(&s(&["schema:wubi86", "aux_code/stroke.txt"]), &base, &o), AuxSrcSeed::Manual(_)), "多条 ⇒ 只读");
        assert!(matches!(seed_aux_src(&s(&["schema:nope"]), &base, &o), AuxSrcSeed::Manual(_)), "下拉里没有 ⇒ 只读");
    }

    #[test]
    fn aux_src_write_null_when_following() {
        assert_eq!(aux_src_write(false, Some("schema:wubi86")), Value::Null);
        assert_eq!(aux_src_write(true, Some("schema:wubi86")), json!(["schema:wubi86"]));
    }
```

运行 `cd /home/dufeng/develop/windinput/wt-auxsrc/wind-setting && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-setting-auxsrc ./scripts/dev.sh st`，确认编译失败（函数不存在）。

- [ ] **Step 3: 实现纯函数**

```rust
/// 「辅助码来源」下拉的一项。`entry` 是写进 `[engine.aux_code].files` 的原文。
#[derive(Debug, Clone, Default)]
struct AuxSourceItem {
    entry: String,
    label: String,
    is_schema: bool,
}

/// `schema.auxCodeSources` 的结果：方案在前、码表在后（下拉即按此顺序）。
#[derive(Debug, Clone, Default)]
struct AuxSourceOpts {
    items: Vec<AuxSourceItem>,
}

impl AuxSourceOpts {
    fn label_of(&self, entry: &str) -> Option<&str> {
        self.items.iter().find(|i| i.entry == entry).map(|i| i.label.as_str())
    }
    fn index_of(&self, entry: &str) -> Option<usize> {
        self.items.iter().position(|i| i.entry == entry)
    }
}

fn parse_aux_source_opts(v: &Value) -> AuxSourceOpts {
    let arr = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut items: Vec<AuxSourceItem> = arr("schemas")
        .iter()
        .filter(|x| !s(x, "id").is_empty())
        .map(|x| AuxSourceItem { entry: format!("schema:{}", s(x, "id")), label: s(x, "name"), is_schema: true })
        .collect();
    items.extend(arr("files").iter().filter(|x| !s(x, "path").is_empty()).map(|x| AuxSourceItem {
        entry: s(x, "path"),
        label: s(x, "label"),
        is_schema: false,
    }));
    AuxSourceOpts { items }
}

/// 跟随档括号里的文字：方案文件声明的来源名，多个用「+」连接；可选项里没有的标「（未安装）」。
/// 可选项整个拿不到（RPC 失败 / 空）时退回原文——那时判不了装没装，不能乱报缺失。
fn aux_src_follow_text(base: &[String], opts: &AuxSourceOpts) -> String {
    if base.is_empty() {
        return "无".to_string();
    }
    base.iter()
        .map(|e| match opts.label_of(e) {
            Some(l) if !l.is_empty() => l.to_string(),
            Some(_) => e.clone(),
            None if opts.items.is_empty() => e.clone(),
            None => format!("{e}（未安装）"),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 打开时这一行的状态。判据只看「合并值是否等于方案文件自己的值」——相等就是跟随
/// （即便 override 里手写了一份相同的，core 的 `json_diff` 保存时也会把它当无差异删掉）。
#[derive(Debug)]
enum AuxSrcSeed {
    Follow,
    Pick(usize),
    /// 多条或下拉里没有的条目：只读展示，保存原样不动。
    Manual(Vec<String>),
}

fn seed_aux_src(merged: &[String], base: &[String], opts: &AuxSourceOpts) -> AuxSrcSeed {
    if merged == base {
        return AuxSrcSeed::Follow;
    }
    match merged {
        [one] => opts.index_of(one).map_or_else(|| AuxSrcSeed::Manual(merged.to_vec()), AuxSrcSeed::Pick),
        _ => AuxSrcSeed::Manual(merged.to_vec()),
    }
}

/// 这一行的落盘值：不勾写 `null`（删键 ⇒ 回落方案文件），勾上写单元素数组。
fn aux_src_write(over: bool, pick: Option<&str>) -> Value {
    match (over, pick) {
        (true, Some(e)) => json!([e]),
        _ => Value::Null,
    }
}
```

- [ ] **Step 4: 状态接线**
  1. `SchemaManagerState` 加 Interfaces 里列的信号与字段，并在构造处初始化（`idx` 0、`over`/`manual` false、`opts` 默认、`follow` 空串、`at_open` `(false, 0, false)`）。
  2. 打开时 seed（`has_aux_code_table` 分支内、现有两行 seed 之后）：`merged` 取 `/engine/aux_code/files`、`base` 取 `/auxCodeBaseFiles`（缺失按 `merged` 处理 ⇒ 跟随，兼容老核心）；`opts` 调 `rpc::call_core("schema.auxCodeSources", &json!({ "id": <当前方案 id> }))`，失败给默认；按 `seed_aux_src` 设 `over` / `idx` / `manual`；`follow` 设为 `format!("跟随方案（{}）", aux_src_follow_text(&base, &opts))` 里括号部分——**信号里只存括号内容**，前缀由行的 `follow_source` 给；`at_open` 记 `(over, idx, manual)`；摘要计数加上 `over as usize`。
  3. 保存（现有 `if has_aux_code_table(&cfg)` 块内）：`manual` 时跳过本行；否则 `now = aux_src_write(over, opts.items.get(idx).map(|i| i.entry.as_str()))` 与 `before`（用 `at_open` 同样算）不同时 `set_json_path(&mut cfg, &["engine", "aux_code", "files"], now)`，并置 `cfg_changed = true`。
  4. `aux_reset_all`：再清 `settings_aux_src_over`、`settings_aux_src_manual`（manual 态被「全部恢复跟随」清掉 ⇒ 保存时写 `null`——此时保存逻辑需要：`manual` 由 true 变 false 且 `over` 为 false ⇒ 写 `null`；实现时让 manual 分支只在「manual 仍为 true」时跳过）。
  5. 卡片摘要（`aux_custom` 计数与其 `version()` 依赖）加上 `settings_aux_src_over`。

- [ ] **Step 5: 接线测试**（沿用 `aux_reset_all_clears_both_rows_writes_null` 的写法，用 `SchemaManagerState` 真信号）

```rust
    /// 「全部恢复跟随」连来源这一行一起清，落盘回到 null；多条的只读态也被清掉。
    #[test]
    fn aux_reset_all_also_clears_source_row() {
        let state = SchemaManagerState::new_for_test(); // 以实际的测试构造方式为准
        state.settings_aux_src_over.set(true);
        state.settings_aux_src_manual.set(true);
        state.settings_aux_src_idx.set(1);
        aux_reset_all(&state, true, 4);
        assert!(!state.settings_aux_src_over.get());
        assert!(!state.settings_aux_src_manual.get());
        assert_eq!(aux_src_write(state.settings_aux_src_over.get(), Some("x")), Value::Null);
    }
```

另加一条走 mock 的端到端：打开拼音族方案（mock 下 `auxCodeBaseFiles == files`）⇒ `settings_aux_src_over` 为 false、跟随文字为「笔画」；不做任何修改走一遍保存逻辑 ⇒ `cfg` 里 `engine.aux_code.files` 保持原值、不写 null（打开即关不脏）。（照现有 mock 端到端测试的构造方式写；若该对话框的 seed/save 需要实际 UI 才能驱动，改为直接对 seed/save 抽出的函数测，并在报告里说明。）

- [ ] **Step 6: 跑设置端全量** `CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-setting-auxsrc ./scripts/dev.sh st`，0 失败。

- [ ] **Step 7: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/wind-setting
rustfmt --edition 2024 src/rpc.rs src/dialogs/schema_manager.rs
git commit -F - --only -- src/rpc.rs src/dialogs/schema_manager.rs <<'EOF'
feat(schema): 辅助码来源的判定与落盘 —— 跟随方案 / 选一个来源 / 多条只读，接 schema.auxCodeSources

「跟随」按合并值是否等于方案文件自己的值判（core 旁路字段 auxCodeBaseFiles），相等即跟随，
与 json_diff 保存时的口径一致。多条或下拉里没有的条目只读展示、保存原样不动。
EOF
```

---

### Task 3: 设置端界面 —— 「辅助码来源」勾选行 + 检索项 + 目视验证

**Files:**
- Modify: `src/dialogs/schema_manager.rs`（`SettingsSection::AuxCode` 渲染）
- Modify: `src/search/index.rs`（`SchemaLevelItem` 加一项）

**Interfaces:**
- Consumes: Task 2 的信号、`AuxSourceOpts`、跟随文字信号。

- [ ] **Step 1: 渲染行**（`row_on` 之后、`row_len` 之前）

```rust
            // 辅助码来源：R8.1 勾选行。不勾显示方案文件声明的来源（跟随方案），勾上选一个。
            // 下拉每项带徽章区分「输入方案 / 辅助码表」——同名的（如笔画方案与笔画码表）
            // 不看徽章分不出来。
            let src_over = state.settings_aux_src_over;
            let src_manual = state.settings_aux_src_manual;
            let items: Vec<windui::ui::select::DropdownItem> = state
                .settings_aux_src_opts
                .borrow()
                .items
                .iter()
                .map(|i| {
                    windui::ui::select::DropdownItem::new(&i.label)
                        .badge(if i.is_schema { "输入方案" } else { "辅助码表" }, Intent::Neutral)
                })
                .collect();
            let editor = Element::dropdown_items(items, state.settings_aux_src_idx).width(AUX_SRC_W);
            let row_src = crate::widgets::setting_row_override(
                src_over,
                "辅助码来源",
                Some("用哪张表的字形码筛选：一个输入方案（取它的系统词库 + 你加的词），或一张辅助码表"),
                "跟随方案",
                Role::TextMuted,
                crate::widgets::override_control_signal(
                    src_over,
                    Rc::new(move || !src_manual.get()),
                    editor,
                    state.settings_aux_src_follow,
                    "多个来源，请在方案文件里编辑",
                    AUX_SRC_W,
                ),
            );
```

（`DropdownItem` / `Intent` 的实际路径、`badge` 取哪个 Intent 以 windui 现有用法为准；`AUX_SRC_W` 新常量，宽度要放得下最长的「五笔 86 + 徽章」，先取 220 目视再定。manual 态下 `gate` 为 false，`override_control` 显示 `reason` 文案——若它的语义不是这样，改为 manual 时用一个只读 label 显示各条名字 + 「在方案文件里编辑」，并在报告里说明。）`override_group_card` 的行向量改为 `vec![row_on, row_src, row_len]`。

- [ ] **Step 2: 检索项**：`src/search/index.rs` 辅助码那两行之后加
  `SchemaLevelItem { title: "辅助码来源", tab: "方案自定义", section: "辅助码", keywords: "辅码 码表 笔画 五笔 小鹤 自然码 跟随方案" },`
  若拼音检索表测试因生字变红，按该测试的提示补表（记忆：本机无 pwsh，用等价实现重算）。

- [ ] **Step 3: 全量测试** `./scripts/dev.sh st`（带上面的 CARGO_TARGET_DIR），0 失败。

- [ ] **Step 4: 目视**：在 mock 下截这一屏两张图（跟随态、勾选态）：
  `cd /home/dufeng/develop/windinput/wt-auxsrc/wind-setting && WIND_RPC_MOCK=1 CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-setting-auxsrc cargo xwin run --target x86_64-pc-windows-msvc -- <打开方案设置对话框到拼音族方案「方案自定义」页的参数> --screenshot /tmp/wct-auxsrc/aux-src-follow.png`
  （参数以 `src/main.rs` 的截图 / 直达参数为准；若该对话框没有直达参数，报告里说明并改为给出人工验证判据。）用 Read 看图，确认：行在两行之间、跟随文字「跟随方案（笔画）」完整不截断、三行右侧控件左边界对齐。

- [ ] **Step 5: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/wind-setting
rustfmt --edition 2024 src/dialogs/schema_manager.rs src/search/index.rs
git commit -F - --only -- src/dialogs/schema_manager.rs src/search/index.rs <<'EOF'
feat(schema): 辅助码卡片加「辅助码来源」一行 —— 跟随方案或选一个输入方案 / 辅助码表

R8.1 勾选行：不勾显示「跟随方案（笔画）」，勾上用下拉选，每项徽章区分输入方案与辅助码表。
多条来源只读，提示去方案文件里编辑。「全部恢复跟随」与卡片摘要一并计入。
EOF
```
