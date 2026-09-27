# 辅助码来源 · 第一期实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 「按词查编码」统一入口（系统 + 用户词库作为整体）第一期落地，辅助码借它支持 `files = ["schema:<id>"]` 引用码表方案，并修掉「改了来源要切方案才生效」的缓存 bug。

**Architecture:** store 加按方案的写代次；wind-engine 新增用户词「词 → 全部编码」内存索引（按方案分槽、代次过期、后台单飞重建），与现有反查索引合成 `TextCodeView`；编码显示改走它。wind-aux-code 的筛选改为面向 `AuxCodeLookup` 接口；协调器把 `files` 里的文件表与方案视图按序拼成运行时查询对象，按来源清单做缓存键。

**Tech Stack:** Rust 2024（workspace `wind_input/`），redb store，serde/toml，tracing。

**Spec:** `docs/design/text-code-lookup.md`（第一期）、`docs/design/aux-code-schema-source.md`（第 0、1 期）。

**工作位置：** worktree `/home/dufeng/develop/windinput/wt-auxsrc/WindInput`，分支 `feat/aux-code-source`。
harness 每条命令后会把 cwd 重置回主仓——**每条 Bash 都要显式 `cd` 到 worktree**。

## Global Constraints

- 条目写法：以 `schema:` 开头是方案引用，其余是文件路径；键名仍是 `files`，顺序即优先级。
- 可引用方案：引擎类型 `codetable`、不是自己；不要求在 `schema.available` 里，不看 `hidden`。
- 方案来源取「系统层 + 用户层」，**不取临时词**；用户隐藏（shadow）的系统词**不扣除**。
- 用户层「只在用到时检查」：过期时本次照用旧的，另起后台线程重建（单飞），**绝不在按键线程上扫 store**。
- 按键线程不建反查索引：方案来源的系统层未就绪时，辅助码**不进入、不吞键**，并派后台构建。
- 加词去重（`handle_addword::encode_and_dedup`）**不改**，`word_codes_in` 保持「仅系统层」语义。
- 提交：`git commit -F - --only -- <自己的路径…>`；新文件先 `git add -- <该文件>`。禁用 `git add -A` / `git add .` / `git commit -a` / `git stash` / `reset --hard`。
- 格式化：只对改动文件 `rustfmt --edition 2024 <file>`，**不跑 `cargo fmt --all`**。
- 测试命令（在 `wind_input/` 下）：
  `mkdir -p /tmp/wct-auxsrc && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p <crate> <filter>`。
  TMPDIR 不要用 scratchpad（路径过长会让 socket 测试假红）。
- 注释与测试说明用中文，风格对齐所在文件（「为什么」写在注释里，不写流水账）。

## Review Focus

1. `files` 里 `schema:` 指向不存在 / 非码表 / 自己 / 空 id —— 应 warn 并跳过该条，其余来源照常可用（Task 5 测试）。
2. 方案来源的反查索引还没建好时按触发键 —— 不进入辅助码、键不被吞、`state.active` 不变（Task 6 测试）。
3. 在拼音里造词后，五笔的用户层不应判过期（否则每次上屏都后台重扫五笔用户词）（Task 1 / Task 2 测试）。
4. 备份还原（`Store::resume`）后所有方案的用户层都应判过期（Task 1 测试）。
5. 文件来源与方案来源混列时，两边的码都参与筛选、互不遮蔽（Task 6 测试）。

---

### Task 1: store 按方案的写代次

**Files:**
- Modify: `wind_input/crates/wind-store/src/store.rs`（字段 `words_gen` 附近 ~130-175；`words_generation` / `bump_words_gen` ~259-266；`resume` 里的 `bump_words_gen()` ~373）
- Modify: `wind_input/crates/wind-store/src/user_words.rs`（`bump_words_gen()` 调用点：`add_user_word`、`remove_user_word`、`update_user_word_weight`、`on_word_selected`、`clear_user_words`、`import_user_words`）
- Modify: `wind_input/crates/wind-store/src/temp_words.rs`（`learn_temp_word`、`evict_temp_words`、`import_temp_word_rows`、`clear_temp_words`、`remove_temp_word`、`promote_temp_word`）
- Modify: `wind_input/crates/wind-store/src/draft_words.rs`（`promote_draft_to_temp`）
- Test: `wind_input/crates/wind-store/tests/words_generation_of.rs`（新建）

**Interfaces:**
- Produces: `Store::words_generation_of(&self, schema: &str) -> (u64, u64)`（`(全体纪元, 该方案计数)`，任一变化即过期）；`pub(crate) fn bump_words_gen(&self, schema: &str)`；`pub(crate) fn bump_words_epoch(&self)`。
- 全局 `words_generation()` 语义不变（每次 bump 仍 +1）。

- [ ] **Step 1: 写失败测试**

```rust
//! 按方案的写代次：一个方案的写入不该让别的方案的内存索引过期（见 `Store::words_generation_of`）。
use wind_store::Store;

fn open(tag: &str) -> Store {
    let p = std::env::temp_dir().join(format!("wind_store_gen_of_{tag}_{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&p);
    Store::open(&p).unwrap()
}

/// ★ 在拼音里造词不能让五笔的索引过期——否则辅助码引用五笔时，每次上屏都要后台重扫五笔用户词。
#[test]
fn write_in_one_schema_does_not_touch_another() {
    let s = open("iso");
    let wb0 = s.words_generation_of("wb");
    let py0 = s.words_generation_of("pinyin");
    s.add_user_word("pinyin", "nihao", "你好", 0, 0).unwrap();
    assert_eq!(s.words_generation_of("wb"), wb0, "拼音写入不该推进五笔的代次");
    assert_ne!(s.words_generation_of("pinyin"), py0, "拼音自己的代次必须前进");
    s.learn_temp_word("wb", "aaaa", "工", 0, 0).unwrap();
    assert_ne!(s.words_generation_of("wb"), wb0, "临时词写入同样推进本方案代次");
}

/// 备份还原换了整个文件，哪个方案的索引都不可信。
#[test]
fn resume_invalidates_every_schema() {
    let s = open("resume");
    let wb0 = s.words_generation_of("wb");
    let py0 = s.words_generation_of("pinyin");
    s.pause().unwrap();
    s.resume().unwrap();
    assert_ne!(s.words_generation_of("wb"), wb0);
    assert_ne!(s.words_generation_of("pinyin"), py0);
}

/// 全局代次保持原语义：任何方案的结构写入都 +1（词语联想仍靠它）。
#[test]
fn global_generation_still_moves_on_any_write() {
    let s = open("global");
    let g0 = s.words_generation();
    s.add_user_word("wb", "aaaa", "工", 0, 0).unwrap();
    assert_ne!(s.words_generation(), g0);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && mkdir -p /tmp/wct-auxsrc && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-store --test words_generation_of`
Expected: 编译失败，`no method named words_generation_of`。

- [ ] **Step 3: 实现**

`store.rs` 字段（紧跟 `words_count_gen` 之后）：

```rust
    /// **按方案**的结构代次：`bump_words_gen(schema)` 同时推进全局 `words_gen` 与这里该方案的计数。
    ///
    /// 全局代次对「引用另一个方案」的索引太粗：在拼音里打字自动造词，会让五笔的用户词索引
    /// （辅助码引用五笔时要用）也判过期、后台重扫。见 `docs/design/text-code-lookup.md` §3.2。
    words_gen_by_schema: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// 不分方案的写入（`resume` 换了整个文件）推进的纪元；`words_generation_of` 把它并进返回值。
    words_epoch: std::sync::atomic::AtomicU64,
```

构造处（`words_gen: AtomicU64::new(0)` 旁）初始化 `words_gen_by_schema: Default::default(), words_epoch: AtomicU64::new(0)`。

方法：

```rust
    /// 某方案的结构代次：`(全体纪元, 该方案计数)`，任一分量变化即说明该方案的用户词 / 临时词变了。
    pub fn words_generation_of(&self, schema: &str) -> (u64, u64) {
        let epoch = self.words_epoch.load(std::sync::atomic::Ordering::Acquire);
        let n = self
            .words_gen_by_schema
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(schema)
            .copied()
            .unwrap_or(0);
        (epoch, n)
    }

    pub(crate) fn bump_words_gen(&self, schema: &str) {
        self.words_gen
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        *self
            .words_gen_by_schema
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(schema.to_string())
            .or_default() += 1;
    }

    /// 不分方案的结构变化（整库还原）：全局与纪元一起推进。
    pub(crate) fn bump_words_epoch(&self) {
        self.words_gen
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.words_epoch
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
```

`resume` 里 `self.bump_words_gen();` → `self.bump_words_epoch();`。
其余每个调用点 `self.bump_words_gen();` → `self.bump_words_gen(schema);`（这些函数的形参都叫 `schema`）。
改完 `grep -rn "bump_words_gen()" crates/wind-store/src` 必须无结果。字段文档 `words_gen` 那段补一句「按方案的见 `words_gen_by_schema`」。

- [ ] **Step 4: 跑测试确认通过，并跑整个 crate**

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-store`
Expected: 全部 PASS。

- [ ] **Step 5: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-store/src/store.rs crates/wind-store/src/user_words.rs crates/wind-store/src/temp_words.rs crates/wind-store/src/draft_words.rs crates/wind-store/tests/words_generation_of.rs
git add -- crates/wind-store/tests/words_generation_of.rs
git commit -F - --only -- crates/wind-store/src/store.rs crates/wind-store/src/user_words.rs crates/wind-store/src/temp_words.rs crates/wind-store/src/draft_words.rs crates/wind-store/tests/words_generation_of.rs <<'EOF'
feat(store): 用户词 / 临时词的写代次按方案计 —— 一个方案的写入不再让别的方案的索引过期

按词查编码的用户层要按方案缓存（辅助码会引用五笔，而用户在拼音里打字）。只有全局代次时，
拼音里的每次自动造词都会让五笔那份判过期、后台重扫。全局代次语义不变，整库还原推进纪元。
EOF
```

---

### Task 2: 用户层索引 `UserTextIndex`（词 → 全部编码，按方案分槽）

