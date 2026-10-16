# 用户数据存储 P4a 实施计划：覆盖表与小文件

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 redb 里剩下的运行期数据全部搬到文件里，redb 只供迁移时读旧数据：
- 三张覆盖表进用户目录：快捷格式调整 `user/quick_format.yaml`、常用字覆盖 `user/common_chars.yaml`、字符集用户层 `user/charsets/<key>.yaml`；
- 两类机器数据进本机状态目录：补全历史 `state/…/completion.json`，每日统计 `state/…/stats/<YYYY-MM>.json` 与 `stats/meta.json`；
- `META` 里的 `phrase_sys_hash` 也进状态目录。

`Store` 方法签名不变。旧表一次性迁移，迁完 `format_version` 升到 5。备份补上快捷格式，还原后重载内存镜像。

**Architecture:**
- **覆盖表**：真源是用户目录下的小文件，内存持有解析结果。写入是「读当前文件 → 改 → 带指纹复核的整文件提交」：编辑器恰好在改，本次写入报错，不静默覆盖；手改经 `sync_user_texts` 生效。通用持有者 `wind-userdata::text::doc::TextDoc` 承担这一套。
- **补全历史**：内存持有，写入只标脏，后台每 2 秒落盘一次，`Store` 释放时和 `dict.flush` 时再各落一次。按键路径（邮箱 / 网址模式、快捷输入历史）读内存，不再碰 redb。
- **统计**：按月一个 JSON 文件，统计收集器每 30 秒那次写盘改写当月文件。

**Tech Stack:** Rust（edition 2024），`wind-userdata`（text 模块）、`wind-store`、`wind-coordinator`、`wind-webdata`、`wind-transfer`、`wind-config`；`serde_yaml` / `serde_json`。

**Spec:** `docs/design/user-data-storage-redesign.md`（§2.1 目录策略、§2.4 覆盖表与小文件、§6、§7.2、§10 P4 行）

## Global Constraints

- 覆盖表是用户策划的小表：「文件是真相源」，改一次整文件 tmp + fsync + rename；不走引擎、不进日志（§2.4）。
- `completion`、`stats` 是机器数据：内存持有，去抖 2 秒后整文件原子写；退出时写一次；JSON，带版本号（§2.4）。
- 状态类放 `local_dir()/state/`（不漫游），真源类放用户目录（§2.1）。
- 迁移：没有标记就整体重做；迁移成功后旧表只读保留，`format_version` 升一级（§7.2）。
- 提交 `git commit -F - --only -- <路径…>`；`cargo fmt -p <crate>`；测试带 `CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录>`，全量用 `--no-fail-fast`。
- INFO 日志不带词条内容。

## 计划内裁定（执行前已定）

1. **P4 拆成 P4a / P4b**。P4b 删掉 redb 依赖、做独立迁移器 `wind_migrate`（§7.3），必须等 P1–P4a 的迁移版本发出去之后才能做（§7：兼容护栏先于破坏性变更）。发版时机由维护者定，P4b 不在本分支。P4a 结束时 redb 仍被打开：迁移要读旧表，暂停 / 恢复与页缓存回收这套机制原样保留。
2. **字符集用户层改回文件**（`charsets.rs` 模块文档里「库是唯一真相源」的取舍随之反转，§2.4）。当初放进库的唯一理由是「程序和人同写一个文件，后保存者赢」。现在写入带指纹复核，编辑器改过之后程序这次写入会失败并提示，不再静默覆盖，这条理由不再成立。原有的「导出编辑副本 → 回读」流程不变；回读改为写真源文件。
3. **文件格式**：
   - `quick_format.yaml`：`kinds: {<kind>: {moved: [{id, position}], disabled: [..], added: [{id, text}]}}`，字段即 `QuickFormatRecord` 的 serde 形态；
   - `common_chars.yaml`：`common: [字…]` 与 `uncommon: [字…]` 两个列表，按字素簇排序；
   - `charsets/<key>.yaml`：原文，与备份包里的 `charsets/<key>.yaml` 是同一份；
   - `completion.json`：`{version: 1, items: [{kind, text, count, last_used}]}`；
   - `stats/<YYYY-MM>.json`：`{version: 1, days: {<YYYY-MM-DD>: DailyStats}}`，`stats/meta.json` 是 `StatsMeta`；
   - `phrase_sys_hash`：单行文本。

   所有文件都在文件头或 `version` 字段标版本。
