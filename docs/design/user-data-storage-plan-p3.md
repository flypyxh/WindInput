# 用户数据存储 P3 实施计划：短语与 Shadow 文本化

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 全局用户短语（redb `PHRASES`）与按方案的 Shadow 规则（redb `SHADOW`）的真源换成 `user/phrases.dict.yaml` 与 `user/<schema>/shadow.dict.yaml`。`Store` 方法签名不变；按键路径上的 Shadow 查询改成读内存表，不再碰 redb。旧表一次性迁移，迁完 `format_version` → 4。

**Architecture:** 两张表都小（短语约百行，Shadow 是用户逐条操作攒的），不走 P0–P2 的日志 + delta 引擎，而是 §2.4 的「小表」口径：内存持有解析结果（`Arc` 快照，读零 IO），写入时按行补丁、整文件提交（tmp + fsync + 复核指纹 + rename），未改动的行逐字节不变；手改文件在 reload / 切方案 / 配置重载时按指纹重新解析。通用部分做成 `wind-userdata::text::table`（按列名读写的 TSV 表 + 带指纹的持有者）。短语改为**叠加模型**：系统短语照旧来自 `system.phrases.toml`（只读、随版本更新），用户文件只记用户行与「对系统条目的修改」行（`overrides_system: 1`），读时叠加。

**Tech Stack:** Rust（edition 2024），`wind-userdata`（text 模块）、`wind-store`、`wind-coordinator`、`wind-webdata`、`wind-transfer`、`wind-config`。

**Spec:** `docs/design/user-data-storage-redesign.md`（§2.2 短语 / Shadow 文件格式、§2.4、§6、§7.2、§10 P3 行）

## Global Constraints

- 短语文件 `<userdata>/user/phrases.dict.yaml`：`columns: [code, text, weight, position, enabled, category, overrides_system]`，`wind: {kind: phrases, escape: true}`；后四列可省略，省略取默认（§2.2）。
- Shadow 文件 `<userdata>/user/<schema>/shadow.dict.yaml`：`columns: [code, text, action, position, cand_id]`，`action ∈ {pin, delete}`，`cand_id` 可空（§2.2）。
- 小表改一次整文件 tmp + fsync + rename；不走引擎、不进日志（§2.4）。
- 迁移：没有完成标记就整体重做；迁移成功后旧表只读保留，`format_version` 升一级（§7.2）。
- 写回保持原 BOM 与换行风格；非 UTF-8 / 头部坏的文件隔离：不往里写、按已有内容提供查询、WARN（§2.3）。
- 提交 `git commit -F - --only -- <路径…>`；`cargo fmt -p <crate>`；测试带 `CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录>`，全量 `--no-fail-fast`。
- INFO 日志不带词条内容。

## 计划内裁定（执行前已定）

1. **小表不走引擎**：短语 / Shadow 写入是用户逐条的显式动作（每次几毫秒的整文件提交可接受），读取在按键路径上要零 IO；日志 + delta 那套为高频写设计，用在这里只增加复杂度。代价：两张表没有「多设备 Incr 合并」能力，将来同步（P5）按整文件 LWW 处理。
2. **短语叠加模型**（设计 §2.2 的 `overrides_system` 语义落地）：
   - 文件里 `overrides_system = 1` 的行 = 对同键系统条目的修改（归属仍是系统：`is_system = true`）；
   - `overrides_system = 0` 而键与系统条目相同 = 用户行遮蔽系统条目（`is_system = false, overrides_system = true`，同现状）；
   - 系统条目没被任何行覆盖时原样呈现；
   - 系统 TOML 更新（哈希变）时，修改行的 weight / position / category 回到 TOML 值、`enabled` 保留（同现状「同步覆盖系统行上的编辑、保留启用状态」）；TOML 里已删的键，其修改行一并删除。
3. **系统短语改键**（`update_phrase` 带 new_code / new_text 作用在系统条目上）：原键写一行修改行 `enabled = 0`，新键作为用户行写入。归属从系统变为用户。代价：与现状「改键后仍归系统」不同；系统短语改键在设置页罕见，且语义上新键本就不在 TOML 里。
4. **Shadow 的 `position` 是目标槽位**（现状语义，设计 §2.2 的「队首次序」写法不准确）；同一编码下多条 pin 的新旧先后由**行序**表达（文件里先出现的更新），与 `ShadowRecord.pinned` 的 newest-first 一致。设计文档随之改写。
5. **Shadow 文件也开 `escape: true`**：候选文本（短语模板）可含换行 / 制表符。
6. **`phrase_sys_hash` 继续放 redb `META`**：`META` 归 P4 处理，P3 不动它。
7. **redb 页缓存机制测试的负载**从短语表换成仍在 redb 的表（`COMPLETION` 或 `STATS_DAILY`，执行时按其批量写 API 选一个，账本记一行）。

## Review Focus

