# 用户数据 P5：用户目录装下全部数据 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 只复制用户目录就带走全部数据：学到的数据、补全历史、统计都进 `<userdata>/data/`，格式统一为 TOML / 表格文本 / 出厂同格式 YAML，`format_version` 6 是唯一迁移判据。

**Architecture:** 引擎的「文本目标」从只认 `user.dict.yaml` 推广为按列规格读写的表格文本（`TextFormat::Table`），temp / draft / freq / assoc 四个目标由规则自动绑定到 `data/<schema>/` 下的文件，快照段降为可重建的加速结构；补全、统计这两个机写小文件改为表格文本 / TOML 并统一「`.bak` 自愈 → 改名留存后重建」；手改文件解析失败一律隔离不覆盖。迁移按「目标在不在」逐步幂等，全部完成才把 `format_version` 写成 6、删掉 `migrated-*` 标记、把 `userdata.redb` 改名 `.legacy`。

**Tech Stack:** Rust（wind-userdata 引擎、wind-store、wind-coordinator、wind-webdata、wind-config），serde + toml / serde_yaml；wind-setting（egui）设置端提示。

> **2026-10-15 维护者修订（执行中）**：`user/` 等中间布局从未发布，只在维护者测试机上用过——不做 `user/`→`data/` 改名与补全 / 统计 / 快捷格式的搬家迁移，`format_version` 不升（仍为 5）；Task 10 改为「到 5 时删标记、旧库改名 `.legacy`」。下文 Task 10 中涉及 6 与搬家的步骤以此为准作废。

**Spec:** `docs/design/user-data-storage-redesign.md`（§2.1、§2.4、§2.5、§3.2、§7.4、§10 P5 行）

## Global Constraints

- 提交只用 `git commit -F - --only -- <自己的路径…>`；不用 `git add -A` / `git add .` / `git commit -a`；新文件先 `git add <path>` 再 `--only`。不用 `git stash`、不 `--hard`、不 `--no-verify`。
- 格式化只 `cargo fmt -p <crate>`，不 `--all`。
- INFO 及以上日志不得包含词条内容（码、词、上文、补全文本）；坏行 WARN 只记文件与行数。
- 测试环境：`CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录>`，全量用 `--no-fail-fast`。
- 工作目录：worktree `本分支的 worktree`，分支 `feat/user-data-storage`；cargo 命令在其 `wind_input/` 下跑。
- 目录名：用户目录下 `data/`（常量 `wind_store::DATA_DIR_NAME = "data"`）；旧名 `user/` 只在迁移里出现。
- 文件名（逐字）：`temp.dict.yaml`、`draft.dict.yaml`、`freq.table.yaml`、`assoc.table.yaml`、`completion.table.yaml`、`stats/<YYYY-MM>.toml`、`stats/meta.toml`、`quick_format.toml`；旧 redb 改名为 `userdata.redb.legacy`；坏文件留存名 `<文件名>.corrupt-<YYYYMMDD-HHMMSS>`；未迁移的快捷格式 `quick_format.yaml.unmigrated`。
- `KNOWN_FORMAT_VERSION = 6`；`FULLY_MIGRATED_VERSION` 仍为 5（含义：旧库各表迁完）；新增 `P5_FORMAT_VERSION = 6`。
- 时间戳列写 Unix 秒整数。
- 日志定期 fsync：有新写入时每 5 秒（复用 `USERDATA_TICK`）。
- 不引入新依赖；不发明二进制源格式；不用 JSON 写任何新文件。
- redb 依赖与 P1–P4 迁移代码保留。
- 备份包格式不变（条目名 `userdata/quick_format.yaml` 等照旧，内容经 Store API 导出 / 导入）。

## Review Focus

1. 升级中途断电 / 失败（例如 `user/` 已改名、补全已迁、统计没迁）：再次启动必须从断点接着做，数据不丢不重复，`format_version` 在全部完成前不得写 6。
2. v5 用户的学到的数据只在 `state/runtime/…` 快照里、`data/` 下还没有文本：绑定时绝不能由「文件不存在」推出「数据为空」而装空段——要先由当前视图渲染出文本。
3. 用户手改学到的数据文件写坏一行 / 整份写坏：坏行原样保留、其余照常；整份坏时按旧数据继续服务、打字仍在学习（进 delta），修好后自动合入；程序任何路径都不覆盖它（自动的草稿清理也不行）。
4. 统计元数据文件坏：不得再被当成全 0 写回（M-3）；补全历史文件坏：不得让整个会话停止学习（M-2）。
5. 删掉整个 `state/` 后重启：学到的数据、补全、统计全部还在（P5 退出判据）。

每条的守护测试分别落在：1 → Task 10 `p5_migration_resumes_after_partial_run`；2 → Task 4 `bind_adopts_existing_view_when_text_missing`；3 → Task 4 `isolated_learned_text_keeps_learning_and_merges_after_fix`、`auto_purge_never_rewrites_isolated_text`；4 → Task 7 / Task 8 的 `*_self_heals_*` 与 `corrupt_stats_meta_is_recalculated_not_zeroed`；5 → Task 10 `wiping_state_dir_keeps_all_user_data`。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `wind-userdata/src/journal.rs` | `dirty` 标记、`sync_if_dirty` | 改 |
| `wind-userdata/src/issues.rs` | 进程级「文件问题」登记表（坏行数 / 隔离 / 已重建） | 新建 |
| `wind-userdata/src/text/codec.rs` | 学到的数据的列规格、解码（行级跳过计数）、渲染、按键补丁 | 新建 |
| `wind-userdata/src/text/format.rs` | 用户词解析报告坏行数 | 改 |
| `wind-userdata/src/text/fold.rs` | `preserve_corrupt`（改名留存） | 改 |
| `wind-userdata/src/text/doc.rs` | `modify` 闭包可报错（M-1） | 改 |
| `wind-userdata/src/engine.rs` | `TextFormat`、表格目标折回 / 同步 / 整体替换、采纳视图、自动绑定规则、日志 fsync | 改 |
| `wind-store/src/learned_text.rs` | 学到的数据的绑定规则 | 新建 |
| `wind-store/src/machine_file.rs` | 机写文件读取：`.bak` 自愈 / 留存重建 | 新建 |
| `wind-store/src/completion.rs` | 表格文本、机写策略 | 改 |
| `wind-store/src/stats.rs` | TOML、机写策略、元数据坏则重算 | 改 |
| `wind-store/src/quick_format.rs` / `common_chars.rs` | TOML、保留最后一次好数据、解析失败不重写 | 改 |
| `wind-store/src/migrate_p5.rs` | P5 迁移各步 | 新建 |
| `wind-store/src/store.rs` / `user_words.rs` | `data/` 目录、标记、`dict.reload` 新路径 | 改 |
| `wind-coordinator/src/construct.rs` / `coordinator.rs` | 目录改名、`format_version` 6、redb 改名 | 改 |
| `wind-config/src/user_meta.rs` | `KNOWN_FORMAT_VERSION = 6`、全新目录判定 | 改 |
| `wind-webdata/src/lib.rs` | `system.storageStatus` 带 `issues` | 改 |
| `config/app.toml` | 卸载器 markers / 注释 | 改 |
| wind-setting `src/pages/dict/…` | 词库页显示数据文件问题 | 改（另一仓） |

---

### Task 1: 日志有写入时每 5 秒 fsync

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/journal.rs`
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`spawn_ticker`，新增 `sync_journal`）
- Test: `journal.rs` 内 `#[cfg(test)]`，`engine.rs` 内测试

**Interfaces:**
- Produces: `Journal::sync_if_dirty(&mut self) -> anyhow::Result<bool>`；`Journal::is_dirty(&self) -> bool`；`UserData::sync_journal(&self) -> anyhow::Result<bool>`

- [ ] **Step 1: 写失败测试**（journal.rs tests）

