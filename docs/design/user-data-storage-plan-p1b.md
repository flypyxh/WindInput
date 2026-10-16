# 用户数据存储重设计：P1b 实施计划（接线、迁移、退役）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让生产与全部测试的临时词 / 草稿 / 词频 / 联想历史读写都落到 `wind-userdata` 引擎上；老用户首次启动时把 redb 里这四张表（含临时词简拼索引）一次性迁入引擎；迁移后 `format_version` 升到 2，五张旧表只读保留、不再写入。

**Architecture:** **`Store` 做门面**：四组数据的公开方法签名不变，方法体改为委托给 `Store` 持有的 `wind_userdata::runtime::Runtime`；`words_generation` 系列代次仍由 `Store` 推进，`UserAssocIndex` / `text_codes` 等消费方不动。为此先拆依赖环：`wdict` 抽成独立 crate `wind-wdict`，`store_layer.rs`（三个 Store 层）从 `wind-dict` 挪进 `wind-store`，`wind-dict` 不再依赖 `wind-store`，于是 `wind-store → wind-userdata → wind-dict` 单向成立。`Store::open(path)`（测试、工具）把引擎状态放在同级派生目录 `<path>.state/`；生产走 `Store::open_with_state(path, state_dir, dir_id)`，状态目录是设计 §2.1 的 `local_dir()/state/`。迁移在 `open_*` 内完成，以 `state/migrated-p1` 为完成标记。

**Tech Stack:** Rust 2024；`wind-userdata`（P1a 的 `Runtime`、`UserData::fold_with`、`UserDataTempLayer` / `UserDataDraftLayer`）；redb（只在 `wind-store` 内读旧表）。

**Spec:** `docs/design/user-data-storage-redesign.md`（§2.1 目录、§6 表去向、§7.1 降级保护、§7.2 逐阶段迁移、§10 P1 退出判据、§11 等价验证）；P1a 计划 `docs/design/user-data-storage-plan-p1a.md`。

## 计划内裁定

- **门面委托，而不是逐个调用点改接。** 四类 API 在其他 crate 里有约 200 处引用（协调器 ~60、设置页 53、备份 24、引擎 30，外加几百条直接读 `store.get_freq` / `get_temp_word` 断言结果的测试）。逐处改接的爆炸半径大，且两套后端在测试与生产之间容易分叉。门面委托让调用方零改动，现有几千条测试自动成为新后端的对拍。代价：设计 §6「P4 删除 `store.rs`」的形态改为「P4 时 `Store` 只剩门面、去掉 redb」——同样满足 `cargo tree` 无 redb 的退出判据。
- **`format_version` 只防旧版本写，迁移以标记为准。** 迁移是否已做只看 `state/migrated-p1`；`format_version` 在迁移成功后升到 2，作用是让旧版本（认识 1）进入只读。崩溃在两者之间：标记已写、meta 仍为 1 ⇒ 下次启动再升，无害。
- **迁移按目标整体替换（`fold_with` 返回旧表全部行）**，不合并：重跑得到同一结果，不会重复计数。迁移在 `open_*` 返回之前完成，期间不可能有新写入。
- **旧表不删、不再写**：`TEMP_WORDS` / `TEMP_ABBREV` / `DRAFT_WORDS` / `FREQ` / `ASSOC_HIST` 只读保留到 P4（§7.2「旧表只读保留」）。`list_data_schemas` 不再扫这三张运行时表，改并上 `Runtime::schemas()`。
- **`UserDataTempLayer::search_prefix` 的 `is_prefix` 恒为 true**，与 `StoreTempLayer` 原行为一致（P1a 审查指出的差异），避免排序链上出现新分支。

## Global Constraints

- 四组 API 的签名与返回类型不变（`UserWordRecord` / `UserWordView` / `FreqRecord` / `FreqPage` / `wdict::{WordIo, FreqIo}`）；调用方不改。
- `Store::bump_words_gen` / `bump_words_count_gen` 的推进时机与旧实现逐条一致（旧实现：新词、删词、清空、导入、淘汰到、晋升、草稿转正 ⇒ words_gen；已有临时词 count+1 ⇒ count_gen；词频 / 联想 / 草稿写入不推进）。
- 每次门面写入后调 `after_write()`：`needs_maintain()` 为真才 `spawn_maintain()`（P1a 已保证判定 O(目标数)）。
- 按键路径（`learn_temp_word` / `record_freq` / `get_freq_batch` / 临时词层查询）不得经过 `Store::with_db`（redb Mutex）——这是 §10 的退出判据之一。
- 生产状态目录 = `Config::state_dir()`（`local_dir()/state`）；拿不到时退回派生目录并 WARN。
- 迁移完成标记 `state/migrated-p1` 用 `wind_userdata::state::atomic_write` 写。
- INFO 级日志不得含词条内容。
- 提交走 `git commit -F - --only -- <路径>`；新文件先 `git add <该文件>`；`cargo fmt -p <crate>`，不跑 `--all`。
- 测试：`cd wind_input && TMPDIR=<独立临时目录> cargo test -p <crate> --no-fail-fast`；跨 crate 回归见 Task 8。

## Review Focus

1. **老用户升级**：redb 里有临时词（含边界）、草稿、词频、联想历史 ⇒ 首次启动全部可见，简拼可召回；再次启动不重复迁移。→ Task 6 `legacy_tables_are_migrated_once`。
2. **迁移中途崩溃**（标记未写）⇒ 下次重跑结果与一次成功相同，计数不翻倍。→ Task 6 `rerun_after_crash_does_not_double_count`。
3. **旧版本回退**：迁移后 meta 为 2，认识 1 的版本进入只读；meta 为 1 的目录被新版打开后升到 2。→ Task 7 `opening_migrates_and_raises_format_version`。
4. **测试间串数据**：同一路径删掉 redb 再 `Store::open`，派生状态目录里的旧引擎数据不得残留。→ Task 3 `fresh_redb_starts_with_empty_runtime`。
5. **联想索引失效**：临时词新增 / count+1 后 `words_generation` / `words_count_generation` 照旧推进，`UserAssocIndex` 能看到新词。→ Task 4 `temp_writes_bump_generations_like_before`（wind-engine `user_assoc` 现有测试同时覆盖）。

---

### Task 0: 记录回归基线

**Files:** 无改动。

- [ ] **Step 1: 跑改动前的全量基线并记账**

Run:

```bash
cd wind_input && for c in wind-store wind-dict wind-engine wind-coordinator wind-webdata wind-transfer wind-userdata wind-config; do
  TMPDIR=<独立临时目录> cargo test -p $c --no-fail-fast 2>&1 | grep -E "^test result|FAILED|panicked" > <独立临时目录>/p1b-base-$c.log
  echo "$c: $(grep -c 'FAILED' <独立临时目录>/p1b-base-$c.log) failed, $(awk '/^test result/{s+=$4} END{print s}' <独立临时目录>/p1b-base-$c.log) passed"
done
```

Expected: 各 crate 的通过数与失败数（失败的逐条记进账本 `Task 0: baseline`——它们是既有红，后续任务不负责、但不得增加）。

---

### Task 1: `wdict` 抽成 `wind-wdict` crate

**Files:**
- Create: `wind_input/crates/wind-wdict/Cargo.toml`、`wind_input/crates/wind-wdict/src/lib.rs`（内容 = 原 `wind-store/src/wdict.rs`，含其测试）
- Delete: `wind_input/crates/wind-store/src/wdict.rs`
- Modify: `wind_input/crates/wind-store/src/lib.rs`（`pub mod wdict` → `pub use wind_wdict as wdict;`）、`wind_input/crates/wind-store/Cargo.toml`
- Modify: `wind_input/crates/wind-dict/Cargo.toml`（加 `wind-wdict`）、`wind_input/crates/wind-dict/src/codetable.rs`（3 处 `wind_store::wdict::` → `wind_wdict::`）
- Modify: `wind_input/Cargo.toml`（members 加 `crates/wind-wdict`）

**Interfaces:**
- Produces: crate `wind_wdict`（原 `wind_store::wdict` 全部公开项）；`wind_store::wdict` 路径继续可用（re-export）。

- [ ] **Step 1: 建 crate**

`wind_input/crates/wind-wdict/Cargo.toml`：

```toml
[package]
name = "wind-wdict"
version = "0.1.0"
edition = "2024"

[dependencies]
wind-utils = { path = "../wind-utils" }
# 词条文本域的转义分流判据（`is_cmdbar_grammar`）：命令栏语法条目的反斜杠归 lexer 管，
# 本层不得再转一次。判据只此一份，抄第二份就是下次漂移的起点。
wind-cmdbar = { path = "../wind-cmdbar" }
serde = { workspace = true }

[lints]
workspace = true
```

（`wdict.rs` 若还用到 `serde_json` / `anyhow` / `chrono` 等，按编译报错补到这里，版本写法照 `wind-store/Cargo.toml`。）

```bash
git mv wind_input/crates/wind-store/src/wdict.rs wind_input/crates/wind-wdict/src/lib.rs
```