1. **系统短语的修改在 TOML 升级后**：用户禁用了某条系统短语、又改过另一条的权重；升级后 TOML 改了这两条的权重 → 禁用的仍禁用，改过权重的回到新 TOML 值（同现状）；TOML 删掉的条目连同修改行消失。→ Task 3 的 `toml_upgrade_resets_edits_but_keeps_disabled`。
2. **按键路径零 IO**：每次出候选都查 Shadow，改成内存表后不得读文件、不得取 redb 锁；手改 Shadow 文件后 reload 才生效。→ Task 2 的 `shadow_lookup_does_not_touch_redb_or_file`（`pause()` + 删文件后照样查得到）。
3. **Shadow 空编码桶**：特殊模式用 `code = ""` 记规则，文本层不得把空码行当坏行丢掉。→ Task 2 的 `empty_code_rules_roundtrip`。
4. **手改与程序写交错**：用户在编辑器里改了 `phrases.dict.yaml`（加注释、调顺序）没 reload，程序随后写一条短语 → 先按文件现状重新解析再补丁，手改不丢、注释与顺序保留。→ Task 1 的 `write_reparses_hand_edited_file_first`。
5. **隔离文件**：短语文件头写坏 → 加短语报错（不覆盖用户文件），输入时短语照旧按最后一次成功解析的内容提供；修好后 reload 恢复。→ Task 1 的 `isolated_table_refuses_writes_keeps_last_good`。

---

### Task 1：通用小表 `wind-userdata::text::table`

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/table.rs`
- Modify: `wind_input/crates/wind-userdata/src/text/mod.rs`（`pub mod table;`）
- Test: `table.rs` 内 tests 模块

**Interfaces:**
- Produces:
  - `pub struct TableRow { pub line: usize, pub fields: Vec<String> }`：`fields` 按表头列序，已反转义；短行补空串。
  - `pub struct TableDoc { pub columns: Vec<String>, pub kind: Option<String>, pub escape: bool, pub rows: Vec<TableRow> }`
  - `pub fn parse_table(bytes: &[u8], path: &Path) -> Result<TableDoc, ParseError>`：正文从 `rime_layout` 的 `body_offset` 起；跳过空行；`# no comment` 之前以 `#` 开头的行是注释；**空的首列（如空编码）照样是数据行**。
  - `pub fn col(doc: &TableDoc, name: &str) -> Option<usize>`
  - `pub fn render_table(columns: &[&str], kind: &str, rows: &[Vec<String>]) -> String`：LF、无 BOM，`...` 后固定一行 `# no comment`，`escape: true`。
  - `pub fn patch_groups(original: &[u8], path: &Path, group_cols: &[&str], edits: &[(Vec<String>, Vec<Vec<String>>)], columns: &[&str], kind: &str) -> Result<Vec<u8>, ParseError>`：每个 edit = (分组键的取值, 该组的新行全集，按 `columns` 列序)。原文件里该组的行全部删掉，新行插在原第一行的位置（原文件没有该组 ⇒ 追加到末尾）；其余字节不变；新行按**文件的**列序输出、按文件的 escape 设置转义；`original` 为空 ⇒ 先渲染表头。
  - `pub struct TextTable { path: PathBuf, columns: Vec<&'static str>, kind: &'static str, state: Mutex<TableState> }`，`TableState { fp: String, doc: Arc<TableDoc>, isolated: bool }`：
    - `TextTable::open(path, columns, kind) -> TextTable`：读文件并解析；文件不在 ⇒ 空表；解析失败 ⇒ 空表 + `isolated = true` + WARN。
    - `fn doc(&self) -> Arc<TableDoc>`（读，零 IO）。
    - `fn sync(&self) -> bool`：指纹变了就重新解析（隔离 ⇒ 保留上次成功的 doc、置 isolated、WARN），返回是否换了 doc。
    - `fn edit(&self, edits: &[(Vec<String>, Vec<Vec<String>>)], group_cols: &[&str]) -> anyhow::Result<()>`：重读文件（指纹与缓存不同 ⇒ 先按文件重新解析，手改优先）→ 隔离 ⇒ Err → `patch_groups` → `commit_text(path, bytes, Some(&fp))` → 解析新内容换入。
    - `fn replace_all(&self, rows: &[Vec<String>]) -> anyhow::Result<()>`：整文件重写（清空 / 还原 / 迁移用；不复核指纹）。

- [ ] **Step 1: 写失败测试**（`table.rs` tests 模块；`fn bytes(s)` / `fn p(s)` 小助手自定义）