```rust
#[test]
fn sync_if_dirty_only_after_append() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.wjl");
    let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
    assert!(!j.is_dirty());
    assert!(!j.sync_if_dirty().unwrap(), "没有新写入不 fsync");
    j.append(&rec(j.next_seq())).unwrap();
    assert!(j.is_dirty());
    assert!(j.sync_if_dirty().unwrap());
    assert!(!j.is_dirty());
    assert!(!j.sync_if_dirty().unwrap());
}
```
（`rec(seq)` 用本文件测试里已有的记录构造辅助；没有就照 `Record { seq, ts_ms: 1, kind: OpKind::Upsert, target: Target::Temp, schema: "s".into(), code: "a".into(), text: "啊".into(), payload: Payload::Weight(1), boundary: 0 }` 写一个。）

engine.rs tests：

```rust
#[test]
fn sync_journal_reports_whether_it_synced() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    assert!(!ud.sync_journal().unwrap());
    ud.apply(&[Op::new(Target::Temp, "s", OpKind::Incr, "a", "啊", Payload::Count(1), 0)]).unwrap();
    assert!(ud.sync_journal().unwrap());
    assert!(!ud.sync_journal().unwrap());
}
```
（`open_ud` / `apply` 用 engine.rs 测试已有的打开与写入辅助，名字以文件里的为准。）

- [ ] **Step 2: 跑，确认失败**：`cargo test -p wind-userdata sync_if_dirty sync_journal_reports` → 编译失败（方法不存在）。

- [ ] **Step 3: 实现**

journal.rs：`Journal` 加字段 `dirty: bool`（open 时 false）；`append` 成功后 `self.dirty = true`；`prune_before` 里已 `sync_data` 的路径置 `dirty = false`。

```rust
pub fn is_dirty(&self) -> bool { self.dirty }

/// 有新写入就 fdatasync 一次（§3.2：断电最多丢最近一个节拍）。返回是否真的刷了。
pub fn sync_if_dirty(&mut self) -> anyhow::Result<bool> {
    if !self.dirty { return Ok(false); }
    self.file.sync_data().context("刷日志")?;
    self.dirty = false;
    Ok(true)
}
```

engine.rs：

```rust
/// 日志有新写入就落盘一次（后台节拍每拍调）。只读实例无日志 ⇒ Ok(false)。
pub fn sync_journal(&self) -> anyhow::Result<bool> {
    match self.journal.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        Some(j) => j.sync_if_dirty(),
        None => Ok(false),
    }
}
```
`spawn_ticker` 循环体里在 `needs_maintain` 之前：`if let Err(e) = me.sync_journal() { tracing::warn!("日志落盘失败：{e:#}"); }`。

- [ ] **Step 4: 跑，确认通过**：同上命令 → PASS。
- [ ] **Step 5: 提交** `feat(userdata): 日志有新写入时每个节拍 fsync 一次`

---

### Task 2: 文件问题登记表 + 用户词坏行计数

**Files:**
- Create: `wind_input/crates/wind-userdata/src/issues.rs`（`lib.rs` 加 `pub mod issues;`）
- Modify: `wind_input/crates/wind-userdata/src/text/format.rs`（`TextDict.bad_lines`）
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`text_entries` 报告）

**Interfaces:**
- Produces:
```rust
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum IssueKind { BadLines(usize), Isolated, Rebuilt { kept_as: String }, Unmigrated { kept_as: String } }
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileIssue { pub path: std::path::PathBuf, pub kind: IssueKind }
pub fn report(path: &Path, kind: IssueKind);   // 同一路径覆盖旧值
pub fn clear(path: &Path);
pub fn list_under(root: &Path) -> Vec<FileIssue>; // 按路径排序
```
- `TextDict { …, pub bad_lines: usize }`

- [ ] **Step 1: 写失败测试**

issues.rs：
```rust
#[test]
fn report_overwrites_and_list_filters_by_root() {
    let a = Path::new("/tmp/issues-t1/data/a.dict.yaml");
    let b = Path::new("/tmp/issues-t1-other/b");
    report(a, IssueKind::BadLines(2));
    report(a, IssueKind::Isolated);
    report(b, IssueKind::Isolated);
    let v = list_under(Path::new("/tmp/issues-t1"));
    assert_eq!(v, vec![FileIssue { path: a.into(), kind: IssueKind::Isolated }]);
    clear(a);
    assert!(list_under(Path::new("/tmp/issues-t1")).is_empty());
    clear(b);
}
```
（`list_under` 用 `Path::starts_with`，按分量比较，`/tmp/issues-t1-other` 不算在 `/tmp/issues-t1` 下。）

format.rs：
```rust
#[test]
fn bad_lines_are_skipped_and_counted() {
    let d = p(&format!("{HEAD}你好\tni hao\t100\n坏\n缺码\t\t3\n权重坏\tquan\tabc\n好\thao\t1\n"), false);
    let texts: Vec<&str> = d.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, vec!["你好", "好"]);
    assert_eq!(d.bad_lines, 3, "列不够、码空、权重不是数字各算一行");
}
```
注意：空权重（`西安\txi an\t`）仍是合法行、权重 0（已有测试 `parses_header_entries_and_boundaries` 守）。

engine.rs：
```rust
#[test]
fn text_bad_lines_are_reported_to_issues() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    let f = d.path().join("txt/s/user.dict.yaml");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, "---\ncolumns: [text, code, weight]\n...\n好\thao\t1\n坏\n").unwrap();
    ud.bind_text(Target::User, "s", words_binding(&f)).unwrap();
    assert_eq!(
        crate::issues::list_under(d.path()),
        vec![crate::issues::FileIssue { path: f.clone(), kind: crate::issues::IssueKind::BadLines(1) }]
    );
    std::fs::write(&f, "---\ncolumns: [text, code, weight]\n...\n好\thao\t1\n").unwrap();
    ud.sync_text(Target::User, "s").unwrap();
    assert!(crate::issues::list_under(d.path()).is_empty(), "修好就清");
}
```
（`words_binding(path)` 是测试辅助：`TextBinding { path, name: "s.user".into(), lowercase: false, syllable_codes: false, format: TextFormat::Words }`——`format` 字段 Task 4 才加；本任务里先不带 `format`，Task 4 统一补。）

- [ ] **Step 2: 跑，确认失败**：`cargo test -p wind-userdata issues bad_lines_are_skipped text_bad_lines_are_reported` → 编译失败。

- [ ] **Step 3: 实现**

issues.rs：
```rust
//! 数据文件问题登记（设计 §2.5）：坏行数、隔离、已重建 / 未迁移。进程级，按绝对路径为键；
//! 设置页经 `system.storageStatus` 读。只记路径与计数，不记文件内容。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ISSUES: Mutex<BTreeMap<PathBuf, IssueKind>> = Mutex::new(BTreeMap::new());

fn lock() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, IssueKind>> {
    ISSUES.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn report(path: &Path, kind: IssueKind) { lock().insert(path.to_path_buf(), kind); }
pub fn clear(path: &Path) { lock().remove(path); }
pub fn list_under(root: &Path) -> Vec<FileIssue> {
    lock().iter().filter(|(p, _)| p.starts_with(root))
        .map(|(p, k)| FileIssue { path: p.clone(), kind: k.clone() }).collect()
}
/// 解析结果的统一上报：有坏行记 WARN（只记行数）+ 登记；没有就清。
pub fn report_parse(path: &Path, bad_lines: usize) {
    if bad_lines > 0 {
        tracing::warn!("{} 有 {bad_lines} 行无法解析，已跳过（原样保留在文件里）", path.display());
        report(path, IssueKind::BadLines(bad_lines));
    } else {
        clear(path);
    }
}
```

format.rs：`parse` 里，跳过「列不够 / text 空 / code 空」时 `bad_lines += 1`（空行、注释不算）；权重列存在且非空且 `parse::<i32>()` 失败 ⇒ `bad_lines += 1; continue`（原来是当 0）。`TextDict` 加 `pub bad_lines: usize`。

engine.rs `text_entries`：解析成功后 `crate::issues::report_parse(&b.path, dict.bad_lines)`；解析失败分支 `crate::issues::report(&b.path, IssueKind::Isolated)`。`sync_text` 里 `cur.is_none()`（文件不存在）⇒ `crate::issues::clear(&b.path)`。