4. **补全历史的写盘节奏**：写入只改内存并标脏。后台落盘线程只持 `Weak`，生产由协调器起，与用户数据节拍同一处；`Store` 释放时同步落一次，所以测试和工具不起线程也不丢数据。代价：进程被强杀时可能丢最后 2 秒的补全记录（§2.4 接受）。
5. **备份包格式不变**（老备份照样能还原）。只新增 `userdata/quick_format.yaml` 一节，并补两处还原后的缺口：
   - 还原后重载常用字与快捷格式的内存镜像；
   - 还原统计后重置统计收集器，免得它下一次写盘把刚还原的「今天」和总计覆盖掉。

   「备份按类别路由」若改成直接打包 `user/` 与 `state/` 目录，那是备份格式的变更，与 P4b 一起做。
6. **`schema_version`（META）随 redb 一起留到 P4b**：它只描述 redb 文件本身。

## Review Focus

1. **按键路径不碰 redb**：邮箱 / 网址补全、快捷输入历史，每键读内存；上屏后的记录与裁剪只改内存。→ Task 3 的 `completion_hot_path_does_not_touch_redb`（`pause()` 后照常补全、记录）。
2. **编辑器与程序同改覆盖表**：用户在编辑器里改着 `common_chars.yaml`，同时在右键菜单里设常用 → 程序这次写入报错，不覆盖；`reload` 之后再改成功。→ Task 2 的 `override_write_refuses_when_file_changed_underneath`。
3. **退出时不丢补全记录**：最后一次记录还没到 2 秒落盘就释放 `Store`，重开后记录在。→ Task 3 的 `completion_survives_drop_without_flusher`。
4. **统计还原不被收集器覆盖**：Replace 还原 stats 后，收集器下一次写盘不得把「今天」和 `meta` 写回还原前的值。→ Task 5 的 `restore_stats_resets_collector`。
5. **迁移不覆盖已有文件**：用户目录里已经有手写的 `common_chars.yaml`，迁移跳过这一类，不拿旧表盖掉它。→ Task 5 的 `p4_migration_keeps_existing_files`。

---

### Task 1：`wind-userdata::text::doc::TextDoc`（带指纹的单文件持有者）

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/doc.rs`
- Modify: `wind_input/crates/wind-userdata/src/text/mod.rs`（`pub mod doc;`）

**Interfaces（Produces）:**

```rust
pub struct TextDoc { /* path, Mutex<(fp: String, content: Option<Arc<String>>)> */ }
impl TextDoc {
    /// 读文件（不存在 ⇒ None）。读不了 ⇒ None + WARN（不当成删除：fp 记成 "unreadable"）。
    pub fn open(path: PathBuf) -> TextDoc;
    pub fn path(&self) -> &Path;
    /// 内存里的内容（零 IO）。
    pub fn get(&self) -> Option<Arc<String>>;
    /// 文件指纹变了就重读，返回是否换了内容。
    pub fn sync(&self) -> bool;
    /// 读改写：持锁 → 按文件现状重读（手改优先）→ f(当前内容) → Some(new) 写入 / None 删文件 →
    /// 带指纹复核提交（复核失败 ⇒ Err，内存不变）。
    pub fn modify(&self, f: &mut dyn FnMut(Option<&str>) -> Option<String>) -> anyhow::Result<()>;
    /// 整文件替换（还原 / 迁移；不复核）。None ⇒ 删文件。
    pub fn replace(&self, content: Option<&str>) -> anyhow::Result<()>;
}
```

- [ ] **Step 1: 写失败测试**（`doc.rs` tests）

```rust
#[test]
fn modify_rereads_and_commits() {
    let d = tmpdir("modify");
    let doc = TextDoc::open(d.join("a.yaml"));
    assert!(doc.get().is_none());
    doc.modify(&mut |cur| { assert!(cur.is_none()); Some("x: 1\n".into()) }).unwrap();
    std::fs::write(d.join("a.yaml"), "x: 2\n").unwrap();          // 手改
    doc.modify(&mut |cur| { assert_eq!(cur, Some("x: 2\n")); Some(format!("{}y: 3\n", cur.unwrap())) }).unwrap();
    assert_eq!(std::fs::read_to_string(d.join("a.yaml")).unwrap(), "x: 2\ny: 3\n");
    assert_eq!(doc.get().as_deref().map(String::as_str), Some("x: 2\ny: 3\n"));
}