```rust
const COLS: &[&str] = &["code", "text", "action", "position", "cand_id"];

/// 空首列（空编码）是数据行；省略的尾列补空串；注释与空行跳过；escape 反转义。
#[test]
fn parses_by_column_name_and_keeps_empty_code_rows() {
    let f = "---\ncolumns: [code, text, action, position, cand_id]\nwind: {kind: shadow, escape: true}\n...\n# 注释\n\naa\t恭\tpin\t0\n\t日期\tpin\t1\tphrase:rq:{date}\nbb\t见\\n了\tdelete\n";
    let d = parse_table(f.as_bytes(), Path::new("s")).unwrap();
    assert_eq!(d.kind.as_deref(), Some("shadow"));
    let rows: Vec<Vec<&str>> = d.rows.iter().map(|r| r.fields.iter().map(String::as_str).collect()).collect();
    assert_eq!(rows, vec![
        vec!["aa", "恭", "pin", "0", ""],
        vec!["", "日期", "pin", "1", "phrase:rq:{date}"],
        vec!["bb", "见\n了", "delete", "", ""],
    ]);
}

/// 按组替换：组内行整体换掉、位置不变；别的行、注释、CRLF、BOM 逐字节不变；新组追加。
#[test]
fn patch_groups_replaces_in_place_and_preserves_the_rest() {
    let orig = "\u{feff}---\r\ncolumns: [code, text, action, position, cand_id]\r\nwind: {kind: shadow, escape: true}\r\n...\r\n# 我的规则\r\naa\t恭\tpin\t0\t\r\naa\t工\tdelete\t\t\r\ncc\t从\tpin\t2\t\r\n";
    let out = patch_groups(orig.as_bytes(), Path::new("s"), &["code"],
        &[(vec!["aa".into()], vec![vec!["aa".into(), "共".into(), "pin".into(), "1".into(), "".into()]]),
          (vec!["zz".into()], vec![vec!["zz".into(), "在".into(), "delete".into(), "".into(), "".into()]])],
        COLS, "shadow").unwrap();
    assert_eq!(String::from_utf8(out).unwrap(),
        "\u{feff}---\r\ncolumns: [code, text, action, position, cand_id]\r\nwind: {kind: shadow, escape: true}\r\n...\r\n# 我的规则\r\naa\t共\tpin\t1\t\r\ncc\t从\tpin\t2\t\r\nzz\t在\tdelete\t\t\r\n");
}

/// Review Focus 4：写之前文件被手改过 ⇒ 先按文件重新解析，手改（注释、顺序、新增行）不丢。
#[test]
fn write_reparses_hand_edited_file_first() {
    let dir = tmpdir("handedit");
    let path = dir.join("t.dict.yaml");
    let t = TextTable::open(path.clone(), COLS.to_vec(), "shadow");
    t.edit(&[(vec!["aa".into()], vec![row(&["aa", "恭", "pin", "0", ""])])], &["code"]).unwrap();
    let mut s = std::fs::read_to_string(&path).unwrap();
    s.push_str("# 手写\nbb\t八\tdelete\t\t\n");
    std::fs::write(&path, &s).unwrap();
    t.edit(&[(vec!["cc".into()], vec![row(&["cc", "从", "pin", "1", ""])])], &["code"]).unwrap();
    let s = std::fs::read_to_string(&path).unwrap();
    assert!(s.contains("# 手写\nbb\t八\tdelete\t\t\ncc\t从\tpin\t1\t\n"), "{s}");
    assert_eq!(t.doc().rows.len(), 3);
}

/// Review Focus 5：头部坏 ⇒ 写入报错不覆盖；读按最后一次成功解析的内容；修好后 sync 恢复。
#[test]
fn isolated_table_refuses_writes_keeps_last_good() {
    let dir = tmpdir("isolated");
    let path = dir.join("t.dict.yaml");
    let t = TextTable::open(path.clone(), COLS.to_vec(), "shadow");
    t.edit(&[(vec!["aa".into()], vec![row(&["aa", "恭", "pin", "0", ""])])], &["code"]).unwrap();
    std::fs::write(&path, "---\ncolumns: [\n...\n").unwrap();
    assert!(!t.sync());
    assert_eq!(t.doc().rows.len(), 1, "保留上次成功解析的内容");
    assert!(t.edit(&[(vec!["bb".into()], vec![])], &["code"]).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "---\ncolumns: [\n...\n", "不覆盖");
    t.replace_all(&[row(&["cc", "从", "pin", "0", ""])]).unwrap();
    assert_eq!(t.doc().rows.len(), 1);
    assert!(!t.doc().rows[0].fields.is_empty());
}
```