**Files:**
- Create: `wind_input/crates/wind-engine/src/text_codes.rs`
- Modify: `wind_input/crates/wind-engine/src/lib.rs`（`pub mod text_codes;`）

**Interfaces:**
- Consumes: `Store::words_generation_of(&str) -> (u64, u64)`（Task 1）；`Store::for_each_user_word(schema, prefix, &mut |w| -> bool)`（现有，`w.text` / `w.code` 为 `&str`）。
- Produces:
  - `pub struct UserTextIndex` + `pub fn codes_of(&self, text: &str) -> impl Iterator<Item = &str>`（码长升序、同长按字典序、去重）；
  - `pub(crate) type SharedSlots = Arc<Mutex<UserTextSlots>>`；
  - `pub(crate) fn get_or_refresh(slots: &SharedSlots, store: &Arc<wind_store::Store>, data_schema: &str) -> Option<Arc<UserTextIndex>>`；
  - `pub(crate) fn prewarm(slots: &SharedSlots, store: &wind_store::Store, data_schema: &str) -> bool`。

- [ ] **Step 1: 写模块骨架与失败测试**

`text_codes.rs` 先只写模块文档、类型签名（函数体 `todo!()`）与下面的测试，确认测试编译并失败：

```rust
//! 「按词查编码」的**用户层**：某方案用户词的「词 → 全部编码」内存索引。
//!
//! 系统层是反查索引（`wind_dict::ReverseIndex`）。用户词在 store 里按 `schema\0code\0text`
//! 排序，按词查只能扫全表（可达十九万条），故在内存里按词排一份。形态照搬词语联想的
//! `user_assoc`：代次过期、过期时照用旧的、后台单飞重建，绝不在按键线程上扫表。
//!
//! 与 `user_assoc` 的区别：收**全部长度**（单字也要）、留**全部编码**、**按方案分槽**。
//! 设计见 `docs/design/text-code-lookup.md` §3.2。临时词第一期不收（没有使用方要）。

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect()
    }

    #[test]
    fn groups_codes_by_text_sorted_by_length() {
        let idx = UserTextIndex::from_rows(
            "wb",
            (0, 0),
            rows(&[("工", "aaaa"), ("我", "q"), ("工", "a"), ("工", "aaaa"), ("工程", "aakg")]),
        );
        assert_eq!(idx.codes_of("工").collect::<Vec<_>>(), vec!["a", "aaaa"], "码长升序、去重");
        assert_eq!(idx.codes_of("我").collect::<Vec<_>>(), vec!["q"]);
        assert_eq!(idx.codes_of("工程").collect::<Vec<_>>(), vec!["aakg"], "词组也收");
        assert_eq!(idx.codes_of("无").count(), 0);
        assert_eq!(idx.codes_of("").count(), 0);
    }

    fn tmp_store(tag: &str) -> std::sync::Arc<wind_store::Store> {
        let p = std::env::temp_dir()
            .join(format!("wind_text_codes_{tag}_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        std::sync::Arc::new(wind_store::Store::open(&p).unwrap())
    }

    #[test]
    fn prewarm_then_get_reads_user_words_of_that_schema_only() {
        let s = tmp_store("read");
        s.add_user_word("wb", "zzzz", "嗨", 0, 0).unwrap();
        s.add_user_word("pinyin", "hai", "嗨", 0, 0).unwrap();
        let slots = SharedSlots::default();
        assert!(prewarm(&slots, &s, "wb"));
        let idx = get_or_refresh(&slots, &s, "wb").expect("预热后就绪");
        assert_eq!(idx.codes_of("嗨").collect::<Vec<_>>(), vec!["zzzz"], "不串方案");
        assert!(!prewarm(&slots, &s, "wb"), "已是最新则不重建");
    }

    /// ★ 别的方案写入不让本方案过期（依赖 store 的按方案代次）。
    #[test]
    fn other_schema_write_keeps_index_fresh() {
        let s = tmp_store("fresh");
        let slots = SharedSlots::default();
        prewarm(&slots, &s, "wb");
        s.add_user_word("pinyin", "nihao", "你好", 0, 0).unwrap();
        assert!(!prewarm(&slots, &s, "wb"), "拼音写入后五笔的索引仍是最新");
        s.add_user_word("wb", "wqvb", "你好", 0, 0).unwrap();
        assert!(prewarm(&slots, &s, "wb"), "本方案写入后必须重建");
    }

    #[test]
    fn get_or_refresh_returns_none_before_first_build() {
        let s = tmp_store("cold");
        let slots = SharedSlots::default();
        // 冷启动：本次拿不到（后台去建），调用方按「这一层没就绪」处理。
        assert!(get_or_refresh(&slots, &s, "wb").is_none());
    }

    #[test]
    fn slots_are_capped() {
        let s = tmp_store("cap");
        let slots = SharedSlots::default();
        for id in ["a", "b", "c", "d", "e", "f"] {
            prewarm(&slots, &s, id);
        }
        assert!(slots.lock().unwrap().map.len() <= MAX_SLOTS);
    }
}
```

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --lib text_codes`
Expected: FAIL（`todo!()` panic）。

- [ ] **Step 2: 实现**

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 同时保留的方案份数上限。在用的方案通常是：主码表、联想方案、辅助码引用的方案，
/// 取 4 留一格余量；超出时淘汰最久未用且不在重建中的那份。
const MAX_SLOTS: usize = 4;

/// 某方案用户词的「词 → 全部编码」紧凑表：一整块 `buf`（词 + 以 `\t` 连接的编码）+ 定长条目。
#[derive(Default)]
pub struct UserTextIndex {
    data_schema: String,
    /// 建表**之前**读到的代次。先读后扫：扫描期间有写入，代次已前进，下次必判过期。
    generation: (u64, u64),
    buf: String,
    /// 按词字节序升序、词唯一。
    entries: Vec<Entry>,
}

struct Entry {
    off: u32,
    text_len: u32,
    codes_len: u32,
}

impl UserTextIndex {
    /// `rows` 为 (词, 码)，可重复、可乱序。
    pub fn from_rows(data_schema: &str, generation: (u64, u64), mut rows: Vec<(String, String)>) -> Self {
        rows.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.len().cmp(&b.1.len()))
                .then(a.1.cmp(&b.1))
        });
        rows.dedup();
        let mut idx = UserTextIndex {
            data_schema: data_schema.to_string(),
            generation,
            ..Default::default()
        };
        let mut i = 0;
        while i < rows.len() {
            let text = rows[i].0.as_str();
            let off = idx.buf.len();
            idx.buf.push_str(text);
            let codes_start = idx.buf.len();
            let mut first = true;
            while i < rows.len() && rows[i].0 == text {
                if !first {
                    idx.buf.push('\t');
                }
                idx.buf.push_str(&rows[i].1);
                first = false;
                i += 1;
            }
            idx.entries.push(Entry {
                off: off as u32,
                text_len: (codes_start - off) as u32,
                codes_len: (idx.buf.len() - codes_start) as u32,
            });
        }
        idx
    }

    /// 全量扫该方案的用户词建表（**会扫整张用户词表**，只在后台线程 / 预热 / 测试里调）。
    pub fn build(store: &wind_store::Store, data_schema: &str) -> Self {
        let generation = store.words_generation_of(data_schema);
        let mut rows = Vec::new();
        if let Err(e) = store.for_each_user_word(data_schema, "", &mut |w| {
            rows.push((w.text.to_string(), w.code.to_string()));
            true
        }) {
            tracing::warn!("按词查编码用户层：读 store 失败 schema={data_schema}: {e}");
        }
        Self::from_rows(data_schema, generation, rows)
    }

    fn text_of(&self, e: &Entry) -> &str {
        &self.buf[e.off as usize..(e.off + e.text_len) as usize]
    }

    /// 该词的全部用户编码（码长升序）；不在表里返回空迭代。
    pub fn codes_of(&self, text: &str) -> impl Iterator<Item = &str> {
        let i = self.entries.partition_point(|e| self.text_of(e) < text);
        let codes = self
            .entries
            .get(i)
            .filter(|e| !text.is_empty() && self.text_of(e) == text)
            .map(|e| {
                let s = (e.off + e.text_len) as usize;
                &self.buf[s..s + e.codes_len as usize]
            });
        codes.into_iter().flat_map(|s| s.split('\t'))
    }

    fn is_stale(&self, store: &wind_store::Store) -> bool {
        self.generation != store.words_generation_of(&self.data_schema)
    }
}

#[derive(Default)]
struct Slot {
    index: Option<Arc<UserTextIndex>>,
    building: bool,
    last_used: u64,
}

#[derive(Default)]
pub(crate) struct UserTextSlots {
    map: HashMap<String, Slot>,
    tick: u64,
}

pub(crate) type SharedSlots = Arc<Mutex<UserTextSlots>>;

impl UserTextSlots {
    /// 为 `key` 腾位：新方案进来且已满时，淘汰最久未用、且不在重建中的一份。
    fn make_room_for(&mut self, key: &str) {
        if self.map.contains_key(key) || self.map.len() < MAX_SLOTS {
            return;
        }
        if let Some(victim) = self
            .map
            .iter()
            .filter(|(_, s)| !s.building)
            .min_by_key(|(_, s)| s.last_used)
            .map(|(k, _)| k.clone())
        {
            self.map.remove(&victim);
        }
    }

    fn touch(&mut self, key: &str) -> &mut Slot {
        self.make_room_for(key);
        self.tick += 1;
        let tick = self.tick;
        let slot = self.map.entry(key.to_string()).or_default();
        slot.last_used = tick;
        slot
    }
}

/// 取可用索引（可能略旧）；缺失或过期时起一次后台重建（已有在建则不重复起）。
pub(crate) fn get_or_refresh(
    slots: &SharedSlots,
    store: &Arc<wind_store::Store>,
    data_schema: &str,
) -> Option<Arc<UserTextIndex>> {
    let mut g = slots.lock().unwrap_or_else(|e| e.into_inner());
    let slot = g.touch(data_schema);
    let current = slot.index.clone();
    let stale = current.as_ref().is_none_or(|i| i.is_stale(store));
    if stale && !slot.building {
        slot.building = true;
        let (slots2, store2, key) = (slots.clone(), store.clone(), data_schema.to_string());
        let spawned = std::thread::Builder::new()
            .name("user-text-index".into())
            .spawn(move || {
                // 建表中途 panic 也要复位 `building`，否则单飞标记卡死、此后永不重建。
                let _reset = BuildingGuard(slots2.clone(), key.clone());
                let idx = Arc::new(UserTextIndex::build(&store2, &key));
                if let Some(s) = slots2.lock().unwrap_or_else(|e| e.into_inner()).map.get_mut(&key) {
                    s.index = Some(idx);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("按词查编码用户层：起重建线程失败: {e}");
            if let Some(s) = g.map.get_mut(data_schema) {
                s.building = false;
            }
        }
    }
    current
}

struct BuildingGuard(SharedSlots, String);

impl Drop for BuildingGuard {
    fn drop(&mut self) {
        if let Some(s) = self.0.lock().unwrap_or_else(|e| e.into_inner()).map.get_mut(&self.1) {
            s.building = false;
        }
    }
}

/// 阻塞地建好并放进槽（预热 / 测试用）。已是最新则不重建，返回是否真的建了。
pub(crate) fn prewarm(slots: &SharedSlots, store: &wind_store::Store, data_schema: &str) -> bool {
    {
        let mut g = slots.lock().unwrap_or_else(|e| e.into_inner());
        if g.touch(data_schema)
            .index
            .as_ref()
            .is_some_and(|i| !i.is_stale(store))
        {
            return false;
        }
    }
    let idx = Arc::new(UserTextIndex::build(store, data_schema));
    slots.lock().unwrap_or_else(|e| e.into_inner()).touch(data_schema).index = Some(idx);
    true
}
```