文件内 `crate::` 引用（若有）按编译报错改为本 crate 内路径；顶部加 crate 文档一行 `//! wdict 文本交换格式（原 wind-store::wdict，抽出以免 wind-dict 依赖用户库）。`。`wind-store/src/lib.rs` 的 `pub mod wdict;` 换成 `pub use wind_wdict as wdict;`，`wind-store/Cargo.toml` 加 `wind-wdict = { path = "../wind-wdict" }`。工作区 `Cargo.toml` members 加一行。

- [ ] **Step 2: `wind-dict` 改依赖**

`wind-dict/Cargo.toml` 加 `wind-wdict = { path = "../wind-wdict" }`；`codetable.rs` 的 `wind_store::wdict::is_misplaced_cmdbar_code` / `is_cmdbar_text` / `unescape_text_field` 改成 `wind_wdict::…`。（`wind-dict` 对 `wind-store` 的依赖此时还在——`store_layer.rs` 用，Task 2 去掉。）

- [ ] **Step 3: 编译与测试**

Run: `cd wind_input && cargo build -p wind-wdict -p wind-store -p wind-dict 2>&1 | grep -E "^error" | head; TMPDIR=<独立临时目录> cargo test -p wind-wdict -p wind-store -p wind-dict --no-fail-fast 2>&1 | grep -E "FAILED|test result" | sort | uniq -c`
Expected: 无编译错误；`wind-wdict` 的测试数 = 原 `wdict.rs` 测试数；`wind-store` 通过数 = 基线减去 `wdict` 那部分；无新增 FAILED。

- [ ] **Step 4: 提交**

```bash
git add wind_input/crates/wind-wdict
cd wind_input && cargo fmt -p wind-wdict -p wind-store -p wind-dict && cd .. && git commit -F - --only -- wind_input/Cargo.toml wind_input/Cargo.lock wind_input/crates/wind-wdict wind_input/crates/wind-store wind_input/crates/wind-dict <<'EOF'
refactor(wdict): 抽成独立 crate wind-wdict，wind-store 原路径 re-export

wind-dict 只为三个文本函数依赖 wind-store，挡住了 wind-store → wind-userdata → wind-dict 的方向。
EOF
```

---

### Task 2: 三个 Store 层挪进 `wind-store`，`wind-dict` 去掉 `wind-store` 依赖

**Files:**
- Move: `wind_input/crates/wind-dict/src/store_layer.rs` → `wind_input/crates/wind-store/src/store_layer.rs`
- Modify: `wind-dict/src/lib.rs`（删 `pub mod store_layer;` 与 `pub use store_layer::{…}`）、`wind-dict/Cargo.toml`（删 `wind-store`；`[dev-dependencies]` 若测试需要也删）
- Modify: `wind-store/src/lib.rs`（`pub mod store_layer; pub use store_layer::{StoreDraftLayer, StoreTempLayer, StoreUserLayer};`）、`wind-store/Cargo.toml`（加 `wind-dict`、`wind-candidate`）
- Modify（路径 `wind_dict::Store*Layer` → `wind_store::Store*Layer`）：`wind-engine/src/manager.rs`、`wind-engine/src/pinyin/mod.rs`、`wind-engine/tests/pinyin_*.rs`（7 个）、`wind-coordinator/tests/user_word_order.rs`、`wind-coordinator/src/handle_draft.rs`、`wind-phrase/src/lib.rs`、`wind-candidate/src/candidate.rs`（仅注释）、`wind-dict/src/{layer,composite}.rs`（仅注释）

**Interfaces:**
- Produces: `wind_store::{StoreUserLayer, StoreTempLayer, StoreDraftLayer}`，签名不变。

- [ ] **Step 1: 挪文件**

```bash
git mv wind_input/crates/wind-dict/src/store_layer.rs wind_input/crates/wind-store/src/store_layer.rs
```

`store_layer.rs` 内：`use crate::layer::{DictLayer, LayerType};` → `use wind_dict::{DictLayer, LayerType};`；`use wind_store::Store;` → `use crate::Store;`；`use wind_store::user_words::UserWordRecord;` → `use crate::user_words::UserWordRecord;`；`crate::layer::literal_prefix` / `pattern_matches` / `cmp_pattern` → `wind_dict::layer::…`；测试模块里的 `crate::composite::CompositeDict` → `wind_dict::CompositeDict`。

- [ ] **Step 2: 改依赖与引用**

按 Files 清单改。`wind-dict/Cargo.toml` 删掉 `wind-store` 后跑 `cargo tree -p wind-dict | grep wind-store`，Expected 无输出。引用改名用：

```bash
cd wind_input && grep -rlE "wind_dict::(StoreUserLayer|StoreTempLayer|StoreDraftLayer|store_layer)" --include='*.rs' crates | xargs sed -i -E 's/wind_dict::store_layer::/wind_store::store_layer::/g; s/wind_dict::(StoreUserLayer|StoreTempLayer|StoreDraftLayer)/wind_store::\1/g'
```

`use wind_dict::{…, StoreTempLayer, …}` 这类成组导入用 `grep -rn "StoreTempLayer\|StoreUserLayer\|StoreDraftLayer" --include='*.rs' crates | grep "use wind_dict"` 找出来手改。

- [ ] **Step 3: 编译与测试**

Run: `cd wind_input && cargo build --workspace --all-targets 2>&1 | grep -E "^error" | head; for c in wind-store wind-dict wind-engine wind-coordinator wind-phrase; do TMPDIR=<独立临时目录> cargo test -p $c --no-fail-fast 2>&1 | grep -E "^test result" | awk -v c=$c '{p+=$4; f+=$6} END{print c, p, "passed", f, "failed"}'; done`
Expected: 全工作区编译通过；各 crate 通过数与 Task 0 基线一致（`store_layer` 的测试从 `wind-dict` 计入 `wind-store`，两者之和不变），失败数不超过基线。

- [ ] **Step 4: 提交**

```bash
cd wind_input && cargo fmt -p wind-store -p wind-dict -p wind-engine -p wind-coordinator -p wind-phrase -p wind-candidate && cd .. && git commit -F - --only -- wind_input/Cargo.lock wind_input/crates <<'EOF'
refactor(store): Store 三层从 wind-dict 挪进 wind-store，wind-dict 不再依赖用户库
EOF
```

（`--only -- wind_input/crates` 前先 `git status --short wind_input/crates` 核对：只能有本任务改的文件；有别人的未提交改动就逐个列路径。）

---

### Task 3: `Store` 持有 `Runtime`（尚不委托）

**Files:**
- Modify: `wind_input/crates/wind-store/Cargo.toml`（加 `wind-userdata`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（字段、`open` / `open_with_state` / `open_with_cache_size`、`runtime()`、`after_write()`）
- Test: `wind-store/src/store.rs` tests 模块

**Interfaces:**
- Produces:
  - `pub struct StateLoc { pub state_dir: PathBuf, pub dir_id: String }`
  - `Store::open(path)`：引擎状态放在派生目录 `derived_state_dir(path)` = `path` 文件名后追加 `.state`（`userdata.redb` → `userdata.redb.state/`），dir_id 固定 `"derived"`；**打开前 redb 文件不存在时先清空派生目录**（新库配新状态）。
  - `Store::open_with_state(path, loc: StateLoc) -> anyhow::Result<Store>`：生产用，不清任何目录。
  - `Store::runtime(&self) -> &Arc<wind_userdata::runtime::Runtime>`
  - `pub(crate) fn after_write(&self)`

- [ ] **Step 1: 写失败测试（store.rs tests）**

```rust
    fn tmp_db(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_store_rt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d.join(format!("{name}.redb"))
    }

    #[test]
    fn open_attaches_runtime_in_derived_state_dir() {
        let p = tmp_db("derived");
        let _ = std::fs::remove_file(&p);
        let s = Store::open(&p).unwrap();
        s.runtime().learn_temp("py", "a", "甲", 1, 0).unwrap();
        assert!(derived_state_dir(&p).join("journal").is_dir());
        drop(s);
        let s = Store::open(&p).unwrap();
        assert!(s.runtime().temp_word("py", "a", "甲").is_some(), "同一库重开，引擎数据仍在");
    }

    /// Review Focus 4：删掉 redb 再开同一路径 = 新库，派生目录里的旧引擎数据不得残留。
    #[test]
    fn fresh_redb_starts_with_empty_runtime() {
        let p = tmp_db("fresh");
        let _ = std::fs::remove_file(&p);
        let s = Store::open(&p).unwrap();
        s.runtime().learn_temp("py", "a", "甲", 1, 0).unwrap();
        drop(s);
        std::fs::remove_file(&p).unwrap();
        let s = Store::open(&p).unwrap();
        assert!(s.runtime().temp_word("py", "a", "甲").is_none());
    }

    #[test]
    fn open_with_state_uses_the_given_dir_and_never_wipes() {
        let p = tmp_db("explicit");
        let _ = std::fs::remove_file(&p);
        let st = std::env::temp_dir().join(format!("wind_store_rt_state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&st);
        let loc = || StateLoc {
            state_dir: st.clone(),
            dir_id: "dir-x".into(),
        };
        let s = Store::open_with_state(&p, loc()).unwrap();
        s.runtime().learn_temp("py", "a", "甲", 1, 0).unwrap();
        drop(s);
        std::fs::remove_file(&p).unwrap();
        let s = Store::open_with_state(&p, loc()).unwrap();
        assert!(s.runtime().temp_word("py", "a", "甲").is_some(), "显式状态目录不随 redb 清空");
        assert!(!derived_state_dir(&p).exists());
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store store::tests 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`runtime` / `derived_state_dir` / `StateLoc` / `open_with_state` 未定义。

- [ ] **Step 3: 实现**

`wind-store/Cargo.toml` 加 `wind-userdata = { path = "../wind-userdata" }`。`store.rs`：

```rust
/// 引擎状态目录与目录 id（设计 §2.1 / §3.7）。生产 = `local_dir()/state` + `meta.toml` 的 dir_id。
#[derive(Clone, Debug)]
pub struct StateLoc {
    pub state_dir: PathBuf,
    pub dir_id: String,
}