（`row(&[&str]) -> Vec<String>`、`tmpdir(name) -> PathBuf`（`temp_dir()/wind_tbl_<name>-<pid>`，先删）为测试助手。）

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p wind-userdata --lib table`，Expected：编译失败（模块不存在）。

- [ ] **Step 3: 实现**
  - `parse_table`：剥 BOM；`rime_layout(s, path)` 取 `body_offset` / `comment_cutoff`。头部 `serde_yaml` 解析成 `{columns: Vec<String>, wind: {kind, escape}}`，没有 `columns` ⇒ `ParseError::BadHeader("缺 columns")`。正文逐行处理：
    - 记行号 `line`（按 `\n` 切开的全部行下标，同 `format.rs`），剥 `\r`；
    - 空行（trim 后为空）跳过；
    - 注释区内以 `#` 开头的行跳过；
    - 其余按 `\t` 切开，按 escape 反转义（`wind_wdict::unescape_text_field`），不足列数补空串。
  - `patch_groups`：
    - `original` 为空 ⇒ 用 `render_table(columns, kind, &[])`；
    - 解析拿到 doc 与按 `\n` 切开的行；
    - 对每个 edit，找出分组键（`group_cols` 映射到文件列下标）等于给定值的所有行：第一行原地换成新行的渲染（按文件列序，`\r` 风格同 `patch.rs::with_col`），其余标删；没有该组 ⇒ 记入追加；
    - 新行若以 `#` 开头且文件没有 `comment_cutoff` ⇒ 先补一行 `# no comment`（同 `patch.rs`）。
  - `TextTable`：用 `text::fold::{read_text, fingerprint, commit_text}`。

- [ ] **Step 4: 跑测试确认通过** — `cargo test -p wind-userdata --no-fail-fast`，Expected: 全绿。

- [ ] **Step 5: 提交** — `cargo fmt -p wind-userdata`；`git commit -F - --only -- wind_input/crates/wind-userdata`（消息：`feat(userdata): 通用小表——按列名读写的 TSV、按组补丁、带指纹的持有者`）。

---

### Task 2：Shadow 规则落文本（内存表，按键路径零 IO）

**Files:**
- Modify: `wind_input/crates/wind-store/src/shadow.rs`（`Store` 的 Shadow 方法委托 `ShadowTables`；删除无人用的 `ShadowStore` 及其 2 条测试）
- Modify: `wind_input/crates/wind-store/src/store.rs`（字段 `shadows: Mutex<HashMap<String, Arc<TextTable>>>`；`list_data_schemas` 改扫 `user/*/shadow.dict.yaml`）
- Create: `wind_input/crates/wind-store/src/migrate_p3.rs`（本任务先放 Shadow 部分）
- Test: `shadow.rs` tests、`migrate_p3.rs` tests

**Interfaces:**
- Consumes: Task 1 的 `TextTable` / `parse_table`；P2b 的 `Store::user_text_dir()`、`check_schema_id`。
- Produces:
  - `Store::shadow_path(&self, schema) -> PathBuf`（`<text_dir>/<schema>/shadow.dict.yaml`）
  - `Store` 的 Shadow 方法签名全部不变：`pin_shadow`、`delete_shadow`、`remove_shadow_rule`、`get_shadow_rules`、`export/import_shadow_jsonl`、`export/import_shadow_actions`、`clear_shadow`、`list_shadow_rules`。
  - `Store::sync_shadows(&self) -> Vec<String>`（变了的方案）
  - `pub const MIGRATED_P3_SHADOW: &str = "migrated-p3-shadow";`

映射：`ShadowRecord` ⇄ 同一编码的一组行。行序 = pinned（newest first）在前、deleted 在后；pin 行 `action = pin`、`position` = 槽位、`cand_id` 可空；delete 行 `action = delete`、`position` / `cand_id` 空。按 `code` 分组；空编码合法。

- [ ] **Step 1: 写失败测试**（`shadow.rs` tests 追加）

```rust
/// 规则落文本：pin / delete 按行写进 shadow.dict.yaml，重开读回；newest-first 保持。
#[test]
fn shadow_rules_live_in_text() {
    let p = tmp("wind_shadow_text.redb");
    {
        let s = Store::open(&p).unwrap();
        s.pin_shadow("wb", "aa", "恭", None, 0).unwrap();
        s.pin_shadow("wb", "aa", "工", None, 1).unwrap();
        s.delete_shadow("wb", "aa", "式").unwrap();
        let f = std::fs::read_to_string(s.shadow_path("wb")).unwrap();
        assert!(f.contains("aa\t工\tpin\t1\t\naa\t恭\tpin\t0\t\naa\t式\tdelete\t\t\n"), "{f}");
    }
    let s = Store::open(&p).unwrap();
    let r = s.get_shadow_rules("wb", "aa").unwrap().unwrap();
    assert_eq!(r.pinned.iter().map(|x| x.word.as_str()).collect::<Vec<_>>(), vec!["工", "恭"]);
    assert_eq!(r.deleted, vec!["式".to_string()]);
}

/// Review Focus 3：空编码桶（特殊模式）往返不丢。
#[test]
fn empty_code_rules_roundtrip() {
    let p = tmp("wind_shadow_empty_code.redb");
    {
        let s = Store::open(&p).unwrap();
        s.pin_shadow("wb", "", "日期", Some("phrase:rq:{date}".into()), 2).unwrap();
    }
    let s = Store::open(&p).unwrap();
    let r = s.get_shadow_rules("wb", "").unwrap().unwrap();
    assert_eq!(r.pinned[0].cand_id.as_deref(), Some("phrase:rq:{date}"));
    assert_eq!(r.pinned[0].position, 2);
}

/// Review Focus 2：按键路径查 Shadow 不碰 redb、不读文件（加载后删掉文件、暂停 redb 照样查得到）；
/// 手改文件 sync 后生效。
#[test]
fn shadow_lookup_does_not_touch_redb_or_file() {
    let s = Store::open(tmp("wind_shadow_hot.redb")).unwrap();
    s.pin_shadow("wb", "aa", "恭", None, 0).unwrap();
    let f = s.shadow_path("wb");
    let saved = std::fs::read(&f).unwrap();
    std::fs::remove_file(&f).unwrap();
    s.pause().unwrap();
    assert!(s.get_shadow_rules("wb", "aa").unwrap().is_some());
    s.resume().unwrap();
    std::fs::write(&f, [saved.as_slice(), b"bb\t\xe5\x85\xab\tdelete\t\t\n"].concat()).unwrap();
    assert!(s.get_shadow_rules("wb", "bb").unwrap().is_none(), "未 sync 不生效");
    assert_eq!(s.sync_shadows(), vec!["wb".to_string()]);
    assert_eq!(s.get_shadow_rules("wb", "bb").unwrap().unwrap().deleted, vec!["八".to_string()]);
}
```