#[test]
fn concurrent_hand_edit_during_commit_is_refused() {
    let d = tmpdir("race");
    let p = d.join("a.yaml");
    let doc = TextDoc::open(p.clone());
    doc.modify(&mut |_| Some("v: 1\n".into())).unwrap();
    let p2 = p.clone();
    crate::text::fold::set_before_rename_hook(Some(Box::new(move || {
        std::fs::write(&p2, "v: hand\n").unwrap();
    })));
    let r = doc.modify(&mut |_| Some("v: prog\n".into()));
    crate::text::fold::set_before_rename_hook(None);
    assert!(r.is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "v: hand\n", "手改不被覆盖");
}

#[test]
fn none_deletes_and_sync_sees_external_changes() {
    let d = tmpdir("sync");
    let p = d.join("a.yaml");
    let doc = TextDoc::open(p.clone());
    doc.modify(&mut |_| Some("a\n".into())).unwrap();
    std::fs::write(&p, "b\n").unwrap();
    assert!(doc.sync());
    assert!(!doc.sync());
    doc.modify(&mut |_| None).unwrap();
    assert!(!p.exists() && doc.get().is_none());
}
```

（`set_before_rename_hook` 是全局钩子：本测试与 `text_target` 的同名用法分属不同测试二进制，不会互相干扰；同一个二进制里只有这一处用它。）

- [ ] **Step 2** 跑测试确认失败。**Step 3** 实现：复用 `text::fold::{read_text, fingerprint, commit_text}`；删文件时 `std::fs::remove_file`，NotFound 不算错。**Step 4** 跑测试确认通过。**Step 5** 提交：`feat(userdata): 单文件持有者 TextDoc——读改写带指纹复核`。

---

### Task 2：覆盖表落文件（快捷格式、常用字、字符集用户层）

**Files:**
- Modify: `wind_input/crates/wind-store/src/quick_format.rs`、`src/common_chars.rs`、`src/charsets.rs`（方法委托文件）
- Modify: `wind_input/crates/wind-store/src/store.rs`（字段 `overrides: Overrides { quick: TextDoc, common: TextDoc, charsets: Mutex<HashMap<String, Arc<TextDoc>>> }`，`open_inner` 初始化）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`：`sync_user_texts` 一并同步三类覆盖表，返回值里分别记 `"quick_format"`、`"common_chars"`、`"charsets"`；`reload_user_text_paths` 也认这三类路径
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`reload_user_config` 里根据返回值调用 `reload_quick_adjust` / `reload_common_chars` / 字符集 registry 重建）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（`web_dict_reload` 同样按返回值重载这三类镜像）

**Interfaces:**
- Produces:
  - `Store::quick_format_path()`、`Store::common_chars_path()`、`Store::charset_path(key)`。`key` 先过 `check_schema_id` 同款的路径分量检查，复用 `wind_userdata::engine::check_schema_id`。
  - 各表原有方法签名不变。

- [ ] **Step 1: 写失败测试**（三个模块的 tests 各加一条；第三条放 `common_chars.rs`）

