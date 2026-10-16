# 用户数据存储 P2b 实施计划：用户词接线

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用户词的真源从 redb `USER_WORDS` 换成 `user/<schema>/user.dict.yaml`：`wind-store` 的用户词方法签名不变、方法体委托 P2a 的文本层；旧表一次性迁移成文本，`format_version` → 3；手改文件在启动 / 切方案 / 配置重载 / `dict.reload` 时生效，`dict.flush` 显式折回。

**Architecture:** 沿用 P1b 的「`Store` 做门面」：调用方零改动，现有测试即对拍。文本放在用户目录 `<userdata>/user/<schema>/user.dict.yaml`（`StateLoc.text_dir` 给出；测试 / 工具的派生模式放在 `<redb>.user/`，与派生状态目录一起随新库清空）。`Store` 打开时把已有文本与引擎里有用户词的方案都绑定上，之后首次访问某方案时按需绑定。空闲折回由一个只持 `Weak` 的低频后台节拍驱动。

**Tech Stack:** Rust（edition 2024），`wind-userdata`（P2a 文本层）、`wind-store`、`wind-coordinator`、`wind-webdata`、`wind-config`。

**Spec:** `docs/design/user-data-storage-redesign.md`（§2.1、§2.2、§3.5、§3.8、§5、§6、§7.2；P2a 已实现部分见 §10 的 P2a 行与 `user-data-storage-plan-p2a.md`）

## Global Constraints

- 文本路径 `<userdata>/user/<schema>/user.dict.yaml`，`<schema>` 是数据方案 id（拼音族折叠为 `pinyin`）（§2.1）。
- 写回与迁移导出一律带空格码（`join_code_by_boundary`）；边界未知原样输出无空格码（§2.2）。
- 迁移：没有完成标记就整体重做，不按「文件已存在」跳过；迁移成功后旧表只读保留，`format_version` 升一级（§7.2）。
- 同一类别不并存两套真源：P2b 之后用户词只在文本 + 引擎里，`USER_WORDS` / `USER_ABBREV` 不再写（§7.2、§6）。
- `dict.reload(paths?: [string])`：路径限定在 `<userdata>/user/` 内，越界拒绝；空参数全量扫描。`dict.flush`：显式折回全部目标（§5）。
- 提交：`git commit -F - --only -- <路径…>`，不用 `git add -A`；格式化只 `cargo fmt -p <crate>`。
- 测试命令统一带 `CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录>`，在 `wind_input/` 下跑；全量用 `--no-fail-fast`。
- INFO 日志不带词条内容。

## 计划内裁定（执行前已定）

1. **用户分库（用户自放的只读 `*.dict.yaml`）不在 P2b**，另立 P2c：它是新功能（登记 / 白名单、只读段预算、删除转 Shadow），与「把现有用户词搬家」无依赖；P2b 只做搬家与接线。代价：设计 §10 的 P2b 行要改写，P2c 补一行。
2. **`on_word_selected` 删除**：生产无调用方（只有测试），而它的语义依赖 `count`——P2a 已裁定用户词 count 不进文本。代价：将来要「选词微调用户词权重」得另设计。
3. **`UserWordRecord.count` / `created_at` 恒 0**（P2a 裁定的延续）；`order` 在门面上映射为「未折回的新词 = `u32::MAX`」，与用户词层「新词排在已有词之后」同向，导出按 order 排序时新词在末尾。
4. **`lowercase` 恒 false**：redb 版用户词码不做大小写折叠（调用方已规范化），保持同语义；手改文件里的大写码按原样匹配。代价：英文方案用户手写大写码查不到，需要时再按方案下发。
5. **redb 机制测试改用短语表做负载**：`drop_page_cache.rs`、`idle_cache_reclaim.rs`、`scan_reclaim.rs`、`redb_cache_high_water.rs` 测的是 redb 页缓存 / 回收，不是用户词语义；用户词搬走后它们改用仍在 redb 的 `PHRASES`。`perf_abbrev_index.rs`、`perf_cache_size.rs` 只量退役的用户词 redb 路径，删除（引擎侧性能护栏在 `wind-userdata/tests/perf_probe.rs`）。
6. **`meta.toml` 初次生成时**：用户目录已有 `userdata.redb` ⇒ 写 `format_version = 1`（等迁移完成再升），否则写 `KNOWN_FORMAT_VERSION`。修掉「从无 meta 的旧版升级时，迁移还没跑就先写成最新版本号」的口子。

## Review Focus

1. **真实大库迁移**：19 万词、含 `#` 开头 / 含 tab 与换行文本 / 边界未知 / 同码多词的旧库，迁移后同一批码的候选（文本、权重、同码等权的先后、边界）与迁移前逐条相同；迁移耗时在秒级以内。→ Task 4 的 `migration_preserves_words_order_and_boundary` + `examples/p2_migrate_check.rs`。
2. **手改生效**：输入法运行中用编辑器改 / 删 / 加 `user.dict.yaml` 的行，`dict.reload` 或切方案后候选立即反映，联想 / 按词查码索引随之更新（`words_generation_of` 推进）；没被手改的未折回操作照常叠加。→ Task 5 的 `reload_picks_up_hand_edit_and_bumps_generation`。
3. **未折回期间的设置页一致性**：刚加的词还没写回文件时，分页列表的 total 与 `count_user_words` 一致、导出包含它且排在末尾、导入预览的「新增 / 更新 / 不变」与实际导入一致。→ Task 2 的 `unfolded_words_are_listed_counted_and_exported_last`、`preview_matches_import_counts`。
4. **迁移中途崩溃重跑**：标记写入前崩溃，重跑不重复、不丢、不把重跑之间新加的词盖掉。→ Task 4 的 `rerun_after_crash_merges_without_loss`。
5. **`dict.reload` 路径越界**：`../`、绝对路径指向用户目录外、指向 `user/` 下非 `user.dict.yaml` 的文件，一律拒绝且不读文件。→ Task 5 的 `reload_rejects_paths_outside_user_dir`。

---

### Task 1：wind-userdata 补强（导入分类、大批追加线性化、后台节拍）

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/text/patch.rs`（本轮新增行的按键索引）
- Modify: `wind_input/crates/wind-userdata/src/runtime/user.rs`（`UserImportCounts`、`user_row_effect`、`import_user_rows` 返回分类）
- Modify: `wind_input/crates/wind-userdata/src/runtime/mod.rs`（导出新类型）
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`spawn_ticker`）
- Test: `wind_input/crates/wind-userdata/tests/runtime_user.rs`、`wind_input/crates/wind-userdata/src/text/patch.rs`（tests 模块）、`wind_input/crates/wind-userdata/tests/text_target.rs`

**Interfaces:**
- Produces:
  - `pub struct UserImportCounts { pub added: usize, pub updated: usize, pub unchanged: usize }`（`#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]`），从 `wind_userdata::runtime` 导出。
  - `pub enum RowEffect { Added, Updated, Unchanged }`，从 `wind_userdata::runtime` 导出。
  - `Runtime::user_row_effect(&self, schema: &str, row: &UserWord) -> RowEffect`
  - `Runtime::import_user_rows(&self, schema: &str, rows: &[UserWord]) -> anyhow::Result<UserImportCounts>`（原返回 `usize`）
  - `UserData::spawn_ticker(self: &Arc<Self>, every: std::time::Duration) -> Option<std::thread::JoinHandle<()>>`

- [ ] **Step 1: 写失败测试**

`tests/runtime_user.rs` 追加：

```rust
/// 导入分类：新键 added；权重更大或补边界 updated；其余 unchanged。预览判据与落盘一致。
#[test]
fn import_classifies_rows_like_wind_store() {
    use wind_userdata::runtime::{RowEffect, UserImportCounts};
    let env = Env::new("classify");
    let r = env.open(&["py"]);
    r.add_user_word("py", "nihao", "你好", 100, 0).unwrap();
    r.add_user_word("py", "ta", "他", 50, 0b1).unwrap();
    let rows = [
        row("nihao", "你好", 10),               // 权重更小、边界 0 → unchanged
        UserWord { boundary: 0b101, ..row("nihao", "你好", 10) }, // 补边界 → updated
        row("ta", "他", 80),                    // 权重更大 → updated
        row("xin", "新", 1),                    // 新键 → added
    ];
    assert_eq!(r.user_row_effect("py", &rows[0]), RowEffect::Unchanged);
    assert_eq!(r.user_row_effect("py", &rows[1]), RowEffect::Updated);
    assert_eq!(r.user_row_effect("py", &rows[3]), RowEffect::Added);
    let c = r.import_user_rows("py", &rows[1..]).unwrap();
    assert_eq!(
        c,
        UserImportCounts { added: 1, updated: 2, unchanged: 0 }
    );
    assert_eq!(r.user_word("py", "nihao", "你好").unwrap().boundary, 0b101);
}
```