`migrate_p3.rs` tests：

```rust
/// 旧 SHADOW 表迁成文本：每方案一文件、规则与顺序原样；完成写标记；重跑幂等。
#[test]
fn shadow_table_migrates_to_text() {
    let p = db("shadow");
    {
        let s = Store::open(&p).unwrap();
        s.legacy_put_shadow("wb", "aa", &ShadowRecord {
            pinned: vec![pin("工", None, 1), pin("恭", None, 0)],
            deleted: vec!["式".into()],
        }).unwrap();
        s.legacy_put_shadow("pinyin", "", &ShadowRecord {
            pinned: vec![pin("日期", Some("phrase:rq:{date}"), 2)],
            deleted: vec![],
        }).unwrap();
    }
    remove_marker(&p, MIGRATED_P3_SHADOW);
    let s = Store::open(&p).unwrap();
    assert!(s.state_dir().join(MIGRATED_P3_SHADOW).is_file());
    let r = s.get_shadow_rules("wb", "aa").unwrap().unwrap();
    assert_eq!(r.pinned.iter().map(|x| x.word.as_str()).collect::<Vec<_>>(), vec!["工", "恭"]);
    assert_eq!(s.get_shadow_rules("pinyin", "").unwrap().unwrap().pinned[0].position, 2);
    drop(s);
    remove_marker(&p, MIGRATED_P3_SHADOW);
    let s = Store::open(&p).unwrap();
    assert_eq!(s.get_shadow_rules("wb", "aa").unwrap().unwrap().pinned.len(), 2, "重跑不重复");
}
```