```rust
// quick_format.rs
#[test]
fn quick_format_lives_in_user_dir() {
    let p = tmp("wind_qf_file.redb");
    {
        let s = Store::open(&p).unwrap();
        s.add_quick_format("date", "my", "yyyy").unwrap();
        s.set_quick_format_enabled("date", "iso", false).unwrap();
        let f = std::fs::read_to_string(s.quick_format_path()).unwrap();
        assert!(f.contains("my") && f.contains("iso"), "{f}");
    }
    let s = Store::open(&p).unwrap();
    let r = s.get_quick_format("date").unwrap().unwrap();
    assert_eq!(r.added[0].id, "my");
    assert_eq!(r.disabled, vec!["iso".to_string()]);
}

// common_chars.rs
#[test]
fn common_chars_live_in_user_dir() {
    let s = Store::open(tmp("wind_cc_file.redb")).unwrap();
    s.set_common_char_override("槮", true).unwrap();
    s.set_common_char_override("的", false).unwrap();
    let f = std::fs::read_to_string(s.common_chars_path()).unwrap();
    assert!(f.contains("common:") && f.contains("槮") && f.contains("uncommon:"), "{f}");
    assert_eq!(s.get_common_char_override("的").unwrap(), Some(false));
}

/// Review Focus 2：编辑器正在改文件（复核前被改）⇒ 程序这次写入报错、不覆盖；sync 后再写成功。
#[test]
fn override_write_refuses_when_file_changed_underneath() {
    let s = Store::open(tmp("wind_cc_race.redb")).unwrap();
    s.set_common_char_override("槮", true).unwrap();
    let f = s.common_chars_path();
    let f2 = f.clone();
    wind_userdata::text::fold::set_before_rename_hook(Some(Box::new(move || {
        std::fs::write(&f2, "common: [甲]\nuncommon: []\n").unwrap();
    })));
    let r = s.set_common_char_override("乙", true);
    wind_userdata::text::fold::set_before_rename_hook(None);
    assert!(r.is_err());
    assert!(std::fs::read_to_string(&f).unwrap().contains("甲"));
    s.sync_user_texts();
    s.set_common_char_override("乙", true).unwrap();
    let f = std::fs::read_to_string(&f).unwrap();
    assert!(f.contains("甲") && f.contains("乙"));
}

// charsets.rs
#[test]
fn charset_docs_live_in_user_dir() {
    let s = Store::open(tmp("wind_cs_file.redb")).unwrap();
    s.set_charset_doc("rare", "key: rare\nname: 生僻\n").unwrap();
    assert_eq!(std::fs::read_to_string(s.charset_path("rare").unwrap()).unwrap(), "key: rare\nname: 生僻\n");
    assert_eq!(s.list_charset_docs().unwrap().len(), 1);
    assert!(s.remove_charset_doc("rare").unwrap());
    assert!(!s.charset_path("rare").unwrap().exists());
    assert!(s.set_charset_doc("../x", "k").is_err());
}
```

- [ ] **Step 2** 确认失败。
- [ ] **Step 3** 实现：
  - **快捷格式**：`modify_quick_format(kind, f)` 改为对 `quick` 做一次 `TextDoc::modify`：解析 YAML 成 `BTreeMap<String, QuickFormatRecord>`，应用 f，记录为空的 kind 删掉，全表为空 ⇒ 删文件，否则写回 YAML（头部一行 `# 快捷格式调整（WindInput 维护，可手改）`）。读方法从 `quick.get()` 解析；解析失败 ⇒ WARN、按空表处理，写入时报错不覆盖。
  - **常用字**：同样做法，在 `BTreeMap<String, bool>` 与 `common` / `uncommon` 两个列表之间互转；`apply_common_char_overrides` 一次提交。
  - **字符集**：
    - 每个 key 一个 `TextDoc`：`set` 是 `replace(Some)`、`remove` 是 `replace(None)`，原文不经解析。
    - `list` 扫描 `user/charsets/*.yaml`，结果按 key 排序；`clear` 删掉整个目录下的 yaml。
    - 编辑流程保留「导出副本、显式回读」：回读要整体覆盖，所以 `set` 用 `replace`、不走 `modify`。
- [ ] **Step 4** 跑 `cargo test -p wind-store -p wind-coordinator -p wind-engine -p wind-webdata -p wind-transfer --no-fail-fast`，Expected: 0 failed。旧测试分拣同前几阶段：断言 redb 内部机制的改写或删除，账本逐条记。
- [ ] **Step 5** 提交：`feat(store): 快捷格式 / 常用字 / 字符集用户层落用户目录文件（文件是真源，写入带指纹复核）`。

---

### Task 3：补全历史落本机状态文件（内存持有、去抖落盘）