`lib.rs` 在 `pub mod user_assoc;` 旁加 `pub mod text_codes;`。

- [ ] **Step 3: 跑测试确认通过**

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --lib text_codes`
Expected: 5 passed。

- [ ] **Step 4: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-engine/src/text_codes.rs crates/wind-engine/src/lib.rs
git add -- crates/wind-engine/src/text_codes.rs
git commit -F - --only -- crates/wind-engine/src/text_codes.rs crates/wind-engine/src/lib.rs <<'EOF'
feat(engine): 按词查编码的用户层 —— 用户词「词 → 全部编码」索引，按方案分槽、代次过期后台重建

形态照搬词语联想的 user_assoc（按键线程只读、过期照用旧的、单飞重建），区别是收单字、
留全部编码、按方案分槽。第一期不收临时词。见 docs/design/text-code-lookup.md §3.2。
EOF
```

---

### Task 3: `TextCodeView` 与编码显示接入（系统层 + 用户层）

**Files:**
- Modify: `wind_input/crates/wind-engine/src/text_codes.rs`（加 `TextCodeView`）
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（字段 `user_text` ~488 / 初始化 ~763；新方法放在 `word_codes_in` ~1165 附近；改 `codetable_reverse_hint` ~1071；改 `reverse_index_for` 的 `retain` ~1310）
- Modify: `wind_input/crates/wind-engine/src/lib.rs`（`pub use text_codes::TextCodeView;`）
- Modify: `wind_input/crates/wind-coordinator/src/comment.rs:679,836`、`wind_input/crates/wind-coordinator/src/coordinator.rs:6157`（`word_codes_in` → `word_codes_display`）
- Test: `wind_input/crates/wind-engine/tests/text_codes.rs`（新建）

**Interfaces:**
- Consumes: Task 2 的 `get_or_refresh` / `prewarm` / `UserTextIndex::codes_of`。
- Produces:
  - `pub struct TextCodeView`（`Default`、`Clone`）：`pub fn system_ready(&self) -> bool`、`pub fn has_any(&self) -> bool`、`pub fn any_code(&self, text: &str, pred: &mut dyn FnMut(&str) -> bool) -> bool`、`pub fn codes_of(&self, text: &str) -> Vec<&str>`；
  - `EngineManager::text_codes(&self, schema_id: &str) -> TextCodeView`（不阻塞）；
  - `EngineManager::prewarm_text_codes(&self, schema_id: &str) -> bool`（阻塞，预热 / 测试）；
  - `EngineManager::word_codes_display(&self, schema_id: &str, text: &str) -> Option<String>`（三态同 `word_codes_in`，`/` 连接）；
  - `fn reverse_index_pins(&self) -> Vec<String>`（本任务只含 `assoc_word_schema()`，Task 6 追加辅助码来源）。

- [ ] **Step 1: 写失败测试（集成，自造码表方案）**

`crates/wind-engine/tests/text_codes.rs`：

```rust
//! 按词查编码统一入口的接线验证：系统层（反查索引）+ 用户层（用户词），见
//! `docs/design/text-code-lookup.md`。夹具自造一个小码表方案，不依赖 build_dev/data。

use std::path::PathBuf;
use std::sync::Arc;
use wind_config::Config;
use wind_engine::EngineManager;

/// 造一个码表方案 `wbx`：工=a/aaaa、攻=atyy、公=wcu。
pub fn write_wbx(schemas: &std::path::Path) {
    std::fs::create_dir_all(schemas.join("wbx")).unwrap();
    std::fs::write(
        schemas.join("wbx.schema.toml"),
        "[schema]\nid = \"wbx\"\nname = \"测五\"\n[engine]\ntype = \"codetable\"\n\
         [engine.codetable]\nmax_code_length = 4\n\
         [[dictionaries]]\nid = \"wbx_main\"\npath = \"wbx/wbx.dict.yaml\"\n\
         type = \"rime_codetable\"\ndefault = true\n",
    )
    .unwrap();
    std::fs::write(
        schemas.join("wbx/wbx.dict.yaml"),
        "---\nname: wbx\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n\
         a\t工\t100\naaaa\t工\t90\natyy\t攻\t80\nwcu\t公\t80\n",
    )
    .unwrap();
}

fn setup(tag: &str) -> (EngineManager, Arc<wind_store::Store>) {
    let dir = std::env::temp_dir().join(format!("wind_text_codes_it_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_wbx(&dir.join("schemas"));
    let sp = dir.join("store.redb");
    let store = Arc::new(wind_store::Store::open(&sp).unwrap());
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wbx".into()];
    cfg.schema.active = "wbx".into();
    let m = EngineManager::with_store_override(&cfg, Some(&dir), Some(store.clone()), None::<PathBuf>);
    (m, store)
}

#[test]
fn display_is_none_until_system_layer_ready() {
    let (m, _) = setup("ready");
    assert_eq!(m.word_codes_display("wbx", "工"), None, "没就绪 ≠ 查不到");
    assert!(m.prewarm_text_codes("wbx"));
    assert_eq!(m.word_codes_display("wbx", "工").as_deref(), Some("a/aaaa"));
    assert_eq!(m.word_codes_display("wbx", "无").as_deref(), Some(""));
}

/// ★ 用户在该方案里加的编码进入显示；而加词去重用的 `word_codes_in` 仍只看系统层。
#[test]
fn user_codes_join_display_but_not_word_codes_in() {
    let (m, store) = setup("user");
    store.add_user_word("wbx", "gggg", "工", 0, 0).unwrap();
    store.add_user_word("wbx", "zzzz", "嗨", 0, 0).unwrap();
    m.prewarm_text_codes("wbx");
    assert_eq!(m.word_codes_display("wbx", "工").as_deref(), Some("a/aaaa/gggg"));
    assert_eq!(m.word_codes_display("wbx", "嗨").as_deref(), Some("zzzz"), "系统没有的字也能查到");
    assert_eq!(m.word_codes_in("wbx", "工").as_deref(), Some("a/aaaa"), "去重口径不变");
}

#[test]
fn view_any_code_walks_both_layers() {
    let (m, store) = setup("any");
    store.add_user_word("wbx", "zzzz", "工", 0, 0).unwrap();
    m.prewarm_text_codes("wbx");
    let v = m.text_codes("wbx");
    assert!(v.any_code("工", &mut |c| c.starts_with('a')), "系统层");
    assert!(v.any_code("工", &mut |c| c.starts_with('z')), "用户层");
    assert!(!v.any_code("工", &mut |c| c.starts_with('q')));
}
```

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --test text_codes`
Expected: 编译失败（`word_codes_display` / `prewarm_text_codes` / `text_codes` 不存在）。

- [ ] **Step 2: 实现 `TextCodeView`（text_codes.rs 追加）**

```rust
/// 「按词查编码」的一次快照：系统层（反查索引）+ 用户层。某层 `None` = 这一层还没就绪，
/// **不是**「查不到」（沿用 `EngineManager::word_codes_in` 的三态约定）。
#[derive(Default, Clone)]
pub struct TextCodeView {
    pub(crate) system: Option<Arc<wind_dict::ReverseIndex>>,
    pub(crate) user: Option<Arc<UserTextIndex>>,
}

impl TextCodeView {
    pub fn system_ready(&self) -> bool {
        self.system.is_some()
    }

    /// 至少有一层可查。
    pub fn has_any(&self) -> bool {
        self.system.is_some() || self.user.is_some()
    }

    /// 按「系统层 → 用户层」依次把该词的码交给 `pred`，任一返回 true 即停并返回 true。零分配。
    pub fn any_code(&self, text: &str, pred: &mut dyn FnMut(&str) -> bool) -> bool {
        if let Some(sys) = &self.system
            && let Some(list) = sys.codes_of(text)
            && list.iter().any(&mut *pred)
        {
            return true;
        }
        self.user
            .as_ref()
            .is_some_and(|u| u.codes_of(text).any(&mut *pred))
    }