- [ ] **Step 4: 跑，确认通过**；再跑 `cargo test -p wind-userdata` 全部 → PASS（若有测试依赖「坏权重当 0」，按新规则改其期望并在账本记 Ruling）。
- [ ] **Step 5: 提交** `feat(userdata): 文件问题登记表，用户词坏行跳过并计数`

---

### Task 3: 学到的数据的表格编解码

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/codec.rs`（`text/mod.rs` 加 `pub mod codec;`）

**Interfaces:**
- Consumes: `text::table::{parse_table, render_table, TableDoc}`、`text::format::normalize_key`、`wind_dict::join_code_by_boundary`（P2a 渲染用的同一个函数，以 `text/patch.rs` 里的引用为准）、`SnapshotEntry`
- Produces:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field { Code, Context, Text, Weight, Count, CreatedAt, LastUsed }
#[derive(Debug)]
pub struct TableSpec { pub kind: &'static str, pub columns: &'static [(&'static str, Field)] }
pub static TEMP: TableSpec;   // kind "temp":  text, code, weight, count, created_at
pub static DRAFT: TableSpec;  // kind "draft": text, code, last_used
pub static FREQ: TableSpec;   // kind "freq":  code, text, count, last_used
pub static ASSOC: TableSpec;  // kind "assoc": context, text, count, last_used
pub struct Decoded { pub entries: Vec<SnapshotEntry>, pub bad_lines: usize }
pub fn decode(bytes: &[u8], path: &Path, spec: &TableSpec, lowercase: bool, syllable: bool)
    -> Result<Decoded, ParseError>;               // 按 (code,text) 排序去重，取文件里第一行
pub fn encode_row(spec: &TableSpec, e: &SnapshotEntry, syllable: bool) -> Vec<String>;
pub fn render(spec: &TableSpec, syllable: bool, entries: &[SnapshotEntry]) -> String;
pub fn patch(bytes: &[u8], path: &Path, spec: &TableSpec, lowercase: bool, syllable: bool,
             edits: &BTreeMap<(String, String), Option<SnapshotEntry>>) -> Result<Vec<u8>, ParseError>;
```
语义：
- `Field::Code`：读时 `normalize_key(code, text, lowercase)` 得扁平码与边界；`syllable` 为真且边界 0、文本是单个汉字 ⇒ 0b1（同 format.rs）；写时 `syllable` 为真用 `join_code_by_boundary(code, boundary)`，否则原样。
- `Field::Context`：原样读写（上文，不规范化），存进 `SnapshotEntry.code`，边界恒 0。
- 数字列空串 ⇒ 0；非空且解析失败 ⇒ 坏行。`Code`/`Context`/`Text` 为空 ⇒ 坏行。
- `patch`：空文件 ⇒ 先 `render(spec, syllable, &[])` 得表头。逐行解码得键（坏行无键、不动）；`edits` 里 `Some(e)` 的键：第一处出现的行换成新行、其余同键行删除，文件里没有则追加末尾；`None` 的键：同键行全删。行替换按文件**自身**列序输出（文件少列丢弃、多出的列留空）。保留 BOM、CRLF。一次线性扫描 + `HashMap` 查找。

- [ ] **Step 1: 写失败测试**（codec.rs tests）

```rust
fn e(code: &str, text: &str, w: i32, count: u32, created: i64, last: i64, boundary: u64) -> SnapshotEntry {
    SnapshotEntry { code: code.into(), text: text.into(), weight: w, boundary, count,
        created_at: created, last_used: last, flags: 0, order: 0 }
}

#[test]
fn temp_round_trips_through_render_and_decode() {
    let v = vec![e("nihao", "你好", 3, 2, 1700000000, 1700000000, 0b101)];
    let s = render(&TEMP, true, &v);
    assert!(s.contains("你好\tni hao\t3\t2\t1700000000"));
    let d = decode(s.as_bytes(), Path::new("temp.dict.yaml"), &TEMP, false, true).unwrap();
    assert_eq!(d.bad_lines, 0);
    assert_eq!(d.entries[0].count, 2);
    assert_eq!(d.entries[0].created_at, 1700000000);
    assert_eq!(d.entries[0].boundary, 0b101);
}

#[test]
fn assoc_context_is_kept_verbatim() {
    let v = vec![e("我们 的", "我们的事", 0, 5, 0, 1700000001, 0)];
    let s = render(&ASSOC, false, &v);
    let d = decode(s.as_bytes(), Path::new("assoc.table.yaml"), &ASSOC, false, false).unwrap();
    assert_eq!(d.entries[0].code, "我们 的", "上文不去空格、不小写");
}

#[test]
fn bad_rows_are_skipped_counted_and_left_untouched_by_patch() {
    let head = render(&FREQ, false, &[]);
    let src = format!("{head}a\t啊\t3\t10\nb\t不\tx\t10\n\t空码\t1\t1\n");
    let d = decode(src.as_bytes(), Path::new("freq.table.yaml"), &FREQ, false, false).unwrap();
    assert_eq!(d.entries.len(), 1);
    assert_eq!(d.bad_lines, 2);
    let mut edits = BTreeMap::new();
    edits.insert(("a".to_string(), "啊".to_string()), Some(e("a", "啊", 0, 4, 0, 20, 0)));
    let out = String::from_utf8(patch(src.as_bytes(), Path::new("freq.table.yaml"), &FREQ, false, false, &edits).unwrap()).unwrap();
    assert!(out.contains("a\t啊\t4\t20\n"));
    assert!(out.contains("b\t不\tx\t10\n"), "坏行原样保留");
    assert!(out.contains("\t空码\t1\t1\n"));
}

#[test]
fn patch_deletes_all_duplicates_and_appends_new_keys() {
    let head = render(&FREQ, false, &[]);
    let src = format!("{head}# 我的注释\na\t啊\t1\t1\na\t啊\t2\t2\n");
    let mut edits = BTreeMap::new();
    edits.insert(("a".to_string(), "啊".to_string()), None);
    edits.insert(("b".to_string(), "不".to_string()), Some(e("b", "不", 0, 1, 0, 5, 0)));
    let out = String::from_utf8(patch(src.as_bytes(), Path::new("f"), &FREQ, false, false, &edits).unwrap()).unwrap();
    assert!(!out.contains("啊"));
    assert!(out.contains("# 我的注释\n"));
    assert!(out.ends_with("b\t不\t1\t5\n"));
}

#[test]
fn patch_keeps_crlf_and_bom() {
    let head = render(&FREQ, false, &[]).replace('\n', "\r\n");
    let src = format!("\u{feff}{head}a\t啊\t1\t1\r\n");
    let mut edits = BTreeMap::new();
    edits.insert(("a".to_string(), "啊".to_string()), Some(e("a", "啊", 0, 2, 0, 2, 0)));
    let out = String::from_utf8(patch(src.as_bytes(), Path::new("f"), &FREQ, false, false, &edits).unwrap()).unwrap();
    assert!(out.starts_with('\u{feff}'));
    assert!(out.contains("a\t啊\t2\t2\r\n"));
}

#[test]
fn patch_on_200k_rows_is_linear() {
    let v: Vec<SnapshotEntry> = (0..200_000).map(|i| e(&format!("c{i}"), &format!("t{i}"), 0, 1, 0, 1, 0)).collect();
    let src = render(&FREQ, false, &v);
    let mut edits = BTreeMap::new();
    for i in (0..200_000).step_by(100) {
        edits.insert((format!("c{i}"), format!("t{i}")), Some(e(&format!("c{i}"), &format!("t{i}"), 0, 9, 0, 9, 0)));
    }
    let t = std::time::Instant::now();
    patch(src.as_bytes(), Path::new("f"), &FREQ, false, false, &edits).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
}
```