**Files:**
- Modify: `wind_input/crates/wind-store/src/completion.rs`（方法改读写内存表 `Completions { map: BTreeMap<(kind, text), CompletionRecord>, dirty: bool }`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（字段、`Drop for Store` 里 `flush_small_files()`、`spawn_small_file_flusher(every)`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（起落盘线程，节拍 2 秒）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`：`flush_user_data` 里也落小文件
- Modify: `wind_input/crates/wind-store/tests/*`：redb 页缓存测试的负载改用 `STATS_DAILY` 的旧表写入器，即 Task 4 新增的 `legacy_put_daily_stat`，并配一个只读扫描 `legacy_scan_count`（`#[doc(hidden)]`）

**Interfaces:**
- Produces:
  - `Store::completion_path()`：`<state_dir>/completion.json`
  - `Store::flush_small_files(&self) -> anyhow::Result<()>`
  - `Store::spawn_small_file_flusher(&self, every: Duration)`：只持 `Weak`

- [ ] **Step 1: 写失败测试**（`completion.rs` tests）

```rust
/// Review Focus 1：按键路径读写都只碰内存（redb 暂停照样补全、记录）。
#[test]
fn completion_hot_path_does_not_touch_redb() {
    let s = store("hot");
    s.record_completion(CompletionKind::EmailSuffix, "qq.com").unwrap();
    s.pause().unwrap();
    assert!(s.record_completion(CompletionKind::EmailSuffix, "163.com").unwrap());
    assert_eq!(s.list_completions(CompletionKind::EmailSuffix, "", 0, 0).unwrap().1, 2);
    assert_eq!(s.prune_completions(CompletionKind::EmailSuffix, 1, None).unwrap(), 1);
    s.resume().unwrap();
}

/// Review Focus 3：不起落盘线程，释放 Store 时同步落盘，重开后记录在。
#[test]
fn completion_survives_drop_without_flusher() {
    let p = std::env::temp_dir().join(format!("wind_cmp_drop_{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&p);
    {
        let s = Store::open(&p).unwrap();
        s.record_completion(CompletionKind::UrlHistory, "example.com").unwrap();
        s.record_completion(CompletionKind::UrlHistory, "example.com").unwrap();
    }
    let s = Store::open(&p).unwrap();
    let rec = s.get_completion(CompletionKind::UrlHistory, "example.com").unwrap().unwrap();
    assert_eq!(rec.count, 2);
    assert!(s.completion_path().is_file());
}
```

- [ ] **Step 2** 确认失败。
- [ ] **Step 3** 实现：
  - 打开时读 `completion.json`：读不了或版本不认识 ⇒ WARN、空表，同时**不覆盖**原文件（记 `isolated`，落盘时跳过）。
  - 各方法在内存表上实现，语义逐条照旧（排序、上限、prune 的 `keep`）。写入后置 `dirty`。
  - `flush_small_files` 在 `dirty` 时整文件原子写（`commit_text(…, None)`），写成功清 `dirty`。
  - `Drop for Store` 调 `flush_small_files`，失败只 WARN。
  - redb 机制测试换负载，见 Files。
- [ ] **Step 4** 跑测试确认通过，含 `wind-coordinator` 里邮箱 / 网址 / 快捷输入的既有测试。
- [ ] **Step 5** 提交：`feat(store): 补全历史改为内存持有、去抖落本机状态文件，按键路径不碰 redb`。

---

### Task 4：统计与 `phrase_sys_hash` 落本机状态文件

**Files:**
- Modify: `wind_input/crates/wind-store/src/stats.rs`（`put_daily_stat` / `get_daily_stat` / `daily_stats` / `clear_stats` / `prune_stats_before` / `export` / `import` / `get_stats_meta` / `put_stats_meta` 改读写 `<state_dir>/stats/`；新增 `legacy_put_daily_stat` 与 `legacy_scan_count`，供 Task 3 的测试负载用）
- Modify: `wind_input/crates/wind-store/src/phrases.rs`（`phrase_sys_hash` / `set_phrase_sys_hash` 改读写 `<state_dir>/phrase_sys_hash`）
- Modify: `wind_input/crates/wind-store/src/stat_collector.rs`（`is_paused()` 检查删掉：统计已不在 redb）

- [ ] **Step 1: 写失败测试**（`stats.rs` tests）

```rust
#[test]
fn daily_stats_live_in_monthly_state_files() {
    let s = Store::open(tmp("wind_stats_files.redb")).unwrap();
    let mut d = DailyStats::default();
    d.chinese = 12;
    s.put_daily_stat("2026-10-14", &d).unwrap();
    s.put_daily_stat("2026-09-30", &d).unwrap();
    assert!(s.stats_dir().join("2026-10.json").is_file());
    assert!(s.stats_dir().join("2026-09.json").is_file());
    assert_eq!(s.daily_stats("2026-09-01", "2026-10-31").unwrap().len(), 2);
    assert_eq!(s.prune_stats_before("2026-10-01").unwrap(), 1);
    assert!(!s.stats_dir().join("2026-09.json").exists());
    s.put_stats_meta(&StatsMeta { total_chars: 99, ..Default::default() }).unwrap();
    assert_eq!(s.get_stats_meta().unwrap().total_chars, 99);
}
```

`phrases.rs` tests 加 `phrase_sys_hash_lives_in_state_dir`：`set_phrase_sys_hash("h1")` 之后重开、读回 `"h1"`，并断言 `<state_dir>/phrase_sys_hash` 文件存在。

- [ ] **Step 2–4** 同上。`daily_stats(from, to)` 只读区间涉及的月份文件；当月文件写入是读改写 + 原子提交（收集器线程 30 秒一次，不在按键路径）。
- [ ] **Step 5** 提交：`feat(store): 每日统计按月落本机状态文件；phrase_sys_hash 离开 redb`。

---

### Task 5：迁移、版本、备份补齐、文档

**Files:**
- Create: `wind_input/crates/wind-store/src/migrate_p4.rs`（标记 `migrated-p4`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（`open_inner` 调 P4 迁移；`format_version_reached` 多一档 → 5）
- Modify: `wind_input/crates/wind-config/src/user_meta.rs`（`KNOWN_FORMAT_VERSION = 5`）
- Modify: `wind_input/crates/wind-store/src/quick_format.rs`（`export_quick_format_yaml` / `import_quick_format_yaml`，供备份用）
- Modify: `wind_input/crates/wind-transfer/src/backup.rs`：
  - 备份写 `userdata/quick_format.yaml`，还原读它（`section_of` 归 `quick_format`）；
  - `wind_input/apps/service/src/backup_cli.rs` 的 `RESTORE_SECTIONS` 同步登记。
- Modify: `wind_input/crates/wind-webdata/src/lib.rs` 的 `backup.restore` 收尾：
  - 涉及常用字 ⇒ `reload_common_chars`；
  - 涉及快捷格式 ⇒ `reload_quick_adjust`；
  - 涉及统计 ⇒ 收集器 `reset()` 后 `resume()`，从还原后的 meta 重新载入。
- Modify: `docs/design/user-data-storage-redesign.md`（§10 P4 行拆成 P4a / P4b）、`wind_input/crates/wind-store/AGENTS.md`、`AGENTS.md`

- [ ] **Step 1: 写失败测试**

`migrate_p4.rs` tests：

```rust
/// 旧表各类数据迁成文件：快捷格式、常用字、字符集、补全、统计、meta、phrase_sys_hash。
#[test]
fn p4_tables_migrate_to_files() { /* legacy_put_* 造旧表 → 删标记重开 → 各 get_* 读回、各文件存在、标记在 */ }