    /// 该词全部编码：系统层在前、用户层补不重复者，整体按码长稳定排序（与反查索引「码长升序」同口径）。
    pub fn codes_of(&self, text: &str) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .system
            .as_ref()
            .and_then(|s| s.codes_of(text))
            .map(|l| l.iter().collect())
            .unwrap_or_default();
        if let Some(u) = &self.user {
            for c in u.codes_of(text) {
                if !v.contains(&c) {
                    v.push(c);
                }
            }
        }
        v.sort_by_key(|c| c.len());
        v
    }
}
```

（`ReverseIndex` 的实际路径以 `crates/wind-engine/src/manager.rs` 里 `reverse_index` 字段的类型为准；`CodeList::iter(self)` 按值取，`l.iter()` 可直接用。）

- [ ] **Step 3: 实现 `EngineManager` 方法**

字段（`user_assoc` 旁）：

```rust
    /// 按词查编码的用户层，按方案分槽。见 [`crate::text_codes`]。
    user_text: crate::text_codes::SharedSlots,
```

初始化 `user_text: Default::default(),`。方法放在 `word_codes_in` 之后：

```rust
    /// 「按词查编码」统一入口：系统层 + 用户层的一次快照，**不阻塞**。
    ///
    /// 用户层过期或缺失时，本次照用旧的（或没有），另起后台重建；系统层没就绪就是 `None`，
    /// 由调用方决定要不要派后台构建（打字链路用 `Coordinator::spawn_index_warm`）。
    /// 设计见 `docs/design/text-code-lookup.md`。
    pub fn text_codes(&self, schema_id: &str) -> crate::text_codes::TextCodeView {
        if schema_id.is_empty() {
            return Default::default();
        }
        let system = self.reverse_index_if_ready(schema_id);
        let user = self.store.as_ref().and_then(|s| {
            crate::text_codes::get_or_refresh(&self.user_text, s, &self.data_schema_id(schema_id))
        });
        crate::text_codes::TextCodeView { system, user }
    }

    /// 阻塞地把两层都建好（预热线程 / 测试用，**不可进按键链路**）。返回是否真的建了东西。
    pub fn prewarm_text_codes(&self, schema_id: &str) -> bool {
        if schema_id.is_empty() {
            return false;
        }
        let built_sys = self.prewarm_reverse_index(schema_id);
        let built_user = self.store.as_ref().is_some_and(|s| {
            crate::text_codes::prewarm(&self.user_text, s, &self.data_schema_id(schema_id))
        });
        built_sys || built_user
    }

    /// 展示用的「这个词怎么打」：系统层 + 用户层，`/` 连接、码长升序。
    ///
    /// 三态同 [`Self::word_codes_in`]：`None` = 系统层还没就绪。与它的区别只在**含用户层**——
    /// 加词去重要的是「系统词库里有没有」，那条口径刻意不变，仍用 `word_codes_in`。
    pub fn word_codes_display(&self, schema_id: &str, text: &str) -> Option<String> {
        if schema_id.is_empty() {
            return Some(String::new());
        }
        let v = self.text_codes(schema_id);
        if !v.system_ready() {
            return None;
        }
        Some(v.codes_of(text).join("/"))
    }
```

`codetable_reverse_hint` 的尾部改为走视图（口径仍是「最长码」）：

```rust
        let v = self.text_codes(&primary);
        if !v.system_ready() {
            return None;
        }
        Some(v.codes_of(text).last().map(|s| s.to_string()).unwrap_or_default())
```

`reverse_index_for` 的保留策略：在取 `guard` **之前**算 `let pins = self.reverse_index_pins();`，
把 `guard.retain(|k, _| k == schema_id || k == &primary);` 改为
`guard.retain(|k, _| reverse_index_keeps(k, schema_id, &primary, &pins));`，并新增：

```rust
/// 反查索引的保留判据：本次请求的、主码表、以及当前在用集合里的（联想方案、辅助码引用的方案）。
///
/// 原先只留「本次 + 主码表」两份：辅助码引用五笔、联想用拼音时，三者会互相顶掉、反复秒级重建。
fn reverse_index_keeps(k: &str, requested: &str, primary: &str, pins: &[String]) -> bool {
    k == requested || k == primary || pins.iter().any(|p| p == k)
}
```

```rust
    /// 反查索引「在用集合」里除主码表外的方案。Task 6 会追加辅助码引用的方案。
    fn reverse_index_pins(&self) -> Vec<String> {
        vec![self.assoc_word_schema()]
    }
```

在 manager.rs 的 `#[cfg(test)] mod tests` 里补纯函数测试：

```rust
    #[test]
    fn reverse_index_keeps_pinned_schemas() {
        let pins = vec!["pinyin".to_string(), "wbx".to_string()];
        assert!(reverse_index_keeps("wbx", "stroke", "wubi86", &pins), "在用集合里的不淘汰");
        assert!(reverse_index_keeps("wubi86", "stroke", "wubi86", &pins), "主码表不淘汰");
        assert!(reverse_index_keeps("stroke", "stroke", "wubi86", &pins), "本次请求的不淘汰");
        assert!(!reverse_index_keeps("old", "stroke", "wubi86", &pins));
    }
```

`lib.rs` 加 `pub use text_codes::TextCodeView;`。

- [ ] **Step 4: 编码显示改走统一入口**

三处把 `word_codes_in(` 换成 `word_codes_display(`：`comment.rs` 的 `code_rev_all | code_all` 两个臂（~679、~836），`coordinator.rs` 悬停 [编码] 段（~6157）。相邻注释里提到 `word_codes_in` 的句子同步改名，并在悬停那段补一句「含用户层：自己造的词也显示编码（text-code-lookup.md）」。
`handle_addword.rs:324` **不改**。

- [ ] **Step 5: 跑测试**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --test text_codes
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --lib reverse_index_keeps
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine -p wind-coordinator
```
Expected: 全部 PASS。

- [ ] **Step 6: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-engine/src/text_codes.rs crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-engine/tests/text_codes.rs crates/wind-coordinator/src/comment.rs crates/wind-coordinator/src/coordinator.rs
git add -- crates/wind-engine/tests/text_codes.rs
git commit -F - --only -- crates/wind-engine/src/text_codes.rs crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-engine/tests/text_codes.rs crates/wind-coordinator/src/comment.rs crates/wind-coordinator/src/coordinator.rs <<'EOF'
feat(engine): 按词查编码统一入口 TextCodeView —— 编码显示含用户层，自己造的词也显示编码

系统层沿用反查索引，用户层取 Task 2 的用户词索引。候选注释 ${code}/${code_all} 与悬停
[编码] 段改走 word_codes_display；加词去重仍用只看系统层的 word_codes_in（口径刻意不变）。
反查索引的保留策略由「本次 + 主码表」两份改为再加在用集合，免得多个方案互相顶掉。
EOF
```

---

### Task 4: wind-aux-code 面向 `AuxCodeLookup` 接口

**Files:**
- Create: `wind_input/crates/wind-aux-code/src/lookup.rs`
- Modify: `wind_input/crates/wind-aux-code/src/lib.rs`（`pub mod lookup; pub use lookup::AuxCodeLookup; pub use loader::read_name;`）
- Modify: `wind_input/crates/wind-aux-code/src/filter.rs`（`phrase_matches_per_char_prefix` / `aux_code_matches` / `filter_by_aux_code` 的 `table: &AuxCodeTable` → `table: &dyn AuxCodeLookup`）
- Modify: `wind_input/crates/wind-aux-code/src/session.rs:71`（`apply` 同上）
- Modify: `wind_input/crates/wind-aux-code/src/loader.rs`（加 `read_name`）

**Interfaces:**
- Produces:
  - `pub trait AuxCodeLookup { fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool; fn is_empty(&self) -> bool; fn any_code_starts_with(&self, ch: char, prefix: &str) -> bool {…} fn any_code_starts_with_char(&self, ch: char, c: char) -> bool {…} }`，`AuxCodeTable` 实现它；
  - `AuxCodeSession::apply(&mut self, table: &dyn AuxCodeLookup, options: &AuxCodeFilterOptions) -> Vec<Candidate>`；
  - `pub fn read_name(path: &std::path::Path) -> Option<String>`（只读首行 `# name:`）。

- [ ] **Step 1: 写失败测试（lookup.rs 内）**

```rust
//! 「这个字有哪些辅助码」的查询接口。筛选只通过它取码，不再假定背后是一张静态表：
//! 来源可以是码表文件，也可以是一个输入方案（系统词库 + 用户词库，见协调器的
//! `aux_code_source`）。设计见 `docs/design/aux-code-schema-source.md` §4。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuxCodeFilterOptions, AuxCodeTable, filter_by_aux_code};
    use wind_candidate::Candidate;

    /// 两张表按序拼接的最小实现，证明筛选对「多层来源」成立。
    struct Pair(AuxCodeTable, AuxCodeTable);
    impl AuxCodeLookup for Pair {
        fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
            self.0.any_code(ch, pred) || self.1.any_code(ch, pred)
        }
        fn is_empty(&self) -> bool {
            AuxCodeLookup::is_empty(&self.0) && AuxCodeLookup::is_empty(&self.1)
        }
    }

    fn cand(t: &str) -> Candidate {
        Candidate { text: t.into(), ..Default::default() }
    }

    #[test]
    fn filter_sees_codes_from_every_layer() {
        let lk = Pair(
            AuxCodeTable::from_rows(vec![('李', "mz")]),
            AuxCodeTable::from_rows(vec![('河', "sk")]),
        );
        let out = filter_by_aux_code(
            vec![cand("李"), cand("河"), cand("樱")],
            &lk,
            "s",
            &AuxCodeFilterOptions::default(),
        );
        let kept: Vec<&str> = out.kept.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(kept, vec!["河"], "第二层的码同样参与筛选");
    }

    #[test]
    fn table_impl_matches_inherent_methods() {
        let t = AuxCodeTable::from_rows(vec![('厑', "ib"), ('厑', "ii")]);
        let lk: &dyn AuxCodeLookup = &t;
        assert!(lk.any_code_starts_with('厑', "ib"));
        assert!(lk.any_code_starts_with_char('厑', 'i'));
        assert!(!lk.any_code_starts_with('厑', "x"));
        assert!(!lk.is_empty());
    }
}
```

`loader.rs` 测试模块追加：