`src/text/patch.rs` 的 tests 模块追加（守「本轮新增行」的查找是按键索引而不是线性扫描）：

```rust
/// 大批追加线性：3 万条新词一轮补丁（导入 / 迁移走这条）不能是平方级。
#[test]
fn appending_many_rows_is_linear() {
    let ops: Vec<PatchOp> = (0..30_000)
        .map(|i| up(&format!("c{i:05}"), &format!("词{i:05}"), 0, 1))
        .collect();
    let t = std::time::Instant::now();
    let out = run("", &ops);
    assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(out.lines().filter(|l| l.contains('\t')).count(), 30_000);
}
```

`tests/text_target.rs` 追加：

```rust
/// 后台节拍：没有新写入也会把空闲够久的文本目标折回；引擎释放后线程自行退出。
#[test]
fn ticker_folds_idle_text_without_further_writes() {
    let (_d, ud, file) = setup("ticker");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    let h = ud.spawn_ticker(std::time::Duration::from_millis(20)).unwrap();
    ud.set_text_idle_for_test(std::time::Duration::ZERO);
    let t0 = std::time::Instant::now();
    while !file.is_file() && t0.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(file.is_file(), "节拍应把空闲的文本目标折回");
    drop(ud);
    let t0 = std::time::Instant::now();
    while !h.is_finished() && t0.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.is_finished(), "引擎释放后节拍线程退出");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "^test .*FAILED|error\[" | head`
Expected: 编译失败（`RowEffect` / `UserImportCounts` / `spawn_ticker` / `set_text_idle_for_test` 不存在）。先把这三处测试临时注释掉单独跑 `appending_many_rows_is_linear`，Expected: FAIL（超过 3 秒）——确认平方级存在后再取消注释。

- [ ] **Step 3: 实现**

`patch.rs`：在 `let mut added: Vec<Added> = Vec::new();` 旁加 `let mut added_at: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();`。
- `first_added` 改为 `added_at.get(&key).copied().filter(|&i| added[i].alive)`。
- 追加时 `added_at.insert(key.clone(), added.len()); added.push(Added { key, .. })`。
- `Delete` 时 `if let Some(&i) = added_at.get(&key) { added[i].alive = false; }`，替换原来的 `iter_mut().filter`。

同键在同一轮内只会有一条活的 `Added`（Upsert 命中已有活行就合并），所以一个键对应一个下标就够。