（`db` / `remove_marker` / `pin` 为测试助手；`legacy_put_shadow` 是本任务新增的 `#[doc(hidden)] pub` 旧表写入器。）

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p wind-store --lib shadow`，Expected：编译失败（`shadow_path` / `sync_shadows` / `legacy_put_shadow` 不存在）。

- [ ] **Step 3: 实现**
  - `shadow.rs`：
    - `fn shadow_table(&self, schema) -> anyhow::Result<Arc<TextTable>>`：`check_schema_id`；从 `self.shadows` 取，没有则 `TextTable::open(shadow_path, COLS, "shadow")` 放进去。
    - `fn record_of(doc, code) -> Option<ShadowRecord>`：按行序把 pin 行收进 `pinned`、delete 行收进 `deleted`。
    - `fn rows_of(code, rec) -> Vec<Vec<String>>`：上面的逆过程。
    - `modify_shadow(schema, code, f)` 改为：取 doc → `record_of` → `f(&mut rec)` → `table.edit(&[(vec![code], rows_of(code, &rec))], &["code"])`；空记录 ⇒ 新行全集为空，即删掉这组。
    - `get_shadow_rules` 只读 `table.doc()`。
    - `list_shadow_rules` 按 doc 分组。
    - `clear_shadow`：`replace_all(&[])`，返回原有的编码组数。
    - `export / import` 两对函数改用上面这些。
    - 删除 `ShadowStore` 与其两条测试。
  - `store.rs`：
    - `list_data_schemas` 把 `[SHADOW]` 的表扫描换成扫 `text_dir` 下有 `shadow.dict.yaml` 的子目录；
    - `open_inner` 在 P2 迁移之后调 `migrate_p3_shadow_if_needed`。
  - `migrate_p3.rs`：
    - 读旧 `SHADOW` 全表（key `schema\0code`、value JSON），按方案分组；
    - 每方案 `table.replace_all(rows)`，其中 rows = 各 code 的 `rows_of` 按 code 字典序拼接；
    - 文件已存在且非空 ⇒ 改为逐 code 合并：文件里已有该 code 的组，保留文件的（新）；
    - 写标记 `MIGRATED_P3_SHADOW`。

- [ ] **Step 4: 跑测试确认通过** — `cargo test -p wind-store -p wind-coordinator -p wind-engine -p wind-webdata -p wind-transfer --no-fail-fast 2>&1 | grep -E "FAILED|test result" | sort | uniq -c`，Expected: 0 failed。会红的旧测试按语义分拣：只断言 redb 内部的删除或改写，确属回归的修代码；每条处置记账本。

- [ ] **Step 5: 提交** — `feat(store): Shadow 规则落 user/<schema>/shadow.dict.yaml，按键路径读内存表；旧表迁移`。

---

### Task 3：短语落文本（叠加模型）

**Files:**
- Modify: `wind_input/crates/wind-store/src/phrases.rs`（方法委托 `PhraseBook`：内存的系统条目 + 用户文件表）
- Modify: `wind_input/crates/wind-store/src/migrate_p3.rs`（短语部分：在首次 `set_system_phrases` 时迁移，需要 TOML 才能区分「系统行是否被改过」）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（启动时**总是**先 `set_system_phrases`；哈希变了再 `sync_system_phrases`；短语迁移完成后升 `format_version`）
- Modify: `wind_input/crates/wind-store/tests/{drop_page_cache,idle_cache_reclaim,scan_reclaim,redb_cache_high_water,drop_page_cache_timing}.rs`（负载换表，裁定 7）
- Test: `phrases.rs` tests、`migrate_p3.rs` tests

**Interfaces:**
- Consumes: Task 1 `TextTable`；现有 `SystemPhrase`、`PhraseRecord`、`SyncStats`。
- Produces:
  - `Store::phrases_path(&self) -> PathBuf`（`<text_dir>/phrases.dict.yaml`）
  - `Store::set_system_phrases(&self, entries: &[SystemPhrase]) -> anyhow::Result<()>`：设定内存中的系统条目；短语迁移标记缺失时先做迁移（见下）。
  - 其余短语方法签名不变。语义对照：

| 方法 | 叠加模型下的实现 |
|---|---|
| `list_phrases` | 叠加视图：系统条目（被修改行覆盖则用修改行的字段，`is_system=true`）∪ 用户行（键撞系统 ⇒ `overrides_system=true`）；按 `code\0text` 排序 |
| `add_phrase` | 写用户行（`overrides_system=0`），已有同键修改行则替换掉它 |
| `update_phrase` | 用户行：改该行（改键 = 删旧行 + 写新行）。系统条目：不改键 ⇒ 写 / 改修改行；改键 ⇒ 裁定 3 |
| `remove_phrase` | 删该键的文件行（用户行或修改行）；系统条目随之恢复默认 |
| `set_phrase_enabled` | 用户行改 `enabled`；系统条目写 / 改修改行的 `enabled`；都不存在 ⇒ 写用户行（同现状） |
| `list_system_phrases` / `list_user_phrases_paged` / `enabled_phrases_for_input` | 叠加视图上过滤 |
| `sync_system_phrases(entries)` | 设定系统条目；修改行的 weight / position / category 改成 TOML 值、保留 `enabled`；TOML 已无该键的修改行删掉。返回 `SyncStats` |
| `ensure_system_phrases` | 系统条目总在，返回 0 |
| `reclaim_system_phrases(entries)` | 键在 TOML 里的用户行删掉（归还系统）。返回删除条数 |
| `reset_system_enabled` | 修改行的 `enabled` 全置 1。返回条数 |
| `reset_user_phrases` | 删全部用户行（`overrides_system=0`），保留修改行 |
| `export_user_phrases_wdict` / `import_user_phrases_wdict` / `plan_phrase_import` / `import_phrases_appending` | 在叠加视图与用户行上实现，语义同现状 |
| `reset_phrases` | 无生产调用方：清空用户文件 |

- [ ] **Step 1: 写失败测试**（`phrases.rs` tests 追加）

```rust
fn sys(code: &str, text: &str, w: i32) -> SystemPhrase {
    SystemPhrase { code: code.into(), text: text.into(), weight: w, position: 0, category: String::new() }
}

