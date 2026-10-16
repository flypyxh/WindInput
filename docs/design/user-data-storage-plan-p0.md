# 用户数据存储重设计：前置 + P0 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 发出去降级保护与目录归位（前置），并建成「快照 + 日志 + 增量表」引擎 crate `wind-userdata`（P0），为 P1 把临时词 / 草稿 / 计数接进引擎铺路。

**Architecture:** 前置改 `wind-config`（`state_dir()` 归位到 `state/`、`meta.toml` 的 `format_version`）与协调器（高版本只读）。P0 新建 crate `wind-userdata`：`journal`（按设备二进制日志，一条一次 write，CRC 尾截断）、`state`（折回水位，tmp+fsync+rename）、`device`（设备 id、机器指纹、锁文件）、`delta`（两级增量表 + 声母分组索引）、`snapshot`（wdat 段 + 定长 sidecar）、`engine`（打开 / 写入发布 / 折回）、`layer`（`DictLayer` 适配）。P0 不改 `wind-engine` 的接线，P1 再接。

**Tech Stack:** Rust 2024（工具链 1.96，`File::try_lock` 可用）；`wind-dict` 的 `WdatStreamWriter` / `WdatReader` / `reader_pool`；`crc32fast`（已在 Cargo.lock）；`toml`（工作区依赖）；`memmap2`（工作区已用）。不引入 `arc-swap`，快照发布用 `RwLock<Arc<Snapshot>>` 只在克隆 Arc 时持读锁。

**Spec:** `docs/design/user-data-storage-redesign.md`（§1 原语、§2.1 目录、§3 引擎、§7.1 降级保护、§11 验证）。

## Global Constraints

- 日志记录**一条一次 `write(2)`，无用户态缓冲**（§3.2）。
- 真源级持久化次序：tmp → fsync tmp → rename → fsync 目录（Windows 用 `MoveFileExW(MOVEFILE_WRITE_THROUGH)`）→ 最后写 state（§3.5）。
- 日志永不 mmap；裁剪走重写 + rename（§3.2）。
- 快照文件头记录其覆盖到的 seq；state 读不出时以快照水位为准，**不整份回放**（§3.6）。
- delta 冻结层 `Arc<[Entry]>` + 尾巴 ≤ 128 条；声母分组索引复用 `wind-store::abbrev_index` 的 `group_of` 规则（含无边界组 `\u{1}` + 码首字符）（§3.3）。
- 本机根下只有 `cache/`、`state/`、`logs/`；`state.toml` 挪进 `state/`（§2.1）。
- `meta.toml` 的 `format_version` 当前 = 1；读到更高版本进入只读模式（§7.1）。
- INFO 级日志不得含词条内容（根 AGENTS.md）。
- 提交走 `git commit -F - --only -- <路径>`，不用 `git add -A`；每个任务一笔提交；`cargo fmt -p <crate>`，不跑 `--all`。
- 测试用 `TMPDIR=<独立临时目录>`（或任意短路径）隔离；临时目录名带 pid。

## Review Focus

1. **日志尾部半条记录**（断电 / SIGKILL 在 write 中途）：启动必须恢复到最后一条完整记录，不得 panic、不得丢前面的记录。→ Task 4 `truncated_tail_stops_at_last_complete_record`。
2. **state 与快照水位不一致**（state 比快照旧 / 比快照新 / 缺失）：以快照内嵌的 `max_seq` 为准，只回放更大的 seq，不重复应用。→ Task 8 `state_missing_replays_only_beyond_snapshot_watermark`。
3. **同键多操作**（晋升 = Delete + Upsert 同 seq 对）：按 seq 顺序叠加，不合并成一条。→ Task 6 `same_key_ops_apply_in_seq_order`。
4. **折回期间新到的操作**：折回只摘除 `seq ≤ 水位` 的条目，期间写入的保留。→ Task 8 `fold_keeps_ops_that_arrived_during_fold`。
5. **目录被复制到另一台机器**（`machine_fp` 不符）：生成新 `device_id`，旧日志封存为 `.wjl.sealed`，不与新日志混号。→ Task 5 `copied_dir_gets_new_device_and_seals_old_journal`。

---

## 前置

### Task 1: `state_dir()` 归位到 `local_dir()/state/`，`state.toml` 一次性迁移