/// `Store::open(path)` 的引擎状态目录：redb 文件名后追加 `.state`。测试与工具用；生产走
/// [`Store::open_with_state`]。
pub fn derived_state_dir(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".state");
    PathBuf::from(s)
}
```

`Store` 加字段（放在 `path` 之后）：

```rust
    /// 运行时数据引擎（临时词 / 草稿 / 词频 / 联想历史，设计 §6 P1）。四组 API 委托给它。
    runtime: std::sync::Arc<wind_userdata::runtime::Runtime>,
```

`open` 改为：

```rust
    /// 打开数据库，引擎状态放在派生目录（见 [`derived_state_dir`]）。redb 文件此前不存在 ⇒
    /// 视为新库，先清空派生目录（新库配新状态，测试反复用同一路径时不串数据）。
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let derived = derived_state_dir(path);
        if !path.exists() {
            let _ = std::fs::remove_dir_all(&derived);
        }
        Self::open_inner(
            path,
            DEFAULT_CACHE_SIZE_BYTES,
            StateLoc {
                state_dir: derived,
                dir_id: "derived".into(),
            },
        )
    }

    /// 生产入口：引擎状态放在给定目录，从不清空。
    pub fn open_with_state(path: impl AsRef<Path>, loc: StateLoc) -> anyhow::Result<Self> {
        Self::open_inner(path.as_ref(), DEFAULT_CACHE_SIZE_BYTES, loc)
    }

    /// 指定页缓存上限打开（状态目录同 [`Self::open`]）。供 `perf_cache_size.rs` 标定用。
    pub fn open_with_cache_size(path: impl AsRef<Path>, cache_bytes: usize) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let derived = derived_state_dir(path);
        if !path.exists() {
            let _ = std::fs::remove_dir_all(&derived);
        }
        Self::open_inner(
            path,
            cache_bytes,
            StateLoc {
                state_dir: derived,
                dir_id: "derived".into(),
            },
        )
    }
```

原 `open_with_cache_size` 的函数体改名为 `fn open_inner(path: &Path, cache_bytes: usize, loc: StateLoc) -> anyhow::Result<Self>`，在构造 `Self` 之前打开引擎：

```rust
        let ud = wind_userdata::engine::UserData::open(wind_userdata::engine::Dirs {
            state_dir: loc.state_dir.clone(),
            dir_id: loc.dir_id.clone(),
        })?;
        let runtime = std::sync::Arc::new(wind_userdata::runtime::Runtime::new(std::sync::Arc::new(ud)));