```rust
    #[test]
    fn read_name_reads_only_first_line() {
        let dir = std::env::temp_dir().join(format!("wind-aux-read-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        std::fs::write(&a, "# name: 小鹤\n# version: 1\n阿=ek\n").unwrap();
        let b = dir.join("b.txt");
        std::fs::write(&b, "阿=ek\n# name: 不算\n").unwrap();
        assert_eq!(read_name(&a).as_deref(), Some("小鹤"));
        assert_eq!(read_name(&b), None);
        assert_eq!(read_name(&dir.join("missing.txt")), None);
    }
```

（`AuxCodeTable::from_rows` 的实际签名以 `table.rs:97` 为准：`Vec<(char, &str)>`。`Candidate` 若没有 `Default`，照 `session.rs` 测试里的构造方式写。）

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-aux-code`
Expected: 编译失败（`AuxCodeLookup` / `read_name` 未定义）。

- [ ] **Step 2: 实现**

`lookup.rs`（放在测试模块之前）：

```rust
use crate::table::AuxCodeTable;

pub trait AuxCodeLookup {
    /// 按优先级把 `ch` 的每个辅助码交给 `pred`，任一返回 true 即停并返回 true；无码返回 false。
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool;

    /// 一个字都没有（未挂载）。筛选据此走「不过滤、原样放行」的防御语义。
    fn is_empty(&self) -> bool;

    /// 任一码以 `prefix` 开头（单字筛选）。
    fn any_code_starts_with(&self, ch: char, prefix: &str) -> bool {
        self.any_code(ch, &mut |c| c.starts_with(prefix))
    }

    /// 任一码以单字符 `c` 开头（词组逐字首码，零分配）。
    fn any_code_starts_with_char(&self, ch: char, c: char) -> bool {
        self.any_code(ch, &mut |code| code.starts_with(c))
    }
}

impl AuxCodeLookup for AuxCodeTable {
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
        self.view_codes(ch).is_some_and(|v| v.iter().any(|c| pred(c)))
    }

    fn is_empty(&self) -> bool {
        AuxCodeTable::is_empty(self)
    }
}
```

（`view_codes` 若是私有，改为 `pub(crate)`。）

`filter.rs` / `session.rs`：把参数类型改为 `&dyn AuxCodeLookup`，`use crate::lookup::AuxCodeLookup;`。函数体里的 `table.any_code_starts_with(...)` / `table.is_empty()` 调用不变（走 trait 方法）。文档里「参数 `table` 建议使用 `AuxCodeTable::merge` 预构建」一句改为「任何 `AuxCodeLookup` 实现均可（文件表、方案视图、按序拼接的多来源）」。

`loader.rs`：

```rust
/// 只读首行取码表名（`# name: 小鹤`）。设置页列可选码表时用：为了一个名字整张读入四万行不值。
pub fn read_name(path: &std::path::Path) -> Option<String> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(f).read_line(&mut line).ok()?;
    parse_name_from_first_line(line.trim_start_matches('\u{feff}').trim_end())
}
```

- [ ] **Step 3: 跑测试（本 crate + 下游编译）**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-aux-code
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc cargo check -p wind-coordinator --tests
```
Expected: 全部 PASS；协调器照常编译（`&AuxCodeTable` 自动转 `&dyn`）。

- [ ] **Step 4: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-aux-code/src/lookup.rs crates/wind-aux-code/src/lib.rs crates/wind-aux-code/src/filter.rs crates/wind-aux-code/src/session.rs crates/wind-aux-code/src/loader.rs
git add -- crates/wind-aux-code/src/lookup.rs
git commit -F - --only -- crates/wind-aux-code/src/lookup.rs crates/wind-aux-code/src/lib.rs crates/wind-aux-code/src/filter.rs crates/wind-aux-code/src/session.rs crates/wind-aux-code/src/loader.rs crates/wind-aux-code/src/table.rs <<'EOF'
refactor(aux-code): 筛选面向 AuxCodeLookup 接口，不再假定背后是一张静态表

为「引用一个输入方案作辅助码」铺路：那种来源的码来自系统词库 + 用户词库、会随用户造词
变化，不能预先合并成一张表。AuxCodeTable 实现该接口，现有行为不变。另加 read_name，
设置页列码表名时只读首行。
EOF
```

---

### Task 5: 配置层 —— `files` 里的 `schema:` 条目与 `AuxSource`

**Files:**
- Modify: `wind_input/crates/wind-config/src/schema.rs:289-330`（`AuxCodeSpec` 文档、新常量）
- Modify: `wind_input/crates/wind-engine/src/manager.rs:305-312`（`AuxCodeSettings`）、`~4188-4230`（`aux_code_settings_of`）、Task 3 的 `reverse_index_pins`
- Modify: `wind_input/crates/wind-engine/src/lib.rs`（导出 `AuxSource`）
- Test: `wind_input/crates/wind-engine/tests/aux_code_sources.rs`（新建）

**Interfaces:**
- Produces:
  - `wind_config::schema::AUX_SCHEMA_SOURCE_PREFIX: &str = "schema:"`；
  - `pub enum AuxSource { File(std::path::PathBuf), Schema(String) }`（`Debug, Clone, PartialEq, Eq`）；
  - `AuxCodeSettings { enabled, max_phrase_len, sources: Vec<AuxSource> }`（**替换**原 `files` 字段）+ `pub fn schema_sources(&self) -> impl Iterator<Item = &str>`；
  - `reverse_index_pins` 追加 `self.aux_code_settings().schema_sources()`。

- [ ] **Step 1: 写失败测试**

`crates/wind-engine/tests/aux_code_sources.rs`：

```rust
//! `[engine.aux_code].files` 的两种条目：文件路径与 `schema:<id>` 方案引用。

use std::path::PathBuf;
use wind_config::Config;
use wind_engine::{AuxSource, EngineManager};