- [ ] **Step 2: 跑，确认失败**：`cargo test -p wind-userdata codec` → 编译失败。
- [ ] **Step 3: 实现** `codec.rs`：`decode` 基于 `parse_table`（它给出 `fields` 与 `line`），按 `spec.columns` 的列名在 `doc.columns` 里找下标（文件缺某列 ⇒ 该字段当空串 / 0）；`render` 用 `render_table(columns, kind, rows)`；`patch` 自己做一遍按行处理（参照 `table::patch_groups` 的 BOM / CRLF / `ends_with_nl` 处理，但用 `HashMap<(String,String), usize>` 记「该键第一次出现的行」，单趟完成）。`kind` 头：`wind: {kind: <spec.kind>, escape: true}`；`syllable` 为真时头部再加 `code_spacing: syllable`（`render_table` 不支持就在 codec 里自行拼头部字符串，格式与 `render_table` 一致）。
- [ ] **Step 4: 跑，确认通过**：`cargo test -p wind-userdata codec` → 6 passed。
- [ ] **Step 5: 提交** `feat(userdata): 学到的数据的表格编解码（行级跳过计数、按键线性补丁）`

---

### Task 4: 引擎支持表格文本目标

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`
- Modify: `wind_input/crates/wind-userdata/src/text/fold.rs`（`preserve_corrupt`）
- Modify: 所有构造 `TextBinding` 的地方加 `format: TextFormat::Words`（`wind-store/src/store.rs` `ensure_user_text`、各测试辅助）

**Interfaces:**
- Consumes: Task 2 `issues::*`，Task 3 `codec::*`
- Produces:
```rust
#[derive(Clone, Copy, Debug)]
pub enum TextFormat { Words, Table(&'static crate::text::codec::TableSpec) }
pub struct TextBinding { pub path, pub name, pub lowercase, pub syllable_codes, pub format: TextFormat }
pub fn fold::preserve_corrupt(path: &Path) -> std::io::Result<Option<PathBuf>>; // 改名 <name>.corrupt-<YYYYMMDD-HHMMSS>，返回新名
pub fn UserData::text_isolated(&self, target: Target, schema: &str) -> bool;    // 现在读文件判一次
```
行为：
- `text_entries` 按 `b.format` 分派：`Words` 走原逻辑；`Table(spec)` 走 `codec::decode(bytes, path, spec, b.lowercase, b.syllable_codes)`，坏行 `issues::report_parse`。
- `fold_text_locked` 对 `Table`：取待处理操作涉及的键集合；由**当前文件**解码出这些键的条目，用 `apply_ops`（按 seq）叠加得到终值（`None` = 删）；`codec::patch` 写回（带 `fp_before` 复核）；Incr / Touch **不再丢弃**。`Words` 保持原逻辑。
- `bind_text` 采纳：未绑定、state 没有 `text_fp`、文件不存在、但视图里有该目标的段或 delta ⇒ 先在 `fold_lock` 下取 `(upto, view)`，`merge_for_fold` 得全集，按 `b.format` 渲染（`Words` 用 `text::render`，`Table` 用 `codec::render`）写文件，再 `install_text_segment(..., Some(upto), fp, Some(upto))`；之后照常登记绑定。
- `fold_with` 遇到归文本管的目标：`Words` 照旧 bail；`Table` ⇒ 先 `fold_text_locked`（失败即返回 Err，不往下走——文件坏时自动清理不得覆盖），再由新段条目跑 `policy`，`codec::render` 整写（不复核，用户显式动作 / 已在 fold_lock 下刚复核过），装段推进水位。
- `rewrite_text`（用户词清空 / 导入 / 还原）与新的 `replace_text_entries(target, schema, entries: Vec<SnapshotEntry>)`（表格目标的显式整体替换）：当前文件存在且解析失败 ⇒ 先 `preserve_corrupt` 留存、`issues::report(path, Rebuilt{kept_as})`，再写新文件。
- `check_text_writable` 按格式分派；报错文案改为「数据文件 … 无法解析（…），请先修复或删除它」。

- [ ] **Step 1: 写失败测试**（engine.rs tests）

```rust
fn table_binding(path: &Path, spec: &'static TableSpec) -> TextBinding {
    TextBinding { path: path.into(), name: "s.x".into(), lowercase: false, syllable_codes: false,
        format: TextFormat::Table(spec) }
}

#[test]
fn table_target_folds_counts_into_text() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    let f = d.path().join("data/s/freq.table.yaml");
    ud.bind_text(Target::Freq, "s", table_binding(&f, &codec::FREQ)).unwrap();
    for _ in 0..3 {
        ud.apply(&[Op::new(Target::Freq, "s", OpKind::Incr, "a", "啊", Payload::Count(1), 0)]).unwrap();
    }
    ud.fold_text(Target::Freq, "s").unwrap();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("a\t啊\t3\t"), "{s}");
    assert_eq!(ud.view().delta.pending(Target::Freq, "s"), 0);
}

#[test]
fn bind_adopts_existing_view_when_text_missing() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    ud.apply(&[Op::new(Target::Temp, "s", OpKind::Upsert, "ab", "阿布", Payload::Weight(5), 0),
               Op::new(Target::Temp, "s", OpKind::Incr, "ab", "阿布", Payload::Count(2), 0)]).unwrap();
    ud.fold_with(Target::Temp, "s", &mut |v| v).unwrap(); // 模拟 v5：数据只在快照里
    let f = d.path().join("data/s/temp.dict.yaml");
    ud.bind_text(Target::Temp, "s", table_binding(&f, &codec::TEMP)).unwrap();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("阿布\tab\t5\t2\t"), "{s}");
    let snap = ud.view().snapshots.get(&(Target::Temp, "s".into())).cloned().unwrap();
    assert_eq!(snap.len(), 1, "绑定后段仍是那一条，没有被空文件清掉");
}

#[test]
fn deleting_state_dir_rebuilds_learned_target_from_text() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("data/s/freq.table.yaml");
    {
        let ud = open_ud(d.path());
        ud.bind_text(Target::Freq, "s", table_binding(&f, &codec::FREQ)).unwrap();
        ud.apply(&[Op::new(Target::Freq, "s", OpKind::Incr, "a", "啊", Payload::Count(4), 0)]).unwrap();
        ud.fold_text(Target::Freq, "s").unwrap();
    }
    std::fs::remove_dir_all(state_dir_of(d.path())).unwrap();
    let ud = open_ud(d.path());
    ud.bind_text(Target::Freq, "s", table_binding(&f, &codec::FREQ)).unwrap();
    let r = crate::runtime::Runtime::new(std::sync::Arc::new(ud));
    assert_eq!(r.freq("s", "a", "啊").map(|x| x.count), Some(4));
}

#[test]
fn isolated_learned_text_keeps_learning_and_merges_after_fix() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    let f = d.path().join("data/s/freq.table.yaml");
    ud.bind_text(Target::Freq, "s", table_binding(&f, &codec::FREQ)).unwrap();
    ud.apply(&[Op::new(Target::Freq, "s", OpKind::Incr, "a", "啊", Payload::Count(1), 0)]).unwrap();
    ud.fold_text(Target::Freq, "s").unwrap();
    let good = std::fs::read(&f).unwrap();
    std::fs::write(&f, b"\xff\xfe not utf8").unwrap();
    ud.apply(&[Op::new(Target::Freq, "s", OpKind::Incr, "a", "啊", Payload::Count(1), 0)]).unwrap();
    assert!(ud.fold_text(Target::Freq, "s").is_err());
    assert_eq!(std::fs::read(&f).unwrap(), b"\xff\xfe not utf8", "坏文件不被覆盖");
    assert!(ud.text_isolated(Target::Freq, "s"));
    std::fs::write(&f, &good).unwrap();
    ud.sync_text(Target::Freq, "s").unwrap();
    ud.fold_text(Target::Freq, "s").unwrap();
    assert!(std::fs::read_to_string(&f).unwrap().contains("a\t啊\t2\t"), "修好后合入");
}