**Files:**
- Modify: `wind_input/crates/wind-config/src/config.rs:9947-9952`（`state_dir`）
- Modify: `wind_input/crates/wind-config/src/runtime_state.rs:118-135`（`load` 迁移旧文件）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs:2098` 与 `:2158`（备份 / 还原的 `state_file` 改用 `Config::state_dir()`）
- Modify: `wind_input/crates/wind-coordinator/tests/app_schema_remember_persist.rs:94-101`
- Test: `wind_input/crates/wind-config/src/runtime_state.rs`（tests 模块）

**Interfaces:**
- Produces: `Config::state_dir() -> Option<PathBuf>` = `local_dir().join("state")`；`RuntimeState::load(state_dir)` 在 `state_dir/state.toml` 不存在而 `state_dir.parent()/state.toml` 存在时把它 rename 过来再读。

- [ ] **Step 1: 写失败的测试**（`runtime_state.rs` tests 模块末尾）

```rust
    /// 前置（设计 §2.1）：state.toml 从本机根挪进 state/，老位置的文件首次加载时搬过来。
    #[test]
    fn legacy_state_toml_is_moved_into_state_dir_on_load() {
        let root = std::env::temp_dir().join(format!("wind_state_move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("state.toml"), "last_chinese_mode = false\n").unwrap();
        let state_dir = root.join("state");
        let st = RuntimeState::load(&state_dir);
        assert!(!st.last_chinese_mode, "应读到老位置的内容");
        assert!(state_dir.join("state.toml").is_file(), "应已搬进 state/");
        assert!(!root.join("state.toml").exists(), "老位置应已清空");
        // 再读一次走新位置
        assert!(!RuntimeState::load(&state_dir).last_chinese_mode);
        let _ = std::fs::remove_dir_all(&root);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config legacy_state_toml_is_moved`
Expected: FAIL（`应已搬进 state/` 断言）

- [ ] **Step 3: 实现**

`config.rs` 的 `state_dir`：

```rust
    /// 本机**状态**目录（设计 `user-data-storage-redesign.md` §2.1）：机器写、不可再生、
    /// 不漫游。`state.toml`、设备 id、锁、运行时快照、小文件都在这里。与 `cache/`（可删）
    /// 和 `logs/` 并列，本机根下只有这三个目录。
    pub fn state_dir() -> Option<PathBuf> {
        Self::local_dir().map(|d| d.join("state"))
    }
```

`runtime_state.rs` 的 `load`：

```rust
    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("state.toml");
        // 一次性迁移：此前 state.toml 直接躺在本机根下（`local_dir()`），归位到 state/
        if !path.exists()
            && let Some(parent) = state_dir.parent()
            && parent.join("state.toml").is_file()
        {
            let _ = std::fs::create_dir_all(state_dir);
            if let Err(e) = std::fs::rename(parent.join("state.toml"), &path) {
                tracing::warn!("迁移 state.toml 到 state/ 失败: {e}");
            }
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
```

`wind-webdata/src/lib.rs` 两处：`wind_config::Config::local_dir().map(|d| d.join("state.toml"))` → `wind_config::Config::state_dir().map(|d| d.join("state.toml"))`。

`app_schema_remember_persist.rs`：`let state = root.join("localdata/state.toml");` → `root.join("localdata/state/state.toml")`；断言 `Config::state_dir() == Some(root.join("localdata/state"))`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config && TMPDIR=<独立临时目录> cargo test -p wind-coordinator --test app_schema_remember_persist && TMPDIR=<独立临时目录> cargo test -p wind-webdata`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-config -p wind-webdata -p wind-coordinator
cd .. && git commit -F - --only -- wind_input/crates/wind-config/src/config.rs wind_input/crates/wind-config/src/runtime_state.rs wind_input/crates/wind-webdata/src/lib.rs wind_input/crates/wind-coordinator/tests/app_schema_remember_persist.rs <<'EOF'
refactor(config): state.toml 归位到本机根下的 state/，首次加载自动迁移（用户数据设计 §2.1 前置）
EOF
```

### Task 2: `meta.toml` 的 `format_version` 与只读模式

**Files:**
- Create: `wind_input/crates/wind-config/src/user_meta.rs`
- Modify: `wind_input/crates/wind-config/src/lib.rs`（`pub mod user_meta;`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs:344-382`（`open_user_store` 前先查版本）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（字段 `storage_readonly: Option<String>` + 访问器）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs:464-472`（新增 `"system.storageStatus"`）
- Test: `wind_input/crates/wind-config/src/user_meta.rs`（tests 模块）

**Interfaces:**
- Produces:
  ```rust
  pub const KNOWN_FORMAT_VERSION: u32 = 1;
  pub struct UserMeta { pub format_version: u32, pub dir_id: String }
  pub enum MetaCheck { Ok(UserMeta), Incompatible { found: u32, known: u32 }, Unreadable(String) }
  pub fn load_or_init(user_dir: &Path) -> MetaCheck
  ```
  `Coordinator::storage_readonly_reason(&self) -> Option<&str>`；RPC `system.storageStatus` → `{"readonly": bool, "reason": string|null, "format_version": u32, "known_format_version": u32}`。

- [ ] **Step 1: 写失败的测试**（`user_meta.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_meta_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_meta_is_initialised_with_known_version_and_random_dir_id() {
        let d = dir("init");
        let MetaCheck::Ok(m) = load_or_init(&d) else { panic!("应初始化") };
        assert_eq!(m.format_version, KNOWN_FORMAT_VERSION);
        assert_eq!(m.dir_id.len(), 32);
        let MetaCheck::Ok(again) = load_or_init(&d) else { panic!() };
        assert_eq!(again.dir_id, m.dir_id, "第二次读到同一个 dir_id");
    }

    #[test]
    fn newer_version_is_incompatible_and_file_is_left_untouched() {
        let d = dir("newer");
        let text = "format_version = 99\ndir_id = \"abc\"\n";
        std::fs::write(d.join("meta.toml"), text).unwrap();
        assert!(matches!(load_or_init(&d), MetaCheck::Incompatible { found: 99, known: 1 }));
        assert_eq!(std::fs::read_to_string(d.join("meta.toml")).unwrap(), text, "只读：一字不改");
    }

    #[test]
    fn unparsable_meta_is_reported_not_overwritten() {
        let d = dir("bad");
        std::fs::write(d.join("meta.toml"), "not toml ==").unwrap();
        assert!(matches!(load_or_init(&d), MetaCheck::Unreadable(_)));
        assert_eq!(std::fs::read_to_string(d.join("meta.toml")).unwrap(), "not toml ==");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config user_meta`
Expected: 编译失败（模块不存在）

- [ ] **Step 3: 实现 `user_meta.rs`**

```rust
//! `<userdata>/meta.toml`：用户目录内数据格式的版本与目录 id（设计 §2.4、§7.1）。
//!
//! **降级保护必须先于任何新格式发布**：新版本写出新格式后用户回退，旧版本若照常在
//! 旁边继续写 redb，两边就分叉。读到比自己认识的更高的版本号 ⇒ 只读模式，一个字节都不写。

use std::path::Path;

/// 本版本认识的格式版本。1 = 纯 redb。每个 P 阶段迁移完成 +1。
pub const KNOWN_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserMeta {
    pub format_version: u32,
    /// 随数据目录生成的随机 id（32 位十六进制）。随目录漫游 / 复制；与本机 `device.toml`
    /// 里记的 `dir_id` 对不上就说明目录被复制或换了目录（设计 §3.7）。
    pub dir_id: String,
}

#[derive(Debug)]
pub enum MetaCheck {
    Ok(UserMeta),
    /// 数据由更新版本写入：只读模式，`found` 给提示用。
    Incompatible { found: u32, known: u32 },
    /// 文件在但读不出 / 解析不出：不覆盖（那可能是更新版本的格式），按只读处理。
    Unreadable(String),
}

pub fn load_or_init(user_dir: &Path) -> MetaCheck {
    let path = user_dir.join("meta.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<UserMeta>(&text) {
            Ok(m) if m.format_version > KNOWN_FORMAT_VERSION => MetaCheck::Incompatible {
                found: m.format_version,
                known: KNOWN_FORMAT_VERSION,
            },
            Ok(m) => MetaCheck::Ok(m),
            Err(e) => MetaCheck::Unreadable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let m = UserMeta {
                format_version: KNOWN_FORMAT_VERSION,
                dir_id: uuid::Uuid::new_v4().simple().to_string(),
            };
            let _ = std::fs::create_dir_all(user_dir);
            match toml::to_string(&m) {
                Ok(s) => {
                    let tmp = user_dir.join("meta.toml.tmp");
                    if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                        MetaCheck::Ok(m)
                    } else {
                        // 写不进用户目录：不阻断启动，但没有稳定的 dir_id，按只读对待更稳
                        MetaCheck::Unreadable("meta.toml 写入失败".into())
                    }
                }
                Err(e) => MetaCheck::Unreadable(e.to_string()),
            }
        }
        Err(e) => MetaCheck::Unreadable(e.to_string()),
    }
}
```

`wind-config/Cargo.toml` 确认已有 `uuid`、`toml`、`serde`（已有则不动）。`lib.rs` 加 `pub mod user_meta;`。

- [ ] **Step 4: 协调器接入**

`construct.rs` 两处 `let store = user_dir.as_deref().and_then(Self::open_user_store);` 改为：

```rust
        let (store, storage_readonly) = match user_dir.as_deref() {
            None => (None, None),
            Some(d) => match wind_config::user_meta::load_or_init(d) {
                wind_config::user_meta::MetaCheck::Ok(_) => (Self::open_user_store(d), None),
                wind_config::user_meta::MetaCheck::Incompatible { found, known } => {
                    let reason = format!(
                        "用户数据由更新版本写入（格式 {found}，本版本只认识 {known}）：已进入只读模式，\
                         不会修改任何用户数据。请升级到新版本，或从备份还原。"
                    );
                    error!("{reason}");
                    (None, Some(reason))
                }
                wind_config::user_meta::MetaCheck::Unreadable(e) => {
                    let reason = format!("用户目录的 meta.toml 读不出（{e}）：按只读模式运行。");
                    error!("{reason}");
                    (None, Some(reason))
                }
            },
        };
```

`Self::build(..)` 多传 `storage_readonly`，`Coordinator` 加字段 `storage_readonly: Option<String>` 与

```rust
    /// 只读模式的原因（设计 §7.1 降级保护）；`None` = 正常读写。
    pub fn storage_readonly_reason(&self) -> Option<&str> {
        self.storage_readonly.as_deref()
    }
```

`wind-webdata/src/lib.rs` 在 `"system.memoryStats"` 旁加：

```rust
        "system.storageStatus" => Ok(serde_json::json!({
            "readonly": h.storage_readonly_reason().is_some(),
            "reason": h.storage_readonly_reason(),
            "format_version": wind_config::user_meta::KNOWN_FORMAT_VERSION,
            "known_format_version": wind_config::user_meta::KNOWN_FORMAT_VERSION,
        })),
```

（`h` 是该 match 里既有的协调器句柄名，照旁边 `memory_stats_json(h)` 的写法。）

- [ ] **Step 5: 协调器测试**（`wind-coordinator/tests/storage_readonly.rs` 新文件）

```rust
//! 设计 §7.1：meta.toml 版本高于本版本 ⇒ 只读模式，store 不打开，RPC 能查到原因。
use wind_config::Config;

#[test]
fn newer_meta_version_puts_coordinator_in_readonly_mode() {
    let root = std::env::temp_dir().join(format!("wind_ro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("userdata")).unwrap();
    std::fs::create_dir_all(root.join("localdata")).unwrap();
    std::fs::write(root.join("portable.flag"), "").ok(); // 以 app_schema_remember_persist.rs 的 setup 为准
    unsafe { std::env::set_var("WIND_INSTALL_ROOT", &root) };
    std::fs::write(
        root.join("userdata/meta.toml"),
        "format_version = 99\ndir_id = \"x\"\n",
    )
    .unwrap();
    let user = Config::user_config_dir().expect("用户目录须已重定向");
    let (c, _rx) = wind_coordinator::Coordinator::new_for_test(&user); // 以 app_schema_remember_persist.rs 的 make() 为准
    assert!(c.storage_readonly_reason().is_some());
    let v = c.web_data_rpc("system.storageStatus", &serde_json::json!({})).unwrap();
    assert_eq!(v["readonly"], true);
    assert!(!user.join("userdata.redb").exists(), "只读模式不得创建 redb");
    let _ = std::fs::remove_dir_all(&root);
}
```

构造方式照抄 `app_schema_remember_persist.rs` 里的 `setup()` / `make()`（便携标记文件名与构造函数以该文件为准，本计划不复述）。

- [ ] **Step 6: 跑测试**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config user_meta && TMPDIR=<独立临时目录> cargo test -p wind-coordinator --test storage_readonly`
Expected: PASS

- [ ] **Step 7: 提交**

```bash
cd wind_input && cargo fmt -p wind-config -p wind-coordinator -p wind-webdata
cd .. && git add wind_input/crates/wind-config/src/user_meta.rs wind_input/crates/wind-coordinator/tests/storage_readonly.rs
git commit -F - --only -- wind_input/crates/wind-config/src/user_meta.rs wind_input/crates/wind-config/src/lib.rs wind_input/crates/wind-coordinator/src/construct.rs wind_input/crates/wind-coordinator/src/coordinator.rs wind_input/crates/wind-webdata/src/lib.rs wind_input/crates/wind-coordinator/tests/storage_readonly.rs <<'EOF'
feat(config): meta.toml 记 format_version，读到更高版本进入只读模式（用户数据设计 §7.1 降级保护）
EOF
```

---

## P0：`wind-userdata` 引擎

### Task 3: crate 骨架与记录模型

**Files:**
- Create: `wind_input/crates/wind-userdata/Cargo.toml`、`src/lib.rs`、`src/record.rs`
- Modify: `wind_input/Cargo.toml`（members 加 `"crates/wind-userdata"`）
- Test: `src/record.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub enum Target { User = 0, Temp = 1, Draft = 2, Freq = 3, Assoc = 4, Phrases = 5, Shadow = 6 }
  pub enum OpKind { Upsert, Delete, SetWeight, Incr, Touch, Promote, Expire, BatchMarker }
  pub struct Record { pub seq: u64, pub ts_ms: i64, pub kind: OpKind, pub target: Target,
                      pub schema: String, pub code: String, pub text: String, pub payload: Payload }
  pub enum Payload { None, Weight(i32), Count(u32), Ts(i64), Kind(u8) }
  impl Record { pub fn encode(&self) -> Vec<u8>; pub fn decode(bytes: &[u8]) -> anyhow::Result<Record>; }
  ```
  编码：`len u32 | crc32 u32 | seq u64 | ts_ms i64 | kind u8 | target u8 | schema(len u16 + bytes) | code | text | payload(tag u8 + 值)`；crc32 覆盖 `seq` 起到末尾。

- [ ] **Step 1: 骨架**

`Cargo.toml`：

```toml
[package]
name = "wind-userdata"
version.workspace = true
edition.workspace = true

[dependencies]
wind-dict = { path = "../wind-dict" }
wind-candidate = { path = "../wind-candidate" }
anyhow = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
tracing = { workspace = true }
crc32fast = "1"
memmap2 = "0.9"
uuid = { workspace = true }

[lints]
workspace = true
```

（`memmap2` 版本以 `wind-dict/Cargo.toml` 里的为准。）`lib.rs`：

```rust
//! 用户数据引擎：快照 + 日志 + 增量表（设计 `docs/design/user-data-storage-redesign.md` §3）。
pub mod record;
pub mod journal;
pub mod state;
pub mod device;
pub mod delta;
pub mod snapshot;
pub mod engine;
pub mod layer;
```

先只写 `pub mod record;`，后续任务逐个加。

- [ ] **Step 2: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_and_crc_catches_corruption() {
        let r = Record {
            seq: 7, ts_ms: 1_700_000_000_000, kind: OpKind::Upsert, target: Target::Temp,
            schema: "pinyin".into(), code: "nihao".into(), text: "你好".into(),
            payload: Payload::Weight(10),
        };
        let bytes = r.encode();
        assert_eq!(Record::decode(&bytes).unwrap(), r);
        let mut bad = bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(Record::decode(&bad).is_err(), "CRC 必须抓到篡改");
        let short = &bytes[..bytes.len() - 3];
        assert!(Record::decode(short).is_err(), "半条记录必须报错而不是 panic");
    }
}
```

- [ ] **Step 3: 实现 `record.rs`**

```rust
use anyhow::{bail, Context};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Target { User = 0, Temp = 1, Draft = 2, Freq = 3, Assoc = 4, Phrases = 5, Shadow = 6 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OpKind { Upsert = 0, Delete = 1, SetWeight = 2, Incr = 3, Touch = 4, Promote = 5, Expire = 6, BatchMarker = 7 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload { None, Weight(i32), Count(u32), Ts(i64), Kind(u8) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub ts_ms: i64,
    pub kind: OpKind,
    pub target: Target,
    pub schema: String,
    pub code: String,
    pub text: String,
    pub payload: Payload,
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn get_str<'a>(b: &'a [u8], pos: &mut usize) -> anyhow::Result<&'a str> {
    let n = u16::from_le_bytes(b.get(*pos..*pos + 2).context("短")?.try_into()?) as usize;
    *pos += 2;
    let s = std::str::from_utf8(b.get(*pos..*pos + n).context("短")?)?;
    *pos += n;
    Ok(s)
}

impl Record {
    /// 帧：`len u32 | crc32 u32 | body`；crc 覆盖 body。`len` 是 body 长度。
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(64);
        body.extend_from_slice(&self.seq.to_le_bytes());
        body.extend_from_slice(&self.ts_ms.to_le_bytes());
        body.push(self.kind as u8);
        body.push(self.target as u8);
        put_str(&mut body, &self.schema);
        put_str(&mut body, &self.code);
        put_str(&mut body, &self.text);
        match &self.payload {
            Payload::None => body.push(0),
            Payload::Weight(w) => { body.push(1); body.extend_from_slice(&w.to_le_bytes()); }
            Payload::Count(c) => { body.push(2); body.extend_from_slice(&c.to_le_bytes()); }
            Payload::Ts(t) => { body.push(3); body.extend_from_slice(&t.to_le_bytes()); }
            Payload::Kind(k) => { body.push(4); body.push(*k); }
        }
        let mut out = Vec::with_capacity(body.len() + 8);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// 只解一条；`bytes` 多出来的尾巴忽略。帧不完整 / CRC 不符 ⇒ Err。
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Record> {
        let len = u32::from_le_bytes(bytes.get(0..4).context("短")?.try_into()?) as usize;
        let crc = u32::from_le_bytes(bytes.get(4..8).context("短")?.try_into()?);
        let body = bytes.get(8..8 + len).context("帧不完整")?;
        if crc32fast::hash(body) != crc {
            bail!("crc 不符");
        }
        let mut p = 0usize;
        let seq = u64::from_le_bytes(body.get(p..p + 8).context("短")?.try_into()?); p += 8;
        let ts_ms = i64::from_le_bytes(body.get(p..p + 8).context("短")?.try_into()?); p += 8;
        let kind = match *body.get(p).context("短")? { 0 => OpKind::Upsert, 1 => OpKind::Delete, 2 => OpKind::SetWeight, 3 => OpKind::Incr, 4 => OpKind::Touch, 5 => OpKind::Promote, 6 => OpKind::Expire, 7 => OpKind::BatchMarker, k => bail!("未知 kind {k}") }; p += 1;
        let target = match *body.get(p).context("短")? { 0 => Target::User, 1 => Target::Temp, 2 => Target::Draft, 3 => Target::Freq, 4 => Target::Assoc, 5 => Target::Phrases, 6 => Target::Shadow, t => bail!("未知 target {t}") }; p += 1;
        let schema = get_str(body, &mut p)?.to_string();
        let code = get_str(body, &mut p)?.to_string();
        let text = get_str(body, &mut p)?.to_string();
        let payload = match *body.get(p).context("短")? {
            0 => Payload::None,
            1 => Payload::Weight(i32::from_le_bytes(body.get(p + 1..p + 5).context("短")?.try_into()?)),
            2 => Payload::Count(u32::from_le_bytes(body.get(p + 1..p + 5).context("短")?.try_into()?)),
            3 => Payload::Ts(i64::from_le_bytes(body.get(p + 1..p + 9).context("短")?.try_into()?)),
            4 => Payload::Kind(*body.get(p + 1).context("短")?),
            t => bail!("未知 payload {t}"),
        };
        Ok(Record { seq, ts_ms, kind, target, schema, code, text, payload })
    }

    /// 整帧长度（含 8 字节帧头），供日志顺序读取推进游标。
    pub fn frame_len(bytes: &[u8]) -> Option<usize> {
        let len = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
        Some(8 + len)
    }
}
```

- [ ] **Step 4: 跑测试通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata
cd .. && git add wind_input/crates/wind-userdata
git commit -F - --only -- wind_input/Cargo.toml wind_input/Cargo.lock wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 新建 wind-userdata crate，日志记录的帧编码与 CRC（设计 §3.2）
EOF
```

### Task 4: 日志 `journal.rs`——追加、回放、尾截断、裁剪

**Files:**
- Create: `src/journal.rs`
- Test: 同文件 tests

**Interfaces:**
- Consumes: `Record`（Task 3）
- Produces:
  ```rust
  pub struct Journal { /* path, file: File, next_seq: u64 */ }
  impl Journal {
      pub fn open(path: &Path, device_id: [u8;16], dir_id: [u8;16]) -> anyhow::Result<Journal>; // 不存在则建并写头
      pub fn append(&mut self, rec: &Record) -> anyhow::Result<()>;   // 一条一次 write
      pub fn next_seq(&self) -> u64;
      pub fn replay(path: &Path, from_seq_exclusive: u64) -> anyhow::Result<Vec<Record>>; // 尾部坏帧处截断
      pub fn tail_seq(path: &Path) -> anyhow::Result<u64>;            // 最后一条完整记录的 seq，空日志 0
      pub fn prune_before(path: &Path, keep_from_seq: u64) -> anyhow::Result<()>; // 重写 + rename
  }
  ```
  文件头 40 字节：`"WJNL" | version u16 = 1 | reserved u16 | device_id [u8;16] | dir_id [u8;16]`。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_jnl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }
    fn rec(seq: u64, code: &str) -> Record {
        Record { seq, ts_ms: 1, kind: OpKind::Upsert, target: Target::Temp, schema: "s".into(),
                 code: code.into(), text: "t".into(), payload: Payload::Weight(1) }
    }

    #[test]
    fn append_then_replay_from_watermark() {
        let p = tmp("a.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=5 { j.append(&rec(i, "c")).unwrap(); }
        assert_eq!(j.next_seq(), 6);
        let got = Journal::replay(&p, 3).unwrap();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![4, 5]);
        assert_eq!(Journal::tail_seq(&p).unwrap(), 5);
        // 重开续号
        let j2 = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j2.next_seq(), 6);
    }

    /// Review Focus 1：尾部半条记录（断电 / SIGKILL）⇒ 截到最后一条完整记录，不 panic。
    #[test]
    fn truncated_tail_stops_at_last_complete_record() {
        let p = tmp("t.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=3 { j.append(&rec(i, "c")).unwrap(); }
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.truncate(bytes.len() - 5);
        std::fs::write(&p, &bytes).unwrap();
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(Journal::tail_seq(&p).unwrap(), 2);
        // 重开后在截断点续写，新记录可读
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j.next_seq(), 3);
        j.append(&rec(3, "d")).unwrap();
        assert_eq!(Journal::replay(&p, 0).unwrap().len(), 3);
    }

    #[test]
    fn prune_rewrites_without_old_head_and_keeps_header() {
        let p = tmp("p.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=6 { j.append(&rec(i, "c")).unwrap(); }
        drop(j);
        Journal::prune_before(&p, 4).unwrap();
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![4, 5, 6]);
        let j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j.next_seq(), 7);
    }

    #[test]
    fn wrong_header_is_rejected() {
        let p = tmp("h.wjl");
        std::fs::write(&p, b"NOPE").unwrap();
        assert!(Journal::open(&p, [1; 16], [2; 16]).is_err());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**（编译失败）

- [ ] **Step 3: 实现**

```rust
//! 按设备的追加日志 `.wjl`（设计 §3.2）：一条记录一次 `write(2)`，无用户态缓冲；崩溃最多
//! 丢最后一条；尾部 CRC 不符处截断。永不 mmap；裁剪走重写 + rename。

use crate::record::Record;
use anyhow::{bail, Context};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"WJNL";
const HEADER_LEN: u64 = 40;

pub struct Journal {
    file: std::fs::File,
    next_seq: u64,
}

fn header(device_id: [u8; 16], dir_id: [u8; 16]) -> [u8; 40] {
    let mut h = [0u8; 40];
    h[..4].copy_from_slice(MAGIC);
    h[4..6].copy_from_slice(&1u16.to_le_bytes());
    h[8..24].copy_from_slice(&device_id);
    h[24..40].copy_from_slice(&dir_id);
    h
}

/// 顺序扫描：返回 (完整记录们, 最后一条完整记录的结束偏移)。坏帧处停。
fn scan(path: &Path, from_seq_exclusive: u64, collect: bool) -> anyhow::Result<(Vec<Record>, u64, u64)> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < HEADER_LEN as usize || &bytes[..4] != MAGIC {
        bail!("不是 .wjl 日志: {}", path.display());
    }
    let mut pos = HEADER_LEN as usize;
    let mut out = Vec::new();
    let mut last_seq = 0u64;
    while pos < bytes.len() {
        let Some(flen) = Record::frame_len(&bytes[pos..]) else { break };
        if pos + flen > bytes.len() { break; }
        let Ok(r) = Record::decode(&bytes[pos..pos + flen]) else { break };
        last_seq = r.seq;
        if collect && r.seq > from_seq_exclusive { out.push(r); }
        pos += flen;
    }
    Ok((out, pos as u64, last_seq))
}

impl Journal {
    pub fn open(path: &Path, device_id: [u8; 16], dir_id: [u8; 16]) -> anyhow::Result<Journal> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let exists = path.is_file();
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        let next_seq = if !exists || file.metadata()?.len() == 0 {
            file.write_all(&header(device_id, dir_id))?;
            file.sync_data()?;
            1
        } else {
            let (_, end, last) = scan(path, 0, false)?;
            // 坏尾巴截掉：后续追加从最后一条完整记录之后开始
            file.set_len(end)?;
            file.seek(SeekFrom::End(0))?;
            last + 1
        };
        Ok(Journal { file, next_seq })
    }

    /// 一条一次 write。`rec.seq` 必须等于 `next_seq()`（调用方按序分配）。
    pub fn append(&mut self, rec: &Record) -> anyhow::Result<()> {
        if rec.seq != self.next_seq { bail!("seq 不连续: 期望 {} 实际 {}", self.next_seq, rec.seq); }
        let frame = rec.encode();
        self.file.write_all(&frame).context("写日志")?;
        self.next_seq += 1;
        Ok(())
    }

    pub fn next_seq(&self) -> u64 { self.next_seq }

    pub fn replay(path: &Path, from_seq_exclusive: u64) -> anyhow::Result<Vec<Record>> {
        Ok(scan(path, from_seq_exclusive, true)?.0)
    }

    pub fn tail_seq(path: &Path) -> anyhow::Result<u64> {
        Ok(scan(path, 0, false)?.2)
    }

    /// 裁掉 `seq < keep_from_seq` 的头部：重写到 tmp 再 rename（永不原地截断，见模块注释）。
    pub fn prune_before(path: &Path, keep_from_seq: u64) -> anyhow::Result<()> {
        let bytes = std::fs::read(path)?;
        if bytes.len() < HEADER_LEN as usize { return Ok(()); }
        let (recs, _, _) = scan(path, keep_from_seq.saturating_sub(1), true)?;
        let mut out = Vec::with_capacity(bytes.len());
        out.extend_from_slice(&bytes[..HEADER_LEN as usize]);
        for r in &recs { out.extend_from_slice(&r.encode()); }
        let tmp = path.with_extension("wjl.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_data()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
```

（`Read` 若未用到就去掉 import。）

- [ ] **Step 4: 跑测试通过**；**Step 5: 提交** `feat(userdata): 按设备追加日志——一条一次 write、尾截断、裁剪重写（设计 §3.2）`

### Task 5: `state.rs` 与 `device.rs`——折回水位、设备 id、机器指纹、锁

**Files:**
- Create: `src/state.rs`、`src/device.rs`
- Test: 各自 tests

**Interfaces:**
- Produces（`state.rs`）：
  ```rust
  #[derive(Default, Serialize, Deserialize, Clone, PartialEq, Debug)]
  pub struct TargetState { pub folded_seq: u64, pub text_fp: Option<String>, pub snapshot_fp: String }
  #[derive(Default, Serialize, Deserialize, Clone, PartialEq, Debug)]
  pub struct State { pub generation: u32, pub next_seq: u64, pub targets: BTreeMap<String, TargetState> }
  impl State { pub fn load(path: &Path) -> Option<State>;      // 读不出 / 解析不出 ⇒ None（水位未知）
               pub fn save(&self, path: &Path) -> anyhow::Result<()>; // tmp + fsync + rename + fsync 目录
  }
  pub fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()>; // 真源级次序，供快照与文本共用
  ```
- Produces（`device.rs`）：
  ```rust
  pub struct Device { pub device_id: [u8;16], pub machine_fp: String, pub dir_id: String }
  pub enum DeviceOutcome { Same(Device), Renewed { new: Device, sealed_journal: Option<PathBuf> } }
  pub fn load_or_renew(state_dir: &Path, dir_id: &str, journal_dir: &Path) -> anyhow::Result<DeviceOutcome>;
  pub fn machine_fingerprint() -> String;   // hostname + 用户名 + OS（可得的就用；不可得为空串）
  pub struct Lock(std::fs::File);
  pub fn try_lock(state_dir: &Path) -> anyhow::Result<Option<Lock>>; // None = 别的进程持有 ⇒ 只读
  ```

- [ ] **Step 1: 测试**（`state.rs`）

```rust
    #[test]
    fn state_roundtrip_and_corrupt_file_reads_as_unknown() {
        let d = std::env::temp_dir().join(format!("wind_state-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("state");
        let mut s = State { generation: 2, next_seq: 10, targets: Default::default() };
        s.targets.insert("temp:pinyin".into(), TargetState { folded_seq: 9, text_fp: None, snapshot_fp: "ab".into() });
        s.save(&p).unwrap();
        assert_eq!(State::load(&p), Some(s));
        std::fs::write(&p, "garbage ==").unwrap();
        assert_eq!(State::load(&p), None, "损坏 ⇒ 水位未知，由快照内嵌水位兜底");
        assert!(!d.join("state.tmp").exists(), "tmp 不残留");
    }
```

（`device.rs`，Review Focus 5）

```rust
    #[test]
    fn copied_dir_gets_new_device_and_seals_old_journal() {
        let d = std::env::temp_dir().join(format!("wind_dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("journal")).unwrap();
        let DeviceOutcome::Renewed { new: first, sealed_journal: None } = load_or_renew(&d, "dir-A", &d.join("journal")).unwrap() else { panic!("首次应生成") };
        // 同机同目录：不变
        assert!(matches!(load_or_renew(&d, "dir-A", &d.join("journal")).unwrap(), DeviceOutcome::Same(ref x) if x.device_id == first.device_id));
        // 模拟「目录被复制到另一台机器」：device.toml 里的 machine_fp 对不上
        let text = std::fs::read_to_string(d.join("device.toml")).unwrap().replace(&machine_fingerprint(), "other-machine");
        std::fs::write(d.join("device.toml"), text).unwrap();
        let jnl = d.join("journal").join(format!("{}.wjl", hex(&first.device_id)));
        std::fs::write(&jnl, b"WJNL").unwrap();
        let DeviceOutcome::Renewed { new, sealed_journal } = load_or_renew(&d, "dir-A", &d.join("journal")).unwrap() else { panic!("指纹不符应换号") };
        assert_ne!(new.device_id, first.device_id);
        assert_eq!(sealed_journal.as_deref(), Some(jnl.with_extension("wjl.sealed").as_path()));
        assert!(!jnl.exists() && jnl.with_extension("wjl.sealed").exists());
        // 换了用户目录（dir_id 不同）同样换号
        assert!(matches!(load_or_renew(&d, "dir-B", &d.join("journal")).unwrap(), DeviceOutcome::Renewed { .. }));
    }

    #[test]
    fn second_lock_holder_gets_none() {
        let d = std::env::temp_dir().join(format!("wind_lock-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let a = try_lock(&d).unwrap();
        assert!(a.is_some());
        assert!(try_lock(&d).unwrap().is_none(), "同进程再锁一次也该拿不到（File::try_lock 按文件描述表计）");
        drop(a);
        assert!(try_lock(&d).unwrap().is_some());
    }
```

- [ ] **Step 2: 实现 `state.rs`**

```rust
//! 折回水位 `state`（设计 §3.6）与真源级原子写（§3.5 次序）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TargetState { pub folded_seq: u64, pub text_fp: Option<String>, pub snapshot_fp: String }

#[derive(Default, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct State { pub generation: u32, pub next_seq: u64, pub targets: BTreeMap<String, TargetState> }

/// tmp → fsync tmp → rename → fsync 目录。Windows 没有目录 fsync，`rename` 后对目标再
/// `sync_all` 一次（NTFS 元数据日志保证目录项）。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow::anyhow!("无父目录"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp.{}", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    { std::fs::File::open(dir)?.sync_all()?; }
    #[cfg(not(unix))]
    { let _ = std::fs::File::open(path).and_then(|f| f.sync_all()); }
    Ok(())
}

impl State {
    pub fn load(path: &Path) -> Option<State> {
        toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        atomic_write(path, toml::to_string(self)?.as_bytes())
    }
}
```

- [ ] **Step 3: 实现 `device.rs`**

```rust
//! 设备 id、机器指纹、单写者锁（设计 §3.7）。
//!
//! `device_id` 不是机器 id 也不是目录 id：本机 `state/device.toml` 记 `(device_id, machine_fp,
//! 上次配对的 dir_id)`。机器指纹不符 = 用户目录被复制到另一台机器；dir_id 不符 = 本机换了
//! 用户目录。两种情况都换新 id，旧日志改名 `.wjl.sealed` 封存（仍可作同步源）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
struct DeviceFile { device_id: String, machine_fp: String, dir_id: String }

#[derive(Clone, Debug)]
pub struct Device { pub device_id: [u8; 16], pub machine_fp: String, pub dir_id: String }

pub enum DeviceOutcome { Same(Device), Renewed { new: Device, sealed_journal: Option<PathBuf> } }

pub fn hex(id: &[u8; 16]) -> String { id.iter().map(|b| format!("{b:02x}")).collect() }

fn unhex(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 { return None; }
    let mut out = [0u8; 16];
    for i in 0..16 { out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?; }
    Some(out)
}

pub fn machine_fingerprint() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME"))
        .ok().or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .unwrap_or_default();
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    format!("{host}|{user}|{}", std::env::consts::OS)
}

pub fn load_or_renew(state_dir: &Path, dir_id: &str, journal_dir: &Path) -> anyhow::Result<DeviceOutcome> {
    let path = state_dir.join("device.toml");
    let fp = machine_fingerprint();
    let old: Option<DeviceFile> = std::fs::read_to_string(&path).ok().and_then(|s| toml::from_str(&s).ok());
    if let Some(o) = &old
        && o.machine_fp == fp && o.dir_id == dir_id
        && let Some(id) = unhex(&o.device_id)
    {
        return Ok(DeviceOutcome::Same(Device { device_id: id, machine_fp: fp, dir_id: dir_id.into() }));
    }
    let sealed = old.as_ref().and_then(|o| {
        let j = journal_dir.join(format!("{}.wjl", o.device_id));
        if j.is_file() {
            let s = j.with_extension("wjl.sealed");
            std::fs::rename(&j, &s).ok().map(|_| s)
        } else { None }
    });
    let id = *uuid::Uuid::new_v4().as_bytes();
    let new = Device { device_id: id, machine_fp: fp.clone(), dir_id: dir_id.into() };
    let text = toml::to_string(&DeviceFile { device_id: hex(&id), machine_fp: fp, dir_id: dir_id.into() })?;
    crate::state::atomic_write(&path, text.as_bytes())?;
    Ok(DeviceOutcome::Renewed { new, sealed_journal: sealed })
}

pub struct Lock(#[allow(dead_code)] std::fs::File);

/// `state/lock` 独占锁。拿不到 ⇒ `None`，调用方进入只读模式（查询正常、写入丢弃并记 WARN）。
pub fn try_lock(state_dir: &Path) -> anyhow::Result<Option<Lock>> {
    std::fs::create_dir_all(state_dir)?;
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(state_dir.join("lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(Lock(f))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
```

（`File::try_lock` 的错误类型以本机 `rustc 1.96` 的 std 文档为准；若签名不同，按文档改 match 分支。）

- [ ] **Step 4: 跑测试通过**；**Step 5: 提交** `feat(userdata): 折回水位 state、真源级原子写、设备 id 与锁（设计 §3.5–§3.7）`

### Task 6: `delta.rs`——两级增量表与声母分组索引

**Files:**
- Create: `src/delta.rs`
- Test: 同文件 tests

**Interfaces:**
- Consumes: `Record`、`Target`
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub struct Entry { pub seq: u64, pub target: Target, pub schema: String, pub code: String, pub text: String,
                     pub op: DeltaOp, pub boundary: u64 }
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub enum DeltaOp { Upsert { weight: i32, kind: u8 }, Delete, SetWeight(i32), Incr(u32), Touch(i64) }
  #[derive(Clone, Default)]
  pub struct Delta { frozen: Arc<[Entry]>, tail: Vec<Entry>, groups: BTreeMap<(Target, String, String), Vec<usize>> /* (target, schema, group) → frozen 索引 */ }
  impl Delta {
      pub fn with(&self, e: Entry) -> Delta;                       // 返回新表（克隆尾巴；尾巴满 128 并入冻结层并重建分组）
      pub fn ops_for(&self, target: Target, schema: &str, code: &str, text: &str) -> Vec<&Entry>; // 按 seq 升序
      pub fn exact(&self, target: Target, schema: &str, code: &str) -> Vec<&Entry>;
      pub fn prefix(&self, target: Target, schema: &str, prefix: &str) -> Vec<&Entry>;
      pub fn by_group(&self, target: Target, schema: &str, group: &str) -> Vec<&Entry>;
      pub fn drop_folded(&self, target: Target, upto_seq: u64) -> Delta;  // 摘除 seq ≤ upto 的该目标条目
      pub fn len(&self) -> usize;
  }
  pub fn group_of(code: &str, boundary: u64) -> String; // 复制 wind-store::abbrev_index 的规则：有边界 ⇒ 音节首字母串；无边界 ⇒ '\u{1}' + 码首字符
  pub fn entry_from_record(r: &Record, boundary: u64) -> Option<Entry>;
  ```

- [ ] **Step 1: 测试**

```rust
    fn e(seq: u64, code: &str, text: &str, op: DeltaOp) -> Entry {
        Entry { seq, target: Target::Temp, schema: "py".into(), code: code.into(), text: text.into(), op, boundary: 0b101 }
    }

    /// Review Focus 3：同键多操作按 seq 顺序保留，不合并。
    #[test]
    fn same_key_ops_apply_in_seq_order() {
        let d = Delta::default()
            .with(e(1, "nihao", "你好", DeltaOp::Upsert { weight: 10, kind: 0 }))
            .with(e(2, "nihao", "你好", DeltaOp::Delete))
            .with(e(3, "nihao", "你好", DeltaOp::Upsert { weight: 20, kind: 1 }));
        let ops: Vec<u64> = d.ops_for(Target::Temp, "py", "nihao", "你好").iter().map(|x| x.seq).collect();
        assert_eq!(ops, vec![1, 2, 3]);
    }

    #[test]
    fn tail_overflow_freezes_and_queries_still_find_everything() {
        let mut d = Delta::default();
        for i in 0..300u64 { d = d.with(e(i + 1, &format!("ni{i}"), "x", DeltaOp::Upsert { weight: 1, kind: 0 })); }
        assert_eq!(d.len(), 300);
        assert_eq!(d.prefix(Target::Temp, "py", "ni").len(), 300);
        assert_eq!(d.exact(Target::Temp, "py", "ni7").len(), 1);
        assert_eq!(d.by_group(Target::Temp, "py", &group_of("ni7", 0b101)).len(), 300, "全部同组 n?");
    }

    #[test]
    fn drop_folded_keeps_newer_and_other_targets() {
        let d = Delta::default()
            .with(e(1, "a", "x", DeltaOp::Delete))
            .with(Entry { target: Target::Freq, ..e(2, "a", "x", DeltaOp::Incr(1)) })
            .with(e(3, "b", "y", DeltaOp::Delete));
        let d = d.drop_folded(Target::Temp, 2);
        let left: Vec<u64> = d.ops_for(Target::Temp, "py", "b", "y").iter().map(|x| x.seq).collect();
        assert_eq!(left, vec![3]);
        assert_eq!(d.ops_for(Target::Temp, "py", "a", "x").len(), 0);
        assert_eq!(d.len(), 2, "别的目标不动");
    }

    #[test]
    fn group_of_matches_store_rule() {
        assert_eq!(group_of("nihao", 0b101), "nh");
        assert_eq!(group_of("nihao", 0), "\u{1}n");
        assert_eq!(group_of("", 0), "");
    }
```

- [ ] **Step 2: 实现**

```rust
//! 两级增量表（设计 §3.3）：冻结层 `Arc<[Entry]>` 按键有序 + 尾巴 ≤ TAIL_MAX 条。写入只克隆尾巴；
//! 尾巴满时并入冻结层并重建分组索引。按键线程只读 `Arc<Delta>`，不碰锁。

use crate::record::{OpKind, Payload, Record, Target};
use std::collections::BTreeMap;
use std::sync::Arc;

const TAIL_MAX: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub seq: u64, pub target: Target, pub schema: String, pub code: String, pub text: String, pub op: DeltaOp, pub boundary: u64 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeltaOp { Upsert { weight: i32, kind: u8 }, Delete, SetWeight(i32), Incr(u32), Touch(i64) }

pub fn group_of(code: &str, boundary: u64) -> String {
    if boundary != 0 {
        return code.char_indices().filter(|(i, _)| *i < 64 && (boundary >> i) & 1 == 1).map(|(_, c)| c).collect();
    }
    match code.chars().next() { Some(c) => format!("\u{1}{c}"), None => String::new() }
}

fn key(e: &Entry) -> (Target, &str, &str, &str, u64) { (e.target, &e.schema, &e.code, &e.text, e.seq) }

#[derive(Clone, Default)]
pub struct Delta {
    frozen: Arc<[Entry]>,
    tail: Vec<Entry>,
    groups: Arc<BTreeMap<(Target, String, String), Vec<usize>>>,
}

impl Delta {
    pub fn with(&self, e: Entry) -> Delta {
        let mut tail = self.tail.clone();
        tail.push(e);
        if tail.len() <= TAIL_MAX {
            return Delta { frozen: self.frozen.clone(), tail, groups: self.groups.clone() };
        }
        let mut all: Vec<Entry> = self.frozen.iter().cloned().chain(tail).collect();
        all.sort_by(|a, b| key(a).cmp(&key(b)));
        Self::freeze(all)
    }

    fn freeze(all: Vec<Entry>) -> Delta {
        let mut groups: BTreeMap<(Target, String, String), Vec<usize>> = BTreeMap::new();
        for (i, e) in all.iter().enumerate() {
            groups.entry((e.target, e.schema.clone(), group_of(&e.code, e.boundary))).or_default().push(i);
        }
        Delta { frozen: all.into(), tail: Vec::new(), groups: Arc::new(groups) }
    }

    fn all(&self) -> impl Iterator<Item = &Entry> { self.frozen.iter().chain(self.tail.iter()) }

    pub fn ops_for(&self, target: Target, schema: &str, code: &str, text: &str) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.all().filter(|e| e.target == target && e.schema == schema && e.code == code && e.text == text).collect();
        v.sort_by_key(|e| e.seq);
        v
    }

    pub fn exact(&self, target: Target, schema: &str, code: &str) -> Vec<&Entry> {
        let lo = self.frozen.partition_point(|e| key(e) < (target, schema, code, "", 0));
        let mut v: Vec<&Entry> = self.frozen[lo..].iter().take_while(|e| e.target == target && e.schema == schema && e.code == code).collect();
        v.extend(self.tail.iter().filter(|e| e.target == target && e.schema == schema && e.code == code));
        v.sort_by_key(|e| e.seq);
        v
    }

    pub fn prefix(&self, target: Target, schema: &str, prefix: &str) -> Vec<&Entry> {
        let lo = self.frozen.partition_point(|e| key(e) < (target, schema, prefix, "", 0));
        let mut v: Vec<&Entry> = self.frozen[lo..].iter().take_while(|e| e.target == target && e.schema == schema && e.code.starts_with(prefix)).collect();
        v.extend(self.tail.iter().filter(|e| e.target == target && e.schema == schema && e.code.starts_with(prefix)));
        v.sort_by_key(|e| e.seq);
        v
    }

    pub fn by_group(&self, target: Target, schema: &str, group: &str) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.groups.get(&(target, schema.to_string(), group.to_string())).map(|ix| ix.iter().map(|&i| &self.frozen[i]).collect()).unwrap_or_default();
        v.extend(self.tail.iter().filter(|e| e.target == target && e.schema == schema && group_of(&e.code, e.boundary) == group));
        v
    }

    pub fn drop_folded(&self, target: Target, upto_seq: u64) -> Delta {
        let all: Vec<Entry> = self.all().filter(|e| !(e.target == target && e.seq <= upto_seq)).cloned().collect();
        let mut all = all;
        all.sort_by(|a, b| key(a).cmp(&key(b)));
        Self::freeze(all)
    }

    pub fn len(&self) -> usize { self.frozen.len() + self.tail.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

/// 日志记录 → 增量表条目。`boundary` 由调用方按 code 的音节边界给（无则 0）。
pub fn entry_from_record(r: &Record, boundary: u64) -> Option<Entry> {
    let op = match (r.kind, &r.payload) {
        (OpKind::Upsert | OpKind::Promote, Payload::Weight(w)) => DeltaOp::Upsert { weight: *w, kind: 0 },
        (OpKind::Delete | OpKind::Expire, _) => DeltaOp::Delete,
        (OpKind::SetWeight, Payload::Weight(w)) => DeltaOp::SetWeight(*w),
        (OpKind::Incr, Payload::Count(c)) => DeltaOp::Incr(*c),
        (OpKind::Touch, Payload::Ts(t)) => DeltaOp::Touch(*t),
        _ => return None,
    };
    Some(Entry { seq: r.seq, target: r.target, schema: r.schema.clone(), code: r.code.clone(), text: r.text.clone(), op, boundary })
}
```

`ops_for` 的全表过滤只在写路径与折回用；按键路径走 `exact` / `prefix` / `by_group`。

- [ ] **Step 3: 跑测试通过**；**Step 4: 提交** `feat(userdata): 两级增量表与声母分组索引（设计 §3.3）`

### Task 7: `snapshot.rs`——wdat 段 + 定长 sidecar

**Files:**
- Create: `src/snapshot.rs`
- Test: 同文件 tests

**Interfaces:**
- Consumes: `wind_dict::datformat::{WdatStreamWriter, WdatReader}`、`wind_dict::reader_pool::open_wdat`、`crate::state::atomic_write`
- Produces:
  ```rust
  pub struct SnapshotEntry { pub code: String, pub text: String, pub weight: i32, pub boundary: u64,
                             pub count: u32, pub created_at: i64, pub last_used: i64, pub flags: u8 }
  pub struct Snapshot { pub reader: Arc<WdatReader>, meta: Mmap, pub max_seq: u64, pub entry_count: u32 }
  /// 从**按 (code, text) 排好序**的条目流建段：`<dir>/<name>.wdat` + `<dir>/<name>.meta`。
  pub fn build(dir: &Path, name: &str, max_seq: u64, entries: impl Iterator<Item = SnapshotEntry>) -> anyhow::Result<Snapshot>;
  pub fn open(dir: &Path, name: &str) -> anyhow::Result<Option<Snapshot>>;   // 没有文件 ⇒ None（零段）
  impl Snapshot {
      pub fn meta_of(&self, entry_idx: u32) -> (u32 /*count*/, i64 /*created*/, i64 /*last_used*/, u8 /*flags*/);
      pub fn exact(&self, code: &str) -> Vec<(String, i32, u64, u32 /*entry_idx*/)>;
      pub fn prefix(&self, prefix: &str, limit: usize) -> Vec<(String /*code*/, String, i32, u64, u32)>;
      pub fn abbrev(&self, abbrev: &str, limit: usize) -> Vec<(String /*code*/, String, i32, u64, u32)>;
      pub fn for_each(&self, f: &mut dyn FnMut(SnapshotEntry));
  }
  ```
  sidecar `.meta`：头 32 字节 `"WUDM" | version u16 = 1 | reserved u16 | max_seq u64 | entry_count u32 | reserved [u8;12]`，其后每条 `count u32 | created_at i64 | last_used i64 | flags u8 | pad [u8;3]`（24 字节），按 wdat 条目的全局序号对齐（`for_each` 的顺序 = DAT 键序 × 叶内序）。
  简拼区：每个 code 的（边界 ≥2 音节 ⇒ 首字母串；无边界 ⇒ `\u{1}` + 首字符）两组键都写进 AbbrevSection，值为该 code（权重取该 code 下最高）。这就是 `wind-store::abbrev_index` 的「兜底组」在段里的等价物。

- [ ] **Step 1: 测试**

```rust
    fn se(code: &str, text: &str, w: i32, b: u64, count: u32) -> SnapshotEntry {
        SnapshotEntry { code: code.into(), text: text.into(), weight: w, boundary: b, count, created_at: 100, last_used: 200, flags: 0 }
    }

    #[test]
    fn build_open_query_and_meta_roundtrip() {
        let d = std::env::temp_dir().join(format!("wind_snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut es = vec![se("nihao", "你好", 10, 0b101, 3), se("ni", "你", 50, 0b1, 9), se("nihao", "拟好", 5, 0b101, 1), se("wubi", "五笔", 7, 0, 2)];
        es.sort_by(|a, b| (a.code.as_str(), a.text.as_str()).cmp(&(b.code.as_str(), b.text.as_str())));
        let s = build(&d, "temp", 42, es.into_iter()).unwrap();
        assert_eq!(s.max_seq, 42);
        assert_eq!(s.entry_count, 4);
        let s = open(&d, "temp").unwrap().expect("应能打开");
        let hits = s.exact("nihao");
        assert_eq!(hits.iter().map(|h| h.0.as_str()).collect::<Vec<_>>(), vec!["你好", "拟好"], "同码按权重降序");
        let (count, created, used, _) = s.meta_of(hits[0].3);
        assert_eq!((count, created, used), (3, 100, 200));
        assert_eq!(s.abbrev("nh", 10).len(), 1, "多音节简拼");
        assert_eq!(s.abbrev("\u{1}w", 10).len(), 1, "无边界兜底组");
        assert_eq!(s.prefix("ni", 10).len(), 3);
        assert!(open(&d, "nope").unwrap().is_none(), "没有文件 = 零段，不是错误");
        let _ = std::fs::remove_dir_all(&d);
    }
```

- [ ] **Step 2: 实现**（要点；完整代码按接口写）

```rust
pub fn build(dir: &Path, name: &str, max_seq: u64, entries: impl Iterator<Item = SnapshotEntry>) -> anyhow::Result<Snapshot> {
    std::fs::create_dir_all(dir)?;
    let mut sw = wind_dict::datformat::WdatStreamWriter::new();
    let mut meta = Vec::new();
    let mut abbrevs: Vec<(String, String, i32)> = Vec::new(); // (简拼键, code, 该 code 最高权重)
    let mut cur: Option<String> = None;
    let mut group: Vec<SnapshotEntry> = Vec::new();
    let mut flush = |sw: &mut WdatStreamWriter, code: &str, g: &mut Vec<SnapshotEntry>, meta: &mut Vec<u8>, ab: &mut Vec<(String, String, i32)>| {
        g.sort_by_key(|e| std::cmp::Reverse(e.weight));
        let top = g.first().map(|e| e.weight).unwrap_or(0);
        let b = g.first().map(|e| e.boundary).unwrap_or(0);
        ab.push((crate::delta::group_of(code, b), code.to_string(), top));
        sw.push_code(code, g.iter().enumerate().map(|(i, e)| (e.text.as_str(), e.weight, i as u32, e.boundary)));
        for e in g.iter() {
            meta.extend_from_slice(&e.count.to_le_bytes());
            meta.extend_from_slice(&e.created_at.to_le_bytes());
            meta.extend_from_slice(&e.last_used.to_le_bytes());
            meta.push(e.flags); meta.extend_from_slice(&[0u8; 3]);
        }
        g.clear();
    };
    let mut n = 0u32;
    for e in entries {
        if cur.as_deref() != Some(e.code.as_str()) {
            if let Some(c) = cur.take() { flush(&mut sw, &c, &mut group, &mut meta, &mut abbrevs); }
            cur = Some(e.code.clone());
        }
        group.push(e); n += 1;
    }
    if let Some(c) = cur.take() { flush(&mut sw, &c, &mut group, &mut meta, &mut abbrevs); }
    abbrevs.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)).then(a.1.cmp(&b.1)));
    let mut i = 0;
    while i < abbrevs.len() {
        let k = abbrevs[i].0.clone();
        let mut codes: Vec<(&str, i32)> = Vec::new();
        while i < abbrevs.len() && abbrevs[i].0 == k { codes.push((abbrevs[i].1.as_str(), abbrevs[i].2)); i += 1; }
        if !k.is_empty() { sw.push_abbrev_sorted(&k, codes.into_iter()); }
    }
    let wdat = dir.join(format!("{name}.wdat"));
    sw.build().emit(&wdat)?;                                  // tmp + rename（datformat 内部）
    std::fs::File::open(&wdat)?.sync_all()?;
    let mut head = Vec::with_capacity(32 + meta.len());
    head.extend_from_slice(b"WUDM"); head.extend_from_slice(&1u16.to_le_bytes()); head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&max_seq.to_le_bytes()); head.extend_from_slice(&n.to_le_bytes()); head.extend_from_slice(&[0u8; 12]);
    head.extend_from_slice(&meta);
    crate::state::atomic_write(&dir.join(format!("{name}.meta")), &head)?;
    open(dir, name)?.ok_or_else(|| anyhow::anyhow!("刚建好的段打不开"))
}
```

`open`：两文件任一不存在 ⇒ `Ok(None)`；`reader_pool::open_wdat(&wdat)`；`memmap2::Mmap::map(&File::open(meta))`；校验 magic 与 `entry_count == reader 条目总数`（不符 ⇒ `Err`，调用方当零段重建）。`exact/prefix/abbrev` 包装 `reader.search/search_prefix/search_abbrev`，`entry_idx` 由 `reader.codes()` 的全局计数推导：在 `open` 时建一张 `code → 首条目全局序号` 的 `BTreeMap<String,u32>`（临时词规模几万，常驻几 MB 可接受；P1 若嫌大改为按 leaf 偏移推导）。`for_each` 走 `reader.codes()` + `leaf_entries()`。

- [ ] **Step 3: 跑测试通过**；**Step 4: 提交** `feat(userdata): wdat 段 + 定长 sidecar 快照，简拼区含无边界兜底组（设计 §3.1）`

### Task 8: `engine.rs`——打开、写入发布、折回

**Files:**
- Create: `src/engine.rs`
- Test: `tests/engine_crash_paths.rs`

**Interfaces:**
- Consumes: Task 3–7 全部
- Produces:
  ```rust
  pub struct Dirs { pub state_dir: PathBuf, pub dir_id: String }
  pub struct View { pub snapshots: BTreeMap<(Target, String /*schema*/), Arc<Snapshot>>, pub delta: Arc<Delta> }
  pub struct UserData { /* dirs, journal: Mutex<Journal>, view: RwLock<Arc<View>>, state: Mutex<State>, device, lock: Option<Lock>, write_lock: Mutex<()> */ }
  impl UserData {
      pub fn open(dirs: Dirs) -> anyhow::Result<UserData>;      // 锁、设备、state、各目标快照、回放日志
      pub fn view(&self) -> Arc<View>;                           // 克隆 Arc，读锁只持这一瞬
      pub fn apply(&self, target: Target, schema: &str, kind: OpKind, code: &str, text: &str, payload: Payload, boundary: u64) -> anyhow::Result<u64>; // 追加 + 发布，返回 seq；只读模式返回 Err
      pub fn fold(&self, target: Target, schema: &str) -> anyhow::Result<()>; // 旧快照 + delta → 新快照 → 换入 → 摘 delta → state
      pub fn readonly(&self) -> bool;
  }
  ```
  目标键 `format!("{target:?}:{schema}")` 既作 state 的 key，也作快照文件名 `state/<schema>/<target>`（Draft/Freq/Assoc 也用同一机制，P0 只实现到快照层面，不区分形态）。
  折回语义（Temp 目标）：对 delta 里该 schema 的条目按 (code,text) 分组、按 seq 叠加到旧快照条目上：Upsert 新建或覆盖 weight（count/created 保留，不存在则 created=ts）、Delete 删除、SetWeight 改 weight、Incr 加 count、Touch 改 last_used；输出按 (code,text) 排序喂 `snapshot::build`。

- [ ] **Step 1: 测试**（`tests/engine_crash_paths.rs`）

```rust
use wind_userdata::engine::{Dirs, UserData};
use wind_userdata::record::{OpKind, Payload, Target};

fn dirs(name: &str) -> Dirs {
    let d = std::env::temp_dir().join(format!("wind_ud_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Dirs { state_dir: d, dir_id: "dir".into() }
}
fn texts(ud: &UserData, code: &str) -> Vec<String> {
    let v = ud.view();
    // 由 layer（Task 9）之前先用引擎自带的调试查询
    ud.query_exact(Target::Temp, "py", code).into_iter().map(|(t, _w)| t).collect()
}

#[test]
fn write_is_visible_immediately_and_survives_reopen() {
    let d = dirs("vis");
    let ud = UserData::open(Dirs { state_dir: d.state_dir.clone(), dir_id: d.dir_id.clone() }).unwrap();
    ud.apply(Target::Temp, "py", OpKind::Upsert, "nihao", "你好", Payload::Weight(10), 0b101).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "写后立即可读");
    drop(ud);
    let ud = UserData::open(d).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "重开后由日志回放可读");
}

/// Review Focus 2：state 缺失 ⇒ 以快照内嵌水位为准，只回放更大的 seq，不重复应用。
#[test]
fn state_missing_replays_only_beyond_snapshot_watermark() {
    let d = dirs("wm");
    let ud = UserData::open(Dirs { state_dir: d.state_dir.clone(), dir_id: d.dir_id.clone() }).unwrap();
    ud.apply(Target::Temp, "py", OpKind::Upsert, "a", "甲", Payload::Weight(1), 0).unwrap();
    ud.apply(Target::Temp, "py", OpKind::Incr, "a", "甲", Payload::Count(1), 0).unwrap();
    ud.fold(Target::Temp, "py").unwrap();
    ud.apply(Target::Temp, "py", OpKind::Incr, "a", "甲", Payload::Count(1), 0).unwrap(); // 折回后一条
    drop(ud);
    std::fs::remove_file(d.state_dir.join("journal/state")).unwrap();
    let ud = UserData::open(d).unwrap();
    assert_eq!(ud.query_count(Target::Temp, "py", "a", "甲"), 2, "快照里 1 + 日志里只回放折回后的 1；重复回放会得 3");
}

/// Review Focus 4：折回期间新到的操作保留。
#[test]
fn fold_keeps_ops_that_arrived_during_fold() {
    let d = dirs("race");
    let ud = std::sync::Arc::new(UserData::open(d).unwrap());
    for i in 0..500 { ud.apply(Target::Temp, "py", OpKind::Upsert, &format!("c{i}"), "x", Payload::Weight(1), 0).unwrap(); }
    let w = { let ud = ud.clone(); std::thread::spawn(move || { for i in 500..600 { ud.apply(Target::Temp, "py", OpKind::Upsert, &format!("c{i}"), "x", Payload::Weight(1), 0).unwrap(); } }) };
    ud.fold(Target::Temp, "py").unwrap();
    w.join().unwrap();
    for i in 0..600 { assert_eq!(ud.query_exact(Target::Temp, "py", &format!("c{i}")).len(), 1, "c{i} 丢了"); }
}

#[test]
fn second_instance_is_readonly() {
    let d = dirs("ro");
    let a = UserData::open(Dirs { state_dir: d.state_dir.clone(), dir_id: d.dir_id.clone() }).unwrap();
    let b = UserData::open(d).unwrap();
    assert!(!a.readonly() && b.readonly());
    assert!(b.apply(Target::Temp, "py", OpKind::Upsert, "a", "甲", Payload::Weight(1), 0).is_err());
}
```

- [ ] **Step 2: 实现要点**

```rust
impl UserData {
    pub fn open(dirs: Dirs) -> anyhow::Result<UserData> {
        let lock = crate::device::try_lock(&dirs.state_dir)?;
        let journal_dir = dirs.state_dir.join("journal");
        let device = match crate::device::load_or_renew(&dirs.state_dir, &dirs.dir_id, &journal_dir)? {
            DeviceOutcome::Same(d) => d,
            DeviceOutcome::Renewed { new, sealed_journal } => { if let Some(s) = sealed_journal { tracing::info!("旧日志已封存: {}", s.display()); } new }
        };
        let jpath = journal_dir.join(format!("{}.wjl", crate::device::hex(&device.device_id)));
        let dir_id16 = { let mut b = [0u8; 16]; for (i, x) in dirs.dir_id.bytes().take(16).enumerate() { b[i] = x; } b };
        let journal = Journal::open(&jpath, device.device_id, dir_id16)?;
        let state_path = journal_dir.join("state");
        let mut state = State::load(&state_path).unwrap_or_default();
        // next_seq 以日志尾与 state 较大者为准（§3.6）
        state.next_seq = state.next_seq.max(journal.next_seq());
        // 打开各目标快照：目录 state/<schema>/<target>.wdat 按已有文件枚举
        let mut snapshots = BTreeMap::new();
        for (target, schema, snap) in enumerate_snapshots(&dirs.state_dir)? { snapshots.insert((target, schema), Arc::new(snap)); }
        // 回放：每条记录只在 seq > 该目标快照的 max_seq（state 更可信时取 state.folded_seq，但不得低于快照水位）时进 delta
        let mut delta = Delta::default();
        for r in Journal::replay(&jpath, 0)? {
            let wm = snapshots.get(&(r.target, r.schema.clone())).map(|s| s.max_seq).unwrap_or(0)
                .max(state.targets.get(&target_key(r.target, &r.schema)).map(|t| t.folded_seq).unwrap_or(0));
            if r.seq > wm && let Some(e) = entry_from_record(&r, 0) { delta = delta.with(e); }
        }
        Ok(UserData { dirs, journal: Mutex::new(journal), view: RwLock::new(Arc::new(View { snapshots, delta: Arc::new(delta) })), state: Mutex::new(state), device, lock, write_lock: Mutex::new(()) })
    }

    pub fn apply(&self, target: Target, schema: &str, kind: OpKind, code: &str, text: &str, payload: Payload, boundary: u64) -> anyhow::Result<u64> {
        if self.lock.is_none() { anyhow::bail!("只读模式：另一个实例持有锁"); }
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut j = self.journal.lock().unwrap_or_else(|e| e.into_inner());
        let seq = j.next_seq();
        let rec = Record { seq, ts_ms: now_ms(), kind, target, schema: schema.into(), code: code.into(), text: text.into(), payload };
        j.append(&rec)?;
        drop(j);
        if let Some(e) = entry_from_record(&rec, boundary) {
            let cur = self.view();
            let next = Arc::new(View { snapshots: cur.snapshots.clone(), delta: Arc::new(cur.delta.with(e)) });
            *self.view.write().unwrap_or_else(|e| e.into_inner()) = next;
        }
        Ok(seq)
    }

    pub fn fold(&self, target: Target, schema: &str) -> anyhow::Result<()> {
        if self.lock.is_none() { anyhow::bail!("只读模式"); }
        let base = self.view();                       // 折回起点；期间新到的写入进的是更新的 view
        let upto = self.journal.lock().unwrap_or_else(|e| e.into_inner()).next_seq() - 1;
        let merged = merge_for_fold(base.snapshots.get(&(target, schema.into())).map(|s| s.as_ref()), &base.delta, target, schema, upto);
        let dir = self.dirs.state_dir.join(schema);
        let snap = Arc::new(crate::snapshot::build(&dir, &format!("{target:?}").to_lowercase(), upto, merged.into_iter())?);
        {
            let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());   // 与 apply 互斥，换入期间不丢新 delta
            let cur = self.view();
            let mut snapshots = cur.snapshots.clone();
            snapshots.insert((target, schema.into()), snap.clone());
            let delta = Arc::new(cur.delta.drop_folded_for_schema(target, schema, upto)); // 只摘 seq ≤ upto 的该目标条目
            *self.view.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(View { snapshots, delta });
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.targets.insert(target_key(target, schema), TargetState { folded_seq: upto, text_fp: None, snapshot_fp: String::new() });
        st.next_seq = upto + 1;
        st.save(&self.dirs.state_dir.join("journal/state"))
    }
}
```

`drop_folded_for_schema` 是 Task 6 `drop_folded` 加 schema 过滤的变体（在 Task 6 的 `Delta` 上补这个方法，签名 `pub fn drop_folded_for_schema(&self, target: Target, schema: &str, upto_seq: u64) -> Delta`）。`merge_for_fold` 按接口描述的折回语义把快照条目（`for_each`）与 delta 条目（`ops_for` 逐键）合并成 `Vec<SnapshotEntry>` 并排序。`query_exact` / `query_count` 是引擎自带的调试查询（快照命中 + delta 叠加），Task 9 的 layer 复用同一套叠加函数 `overlay(snapshot_hits, delta_ops) -> Vec<(text, weight, count)>`。

- [ ] **Step 3: 跑测试通过**（含 `fold_keeps_ops_that_arrived_during_fold` 多跑 10 次：`for i in $(seq 10); do cargo test ... fold_keeps || break; done`）
- [ ] **Step 4: 提交** `feat(userdata): 引擎——打开回放、写入即发布、折回换段与水位（设计 §3.4–§3.6）`

### Task 9: `layer.rs`——`DictLayer` 适配（Temp 目标）

**Files:**
- Create: `src/layer.rs`
- Test: 同文件 tests

**Interfaces:**
- Consumes: `UserData::view()`、`wind_dict::layer::{DictLayer, LayerType}`、`wind_candidate::Candidate`
- Produces: `pub struct UserDataTempLayer { ud: Arc<UserData>, schema: String }` 实现 `DictLayer`：`name() = "userdata-temp"`、`layer_type() = LayerType::Temp`、`search` / `search_prefix` / `search_abbrev` / `search_abbrev_exact` 走快照 + delta 叠加；候选字段照 `wind-dict/src/store_layer.rs::record_to_candidate` 的写法（`meta.is_temp_dict = true`、`meta.raw_weight`、`natural_order` 取快照全局序号、`boundary`）。

- [ ] **Step 1: 测试**

```rust
    #[test]
    fn layer_sees_snapshot_delta_and_deletes() {
        let d = std::env::temp_dir().join(format!("wind_layer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ud = Arc::new(UserData::open(Dirs { state_dir: d, dir_id: "x".into() }).unwrap());
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihao", "你好", Payload::Weight(10), 0b101).unwrap();
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihao", "拟好", Payload::Weight(20), 0b101).unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.apply(Target::Temp, "py", OpKind::Delete, "nihao", "拟好", Payload::None, 0b101).unwrap();
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihaoma", "你好吗", Payload::Weight(1), 0b10101).unwrap();
        let layer = UserDataTempLayer::new(ud, "py");
        let names = |v: Vec<wind_candidate::Candidate>| v.into_iter().map(|c| c.text).collect::<Vec<_>>();
        assert_eq!(names(layer.search("nihao", 10)), vec!["你好"], "快照里的 + delta 删除生效");
        assert_eq!(names(layer.search_prefix("niha", 10)), vec!["你好", "你好吗"], "快照 + delta 新增");
        assert_eq!(names(layer.search_abbrev("nh", 10)), vec!["你好"]);
        assert!(layer.search("nihao", 10)[0].meta.is_temp_dict);
    }
```

- [ ] **Step 2: 实现**：`search(code)` = `overlay(snapshot.exact(code), delta.exact(..))`；`search_prefix` = 快照 `prefix(prefix, limit + delta 覆盖条数)` 与 delta `prefix` 合并后按 `wind_candidate::better` 排序截断；`search_abbrev` = 快照 `abbrev(ab)` + delta `by_group(group)`。`overlay` 规则：Delete 剔除、SetWeight 覆盖、Upsert 并入或覆盖、Incr/Touch 不影响候选。

- [ ] **Step 3: 跑测试通过**；**Step 4: 提交** `feat(userdata): DictLayer 适配——快照 + 增量表叠加（设计 §4）`

### Task 10: 文档与收尾

**Files:**
- Create: `wind_input/crates/wind-userdata/AGENTS.md`（照 `wind-dict/AGENTS.md` 的结构：Purpose / Key Files / For AI Agents / Testing / Dependencies）
- Modify: `AGENTS.md`（crate 索引表加一行 `wind-userdata`）
- Modify: `docs/design/user-data-storage-redesign.md`（§10 阶段表「前置」「P0」标记已实现，记测试计数）

- [ ] **Step 1: 写 AGENTS.md 与索引行**；**Step 2: 全量测试** `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata -p wind-config -p wind-coordinator -p wind-webdata --no-fail-fast`；**Step 3: 提交** `docs(userdata): wind-userdata 的 AGENTS.md 与设计文档阶段状态`

---

## 自查记录

- 规格覆盖：§2.1 目录（Task 1）、§7.1 降级保护（Task 2）、§3.2 日志（Task 4）、§3.5 持久化次序（Task 5 `atomic_write`，Task 7/8 使用）、§3.6 state 与水位（Task 5/8）、§3.7 锁与设备（Task 5）、§3.3 delta（Task 6）、§3.1 快照（Task 7）、§3.4 写后可读（Task 8 `apply` 同步发布）、§4 适配（Task 9）。P0 不含文本目标折回（P2）、不含 `dict.reload`（P2）、不含 redb 迁移（P1）。
- 类型一致性：`Target` / `OpKind` / `Payload` / `Record`（Task 3）在 4–9 中签名一致；`Delta::with/exact/prefix/by_group/drop_folded(_for_schema)`；`Snapshot::exact/prefix/abbrev/meta_of/for_each`；`UserData::open/view/apply/fold/readonly/query_exact/query_count`。
- Review Focus 五条各有归属测试（Task 4、8、6、8、5）。