fn setup(tag: &str, files: &str) -> (EngineManager, PathBuf) {
    let dir = std::env::temp_dir().join(format!("wind_aux_src_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
    std::fs::write(schemas.join("aux_code/t.txt"), "李=mz\n").unwrap();
    std::fs::write(
        schemas.join("pinyin.schema.toml"),
        format!(
            "[schema]\nid = \"pinyin\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n\
             [engine.aux_code]\nfiles = {files}\nenabled = true\n"
        ),
    )
    .unwrap();
    std::fs::write(
        schemas.join("wbx.schema.toml"),
        "[schema]\nid = \"wbx\"\nname = \"测五\"\n[engine]\ntype = \"codetable\"\n",
    )
    .unwrap();
    std::fs::write(
        schemas.join("en.schema.toml"),
        "[schema]\nid = \"en\"\nname = \"英\"\n[engine]\ntype = \"english\"\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".into()];
    cfg.schema.active = "pinyin".into();
    (EngineManager::with_store_override(&cfg, Some(&dir), None, None::<PathBuf>), schemas)
}

/// 顺序即优先级；坏条目（不存在 / 非码表 / 自己 / 空 id）逐条跳过，其余照常。
#[test]
fn schema_entries_are_resolved_in_order_and_bad_ones_skipped() {
    let (m, schemas) = setup(
        "mix",
        r#"["schema:wbx", "schema:nope", "schema:en", "schema:pinyin", "schema: ", "aux_code/t.txt"]"#,
    );
    let s = m.aux_code_settings();
    assert_eq!(
        s.sources,
        vec![
            AuxSource::Schema("wbx".into()),
            AuxSource::File(schemas.join("aux_code/t.txt")),
        ]
    );
    assert_eq!(s.schema_sources().collect::<Vec<_>>(), vec!["wbx"]);
}

#[test]
fn plain_file_entries_behave_as_before() {
    let (m, schemas) = setup("file", r#"["aux_code/t.txt", "aux_code/missing.txt"]"#);
    assert_eq!(
        m.aux_code_settings().sources,
        vec![AuxSource::File(schemas.join("aux_code/t.txt"))]
    );
}
```

（`File` 里的路径以 `resolve_schema_resource` 实际返回为准；若它返回的是规范化后的路径，断言改为比较 `ends_with("aux_code/t.txt")`。）

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --test aux_code_sources`
Expected: 编译失败（`AuxSource` 未导出 / 无 `sources` 字段）。

- [ ] **Step 2: 实现**

`wind-config/src/schema.rs`，`AuxCodeSpec` 上方：

```rust
/// `[engine.aux_code].files` 里「引用一个码表方案」的条目前缀：`"schema:wubi86"`。
/// 其余条目是码表文件路径（相对 schemas 目录）。见 `docs/design/aux-code-schema-source.md` §3。
pub const AUX_SCHEMA_SOURCE_PREFIX: &str = "schema:";
```

`files` 字段文档改为：

```rust
    /// 辅助码来源列表，**顺序即优先级**（先出现 = 高优）。两种条目：
    /// - `"aux_code/flypy_full.txt"`：码表文件（`字=码` 文本），相对 schemas 目录；
    /// - `"schema:wubi86"`：引用一个码表方案，取它的系统词库 + 用户词库里的编码
    ///   （见 [`AUX_SCHEMA_SOURCE_PREFIX`]）。只能引用 `codetable` 方案、不能引用自己。
    ///
    /// override 层写这个键即**整组替换**方案文件的基线（`merge_toml` 对数组整体替换）。
```

`manager.rs`：

```rust
/// 一条已解析的辅助码来源，见 `[engine.aux_code].files`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxSource {
    /// 码表文件的绝对路径。
    File(std::path::PathBuf),
    /// 被引用的码表方案 id（已验证存在、是 codetable、不是自己）。
    Schema(String),
}

pub struct AuxCodeSettings {
    pub enabled: bool,
    pub max_phrase_len: usize,
    /// 已解析的来源，顺序即优先级。**`enabled == false` 时恒空**（关闭即不解析）。
    pub sources: Vec<AuxSource>,
}

impl AuxCodeSettings {
    /// 其中被引用的方案 id。
    pub fn schema_sources(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().filter_map(|s| match s {
            AuxSource::Schema(id) => Some(id.as_str()),
            AuxSource::File(_) => None,
        })
    }
}
```

`aux_code_settings_of` 里原 `files` 的 `filter_map` 改为按条目分派：

```rust
                    .filter_map(|entry| {
                        if let Some(id) = entry.strip_prefix(wind_config::schema::AUX_SCHEMA_SOURCE_PREFIX) {
                            let id = id.trim();
                            if id.is_empty() || id == schema_id {
                                tracing::warn!("辅助码来源无效（空或引用自己）: {entry}");
                                return None;
                            }
                            if self.schema_engine_type(id).as_deref() != Some("codetable") {
                                tracing::warn!("辅助码来源方案不存在或不是码表方案: {id}");
                                return None;
                            }
                            return Some(AuxSource::Schema(id.to_string()));
                        }
                        let p = wind_config::Config::resolve_schema_resource(self.data_dir.as_deref(), entry);
                        if p.is_none() {
                            tracing::warn!("辅助码文件不存在（用户/系统 schemas 目录均未找到）: {entry}");
                        }
                        p.map(AuxSource::File)
                    })
```

构造处 `files` → `sources`。`reverse_index_pins` 改为：

```rust
    fn reverse_index_pins(&self) -> Vec<String> {
        let mut v = vec![self.assoc_word_schema()];
        v.extend(self.aux_code_settings().schema_sources().map(str::to_string));
        v
    }
```

`lib.rs`：`AuxSource` 与 `AuxCodeSettings` 一起导出（看 `AuxCodeSettings` 现在怎么导出就照做）。

此时 `wind-coordinator` 的 `handle_aux_code.rs` 仍引用 `settings.files`，编译会断——**本任务在协调器里只做最小适配**：把 `let paths = settings.files;` 改为

```rust
        // 第一期协调器接入前的过渡：只取文件来源，行为与改动前一致（Task 6 换成完整实现）。
        let paths: Vec<std::path::PathBuf> = settings
            .sources
            .iter()
            .filter_map(|s| match s {
                wind_engine::AuxSource::File(p) => Some(p.clone()),
                wind_engine::AuxSource::Schema(_) => None,
            })
            .collect();
```

- [ ] **Step 3: 跑测试**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-engine --test aux_code_sources
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-config -p wind-engine -p wind-coordinator aux
```
Expected: 全部 PASS。

- [ ] **Step 4: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-config/src/schema.rs crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-engine/tests/aux_code_sources.rs crates/wind-coordinator/src/handle_aux_code.rs
git add -- crates/wind-engine/tests/aux_code_sources.rs
git commit -F - --only -- crates/wind-config/src/schema.rs crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-engine/tests/aux_code_sources.rs crates/wind-coordinator/src/handle_aux_code.rs <<'EOF'
feat(config): [engine.aux_code].files 支持 schema:<id> 条目 —— 引用一个码表方案作辅助码来源

同一个键、顺序即优先级、override 整组替换，不引入哨兵值。引用不存在 / 非码表 / 自己的
条目 warn 后跳过。被引用方案进入反查索引的在用集合。协调器暂只取文件来源（下一步接入）。
EOF
```

---

### Task 6: 协调器接入 —— 多来源运行时、缓存键、就绪门卫

**Files:**
- Create: `wind_input/crates/wind-coordinator/src/aux_code_source.rs`
- Modify: `wind_input/crates/wind-coordinator/src/lib.rs`（`mod aux_code_source;`，看现有模块声明风格）
- Modify: `wind_input/crates/wind-coordinator/src/coordinator.rs:1346-1348`（字段）、`~2548`（初始化）、`~3937`（`invalidate_aux_code_table`）、`spawn_index_warm`（可见性改 `pub(crate)`）
- Modify: `wind_input/crates/wind-coordinator/src/handle_aux_code.rs`（`ensure_aux_code_table` ~85、`enter_aux_code` ~170-180、`refresh_aux_code_candidates` ~215-230、测试模块）
- Modify: `wind_input/crates/wind-coordinator/src/handle_mode.rs:1518-1531`（模式指示名）

**Interfaces:**
- Consumes: `AuxSource` / `AuxCodeSettings::sources`（Task 5）、`EngineManager::text_codes` / `prewarm_text_codes` / `reverse_index_if_ready`（Task 3）、`AuxCodeLookup`（Task 4）、`EngineManager::schema_name(&str) -> String`（现有）。
- Produces:
  - `pub(crate) struct AuxCodeRuntime` + `build(&[AuxSource], &EngineManager) -> Self`、`matches(&[AuxSource]) -> bool`、`name(&self) -> &str`、`schema_ids(&self) -> impl Iterator<Item = &str>`、`lookup(&self, &EngineManager) -> AuxLookupNow<'_>`；
  - 字段 `aux_code_runtime: RwLock<Option<Arc<AuxCodeRuntime>>>`（替换 `aux_code_table`）；
  - `fn ensure_aux_code_runtime(&self, sources: &[AuxSource]) -> Arc<AuxCodeRuntime>`（替换 `ensure_aux_code_table`）。

- [ ] **Step 1: 写失败测试（handle_aux_code.rs 测试模块）**

在测试模块加夹具与四个用例：

```rust
    /// 自造码表方案 `wbx`（工=a/aaaa、攻=atyy、公=wcu），供 `schema:wbx` 引用。
    fn write_wbx(schemas: &std::path::Path) {
        std::fs::create_dir_all(schemas.join("wbx")).unwrap();
        std::fs::write(
            schemas.join("wbx.schema.toml"),
            "[schema]\nid = \"wbx\"\nname = \"测五\"\n[engine]\ntype = \"codetable\"\n\
             [engine.codetable]\nmax_code_length = 4\n\
             [[dictionaries]]\nid = \"wbx_main\"\npath = \"wbx/wbx.dict.yaml\"\n\
             type = \"rime_codetable\"\ndefault = true\n",
        )
        .unwrap();
        std::fs::write(
            schemas.join("wbx/wbx.dict.yaml"),
            "---\nname: wbx\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n\
             a\t工\t100\naaaa\t工\t90\natyy\t攻\t80\nwcu\t公\t80\n",
        )
        .unwrap();
    }

    /// pinyin 方案的 `files` 由调用方给（TOML 数组字面量），并带上 `wbx` 与 `flypy_test.txt`。
    fn data_dir_with_files(tag: &str, files: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wind_aux_code_src_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
        std::fs::write(
            schemas.join("pinyin.schema.toml"),
            format!(
                "[schema]\nid = \"pinyin\"\nname = \"pinyin\"\n[engine]\ntype = \"pinyin\"\n\
                 [engine.aux_code]\nfiles = {files}\nenabled = true\n\
                 [key_actions]\nbacktick = \"aux_code\"\n"
            ),
        )
        .unwrap();
        std::fs::write(schemas.join("aux_code/flypy_test.txt"), "李=mz\n樱=my\n河=sk\n").unwrap();
        std::fs::write(schemas.join("aux_code/other.txt"), "李=qq\n樱=qq\n河=qq\n").unwrap();
        write_wbx(&schemas);
        dir
    }

    fn seed(c: &Arc<Coordinator>, texts: &[&str]) -> std::sync::MutexGuard<'_, crate::coordinator::State> {
        let mut st = c.state.lock().unwrap();
        st.chinese_mode = true;
        st.input_buffer = "gong".to_string();
        st.candidates = texts.iter().map(|t| cand(t)).collect();
        st.selected_index = 0;
        st.current_page = 0;
        st.preedit = "gong".to_string();
        st
    }

    fn kept(st: &crate::coordinator::State) -> Vec<String> {
        st.candidates.iter().map(|c| c.text.clone()).collect()
    }

    /// `schema:wbx`：用码表方案的编码筛拼音候选；与文件来源混列时两边的码都生效。
    #[test]
    fn schema_source_filters_by_codetable_codes() {
        let dir = data_dir_with_files("schema", r#"["schema:wbx", "aux_code/flypy_test.txt"]"#);
        let c = coord_with_data("schema_src", dir);
        c.engine_mgr.prewarm_text_codes("wbx");
        let mut st = seed(&c, &["工", "攻", "公", "河"]);
        assert!(c.enter_aux_code(&mut st, keymap::VK_BACKTICK).is_some());
        let _ = c.handle_aux_code_key(&mut st, &key(vk_letter('A'), 0));
        assert_eq!(kept(&st), vec!["工", "攻"], "方案来源：a 命中工/攻");
        let _ = c.handle_aux_code_key(&mut st, &key(keymap::VK_BACK, 0));
        let _ = c.handle_aux_code_key(&mut st, &key(vk_letter('S'), 0));
        assert_eq!(kept(&st), vec!["河"], "文件来源的码同样生效");
    }

    /// 被引用方案里用户自己加的编码也能筛到（系统 + 用户词库是一个整体）。
    #[test]
    fn schema_source_includes_user_words() {
        let dir = data_dir_with_files("user", r#"["schema:wbx"]"#);
        let c = coord_with_data("schema_user", dir);
        c.store.as_ref().unwrap().add_user_word("wbx", "zzzz", "嗨", 0, 0).unwrap();
        c.engine_mgr.prewarm_text_codes("wbx");
        let mut st = seed(&c, &["工", "嗨"]);
        let _ = c.enter_aux_code(&mut st, keymap::VK_BACKTICK);
        let _ = c.handle_aux_code_key(&mut st, &key(vk_letter('Z'), 0));
        assert_eq!(kept(&st), vec!["嗨"]);
    }

    /// ★ 系统层没就绪：不进入、不吞键——按键线程绝不现建反查索引。
    #[test]
    fn schema_source_not_ready_does_not_enter() {
        let dir = data_dir_with_files("cold", r#"["schema:wbx"]"#);
        let c = coord_with_data("schema_cold", dir);
        let mut st = seed(&c, &["工", "攻"]);
        assert!(c.enter_aux_code(&mut st, keymap::VK_BACKTICK).is_none());
        assert_eq!(st.active, None);
        assert_eq!(kept(&st), vec!["工", "攻"], "候选原封不动");
    }

    /// ★ 改了来源（override 层换 files）不切方案也要生效——此前缓存只在切方案时清。
    #[test]
    fn source_change_takes_effect_without_schema_switch() {
        let dir = data_dir_with_files("ovr", r#"["aux_code/flypy_test.txt"]"#);
        let ov = dir.join("overrides");
        std::fs::create_dir_all(&ov).unwrap();
        let path = std::env::temp_dir().join(format!("wind_aux_code_ovr_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Arc::new(Store::open(&path).unwrap());
        let mut cfg = Config::default();
        cfg.schema.active = "pinyin".to_string();
        let c = Coordinator::new_headless_with_store_override(cfg, Some(&dir), store, Some(ov.clone()));
        let mut st = seed(&c, &["李", "樱", "河"]);
        let _ = c.enter_aux_code(&mut st, keymap::VK_BACKTICK);
        let _ = c.handle_aux_code_key(&mut st, &key(vk_letter('M'), 0));
        assert_eq!(kept(&st), vec!["李", "樱"]);
        let _ = c.handle_aux_code_key(&mut st, &key(keymap::VK_ESCAPE, 0));
        std::fs::write(ov.join("pinyin.toml"), "[engine.aux_code]\nfiles = [\"aux_code/other.txt\"]\n").unwrap();
        let _ = c.enter_aux_code(&mut st, keymap::VK_BACKTICK);
        let _ = c.handle_aux_code_key(&mut st, &key(vk_letter('Q'), 0));
        assert_eq!(kept(&st), vec!["李", "樱", "河"], "新来源 other.txt 生效");
    }
```

（`c.store` 的实际字段名以 `Coordinator` 为准；`Esc` 退出后候选是否复原为进入前那组，照已有用例 `schema_switch_invalidates_aux_code_table` 的写法——它在 Esc 后直接再次进入。）

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-coordinator --lib handle_aux_code`
Expected: `schema_source_filters_by_codetable_codes` / `schema_source_includes_user_words` FAIL（方案来源尚被忽略，`enter` 因无来源返回 None 或不筛）；`source_change_takes_effect_without_schema_switch` FAIL（缓存沿用旧表，`q` 全滤空）。**记下这三条红的输出**。

- [ ] **Step 2: 实现 `aux_code_source.rs`**

```rust
//! 辅助码的运行时来源：`[engine.aux_code].files` 按序解析出的各层，查询时顺序拼接。
//!
//! 不预先合并成一张表：方案来源的码来自「系统词库 + 用户词库」，用户层会随造词后台重建，
//! 只能在每次筛选时取当下的视图（`EngineManager::text_codes`）。文件来源仍是进来时读一次的
//! 静态表。设计见 `docs/design/aux-code-schema-source.md` §4、§5。

use std::sync::Arc;
use wind_aux_code::{AuxCodeLookup, AuxCodeTable};
use wind_engine::{AuxSource, EngineManager, TextCodeView};

pub(crate) struct AuxCodeRuntime {
    /// 由哪组来源建成——缓存键。来源变了（改了 override、换了方案）就重建。
    key: Vec<AuxSource>,
    /// 模式指示用的名字：首个来源的名（码表 `# name:` 或方案名）。
    name: String,
    layers: Vec<Layer>,
}

enum Layer {
    Table(AuxCodeTable),
    Schema(String),
}

impl AuxCodeRuntime {
    /// 文件来源同步读（码表小，与改动前同一时机）；方案来源只记 id，数据在查询时取。
    pub(crate) fn build(sources: &[AuxSource], engine: &EngineManager) -> Self {
        let mut name = String::new();
        let layers = sources
            .iter()
            .map(|s| match s {
                AuxSource::File(p) => {
                    let t = wind_aux_code::load_from_file(p);
                    if name.is_empty() {
                        name = t.name.clone();
                    }
                    Layer::Table(t)
                }
                AuxSource::Schema(id) => {
                    if name.is_empty() {
                        name = engine.schema_name(id);
                    }
                    Layer::Schema(id.clone())
                }
            })
            .collect();
        Self { key: sources.to_vec(), name, layers }
    }

    pub(crate) fn matches(&self, sources: &[AuxSource]) -> bool {
        self.key == sources
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn schema_ids(&self) -> impl Iterator<Item = &str> {
        self.layers.iter().filter_map(|l| match l {
            Layer::Schema(id) => Some(id.as_str()),
            Layer::Table(_) => None,
        })
    }

    /// 本次筛选用的查询对象：方案层取当下视图（用户层过期会在这里触发后台重建）。
    pub(crate) fn lookup(&self, engine: &EngineManager) -> AuxLookupNow<'_> {
        AuxLookupNow {
            layers: self
                .layers
                .iter()
                .map(|l| match l {
                    Layer::Table(t) => Now::Table(t),
                    Layer::Schema(id) => Now::View(engine.text_codes(id)),
                })
                .collect(),
        }
    }
}

pub(crate) struct AuxLookupNow<'a> {
    layers: Vec<Now<'a>>,
}

enum Now<'a> {
    Table(&'a AuxCodeTable),
    View(TextCodeView),
}

impl AuxCodeLookup for AuxLookupNow<'_> {
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
        let mut buf = [0u8; 4];
        let text: &str = ch.encode_utf8(&mut buf);
        self.layers.iter().any(|l| match l {
            Now::Table(t) => t.any_code(ch, pred),
            Now::View(v) => v.any_code(text, pred),
        })
    }

    fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| match l {
            Now::Table(t) => AuxCodeLookup::is_empty(*t),
            Now::View(v) => !v.has_any(),
        })
    }
}
```

- [ ] **Step 3: 接入协调器**

`coordinator.rs` 字段替换：

```rust
    /// 辅助码运行时来源（进入辅助码时按 `[engine.aux_code].files` 建，来源清单即缓存键）。
    /// `None` = 尚未建 / 已失效。见 [`crate::aux_code_source`]。
    pub(crate) aux_code_runtime:
        std::sync::RwLock<Option<Arc<crate::aux_code_source::AuxCodeRuntime>>>,