`runtime/user.rs`：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowEffect {
    Added,
    Updated,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserImportCounts {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

impl Runtime {
    /// 一行导入会落成什么（与 `user_upsert_op` 同一判据：权重更大或补上 0 边界才写）。
    pub fn user_row_effect(&self, schema: &str, row: &UserWord) -> RowEffect {
        match self.ud.get(U, schema, &row.code, &row.text) {
            None => RowEffect::Added,
            Some(e) if row.weight > e.weight || (e.boundary == 0 && row.boundary != 0) => {
                RowEffect::Updated
            }
            Some(_) => RowEffect::Unchanged,
        }
    }
}
```

`import_user_rows` 改为先逐行 `user_row_effect` 计数（在发操作之前算，同一批里重复的键按第一次的效果计——与 wind-store 逐行事务内判定一致：同批第二次出现时已是「存在」），再按原逻辑发操作、折回，返回 `UserImportCounts`。同批重复键的精确判定：维护一个 `HashMap<(String, String), (i32, u64)>` 记本批已处理键的「合并后权重 / 边界」，第二次出现时用它代替 `ud.get` 的结果判定。

`engine.rs`：

```rust
/// 低频后台节拍：每 `every` 看一次是否需要维护（含文本目标空闲折回），需要就起后台折回。
/// 只持 `Weak`，引擎释放后下一拍退出。只读实例不起。
pub fn spawn_ticker(self: &Arc<Self>, every: std::time::Duration) -> Option<std::thread::JoinHandle<()>> {
    if self.readonly() {
        return None;
    }
    let weak = Arc::downgrade(self);
    std::thread::Builder::new()
        .name("userdata-tick".into())
        .spawn(move || loop {
            std::thread::sleep(every);
            let Some(me) = weak.upgrade() else { return };
            if me.needs_maintain() {
                me.spawn_maintain();
            }
        })
        .ok()
}
```

空闲时长做成字段：`text_idle: Mutex<std::time::Duration>`，初值 `TEXT_IDLE`；`needs_maintain_with` / `maintain_with` 里原来用 `TEXT_IDLE` 的两处改读这个字段。加 `#[doc(hidden)] pub fn set_text_idle_for_test(&self, d: std::time::Duration)`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全 ok。

- [ ] **Step 5: 提交**

```bash
cargo fmt -p wind-userdata
git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 导入分类、大批追加线性化、后台节拍驱动空闲折回
EOF
```

---

### Task 2：wind-store 用户词门面委托（文本位置、绑定、读写方法）

**Files:**
- Modify: `wind_input/crates/wind-store/src/store.rs`（`StateLoc.text_dir`、派生 `<redb>.user`、打开时绑定、`user_text_dir()`、`ensure_user_text()`）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（方法体委托；删 `on_word_selected`；测试分拣）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（`runtime_state_loc` 多传用户目录）
- Test: `wind_input/crates/wind-store/src/user_words.rs`（tests 模块）

**Interfaces:**
- Consumes: Task 1 的 `UserImportCounts` / `RowEffect` / `user_row_effect` / `import_user_rows`；P2a 的 `UserData::bind_text`、`text_binding`、`fold`、`Runtime` 用户词 API。
- Produces:
  - `pub struct StateLoc { pub state_dir: PathBuf, pub dir_id: String, pub text_dir: PathBuf }`
  - `pub fn derived_text_dir(path: &Path) -> PathBuf`（`<redb>.user`）
  - `Store::user_text_dir(&self) -> &Path`
  - `Store::user_text_path(&self, schema: &str) -> PathBuf`（`<text_dir>/<schema>/user.dict.yaml`）
  - `pub(crate) fn Store::ensure_user_text(&self, schema: &str)`（未绑定则绑定；绑定失败只 WARN，绑定照样登记）
  - `wind_coordinator::construct::runtime_state_loc(state_base: &Path, dir_id: &str, user_dir: &Path) -> StateLoc`

- [ ] **Step 1: 写失败测试**

`user_words.rs` tests 模块追加：

```rust
/// 用户词落进文本：门面写 → 折回 → 文件里有；重开后从文本读回。
#[test]
fn user_words_live_in_text() {
    let p = db("text");
    {
        let s = Store::open(&p).unwrap();
        s.add_user_word("pinyin", "nihao", "你好", 9, 0b101).unwrap();
        s.runtime().data().fold(wind_userdata::record::Target::User, "pinyin").unwrap();
        let f = s.user_text_path("pinyin");
        assert!(std::fs::read_to_string(&f).unwrap().contains("你好\tni hao\t9"));
    }
    let s = Store::open(&p).unwrap();
    let got = s.get_user_words("pinyin", "nihao").unwrap();
    assert_eq!((got[0].text.as_str(), got[0].weight, got[0].boundary), ("你好", 9, 0b101));
}

/// Review Focus 3：未折回的新词列得出、数得对、导出排最后。
#[test]
fn unfolded_words_are_listed_counted_and_exported_last() {
    let s = Store::open(db("unfolded")).unwrap();
    s.add_user_word("wb", "aa", "式", 5, 0).unwrap();
    s.runtime().data().fold(wind_userdata::record::Target::User, "wb").unwrap();
    s.add_user_word("wb", "aa", "工", 5, 0).unwrap(); // 未折回
    let mut n = 0;
    s.for_each_user_word("wb", "", &mut |_| { n += 1; true }).unwrap();
    assert_eq!(n, 2);
    assert_eq!(s.count_user_words("wb").unwrap(), 2);
    let rows = s.collect_user_word_rows("wb").unwrap();
    assert_eq!(rows.last().unwrap().text, "工", "未折回的新词导出在末尾");
}

/// Review Focus 3：预览分类与实际导入一致（含同批重复键、补边界）。
#[test]
fn preview_matches_import_counts() {
    let s = Store::open(db("preview2")).unwrap();
    s.add_user_word("pinyin", "nihao", "你好", 100, 0).unwrap();
    let rows = vec![
        io("ni hao", "你好", 1),
        io("ta", "他", 3),
        io("ta", "他", 9),
    ];
    let (pc, _) = s.preview_import_user_words("pinyin", &rows).unwrap();
    let ic = s.import_user_words("pinyin", &rows).unwrap();
    assert_eq!((pc.added, pc.updated, pc.unchanged), (ic.added, ic.updated, ic.unchanged));
    assert_eq!((ic.added, ic.updated, ic.unchanged), (1, 2, 0));
}
```

（`db(name)` 给 `temp_dir()/wind_uw_<name>_<pid>.redb` 且先删同名文件；`io(code, text, w)` 造 `wdict::WordIo { boundary: None, count: 0, .. }`。tests 模块里已有同类 helper 就复用，没有就加这两个。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p wind-store --lib user_words 2>&1 | grep -E "^test .*FAILED|error\[" | head`
Expected: 编译失败（`user_text_path` 不存在）。

- [ ] **Step 3: 实现文本位置与绑定**

`store.rs`：
- `StateLoc` 加 `text_dir: PathBuf`。
- 加 `pub fn derived_text_dir(path: &Path) -> PathBuf`：在 redb 文件名后追加 `.user`，写法同 `derived_state_dir`。
- `open_with_cache_size` 在 `!path.exists()` 时同样 `remove_dir_all(derived_text_dir(path))`，并在 `StateLoc` 里填 `text_dir: derived_text_dir(path)`。
- `Store` 加字段 `text_dir: PathBuf`。

```rust
pub fn user_text_dir(&self) -> &Path {
    &self.text_dir
}

pub fn user_text_path(&self, schema: &str) -> PathBuf {
    self.text_dir.join(schema).join("user.dict.yaml")
}

/// 该方案的用户词绑定到文本（已绑定即返回）。同步失败（文本隔离 / 读不了）只 WARN：绑定照样
/// 登记，之后的折回 / 同步会重试；查询按已有段与 delta 提供。
pub(crate) fn ensure_user_text(&self, schema: &str) {
    use wind_userdata::record::Target;
    let ud = self.runtime.data();
    if ud.text_binding(Target::User, schema).is_some() {
        return;
    }
    let b = wind_userdata::engine::TextBinding {
        path: self.user_text_path(schema),
        name: format!("{schema}.user"),
        lowercase: false,
        syllable_codes: schema == "pinyin",
    };
    if let Err(e) = ud.bind_text(Target::User, schema, b) {
        warn!("用户词文本 {schema} 同步失败（按已有数据提供查询）：{e:#}");
    }
}

/// 打开时绑定：用户目录下已有文本的方案 ∪ 引擎里已有用户词数据的方案。
fn bind_user_texts_at_open(&self) {
    let mut schemas: std::collections::BTreeSet<String> = self
        .runtime
        .data()
        .keys()
        .into_iter()
        .filter(|(t, _)| *t == wind_userdata::record::Target::User)
        .map(|(_, s)| s)
        .collect();
    if let Ok(rd) = std::fs::read_dir(&self.text_dir) {
        for e in rd.flatten() {
            if e.path().join("user.dict.yaml").is_file()
                && let Some(n) = e.file_name().to_str()
            {
                schemas.insert(n.to_string());
            }
        }
    }
    for s in schemas {
        self.ensure_user_text(&s);
    }
}
```

`open_inner` 在 `run_migrations()` 之后、`migrate_p1_if_needed` 之前调 `store.bind_user_texts_at_open()`；删掉 `store.backfill_abbrev_indexes()` 这一行，函数本体在 Task 3 删。

`construct.rs`：
- `runtime_state_loc` 加参数 `user_dir: &Path`，填 `text_dir: user_dir.join("user")`。
- `open_user_store` 的两处 Production 调用传 `dir`。
- 「取不到本机状态目录」那条回退改为：

```rust
Store::open_with_state(
    &p,
    StateLoc {
        state_dir: derived_state_dir(&p),
        dir_id: dir_id.into(),
        text_dir: dir.join("user"),
    },
)
```

生产环境下文本永远在真实用户目录里。
- `store.rs` 里 tests 的 `StateLoc { .. }`（约 944 行）与 `construct.rs` 的 `p1_meta_tests` 同步补字段。

- [ ] **Step 4: 实现方法委托**

`user_words.rs` 的 `impl Store` 各方法改为下列实现（`use wind_userdata::runtime::UserWord;`）。保留签名，保留 `USER_WORD_TEXT_MAX_CHARS` / `USER_WORD_CODE_MAX_CHARS` 两个常量（委托给 runtime，同值，Runtime 侧报错文案含「过长」）。

```rust
fn order_out(o: u32) -> u32 {
    // 未折回的新词 order 为 0：门面上排到最后（与用户词层同向）
    if o == 0 { u32::MAX } else { o }
}

fn record_of(w: UserWord) -> UserWordRecord {
    UserWordRecord {
        order: order_out(w.order),
        code: w.code,
        text: w.text,
        weight: w.weight,
        count: 0,
        created_at: 0,
        boundary: w.boundary,
    }
}

pub fn add_user_word(&self, schema: &str, code: &str, text: &str, weight: i32, boundary: u64) -> anyhow::Result<()> {
    self.ensure_user_text(schema);
    self.runtime().add_user_word(schema, code, text, weight, boundary)?;
    self.bump_words_gen(schema);
    self.after_write();
    Ok(())
}

pub fn for_each_user_word(&self, schema: &str, prefix: &str, f: &mut dyn FnMut(UserWordView<'_>) -> bool) -> anyhow::Result<()> {
    self.ensure_user_text(schema);
    self.runtime().for_each_user_word(schema, prefix, &mut |w| {
        f(UserWordView {
            code: &w.code,
            text: &w.text,
            weight: w.weight,
            count: 0,
            created_at: 0,
            boundary: w.boundary,
            order: order_out(w.order),
        })
    });
    Ok(())
}

pub fn count_user_words(&self, schema: &str) -> anyhow::Result<usize> {
    self.ensure_user_text(schema);
    Ok(self.runtime().count_user_words(schema))
}

pub fn get_user_words(&self, schema: &str, code: &str) -> anyhow::Result<Vec<UserWordRecord>> {
    self.ensure_user_text(schema);
    Ok(self.runtime().user_words(schema, code).into_iter().map(record_of).collect())
}

pub fn search_user_words_prefix(&self, schema: &str, prefix: &str, limit: usize) -> anyhow::Result<Vec<UserWordRecord>> {
    let mut out = Vec::new();
    self.for_each_user_word(schema, prefix, &mut |w| {
        out.push(w.to_record());
        limit == 0 || out.len() < limit
    })?;
    Ok(out)
}

pub fn remove_user_word(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
    self.ensure_user_text(schema);
    self.runtime().remove_user_word(schema, code, text)?;
    self.bump_words_gen(schema);
    self.after_write();
    Ok(())
}

pub fn update_user_word_weight(&self, schema: &str, code: &str, text: &str, new_weight: i32) -> anyhow::Result<bool> {
    self.ensure_user_text(schema);
    let ok = self.runtime().update_user_word_weight(schema, code, text, new_weight)?;
    self.bump_words_gen(schema);
    self.after_write();
    Ok(ok)
}

pub fn clear_user_words(&self, schema: &str) -> anyhow::Result<usize> {
    self.ensure_user_text(schema);
    let n = self.runtime().clear_user_words(schema)?;
    self.bump_words_gen(schema);
    Ok(n)
}

/// `WordIo` → `UserWord`：码拆成扁平码 + 边界，显式边界优先（同改动前的导入闸口）。
fn user_rows(rows: &[wdict::WordIo]) -> Vec<UserWord> {
    rows.iter()
        .map(|r| {
            let (code, spaced_b) = wdict::split_spaced_code(&r.code);
            UserWord {
                code,
                text: r.text.clone(),
                weight: r.weight,
                boundary: r.boundary.unwrap_or(spaced_b),
                order: 0,
            }
        })
        .collect()
}

pub fn import_user_words(&self, schema: &str, rows: &[wdict::WordIo]) -> anyhow::Result<WordsImportCounts> {
    self.ensure_user_text(schema);
    let c = self.runtime().import_user_rows(schema, &user_rows(rows))?;
    self.bump_words_gen(schema);
    Ok(WordsImportCounts { added: c.added, updated: c.updated, unchanged: c.unchanged })
}

pub fn preview_import_user_words(&self, schema: &str, rows: &[wdict::WordIo]) -> anyhow::Result<(WordsImportCounts, Vec<String>)> {
    self.ensure_user_text(schema);
    use wind_userdata::runtime::RowEffect;
    let mut c = WordsImportCounts::default();
    let mut samples = Vec::new();
    // 同批重复键：第二次出现时按「本批已合并」的结果判定（与 import_user_rows 一致）
    let mut seen: std::collections::HashMap<(String, String), (i32, u64)> = Default::default();
    for (r, u) in rows.iter().zip(user_rows(rows)) {
        let key = (u.code.clone(), u.text.clone());
        let effect = match seen.get(&key) {
            None => self.runtime().user_row_effect(schema, &u),
            Some(&(w, b)) if u.weight > w || (b == 0 && u.boundary != 0) => RowEffect::Updated,
            Some(_) => RowEffect::Unchanged,
        };
        let prev = seen.get(&key).copied().or_else(|| {
            self.runtime().user_word(schema, &u.code, &u.text).map(|e| (e.weight, e.boundary))
        });
        let merged = match prev {
            Some((w, b)) => (w.max(u.weight), if b != 0 { b } else { u.boundary }),
            None => (u.weight, u.boundary),
        };
        seen.insert(key, merged);
        match effect {
            RowEffect::Added => c.added += 1,
            RowEffect::Updated => c.updated += 1,
            RowEffect::Unchanged => c.unchanged += 1,
        }
        if effect != RowEffect::Unchanged && samples.len() < 5 {
            samples.push(format!("{} {}", r.code, r.text));
        }
    }
    Ok((c, samples))
}
```

`collect_user_word_rows` 保持原样：它走 `search_user_words_prefix` 再按 `order` 稳定排序，`order_out` 已把新词排到最后。

删除 `on_word_selected` 及其测试 `test_on_word_selected_threshold_boost`（计划内裁定 2）。

`now_secs`、`take_word_orders`、`enc_val*`、`dec_val*`、`enc_key`、`split_key` 保留：迁移读旧表要用 `dec_val_ordered`，`legacy_put_user`（Task 4）写旧表要用 `enc_val_ordered`。编译器报未用的，在 Task 4 用上之前加 `#[allow(dead_code)]`，Task 4 收尾时去掉。

- [ ] **Step 5: 跑 wind-store 全部测试并分拣**

Run: `cargo test -p wind-store --no-fail-fast 2>&1 | grep -E "^test .*FAILED|test result" > <独立临时目录>/p2b-t2.txt; cat <独立临时目录>/p2b-t2.txt`
Expected: 新加的三条通过。预期会红的旧测试按下表处置，每条处置写账本 `Task 2: Ruling:`。

| 失败原因 | 处置 |
|---|---|
| 断言 `count` / `created_at`（含 `export_dict_wdict_roundtrips_words_count_and_shadow` 的 count 部分） | 改断言为 0 / 删掉该断言；词与权重部分保留 |
| 测 redb 编码（`dec_val_reads_v1_16byte_records`、`dec_val_ordered_reads_v1_and_v2_records`） | 保留（迁移要用这些解码） |
| `v1_record_upgrades_on_write`（写路径惰性升级旧记录） | 删除：写路径不再碰 redb |
| `USER_ABBREV` 维护（`every_write_path_maintains_the_index`、`weight_changes_do_not_touch_the_index`、`filling_boundary_moves_the_index_key`、`rebuild_covers_preexisting_words`） | 留到 Task 3 统一处理（Task 3 删 `abbrev_index_len` / `rebuild_abbrev_indexes`）；本任务先 `#[ignore = "Task 3 处理"]` |
| 测 `order` 精确数值 | 改为比较相对先后（行号代替旧序号） |
| 其它语义失败 | 是回归：修代码，不改测试 |

`tests/` 下的 redb 机制测试（`drop_page_cache.rs` 等）若在本任务就红，同样先 `#[ignore = "Task 3 处理"]`。

- [ ] **Step 6: 跑 wind-store + wind-coordinator 确认绿**

Run: `cargo test -p wind-store -p wind-coordinator --no-fail-fast 2>&1 | grep -E "FAILED|test result" | sort | uniq -c | sort -rn | head`
Expected: 0 failed（ignored 若干，均标注「Task 3 处理」）。

- [ ] **Step 7: 提交**

```bash
cargo fmt -p wind-store -p wind-coordinator
git commit -F - --only -- wind_input/crates/wind-store wind_input/crates/wind-coordinator/src/construct.rs <<'EOF'
feat(store): 用户词门面委托文本层；文本位置随用户目录，打开时绑定
EOF
```

---

### Task 3：用户词层、简拼、晋升接到引擎；退役 USER_ABBREV 维护；redb 机制测试换负载

**Files:**
- Modify: `wind_input/crates/wind-store/src/store_layer.rs`（`StoreUserLayer` 改薄包装）
- Modify: `wind_input/crates/wind-store/src/abbrev_index.rs`（用户词简拼走引擎层；删 `abbrev_index_len` / `rebuild_abbrev_indexes`）
- Modify: `wind_input/crates/wind-store/src/temp_words.rs`（`promote_temp_word` 走 `add_user_word`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（删 `backfill_abbrev_indexes`；`list_data_schemas` 不再读 `USER_WORDS`）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（删 Task 2 标 ignore 的 4 条索引测试）
- Modify: `wind_input/crates/wind-store/tests/{drop_page_cache,idle_cache_reclaim,scan_reclaim,redb_cache_high_water}.rs`（负载换成短语）
- Delete: `wind_input/crates/wind-store/tests/perf_abbrev_index.rs`、`wind_input/crates/wind-store/tests/perf_cache_size.rs`

**Interfaces:**
- Consumes: `wind_userdata::layer::UserDataUserLayer::new(ud: Arc<UserData>, schema: &str)`；Task 2 的 `ensure_user_text`。
- Produces: `StoreUserLayer::new(store: Arc<Store>, schema_id)` 签名不变。

- [ ] **Step 1: 写失败测试**

`store_layer.rs` tests 模块（没有就新建 `#[cfg(test)] mod tests`）追加：

```rust
/// 用户词层：手改文本后经 sync 立即可查；简拼走引擎分组索引（无边界词靠首字符兜底组）。
#[test]
fn user_layer_reads_engine_and_text() {
    use wind_dict::DictLayer;
    let p = std::env::temp_dir().join(format!("wind_sl_user_{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let s = Arc::new(Store::open(&p).unwrap());
    s.add_user_word("pinyin", "nihao", "你好", 9, 0b101).unwrap();
    s.add_user_word("pinyin", "xianning", "西安宁", 5, 0).unwrap();
    let l = StoreUserLayer::new(s.clone(), "pinyin");
    assert_eq!(l.search("nihao", 0)[0].text, "你好");
    assert!(l.search("nihao", 0)[0].meta.is_user_dict);
    assert_eq!(l.search_abbrev("nh", 0)[0].text, "你好");
    assert!(l.search_abbrev("xan", 0).iter().any(|c| c.text == "西安宁"));
    assert_eq!(s.search_user_words_by_abbrev_exact("pinyin", "nh", 0).unwrap()[0].text, "你好");
}

/// 晋升：临时词进用户词（权重至少 PROMOTED_WEIGHT、边界随词），临时词删掉，用户词落文本。
#[test]
fn promote_writes_user_word_through_engine() {
    let p = std::env::temp_dir().join(format!("wind_sl_promote_{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let s = Store::open(&p).unwrap();
    s.learn_temp_word("pinyin", "nihao", "你好", 1, 0b101).unwrap();
    assert!(s.promote_temp_word("pinyin", "nihao", "你好").unwrap());
    assert!(s.runtime().temp_word("pinyin", "nihao", "你好").is_none());
    let w = &s.get_user_words("pinyin", "nihao").unwrap()[0];
    assert_eq!(w.boundary, 0b101);
    s.runtime().data().fold(wind_userdata::record::Target::User, "pinyin").unwrap();
    assert!(std::fs::read_to_string(s.user_text_path("pinyin")).unwrap().contains("你好\tni hao"));
}
```

（`learn_temp_word` 的确切签名以 `temp_words.rs` 为准；参数个数不同就照它调整。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p wind-store --lib store_layer 2>&1 | grep -E "^test |panicked" | head`
Expected:
- `user_layer_reads_engine_and_text` FAIL：`StoreUserLayer` 仍查 redb，空结果下标越界。
- `promote_writes_user_word_through_engine` FAIL：文件里没有这个词，晋升写的是 redb。

- [ ] **Step 3: 实现**

`StoreUserLayer` 改为 `StoreTempLayer` 同款薄包装：

```rust
/// 用户造词层（P2 起数据在文本 + 引擎；本类型是 `UserDataUserLayer` 的薄包装）。
pub struct StoreUserLayer {
    inner: wind_userdata::layer::UserDataUserLayer,
}

impl StoreUserLayer {
    pub fn new(store: Arc<Store>, schema_id: impl Into<String>) -> Self {
        let schema_id = schema_id.into();
        store.ensure_user_text(&schema_id);
        Self {
            inner: wind_userdata::layer::UserDataUserLayer::new(store.runtime().data().clone(), &schema_id),
        }
    }
}
```

`impl DictLayer for StoreUserLayer` 的 `name` / `layer_type` / `entries_digest` / `search` / `search_prefix` / `search_abbrev` / `search_abbrev_exact` / `search_pattern` 全部转发 `self.inner`，写法同 `StoreTempLayer`。

`record_to_candidate`、`pattern_from_records` 若不再有调用方，一并删除。临时词旧实现是否还在用它们，以 `cargo build` 报 unused 为准。

`abbrev_index.rs`：
- `search_user_words_by_abbrev` / `_exact` 改为 `ensure_user_text(schema)` 之后，用 `UserDataUserLayer::new(self.runtime().data().clone(), schema)` 的 `search_abbrev` / `search_abbrev_exact`，再经 `record_of_candidate` 转成记录。写法同临时词那两个函数。
- `record_of_candidate` 的 `order` 改取 `c.natural_order.max(0) as u32`，用户词层已把未折回新词映射为 `i32::MAX`。
- 删除 `abbrev_index_len`、`rebuild_abbrev_indexes`，以及只为它们服务的 `rebuild` 函数。
- `search`、`insert`、`shift`、`remove`、`clear_schema` 若已无调用方，一并删除。临时词旧表、迁移用到的保留。

`store.rs`：删除 `backfill_abbrev_indexes`。`list_data_schemas` 的表循环改为只读 `[SHADOW]`，注释改成「用户词 P2 起在文本 + 引擎（`runtime.schemas()` 已含）」。

`temp_words.rs` 的 `promote_temp_word`：

```rust
pub fn promote_temp_word(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<bool> {
    // 先写用户词、再删临时词：崩溃在两步之间两边各有一份，用户层去重兜得住；反过来会丢词。
    let Some(tw) = self.runtime().temp_word(schema, code, text) else {
        return Ok(false);
    };
    self.add_user_word(schema, code, text, PROMOTED_WEIGHT, tw.boundary)?;
    self.runtime().remove_temp(schema, code, text)?;
    self.bump_words_gen(schema);
    self.after_write();
    Ok(true)
}
```

`add_user_word` 本身就是「已存在则权重取 max、0 边界补齐」，与旧事务里的合并语义一致；只有 count 相加这一项不再保留（计划内裁定 3）。去掉 `temp_words.rs` 对 `USER_WORDS` / `USER_ABBREV` / `enc_val_ordered` 的 import。该文件 tests 里 492、528 行附近清 `USER_ABBREV` 的测试辅助代码，随所在测试一起按 Step 4 分拣。

redb 机制测试换负载（计划内裁定 5）：
- 把 `import_user_words("py", &rows(n))` 换成 `import_phrases_appending(&items, 100)`，`items` 为 `(format!("a{i:04}"), format!("词{i:04}"))`。
- `search_user_words_prefix("py", "", 0)` 换成 `list_phrases()`，`search_user_words_prefix("py", "a01", 10)` 换成 `list_user_phrases_paged(Some("a01"), 0, 10)`。
- `count_user_words("py")` 换成 `list_phrases().map(|v| v.len())`。
- `add_user_word("py", "zz", "新词", 100, 0)` 换成 `add_phrase("zz", "新词", 0, 100)`。
- 断言里的 500 / 501 等条数照搬，短语表初始为空。`scan_reclaim.rs` 里「按键查库」的 `count_user_words` 同样换成 `list_phrases()`。

删除 `tests/perf_abbrev_index.rs`、`tests/perf_cache_size.rs`。用 `grep -rn "perf_cache_size\|perf_abbrev_index" wind_input docs` 找引用它们的注释或文档，改成「已随 P2b 删除（用户词不在 redb）」或删掉该句。

- [ ] **Step 4: 跑测试并收尾分拣**

Run: `cargo test -p wind-store -p wind-engine -p wind-coordinator -p wind-webdata --no-fail-fast 2>&1 | grep -E "^test .*FAILED|test result" | sort | uniq -c | sort -rn | head`
Expected: 0 failed。Task 2 标 `#[ignore = "Task 3 处理"]` 的测试此时一条不剩：要么删了（索引维护类），要么换负载后恢复。

- [ ] **Step 5: 提交**

```bash
cargo fmt -p wind-store
git commit -F - --only -- wind_input/crates/wind-store docs <<'EOF'
feat(store): 用户词层 / 简拼 / 晋升接到引擎，退役用户词简拼索引；redb 机制测试改用短语表
EOF
```

---

### Task 4：USER_WORDS → user.dict.yaml 迁移、format_version 3

**Files:**
- Create: `wind_input/crates/wind-store/src/migrate_p2.rs`
- Modify: `wind_input/crates/wind-store/src/lib.rs`（`mod migrate_p2;`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（`open_inner` 调迁移；`migrated_p2()`）
- Modify: `wind_input/crates/wind-config/src/user_meta.rs`（`KNOWN_FORMAT_VERSION = 3`；初次生成的版本号）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（按迁移进度升版本）
- Create: `wind_input/crates/wind-store/examples/p2_migrate_check.rs`
- Test: `migrate_p2.rs` tests 模块、`wind-config/src/user_meta.rs` tests、`construct.rs` 的 `p1_meta_tests`

**Interfaces:**
- Consumes: Task 2 的 `ensure_user_text`、`user_text_path`；`UserData::fold`、`UserData::rewrite_text(target, schema, rows: &[(String, String, u64, i32)])`；Task 1 的 `Runtime::import_user_rows`。
- Produces:
  - `pub const MIGRATED_P2: &str = "migrated-p2";`
  - `Store::migrated_p2(&self) -> bool`
  - `#[doc(hidden)] pub fn Store::legacy_put_user(&self, schema: &str, code: &str, text: &str, weight: i32, boundary: u64, order: u32) -> anyhow::Result<()>`（测试与探针写旧表用）
  - `wind_config::user_meta::KNOWN_FORMAT_VERSION == 3`

- [ ] **Step 1: 写失败测试**

`migrate_p2.rs` 先只放 tests 模块：

```rust
#[cfg(test)]
mod tests {
    use crate::Store;

    fn db(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("wind_mp2_{name}_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(crate::store::derived_state_dir(&p));
        let _ = std::fs::remove_dir_all(crate::store::derived_text_dir(&p));
        p
    }

    /// 造一个「旧版」库：用户词只在 USER_WORDS 里、没有迁移标记。
    fn legacy(name: &str, rows: &[(&str, &str, &str, i32, u64, u32)]) -> std::path::PathBuf {
        let p = db(name);
        {
            let s = Store::open(&p).unwrap();
            for (schema, code, text, w, b, o) in rows {
                s.legacy_put_user(schema, code, text, *w, *b, *o).unwrap();
            }
        }
        let _ = std::fs::remove_file(
            crate::store::derived_state_dir(&p).join(super::MIGRATED_P2),
        );
        p
    }

    /// Review Focus 1：词、权重、边界、同码先后、特殊文本都原样迁过去；旧表只读保留。
    #[test]
    fn migration_preserves_words_order_and_boundary() {
        let p = legacy("basic", &[
            ("pinyin", "ni", "泥", 1, 0b1, 3),
            ("pinyin", "ni", "你", 1, 0b1, 2),
            ("pinyin", "nihao", "你好", 9, 0b101, 1),
            ("pinyin", "xianning", "西安宁", 5, 0, 0),
            ("pinyin", "tag", "#话题", 5, 0, 4),
            ("wb", "aaaa", "工\t具\n行", 7, 0, 1),
        ]);
        let s = Store::open(&p).unwrap();
        assert!(s.migrated_p2());
        let f = std::fs::read_to_string(s.user_text_path("pinyin")).unwrap();
        let body: Vec<&str> = f.lines().skip_while(|l| *l != "# no comment").skip(1).collect();
        assert_eq!(body, vec![
            "西安宁\txianning\t5", // order 0 的旧记录在前（同旧导出口径）
            "你好\tni hao\t9",
            "你\tni\t1",
            "泥\tni\t1",
            "#话题\ttag\t5",
        ]);
        let got = s.get_user_words("wb", "aaaa").unwrap();
        assert_eq!(got[0].text, "工\t具\n行");
        let ni: Vec<String> = {
            let mut v = s.get_user_words("pinyin", "ni").unwrap();
            v.sort_by_key(|r| r.order);
            v.into_iter().map(|r| r.text).collect()
        };
        assert_eq!(ni, vec!["你", "泥"]);
        assert_eq!(s.get_user_words("pinyin", "tag").unwrap()[0].text, "#话题");
    }

    /// Review Focus 4：标记写入前崩溃，重跑合并：不重复、不丢、不覆盖两次启动之间新加的词。
    #[test]
    fn rerun_after_crash_merges_without_loss() {
        let p = legacy("rerun", &[("wb", "a", "工", 1, 0, 1), ("wb", "b", "了", 1, 0, 2)]);
        {
            let s = Store::open(&p).unwrap();
            s.add_user_word("wb", "c", "以", 3, 0).unwrap();
            s.update_user_word_weight("wb", "a", "工", 50).unwrap();
        }
        let _ = std::fs::remove_file(crate::store::derived_state_dir(&p).join(super::MIGRATED_P2));
        let s = Store::open(&p).unwrap();
        assert!(s.migrated_p2());
        let mut all: Vec<(String, i32)> = s
            .search_user_words_prefix("wb", "", 0)
            .unwrap()
            .into_iter()
            .map(|r| (r.text, r.weight))
            .collect();
        all.sort();
        assert_eq!(all, vec![("了".into(), 1), ("以".into(), 3), ("工".into(), 50)]);
        let f = std::fs::read_to_string(s.user_text_path("wb")).unwrap();
        assert_eq!(f.matches("工\ta").count(), 1, "不重复成行");
    }

    /// 没有旧用户词：只写标记，不建空文件。
    #[test]
    fn empty_legacy_writes_marker_only() {
        let p = legacy("empty", &[]);
        let s = Store::open(&p).unwrap();
        assert!(s.migrated_p2());
        assert!(!s.user_text_path("pinyin").exists());
    }
}
```

`wind-config/src/user_meta.rs` tests 追加：

```rust
#[test]
fn fresh_meta_beside_legacy_redb_starts_at_1() {
    let d = std::env::temp_dir().join(format!("wind_meta_legacy_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("userdata.redb"), b"x").unwrap();
    match load_or_init(&d) {
        MetaCheck::Ok(m) => assert_eq!(m.format_version, 1),
        other => panic!("{other:?}"),
    }
}
```

`construct.rs` 的 `p1_meta_tests` 追加：

```rust
/// P1、P2 都迁完才升到 3；只迁完 P1 升到 2。
#[test]
fn format_version_follows_migration_progress() {
    // 构造方式照本模块已有的 P1 测试：临时用户目录 + open_user_store(dir, StateChoice::Derived, "t")
    // 1) 新目录（无 redb）→ 打开后 meta.format_version == 3
    // 2) 删掉派生状态目录里的 migrated-p2 标记、把 meta 改回 2，再让迁移失败（把 user 目录换成
    //    同名普通文件使 create_dir 失败）→ 打开后 format_version 仍为 2
}
```

把这条用模块里已有 helper 写成可运行代码，两段断言都要有。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p wind-store --lib migrate_p2 2>&1 | grep -E "error\[|FAILED" | head; cargo test -p wind-config user_meta 2>&1 | grep -E "FAILED|test result"`
Expected：
- wind-store 编译失败：`legacy_put_user` / `migrated_p2` / `MIGRATED_P2` 不存在。
- wind-config 的 `fresh_meta_beside_legacy_redb_starts_at_1` FAIL：实际值是 2。

- [ ] **Step 3: 实现迁移**

```rust
//! P2 迁移（设计 §7.2）：redb `USER_WORDS` 一次性迁成 `user/<schema>/user.dict.yaml`。
//! 每个方案先折回已有的待处理操作；文件不存在 ⇒ 按旧序整文件写出（快）；文件已在（上一轮
//! 迁移中途失败、或用户自己放了文本）⇒ 逐条合并（权重取 max、补边界，已有行不挪）。全部完成后写
//! `state/migrated-p2`；没有标记就整体重做（重做是合并，幂等）。旧表只读保留到 P4。

use crate::Store;
use crate::store::USER_WORDS;
use std::collections::BTreeMap;
use std::path::Path;
use wind_userdata::record::Target;
use wind_userdata::runtime::UserWord;

pub const MIGRATED_P2: &str = "migrated-p2";

impl Store {
    /// 旧表全部用户词，按方案分组，组内按旧导出口径排序（order 升序，稳定；order 0 的旧记录在前）。
    fn legacy_user_rows(&self) -> anyhow::Result<BTreeMap<String, Vec<(UserWord, u32)>>> {
        self.with_db(|db| {
            let txn = db.begin_read()?;
            let t = txn.open_table(USER_WORDS)?;
            let mut out: BTreeMap<String, Vec<(UserWord, u32)>> = BTreeMap::new();
            for item in t.range::<&str>(..)? {
                let (k, v) = item?;
                let Some((schema, code, text)) = crate::user_words::split_key(k.value()) else {
                    continue;
                };
                let Some((w, _, _, b, o)) = crate::user_words::dec_val_ordered(v.value()) else {
                    continue;
                };
                if code.is_empty() || text.is_empty() {
                    continue;
                }
                out.entry(schema.to_string()).or_default().push((
                    UserWord { code: code.into(), text: text.into(), weight: w, boundary: b, order: 0 },
                    o,
                ));
            }
            for v in out.values_mut() {
                v.sort_by_key(|(_, o)| *o);
            }
            Ok(out)
        })
    }

    pub(crate) fn migrate_p2_if_needed(&self, state_dir: &Path) -> anyhow::Result<bool> {
        let marker = state_dir.join(MIGRATED_P2);
        if marker.is_file() {
            return Ok(false);
        }
        let ud = self.runtime().data().clone();
        let mut total = 0usize;
        for (schema, rows) in self.legacy_user_rows()? {
            self.ensure_user_text(&schema);
            if ud.pending(Target::User, &schema) > 0 {
                ud.fold(Target::User, &schema)?;
            }
            let rows: Vec<UserWord> = rows.into_iter().map(|(w, _)| w).collect();
            total += rows.len();
            if self.user_text_path(&schema).is_file() {
                self.runtime().import_user_rows(&schema, &rows)?;
            } else {
                let flat: Vec<(String, String, u64, i32)> = rows
                    .into_iter()
                    .map(|w| (w.code, w.text, w.boundary, w.weight))
                    .collect();
                ud.rewrite_text(Target::User, &schema, &flat)?;
            }
            self.bump_words_gen(&schema);
        }
        wind_userdata::state::atomic_write(&marker, b"1\n")?;
        tracing::info!("P2 迁移完成：{total} 条用户词从 redb 迁成文本");
        Ok(true)
    }

    /// P2 迁移是否已完成（标记 `migrated-p2` 在）。
    pub fn migrated_p2(&self) -> bool {
        self.state_dir.join(MIGRATED_P2).is_file()
    }

    /// 测试 / 探针：往旧 `USER_WORDS` 表写一条（模拟升级前的库）。
    #[doc(hidden)]
    pub fn legacy_put_user(&self, schema: &str, code: &str, text: &str, weight: i32, boundary: u64, order: u32) -> anyhow::Result<()> {
        let key = crate::user_words::enc_key(schema, code, text);
        self.with_db(|db| {
            let txn = db.begin_write()?;
            txn.open_table(USER_WORDS)?.insert(
                key.as_str(),
                crate::user_words::enc_val_ordered(weight, 0, 0, boundary, order).as_slice(),
            )?;
            txn.commit()?;
            Ok(())
        })
    }
}
```

`migrated_p2` 放在 `store.rs` 的 `migrated_p1` 旁边，或者放进本文件；两处里只留一处。

`open_inner` 在 `migrate_p1_if_needed` 之后加：

```rust
if let Err(e) = store.migrate_p2_if_needed(&state_dir) {
    warn!("P2 迁移失败，下次启动重试：{e}");
}
```

迁移时只读实例写不了，`ensure_user_text` 只登记绑定、`rewrite_text` 报只读。这时整体返回 Err，不写标记，下一次以写者身份启动时重做。

`legacy_user_rows` 读出的码已经是扁平码（旧表 key 里存的就是扁平码）。拼音的边界随 `rewrite_text` 的 `render` 输出成带空格码，`#` 开头的文本由 `# no comment` 保住，tab / 换行文本由 `escape: true` 保住，都是 P2a 已有的行为。

- [ ] **Step 4: 实现版本号**

- `user_meta.rs`：
  - `KNOWN_FORMAT_VERSION = 3`，文档注释补「3 = P2：用户词在 `user/<schema>/user.dict.yaml`」。
  - `load_or_init` 的 NotFound 分支：`format_version: if user_dir.join("userdata.redb").exists() { 1 } else { KNOWN_FORMAT_VERSION }`。
- `construct.rs` 的 `open_user_store`：

```rust
let target = match (s.migrated_p1(), s.migrated_p2()) {
    (true, true) => Some(wind_config::user_meta::KNOWN_FORMAT_VERSION),
    (true, false) => Some(2),
    _ => None,
};
match target {
    Some(v) => {
        if let Err(e) = wind_config::user_meta::raise_format_version(dir, v) {
            warn!("format_version 升级失败（旧版本仍可能写 redb）：{e}");
        }
        if v < wind_config::user_meta::KNOWN_FORMAT_VERSION {
            warn!("P2 迁移未完成，format_version 停在 {v}，下次启动重试");
        }
    }
    None => warn!("P1 迁移未完成，format_version 暂不升级，下次启动重试"),
}
```

- `grep -rn "KNOWN_FORMAT_VERSION\|format_version" wind_input/crates --include='*.rs'`，把断言写死 `2` 的测试按新语义更新，账本记一行。

- [ ] **Step 5: 加探针 example**

`examples/p2_migrate_check.rs`，照 `examples/p1_migrate_check.rs` 的结构写：
- 读环境变量 `WIND_DB`（真实 `userdata.redb` 的副本路径），把它拷进临时目录后 `Store::open`（派生模式）。
- 迁移前用 `legacy_user_rows` 的同口径读旧表；迁移后对每个方案逐条比较 `get_user_words(schema, code)` 的 (text, weight, boundary) 是否与旧表一致，同码多词按 order 的相对先后也要一致。
- 打印迁移耗时、各方案条数、不一致条数。不一致时退出码为 1。

`p1_migrate_check.rs` 里有可复用的骨架就抄过来；`legacy_user_rows` 是私有的，就在 example 里用 `redb` 直接读，或者给它加 `#[doc(hidden)] pub`。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p wind-store -p wind-config -p wind-coordinator --no-fail-fast 2>&1 | grep -E "^test .*FAILED|test result" | sort | uniq -c | sort -rn | head; cargo build -p wind-store --examples 2>&1 | tail -2`
Expected: 0 failed；examples 编译通过。

- [ ] **Step 7: 提交**

```bash
cargo fmt -p wind-store -p wind-config -p wind-coordinator
git commit -F - --only -- wind_input/crates/wind-store wind_input/crates/wind-config/src/user_meta.rs wind_input/crates/wind-coordinator/src/construct.rs <<'EOF'
feat(store): 用户词旧表一次性迁成 user.dict.yaml（合并、可重跑），迁完 format_version 升到 3
EOF
```

---

### Task 5：手改生效与显式折回（sync 时机、后台节拍、dict.reload / dict.flush）

**Files:**
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（`sync_user_texts`、`sync_user_text`、`reload_user_text_paths`、`flush_user_data`）
- Modify: `wind_input/crates/wind-store/src/store.rs`（`spawn_userdata_ticker`）
- Modify: `wind_input/crates/wind-coordinator/src/construct.rs`（起节拍）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs`（`reload_user_config` 后同步）
- Modify: `wind_input/crates/wind-coordinator/src/handle_mode.rs`（`switch_schema_by_id` 成功后同步该数据方案）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（`dict.reload` / `dict.flush` 分派与 handler）
- Test: `user_words.rs` tests、`wind-webdata/src/lib.rs` tests

**Interfaces:**
- Consumes: `UserData::sync_text(target, schema) -> anyhow::Result<bool>`、`UserData::compact()`、Task 1 的 `spawn_ticker`、Task 2 的 `ensure_user_text` / `user_text_dir` / `user_text_path`。
- Produces:
  - `Store::sync_user_text(&self, schema: &str) -> bool`（变了返回 true 并 bump 代次；失败 WARN 返回 false）
  - `Store::sync_user_texts(&self) -> Vec<String>`（发现 + 同步全部，返回变了的方案）
  - `Store::reload_user_text_paths(&self, paths: &[String]) -> anyhow::Result<Vec<String>>`（越界 ⇒ Err）
  - `Store::flush_user_data(&self) -> anyhow::Result<()>`
  - `Store::spawn_userdata_ticker(&self, every: std::time::Duration)`
  - IPC `dict.reload {paths?: [string]} → {reloaded: [schema]}`，`dict.flush {} → {ok: true}`

- [ ] **Step 1: 写失败测试**

`user_words.rs` tests 追加：

```rust
/// Review Focus 2：手改文本 → sync → 新词可见、删掉的行消失、代次推进；未被手改的未折回操作仍在。
#[test]
fn reload_picks_up_hand_edit_and_bumps_generation() {
    let s = Store::open(db("handedit")).unwrap();
    s.add_user_word("pinyin", "nihao", "你好", 9, 0b101).unwrap();
    s.add_user_word("pinyin", "ta", "他", 1, 0b1).unwrap();
    s.runtime().data().fold(wind_userdata::record::Target::User, "pinyin").unwrap();
    s.add_user_word("pinyin", "wo", "我", 2, 0b1).unwrap(); // 未折回
    let f = s.user_text_path("pinyin");
    let edited = std::fs::read_to_string(&f).unwrap().replace("他\tta\t1\n", "") + "手改\tshou gai\t3\n";
    std::fs::write(&f, edited).unwrap();
    let g0 = s.words_generation_of("pinyin");
    assert_eq!(s.sync_user_texts(), vec!["pinyin".to_string()]);
    assert_ne!(s.words_generation_of("pinyin"), g0);
    assert_eq!(s.get_user_words("pinyin", "shougai").unwrap()[0].text, "手改");
    assert!(s.get_user_words("pinyin", "ta").unwrap().is_empty());
    assert_eq!(s.get_user_words("pinyin", "wo").unwrap()[0].text, "我");
    assert!(s.sync_user_texts().is_empty(), "没变就不重建");
}

/// Review Focus 5：dict.reload 的路径只认 `<text_dir>/<schema>/user.dict.yaml`。
#[test]
fn reload_rejects_paths_outside_user_dir() {
    let s = Store::open(db("reloadpath")).unwrap();
    s.add_user_word("wb", "a", "工", 1, 0).unwrap();
    s.runtime().data().fold(wind_userdata::record::Target::User, "wb").unwrap();
    let ok = s.user_text_path("wb").to_string_lossy().into_owned();
    assert!(s.reload_user_text_paths(&[ok]).is_ok());
    let base = s.user_text_dir().to_string_lossy().into_owned();
    for bad in [
        format!("{base}/../x/user.dict.yaml"),
        format!("{base}/wb/other.dict.yaml"),
        "/etc/passwd".to_string(),
        format!("{base}/wb/../../user.dict.yaml"),
    ] {
        assert!(s.reload_user_text_paths(&[bad.clone()]).is_err(), "{bad}");
    }
}

/// dict.flush：全部待处理操作折回，文本里有。
#[test]
fn flush_folds_everything() {
    let s = Store::open(db("flush")).unwrap();
    s.add_user_word("wb", "a", "工", 1, 0).unwrap();
    s.flush_user_data().unwrap();
    assert!(std::fs::read_to_string(s.user_text_path("wb")).unwrap().contains("工\ta\t1"));
}
```

`wind-webdata/src/lib.rs` tests 追加（`coord(name)` 是模块已有 helper）：

```rust
#[test]
fn dict_reload_and_flush_are_dispatched() {
    let c = coord("dict_reload_flush");
    let r = c.web_data_rpc("dict.reload", &json!({})).unwrap();
    assert!(r.get("reloaded").is_some_and(|v| v.is_array()));
    let r = c.web_data_rpc("dict.flush", &json!({})).unwrap();
    assert_eq!(r, json!({"ok": true}));
    assert!(c.web_data_rpc("dict.reload", &json!({"paths": ["/etc/passwd"]})).is_err());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p wind-store --lib user_words 2>&1 | grep -E "error\[|FAILED" | head -5`
Expected: 编译失败（`sync_user_texts` 等不存在）。

- [ ] **Step 3: 实现 Store 侧**

```rust
/// 由文本重建该方案的用户词段（手改生效）。变了 ⇒ 推进代次（联想 / 按词查码索引据此重建）。
pub fn sync_user_text(&self, schema: &str) -> bool {
    use wind_userdata::record::Target;
    self.ensure_user_text(schema);
    match self.runtime().data().sync_text(Target::User, schema) {
        Ok(true) => {
            self.bump_words_gen(schema);
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!("用户词文本 {schema} 同步失败：{e:#}");
            false
        }
    }
}

/// 发现（用户目录下新出现的方案目录）+ 同步全部已绑定方案，返回变了的方案。
pub fn sync_user_texts(&self) -> Vec<String> {
    use wind_userdata::record::Target;
    let mut schemas: std::collections::BTreeSet<String> = Default::default();
    if let Ok(rd) = std::fs::read_dir(self.user_text_dir()) {
        for e in rd.flatten() {
            if e.path().join("user.dict.yaml").is_file()
                && let Some(n) = e.file_name().to_str()
            {
                schemas.insert(n.to_string());
            }
        }
    }
    schemas.extend(
        self.runtime().data().keys().into_iter()
            .filter(|(t, _)| *t == Target::User)
            .map(|(_, s)| s),
    );
    schemas.into_iter().filter(|s| self.sync_user_text(s)).collect()
}

/// `dict.reload` 带路径：只认 `<text_dir>/<schema>/user.dict.yaml`（按路径分量比对，不跟随
/// 符号链接、不碰文件系统）；任一路径越界 ⇒ 整体 Err、一个都不同步。
pub fn reload_user_text_paths(&self, paths: &[String]) -> anyhow::Result<Vec<String>> {
    use std::path::Component;
    let base: Vec<Component> = self.user_text_dir().components().collect();
    let mut schemas = Vec::new();
    for p in paths {
        let comps: Vec<Component> = Path::new(p).components().collect();
        let ok = comps.len() == base.len() + 2
            && comps[..base.len()] == base[..]
            && matches!(comps[base.len()], Component::Normal(_))
            && comps[base.len() + 1] == Component::Normal("user.dict.yaml".as_ref());
        if !ok {
            anyhow::bail!("路径不在用户词目录内或不是 user.dict.yaml：{p}");
        }
        let Component::Normal(s) = comps[base.len()] else { unreachable!() };
        schemas.push(s.to_string_lossy().into_owned());
    }
    Ok(schemas.into_iter().filter(|s| self.sync_user_text(s)).collect())
}

/// `dict.flush`：折回全部目标（文本目标写回文件），再压实日志。
pub fn flush_user_data(&self) -> anyhow::Result<()> {
    self.runtime().data().compact()
}
```

`Path::components()` 会规整掉 `.`，但保留 `..`；`..` 是 `Component::ParentDir`，不等于 `Normal`，所以含 `..` 的路径必然比对失败。这个检查只做分量比对、不 canonicalize：符号链接指向哪里由用户自己负责，我们只保证不按调用方给的字符串越出目录。

`store.rs`：

```rust
/// 用户数据后台节拍（生产由协调器起）：文本目标空闲折回不依赖下一次写入。
pub fn spawn_userdata_ticker(&self, every: std::time::Duration) {
    let _ = self.runtime().data().spawn_ticker(every);
}

pub const USERDATA_TICK: std::time::Duration = std::time::Duration::from_secs(5);
```

`construct.rs`：在 `spawn_idle_cache_reclaimer_then(...)` 之后加一行 `s.spawn_userdata_ticker(wind_store::store::USERDATA_TICK);`。

`coordinator.rs` 的 `reload_user_config`：在 `self.sync_emoji_dict();` 之后加：

```rust
// 用户词文本手改后，保存配置 / 设置页触发热重载时一并生效（设计 §5）。
if let Some(store) = &self.store {
    let changed = store.sync_user_texts();
    if !changed.is_empty() {
        info!("用户词文本已重新载入：{} 个方案", changed.len());
    }
}
```

`self.store` 的字段名以 `coordinator.rs` 现有用法为准，7847 行一带用的是 `&self.store`。

`handle_mode.rs` 的 `switch_schema_by_id`：在 `self.finish_user_schema_switch(schema_id, "Switched to schema");` 之前加：

```rust
if let Some(store) = &self.store {
    store.sync_user_text(&self.engine_mgr.data_schema_id(schema_id));
}
```

`wind-webdata/src/lib.rs`：分派表 `"dict.import"` 一带加：

```rust
// 用户词文本手改后重新载入（设计 §5）；带 paths 只同步这几个文件，越界拒绝。
"dict.reload" => h.web_dict_reload(params),
// 显式折回全部待写回的数据（备份 / 导出 / 调试前）。
"dict.flush" => h.web_dict_flush(),
```

`trait WebDataRpc` 加默认方法：

```rust
fn web_dict_reload(&self, params: &Value) -> anyhow::Result<Value> {
    let store = self.user_store().ok_or_else(|| anyhow::anyhow!("无持久化存储"))?;
    let reloaded = match params.get("paths").and_then(|v| v.as_array()) {
        Some(arr) => {
            let paths: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            store.reload_user_text_paths(&paths)?
        }
        None => store.sync_user_texts(),
    };
    Ok(json!({ "reloaded": reloaded }))
}

fn web_dict_flush(&self) -> anyhow::Result<Value> {
    let store = self.user_store().ok_or_else(|| anyhow::anyhow!("无持久化存储"))?;
    store.flush_user_data()?;
    Ok(json!({ "ok": true }))
}
```

设置端（wind-setting 仓）暂不接这两个方法，账本记一行：它们是供设置页 / 同步组件日后调用的入口，不在本仓范围。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p wind-store -p wind-webdata -p wind-coordinator --no-fail-fast 2>&1 | grep -E "^test .*FAILED|test result" | sort | uniq -c | sort -rn | head`
Expected: 0 failed。

- [ ] **Step 5: 提交**

```bash
cargo fmt -p wind-store -p wind-coordinator -p wind-webdata
git commit -F - --only -- wind_input/crates/wind-store wind_input/crates/wind-coordinator wind_input/crates/wind-webdata <<'EOF'
feat(userdata): 手改用户词文本在启动 / 切方案 / 配置重载 / dict.reload 时生效；dict.flush 显式折回；后台节拍驱动空闲写回
EOF
```

---

### Task 6：文档与全量回归

**Files:**
- Modify: `docs/design/user-data-storage-redesign.md`（§10 P2b 行写实现状态与差异；补 P2c 行：用户分库）
- Modify: `wind_input/crates/wind-store/AGENTS.md`（用户词在文本 + 引擎、`StateLoc.text_dir`、迁移标记 `migrated-p2`、redb 只剩短语 / shadow / 覆盖表 / 小文件）
- Modify: `wind_input/crates/wind-userdata/AGENTS.md`（`spawn_ticker`、导入分类）
- Modify: `AGENTS.md`（仓库根）中提到「用户词在 redb」的段落（`grep -n "user_words\|用户词" AGENTS.md` 找）

- [ ] **Step 1: 改文档**

§10 的 P2b 行改为「**已实现**（计划 `user-data-storage-plan-p2b.md`）。与设计的差异：用户分库移到 P2c；用户词 count / created_at 不再保留；`lowercase` 未按方案下发」，并在其后加一行：

`| P2c | 用户分库：user/<schema>/*.dict.yaml 只读段（登记 / 白名单、冲突副本排除、段预算 8、删除转 Shadow） | 分库增删改后 reload 生效；冲突副本不加载 |`

- [ ] **Step 2: 全量回归**

Run: `cargo test -p wind-store -p wind-dict -p wind-engine -p wind-coordinator -p wind-webdata -p wind-transfer -p wind-userdata -p wind-config -p wind-wdict --no-fail-fast > <独立临时目录>/p2b-full.log 2>&1; grep "test result" <独立临时目录>/p2b-full.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `failed 0`。总数与 P2a 收尾的 4970 相比：新增测试为正，删除的 redb 专属测试为负，差额要能逐条对上；两项都写进账本。

- [ ] **Step 3: clippy**

Run: `cargo clippy -p wind-store -p wind-userdata -p wind-coordinator -p wind-webdata --all-targets 2>&1 | grep -E "^(warning|error)" | sort | uniq -c`
Expected: 不多于基线（`wind-dict` 的 1 条 `sort_by_key`、`migrate_p1.rs` 的 2 条 `type_complexity`）。

- [ ] **Step 4: 提交**

```bash
git commit -F - --only -- docs/design/user-data-storage-redesign.md wind_input/crates/wind-store/AGENTS.md wind_input/crates/wind-userdata/AGENTS.md AGENTS.md <<'EOF'
docs(userdata): P2b 状态与差异；用户分库另立 P2c
EOF
```