/// 短语落文本：用户行进 phrases.dict.yaml；系统条目不进文件；修改系统条目写一行 overrides_system=1。
#[test]
fn phrases_live_in_text_as_overlay() {
    let s = Store::open(tmp("wind_ph_text.redb")).unwrap();
    s.set_system_phrases(&[sys("rq", "{date}", 100), sys("sj", "{time}", 100)]).unwrap();
    s.add_phrase("yx", "me@x.com", 0, 1800).unwrap();
    s.set_phrase_enabled("sj", "{time}", false).unwrap();
    let f = std::fs::read_to_string(s.phrases_path()).unwrap();
    assert!(f.contains("yx\tme@x.com\t1800\t0\t1\t\t0"), "{f}");
    assert!(f.contains("sj\t{time}\t100\t0\t0\t\t1"), "{f}");
    assert!(!f.contains("rq\t"), "未改的系统条目不进文件");
    let input: Vec<String> = s.enabled_phrases_for_input().unwrap().into_iter().map(|p| p.code).collect();
    assert!(input.contains(&"rq".to_string()) && input.contains(&"yx".to_string()));
    assert!(!input.contains(&"sj".to_string()));
    let sj = s.list_system_phrases().unwrap().into_iter().find(|p| p.code == "sj").unwrap();
    assert!(sj.is_system && !sj.enabled);
}

/// Review Focus 1：TOML 升级 ⇒ 修改行回到新 TOML 的权重、保留禁用；TOML 删掉的条目连同修改行消失。
#[test]
fn toml_upgrade_resets_edits_but_keeps_disabled() {
    let s = Store::open(tmp("wind_ph_upgrade.redb")).unwrap();
    s.set_system_phrases(&[sys("a", "甲", 10), sys("b", "乙", 10), sys("c", "丙", 10)]).unwrap();
    s.set_phrase_enabled("a", "甲", false).unwrap();
    s.update_phrase("b", "乙", None, None, None, Some(99)).unwrap();
    s.set_phrase_enabled("c", "丙", false).unwrap();
    let new = [sys("a", "甲", 20), sys("b", "乙", 30)];
    s.set_system_phrases(&new).unwrap();
    s.sync_system_phrases(&new).unwrap();
    let all = s.list_phrases().unwrap();
    let get = |c: &str| all.iter().find(|p| p.code == c).cloned();
    assert_eq!(get("a").map(|p| (p.weight, p.enabled)), Some((20, false)));
    assert_eq!(get("b").map(|p| (p.weight, p.enabled)), Some((30, true)));
    assert!(get("c").is_none());
    assert!(!std::fs::read_to_string(s.phrases_path()).unwrap().contains("丙"));
}