```

初始化 `aux_code_runtime: std::sync::RwLock::new(None),`；`invalidate_aux_code_table` 里把 `aux_code_table` 换成 `aux_code_runtime`（函数名不变，切方案钩子与已有测试照用）。`fn spawn_index_warm` 改 `pub(crate) fn`。

`handle_aux_code.rs`：

```rust
    /// 取辅助码运行时来源：缓存的来源清单与本次一致就复用，否则重建。
    ///
    /// ★ 按来源清单判断而不是只靠「切方案清空」：在设置页改了来源（写 override）不会切方案，
    /// 旧做法会一直用旧表直到下次切方案。
    pub(crate) fn ensure_aux_code_runtime(
        &self,
        sources: &[wind_engine::AuxSource],
    ) -> Arc<crate::aux_code_source::AuxCodeRuntime> {
        if let Some(rt) = self
            .aux_code_runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|rt| rt.matches(sources))
        {
            return rt.clone();
        }
        let rt = Arc::new(crate::aux_code_source::AuxCodeRuntime::build(sources, &self.engine_mgr));
        info!("Loaded aux-code sources ({} layers)", sources.len());
        *self.aux_code_runtime.write().unwrap_or_else(|e| e.into_inner()) = Some(rt.clone());
        rt
    }
```

`enter_aux_code` 里 Task 5 的过渡代码与 `self.ensure_aux_code_table(&paths);` 一起替换为：

```rust
        if settings.sources.is_empty() || state.candidates.is_empty() {
            return None;
        }
        let rt = self.ensure_aux_code_runtime(&settings.sources);
        // 方案来源的系统层（反查索引）未就绪：不进入、不吞键，派后台构建（建好有提示与重渲染）。
        // 按键线程绝不现建——那是秒级操作，而 TSF→服务是同步 IPC。
        let pending: Vec<String> = rt
            .schema_ids()
            .filter(|id| self.engine_mgr.reverse_index_if_ready(id).is_none())
            .map(str::to_string)
            .collect();
        if !pending.is_empty() {
            for id in &pending {
                self.spawn_index_warm(id, false);
            }
            debug!("aux_code: 方案来源索引未就绪，本次不进入");
            return None;
        }
```

`refresh_aux_code_candidates` 的取表与筛选改为：

```rust
        let rt = self
            .aux_code_runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let overlay = state.aux_code.as_mut().expect("辅助码模式必持 overlay");
        // 未建 = 防御语义：不过滤，还原快照（见 wind-aux-code）。
        state.candidates = match rt {
            Some(rt) => {
                let lookup = rt.lookup(&self.engine_mgr);
                overlay.session.apply(&lookup, &overlay.filter_options)
            }
            None => overlay.session.restore_original(),
        };
```

（`reset_candidate_view(state)` 仍在取 overlay 之前调用，保持原顺序。）

`handle_mode.rs` 模式指示：

```rust
            ModeKind::AuxCode => {
                let rt = self
                    .aux_code_runtime
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()?;
                let name = rt.name().to_string();
                if name.is_empty() {
                    None
                } else {
                    let short = Self::short_or_first("", &name);
                    Some((name, short))
                }
            }
```

`grep -rn "aux_code_table\b" crates/wind-coordinator/src` 只允许剩 `invalidate_aux_code_table` 函数名及其调用、测试名。

- [ ] **Step 4: 跑测试确认转绿，并跑协调器全量**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-coordinator --lib handle_aux_code
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-coordinator
```
Expected: 新增 4 条与既有辅助码用例全部 PASS；协调器全量无回归。

- [ ] **Step 5: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-coordinator/src/aux_code_source.rs crates/wind-coordinator/src/lib.rs crates/wind-coordinator/src/coordinator.rs crates/wind-coordinator/src/handle_aux_code.rs crates/wind-coordinator/src/handle_mode.rs
git add -- crates/wind-coordinator/src/aux_code_source.rs
git commit -F - --only -- crates/wind-coordinator/src/aux_code_source.rs crates/wind-coordinator/src/lib.rs crates/wind-coordinator/src/coordinator.rs crates/wind-coordinator/src/handle_aux_code.rs crates/wind-coordinator/src/handle_mode.rs <<'EOF'
feat(aux-code): 辅助码可引用码表方案（schema:<id>），系统 + 用户词库一起参与筛选