/// Review Focus 5：用户目录里已有手写的 common_chars.yaml ⇒ 这一类跳过，不拿旧表盖掉。
#[test]
fn p4_migration_keeps_existing_files() { /* 先写文件 → 旧表放另一组覆盖 → 迁移后文件内容不变 */ }
```

这两条按 P3 迁移测试的写法写成可运行代码。`legacy_put_*` 是本任务新增的 `#[doc(hidden)]` 旧表写入器，每张表一个。

`wind-webdata` tests：

```rust
/// Review Focus 4：Replace 还原统计后，收集器下一次写盘不覆盖还原值。
#[test]
fn restore_stats_resets_collector() { /* 记几笔统计 → 备份 → 再记 → Replace 还原 → 触发收集器 flush → daily_stats/meta 等于备份时的值 */ }
```

`wind-transfer` tests：备份包里含 `userdata/quick_format.yaml`，还原后 `get_quick_format` 读回。

- [ ] **Step 2–4** 同上。迁移规则：每类目标文件或目录已存在且非空 ⇒ 跳过这一类；全部做完写标记。最后跑全量回归（9 个 crate）与 clippy，不多于基线。
- [ ] **Step 5** 提交：`feat(store): 剩余旧表迁成文件、format_version 升到 5；备份补快捷格式，还原后重载镜像与统计收集器`。