#[test]
fn auto_purge_never_rewrites_isolated_text() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    let f = d.path().join("data/s/draft.dict.yaml");
    ud.bind_text(Target::Draft, "s", table_binding(&f, &codec::DRAFT)).unwrap();
    ud.apply(&[Op::new(Target::Draft, "s", OpKind::Upsert, "a", "啊", Payload::Weight(0), 0)]).unwrap();
    ud.fold_text(Target::Draft, "s").unwrap();
    std::fs::write(&f, b"---\nbroken: [\n").unwrap();
    assert!(ud.fold_with(Target::Draft, "s", &mut |_| Vec::new()).is_err());
    assert_eq!(std::fs::read(&f).unwrap(), b"---\nbroken: [\n");
}

#[test]
fn explicit_replace_preserves_corrupt_file() {
    let d = tempfile::tempdir().unwrap();
    let ud = open_ud(d.path());
    let f = d.path().join("data/s/temp.dict.yaml");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, b"\xff bad").unwrap();
    ud.bind_text(Target::Temp, "s", table_binding(&f, &codec::TEMP)).unwrap();
    ud.replace_text_entries(Target::Temp, "s", Vec::new()).unwrap();
    let kept: Vec<_> = std::fs::read_dir(f.parent().unwrap()).unwrap().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("temp.dict.yaml.corrupt-")).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"\xff bad");
}
```
（`state_dir_of(root)` 返回 `open_ud` 用的引擎状态目录；`open_ud` 的状态目录与 `data/` 分开放。辅助名以 engine.rs 测试模块里现有的为准，没有就加。）

- [ ] **Step 2: 跑，确认失败**：`cargo test -p wind-userdata table_target_ bind_adopts deleting_state_dir isolated_learned auto_purge explicit_replace` → 编译失败。
- [ ] **Step 3: 实现**（按上面「行为」逐条）。`text_isolated`：有绑定且文件存在且按格式解析失败。`preserve_corrupt` 时间戳用 `chrono` 若已是依赖，否则用 `SystemTime` 手算 UTC `YYYYMMDD-HHMMSS`（wind-userdata 现有依赖里有就用，不新增）。
- [ ] **Step 4: 跑，确认通过**；再跑 `cargo test -p wind-userdata` 与 `cargo test -p wind-store` → PASS。
- [ ] **Step 5: 提交** `feat(userdata): 表格文本目标（计数与时间进文本、采纳已有视图、坏文件不被覆盖）`

---

### Task 5: 自动绑定规则

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`
- Create: `wind_input/crates/wind-store/src/learned_text.rs`
- Modify: `wind_input/crates/wind-store/src/store.rs`（打开时设规则、绑定已有）

**Interfaces:**
- Produces:
```rust
pub type TextRule = Box<dyn Fn(Target, &str) -> Option<TextBinding> + Send + Sync>;
pub fn UserData::set_text_rule(&self, rule: TextRule);
pub fn UserData::bind_by_rule(&self, target: Target, schema: &str) -> anyhow::Result<bool>; // 已绑定 / 规则不管 ⇒ Ok(false)
// wind-store
pub(crate) fn learned_binding(data_dir: &Path, target: Target, schema: &str) -> Option<TextBinding>;
pub(crate) const LEARNED_TARGETS: [Target; 4] = [Temp, Draft, Freq, Assoc];
```
- 规则：`Temp` → `data/<schema>/temp.dict.yaml`（`TEMP`）、`Draft` → `draft.dict.yaml`（`DRAFT`）、`Freq` → `freq.table.yaml`（`FREQ`）、`Assoc` → `assoc.table.yaml`（`ASSOC`）；`syllable_codes = schema == "pinyin"`（Assoc 恒 false），`lowercase = false`，`name = "<schema>.<target>"`。
- 引擎在三处按规则绑定未绑定的键：`maintain_with` 开头（对 delta 里有待处理操作的键）、`fold_with` 开头、`needs_maintain` 为真时的节拍（已含在 maintain 里）。
- Store 打开：`set_text_rule` 后，对「引擎里有该目标数据的方案 ∪ `data/` 下已有对应文件的方案」逐个 `bind_by_rule`（失败只 WARN）。

- [ ] **Step 1: 写失败测试**

engine.rs：
```rust
#[test]
fn rule_binds_target_on_maintain() {
    let d = tempfile::tempdir().unwrap();
    let ud = std::sync::Arc::new(open_ud(d.path()));
    let root = d.path().join("data");
    ud.set_text_rule(Box::new(move |t, s| (t == Target::Freq).then(|| table_binding(&root.join(s).join("freq.table.yaml"), &codec::FREQ))));
    ud.set_text_idle(std::time::Duration::ZERO);
    ud.apply(&[Op::new(Target::Freq, "s", OpKind::Incr, "a", "啊", Payload::Count(1), 0)]).unwrap();
    ud.maintain().unwrap();
    assert!(d.path().join("data/s/freq.table.yaml").is_file());
}
```
（`set_text_idle` 用 engine 里已有的测试用空闲时长设置；名字以代码为准。）

wind-store `learned_text.rs`：
```rust
#[test]
fn learned_files_live_under_data_schema_dir() {
    let b = learned_binding(Path::new("/u/data"), Target::Assoc, "pinyin").unwrap();
    assert_eq!(b.path, Path::new("/u/data/pinyin/assoc.table.yaml"));
    assert!(!b.syllable_codes);
    assert!(learned_binding(Path::new("/u/data"), Target::User, "pinyin").is_none());
}
```
store.rs：
```rust
#[test]
fn learned_data_is_written_to_data_dir() {
    let (_d, store) = temp_store("learned_data_is_written");
    store.learn_temp_word("pinyin", "nihao", "你好", 0b101, 10).unwrap(); // 以 Store 门面实际签名为准
    store.runtime().data().set_text_idle(std::time::Duration::ZERO);
    store.runtime().data().maintain().unwrap();
    assert!(store.user_text_dir().join("pinyin/temp.dict.yaml").is_file());
}
```

- [ ] **Step 2: 跑，确认失败** → 编译失败。
- [ ] **Step 3: 实现**（`text_rule: Mutex<Option<Arc<TextRule>>>`，调用规则时不持锁；`bind_by_rule` 内调 `bind_text`）。
- [ ] **Step 4: 跑，确认通过**；`cargo test -p wind-userdata -p wind-store` → PASS。
- [ ] **Step 5: 提交** `feat(store): 学到的数据按规则绑定到 data/<方案>/ 下的文本`

---

### Task 6: `data/` 目录名与 `dict.reload` / 同步

**Files:**
- Modify: `wind_input/crates/wind-store/src/lib.rs`（`pub const DATA_DIR_NAME: &str = "data";`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（三处 `join("user")` → `join(wind_store::DATA_DIR_NAME)`）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（`classify_text_path`、`reload_user_text_paths`、`sync_user_texts`）
- 测试里写死 `"user"` 的：`store.rs`、`phrases.rs`、`construct.rs` 测试按新名改

**Interfaces:**
- `TextPath` 增加 `Learned(Target, String)`、`Completion`；`Quick` 改认 `quick_format.toml`。
- `sync_user_texts`（启动 / 切方案 / 配置重载）对已绑定的学习目标也 `sync_text`，变了就 bump `words_gen`（同用户词）。

- [ ] **Step 1: 写失败测试**（user_words.rs）