/// 用户行遮蔽系统条目 → 删用户行 ⇒ 系统条目恢复；清空用户短语保留对系统条目的修改。
#[test]
fn user_rows_shadow_and_reset_keeps_system_edits() {
    let s = Store::open(tmp("wind_ph_shadowing.redb")).unwrap();
    s.set_system_phrases(&[sys("a", "甲", 10), sys("b", "乙", 10)]).unwrap();
    s.add_phrase("a", "甲", 3, 500).unwrap();
    let a = s.list_phrases().unwrap().into_iter().find(|p| p.code == "a").unwrap();
    assert!(!a.is_system && a.overrides_system && a.weight == 500);
    s.set_phrase_enabled("b", "乙", false).unwrap();
    s.reset_user_phrases().unwrap();
    let all = s.list_phrases().unwrap();
    let a = all.iter().find(|p| p.code == "a").unwrap();
    assert!(a.is_system && a.weight == 10);
    assert!(!all.iter().find(|p| p.code == "b").unwrap().enabled, "对系统条目的修改保留");
}
```

`migrate_p3.rs` tests 追加：

```rust
/// 旧 PHRASES 表迁成文本：用户行原样；与 TOML 相同的系统行不写；被改过的系统行写成修改行。
#[test]
fn phrases_table_migrates_with_toml_knowledge() {
    let p = db("phrases");
    {
        let s = Store::open(&p).unwrap();
        s.legacy_put_phrase("yx", "me@x.com", 1800, 0, true, false, false, "").unwrap();
        s.legacy_put_phrase("rq", "{date}", 100, 0, true, true, false, "").unwrap();
        s.legacy_put_phrase("sj", "{time}", 100, 0, false, true, false, "").unwrap();
    }
    remove_marker(&p, MIGRATED_P3_PHRASES);
    let s = Store::open(&p).unwrap();
    assert!(!s.state_dir().join(MIGRATED_P3_PHRASES).is_file(), "没有 TOML 就不迁");
    s.set_system_phrases(&[sys("rq", "{date}", 100), sys("sj", "{time}", 100)]).unwrap();
    assert!(s.state_dir().join(MIGRATED_P3_PHRASES).is_file());
    let f = std::fs::read_to_string(s.phrases_path()).unwrap();
    assert!(f.contains("yx\tme@x.com"));
    assert!(!f.contains("rq\t"));
    assert!(f.contains("sj\t{time}\t100\t0\t0\t\t1"));
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p wind-store --lib phrases migrate_p3`，Expected：编译失败。

- [ ] **Step 3: 实现** `PhraseBook { system: RwLock<Arc<Vec<SystemPhrase>>>, table: TextTable }` 放在 `Store` 字段。
  - 表的列是 `PHRASE_COLS = ["code", "text", "weight", "position", "enabled", "category", "overrides_system"]`，分组键为 `["code", "text"]`。
  - 叠加视图：
    - 用 `HashMap<(code, text), PhraseRecord>`，先放系统条目（`is_system = true`）；
    - 再逐个文件行覆盖：修改行（`overrides_system = 1` 且键在系统里）沿用 `is_system = true`、字段取行值；
    - 其它行一律是用户行（`is_system = false`，`overrides_system = 键在系统里`）。
  - `migrate_p3_phrases_if_needed(&self, sys: &[SystemPhrase])`：在 `set_system_phrases` 里、标记缺失时调。读旧 `PHRASES`：
    - 用户行（`!is_system`）原样写成用户行；
    - 系统行与 TOML 同键而 weight / position / category / enabled 有任何不同 ⇒ 写修改行；完全相同 ⇒ 不写；
    - TOML 里没有的旧系统行丢弃，同现状 sync 会删掉它们；
    - 文件已有内容 ⇒ 同组以文件为准，与 Shadow 迁移同口径；
    - 写标记 `MIGRATED_P3_PHRASES`。
  - coordinator 启动段：
    - `entries` 解析之后，**无条件** `store.set_system_phrases(&sys)`；
    - 原「哈希不一致或系统为空」判断改为只看哈希（系统条目总在内存里）。
  - 另外两处会读 TOML 的地方（`restore_system_phrases` 与 `restore_missing_system_phrases`）都先调一次 `set_system_phrases`，保证内存里的系统条目与磁盘 TOML 一致。
  - 迁移完成后（`store.migrated_p3()` 为真），协调器在启动段立即 `raise_format_version(user_dir, 4)`。不等下次启动：等到下次启动才升，会让中间回退的旧版本继续往旧表写。
  - redb 机制测试换负载（裁定 7）。

- [ ] **Step 4: 跑测试确认通过** — `cargo test -p wind-store -p wind-coordinator -p wind-webdata -p wind-transfer -p wind-engine --no-fail-fast`，Expected: 0 failed。旧测试分拣同 Task 2，重点核：
  - `sync_overwrites_user_edits_on_system_rows`
  - `editing_system_phrase_keeps_system_ownership`（裁定 3 若使其红：只改键那一条按新语义改断言，记账本）
  - `reclaim_restores_downgraded_system_rows`
  - `clearing_user_phrases_restores_system_entry_without_resetting_edits`
  - `tests/builtin_platform_phrases.rs`

- [ ] **Step 5: 提交** — `feat(store): 短语落 user/phrases.dict.yaml（系统短语叠加、只记用户行与修改行）；旧表按 TOML 迁移`。

---

### Task 4：版本、手改生效、文档、全量回归

**Files:**
- Modify: `wind_input/crates/wind-config/src/user_meta.rs`（`KNOWN_FORMAT_VERSION = 4`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（`migrated_p3()` 参与升版本）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（`sync_user_texts` 一并 `sync_shadows` + 短语表 sync，返回值并入）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（配置重载后短语表变了 ⇒ `rebuild_phrases`）
- Modify: `docs/design/user-data-storage-redesign.md`（§2.2 Shadow `position` 改写为槽位 + 行序；§10 P3 行）、`wind_input/crates/wind-store/AGENTS.md`、`AGENTS.md`

**Interfaces:**
- Produces: `Store::migrated_p3(&self) -> bool`（两个标记都在）；`Store::sync_phrases(&self) -> bool`。

- [ ] **Step 1: 写失败测试** — `construct.rs` `p1_meta_tests`：

```rust
/// P3 迁完（短语迁移要等 set_system_phrases）后升到 4。
#[test]
fn format_version_reaches_4_after_phrase_migration() {
    let d = std::env::temp_dir().join(format!("wind_p3_fv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let (store, _) = Coordinator::open_user_store_checked(Some(&d), StateChoice::Derived);
    let s = store.unwrap();
    s.set_system_phrases(&[]).unwrap();
    assert!(s.migrated_p3());
    drop(s);
    let (store, _) = Coordinator::open_user_store_checked(Some(&d), StateChoice::Derived);
    drop(store);
    assert!(std::fs::read_to_string(d.join("meta.toml")).unwrap().contains("format_version = 4"));
    let _ = std::fs::remove_dir_all(&d);
}
```

`user_words.rs` tests：手改 `phrases.dict.yaml` 与 `wb/shadow.dict.yaml` 后 `sync_user_texts()` 返回含 `"phrases"` 与 `"wb"`，且 `enabled_phrases_for_input` / `get_shadow_rules` 反映手改。

- [ ] **Step 2: 跑确认失败**；**Step 3: 实现**；**Step 4: 全量回归**（9 个 crate，`--no-fail-fast`）+ clippy 不多于基线；**Step 5: 提交** `feat(userdata): 短语 / Shadow 手改生效，format_version 升到 4；文档`。