files 里的文件表与方案视图按序拼成运行时查询对象；方案层每次筛选取当下的 TextCodeView，
用户造的字随用户层后台重建生效。来源清单做缓存键，修掉「设置页改了来源要切一次方案才
生效」。方案来源的反查索引未就绪时不进入、不吞键，交给后台构建。
EOF
```

---

### Task 7: RPC `schema.auxCodeSources`

**Files:**
- Modify: `wind_input/crates/wind-engine/src/manager.rs`（新方法 `aux_code_source_options`，放在 `aux_code_settings_of` 之后）
- Modify: `wind_input/crates/wind-webdata/Cargo.toml`（加 `wind-aux-code = { path = "../wind-aux-code" }`）
- Modify: `wind_input/crates/wind-webdata/src/lib.rs`（分派表 ~371 加一行；处理函数放在 `web_schema_save_config` 附近；测试模块加用例）

**Interfaces:**
- Consumes: `wind_aux_code::read_name`（Task 4）、`Config::list_schema_resource_dir`（现有）、`installed_schemas` / `schema_engine_type` / `schema_name`（现有）。
- Produces: `EngineManager::aux_code_source_options(&self, exclude: &str) -> AuxCodeSourceOptions`，`pub struct AuxCodeSourceOptions { pub schemas: Vec<(String, String)>, pub files: Vec<(String, std::path::PathBuf)> }`；RPC 返回 `{"schemas":[{"id","name"}],"files":[{"path","label"}]}`（供第 3 期设置端）。

- [ ] **Step 1: 写失败测试（wind-webdata 测试模块）**

```rust
    /// 设置页「辅助码来源」的可选项：码表方案（不含请求方自己、不含非码表方案）+ aux_code/*.txt。
    #[test]
    fn schema_aux_code_sources_lists_codetable_schemas_and_tables() {
        let dir = std::env::temp_dir().join(format!("wind_webdata_aux_src_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
        std::fs::write(
            schemas.join("pinyin.schema.toml"),
            "[schema]\nid = \"pinyin\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n",
        )
        .unwrap();
        std::fs::write(
            schemas.join("zz_wb.schema.toml"),
            "[schema]\nid = \"zz_wb\"\nname = \"测五\"\nhidden = true\n[engine]\ntype = \"codetable\"\n",
        )
        .unwrap();
        std::fs::write(schemas.join("aux_code/zz_named.txt"), "# name: 小鹤\n李=mz\n").unwrap();
        std::fs::write(schemas.join("aux_code/zz_plain.txt"), "李=mz\n").unwrap();
        let store_path = dir.join("s.redb");
        let store = std::sync::Arc::new(wind_store::Store::open(&store_path).unwrap());
        let c = Coordinator::new_headless_with_store_override(
            wind_config::Config::default(),
            Some(&dir),
            store,
            None,
        );
        let v = c
            .web_data_rpc("schema.auxCodeSources", &json!({ "id": "pinyin" }))
            .unwrap();
        let schemas_v = v["schemas"].as_array().unwrap();
        assert!(schemas_v.iter().any(|s| s["id"] == "zz_wb" && s["name"] == "测五"), "隐藏的码表方案也列出");
        assert!(!schemas_v.iter().any(|s| s["id"] == "pinyin"), "请求方自己 / 非码表不列");
        let files = v["files"].as_array().unwrap();
        assert!(files.iter().any(|f| f["path"] == "aux_code/zz_named.txt" && f["label"] == "小鹤"));
        assert!(files.iter().any(|f| f["path"] == "aux_code/zz_plain.txt" && f["label"] == "zz_plain"), "无头时退回文件名");
    }
```

（`schema_name` 若对 hidden 方案返回别的值，以其实际行为为准调整断言；`c.web_data_rpc` 的调用方式照同文件既有用例。）

Run: `cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input && CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-webdata schema_aux_code_sources`
Expected: FAIL（未知方法）。

- [ ] **Step 2: 实现**

`manager.rs`：

```rust
/// 设置页「辅助码来源」的可选项。
pub struct AuxCodeSourceOptions {
    /// (方案 id, 方案名)：已安装的码表方案，含隐藏、不看是否启用。
    pub schemas: Vec<(String, String)>,
    /// (相对 schemas 的路径, 绝对路径)：各层 `schemas/aux_code/*.txt`，靠前的层遮蔽同名者。
    pub files: Vec<(String, std::path::PathBuf)>,
}

    /// 设置页「辅助码来源」的可选项：码表方案（排除 `exclude`，即请求方自己）+ 码表文件。
    pub fn aux_code_source_options(&self, exclude: &str) -> AuxCodeSourceOptions {
        let schemas = self
            .installed_schemas()
            .into_iter()
            .filter(|id| id != exclude && self.schema_engine_type(id).as_deref() == Some("codetable"))
            .map(|id| {
                let name = self.schema_name(&id);
                (id, name)
            })
            .collect();
        let files = wind_config::Config::list_schema_resource_dir(self.data_dir.as_deref(), "aux_code", ".txt");
        AuxCodeSourceOptions { schemas, files }
    }
```

（`AuxCodeSourceOptions` 与 `AuxSource` 一并从 `lib.rs` 导出。）

`wind-webdata/src/lib.rs` 分派表 `"schema.saveConfig"` 旁：

```rust
            "schema.auxCodeSources" => self.web_schema_aux_code_sources(params),
```

```rust
    /// 设置页「辅助码来源」下拉的两组可选项，见 `docs/design/aux-code-schema-source.md` §6。
    ///
    /// 码表名只读文件首行（`read_name`），缺头时退回文件名——列出来比滤掉好查。
    fn web_schema_aux_code_sources(&self, params: &Value) -> anyhow::Result<Value> {
        let exclude = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let o = self.engine_mgr().aux_code_source_options(exclude);
        let schemas: Vec<Value> = o
            .schemas
            .iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect();
        let files: Vec<Value> = o
            .files
            .iter()
            .map(|(rel, abs)| {
                let label = wind_aux_code::read_name(abs).unwrap_or_else(|| {
                    abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                });
                json!({ "path": rel, "label": label })
            })
            .collect();
        Ok(json!({ "schemas": schemas, "files": files }))
    }
```

- [ ] **Step 3: 跑测试**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test -p wind-webdata
```
Expected: 全部 PASS。

- [ ] **Step 4: 格式化并提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
rustfmt --edition 2024 crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-webdata/src/lib.rs
git commit -F - --only -- crates/wind-engine/src/manager.rs crates/wind-engine/src/lib.rs crates/wind-webdata/Cargo.toml crates/wind-webdata/src/lib.rs Cargo.lock <<'EOF'
feat(webdata): schema.auxCodeSources —— 列出可作辅助码来源的码表方案与码表文件

供设置页「辅助码来源」下拉（第 3 期）。方案含隐藏、不看启用，排除请求方自己与非码表方案；
码表名只读首行 `# name:`，缺头退回文件名。
EOF
```

（`Cargo.lock` 若无变化则从 `--only` 列表里去掉。）

---

### Task 8: 收尾 —— 全量回归与文档状态

**Files:**
- Modify: `docs/design/text-code-lookup.md`、`docs/design/aux-code-schema-source.md`（状态行）
- Modify: `data/schemas/pinyin.schema.toml`、`data/schemas/shuangpin.schema.toml`（`[engine.aux_code]` 注释补一句 `schema:` 写法，**不改默认值**）

- [ ] **Step 1: 全量测试（worktree 已拷全 `build_dev/data`，结果可作证据）**

Run:
```
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput/wind_input
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc TMPDIR=/tmp/wct-auxsrc cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | awk '/test result/{p+=$4; f+=$6} /FAILED|panicked/{print} END{print "passed="p" failed="f}'
CARGO_TARGET_DIR=/home/dufeng/.cache/wi-tgt-auxsrc cargo clippy -p wind-store -p wind-engine -p wind-aux-code -p wind-coordinator -p wind-webdata --tests -- -D warnings
```
Expected: `failed=0`，passed 为四位数（~4600 量级，见 worktree-test-count-baseline；明显偏少说明数据目录没生效）；clippy 无警告。

- [ ] **Step 2: 文档**

- 两份设计文档状态行改为「第一期已实施（分支 feat/aux-code-source）」，在各自分期表里给第 0/1 期填上本分支提交号（`git log --oneline main..HEAD`）。
- `pinyin.schema.toml` / `shuangpin.schema.toml` 的 `[engine.aux_code]` 注释补：「`files` 也可写 `"schema:<码表方案 id>"`，取该方案系统词库 + 用户词库里的编码，如 `files = ["schema:wubi86"]`。」

- [ ] **Step 3: 提交**

```bash
cd /home/dufeng/develop/windinput/wt-auxsrc/WindInput
git commit -F - --only -- docs/design/text-code-lookup.md docs/design/aux-code-schema-source.md data/schemas/pinyin.schema.toml data/schemas/shuangpin.schema.toml <<'EOF'
docs(aux-code): 第一期落地回填 —— 设计文档状态与提交号，方案文件注释补 schema: 写法
EOF
```

---

## 裁定记录（相对设计稿的取舍）

- **用户层槽位按容量淘汰（4 份、最久未用），不按「在用集合」**：在用集合要读方案文件才能算出，而槽位插入发生在查询路径上；容量上限同样有界，且覆盖主码表 / 联想 / 辅助码三个在用方案。反查索引那边仍按在用集合（它的淘汰发生在后台构建之后，算得起）。代价：同时在用超过 4 个方案时会有一份反复重建。
- **第一期不收临时词、不做 `Layers` 参数**：第一期两个使用方（辅助码、编码显示）都只要系统 + 用户；临时层随第二期词语联想迁移再加。
- **非字母编码不在第一期过滤**：辅助码缓冲只收字母，含 `;` / 数字的码天然匹配不上，不影响正确性；设置端「不列出非字母方案」要扫词库才能判断，放到第 3 期与设置端一起定。
- **缓存键取代「保存后主动清缓存」**：进入辅助码时本来就要读一次方案（`aux_code_settings_of`），比较来源清单即可发现变化，不必在 `refresh_schema_derived_config` 里再加一处失效。切方案的主动失效保留（码表文件内容变化只有它能兜住）。