```rust
#[test]
fn reload_accepts_learned_and_completion_paths() {
    let base = Path::new("/u/data");
    assert!(matches!(classify_text_path(base, &base.join("pinyin/freq.table.yaml"), false).unwrap(),
        TextPath::Learned(Target::Freq, s) if s == "pinyin"));
    assert!(matches!(classify_text_path(base, &base.join("pinyin/temp.dict.yaml"), false).unwrap(),
        TextPath::Learned(Target::Temp, _)));
    assert!(matches!(classify_text_path(base, &base.join("completion.table.yaml"), false).unwrap(), TextPath::Completion));
    assert!(matches!(classify_text_path(base, &base.join("quick_format.toml"), false).unwrap(), TextPath::Quick));
    assert!(classify_text_path(base, &base.join("quick_format.yaml"), false).is_err());
}

#[test]
fn reload_of_hand_edited_freq_text_takes_effect() {
    let (_d, store) = temp_store("reload_freq");
    store.record_freq("pinyin", "a", "啊").unwrap(); // 门面签名以代码为准
    store.flush_userdata().unwrap();                  // dict.flush 对应的方法，以代码为准
    let f = store.user_text_dir().join("pinyin/freq.table.yaml");
    let s = std::fs::read_to_string(&f).unwrap().replace("a\t啊\t1\t", "a\t啊\t9\t");
    std::fs::write(&f, s).unwrap();
    store.reload_user_text_paths(&[f.to_string_lossy().into_owned()]).unwrap();
    assert_eq!(store.get_freq("pinyin", "a", "啊").map(|r| r.count), Some(9));
}
```

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现**；`classify_text_path` 的文件名表：`<schema>/user.dict.yaml` → Words，`shadow.dict.yaml` → Shadow，`temp.dict.yaml` / `draft.dict.yaml` / `freq.table.yaml` / `assoc.table.yaml` → Learned，`completion.table.yaml` → Completion（重读内存表，Task 7 提供 `reload_completions()`；本任务先接到现有的重新加载入口，Task 7 换实现）。
- [ ] **Step 4: 跑**：`cargo test -p wind-store -p wind-coordinator -p wind-webdata` → PASS。
- [ ] **Step 5: 提交** `refactor(store): 用户数据目录改名 data/，dict.reload 认学到的数据与补全文件`

---

### Task 7: 补全历史 → `data/completion.table.yaml`（机写策略，修 M-2）

**Files:**
- Create: `wind_input/crates/wind-store/src/machine_file.rs`
- Modify: `wind_input/crates/wind-store/src/completion.rs`

**Interfaces:**
- Produces（machine_file.rs）：
```rust
/// 读机写文件：解析成功 ⇒ Ok(Some(v))；不存在 ⇒ Ok(None)；解析失败 ⇒ 试 `.bak`（成功则把 .bak
/// 内容写回原文件并登记 Rebuilt{kept_as: ".bak"}）；`.bak` 也不行 ⇒ 坏文件 `preserve_corrupt`、
/// 登记 Rebuilt{kept_as}、返回 Ok(None)（调用方从空开始）。IO 错误 ⇒ Err。
pub(crate) fn load_machine_file<T>(path: &Path, parse: &dyn Fn(&[u8]) -> anyhow::Result<T>) -> anyhow::Result<Option<T>>;
```
- 补全文件：`render_table(&["kind","text","count","last_used"], "completion", rows)`；读用 `parse_table` + 行级跳过计数（未知 kind、数字坏 ⇒ 坏行）。`completion_path()` → `<text_dir>/completion.table.yaml`。`isolated` 字段删除：机写文件不再有「隔离」，只剩「读出错（IO）⇒ 本次不写，下次 flush 重试读」。
- `reload_completions(&self)`：丢弃内存、标记未载入（dict.reload 用）。若内存 `dirty` 先 flush。

- [ ] **Step 1: 写失败测试**（completion.rs）

```rust
#[test]
fn corrupt_completion_self_heals_from_bak() {
    let (_d, store) = temp_store("completion_heal");
    store.record_completion(CompletionKind::EmailSuffix, "qq.com").unwrap(); // 以实际 API 为准
    store.flush_completions().unwrap();
    store.record_completion(CompletionKind::EmailSuffix, "163.com").unwrap();
    store.flush_completions().unwrap(); // 第二次写，.bak = 第一次的内容
    let p = store.completion_path();
    std::fs::write(&p, b"\xff garbage").unwrap();
    let store2 = reopen(&store);
    assert!(store2.completions_of(CompletionKind::EmailSuffix).iter().any(|t| t == "qq.com"));
    store2.record_completion(CompletionKind::EmailSuffix, "gmail.com").unwrap();
    store2.flush_completions().unwrap();
    assert!(std::fs::read_to_string(&p).unwrap().contains("gmail.com"), "M-2：坏文件之后照常学习、照常落盘");
}

#[test]
fn completion_without_good_bak_is_kept_aside_and_rebuilt() {
    let (_d, store) = temp_store("completion_rebuild");
    let p = store.completion_path();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"\xff garbage").unwrap();
    store.record_completion(CompletionKind::UrlHistory, "example.com").unwrap();
    store.flush_completions().unwrap();
    let kept = std::fs::read_dir(p.parent().unwrap()).unwrap().flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("completion.table.yaml.corrupt-"));
    assert!(kept);
    assert!(std::fs::read_to_string(&p).unwrap().contains("example.com"));
}

#[test]
fn completion_file_is_table_text_in_data_dir() {
    let (_d, store) = temp_store("completion_table");
    store.record_completion(CompletionKind::QuickHistory, "a\tb").unwrap();
    store.flush_completions().unwrap();
    let s = std::fs::read_to_string(store.completion_path()).unwrap();
    assert!(store.completion_path().ends_with("completion.table.yaml"));
    assert!(s.contains("kind: completion"));
    assert!(s.contains("quick_history\ta\\tb\t1\t"), "{s}");
}
```

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 跑**：`cargo test -p wind-store completion` → PASS；`cargo test -p wind-store -p wind-transfer -p wind-webdata` → PASS（备份 / 还原补全的测试不改即过：走 Store API）。
- [ ] **Step 5: 提交** `feat(store): 补全历史改为 data/completion.table.yaml，坏文件自愈不再停学`

---

### Task 8: 统计 → `data/stats/*.toml`（机写策略，修 M-3）

**Files:**
- Modify: `wind_input/crates/wind-store/src/stats.rs`
- Modify: `wind_input/crates/wind-store/Cargo.toml`（`toml = { workspace = true }`，工作区已有此依赖，不是新依赖）

**Interfaces:**
- `stats_dir()` → `<text_dir>/stats`；月文件 `<ym>.toml`（`toml::to_string(&MonthFile)`），元数据 `meta.toml`。
- `read_month` 走 `load_machine_file`；坏且无好 `.bak` ⇒ 留存、该月从空开始（不再让 flush 永远失败）。
- `daily_stats`（区间查询）遇到某月读失败（IO）⇒ WARN 跳过该月，不让整个区间失败。
- `get_stats_meta`：文件不存在 ⇒ 默认；坏 ⇒ `load_machine_file`，`.bak` 也不行 ⇒ 由 `recalculate_stats_meta()` 重算并写回（**不再 `unwrap_or_default`**）。
- `stat_months` 按 `.toml` 后缀筛（`.toml.bak`、`.corrupt-*` 不算）。

- [ ] **Step 1: 写失败测试**（stats.rs）

```rust
#[test]
fn month_file_is_toml_in_data_dir() {
    let (_d, store) = temp_store("stats_toml");
    store.put_daily_stat("2026-10-15", &sample_day()).unwrap(); // 以实际 API 为准
    let p = store.stats_dir().join("2026-10.toml");
    assert!(p.is_file());
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert!(v.get("days").is_some());
}

#[test]
fn corrupt_month_is_rebuilt_and_flush_keeps_working() {
    let (_d, store) = temp_store("stats_corrupt_month");
    let p = store.stats_dir().join("2026-10.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "days = [[[").unwrap();
    store.put_daily_stat("2026-10-15", &sample_day()).unwrap();
    assert!(store.get_daily_stat("2026-10-15").unwrap().is_some());
    assert!(std::fs::read_dir(p.parent().unwrap()).unwrap().flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("2026-10.toml.corrupt-")));
}

#[test]
fn corrupt_stats_meta_is_recalculated_not_zeroed() {
    let (_d, store) = temp_store("stats_meta_recalc");
    store.put_daily_stat("2026-10-14", &sample_day()).unwrap();
    store.recalculate_stats_meta().unwrap();
    let before = store.get_stats_meta().unwrap();
    assert!(before.total_chars > 0);
    std::fs::write(store.stats_dir().join("meta.toml"), "total_chars = \"x").unwrap();
    std::fs::remove_file(store.stats_dir().join("meta.toml.bak")).ok();
    let after = store.get_stats_meta().unwrap();
    assert_eq!(after.total_chars, before.total_chars, "M-3：坏元数据重算，不归零");
}

#[test]
fn range_query_skips_unreadable_month() {
    let (_d, store) = temp_store("stats_range_skip");
    store.put_daily_stat("2026-09-01", &sample_day()).unwrap();
    store.put_daily_stat("2026-10-01", &sample_day()).unwrap();
    std::fs::write(store.stats_dir().join("2026-09.toml"), "days = [[[").unwrap();
    std::fs::remove_file(store.stats_dir().join("2026-09.toml.bak")).ok();
    let v = store.daily_stats("2026-09-01", "2026-10-31").unwrap();
    assert!(v.iter().any(|(d, _)| d == "2026-10-01"));
}
```
（`sample_day()` 构造一个 `chinese = 100`、`hours[9] = 100`、`by_schema` 含一项的 `DailyStats`，覆盖嵌套字段的 TOML 往返。）