```

构造里加 `runtime,`。再加：

```rust
    /// 运行时数据引擎。
    pub fn runtime(&self) -> &std::sync::Arc<wind_userdata::runtime::Runtime> {
        &self.runtime
    }

    /// 门面写入之后：攒够了就起后台折回（判定 O(目标数)，单飞）。
    pub(crate) fn after_write(&self) {
        let d = self.runtime.data();
        if d.needs_maintain() {
            d.spawn_maintain();
        }
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store --no-fail-fast 2>&1 | grep -E "FAILED|test result" | awk '/test result/{p+=$4;f+=$6} /FAILED/{print} END{print p, "passed", f, "failed"}'`
Expected: 新增 3 条通过；通过数 = Task 2 后 + 3；无新增失败。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-store && cd .. && git commit -F - --only -- wind_input/Cargo.lock wind_input/crates/wind-store <<'EOF'
feat(store): Store 持有运行时引擎；Store::open 用派生状态目录，生产走 open_with_state
EOF
```

---

### Task 4: 临时词委托给引擎

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/runtime/temp.rs`（`learn_temp_detailed`）
- Modify: `wind_input/crates/wind-userdata/src/layer.rs`（`search_prefix` 的 `is_prefix` 恒 true）
- Modify: `wind_input/crates/wind-store/src/temp_words.rs`（全部公开方法改委托；`promote_temp_word` 改为「读引擎 → 写 redb 用户词 → 删引擎临时词」）
- Modify: `wind_input/crates/wind-store/src/abbrev_index.rs`（`search_temp_words_by_abbrev(_exact)` 改走引擎层）
- Modify: `wind_input/crates/wind-store/src/store_layer.rs`（`StoreTempLayer` 包一层 `UserDataTempLayer`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（`list_data_schemas` 不扫运行时三表，并上 `runtime().schemas()`；`backfill_abbrev_indexes` 只看用户词表）
- Test: `wind-store/src/temp_words.rs` 与 `abbrev_index.rs` 现有测试（对拍）；新增 `temp_writes_bump_generations_like_before`、`temp_hot_path_does_not_touch_redb`

**Interfaces:**
- Consumes: Task 3 的 `runtime()` / `after_write()`；P1a 的 `Runtime` 临时词 API、`UserDataTempLayer`。
- Produces: `Runtime::learn_temp_detailed(&self, schema, code, text, add_weight, boundary) -> anyhow::Result<(u32, bool)>`（新 count、是否新词）；`learn_temp` 改为委托它。

- [ ] **Step 1: 写失败测试（temp_words.rs tests 末尾）**

```rust
    /// Review Focus 5：门面委托后代次推进时机与旧实现一致。
    #[test]
    fn temp_writes_bump_generations_like_before() {
        let s = store("gens");
        let (w0, c0) = (s.words_generation(), s.words_count_generation());
        s.learn_temp_word("py", "a", "甲", 800, 0).unwrap();
        assert!(s.words_generation() > w0, "新词推进结构代次");
        let (w1, c1) = (s.words_generation(), s.words_count_generation());
        assert_eq!(c1, c0);
        s.learn_temp_word("py", "a", "甲", 800, 0).unwrap();
        assert_eq!(s.words_generation(), w1, "count+1 不推进结构代次");
        assert!(s.words_count_generation() > c1);
        s.remove_temp_word("py", "a", "甲").unwrap();
        assert!(s.words_generation() > w1);
    }

    /// §10 退出判据：按键路径（学习 / 点查 / 临时词层查询）不经过 redb Mutex。
    #[test]
    fn temp_hot_path_does_not_touch_redb() {
        let s = std::sync::Arc::new(store("hot"));
        s.pause().unwrap(); // redb 暂停：with_db 一律报错
        s.learn_temp_word("py", "nihao", "你好", 800, 0b101).unwrap();
        assert_eq!(s.get_temp_word("py", "nihao", "你好").unwrap(), Some(1));
        let layer = crate::StoreTempLayer::new(s.clone(), "py");
        use wind_dict::DictLayer;
        assert_eq!(layer.search("nihao", 0).len(), 1);
        assert_eq!(layer.search_abbrev("nh", 0).len(), 1);
    }
```

（`store(name)` 是本模块测试已有的打开辅助；若名字不同，用它的实际名字。`pause` 是 `pub fn pause(&self) -> anyhow::Result<()>`。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store temp_ 2>&1 | grep -E "error\[|FAILED|panicked|test result"`
Expected: `temp_hot_path_does_not_touch_redb` FAIL（暂停后 `learn_temp_word` 走 redb 报错）；代次测试按旧实现应当通过（它是门面改造的护栏，不需要红）。

- [ ] **Step 3: 实现**

`wind-userdata/src/runtime/temp.rs`：把 `learn_temp` 的函数体挪进新方法并让 `learn_temp` 委托：

```rust
    /// 同 [`Self::learn_temp`]，另返回是否新词（门面据此推进结构代次或计数代次）。
    pub fn learn_temp_detailed(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        add_weight: i32,
        boundary: u64,
    ) -> anyhow::Result<(u32, bool)> {
        let (ops, count, is_new) = self.temp_learn_ops(schema, code, text, add_weight, boundary);
        self.ud.apply_batch(&ops)?;
        if is_new {
            self.bump_words(schema);
        } else {
            self.bump_count();
        }
        Ok((count, is_new))
    }

    pub fn learn_temp(&self, schema: &str, code: &str, text: &str, add_weight: i32, boundary: u64) -> anyhow::Result<u32> {
        Ok(self.learn_temp_detailed(schema, code, text, add_weight, boundary)?.0)
    }
```

`wind-userdata/src/layer.rs` 的 `search_prefix`：`Self::candidates(overlay_entries(base, ops), Some(prefix))` 改为 `Self::candidates(overlay_entries(base, ops), None)` 之后把每个候选 `is_prefix = true`（与 `StoreTempLayer` 的 `record_to_candidate(r, true, true)` 一致）：

```rust
        let mut v = Self::candidates(overlay_entries(base, ops), None);
        for c in &mut v {
            c.is_prefix = true;
        }
        sort_trunc(v, limit)
```

（`prefix_with_zero_limit_returns_snapshot_entries_too` 不看 `is_prefix`，不受影响。）

`wind-store/src/temp_words.rs`：顶部加转换函数，公开方法体按下表替换（签名不动；文档注释保留并在首行后补一句「P1 起委托运行时引擎」）。

```rust
fn rec_of(w: wind_userdata::runtime::TempWord) -> UserWordRecord {
    UserWordRecord {
        code: w.code,
        text: w.text,
        weight: w.weight,
        count: w.count,
        created_at: w.created_at,
        boundary: w.boundary,
        order: 0,
    }
}
```

```rust
    pub fn learn_temp_word(&self, schema: &str, code: &str, text: &str, add_weight: i32, boundary: u64) -> anyhow::Result<u32> {
        let (count, is_new) = self
            .runtime
            .learn_temp_detailed(schema, code, text, add_weight, boundary)?;
        if is_new {
            self.bump_words_gen(schema);
        } else {
            self.bump_words_count_gen();
        }
        self.after_write();
        Ok(count)
    }

    pub fn get_temp_word(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<Option<u32>> {
        Ok(self.runtime.temp_word(schema, code, text).map(|w| w.count))
    }

    pub fn increment_temp_if_exists(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<(bool, u32)> {
        let r = self.runtime.increment_temp_if_exists(schema, code, text)?;
        if r.0 {
            self.bump_words_count_gen();
            self.after_write();
        }
        Ok(r)
    }

    pub fn get_temp_words(&self, schema: &str, code: &str) -> anyhow::Result<Vec<UserWordRecord>> {
        Ok(self.runtime.temp_words(schema, code).into_iter().map(rec_of).collect())
    }

    pub fn for_each_temp_word(
        &self,
        schema: &str,
        prefix: &str,
        f: &mut dyn FnMut(crate::user_words::UserWordView<'_>) -> bool,
    ) -> anyhow::Result<()> {
        self.runtime.for_each_temp(schema, prefix, &mut |w| {
            f(crate::user_words::UserWordView {
                code: &w.code,
                text: &w.text,
                weight: w.weight,
                count: w.count,
                created_at: w.created_at,
                boundary: w.boundary,
                order: 0,
            })
        });
        Ok(())
    }

    pub fn count_temp_words(&self, schema: &str) -> anyhow::Result<usize> {
        Ok(self.runtime.count_temp(schema))
    }

    pub fn search_temp_words_prefix(&self, schema: &str, prefix: &str, limit: usize) -> anyhow::Result<Vec<UserWordRecord>> {
        let mut out = Vec::new();
        self.runtime.for_each_temp(schema, prefix, &mut |w| {
            out.push(rec_of(w.clone()));
            limit == 0 || out.len() < limit
        });
        Ok(out)
    }

    pub fn evict_temp_words(&self, schema: &str, max_keep: usize) -> anyhow::Result<usize> {
        let n = self.runtime.evict_temp(schema, max_keep)?;
        if n > 0 {
            self.bump_words_gen(schema);
            self.after_write();
        }
        Ok(n)
    }

    pub fn import_temp_word_rows(&self, schema: &str, rows: &[wdict::WordIo]) -> anyhow::Result<usize> {
        let rows: Vec<wind_userdata::runtime::TempWord> = rows
            .iter()
            .map(|r| {
                let (code, spaced_b) = wdict::split_spaced_code(&r.code);
                wind_userdata::runtime::TempWord {
                    code,
                    text: r.text.clone(),
                    weight: r.weight,
                    count: r.count,
                    created_at: 0,
                    boundary: r.boundary.unwrap_or(spaced_b),
                }
            })
            .collect();
        let n = self.runtime.import_temp_rows(schema, &rows)?;
        self.bump_words_gen(schema);
        Ok(n)
    }

    pub fn clear_temp_words(&self, schema: &str) -> anyhow::Result<usize> {
        let n = self.runtime.clear_temp(schema)?;
        self.bump_words_gen(schema);
        Ok(n)
    }

    pub fn remove_temp_word(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        self.runtime.remove_temp(schema, code, text)?;
        self.bump_words_gen(schema);
        self.after_write();
        Ok(())
    }
```

`export_temp_words_wdict` / `import_temp_words_wdict` 不改（它们建在上面的方法之上）。`promote_temp_word`：开头 `let Some(tw) = self.runtime.temp_word(schema, code, text) else { return Ok(false); };`，用 `(tw.count, tw.created_at, tw.boundary)` 替代原先从 `TEMP_WORDS` 读出的 `(tc, tca, tb)`；redb 事务里**只保留**写 `USER_WORDS` 与 `USER_ABBREV` 的那一段（删掉 `temp_t` 的打开、`temp_t.remove`、`TEMP_ABBREV` 的 `remove`）；`txn.commit()` 之后 `self.runtime.remove_temp(schema, code, text)?; self.bump_words_gen(schema); Ok(true)`。先写用户词再删临时词：崩溃在两步之间两边各有一份，用户层去重兜得住；反过来会丢词。

`abbrev_index.rs`：`search_temp_words_by_abbrev` / `_exact` 改为经引擎层（不再读 `TEMP_ABBREV`）：

```rust
    pub fn search_temp_words_by_abbrev(&self, schema: &str, abbrev: &str, limit: usize) -> anyhow::Result<Vec<UserWordRecord>> {
        use wind_dict::DictLayer;
        let l = wind_userdata::layer::UserDataTempLayer::new(self.runtime().data().clone(), schema);
        Ok(l.search_abbrev(abbrev, limit).into_iter().map(record_of_candidate).collect())
    }

    pub fn search_temp_words_by_abbrev_exact(&self, schema: &str, abbrev: &str, limit: usize) -> anyhow::Result<Vec<UserWordRecord>> {
        use wind_dict::DictLayer;
        let l = wind_userdata::layer::UserDataTempLayer::new(self.runtime().data().clone(), schema);
        Ok(l.search_abbrev_exact(abbrev, limit).into_iter().map(record_of_candidate).collect())
    }
```

```rust
/// 引擎层候选 → 记录（count / created_at 不在候选里，补 0；调用方只用 code/text/weight/boundary）。
fn record_of_candidate(c: wind_candidate::Candidate) -> UserWordRecord {
    UserWordRecord {
        code: c.code,
        text: c.text,
        weight: c.weight,
        count: 0,
        created_at: 0,
        boundary: c.boundary,
        order: 0,
    }
}
```

`abbrev_index_len` / `rebuild_abbrev_indexes` 里与 `TEMP_ABBREV` / `TEMP_WORDS` 相关的项删掉，只管用户词；`store.rs` 的 `backfill_abbrev_indexes` 同样只比 `USER_ABBREV` 与 `USER_WORDS`。

`store_layer.rs` 的 `StoreTempLayer` 改为包一层：

```rust
/// 临时词层：P1 起数据在运行时引擎里，本类型只是 [`wind_userdata::layer::UserDataTempLayer`]
/// 的薄包装（保留 `StoreTempLayer::new(store, schema)` 这个入口，调用方不动）。
pub struct StoreTempLayer {
    inner: wind_userdata::layer::UserDataTempLayer,
}

impl StoreTempLayer {
    pub fn new(store: Arc<Store>, schema_id: impl Into<String>) -> Self {
        let schema_id = schema_id.into();
        Self {
            inner: wind_userdata::layer::UserDataTempLayer::new(store.runtime().data().clone(), &schema_id),
        }
    }
}

impl DictLayer for StoreTempLayer {
    fn name(&self) -> &str { self.inner.name() }
    fn layer_type(&self) -> LayerType { self.inner.layer_type() }
    fn entries_digest(&self) -> Option<String> { self.inner.entries_digest() }
    fn search(&self, code: &str, limit: usize) -> Vec<Candidate> { self.inner.search(code, limit) }
    fn search_prefix(&self, prefix: &str, limit: usize) -> Vec<Candidate> { self.inner.search_prefix(prefix, limit) }
    fn search_abbrev(&self, abbrev: &str, limit: usize) -> Vec<Candidate> { self.inner.search_abbrev(abbrev, limit) }
    fn search_abbrev_exact(&self, abbrev: &str, limit: usize) -> Vec<Candidate> { self.inner.search_abbrev_exact(abbrev, limit) }
    fn search_pattern(&self, pattern: &str, wildcard: char, limit: usize, with_prefix: bool) -> Vec<Candidate> {
        self.inner.search_pattern(pattern, wildcard, limit, with_prefix)
    }
}
```

（`DictLayer` 的其他方法——`has_longer_code` 等——若 `StoreTempLayer` 原先覆盖过，同样转发；没覆盖过的走 trait 默认。）

`store.rs` 的 `list_data_schemas`：redb 只扫 `[USER_WORDS, SHADOW]`，之后 `set.extend(self.runtime.schemas());`。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|panicked|test result" | head -30`
Expected: 新增两条通过；`temp_words.rs` 原有对拍测试全部通过。**预期会红、按语义处理的旧测试**（它们断言 redb 内部结构，不是外部行为）：`every_temp_write_path_maintains_the_index`、`temp_count_bump_does_not_touch_the_index`、`reopening_an_unindexed_db_backfills_automatically`、`rebuild_covers_both_tables` 中针对 `TEMP_ABBREV` 的部分——删除这些断言（或整条用例，若它只测临时词索引），保留用户词部分，并在账本记 `Task 4: Ruling`（列出删了哪几条、为什么）。其余失败一律当实现缺陷修。

- [ ] **Step 5: 跨 crate 回归**

Run: `cd wind_input && for c in wind-engine wind-coordinator wind-webdata wind-transfer; do TMPDIR=<独立临时目录> cargo test -p $c --no-fail-fast 2>&1 | grep -E "^test result" | awk -v c=$c '{p+=$4; f+=$6} END{print c, p, "passed", f, "failed"}'; done`
Expected: 通过 / 失败数与 Task 0 基线一致。若有新红，先确认是不是测试在断言 redb 内部（如直接开 `TEMP_WORDS` 表），是则按上一步的规则处理并记 Ruling；否则是门面语义偏差，修门面。

- [ ] **Step 6: 提交**

```bash
cd wind_input && cargo fmt -p wind-store -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-store wind_input/crates/wind-userdata <<'EOF'
feat(store): 临时词委托运行时引擎（门面签名不变）

按键路径不再经 redb Mutex；晋升先写 redb 用户词再删引擎临时词；临时词层改包 UserDataTempLayer。
EOF
```

---

### Task 5: 草稿、词频、联想历史委托给引擎

**Files:**
- Modify: `wind_input/crates/wind-store/src/draft_words.rs`、`freq.rs`、`assoc_history.rs`、`store_layer.rs`（`StoreDraftLayer` 包 `UserDataDraftLayer`）
- Test: 三个文件的现有测试（对拍）；新增 `draft_freq_assoc_hot_paths_do_not_touch_redb`

**Interfaces:**
- Consumes: Task 3 / 4；P1a 的草稿、词频、联想 API 与 `UserDataDraftLayer`。

- [ ] **Step 1: 写失败测试（freq.rs tests 末尾）**

```rust
    /// §10 退出判据：词频记录 / 批量查、联想、草稿读写都不经过 redb Mutex。
    #[test]
    fn draft_freq_assoc_hot_paths_do_not_touch_redb() {
        let p = std::env::temp_dir().join(format!("wind_freq_hot-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let s = std::sync::Arc::new(Store::open(&p).unwrap());
        s.pause().unwrap();
        s.record_freq("py", "nihao", "你好").unwrap();
        let got = s
            .get_freq_batch(&[("py".into(), "nihao".into(), "你好".into())])
            .unwrap();
        assert_eq!(got[0].map(|r| r.count), Some(1));
        s.record_assoc_pick("py", "天", "气").unwrap();
        assert_eq!(s.assoc_history("py", "天", 0).unwrap().len(), 1);
        s.add_drafts("wb", &[("ggtt".into(), "盖伦".into())]).unwrap();
        assert_eq!(s.search_drafts("wb", "ggtt", 86_400).unwrap(), vec!["盖伦"]);
        use wind_dict::DictLayer;
        let l = crate::StoreDraftLayer::new(s.clone(), "wb", 86_400);
        assert_eq!(l.search("ggtt", 0).len(), 1);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store draft_freq_assoc_hot 2>&1 | grep -E "error\[|panicked|test result"`
Expected: FAIL（暂停后 `record_freq` 报错）。

- [ ] **Step 3: 实现**

转换函数（`freq.rs` 顶部，`pub(crate)`，`assoc_history.rs` 共用）：

```rust
pub(crate) fn freq_of(r: wind_userdata::runtime::FreqRec) -> FreqRecord {
    FreqRecord {
        count: r.count,
        last_used: r.last_used,
    }
}

pub(crate) fn rec_of_freq(r: FreqRecord) -> wind_userdata::runtime::FreqRec {
    wind_userdata::runtime::FreqRec {
        count: r.count,
        last_used: r.last_used,
    }
}
```

`draft_words.rs`（签名不动）：

```rust
    pub fn add_drafts(&self, schema: &str, items: &[(String, String)]) -> anyhow::Result<usize> {
        let n = self.runtime.add_drafts(schema, items)?;
        self.after_write();
        Ok(n)
    }
    pub fn search_drafts(&self, schema: &str, code: &str, ttl_secs: i64) -> anyhow::Result<Vec<String>> {
        Ok(self.runtime.search_drafts(schema, code, ttl_secs))
    }
    pub fn has_draft(&self, schema: &str, code: &str, text: &str, ttl_secs: i64) -> bool {
        self.runtime.has_draft(schema, code, text, ttl_secs)
    }
    pub fn purge_expired_drafts(&self, schema: &str, ttl_secs: i64) -> anyhow::Result<usize> {
        Ok(self.runtime.purge_expired_drafts(Some(schema), ttl_secs)?.0)
    }
    pub fn purge_all_expired_drafts(&self, ttl_secs: i64) -> anyhow::Result<(usize, usize)> {
        self.runtime.purge_expired_drafts(None, ttl_secs)
    }
    pub fn evict_drafts(&self, schema: &str, max_keep: usize) -> anyhow::Result<usize> {
        self.runtime.evict_drafts(schema, max_keep)
    }
    pub fn promote_draft_to_temp(&self, schema: &str, code: &str, text: &str, add_weight: i32, boundary: u64) -> anyhow::Result<bool> {
        let ok = self.runtime.promote_draft_to_temp(schema, code, text, add_weight, boundary)?;
        if ok {
            self.bump_words_gen(schema);
            self.after_write();
        }
        Ok(ok)
    }
    pub fn count_drafts(&self, schema: &str) -> anyhow::Result<usize> {
        Ok(self.runtime.count_drafts(schema))
    }
    pub fn clear_drafts(&self, schema: &str) -> anyhow::Result<usize> {
        self.runtime.clear_drafts(schema)
    }
```

（`parse_draft_key` 若只被旧实现用，留给 Task 6 的迁移读旧表；`enc_draft` / `dec_draft` / `is_expired` 同理。）

`freq.rs`：

```rust
    pub fn record_freq(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        self.runtime.record_freq(schema, code, text)?;
        self.after_write();
        Ok(())
    }
    pub fn get_freq_batch(&self, keys: &[(String, String, String)]) -> anyhow::Result<Vec<Option<FreqRecord>>> {
        Ok(self.runtime.freq_batch(keys).into_iter().map(|r| r.map(freq_of)).collect())
    }
    pub fn get_freq(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<Option<FreqRecord>> {
        Ok(self.runtime.freq(schema, code, text).map(freq_of))
    }
    pub fn for_each_freq(&self, schema: &str, prefix: &str, f: &mut dyn FnMut(&str, &str, FreqRecord) -> bool) -> anyhow::Result<()> {
        self.runtime.for_each_freq(schema, prefix, &mut |c, t, r| f(c, t, freq_of(r)));
        Ok(())
    }
    pub fn list_freq_paged(&self, schema: &str, prefix: &str, offset: usize, limit: usize) -> anyhow::Result<FreqPage> {
        let (rows, total) = self.runtime.list_freq_paged(schema, prefix, offset, limit);
        Ok((rows.into_iter().map(|(c, t, r)| (c, t, freq_of(r))).collect(), total))
    }
    pub fn delete_freq(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        self.runtime.delete_freq(schema, code, text)
    }
    pub fn clear_freq(&self, schema: &str) -> anyhow::Result<usize> {
        self.runtime.clear_freq(schema)
    }
    pub fn import_freq_rows(&self, schema: &str, rows: &[crate::wdict::FreqIo]) -> anyhow::Result<usize> {
        let rows: Vec<(String, String, wind_userdata::runtime::FreqRec)> = rows
            .iter()
            .map(|r| {
                let (code, _) = crate::wdict::split_spaced_code(&r.code);
                (code, r.text.clone(), wind_userdata::runtime::FreqRec { count: r.count, last_used: r.last_used })
            })
            .collect();
        self.runtime.import_freq_rows(schema, &rows)
    }
```

`import_freq_jsonl`：保留解析部分（得到 `rows: Vec<(String, String, u32, i64)>` 与 `skipped`），把 redb 事务那段换成：

```rust
        let rows: Vec<(String, String, wind_userdata::runtime::FreqRec)> = rows
            .into_iter()
            .map(|(c, t, count, last_used)| (c, t, wind_userdata::runtime::FreqRec { count, last_used }))
            .collect();
        self.runtime.import_freq_rows(schema, &rows)?;
        Ok((imported, skipped))
```

`export_freq_jsonl` 不改（建在 `list_freq_paged` 上）。

`assoc_history.rs`：

```rust
    pub fn record_assoc_pick(&self, schema: &str, context: &str, word: &str) -> anyhow::Result<()> {
        if schema.is_empty() || context.is_empty() || word.is_empty() {
            return Ok(());
        }
        self.runtime.record_assoc_pick(schema, context, word)?;
        self.after_write();
        Ok(())
    }
    pub fn assoc_history(&self, schema: &str, context: &str, limit: usize) -> anyhow::Result<Vec<(String, FreqRecord)>> {
        Ok(self
            .runtime
            .assoc_history(schema, context, limit)
            .into_iter()
            .map(|(w, r)| (w, crate::freq::freq_of(r)))
            .collect())
    }
    pub fn clear_assoc_history(&self, schema: &str) -> anyhow::Result<usize> {
        self.runtime.clear_assoc(schema)
    }
```

`export_assoc_history_jsonl` 的 redb 读取换成 `self.runtime.for_each_assoc(schema, &mut |ctx, word, r| { rows.push((ctx.to_string(), word.to_string(), crate::freq::freq_of(r))); true });`，其后拼 JSON 的代码不变。`import_assoc_history_jsonl` 的解析循环里把 `rows.push((enc_key(schema, ctx, word), count, last_used))` 改成 `rows.push((ctx.to_string(), word.to_string(), wind_userdata::runtime::FreqRec { count, last_used }))`，redb 事务那段换成 `self.runtime.import_assoc_rows(schema, &rows)?;`。

`store_layer.rs` 的 `StoreDraftLayer` 照 Task 4 的 `StoreTempLayer` 写法包 `wind_userdata::layer::UserDataDraftLayer::new(store.runtime().data().clone(), &schema_id, ttl_secs)`，转发 `name` / `layer_type` / `search` / `search_prefix`（其余走 trait 默认，与原实现一致）。原结构体文档里「只响应精确查询」那段说明保留在新结构体上。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store --no-fail-fast 2>&1 | grep -E "FAILED|panicked|test result" | head -30`
Expected: 新增一条通过；`draft_words.rs` / `freq.rs` / `assoc_history.rs` 原有测试通过。断言 redb 内部的旧测试（如直接写 `DRAFT_WORDS` 造过期数据的 `expired_…_at_query_time…`、`startup_purge_sweeps_every_schema…`）改为用引擎造数据：`s.runtime().data().apply_batch(&[op])`，`op.ts_ms` 设成过去的时刻（同 P1a `tests/runtime_draft.rs` 的 `draft_at`）——断言不改。每改一条记一行 `Task 5: Ruling`。

- [ ] **Step 5: 跨 crate 回归**

Run: 同 Task 4 Step 5。
Expected: 与 Task 0 基线一致。

- [ ] **Step 6: 提交**

```bash
cd wind_input && cargo fmt -p wind-store && cd .. && git commit -F - --only -- wind_input/crates/wind-store <<'EOF'
feat(store): 草稿、词频、联想历史委托运行时引擎（门面签名不变）
EOF
```

---

### Task 6: redb 旧表 → 引擎的一次性迁移

**Files:**
- Create: `wind_input/crates/wind-store/src/migrate_p1.rs`
- Modify: `wind_input/crates/wind-store/src/lib.rs`（`mod migrate_p1;`）、`store.rs`（`open_inner` 末尾调用迁移）
- Test: `wind-store/src/migrate_p1.rs` tests

**Interfaces:**
- Consumes: 旧表定义 `TEMP_WORDS` / `DRAFT_WORDS` / `FREQ` / `ASSOC_HIST`（`pub(crate)`）、`user_words::{split_key, dec_val}`、`draft_words::dec_draft`、`freq::dec_freq`；`UserData::fold_with`、`SnapshotEntry`、`record::Target`。
- Produces:
  - `pub const MIGRATED_P1: &str = "migrated-p1";`
  - `Store::migrate_p1_if_needed(&self, state_dir: &Path) -> anyhow::Result<bool>`（`pub(crate)`；做了返回 true）
  - 测试辅助 `#[cfg(test)] pub(crate) fn legacy_put_*`：直接往旧表写行（模拟老版本数据）

- [ ] **Step 1: 写失败测试（`migrate_p1.rs` 先只放 tests 模块与空的 `MIGRATED_P1` 常量）**

```rust
#[cfg(test)]
mod tests {
    use crate::Store;

    fn db(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("wind_mig_{name}-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(crate::store::derived_state_dir(&p));
        p
    }

    /// 造一个「老版本写的库」：先开一次、往旧表直接写行，再删掉派生状态目录（老版本没有它）。
    fn legacy_db(name: &str) -> std::path::PathBuf {
        let p = db(name);
        let s = Store::open(&p).unwrap();
        s.legacy_put_temp("py", "nihao", "你好", 800, 3, 1_700_000_000, 0b101).unwrap();
        s.legacy_put_temp("py", "xian", "西安", 500, 1, 1_700_000_001, 0b101).unwrap();
        s.legacy_put_draft("wb", "ggtt", "盖伦", crate::draft_words::now_for_test()).unwrap();
        s.legacy_put_freq("py", "nihao", "你好", 7, 1_700_000_100).unwrap();
        s.legacy_put_assoc("py", "天", "气", 2, 1_700_000_200).unwrap();
        drop(s);
        std::fs::remove_dir_all(crate::store::derived_state_dir(&p)).unwrap();
        p
    }

    /// Review Focus 1：四张表全部迁入、简拼可召回；第二次打开不再迁移。
    #[test]
    fn legacy_tables_are_migrated_once() {
        let p = legacy_db("once");
        let s = std::sync::Arc::new(Store::open(&p).unwrap());
        let w = s.get_temp_words("py", "nihao").unwrap();
        assert_eq!((w[0].weight, w[0].count, w[0].created_at, w[0].boundary), (800, 3, 1_700_000_000, 0b101));
        use wind_dict::DictLayer;
        let l = crate::StoreTempLayer::new(s.clone(), "py");
        assert!(l.search_abbrev("xa", 0).iter().any(|c| c.text == "西安"), "简拼可召回");
        assert_eq!(s.search_drafts("wb", "ggtt", 86_400).unwrap(), vec!["盖伦"]);
        assert_eq!(s.get_freq("py", "nihao", "你好").unwrap().map(|r| (r.count, r.last_used)), Some((7, 1_700_000_100)));
        assert_eq!(s.assoc_history("py", "天", 0).unwrap()[0].1.count, 2);
        let marker = crate::store::derived_state_dir(&p).join(super::MIGRATED_P1);
        assert!(marker.is_file());
        // 迁移后改一笔，再开：不得被旧表覆盖回去
        s.record_freq("py", "nihao", "你好").unwrap();
        drop(l);
        drop(s);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get_freq("py", "nihao", "你好").unwrap().unwrap().count, 8);
    }

    /// Review Focus 2：标记没写成（崩溃）⇒ 重跑，结果与一次成功相同，不翻倍。
    #[test]
    fn rerun_after_crash_does_not_double_count() {
        let p = legacy_db("rerun");
        let s = Store::open(&p).unwrap();
        drop(s);
        std::fs::remove_file(crate::store::derived_state_dir(&p).join(super::MIGRATED_P1)).unwrap();
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get_temp_word("py", "nihao", "你好").unwrap(), Some(3));
        assert_eq!(s.get_freq("py", "nihao", "你好").unwrap().unwrap().count, 7);
        assert_eq!(s.count_temp_words("py").unwrap(), 2);
    }

    #[test]
    fn empty_legacy_db_writes_marker_only() {
        let p = db("empty");
        let s = Store::open(&p).unwrap();
        assert!(crate::store::derived_state_dir(&p).join(super::MIGRATED_P1).is_file());
        assert_eq!(s.count_temp_words("py").unwrap(), 0);
    }
}
```

（`now_for_test()`：在 `draft_words.rs` 加 `#[cfg(test)] pub(crate) fn now_for_test() -> i64 { now_secs() }`，若 `now_secs` 在别处则就近加。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store migrate_p1 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`legacy_put_*` / `MIGRATED_P1` 未定义。

- [ ] **Step 3: 实现 `migrate_p1.rs`**

```rust
//! P1 迁移（设计 §7.2）：redb 的 temp_words / draft_words / freq / assoc_history 四张表一次性
//! 迁入运行时引擎。按 (目标, 方案) **整体替换**（`fold_with` 直接返回旧表的全部行），重跑结果
//! 相同；全部完成后写 `state/migrated-p1`。旧表只读保留到 P4，不删、不再写。

use crate::Store;
use crate::store::{ASSOC_HIST, DRAFT_WORDS, FREQ, TEMP_WORDS};
use redb::ReadableTable;
use std::collections::BTreeMap;
use std::path::Path;
use wind_userdata::record::Target;
use wind_userdata::snapshot::SnapshotEntry;

pub const MIGRATED_P1: &str = "migrated-p1";

type Rows = BTreeMap<String, Vec<SnapshotEntry>>;

fn entry(code: &str, text: &str, weight: i32, count: u32, created_at: i64, last_used: i64, boundary: u64) -> SnapshotEntry {
    SnapshotEntry {
        code: code.to_string(),
        text: text.to_string(),
        weight,
        boundary,
        count,
        created_at,
        last_used,
        flags: 0,
    }
}

impl Store {
    /// 读旧表 `table` 全部行，按方案分组；`decode(code, text, value) -> Option<SnapshotEntry>`。
    fn legacy_rows(
        &self,
        table: redb::TableDefinition<&str, &[u8]>,
        decode: &dyn Fn(&str, &str, &[u8]) -> Option<SnapshotEntry>,
    ) -> anyhow::Result<Rows> {
        self.with_db(|db| {
            let txn = db.begin_read()?;
            let t = txn.open_table(table)?;
            let mut out: Rows = BTreeMap::new();
            for item in t.range::<&str>(..)? {
                let (k, v) = item?;
                let Some((schema, code, text)) = crate::user_words::split_key(k.value()) else {
                    continue;
                };
                if code.is_empty() || text.is_empty() {
                    continue;
                }
                if let Some(e) = decode(code, text, v.value()) {
                    out.entry(schema.to_string()).or_default().push(e);
                }
            }
            Ok(out)
        })
    }

    pub(crate) fn migrate_p1_if_needed(&self, state_dir: &Path) -> anyhow::Result<bool> {
        let marker = state_dir.join(MIGRATED_P1);
        if marker.is_file() {
            return Ok(false);
        }
        let jobs: [(Target, Rows); 4] = [
            (
                Target::Temp,
                self.legacy_rows(TEMP_WORDS, &|c, t, v| {
                    crate::user_words::dec_val(v).map(|(w, n, ca, b)| entry(c, t, w, n, ca, ca, b))
                })?,
            ),
            (
                Target::Draft,
                self.legacy_rows(DRAFT_WORDS, &|c, t, v| {
                    crate::draft_words::dec_draft(v).map(|ca| entry(c, t, 0, 0, ca, ca, 0))
                })?,
            ),
            (
                Target::Freq,
                self.legacy_rows(FREQ, &|c, t, v| {
                    crate::freq::dec_freq(v).map(|r| entry(c, t, 0, r.count, r.last_used, r.last_used, 0))
                })?,
            ),
            (
                Target::Assoc,
                self.legacy_rows(ASSOC_HIST, &|c, t, v| {
                    crate::freq::dec_freq(v).map(|r| entry(c, t, 0, r.count, r.last_used, r.last_used, 0))
                })?,
            ),
        ];
        let ud = self.runtime().data().clone();
        let mut total = 0usize;
        for (target, by_schema) in jobs {
            for (schema, rows) in by_schema {
                total += rows.len();
                let rows = std::cell::RefCell::new(Some(rows));
                ud.fold_with(target, &schema, &mut |_| rows.borrow_mut().take().unwrap_or_default())?;
            }
        }
        wind_userdata::state::atomic_write(&marker, b"1\n")?;
        tracing::info!("P1 迁移完成：{total} 条运行时数据从 redb 迁入引擎");
        Ok(true)
    }
}
```

（`dec_val` / `dec_draft` / `dec_freq` 若为私有，改为 `pub(crate)`；`split_key` 同理。草稿的有效期基准在引擎里是 `last_used`，所以 `created_at` 同时写进 `last_used`。）

测试辅助（同文件，`#[cfg(test)] impl Store`），用旧编码函数直接写表：

```rust
#[cfg(test)]
impl Store {
    pub(crate) fn legacy_put_temp(&self, schema: &str, code: &str, text: &str, w: i32, count: u32, ca: i64, b: u64) -> anyhow::Result<()> {
        self.legacy_put(TEMP_WORDS, schema, code, text, &crate::user_words::enc_val(w, count, ca, b))
    }
    pub(crate) fn legacy_put_draft(&self, schema: &str, code: &str, text: &str, ca: i64) -> anyhow::Result<()> {
        self.legacy_put(DRAFT_WORDS, schema, code, text, &ca.to_le_bytes())
    }
    pub(crate) fn legacy_put_freq(&self, schema: &str, code: &str, text: &str, count: u32, last_used: i64) -> anyhow::Result<()> {
        self.legacy_put(FREQ, schema, code, text, &crate::freq::enc_freq(count, last_used))
    }
    pub(crate) fn legacy_put_assoc(&self, schema: &str, ctx: &str, word: &str, count: u32, last_used: i64) -> anyhow::Result<()> {
        self.legacy_put(ASSOC_HIST, schema, ctx, word, &crate::freq::enc_freq(count, last_used))
    }
    fn legacy_put(&self, table: redb::TableDefinition<&str, &[u8]>, schema: &str, code: &str, text: &str, val: &[u8]) -> anyhow::Result<()> {
        let key = crate::user_words::enc_key(schema, code, text);
        self.with_db(|db| {
            let txn = db.begin_write()?;
            txn.open_table(table)?.insert(key.as_str(), val)?;
            txn.commit()?;
            Ok(())
        })
    }
}
```

`store.rs` 的 `open_inner` 在 `run_migrations()` / `backfill_abbrev_indexes()` 之后：

```rust
        if let Err(e) = store.migrate_p1_if_needed(&loc.state_dir) {
            // 迁移失败不阻断启动：标记没写，下次启动整体重做；本次运行时数据从空开始。
            tracing::warn!("P1 迁移失败，下次启动重试：{e}");
        }
```

（`loc` 在构造 `Self` 时被移动的话，先 `let state_dir = loc.state_dir.clone();`。）

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-store --no-fail-fast 2>&1 | grep -E "FAILED|panicked|test result"`
Expected: 迁移 3 条通过，其余不变。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-store/src/migrate_p1.rs
cd wind_input && cargo fmt -p wind-store && cd .. && git commit -F - --only -- wind_input/crates/wind-store <<'EOF'
feat(store): redb 运行时四表一次性迁入引擎（按目标整体替换，标记 migrated-p1）
EOF
```

---

### Task 7: 生产接线与 `format_version` → 2

**Files:**
- Modify: `wind_input/crates/wind-config/src/user_meta.rs`（`KNOWN_FORMAT_VERSION = 2`；`raise_format_version`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（`open_user_store_checked` / `open_user_store` 带状态位置；成功后升版本）
- Modify: `wind_input/crates/wind-coordinator/tests/storage_readonly.rs`
- Test: `user_meta.rs` tests；`storage_readonly.rs`

**Interfaces:**
- Consumes: Task 3 的 `Store::open_with_state` / `StateLoc`。
- Produces:
  - `pub fn raise_format_version(user_dir: &Path, to: u32) -> std::io::Result<()>`：现值 < `to` 才改，tmp + rename；`dir_id` 保留。
  - `open_user_store_checked(dir: Option<&Path>, state: StateChoice)`，`enum StateChoice { Production, Derived }`：`new()` 传 `Production`（`Config::state_dir()` + meta 的 `dir_id`），`new_headless_with_ui_at` 传 `Derived`（测试路径：派生目录，避免测试共用本机状态目录与锁）。

- [ ] **Step 1: 写失败测试**

`user_meta.rs` tests：

```rust
    #[test]
    fn raise_format_version_only_moves_up_and_keeps_dir_id() {
        let d = dir("raise");
        std::fs::write(d.join("meta.toml"), "format_version = 1\ndir_id = \"abc\"\n").unwrap();
        raise_format_version(&d, 2).unwrap();
        let MetaCheck::Ok(m) = load_or_init(&d) else { panic!("应可读") };
        assert_eq!((m.format_version, m.dir_id.as_str()), (2, "abc"));
        raise_format_version(&d, 1).unwrap();
        let MetaCheck::Ok(m) = load_or_init(&d) else { panic!() };
        assert_eq!(m.format_version, 2, "不往回改");
    }
```

`tests/storage_readonly.rs` 追加（沿用该文件已有的便携布局辅助；下面用 `setup()` 指代它，按实际名字调用）：

```rust
/// Review Focus 3：meta 为 1 的目录被新版打开 ⇒ 迁移后升到 2；旧版本（认识 1）随后会进只读。
#[test]
fn opening_migrates_and_raises_format_version() {
    let (user_dir, _guard) = setup("raise");
    std::fs::write(user_dir.join("meta.toml"), "format_version = 1\ndir_id = \"d1\"\n").unwrap();
    let (store, reason) = wind_coordinator::Coordinator::open_user_store_checked_for_test(Some(&user_dir));
    assert!(store.is_some() && reason.is_none());
    let text = std::fs::read_to_string(user_dir.join("meta.toml")).unwrap();
    assert!(text.contains("format_version = 2"), "{text}");
}
```

（若 `open_user_store_checked` 在集成测试里不可见：在 `construct.rs` 加 `#[doc(hidden)] pub fn open_user_store_checked_for_test(dir: Option<&Path>) -> (Option<Arc<Store>>, Option<String>) { Self::open_user_store_checked(dir, StateChoice::Derived) }`。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config user_meta 2>&1 | grep -E "error\[|FAILED|test result"; TMPDIR=<独立临时目录> cargo test -p wind-coordinator --test storage_readonly 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`raise_format_version` / `open_user_store_checked_for_test` 未定义。

- [ ] **Step 3: 实现**

`user_meta.rs`：

```rust
/// 本版本认识的格式版本。1 = 纯 redb；2 = P1 运行时数据（临时词 / 草稿 / 词频 / 联想）在引擎。
pub const KNOWN_FORMAT_VERSION: u32 = 2;

/// 迁移成功后把 `format_version` 升到 `to`（现值 ≥ `to` 不动）。旧版本读到更高的版本即只读（§7.1）。
pub fn raise_format_version(user_dir: &Path, to: u32) -> std::io::Result<()> {
    let path = user_dir.join("meta.toml");
    let text = std::fs::read_to_string(&path)?;
    let mut m: UserMeta = toml::from_str(&text).map_err(std::io::Error::other)?;
    if m.format_version >= to {
        return Ok(());
    }
    m.format_version = to;
    let s = toml::to_string(&m).map_err(std::io::Error::other)?;
    let tmp = user_dir.join("meta.toml.tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, &path)
}
```

现有测试里断言「新建 meta 的版本 = 1」的改成 `KNOWN_FORMAT_VERSION`（若有，记 Ruling）。

`construct.rs`：

```rust
/// 运行时引擎的状态目录取哪里。
#[derive(Clone, Copy)]
pub(crate) enum StateChoice {
    /// 生产：`Config::state_dir()`（`local_dir()/state`）+ meta 的 dir_id。
    Production,
    /// 测试 / 无头：redb 旁的派生目录（不碰本机状态目录与它的锁）。
    Derived,
}
```

`open_user_store_checked(dir, choice)`：`MetaCheck::Ok(meta)` 分支改为 `(Self::open_user_store(d, choice, &meta.dir_id), None)`。`open_user_store(dir, choice, dir_id)`：

```rust
        let p = dir.join("userdata.redb");
        let opened = match choice {
            StateChoice::Production => match Config::state_dir() {
                Some(state_dir) => Store::open_with_state(
                    &p,
                    wind_store::store::StateLoc {
                        state_dir,
                        dir_id: dir_id.to_string(),
                    },
                ),
                None => {
                    warn!("取不到本机状态目录，运行时数据放在 redb 旁的派生目录");
                    Store::open(&p)
                }
            },
            StateChoice::Derived => Store::open(&p),
        };
        match opened {
            Ok(s) => {
                // 迁移已在 open 里完成（标记 migrated-p1）：此后旧版本必须只读（§7.1）
                if let Err(e) = wind_config::user_meta::raise_format_version(
                    dir,
                    wind_config::user_meta::KNOWN_FORMAT_VERSION,
                ) {
                    warn!("format_version 升级失败（旧版本仍可能写 redb）：{e}");
                }
                // …原有 info! / 空闲回收线程等逻辑不变，s 照旧包 Arc
            }
            Err(e) => { /* 原有 warn + None 不变 */ }
        }
```

`new()` 调用 `open_user_store_checked(user_dir.as_deref(), StateChoice::Production)`；`new_headless_with_ui_at` 传 `StateChoice::Derived`。其他调用 `open_user_store` 的地方按编译报错补参数（测试路径一律 `Derived`）。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-config -p wind-coordinator --no-fail-fast 2>&1 | grep -E "^test result" | awk '{p+=$4; f+=$6} END{print p, "passed", f, "failed"}'`
Expected: 新增两条通过；失败数不超过 Task 0 基线。`storage_readonly` 原有「meta 99 ⇒ 只读、不建 redb、meta 不改」仍通过。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-config -p wind-coordinator && cd .. && git commit -F - --only -- wind_input/crates/wind-config/src/user_meta.rs wind_input/crates/wind-coordinator <<'EOF'
feat(userdata): 生产接线到本机状态目录；迁移后 format_version 升到 2

旧版本（认识 1）读到 2 即进只读，不会在新格式旁继续写 redb（§7.1）。
EOF
```

---

### Task 8: 全量回归、真实库等价、性能核对

**Files:**
- Create: `wind_input/crates/wind-store/examples/p1_migrate_check.rs`
- Modify: 无（只有回归中发现的缺陷修复，按 TDD 各自一笔）

- [ ] **Step 1: 全量回归**

Run: Task 0 的同一循环，输出写 `<独立临时目录>/p1b-after-$c.log`，与 `p1b-base-$c.log` 比对：

```bash
cd wind_input && for c in wind-store wind-dict wind-engine wind-coordinator wind-webdata wind-transfer wind-userdata wind-config wind-wdict; do
  TMPDIR=<独立临时目录> cargo test -p $c --no-fail-fast 2>&1 | grep -E "^test result|FAILED|panicked" > <独立临时目录>/p1b-after-$c.log
  echo "$c: $(grep -c 'FAILED' <独立临时目录>/p1b-after-$c.log) failed, $(awk '/^test result/{s+=$4} END{print s}' <独立临时目录>/p1b-after-$c.log) passed"
done
diff <(cat <独立临时目录>/p1b-base-*.log | grep FAILED | sort) <(cat <独立临时目录>/p1b-after-*.log | grep FAILED | sort)
```

Expected: `diff` 不出现新增的 `FAILED` 行（基线既有红可以消失，不得新增）；通过总数 ≥ 基线（迁移 / 搬家的测试计入新 crate）。新增红逐条按 systematic-debugging 处理。

- [ ] **Step 2: 真实库等价检查工具**

`examples/p1_migrate_check.rs`：

```rust
//! P1 迁移等价检查（设计 §11）：把一份 userdata.redb **副本**用 Store::open 打开（触发迁移到
//! 派生状态目录），逐方案比对旧表行与引擎读回的结果。
//! 用法：cargo run --release -p wind-store --example p1_migrate_check -- <userdata.redb 副本>
fn main() -> anyhow::Result<()> {
    let p = std::env::args().nth(1).expect("参数：userdata.redb 副本路径");
    let _ = std::fs::remove_dir_all(wind_store::store::derived_state_dir(std::path::Path::new(&p)));
    let t = std::time::Instant::now();
    let s = wind_store::Store::open(&p)?;
    println!("打开 + 迁移: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    for schema in s.list_data_schemas()? {
        let (temp, freq, drafts) = (
            s.count_temp_words(&schema)?,
            s.list_freq_paged(&schema, "", 0, 0)?.1,
            s.count_drafts(&schema)?,
        );
        let legacy = s.legacy_counts(&schema)?;
        println!("{schema}: temp {temp}/{} freq {freq}/{} drafts {drafts}/{}", legacy.0, legacy.1, legacy.2);
        assert_eq!((temp, freq, drafts), legacy, "{schema} 条数不一致");
    }
    println!("OK");
    Ok(())
}
```

为它在 `migrate_p1.rs` 加 `#[doc(hidden)] pub fn legacy_counts(&self, schema: &str) -> anyhow::Result<(usize, usize, usize)>`（临时词 / 词频 / 草稿三张旧表里该方案的行数，复用 `legacy_rows`）。

Run（有真实库副本时）：`cp <真实 userdata.redb> <独立临时目录>/ud-copy.redb && cd wind_input && cargo run --release -p wind-store --example p1_migrate_check -- <独立临时目录>/ud-copy.redb`
Expected: 每个方案三项条数一致，`OK`；打印的迁移耗时记进账本。本机没有真实库时，用 `build_dev/data` 下能找到的 redb，或账本记 `Task 8: Ruling: 无真实库，等价检查留给靶机` 并在最终汇报里列出。

- [ ] **Step 3: 性能核对**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test --release -p wind-userdata --test perf_probe -- --ignored --nocapture 2>&1 | grep -E "ms|test result"`
Expected: 数量级断言通过（P1a 已设）。另跑 `wind-engine` 的简拼相关测试（`pinyin_store_abbrev_index`、`pinyin_abbrev_recall_window`）确认通过。

- [ ] **Step 4: 提交**

```bash
git add wind_input/crates/wind-store/examples/p1_migrate_check.rs
cd wind_input && cargo fmt -p wind-store && cd .. && git commit -F - --only -- wind_input/crates/wind-store <<'EOF'
test(store): P1 迁移等价检查工具（真实库副本逐方案比对条数）
EOF
```

---

### Task 9: 文档

**Files:**
- Create: `wind_input/crates/wind-wdict/AGENTS.md`
- Modify: `wind_input/crates/wind-store/AGENTS.md`（四组 API 委托引擎、`StateLoc` / 派生目录、迁移、旧表只读、`store_layer` 搬入）、`wind_input/crates/wind-dict/AGENTS.md`（不再依赖 `wind-store`，Store 层已搬走）、`wind_input/crates/wind-userdata/AGENTS.md`（已接线；`learn_temp_detailed`；层 `is_prefix` 对齐）、根 `AGENTS.md`（crate 索引加 `wind-wdict`）
- Modify: `docs/design/user-data-storage-redesign.md`（§10 P1b 行标已实现与差异：门面委托；§6 表格注明「P1b 起旧表只读」）、`docs/design/auto-phrase-draft-layer.md` §4（存储改为运行时引擎，一段说明 + 指向本计划）

- [ ] **Step 1: 改文档**（内容按上面清单逐条；每份 AGENTS.md 的改动限于事实更新，不重写结构）

- [ ] **Step 2: 最终回归**

Run: Task 8 Step 1 的循环。
Expected: 同 Task 8。

- [ ] **Step 3: 提交**

```bash
git add wind_input/crates/wind-wdict/AGENTS.md
git commit -F - --only -- AGENTS.md wind_input/crates/wind-wdict/AGENTS.md wind_input/crates/wind-store/AGENTS.md wind_input/crates/wind-dict/AGENTS.md wind_input/crates/wind-userdata/AGENTS.md docs/design/user-data-storage-redesign.md docs/design/auto-phrase-draft-layer.md <<'EOF'
docs(userdata): P1b 文档——Store 门面委托引擎、迁移与 format_version 2
EOF
```