现有 `stats.rs:1073-1078` 断言 `2026-10.json` 的测试改为 `.toml`。

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现。**`HashMap` 字段（`by_schema`）的 TOML 序列化需键为字符串，已满足；`[u32; 24]` 数组 TOML 可往返；`u64` 字段超过 `i64::MAX` 时 `toml` 报错——统计值不会到这个量级，序列化失败按 IO 错误处理（WARN、本轮不写）。
- [ ] **Step 4: 跑**：`cargo test -p wind-store stats` 与 `cargo test -p wind-coordinator stat` → PASS。
- [ ] **Step 5: 提交** `feat(store): 统计改为 data/stats/*.toml，坏月文件自愈、坏元数据重算不归零`

---

### Task 9: 快捷格式 TOML + 覆盖表解析失败不重写（修 M-1）

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/text/doc.rs`（`modify` 闭包签名）
- Modify: `wind_input/crates/wind-store/src/quick_format.rs`
- Modify: `wind_input/crates/wind-store/src/common_chars.rs`

**Interfaces:**
- `TextDoc::modify(&self, f: &mut dyn FnMut(Option<&str>) -> anyhow::Result<Option<String>>) -> anyhow::Result<()>`：闭包 Err ⇒ 不提交、原样返回 Err。
- `quick_format_path()` → `<text_dir>/quick_format.toml`；内容 `QUICK_HEADER`（改成 `#` 注释的 TOML 说明）+ `toml::to_string(&QuickFile)`。
- 「最后一次好数据」：`Store` 为快捷格式与常用字各缓存 `(fp, 解析结果)`；文件指纹变了才重解析，解析失败 ⇒ 继续用缓存、`issues::report(path, Isolated)`，修好 ⇒ `issues::clear`。
- 隔离期间写入 ⇒ `Err("文件 … 格式有误，请先修复")`（设置页把错误显示给用户）。

- [ ] **Step 1: 写失败测试**

doc.rs：
```rust
#[test]
fn modify_error_leaves_file_and_bak_untouched() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.toml");
    std::fs::write(&p, "a = 1\n").unwrap();
    let doc = TextDoc::open(p.clone());
    doc.modify(&mut |_| Ok(Some("a = 2\n".into()))).unwrap(); // .bak = "a = 1"
    std::fs::write(&p, "a = [\n").unwrap();
    let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
    assert!(doc.modify(&mut |_| Err(anyhow::anyhow!("坏"))).is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "a = [\n");
    assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime, "M-1：不重写");
    assert_eq!(std::fs::read_to_string(d.path().join("x.toml.bak")).unwrap(), "a = 1\n", "好的 .bak 不被坏内容覆盖");
}
```
quick_format.rs：
```rust
#[test]
fn quick_format_is_toml_and_survives_broken_edit() {
    let (_d, store) = temp_store("quick_toml");
    store.disable_quick_format("date", "lunar").unwrap(); // 以实际 API 为准
    let p = store.quick_format_path();
    assert!(p.ends_with("quick_format.toml"));
    let txt = std::fs::read_to_string(&p).unwrap();
    assert!(toml::from_str::<toml::Value>(&txt).is_ok(), "{txt}");
    std::fs::write(&p, "kinds = [[[").unwrap();
    store.sync_quick_format();
    assert_eq!(store.quick_format_record("date").disabled, vec!["lunar".to_string()], "按最后一次好数据服务");
    assert!(store.disable_quick_format("date", "x").is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "kinds = [[[");
}
```
common_chars.rs：对应一条 `common_chars_broken_edit_keeps_serving_and_is_not_rewritten`（同结构，YAML）。

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现**（`modify_quick_file` / `modify_common` 解析失败分支 `return Err(e)`；`TextDoc::modify` 的其他调用方按新签名包 `Ok(..)`）。
- [ ] **Step 4: 跑**：`cargo test -p wind-userdata doc` `cargo test -p wind-store quick common` `cargo test -p wind-transfer -p wind-webdata quick` → PASS。
- [ ] **Step 5: 提交** `fix(store): 快捷格式改 TOML；覆盖表解析失败不再重写文件、按最后一次好数据服务`

---

### Task 10: P5 迁移编排、`format_version` 6、redb 改名

**Files:**
- Create: `wind_input/crates/wind-store/src/migrate_p5.rs`
- Modify: `wind_input/crates/wind-store/src/store.rs`、`migrate_p3.rs`（`P5_FORMAT_VERSION`、`format_version_reached`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`、`coordinator.rs`（`raise_format_version` 两处）
- Modify: `wind_input/crates/wind-config/src/user_meta.rs`

**Interfaces:**
- Produces（migrate_p5.rs）：
```rust
pub const P5_FORMAT_VERSION: u32 = 6;
/// 步骤 1：`<user_dir>/user` → `<user_dir>/data`（协调器在打开 Store 前调）。
pub fn relocate_text_dir(user_dir: &Path) -> anyhow::Result<()>;
/// 步骤 6：redb 改名（format_version 已写 6 之后调；失败只 WARN）。
pub fn retire_legacy_db(user_dir: &Path);
impl Store {
    /// 步骤 2–5（打开时在旧库链之后跑）：学到的数据采纳、补全 / 统计 / 快捷格式搬迁。返回是否全部完成。
    pub(crate) fn migrate_p5(&self) -> bool;
    /// 协调器据此决定能否写 6。
    pub fn p5_done(&self) -> bool;
    /// 写 6 之后调：删掉 state 里的 migrated-* 标记。
    pub fn forget_legacy_markers(&self);
}
```
- `Store::format_version_reached()`：旧库链完成且 `p5_done()` ⇒ `Some(6)`；否则沿用原规则（≤ 5）。
- `StateLoc.legacy_done` 判据不变（`format_version >= FULLY_MIGRATED_VERSION`）；`legacy_done` 时**不再补写**缺失标记（删掉 `open_inner` 里补写的那段）。
- 旧库链（< 5）仍按标记断点续做；`format_version` 写到 6 后 `forget_legacy_markers`。
- `user_meta` 全新目录判定：`userdata.redb`、`userdata.redb.legacy`、`data/`、`user/` 任一存在即非全新。`KNOWN_FORMAT_VERSION = 6`。
- 协调器：打开 Store 前 `relocate_text_dir(user_dir)`（失败 WARN，`text_dir` 仍指 `data/`——`user/` 改名失败时 Store 会在 `data/` 下看不到旧文件，所以失败时**本次运行把 `text_dir` 指回 `user/`**，下次再试）；打开后 / 短语迁移后，`p5_done()` 为真才 `raise_format_version(6)`，随即 `forget_legacy_markers()` 与 `retire_legacy_db(user_dir)`。已是 6 的启动也调一次 `retire_legacy_db`（上次改名失败时补做）。

- [ ] **Step 1: 写失败测试**

migrate_p5.rs：
```rust
#[test]
fn relocate_renames_user_dir() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("user/pinyin")).unwrap();
    std::fs::write(d.path().join("user/pinyin/user.dict.yaml"), "x").unwrap();
    relocate_text_dir(d.path()).unwrap();
    assert!(d.path().join("data/pinyin/user.dict.yaml").is_file());
    assert!(!d.path().join("user").exists());
}

#[test]
fn relocate_merges_when_both_exist_without_overwriting() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("user/a")).unwrap();
    std::fs::create_dir_all(d.path().join("data/b")).unwrap();
    std::fs::write(d.path().join("user/phrases.dict.yaml"), "old").unwrap();
    std::fs::write(d.path().join("data/phrases.dict.yaml"), "new").unwrap();
    relocate_text_dir(d.path()).unwrap();
    assert!(d.path().join("data/a").is_dir());
    assert_eq!(std::fs::read_to_string(d.path().join("data/phrases.dict.yaml")).unwrap(), "new");
    assert_eq!(std::fs::read_to_string(d.path().join("user/phrases.dict.yaml")).unwrap(), "old", "冲突项留在原处");
}

#[test]
fn retire_renames_redb_to_legacy() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("userdata.redb"), "db").unwrap();
    retire_legacy_db(d.path());
    assert!(d.path().join("userdata.redb.legacy").is_file());
    assert!(!d.path().join("userdata.redb").exists());
}
```
store.rs / migrate_p5 集成（用 v5 布局夹具：`state/runtime/<id>/completion.json`、`stats/2026-10.json`、`stats/meta.json`、`data/quick_format.yaml`、引擎里有 temp/freq 段但 `data/` 无对应文本）：
```rust
#[test]
fn p5_migrates_v5_layout() { /* 打开 Store（legacy_done=true）后断言：
   data/completion.table.yaml、data/stats/2026-10.toml、data/stats/meta.toml、data/quick_format.toml、
   data/pinyin/temp.dict.yaml、data/pinyin/freq.table.yaml 都在且内容等价；旧 json / yaml 已删；p5_done() 为真 */ }

#[test]
fn p5_migration_resumes_after_partial_run() { /* 夹具里补全已迁（completion.table.yaml 在、json 已删）、
   统计 json 还在：打开后统计迁完、补全不重复、p5_done() 为真 */ }

#[test]
fn unparsable_quick_yaml_is_kept_and_does_not_block() { /* quick_format.yaml 写坏 ⇒
   改名 quick_format.yaml.unmigrated、登记 Unmigrated、p5_done() 仍为真 */ }
```
construct.rs：
```rust
#[test]
fn v5_dir_reaches_format_6_and_retires_redb() { /* legacy_user_dir 夹具迁到 5 后再开：
   meta format_version == 6，userdata.redb.legacy 在，userdata.redb 不在，state 下无 migrated-* */ }

#[test]
fn wiping_state_dir_keeps_all_user_data() { /* 新目录：写临时词、词频、联想、补全、统计 → flush → 关；
   删整个 state 根；重开 → 全部读得到 */ }

#[test]
fn deleting_markers_after_format_6_changes_nothing() { /* 6 之后删 state 标记重开：不打开 redb、不重迁、不补写标记 */ }
```
user_meta.rs：
```rust
#[test]
fn legacy_redb_or_data_dir_means_not_fresh() { /* 只有 userdata.redb.legacy ⇒ 非全新；只有 data/ ⇒ 非全新 */ }
```

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现**（步骤 2 学到的数据：对 `LEARNED_TARGETS × 引擎里有数据的方案` 调 `bind_by_rule`——Task 4 的采纳逻辑负责渲染；步骤 3/4/5 各自「目标文件不在且源在 ⇒ 转换 → 提交 → 删源」；`p5_done` = 源文件都不在了（`state` 下无 `completion.json`、无 `stats/*.json`、`data/` 下无 `quick_format.yaml`、`user/` 不在）且学到的数据都已绑定）。
- [ ] **Step 4: 跑**：`cargo test -p wind-store -p wind-coordinator -p wind-config -p wind-webdata` → PASS。
- [ ] **Step 5: 提交** `feat(store): P5 迁移——数据进 data/、format_version 6 为唯一判据、旧库改名 .legacy`

---

### Task 11: 设置页提示数据文件问题

**Files:**
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（`system.storageStatus`）
- Modify: wind-setting 仓（`../wind-setting`，在其分支 `feat/user-data-storage` 上做）：词库页顶部提示条

**Interfaces:**
- `system.storageStatus` 增加 `"issues": [{"path": "<相对 data/ 的路径>", "kind": "bad_lines"|"isolated"|"rebuilt"|"unmigrated", "lines": n?, "kept_as": "…"?}]`；`format_version` 字段改报 meta 里的实际值（`known_format_version` 仍报 `KNOWN_FORMAT_VERSION`）。
- wind-setting：词库页打开时调一次 `system.storageStatus`，`issues` 非空则显示一条警示，逐条列出「文件 · 问题」并给「打开数据目录」按钮。文案示例：`freq.table.yaml：3 行无法解析，已跳过（原样保留）`、`pinyin/temp.dict.yaml：文件格式有误，已暂停写入，修复后自动恢复`、`stats/2026-10.toml：文件损坏，已另存为 …corrupt-… 并重建`。

- [ ] **Step 1: 写失败测试**（wind-webdata）

```rust
#[test]
fn storage_status_lists_data_file_issues() {
    let c = coord("storage_status_issues");
    let f = c.user_data_dir().join("data/pinyin/freq.table.yaml"); // 取数据目录的方法以代码为准
    wind_userdata::issues::report(&f, wind_userdata::issues::IssueKind::BadLines(3));
    let v = c.web_data_rpc("system.storageStatus", &json!({})).unwrap();
    let it = &v["issues"][0];
    assert_eq!(it["path"], "pinyin/freq.table.yaml");
    assert_eq!(it["kind"], "bad_lines");
    assert_eq!(it["lines"], 3);
    wind_userdata::issues::clear(&f);
}
```
wind-setting：对解析 `storageStatus` 的函数写单元测试 `parses_storage_issues`（输入上面的 JSON，输出三条文案）。

- [ ] **Step 2: 跑，确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 跑**：`cargo test -p wind-webdata storage_status`；wind-setting 按其 AGENTS 约定跑本机测试 → PASS。
- [ ] **Step 5: 提交**（两仓各一笔）`feat(webdata): storageStatus 报告数据文件问题` / `feat(dict): 词库页提示数据文件格式问题`

---

### Task 12: 外围收尾

**Files:**
- Modify: `config/app.toml`（`[datadir].markers` 加 `meta.toml`、`data`、`user`、`userdata.redb.legacy`；`state_files` 注释更新）
- Modify: `AGENTS.md:30`、`docs/architecture/user-override.md:67`、`docs/design/quick-input-format-table.md:288`（路径与格式）
- Modify: 过时注释：`wind-coordinator/src/coordinator.rs:1530`、`handle_quick_format.rs:8`、`wind-quick-input/src/format_table.rs:239`、`wind-store/src/migrate_p4.rs:2`、`wind-config/src/user_meta.rs:9-12`、`wind-config/src/config.rs:9420`、`wind-engine/src/manager.rs` `warn_stale_charset_files` 注释表
- Test: `wind-utils/tests/line_ending_coverage.rs`（新增的 `codec.rs` 若含按行解析器，按该测试的规则登记）

- [ ] **Step 1:** 改上述文件。
- [ ] **Step 2:** 跑全量：`CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录> cargo test --workspace --no-fail-fast` → 只允许已知的 `pinyin_multipath` 企鹅一条红（merge-base 同红）。
- [ ] **Step 3: 提交** `docs: 用户数据目录改为 data/、格式与卸载清单随 P5 更新`

---

## 自检记录

- 规格覆盖：§2.1 目录 → Task 5/6/10；§2.4 格式 → Task 3/7/8/9；§2.5 异常策略 → Task 2/4/7/8/9/11；§3.2 定期 fsync → Task 1；§7.4 迁移 → Task 10；卸载 / 文档 → Task 12。
- 接口一致性：`TextFormat`、`TableSpec`（Task 3/4/5）、`issues::{report, clear, list_under, report_parse, IssueKind}`（Task 2/4/7/8/9/11）、`preserve_corrupt`（Task 4/7/8）、`load_machine_file`（Task 7/8）、`bind_by_rule` / `learned_binding`（Task 5/10）、`P5_FORMAT_VERSION`（Task 10）。
