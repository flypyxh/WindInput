# 用户数据存储重设计：P1a 实施计划（引擎补强 + 运行时领域 API）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `wind-userdata` 具备替换 redb 中 temp / draft / freq / assoc 四张表所需的全部能力，并按 `wind-store` 现有语义提供领域 API 与 `DictLayer`，用移植过来的现有测试对拍；P1b 只做接线、迁移与退役。

**Architecture:** 引擎侧补五件事：操作带时间（`created_at` / `last_used` 取写入时刻）、叠加语义只有一份（`apply_ops`，读路径 / 折回 / 层共用）、delta 按键二分、空快照与「带策略的折回」`fold_with`（清空 / 导入 / 淘汰 / 过期都走它，原子换段推进水位）、日志压实与后台折回。其上新增 `runtime` 模块：`Runtime` 包住 `Arc<UserData>`，提供临时词、草稿、词频、联想历史四组 API；`layer` 模块给出与 `StoreTempLayer` / `StoreDraftLayer` 行为一致的两个层。全部在 `wind-userdata` 内，不改其他 crate。

**Tech Stack:** Rust 2024（工具链 1.96）；`wind-dict`（`WdatStreamWriter` / `WdatReader` / `layer::{pattern_matches, literal_prefix, cmp_pattern}`）；`wind-candidate`。不新增依赖；`wind-userdata` **不得依赖 `wind-store`**（去 redb 的方向）。

**Spec:** `docs/design/user-data-storage-redesign.md`（§3 引擎、§3.5 折回、§3.6 state、§3.8 批量、§4 读路径、§6 表去向、§9 指标）；行为对照 `wind_input/crates/wind-store/src/{temp_words,draft_words,freq,assoc_history}.rs` 与 `wind_input/crates/wind-dict/src/store_layer.rs`。

## P1 拆分

| 计划 | 内容 | 不含 |
|---|---|---|
| **P1a（本计划）** | 引擎补强（P0 审查延后项 M3/M4/M5/M6 + 开放问题）；`Runtime` 四组领域 API；两个层 | 任何其他 crate 的改动 |
| P1b（另写） | `construct.rs` 打开 `UserData`、`EngineManager` 换层、协调器 ~15 个写入点、设置页 IPC、备份 / 导出、`UserAssocIndex`、redb → 引擎一次性迁移（§7.2）、`format_version` → 2、五张表停写 | — |

P1b 依赖本计划落地后的真实接口，故等本计划完成再写。

## 计划内裁定（P0 审查延后项）

- **M1（记录帧加 generation）不做。** §3.8 的批量操作在本计划里一律走 `fold_with`：新快照（内嵌 `max_seq = upto`）原子换入即推进水位，`seq ≤ upto` 的旧记录不再回放，无需 generation 区分「代」。P1b 的还原同样走 `fold_with`（逐目标替换）。spec §3.6 的 generation 一句在 Task 9 改为「由 fold_with 的水位承担」。代价：若日后出现「整目录文件级替换」式还原，再加。
- **M2（Upsert 的 kind / sidecar flags）不做。** 四类数据没有一个需要 flags；`DeltaOp::Upsert { kind }` 保留为 0。
- **M3、M4、M5、M6、开放问题（`search_prefix(…, 0)`）** 在本计划修：Task 1（M3、M5、开放问题）、Task 2（M4）、Task 3（M6）。

## Global Constraints

- 时间单位：sidecar 与领域 API 的 `created_at` / `last_used` 是 **unix 秒**（与 `wind-store` 的 `UserWordRecord` / `FreqRecord` 同口径）；日志记录的 `ts_ms` 是毫秒，叠加时 `div_euclid(1000)`。
- 叠加语义只有一份：`engine::apply_ops`。读（`get` / `exact` / `scan`）、折回（`merge_for_fold`）、层（`layer.rs`）都经 `engine::overlay_entries` 调它，不得各写一份。
- 批量操作（清空、导入、淘汰草稿、过期草稿）走 `UserData::fold_with`，不逐条写日志（§3.8）。按键路径上的单条写入走 `apply` / `apply_batch`。
- `wind-userdata` 不依赖 `wind-store`；对照行为时读 `wind-store` 源码，不 `use` 它。
- 日志记录**一条一次 `write(2)`**（§3.2）；`apply_batch` 的每条记录各自一次 `append`。
- 只读实例零写入（P0 审查 I1 的修法不得回退）。
- INFO 级日志不得含词条内容。
- 提交走 `git commit -F - --only -- <路径>`；每个任务一笔；`cargo fmt -p wind-userdata`，不跑 `--all`；新文件先 `git add <该文件>` 再 `--only`。
- 测试：`cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast`；临时目录名带 pid。

## Review Focus

1. **联想上文是中文**（`code` 为任意 UTF-8 字符串）：折回成快照再重开后仍能精确查到。→ Task 7 `assoc_context_non_ascii_survives_fold`。
2. **过期草稿已在快照里**：查询时仍按 TTL 过滤，不靠清理。→ Task 6 `expired_draft_in_snapshot_is_hidden_at_query_time`。
3. **清空后 state 丢失再重开**：被清空的词不复活。→ Task 3 `clear_survives_state_loss`。
4. **临时词一半在快照、一半在 delta**：再学习时 count 从快照计数接着加，晋升阈值判断不归零。→ Task 5 `count_continues_from_snapshot_after_fold`。
5. **淘汰候选跨快照与 delta**：按 `(count, created_at, weight)` 升序淘汰，不因数据所在层不同而错序。→ Task 5 `evict_orders_across_snapshot_and_delta`。

---

### Task 1: 操作带时间、叠加语义唯一、`apply_batch`

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/delta.rs`（`Entry` 加 `ts_ms`；`entry_from_record` 填它；测试辅助 `e()` 补字段）
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`Op`、`apply_batch`、`apply_ops` 新语义、`overlay_entries`、`get` / `exact`；删 `overlay` 与 `query_count`；`merge_for_fold` 改用 `overlay_entries`）
- Modify: `wind_input/crates/wind-userdata/src/snapshot.rs`（`exact` / `prefix` / `abbrev` 返回 `Vec<SnapshotEntry>`；新增 `for_each_prefix`）
- Modify: `wind_input/crates/wind-userdata/src/layer.rs`（改用 `overlay_entries`；`natural_order` 恒 0；`search_prefix(…, 0)` 走全量前缀）
- Modify: `wind_input/crates/wind-userdata/tests/engine_crash_paths.rs`（`query_count` → `get(..).map(|e| e.count)`）
- Test: `engine.rs` / `snapshot.rs` / `layer.rs` 的 tests 模块

**Interfaces:**
- Produces:
  - `pub struct Op { pub target: Target, pub schema: String, pub kind: OpKind, pub code: String, pub text: String, pub payload: Payload, pub boundary: u64, pub ts_ms: i64 }`，`ts_ms == 0` 表示「取当前时刻」；`Op::new(target, schema: &str, kind, code: &str, text: &str, payload, boundary) -> Op`（`ts_ms = 0`）。
  - `UserData::apply_batch(&self, ops: &[Op]) -> anyhow::Result<Vec<u64>>`；`apply` 委托给它。
  - `UserData::get(&self, target: Target, schema: &str, code: &str, text: &str) -> Option<SnapshotEntry>`
  - `UserData::exact(&self, target: Target, schema: &str, code: &str) -> Vec<SnapshotEntry>`（按 text 升序）
  - `pub(crate) fn apply_ops(base: Option<SnapshotEntry>, ops: &[&Entry], code: &str, text: &str) -> Option<SnapshotEntry>`
  - `pub(crate) fn overlay_entries(base: Vec<SnapshotEntry>, ops: Vec<&Entry>) -> Vec<SnapshotEntry>`（按 `(code, text)` 升序；`ops` 对涉及的每个键必须是**该键的全部**操作）
  - `Snapshot::exact(&self, code) -> Vec<SnapshotEntry>`、`Snapshot::prefix(&self, prefix, limit) -> Vec<SnapshotEntry>`、`Snapshot::abbrev(&self, abbrev, limit) -> Vec<SnapshotEntry>`、`Snapshot::for_each_prefix(&self, prefix: &str, f: &mut dyn FnMut(SnapshotEntry) -> bool)`
  - 删除：`UserData::query_count`（调用方改 `get`）。`UserData::query_exact` 保留，改为基于 `exact`。

叠加语义（`apply_ops`，`ts = op.ts_ms.div_euclid(1000)`）：

| 操作 | 已有条目 | 无条目 |
|---|---|---|
| `Upsert{weight,kind}` | weight、flags、boundary 覆盖；`last_used = ts`；count、created_at 保留 | 新建：count 0、`created_at = last_used = ts` |
| `Incr(c)` | `count += c`（饱和）；`last_used = ts` | 新建：weight 0、count c、`created_at = last_used = ts`、boundary 取操作的 |
| `SetWeight(w)` | weight = w | 无操作 |
| `Touch(t)` | `last_used = t`（payload 即秒） | 无操作 |
| `Delete` | 删 | 无操作 |

- [ ] **Step 1: 写失败测试（engine.rs tests 模块末尾追加）**

```rust
    fn dirs_for(name: &str) -> (std::path::PathBuf, Dirs) {
        let d = std::env::temp_dir().join(format!("wind_ud_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        (
            d.clone(),
            Dirs {
                state_dir: d,
                dir_id: "dir".into(),
            },
        )
    }

    /// M3：created_at 是写入时刻，折回不改它；Upsert 刷新 last_used；Incr 对不存在的键新建条目。
    #[test]
    fn timestamps_come_from_the_op_not_the_fold() {
        let (d, dirs) = dirs_for("ts");
        let ud = UserData::open(dirs).unwrap();
        let mut a = Op::new(Target::Temp, "py", OpKind::Upsert, "a", "甲", Payload::Weight(5), 0);
        a.ts_ms = 5_000_000;
        let mut a2 = Op::new(Target::Temp, "py", OpKind::Upsert, "a", "甲", Payload::Weight(6), 0);
        a2.ts_ms = 9_000_000;
        let mut f = Op::new(Target::Freq, "py", OpKind::Incr, "b", "乙", Payload::Count(2), 0);
        f.ts_ms = 7_000_000;
        ud.apply_batch(&[a, a2, f]).unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.fold(Target::Freq, "py").unwrap();
        let e = ud.get(Target::Temp, "py", "a", "甲").unwrap();
        assert_eq!((e.weight, e.created_at, e.last_used), (6, 5_000, 9_000));
        let f = ud.get(Target::Freq, "py", "b", "乙").expect("Incr 对不存在的键新建");
        assert_eq!((f.count, f.created_at, f.last_used), (2, 7_000, 7_000));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// M5：没有增量语义的记录不落日志、不占号。
    #[test]
    fn op_without_delta_semantics_is_refused_before_append() {
        let (d, dirs) = dirs_for("m5");
        let ud = UserData::open(dirs).unwrap();
        let bad = Op::new(Target::Temp, "py", OpKind::BatchMarker, "a", "甲", Payload::None, 0);
        assert!(ud.apply_batch(&[bad]).is_err());
        let seq = ud
            .apply(Target::Temp, "py", OpKind::Upsert, "a", "甲", Payload::Weight(1), 0)
            .unwrap();
        assert_eq!(seq, 1, "被拒的记录不占号");
        let _ = std::fs::remove_dir_all(&d);
    }
```

在 `layer.rs` tests 模块追加（沿用该模块已有的打开 `UserData` 的方式；若没有现成辅助函数，照上面 `dirs_for` 写一个同名本地函数）：

```rust
    /// 开放问题：search_prefix(…, 0) 表示不限，快照里的条目也要返回。
    #[test]
    fn prefix_with_zero_limit_returns_snapshot_entries_too() {
        let d = std::env::temp_dir().join(format!("wind_layer_p0-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ud = Arc::new(
            UserData::open(Dirs {
                state_dir: d.clone(),
                dir_id: "dir".into(),
            })
            .unwrap(),
        );
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihao", "你好", Payload::Weight(5), 0b101)
            .unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihaoma", "你好吗", Payload::Weight(3), 0b100101)
            .unwrap();
        let layer = UserDataTempLayer::new(ud, "py");
        assert_eq!(names(layer.search_prefix("niha", 0)), vec!["你好", "你好吗"]);
        let _ = std::fs::remove_dir_all(&d);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`Op`、`apply_batch`、`get` 未定义。

- [ ] **Step 3: 实现**

`delta.rs`：`Entry` 加字段并在 `entry_from_record` 里填：

```rust
    /// 写入时刻（毫秒，取自日志记录）；叠加时折成秒写进 created_at / last_used。
    pub ts_ms: i64,
```

```rust
    Some(Entry {
        seq: r.seq,
        ts_ms: r.ts_ms,
        // …其余字段不变
    })
```

`delta.rs` tests 的 `e()` 辅助与其他构造 `Entry` 的地方补 `ts_ms: 0`。

`engine.rs`：在 `pub struct Dirs` 之后加：

```rust
/// 一条待写入的操作。`ts_ms == 0` 表示取当前时刻（测试与迁移可显式给定）。
#[derive(Clone, Debug)]
pub struct Op {
    pub target: Target,
    pub schema: String,
    pub kind: OpKind,
    pub code: String,
    pub text: String,
    pub payload: Payload,
    pub boundary: u64,
    pub ts_ms: i64,
}

impl Op {
    pub fn new(
        target: Target,
        schema: &str,
        kind: OpKind,
        code: &str,
        text: &str,
        payload: Payload,
        boundary: u64,
    ) -> Op {
        Op {
            target,
            schema: schema.into(),
            kind,
            code: code.into(),
            text: text.into(),
            payload,
            boundary,
            ts_ms: 0,
        }
    }
}
```

把 `apply_ops` 替换为（签名改 `pub(crate)`）：

```rust
/// 叠加语义的唯一实现（读路径、折回、层共用）。表见 P1a 计划 Task 1。
pub(crate) fn apply_ops(
    base: Option<SnapshotEntry>,
    ops: &[&Entry],
    code: &str,
    text: &str,
) -> Option<SnapshotEntry> {
    let fresh = |weight: i32, count: u32, boundary: u64, ts: i64, flags: u8| SnapshotEntry {
        code: code.to_string(),
        text: text.to_string(),
        weight,
        boundary,
        count,
        created_at: ts,
        last_used: ts,
        flags,
    };
    let mut cur = base;
    for op in ops {
        let ts = op.ts_ms.div_euclid(1000);
        match op.op {
            DeltaOp::Upsert { weight, kind } => match &mut cur {
                Some(e) => {
                    e.weight = weight;
                    e.flags = kind;
                    e.boundary = op.boundary;
                    e.last_used = ts;
                }
                None => cur = Some(fresh(weight, 0, op.boundary, ts, kind)),
            },
            DeltaOp::Delete => cur = None,
            DeltaOp::SetWeight(w) => {
                if let Some(e) = &mut cur {
                    e.weight = w;
                }
            }
            DeltaOp::Incr(c) => match &mut cur {
                Some(e) => {
                    e.count = e.count.saturating_add(c);
                    e.last_used = ts;
                }
                None => cur = Some(fresh(0, c, op.boundary, ts, 0)),
            },
            DeltaOp::Touch(t) => {
                if let Some(e) = &mut cur {
                    e.last_used = t;
                }
            }
        }
    }
    cur
}

/// 快照条目 + delta 操作 → 叠加后的条目，按 (code, text) 升序。`ops` 里涉及的每个键必须带上
/// **该键的全部**操作（`Delta::exact` / `prefix` / `ops_for` 给出的都满足），否则叠加不完整。
pub(crate) fn overlay_entries(base: Vec<SnapshotEntry>, mut ops: Vec<&Entry>) -> Vec<SnapshotEntry> {
    let mut map: BTreeMap<(String, String), Option<SnapshotEntry>> = base
        .into_iter()
        .map(|e| ((e.code.clone(), e.text.clone()), Some(e)))
        .collect();
    ops.sort_by_key(|e| e.seq);
    ops.dedup_by_key(|e| e.seq);
    let mut by_key: BTreeMap<(String, String), Vec<&Entry>> = BTreeMap::new();
    for op in ops {
        by_key
            .entry((op.code.clone(), op.text.clone()))
            .or_default()
            .push(op);
    }
    for ((code, text), ks) in by_key {
        let b = map.remove(&(code.clone(), text.clone())).flatten();
        let merged = apply_ops(b, &ks, &code, &text);
        map.insert((code, text), merged);
    }
    map.into_values().flatten().collect()
}
```

`merge_for_fold` 换成：

```rust
fn merge_for_fold(
    snapshot: Option<&Snapshot>,
    delta: &Delta,
    target: Target,
    schema: &str,
    upto: u64,
) -> Vec<SnapshotEntry> {
    let mut base = Vec::new();
    if let Some(s) = snapshot {
        s.for_each(&mut |e| base.push(e));
    }
    let ops: Vec<&Entry> = delta
        .prefix(target, schema, "")
        .into_iter()
        .filter(|e| e.seq <= upto)
        .collect();
    overlay_entries(base, ops)
}
```

删掉旧的 `fn overlay(...)`。`apply` 改为委托，`apply_batch` 先把每条转换成 `Entry`（转不了就整批拒，不写任何一条），再在写锁下逐条 `append`、最后一次发布：

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        target: Target,
        schema: &str,
        kind: OpKind,
        code: &str,
        text: &str,
        payload: Payload,
        boundary: u64,
    ) -> anyhow::Result<u64> {
        let seqs = self.apply_batch(&[Op::new(target, schema, kind, code, text, payload, boundary)])?;
        Ok(seqs[0])
    }

    /// 一批操作：先全部校验（无增量语义的整批拒、不占号），再逐条追加（各一次 write）、
    /// 最后发布一次视图。写锁跨「追加 + 发布」，与 fold 取水位互斥。
    pub fn apply_batch(&self, ops: &[Op]) -> anyhow::Result<Vec<u64>> {
        if self.readonly() {
            bail!(
                "只读模式：另一个实例持有 {}",
                self.dirs.state_dir.join("lock").display()
            );
        }
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = self.journal.lock().unwrap_or_else(|e| e.into_inner());
        let Some(j) = guard.as_mut() else {
            bail!("只读模式");
        };
        let now = now_ms();
        let mut recs = Vec::with_capacity(ops.len());
        let mut entries = Vec::with_capacity(ops.len());
        for (i, op) in ops.iter().enumerate() {
            let rec = Record {
                seq: j.next_seq() + i as u64,
                ts_ms: if op.ts_ms == 0 { now } else { op.ts_ms },
                kind: op.kind,
                target: op.target,
                schema: op.schema.clone(),
                code: op.code.clone(),
                text: op.text.clone(),
                payload: op.payload.clone(),
                boundary: op.boundary,
            };
            rec.check_lens()?;
            let Some(e) = entry_from_record(&rec) else {
                bail!("操作 {:?} 没有增量语义，不写入", op.kind);
            };
            recs.push(rec);
            entries.push(e);
        }
        let mut seqs = Vec::with_capacity(recs.len());
        for rec in &recs {
            j.append(rec)?;
            seqs.push(rec.seq);
        }
        let cur = self.view();
        let mut delta = (*cur.delta).clone();
        for e in entries {
            delta = delta.with(e);
        }
        self.publish(View {
            snapshots: cur.snapshots.clone(),
            delta: Arc::new(delta),
        });
        Ok(seqs)
    }
```

（`Delta` 已 `#[derive(Clone)]`，克隆只复制两个 `Arc` 与尾巴。）把 `query_exact` / `query_count` 换成：

```rust
    /// 某键叠加后的条目（快照 + delta）。
    pub fn get(&self, target: Target, schema: &str, code: &str, text: &str) -> Option<SnapshotEntry> {
        let v = self.view();
        let base = v
            .snapshots
            .get(&(target, schema.to_string()))
            .and_then(|s| s.exact(code).into_iter().find(|e| e.text == text));
        apply_ops(base, &v.delta.ops_for(target, schema, code, text), code, text)
    }

    /// 某码下叠加后的全部条目，按 text 升序。
    pub fn exact(&self, target: Target, schema: &str, code: &str) -> Vec<SnapshotEntry> {
        let v = self.view();
        let base = v
            .snapshots
            .get(&(target, schema.to_string()))
            .map(|s| s.exact(code))
            .unwrap_or_default();
        overlay_entries(base, v.delta.exact(target, schema, code))
    }

    /// 调试 / 测试用：`(text, weight)`，权重降序、同权按 text。
    pub fn query_exact(&self, target: Target, schema: &str, code: &str) -> Vec<(String, i32)> {
        let mut v: Vec<(String, i32)> = self
            .exact(target, schema, code)
            .into_iter()
            .map(|e| (e.text, e.weight))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
```

`snapshot.rs`：加条目装配辅助，`exact` / `prefix` / `abbrev` 改返回条目，新增 `for_each_prefix`：

```rust
    fn entry(&self, code: &str, text: String, weight: i32, boundary: u64, idx: u32) -> SnapshotEntry {
        let (count, created_at, last_used, flags) = self.meta_of(idx);
        SnapshotEntry {
            code: code.to_string(),
            text,
            weight,
            boundary,
            count,
            created_at,
            last_used,
            flags,
        }
    }

    /// 同码按权重降序（建段时的叶内序）。
    pub fn exact(&self, code: &str) -> Vec<SnapshotEntry> {
        self.reader
            .search(code)
            .into_iter()
            .map(|e| {
                let idx = self.idx_of(code, e.order);
                self.entry(code, e.text, e.weight, e.boundary, idx)
            })
            .collect()
    }

    /// `limit == 0` 由 [`Self::for_each_prefix`] 负责（`WdatReader::search_prefix(…, 0)` 返回空）。
    pub fn prefix(&self, prefix: &str, limit: usize) -> Vec<SnapshotEntry> {
        if limit == 0 {
            let mut v = Vec::new();
            self.for_each_prefix(prefix, &mut |e| {
                v.push(e);
                true
            });
            return v;
        }
        self.reader
            .search_prefix(prefix, limit)
            .into_iter()
            .map(|e| {
                let idx = self.idx_of(&e.code, e.order);
                self.entry(&e.code, e.text, e.weight, e.boundary, idx)
            })
            .collect()
    }

    pub fn abbrev(&self, abbrev: &str, limit: usize) -> Vec<SnapshotEntry> {
        let mut out = Vec::new();
        for a in self.reader.search_abbrev(abbrev, 0) {
            out.extend(self.exact(&a.text));
        }
        out.sort_by(|x, y| y.weight.cmp(&x.weight).then(x.code.cmp(&y.code)));
        if limit > 0 {
            out.truncate(limit);
        }
        out
    }

    /// 按 (code, 叶内序) 顺序遍历 code 以 `prefix` 开头的条目；`f` 返回 false 即停。
    pub fn for_each_prefix(&self, prefix: &str, f: &mut dyn FnMut(SnapshotEntry) -> bool) {
        for (code, leaf) in self.reader.codes() {
            if !code.starts_with(prefix) {
                if code.as_str() > prefix {
                    break;
                }
                continue;
            }
            let first = self.first_idx.get(&code).copied().unwrap_or(0);
            for (i, (text, weight, _order, boundary)) in
                self.reader.leaf_entries(leaf).into_iter().enumerate()
            {
                if !f(self.entry(&code, text.to_string(), weight, boundary, first + i as u32)) {
                    return;
                }
            }
        }
    }
```

（`code.as_str() > prefix` 且不以 prefix 开头 ⇒ 字节序已越过前缀区间，可以停。）`snapshot.rs` 自己的测试里对 `exact` / `abbrev` 返回值的 `.0` / `.1` / `.3` 访问改成 `.text` / `.weight` / `meta_of` 不再需要（条目已含 count），例如 `build_open_query_and_meta_roundtrip` 中：

```rust
        let hits = s.exact("nihao");
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec!["你好", "拟好"],
            "同码按权重降序"
        );
        assert_eq!((hits[0].count, hits[0].created_at, hits[0].last_used), (3, 100, 200));
        assert_eq!(hits[1].count, 1, "sidecar 与条目一一对齐");
```

`abbrev_keys_cover_every_boundary_under_one_code` 里的 `h.1` 改 `h.text`。

`layer.rs`：删掉 `overlay` 与 `DELTA_ORDER`，`candidate` 去掉 `order` 参数、`natural_order` 恒 0（与 `StoreTempLayer` 一致：redb 临时词 value 里没有 order，`record_to_candidate` 给的就是 0），并改为：

```rust
    fn candidates(entries: Vec<SnapshotEntry>, query_code: Option<&str>) -> Vec<Candidate> {
        entries
            .into_iter()
            .map(|e| {
                let is_prefix = query_code.is_some_and(|q| q != e.code);
                Self::candidate(&e.code, &e.text, e.weight, e.boundary, is_prefix)
            })
            .collect()
    }

    fn abbrev_groups(&self, groups: &[String], limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let snap = v.snapshots.get(&(Target::Temp, self.schema.clone()));
        let mut base = Vec::new();
        let mut keys: Vec<(String, String)> = Vec::new();
        for g in groups {
            if let Some(s) = snap {
                base.extend(s.abbrev(g, 0));
            }
            keys.extend(
                v.delta
                    .by_group(Target::Temp, &self.schema, g)
                    .into_iter()
                    .map(|e| (e.code.clone(), e.text.clone())),
            );
        }
        keys.extend(base.iter().map(|e| (e.code.clone(), e.text.clone())));
        keys.sort();
        keys.dedup();
        // 每个涉及的键补齐它在 delta 里的全部操作（Incr 等可能落在别的分组）
        let ops: Vec<&Entry> = keys
            .iter()
            .flat_map(|(c, t)| v.delta.ops_for(Target::Temp, &self.schema, c, t))
            .collect();
        Self::sort_trunc(Self::candidates(overlay_entries(base, ops), None), limit)
    }
```

`search` / `search_prefix`：

```rust
    fn search(&self, code: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let base = v
            .snapshots
            .get(&(Target::Temp, self.schema.clone()))
            .map(|s| s.exact(code))
            .unwrap_or_default();
        let ops = v.delta.exact(Target::Temp, &self.schema, code);
        Self::sort_trunc(Self::candidates(overlay_entries(base, ops), Some(code)), limit)
    }

    fn search_prefix(&self, prefix: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let ops = v.delta.prefix(Target::Temp, &self.schema, prefix);
        // 每段多取「delta 覆盖的条数」：被删条目占名额、被调高的条目可能不在前 limit 里。
        // limit == 0 = 不限（Snapshot::prefix 自己处理）。
        let take = if limit == 0 { 0 } else { limit + ops.len() };
        let base = v
            .snapshots
            .get(&(Target::Temp, self.schema.clone()))
            .map(|s| s.prefix(prefix, take))
            .unwrap_or_default();
        Self::sort_trunc(Self::candidates(overlay_entries(base, ops), Some(prefix)), limit)
    }
```

`use` 行改为 `use crate::engine::{UserData, overlay_entries};` 与 `use crate::snapshot::SnapshotEntry;`，删去不再用的 `BTreeMap` / `DeltaOp`。若 `layer.rs` 现有测试断言了 `natural_order`，改为 0 并在账本记 `Task 1: Ruling`。

`tests/engine_crash_paths.rs` 与 `engine.rs` tests 里的 `query_count(t, s, c, x)` 换成 `get(t, s, c, x).map(|e| e.count).unwrap_or(0)`。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`，含新增三条。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 操作带时间、叠加语义唯一、apply_batch

created_at / last_used 取写入时刻；读路径、折回、层共用 apply_ops；无增量语义的操作写前拒；
search_prefix(…, 0) 不再漏快照条目。
EOF
```

---

### Task 2: delta 按键二分、按目标计数（M4）

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/delta.rs`
- Test: `delta.rs` tests 模块

**Interfaces:**
- Consumes: Task 1 的 `Entry { ts_ms }`。
- Produces:
  - `Delta::ops_for` 签名不变，冻结层改为二分（`O(log n + 尾巴)`）。
  - `Delta::pending(&self, target: Target, schema: &str) -> usize`
  - `Delta::targets(&self) -> std::collections::BTreeSet<(Target, String)>`
  - `Delta::min_seq(&self) -> Option<u64>`

- [ ] **Step 1: 写测试（delta.rs tests 末尾）**

```rust
    fn many() -> Delta {
        let mut d = Delta::default();
        let mut seq = 0;
        for i in 0..600u32 {
            seq += 1;
            let t = if i % 3 == 0 { Target::Freq } else { Target::Temp };
            let mut x = e(seq, &format!("c{}", i % 37), &format!("t{}", i % 11), DeltaOp::Incr(1));
            x.target = t;
            d = d.with(x);
        }
        d
    }

    /// M4 的特征测试：二分版 ops_for 与线性过滤结果逐条相同（冻结层 + 尾巴都覆盖到）。
    #[test]
    fn ops_for_matches_linear_filter() {
        let d = many();
        for c in 0..37 {
            for t in 0..11 {
                let (code, text) = (format!("c{c}"), format!("t{t}"));
                let mut want: Vec<u64> = d
                    .all()
                    .filter(|e| {
                        e.target == Target::Temp && e.schema == "py" && e.code == code && e.text == text
                    })
                    .map(|e| e.seq)
                    .collect();
                want.sort();
                let got: Vec<u64> = d
                    .ops_for(Target::Temp, "py", &code, &text)
                    .iter()
                    .map(|e| e.seq)
                    .collect();
                assert_eq!(got, want, "{code}/{text}");
            }
        }
    }

    #[test]
    fn pending_targets_and_min_seq() {
        let d = many();
        assert_eq!(d.pending(Target::Freq, "py"), 200);
        assert_eq!(d.pending(Target::Temp, "py"), 400);
        assert_eq!(d.pending(Target::Temp, "other"), 0);
        let ts: Vec<(Target, String)> = d.targets().into_iter().collect();
        assert_eq!(ts, vec![(Target::Temp, "py".into()), (Target::Freq, "py".into())]);
        assert_eq!(d.min_seq(), Some(1));
        assert_eq!(Delta::default().min_seq(), None);
    }
```

（`Target` 的 `Ord` 按声明序：`Temp = 1` 在 `Freq = 3` 之前。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata delta:: 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`pending` / `targets` / `min_seq` 未定义。（`ops_for_matches_linear_filter` 是重构前的特征测试，单独注释掉 `pending_targets_and_min_seq` 时应当通过；不需要看它红。）

- [ ] **Step 3: 实现**

```rust
    /// 某键的全部操作，按 seq 升序。冻结层按 (target, schema, code, text, seq) 有序，二分定位。
    pub fn ops_for(&self, target: Target, schema: &str, code: &str, text: &str) -> Vec<&Entry> {
        let lo = self
            .frozen
            .partition_point(|e| key(e) < (target, schema, code, text, 0));
        let mut v: Vec<&Entry> = self.frozen[lo..]
            .iter()
            .take_while(|e| e.target == target && e.schema == schema && e.code == code && e.text == text)
            .collect();
        v.extend(self.tail.iter().filter(|e| {
            e.target == target && e.schema == schema && e.code == code && e.text == text
        }));
        v.sort_by_key(|e| e.seq);
        v
    }

    /// 某 (目标, 方案) 未折回的操作条数（折回阈值判断用）。
    pub fn pending(&self, target: Target, schema: &str) -> usize {
        let lo = self
            .frozen
            .partition_point(|e| key(e) < (target, schema, "", "", 0));
        self.frozen[lo..]
            .iter()
            .take_while(|e| e.target == target && e.schema == schema)
            .count()
            + self
                .tail
                .iter()
                .filter(|e| e.target == target && e.schema == schema)
                .count()
    }

    /// 有未折回操作的 (目标, 方案)。
    pub fn targets(&self) -> std::collections::BTreeSet<(Target, String)> {
        self.all().map(|e| (e.target, e.schema.clone())).collect()
    }

    /// 最小的未折回 seq；空表 None。日志压实只能裁到它之前。
    pub fn min_seq(&self) -> Option<u64> {
        self.all().map(|e| e.seq).min()
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata/src/delta.rs <<'EOF'
perf(userdata): delta 按键二分，加 pending / targets / min_seq

按键路径逐个快照命中调 ops_for，线性扫在 4000 条 delta 时是百万级比较。
EOF
```

---

### Task 3: 空快照、`fold_with`、state 版本（M6）

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/snapshot.rs`（`reader: Option<…>`；零条目只写 sidecar；删 `remove`）
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`fold_with`、`fold_lock`；`fold` 委托；去掉空结果删文件的分支）
- Modify: `wind_input/crates/wind-userdata/src/state.rs`（`version`、`STATE_VERSION`；删 `TargetState::snapshot_fp`）
- Test: 三个文件的 tests 模块；`tests/engine_crash_paths.rs`

**Interfaces:**
- Consumes: Task 1 的 `overlay_entries`、`merge_for_fold`。
- Produces:
  - `UserData::fold_with(&self, target: Target, schema: &str, policy: &mut dyn FnMut(Vec<SnapshotEntry>) -> Vec<SnapshotEntry>) -> anyhow::Result<()>`：取水位 `upto` → 旧快照 + delta(≤upto) 叠加 → `policy` → 排序去重 → 建段（可为零条目）→ 换入 → 摘 delta → state。同一时刻只有一个折回在跑（`fold_lock`）。
  - `UserData::fold(&self, target, schema)` = `fold_with(…, &mut |v| v)`。
  - `snapshot::build` 接受零条目：只写 sidecar（`entry_count = 0`、`gen = 0`），清掉旧代号 wdat；`open` 对零条目 sidecar 给 `Some(空快照)`，`max_seq` 照常可读。
  - `pub const STATE_VERSION: u32 = 1;` `State { version: u32, … }`；`State::load` 遇版本不符返回 `None`（= 水位未知）。

- [ ] **Step 1: 写失败测试**

`snapshot.rs` tests：

```rust
    /// 零条目也是一个快照：它带着水位，清空之后 state 丢了也不会让旧记录复活。
    #[test]
    fn empty_build_keeps_watermark_and_drops_old_segment() {
        let d = std::env::temp_dir().join(format!("wind_snap_empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        drop(build(&d, "t", 3, vec![se("a", "甲", 1, 0, 1)].into_iter()).unwrap());
        let s = build(&d, "t", 7, std::iter::empty()).unwrap();
        assert_eq!((s.max_seq, s.entry_count), (7, 0));
        assert!(s.exact("a").is_empty());
        let wdats = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "wdat"))
            .count();
        assert_eq!(wdats, 0, "旧代号文件已清");
        let s = open(&d, "t").unwrap().expect("零条目 sidecar 也能打开");
        assert_eq!(s.max_seq, 7);
        let _ = std::fs::remove_dir_all(&d);
    }
```

`state.rs` tests（模块不存在就新建 `#[cfg(test)] mod tests { use super::*; … }`）：

```rust
    #[test]
    fn state_with_unknown_version_is_treated_as_missing() {
        let d = std::env::temp_dir().join(format!("wind_state_ver-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("state");
        let s = State {
            next_seq: 9,
            ..Default::default()
        };
        s.save(&p).unwrap();
        let back = State::load(&p).expect("同版本可读");
        assert_eq!((back.version, back.next_seq), (STATE_VERSION, 9));
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace(&format!("version = {STATE_VERSION}"), "version = 99");
        std::fs::write(&p, text).unwrap();
        assert!(State::load(&p).is_none(), "版本不符 = 水位未知");
        let _ = std::fs::remove_dir_all(&d);
    }
```

`tests/engine_crash_paths.rs`：

```rust
/// Review Focus 3：清空（fold_with 返回空）后 state 丢失再重开，被清空的词不复活。
#[test]
fn clear_survives_state_loss() {
    let d = dirs("clear");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "b", "乙");
    ud.fold_with(Target::Temp, "py", &mut |_| Vec::new()).unwrap();
    assert!(texts(&ud, "a").is_empty() && texts(&ud, "b").is_empty());
    drop(ud);
    std::fs::remove_file(d.state_dir.join("journal").join("state")).unwrap();
    let ud = reopen(&d);
    assert!(texts(&ud, "a").is_empty(), "快照里的被清空");
    assert!(texts(&ud, "b").is_empty(), "日志里的也不回放");
}

/// policy 看到的是叠加后的全集（快照 + delta），它的输出就是新快照。
#[test]
fn fold_with_policy_sees_merged_entries() {
    let d = dirs("policy");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "b", "乙");
    let mut seen = Vec::new();
    ud.fold_with(Target::Temp, "py", &mut |v| {
        seen = v.iter().map(|e| e.code.clone()).collect();
        v.into_iter().filter(|e| e.code == "a").collect()
    })
    .unwrap();
    assert_eq!(seen, vec!["a", "b"]);
    drop(ud);
    let ud = reopen(&d);
    assert_eq!(texts(&ud, "a"), vec!["甲"]);
    assert!(texts(&ud, "b").is_empty());
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`fold_with`、`STATE_VERSION`、`State::version` 未定义。

- [ ] **Step 3: 实现**

`snapshot.rs`：

1. `pub struct Snapshot` 的 `reader` 改 `reader: Option<Arc<WdatReader>>`（不再 `pub`），新增方法 `fn reader(&self) -> Option<&WdatReader> { self.reader.as_deref() }`；`exact` / `prefix` / `abbrev` / `for_each` / `for_each_prefix` 开头 `let Some(r) = self.reader() else { return …空… };`，把 `self.reader.` 换成 `r.`。
2. 抽出 sidecar 头：

```rust
fn meta_header(max_seq: u64, n: u32, generation: u64) -> Vec<u8> {
    let mut head = Vec::with_capacity(META_HEADER);
    head.extend_from_slice(META_MAGIC);
    head.extend_from_slice(&1u16.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&max_seq.to_le_bytes());
    head.extend_from_slice(&n.to_le_bytes());
    head.extend_from_slice(&generation.to_le_bytes());
    head.extend_from_slice(&[0u8; 4]);
    head
}
```

3. `build` 的 `if n == 0 { remove(dir, name); bail!(…) }` 换成：

```rust
    if n == 0 {
        // 零段也是快照：只有 sidecar（带水位），没有 wdat。
        crate::state::atomic_write(&meta_path(dir, name), &meta_header(max_seq, 0, 0))?;
        remove_stale_wdats(dir, name, None);
        return open(dir, name)?.ok_or_else(|| anyhow::anyhow!("刚建好的空段打不开"));
    }
```

   非空分支里组装 `head` 的几行换成 `let mut head = meta_header(max_seq, n, generation); head.extend_from_slice(&meta);`。删除 `pub fn remove`。

4. `open`：读出 `entry_count` 后若为 0，直接返回 `Ok(Some(Snapshot { reader: None, meta, max_seq, entry_count: 0, first_idx: BTreeMap::new() }))`，不要求 wdat 存在；否则走原逻辑（`reader: Some(reader)`）。注意读 `entry_count` 要挪到「找 wdat」之前。
5. 模块文档里「空快照不落盘」改为「零条目快照只有 sidecar（带水位）」。

`engine.rs`：

```rust
    /// 带策略的折回（§3.5 次序；§3.8 批量操作都走它）：旧快照 + delta(seq ≤ upto) 叠加 →
    /// `policy` → 新快照（可为零条目）→ 换入 → 摘 delta → state。新快照内嵌 `max_seq = upto`，
    /// 所以换入即推进水位：清空 / 导入 / 淘汰之后，被取代的旧记录无论 state 在不在都不再回放。
    pub fn fold_with(
        &self,
        target: Target,
        schema: &str,
        policy: &mut dyn FnMut(Vec<SnapshotEntry>) -> Vec<SnapshotEntry>,
    ) -> anyhow::Result<()> {
        if self.readonly() {
            bail!("只读模式");
        }
        let _f = self.fold_lock.lock().unwrap_or_else(|e| e.into_inner());
        // …（原 fold 中「取 upto 与 base」的块原样保留）
        let key = (target, schema.to_string());
        let merged = merge_for_fold(
            base.snapshots.get(&key).map(|s| s.as_ref()),
            &base.delta,
            target,
            schema,
            upto,
        );
        let mut out = policy(merged);
        out.sort_by(|a, b| (a.code.as_str(), a.text.as_str()).cmp(&(b.code.as_str(), b.text.as_str())));
        out.dedup_by(|a, b| a.code == b.code && a.text == b.text);
        let dir = self.dirs.state_dir.join(schema);
        let new_snap = Arc::new(crate::snapshot::build(&dir, target_name(target), upto, out.into_iter())?);
        {
            let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let cur = self.view();
            let mut snapshots = cur.snapshots.clone();
            snapshots.insert(key.clone(), new_snap);
            let delta = Arc::new(cur.delta.drop_folded_for_schema(target, schema, upto));
            self.publish(View { snapshots, delta });
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.targets.insert(
            target_key(target, schema),
            TargetState {
                folded_seq: upto,
                text_fp: None,
            },
        );
        st.next_seq = st.next_seq.max(upto + 1);
        st.save(&self.state_path)
    }

    pub fn fold(&self, target: Target, schema: &str) -> anyhow::Result<()> {
        self.fold_with(target, schema, &mut |v| v)
    }
```

`UserData` 加字段 `fold_lock: Mutex<()>`（`open` 里 `fold_lock: Mutex::new(())`）。原 `fold` 函数体整体被上面取代（`merged.is_empty()` 删文件的分支一并删除）。`st.next_seq = upto + 1` 改成取 max：折回期间若有新写入，`next_seq` 已更大。

`state.rs`：

```rust
/// state 文件格式版本；读到别的版本当「水位未知」（§3.6），由快照内嵌水位兜底。
pub const STATE_VERSION: u32 = 1;
```

`State` 加首字段 `#[serde(default)] pub version: u32,`；`TargetState` 删 `snapshot_fp`。`load` 在解析成功后加 `if s.version != STATE_VERSION { return None; }`；`save` 写入前 `let mut s = self.clone(); s.version = STATE_VERSION;` 再序列化 `s`。（`toml` 反序列化默认忽略未知字段，老文件里的 `snapshot_fp` 不会报错；但老文件没有 `version` ⇒ 0 ⇒ 当水位未知——格式未发布，可接受。）

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`。`fold_keeps_ops_that_arrived_during_fold` 再连跑 10 次：`for i in $(seq 10); do TMPDIR=<独立临时目录> cargo test -q -p wind-userdata --test engine_crash_paths fold_keeps 2>&1 | grep -c "1 passed"; done | sort | uniq -c` → `10 1`。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): fold_with（带策略折回）与零条目快照；state 带版本

清空、导入、淘汰都经 fold_with 原子换段推进水位；零条目快照只留带水位的 sidecar，
state 丢了被清空的记录也不回放。
EOF
```

---

### Task 4: 扫描、日志压实、后台折回

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`
- Test: `engine.rs` tests；`tests/engine_crash_paths.rs`

**Interfaces:**
- Consumes: Task 2 的 `Delta::{pending, targets, min_seq}`；Task 3 的 `fold`。
- Produces:
  - `UserData::scan(&self, target, schema: &str, prefix: &str, f: &mut dyn FnMut(SnapshotEntry) -> bool)`：叠加后按 `(code, text)` 升序。
  - `UserData::keys(&self) -> BTreeSet<(Target, String)>`：有数据（快照非空或 delta 有操作）的 (目标, 方案)。
  - `UserData::pending(&self, target, schema: &str) -> usize`
  - `UserData::compact(&self) -> anyhow::Result<()>`：折回所有有待处理操作的目标 → 在写锁下把日志裁到 `min(delta.min_seq, next_seq)` 之前 → 删尾 seq 低于该点的 `.wjl.sealed`。
  - `pub const FOLD_THRESHOLD: usize = 4000;` `pub const JOURNAL_MAX_BYTES: u64 = 8 << 20;`
  - `UserData::maintain_with(&self, threshold: usize, max_bytes: u64) -> anyhow::Result<usize>`：折回 `pending ≥ threshold` 的目标，日志超 `max_bytes` 则 `compact`；返回折回的目标数。`maintain()` = `maintain_with(FOLD_THRESHOLD, JOURNAL_MAX_BYTES)`。
  - `UserData::needs_maintain(&self) -> bool`（廉价判断）。
  - `UserData::spawn_maintain(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>>`：只读或已有一个在跑 ⇒ `None`；否则起名为 `userdata-fold` 的线程跑 `maintain()`。

- [ ] **Step 1: 写失败测试（`tests/engine_crash_paths.rs` 末尾）**

```rust
fn journal_records(d: &Dirs) -> usize {
    let jdir = d.state_dir.join("journal");
    std::fs::read_dir(&jdir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wjl"))
        .map(|p| wind_userdata::journal::Journal::replay(&p, 0).unwrap().len())
        .sum()
}

#[test]
fn compact_prunes_journal_and_keeps_everything_visible() {
    let d = dirs("compact");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    up(&ud, "b", "乙");
    ud.compact().unwrap();
    assert_eq!(journal_records(&d), 0, "全部折回后日志清空");
    up(&ud, "c", "丙");
    drop(ud);
    let ud = reopen(&d);
    for (c, t) in [("a", "甲"), ("b", "乙"), ("c", "丙")] {
        assert_eq!(texts(&ud, c), vec![t], "{c}");
    }
}

#[test]
fn compact_removes_sealed_journals_once_folded() {
    let d = dirs("compact_sealed");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    drop(ud);
    let d2 = Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: "dir-B".into(),
    };
    let ud = reopen(&d2);
    ud.compact().unwrap();
    let sealed = std::fs::read_dir(d.state_dir.join("journal"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".wjl.sealed"))
        .count();
    assert_eq!(sealed, 0);
    drop(ud);
    assert_eq!(texts(&reopen(&d2), "a"), vec!["甲"]);
}

#[test]
fn maintain_folds_only_targets_over_threshold() {
    let d = dirs("maintain");
    let ud = reopen(&d);
    for c in ["a", "b", "c"] {
        up(&ud, c, "字");
    }
    ud.apply(Target::Freq, "py", OpKind::Incr, "a", "字", Payload::Count(1), 0)
        .unwrap();
    assert!(ud.needs_maintain_with(3, u64::MAX));
    assert_eq!(ud.maintain_with(3, u64::MAX).unwrap(), 1);
    assert_eq!(ud.pending(Target::Temp, "py"), 0);
    assert_eq!(ud.pending(Target::Freq, "py"), 1);
    assert!(!ud.needs_maintain_with(3, u64::MAX));
}

#[test]
fn spawn_maintain_runs_in_background_and_is_single_flight() {
    let d = dirs("spawn");
    let ud = std::sync::Arc::new(reopen(&d));
    up(&ud, "a", "甲");
    let h = ud.spawn_maintain_with(1, u64::MAX).expect("第一个起得来");
    assert!(ud.spawn_maintain_with(1, u64::MAX).is_none() || ud.pending(Target::Temp, "py") == 0);
    h.join().unwrap();
    assert_eq!(ud.pending(Target::Temp, "py"), 0);
}

#[test]
fn scan_merges_snapshot_and_delta_in_key_order() {
    let d = dirs("scan");
    let ud = reopen(&d);
    up(&ud, "b", "乙");
    up(&ud, "d", "丁");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "a", "甲");
    up(&ud, "c", "丙");
    ud.apply(Target::Temp, "py", OpKind::Delete, "d", "丁", Payload::None, 0)
        .unwrap();
    let mut got = Vec::new();
    ud.scan(Target::Temp, "py", "", &mut |e| {
        got.push(e.code);
        true
    });
    assert_eq!(got, vec!["a", "b", "c"]);
    let keys: Vec<_> = ud.keys().into_iter().collect();
    assert_eq!(keys, vec![(Target::Temp, "py".to_string())]);
}
```

（`spawn_maintain_with` / `needs_maintain_with` 是带参数的版本，便于测试；无参版本用常量。第二个断言容许「第一个已经跑完」的竞态。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --test engine_crash_paths 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`compact` / `maintain_with` / `scan` / `keys` 等未定义；`journal` 模块若非 `pub` 也会报错（`lib.rs` 已是 `pub mod journal` 则不会）。

- [ ] **Step 3: 实现**

`engine.rs` 顶部常量：

```rust
/// 某 (目标, 方案) 未折回操作达到这么多条就折回（§3.5）。
pub const FOLD_THRESHOLD: usize = 4000;
/// 日志超过这么大就压实（§9：日志 ≤ 8 MB）。
pub const JOURNAL_MAX_BYTES: u64 = 8 << 20;
```

`UserData` 加字段 `jpath: Option<PathBuf>`（open 里已有的 `jpath` 存进去）与 `maintaining: std::sync::atomic::AtomicBool`。方法：

```rust
    pub fn scan(
        &self,
        target: Target,
        schema: &str,
        prefix: &str,
        f: &mut dyn FnMut(SnapshotEntry) -> bool,
    ) {
        let v = self.view();
        let mut base = Vec::new();
        if let Some(s) = v.snapshots.get(&(target, schema.to_string())) {
            s.for_each_prefix(prefix, &mut |e| {
                base.push(e);
                true
            });
        }
        for e in overlay_entries(base, v.delta.prefix(target, schema, prefix)) {
            if !f(e) {
                break;
            }
        }
    }

    pub fn keys(&self) -> BTreeSet<(Target, String)> {
        let v = self.view();
        let mut out = v.delta.targets();
        out.extend(
            v.snapshots
                .iter()
                .filter(|(_, s)| s.entry_count > 0)
                .map(|(k, _)| k.clone()),
        );
        out
    }

    pub fn pending(&self, target: Target, schema: &str) -> usize {
        self.view().delta.pending(target, schema)
    }

    /// 折回全部有待处理操作的目标，再把日志裁到「最小未折回 seq」之前，并删掉已全部折回的封存日志。
    pub fn compact(&self) -> anyhow::Result<()> {
        if self.readonly() {
            bail!("只读模式");
        }
        for (t, s) in self.view().delta.targets() {
            self.fold(t, &s)?;
        }
        // 写锁下裁剪：裁剪是重写 + rename，期间不能有追加。
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = self.journal.lock().unwrap_or_else(|e| e.into_inner());
        let Some(j) = guard.as_mut() else {
            bail!("只读模式");
        };
        let keep_from = self.view().delta.min_seq().unwrap_or(u64::MAX).min(j.next_seq());
        j.prune_before(keep_from)?;
        let jdir = self.dirs.state_dir.join("journal");
        for p in std::fs::read_dir(&jdir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_str().is_some_and(|s| s.ends_with(".wjl.sealed")))
        {
            if Journal::tail_seq(&p).map_or(true, |t| t < keep_from) {
                let _ = std::fs::remove_file(&p);
            }
        }
        Ok(())
    }

    fn journal_len(&self) -> u64 {
        self.jpath
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len())
    }

    pub fn needs_maintain_with(&self, threshold: usize, max_bytes: u64) -> bool {
        if self.readonly() {
            return false;
        }
        let v = self.view();
        v.delta
            .targets()
            .iter()
            .any(|(t, s)| v.delta.pending(*t, s) >= threshold)
            || self.journal_len() > max_bytes
    }

    pub fn needs_maintain(&self) -> bool {
        self.needs_maintain_with(FOLD_THRESHOLD, JOURNAL_MAX_BYTES)
    }

    pub fn maintain_with(&self, threshold: usize, max_bytes: u64) -> anyhow::Result<usize> {
        let mut n = 0;
        let v = self.view();
        for (t, s) in v.delta.targets() {
            if v.delta.pending(t, &s) >= threshold {
                self.fold(t, &s)?;
                n += 1;
            }
        }
        if self.journal_len() > max_bytes {
            self.compact()?;
        }
        Ok(n)
    }

    pub fn maintain(&self) -> anyhow::Result<usize> {
        self.maintain_with(FOLD_THRESHOLD, JOURNAL_MAX_BYTES)
    }

    /// 后台折回（§3.5：折回在独立线程）。只读或已有一个在跑 ⇒ None。
    pub fn spawn_maintain_with(
        self: &Arc<Self>,
        threshold: usize,
        max_bytes: u64,
    ) -> Option<std::thread::JoinHandle<()>> {
        use std::sync::atomic::Ordering;
        if self.readonly() || self.maintaining.swap(true, Ordering::AcqRel) {
            return None;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("userdata-fold".into())
            .spawn(move || {
                if let Err(e) = me.maintain_with(threshold, max_bytes) {
                    tracing::warn!("用户数据折回失败: {e}");
                }
                me.maintaining.store(false, Ordering::Release);
            });
        match spawned {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!("起折回线程失败: {e}");
                self.maintaining.store(false, Ordering::Release);
                None
            }
        }
    }

    pub fn spawn_maintain(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
        self.spawn_maintain_with(FOLD_THRESHOLD, JOURNAL_MAX_BYTES)
    }
```

`compact` 里 `fold` 与 `write_lock` 不嵌套（fold 内部自己拿写锁、在返回前放掉），不会死锁。折回期间到达的新写入 seq 必然 > 该次折回的 `upto`，留在 delta 里，`min_seq` 保住它们。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): scan / keys、日志压实与后台折回

按条数阈值折回、日志超限压实；压实只裁到最小未折回 seq，并清掉已全部折回的封存日志。
EOF
```

---

### Task 5: `Runtime` 与临时词 API

**Files:**
- Create: `wind_input/crates/wind-userdata/src/runtime/mod.rs`
- Create: `wind_input/crates/wind-userdata/src/runtime/temp.rs`
- Modify: `wind_input/crates/wind-userdata/src/lib.rs`（`pub mod runtime;`）
- Test: `wind_input/crates/wind-userdata/tests/runtime_temp.rs`

**Interfaces:**
- Consumes: `UserData::{apply_batch, get, exact, scan, fold_with, keys}`、`Op`。
- Produces（`runtime/mod.rs`）：
  - `pub struct Runtime`；`Runtime::new(ud: Arc<UserData>) -> Runtime`；`Runtime::data(&self) -> &Arc<UserData>`
  - `Runtime::words_gen(&self, schema: &str) -> u64`（新增 / 删除 / 清空 / 导入 / 淘汰到 / 草稿转正时 +1）、`Runtime::count_gen(&self) -> u64`（临时词 count+1 时 +1）——对应 `Store::bump_words_gen` / `bump_words_count_gen`，P1b 的 `UserAssocIndex` 用。
  - `Runtime::schemas(&self) -> BTreeSet<String>`
  - `pub(crate) fn now_secs() -> i64`
- Produces（`runtime/temp.rs`）：
  - `pub const TEMP_WORD_MAX_WEIGHT: i32 = 10000;`
  - `pub struct TempWord { pub code: String, pub text: String, pub weight: i32, pub count: u32, pub created_at: i64, pub boundary: u64 }`
  - `learn_temp(&self, schema, code, text, add_weight: i32, boundary: u64) -> anyhow::Result<u32>`
  - `temp_word(&self, schema, code, text) -> Option<TempWord>`
  - `increment_temp_if_exists(&self, schema, code, text) -> anyhow::Result<(bool, u32)>`
  - `temp_words(&self, schema, code) -> Vec<TempWord>`
  - `for_each_temp(&self, schema, prefix, f: &mut dyn FnMut(&TempWord) -> bool)`
  - `count_temp(&self, schema) -> usize`
  - `remove_temp(&self, schema, code, text) -> anyhow::Result<()>`
  - `clear_temp(&self, schema) -> anyhow::Result<usize>`
  - `evict_temp(&self, schema, max_keep: usize) -> anyhow::Result<usize>`
  - `import_temp_rows(&self, schema, rows: &[TempWord]) -> anyhow::Result<usize>`
  - `pub(crate) fn temp_learn_ops(&self, schema, code, text, add_weight, boundary) -> (Vec<Op>, u32, bool)`（ops、学后的 count、是否新词；草稿转正复用）

语义照 `wind-store/src/temp_words.rs`：新词 `weight = min(add_weight, MAX)`、count 1；已存在只 count+1、权重不变；旧 boundary 为 0 且新值非 0 时补上。淘汰按 `(count, created_at, weight)` 升序删前 `len - max_keep` 条，单条 Delete（按键路径上一次只删几条，不值得整段重建）。导入：已存在则 `weight = max(旧, min(行, MAX))`、`count = max`、boundary 补齐；不存在则 `created_at = now`。晋升（`promote_temp_word`）**不在这里**：P1 期间用户词仍在 redb，P1b 由协调器「读 `temp_word` → 写 redb 用户词 → `remove_temp`」三步完成（先写用户词，崩溃最多两边都有，用户层去重兜得住）。

- [ ] **Step 1: 写失败测试 `tests/runtime_temp.rs`**

```rust
//! 临时词 API 与 wind-store/src/temp_words.rs 的行为对拍。
use std::sync::Arc;
use wind_userdata::engine::{Dirs, UserData};
use wind_userdata::record::{OpKind, Payload, Target};
use wind_userdata::runtime::{Runtime, TEMP_WORD_MAX_WEIGHT};

fn rt(name: &str) -> Runtime {
    let d = std::env::temp_dir().join(format!("wind_rt_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Runtime::new(Arc::new(
        UserData::open(Dirs {
            state_dir: d,
            dir_id: "dir".into(),
        })
        .unwrap(),
    ))
}

/// 对照 test_learn_count_only_weight_unchanged：再学只加 count，权重不变。
#[test]
fn learn_new_then_existing_counts_and_keeps_weight() {
    let r = rt("learn");
    assert_eq!(r.learn_temp("py", "nihao", "你好", 800, 0b101).unwrap(), 1);
    assert_eq!(r.learn_temp("py", "nihao", "你好", 50, 0b101).unwrap(), 2);
    let w = r.temp_word("py", "nihao", "你好").unwrap();
    assert_eq!((w.weight, w.count, w.boundary), (800, 2, 0b101));
}

#[test]
fn learn_caps_weight_and_backfills_zero_boundary_only() {
    let r = rt("cap");
    r.learn_temp("py", "xian", "先", 99_999, 0).unwrap();
    assert_eq!(r.temp_word("py", "xian", "先").unwrap().weight, TEMP_WORD_MAX_WEIGHT);
    r.learn_temp("py", "xian", "先", 1, 0b1).unwrap();
    assert_eq!(r.temp_word("py", "xian", "先").unwrap().boundary, 0b1, "旧为 0 时补上");
    r.learn_temp("py", "xian", "先", 1, 0b101).unwrap();
    assert_eq!(r.temp_word("py", "xian", "先").unwrap().boundary, 0b1, "已有则沿用");
}

/// 对照 test_increment_if_exists。
#[test]
fn increment_only_when_present() {
    let r = rt("incr");
    assert_eq!(r.increment_temp_if_exists("py", "a", "甲").unwrap(), (false, 0));
    assert!(r.temp_word("py", "a", "甲").is_none(), "不创建");
    r.learn_temp("py", "a", "甲", 10, 0).unwrap();
    assert_eq!(r.increment_temp_if_exists("py", "a", "甲").unwrap(), (true, 2));
}

/// Review Focus 4：折回后再学，count 从快照里的计数接着加。
#[test]
fn count_continues_from_snapshot_after_fold() {
    let r = rt("fold");
    r.learn_temp("py", "a", "甲", 10, 0).unwrap();
    r.learn_temp("py", "a", "甲", 10, 0).unwrap();
    r.data().fold(Target::Temp, "py").unwrap();
    assert_eq!(r.learn_temp("py", "a", "甲", 10, 0).unwrap(), 3);
}

/// Review Focus 5 + 对照 evict_keeps_the_used_word_when_weights_are_all_equal /
/// evict_falls_back_to_created_at_when_count_and_weight_tie。
#[test]
fn evict_orders_across_snapshot_and_delta() {
    let r = rt("evict");
    let ud = r.data().clone();
    // old: count 1，created_at 最早，在快照里
    let mut a = wind_userdata::engine::Op::new(Target::Temp, "py", OpKind::Upsert, "a", "旧", Payload::Weight(5), 0);
    a.ts_ms = 1_000_000;
    let mut ai = wind_userdata::engine::Op::new(Target::Temp, "py", OpKind::Incr, "a", "旧", Payload::Count(1), 0);
    ai.ts_ms = 1_000_000;
    ud.apply_batch(&[a, ai]).unwrap();
    ud.fold(Target::Temp, "py").unwrap();
    // used: count 3，在 delta 里
    for _ in 0..3 {
        r.learn_temp("py", "b", "常用", 5, 0).unwrap();
    }
    // new: count 1，较新，在 delta 里
    r.learn_temp("py", "c", "新", 5, 0).unwrap();
    let g = r.words_gen("py");
    assert_eq!(r.evict_temp("py", 2).unwrap(), 1);
    assert!(r.temp_word("py", "a", "旧").is_none(), "count 相同先删最老的");
    assert!(r.temp_word("py", "b", "常用").is_some() && r.temp_word("py", "c", "新").is_some());
    assert!(r.words_gen("py") > g);
    let g = r.words_gen("py");
    assert_eq!(r.evict_temp("py", 2).unwrap(), 0);
    assert_eq!(r.words_gen("py"), g, "没删到不动代次");
}

#[test]
fn listing_count_remove_clear() {
    let r = rt("list");
    r.learn_temp("py", "ni", "你", 5, 0b1).unwrap();
    r.learn_temp("py", "nihao", "你好", 5, 0b101).unwrap();
    r.learn_temp("py", "wo", "我", 5, 0b1).unwrap();
    r.learn_temp("other", "ni", "你", 5, 0b1).unwrap();
    assert_eq!(r.temp_words("py", "ni").len(), 1);
    let mut seen = Vec::new();
    r.for_each_temp("py", "ni", &mut |w| {
        seen.push(w.code.clone());
        true
    });
    assert_eq!(seen, vec!["ni", "nihao"]);
    assert_eq!(r.count_temp("py"), 3);
    r.remove_temp("py", "wo", "我").unwrap();
    assert_eq!(r.count_temp("py"), 2);
    assert_eq!(r.clear_temp("py").unwrap(), 2);
    assert_eq!(r.count_temp("py"), 0);
    assert_eq!(r.count_temp("other"), 1, "方案隔离");
}

/// 对照 import_temp_word_rows：取 max、补边界、不存在则新建。
#[test]
fn import_merges_max_and_backfills_boundary() {
    let r = rt("import");
    r.learn_temp("py", "xian", "先", 100, 0).unwrap();
    let row = |code: &str, text: &str, weight, count, boundary| wind_userdata::runtime::TempWord {
        code: code.into(),
        text: text.into(),
        weight,
        count,
        created_at: 0,
        boundary,
    };
    let n = r
        .import_temp_rows(
            "py",
            &[row("xian", "先", 50, 7, 0b1), row("xian", "西安", 99_999, 2, 0b101)],
        )
        .unwrap();
    assert_eq!(n, 2);
    let a = r.temp_word("py", "xian", "先").unwrap();
    assert_eq!((a.weight, a.count, a.boundary), (100, 7, 0b1));
    let b = r.temp_word("py", "xian", "西安").unwrap();
    assert_eq!((b.weight, b.count), (TEMP_WORD_MAX_WEIGHT, 2));
    assert!(b.created_at > 0, "新建条目 created_at = 现在");
}

#[test]
fn generations_follow_store_semantics() {
    let r = rt("gen");
    let (w0, c0) = (r.words_gen("py"), r.count_gen());
    r.learn_temp("py", "a", "甲", 1, 0).unwrap();
    assert!(r.words_gen("py") > w0, "新词推进 words_gen");
    let (w1, c1) = (r.words_gen("py"), r.count_gen());
    assert_eq!(c1, c0);
    r.learn_temp("py", "a", "甲", 1, 0).unwrap();
    assert_eq!(r.words_gen("py"), w1, "count+1 不推进 words_gen");
    assert!(r.count_gen() > c1);
    assert_eq!(r.schemas().into_iter().collect::<Vec<_>>(), vec!["py".to_string()]);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --test runtime_temp 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`wind_userdata::runtime` 不存在。

- [ ] **Step 3: 实现**

`src/lib.rs` 加 `pub mod runtime;`。

`src/runtime/mod.rs`：

```rust
//! 运行时领域 API（设计 §6 的 temp / draft / freq / assoc 四张表在引擎上的替身）。
//!
//! 语义逐条照 `wind-store` 的同名函数（`temp_words.rs` / `draft_words.rs` / `freq.rs` /
//! `assoc_history.rs`），时间单位同为 unix 秒。按键路径上的单条写入走 `apply_batch`；清空、
//! 导入、草稿过期 / 淘汰这类批量操作走 `fold_with`（§3.8）。本模块不依赖 `wind-store`。

mod draft;
mod freq;
mod temp;

pub use draft::draft_alive;
pub use freq::FreqRec;
pub use temp::{TEMP_WORD_MAX_WEIGHT, TempWord};

use crate::engine::UserData;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct Runtime {
    ud: Arc<UserData>,
    words_gen: Mutex<HashMap<String, u64>>,
    count_gen: AtomicU64,
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Runtime {
    pub fn new(ud: Arc<UserData>) -> Runtime {
        Runtime {
            ud,
            words_gen: Mutex::new(HashMap::new()),
            count_gen: AtomicU64::new(0),
        }
    }

    pub fn data(&self) -> &Arc<UserData> {
        &self.ud
    }

    /// 临时词「结构」代次（新增 / 删除 / 清空 / 导入 / 淘汰到 / 草稿转正）。进程内计数，重启归零。
    pub fn words_gen(&self, schema: &str) -> u64 {
        self.words_gen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(schema)
            .copied()
            .unwrap_or(0)
    }

    /// 临时词 count+1 的代次（不分方案，同 `Store::bump_words_count_gen`）。
    pub fn count_gen(&self) -> u64 {
        self.count_gen.load(Ordering::Acquire)
    }

    pub(crate) fn bump_words(&self, schema: &str) {
        *self
            .words_gen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(schema.to_string())
            .or_insert(0) += 1;
    }

    pub(crate) fn bump_count(&self) {
        self.count_gen.fetch_add(1, Ordering::AcqRel);
    }

    /// 有任何运行时数据的数据方案（备份按它逐个导出）。
    pub fn schemas(&self) -> BTreeSet<String> {
        self.ud.keys().into_iter().map(|(_, s)| s).collect()
    }
}
```

`src/runtime/temp.rs`：

```rust
//! 临时词（对照 `wind-store/src/temp_words.rs`）。

use super::{Runtime, now_secs};
use crate::engine::Op;
use crate::record::{OpKind, Payload, Target};
use crate::snapshot::SnapshotEntry;

pub const TEMP_WORD_MAX_WEIGHT: i32 = 10000;

#[derive(Clone, Debug, PartialEq)]
pub struct TempWord {
    pub code: String,
    pub text: String,
    pub weight: i32,
    pub count: u32,
    /// unix 秒
    pub created_at: i64,
    pub boundary: u64,
}

impl From<SnapshotEntry> for TempWord {
    fn from(e: SnapshotEntry) -> TempWord {
        TempWord {
            code: e.code,
            text: e.text,
            weight: e.weight,
            count: e.count,
            created_at: e.created_at,
            boundary: e.boundary,
        }
    }
}

const T: Target = Target::Temp;

impl Runtime {
    /// 学习一次的操作序列：新词 Upsert + Incr(1)；已有词 Incr(1)，旧边界为 0 时先 Upsert 补边界。
    pub(crate) fn temp_learn_ops(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        add_weight: i32,
        boundary: u64,
    ) -> (Vec<Op>, u32, bool) {
        match self.ud.get(T, schema, code, text) {
            Some(e) => {
                let b = if e.boundary != 0 { e.boundary } else { boundary };
                let mut ops = Vec::new();
                if e.boundary == 0 && boundary != 0 {
                    ops.push(Op::new(T, schema, OpKind::Upsert, code, text, Payload::Weight(e.weight), b));
                }
                ops.push(Op::new(T, schema, OpKind::Incr, code, text, Payload::Count(1), b));
                (ops, e.count.saturating_add(1), false)
            }
            None => (
                vec![
                    Op::new(
                        T,
                        schema,
                        OpKind::Upsert,
                        code,
                        text,
                        Payload::Weight(add_weight.min(TEMP_WORD_MAX_WEIGHT)),
                        boundary,
                    ),
                    Op::new(T, schema, OpKind::Incr, code, text, Payload::Count(1), boundary),
                ],
                1,
                true,
            ),
        }
    }

    /// 新词 weight = min(add_weight, MAX)、count 1；已存在只 count+1、权重不变。返回新 count。
    pub fn learn_temp(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        add_weight: i32,
        boundary: u64,
    ) -> anyhow::Result<u32> {
        let (ops, count, is_new) = self.temp_learn_ops(schema, code, text, add_weight, boundary);
        self.ud.apply_batch(&ops)?;
        if is_new {
            self.bump_words(schema);
        } else {
            self.bump_count();
        }
        Ok(count)
    }

    pub fn temp_word(&self, schema: &str, code: &str, text: &str) -> Option<TempWord> {
        self.ud.get(T, schema, code, text).map(TempWord::from)
    }

    pub fn increment_temp_if_exists(
        &self,
        schema: &str,
        code: &str,
        text: &str,
    ) -> anyhow::Result<(bool, u32)> {
        let Some(e) = self.ud.get(T, schema, code, text) else {
            return Ok((false, 0));
        };
        self.ud
            .apply(T, schema, OpKind::Incr, code, text, Payload::Count(1), e.boundary)?;
        self.bump_count();
        Ok((true, e.count.saturating_add(1)))
    }

    pub fn temp_words(&self, schema: &str, code: &str) -> Vec<TempWord> {
        self.ud.exact(T, schema, code).into_iter().map(TempWord::from).collect()
    }

    pub fn for_each_temp(&self, schema: &str, prefix: &str, f: &mut dyn FnMut(&TempWord) -> bool) {
        self.ud
            .scan(T, schema, prefix, &mut |e| f(&TempWord::from(e)));
    }

    pub fn count_temp(&self, schema: &str) -> usize {
        let mut n = 0;
        self.ud.scan(T, schema, "", &mut |_| {
            n += 1;
            true
        });
        n
    }

    pub fn remove_temp(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        if self.ud.get(T, schema, code, text).is_some() {
            self.ud
                .apply(T, schema, OpKind::Delete, code, text, Payload::None, 0)?;
            self.bump_words(schema);
        }
        Ok(())
    }

    pub fn clear_temp(&self, schema: &str) -> anyhow::Result<usize> {
        let mut n = 0;
        self.ud.fold_with(T, schema, &mut |v| {
            n = v.len();
            Vec::new()
        })?;
        self.bump_words(schema);
        Ok(n)
    }

    /// 超出 `max_keep` 则按 (count, created_at, weight) 升序删前若干条。
    pub fn evict_temp(&self, schema: &str, max_keep: usize) -> anyhow::Result<usize> {
        let mut all: Vec<SnapshotEntry> = Vec::new();
        self.ud.scan(T, schema, "", &mut |e| {
            all.push(e);
            true
        });
        if all.len() <= max_keep {
            return Ok(0);
        }
        all.sort_by_key(|e| (e.count, e.created_at, e.weight));
        let ops: Vec<Op> = all
            .iter()
            .take(all.len() - max_keep)
            .map(|e| Op::new(T, schema, OpKind::Delete, &e.code, &e.text, Payload::None, 0))
            .collect();
        self.ud.apply_batch(&ops)?;
        self.bump_words(schema);
        Ok(ops.len())
    }

    /// 导入（对照 `import_temp_word_rows`）：已存在取 max、补边界；不存在新建（created_at = 现在）。
    pub fn import_temp_rows(&self, schema: &str, rows: &[TempWord]) -> anyhow::Result<usize> {
        let now = now_secs();
        self.ud.fold_with(T, schema, &mut |v| {
            let mut map: std::collections::BTreeMap<(String, String), SnapshotEntry> = v
                .into_iter()
                .map(|e| ((e.code.clone(), e.text.clone()), e))
                .collect();
            for r in rows {
                let cap = r.weight.min(TEMP_WORD_MAX_WEIGHT);
                match map.get_mut(&(r.code.clone(), r.text.clone())) {
                    Some(e) => {
                        e.weight = e.weight.max(cap);
                        e.count = e.count.max(r.count);
                        if e.boundary == 0 {
                            e.boundary = r.boundary;
                        }
                    }
                    None => {
                        map.insert(
                            (r.code.clone(), r.text.clone()),
                            SnapshotEntry {
                                code: r.code.clone(),
                                text: r.text.clone(),
                                weight: cap,
                                boundary: r.boundary,
                                count: r.count,
                                created_at: now,
                                last_used: now,
                                flags: 0,
                            },
                        );
                    }
                }
            }
            map.into_values().collect()
        })?;
        self.bump_words(schema);
        Ok(rows.len())
    }
}
```

此时 `runtime/draft.rs` 与 `runtime/freq.rs` 还不存在：本任务先在 `mod.rs` 里**只**写 `mod temp; pub use temp::{TEMP_WORD_MAX_WEIGHT, TempWord};`，`draft` / `freq` 两行与对应 `pub use` 由 Task 6 / 7 加上。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`，`runtime_temp` 8 条。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/runtime wind_input/crates/wind-userdata/tests/runtime_temp.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): Runtime 与临时词 API（对拍 wind-store temp_words）
EOF
```

---

### Task 6: 草稿 API

**Files:**
- Create: `wind_input/crates/wind-userdata/src/runtime/draft.rs`
- Modify: `wind_input/crates/wind-userdata/src/runtime/mod.rs`（`mod draft; pub use draft::draft_alive;`）
- Test: `wind_input/crates/wind-userdata/tests/runtime_draft.rs`

**Interfaces:**
- Consumes: Task 5 的 `Runtime`、`temp_learn_ops`、`bump_words`。
- Produces：
  - `pub fn draft_alive(last_used: i64, now: i64, ttl_secs: i64) -> bool`（`ttl_secs <= 0` 恒真；层也用）
  - `add_drafts(&self, schema, items: &[(String, String)]) -> anyhow::Result<usize>`
  - `search_drafts(&self, schema, code, ttl_secs: i64) -> Vec<String>`
  - `has_draft(&self, schema, code, text, ttl_secs: i64) -> bool`
  - `purge_expired_drafts(&self, schema: Option<&str>, ttl_secs: i64) -> anyhow::Result<(usize, usize)>`（(删, 留)）
  - `evict_drafts(&self, schema, max_keep: usize) -> anyhow::Result<usize>`
  - `promote_draft_to_temp(&self, schema, code, text, add_weight: i32, boundary: u64) -> anyhow::Result<bool>`
  - `count_drafts(&self, schema) -> usize`、`clear_drafts(&self, schema) -> anyhow::Result<usize>`

语义照 `wind-store/src/draft_words.rs`，有效期基准从「`created_at`（重写即刷新）」换成 `last_used`：每次 `add_drafts` 都是 Upsert，叠加语义里 Upsert 刷新 `last_used`，效果等同「重写即续期」，又保留了真正的首次时间。过期 / 淘汰走 `fold_with`（后台 `draft-flush` 线程调用，可能一次删上千条）。

- [ ] **Step 1: 写失败测试 `tests/runtime_draft.rs`**

```rust
//! 草稿 API 与 wind-store/src/draft_words.rs 的行为对拍。
use std::sync::Arc;
use wind_userdata::engine::{Dirs, Op, UserData};
use wind_userdata::record::{OpKind, Payload, Target};
use wind_userdata::runtime::Runtime;

fn rt(name: &str) -> Runtime {
    let d = std::env::temp_dir().join(format!("wind_rtd_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Runtime::new(Arc::new(
        UserData::open(Dirs {
            state_dir: d,
            dir_id: "dir".into(),
        })
        .unwrap(),
    ))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// 以给定时刻写一条草稿（模拟「很久以前写的」）。
fn draft_at(r: &Runtime, schema: &str, code: &str, text: &str, secs: i64) {
    let mut op = Op::new(Target::Draft, schema, OpKind::Upsert, code, text, Payload::Weight(0), 0);
    op.ts_ms = secs * 1000;
    r.data().apply_batch(&[op]).unwrap();
}

const DAY: i64 = 86_400;

/// 对照 drafts_are_found_by_exact_code_only。
#[test]
fn drafts_are_found_by_exact_code_only() {
    let r = rt("exact");
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())]).unwrap();
    assert_eq!(r.search_drafts("wb", "ggtt", DAY), vec!["盖伦"]);
    assert!(r.search_drafts("wb", "ggt", DAY).is_empty());
    assert!(r.has_draft("wb", "ggtt", "盖伦", DAY));
}

/// Review Focus 2：过期草稿即使已折回进快照，查询时也被过滤。
#[test]
fn expired_draft_in_snapshot_is_hidden_at_query_time() {
    let r = rt("ttl");
    draft_at(&r, "wb", "aaaa", "旧", now() - 2 * DAY);
    r.add_drafts("wb", &[("aaaa".into(), "新".into())]).unwrap();
    r.data().fold(Target::Draft, "wb").unwrap();
    assert_eq!(r.search_drafts("wb", "aaaa", DAY), vec!["新"]);
    assert!(!r.has_draft("wb", "aaaa", "旧", DAY));
    assert_eq!(r.search_drafts("wb", "aaaa", 0).len(), 2, "ttl 0 = 永不过期");
}

/// 对照 rewriting_a_draft_renews_it。
#[test]
fn rewriting_a_draft_renews_it() {
    let r = rt("renew");
    draft_at(&r, "wb", "aaaa", "词", now() - 2 * DAY);
    assert!(r.search_drafts("wb", "aaaa", DAY).is_empty());
    r.add_drafts("wb", &[("aaaa".into(), "词".into())]).unwrap();
    assert_eq!(r.search_drafts("wb", "aaaa", DAY), vec!["词"]);
}

#[test]
fn purge_and_evict() {
    let r = rt("purge");
    draft_at(&r, "wb", "a", "过期", now() - 2 * DAY);
    draft_at(&r, "wb", "b", "较旧", now() - 100);
    r.add_drafts("wb", &[("c".into(), "新".into())]).unwrap();
    draft_at(&r, "py", "x", "过期", now() - 2 * DAY);
    assert_eq!(r.purge_expired_drafts(None, DAY).unwrap(), (2, 2), "跨方案");
    assert_eq!(r.count_drafts("wb"), 2);
    assert_eq!(r.evict_drafts("wb", 1).unwrap(), 1);
    assert!(r.has_draft("wb", "c", "新", DAY), "按 last_used 留最新的");
    assert_eq!(r.evict_drafts("wb", 1).unwrap(), 0);
    assert_eq!(r.purge_expired_drafts(Some("wb"), 0).unwrap(), (0, 0), "ttl 0 不清");
}

/// 对照 promote_draft_to_temp：草稿删除、临时词新建 count 1；已有临时词则 count+1。
#[test]
fn promote_draft_moves_it_into_temp() {
    let r = rt("promote");
    assert!(!r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0).unwrap(), "没有草稿");
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())]).unwrap();
    let g = r.words_gen("wb");
    assert!(r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0).unwrap());
    assert!(!r.has_draft("wb", "ggtt", "盖伦", 0));
    let t = r.temp_word("wb", "ggtt", "盖伦").unwrap();
    assert_eq!((t.weight, t.count), (800, 1));
    assert!(r.words_gen("wb") > g);
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())]).unwrap();
    r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0).unwrap();
    assert_eq!(r.temp_word("wb", "ggtt", "盖伦").unwrap().count, 2);
}

/// 对照 drafts_are_isolated_per_schema + clear。
#[test]
fn isolated_per_schema_and_clear() {
    let r = rt("iso");
    r.add_drafts("wb", &[("a".into(), "甲".into())]).unwrap();
    r.add_drafts("zm", &[("a".into(), "甲".into())]).unwrap();
    assert_eq!(r.clear_drafts("wb").unwrap(), 1);
    assert_eq!(r.count_drafts("wb"), 0);
    assert_eq!(r.count_drafts("zm"), 1);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --test runtime_draft 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`add_drafts` 等未定义。

- [ ] **Step 3: 实现 `src/runtime/draft.rs`**

```rust
//! 草稿（对照 `wind-store/src/draft_words.rs`）。有效期基准是 `last_used`：每次写入都是
//! Upsert，叠加语义里 Upsert 刷新 last_used，等同 redb 版「重写即续期」。

use super::{Runtime, now_secs};
use crate::engine::Op;
use crate::record::{OpKind, Payload, Target};
use crate::snapshot::SnapshotEntry;

const D: Target = Target::Draft;

/// 未过期。`ttl_secs <= 0` 表示永不过期。
pub fn draft_alive(last_used: i64, now: i64, ttl_secs: i64) -> bool {
    !(ttl_secs > 0 && now.saturating_sub(last_used) >= ttl_secs)
}

impl Runtime {
    pub fn add_drafts(&self, schema: &str, items: &[(String, String)]) -> anyhow::Result<usize> {
        if items.is_empty() {
            return Ok(0);
        }
        let ops: Vec<Op> = items
            .iter()
            .map(|(c, t)| Op::new(D, schema, OpKind::Upsert, c, t, Payload::Weight(0), 0))
            .collect();
        self.ud.apply_batch(&ops)?;
        Ok(items.len())
    }

    pub fn search_drafts(&self, schema: &str, code: &str, ttl_secs: i64) -> Vec<String> {
        let now = now_secs();
        self.ud
            .exact(D, schema, code)
            .into_iter()
            .filter(|e| draft_alive(e.last_used, now, ttl_secs))
            .map(|e| e.text)
            .collect()
    }

    pub fn has_draft(&self, schema: &str, code: &str, text: &str, ttl_secs: i64) -> bool {
        self.ud
            .get(D, schema, code, text)
            .is_some_and(|e| draft_alive(e.last_used, now_secs(), ttl_secs))
    }

    /// 清过期草稿；`schema = None` 扫所有方案。返回 (删, 留)。
    pub fn purge_expired_drafts(
        &self,
        schema: Option<&str>,
        ttl_secs: i64,
    ) -> anyhow::Result<(usize, usize)> {
        if ttl_secs <= 0 {
            return Ok((0, 0));
        }
        let schemas: Vec<String> = match schema {
            Some(s) => vec![s.to_string()],
            None => self
                .ud
                .keys()
                .into_iter()
                .filter(|(t, _)| *t == D)
                .map(|(_, s)| s)
                .collect(),
        };
        let now = now_secs();
        let (mut deleted, mut kept) = (0, 0);
        for s in schemas {
            self.ud.fold_with(D, &s, &mut |v: Vec<SnapshotEntry>| {
                let before = v.len();
                let alive: Vec<SnapshotEntry> = v
                    .into_iter()
                    .filter(|e| draft_alive(e.last_used, now, ttl_secs))
                    .collect();
                deleted += before - alive.len();
                kept += alive.len();
                alive
            })?;
        }
        Ok((deleted, kept))
    }

    /// 超出 `max_keep` 则按 last_used 升序淘汰（先数一遍，没超就不折回）。
    pub fn evict_drafts(&self, schema: &str, max_keep: usize) -> anyhow::Result<usize> {
        if self.count_drafts(schema) <= max_keep {
            return Ok(0);
        }
        let mut deleted = 0;
        self.ud.fold_with(D, schema, &mut |mut v: Vec<SnapshotEntry>| {
            if v.len() <= max_keep {
                return v;
            }
            v.sort_by_key(|e| e.last_used);
            deleted = v.len() - max_keep;
            v.split_off(deleted)
        })?;
        Ok(deleted)
    }

    /// 草稿 → 临时词（一批：Delete 草稿 + 临时词学习）。没有该草稿返回 false。
    pub fn promote_draft_to_temp(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        add_weight: i32,
        boundary: u64,
    ) -> anyhow::Result<bool> {
        if self.ud.get(D, schema, code, text).is_none() {
            return Ok(false);
        }
        let (learn, _, _) = self.temp_learn_ops(schema, code, text, add_weight, boundary);
        let mut ops = vec![Op::new(D, schema, OpKind::Delete, code, text, Payload::None, 0)];
        ops.extend(learn);
        self.ud.apply_batch(&ops)?;
        self.bump_words(schema);
        Ok(true)
    }

    /// 全部草稿条数（含已过期未清的，同 redb 版）。
    pub fn count_drafts(&self, schema: &str) -> usize {
        let mut n = 0;
        self.ud.scan(D, schema, "", &mut |_| {
            n += 1;
            true
        });
        n
    }

    pub fn clear_drafts(&self, schema: &str) -> anyhow::Result<usize> {
        let mut n = 0;
        self.ud.fold_with(D, schema, &mut |v| {
            n = v.len();
            Vec::new()
        })?;
        Ok(n)
    }
}
```

`runtime/mod.rs` 加 `mod draft;` 与 `pub use draft::draft_alive;`。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`，`runtime_draft` 6 条。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/runtime/draft.rs wind_input/crates/wind-userdata/tests/runtime_draft.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 草稿 API（对拍 wind-store draft_words）

有效期按 last_used，写入即续期；过期与淘汰走 fold_with。
EOF
```

---

### Task 7: 词频与联想历史 API

**Files:**
- Create: `wind_input/crates/wind-userdata/src/runtime/freq.rs`
- Modify: `wind_input/crates/wind-userdata/src/runtime/mod.rs`（`mod freq; pub use freq::FreqRec;`）
- Test: `wind_input/crates/wind-userdata/tests/runtime_freq.rs`

**Interfaces:**
- Consumes: Task 5 的 `Runtime`。
- Produces：
  - `pub struct FreqRec { pub count: u32, pub last_used: i64 }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `record_freq(&self, schema, code, text) -> anyhow::Result<()>`
  - `freq(&self, schema, code, text) -> Option<FreqRec>`
  - `freq_batch(&self, keys: &[(String, String, String)]) -> Vec<Option<FreqRec>>`
  - `for_each_freq(&self, schema, prefix, f: &mut dyn FnMut(&str, &str, FreqRec) -> bool)`
  - `list_freq_paged(&self, schema, prefix, offset: usize, limit: usize) -> (Vec<(String, String, FreqRec)>, usize)`
  - `delete_freq(&self, schema, code, text) -> anyhow::Result<()>`、`clear_freq(&self, schema) -> anyhow::Result<usize>`
  - `import_freq_rows(&self, schema, rows: &[(String, String, FreqRec)]) -> anyhow::Result<usize>`（同键取 max）
  - `record_assoc_pick(&self, schema, context, word) -> anyhow::Result<()>`
  - `assoc_history(&self, schema, context, limit: usize) -> Vec<(String, FreqRec)>`（`(count, last_used)` 降序，同则按词）
  - `for_each_assoc(&self, schema, f: &mut dyn FnMut(&str, &str, FreqRec) -> bool)`（导出用）
  - `clear_assoc(&self, schema) -> anyhow::Result<usize>`、`import_assoc_rows(&self, schema, rows: &[(String, String, FreqRec)]) -> anyhow::Result<usize>`

语义照 `wind-store/src/freq.rs` 与 `assoc_history.rs`。空 code / text 的写入直接忽略（`record_assoc_pick` 原本如此；`record_freq` 补同样的守卫——wdat 不收空键，且生产路径不会传空）。

- [ ] **Step 1: 写失败测试 `tests/runtime_freq.rs`**

```rust
//! 词频 / 联想历史 API 与 wind-store/src/{freq,assoc_history}.rs 的行为对拍。
use std::sync::Arc;
use wind_userdata::engine::{Dirs, UserData};
use wind_userdata::record::Target;
use wind_userdata::runtime::{FreqRec, Runtime};

fn open(name: &str) -> (std::path::PathBuf, Runtime) {
    let d = std::env::temp_dir().join(format!("wind_rtf_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let r = Runtime::new(Arc::new(
        UserData::open(Dirs {
            state_dir: d.clone(),
            dir_id: "dir".into(),
        })
        .unwrap(),
    ));
    (d, r)
}

#[test]
fn record_counts_and_stamps_last_used() {
    let (_d, r) = open("rec");
    assert!(r.freq("py", "nihao", "你好").is_none());
    r.record_freq("py", "nihao", "你好").unwrap();
    r.record_freq("py", "nihao", "你好").unwrap();
    let f = r.freq("py", "nihao", "你好").unwrap();
    assert_eq!(f.count, 2);
    assert!(f.last_used > 1_700_000_000, "unix 秒");
    r.record_freq("py", "", "空").unwrap();
    assert!(r.freq("py", "", "空").is_none(), "空码忽略");
}

/// 对照 freq_batch_matches_individual_lookups。
#[test]
fn batch_matches_individual_lookups() {
    let (_d, r) = open("batch");
    r.record_freq("py", "a", "甲").unwrap();
    r.record_freq("wb", "a", "甲").unwrap();
    r.data().fold(Target::Freq, "py").unwrap();
    let keys = vec![
        ("py".to_string(), "a".to_string(), "甲".to_string()),
        ("py".to_string(), "b".to_string(), "乙".to_string()),
        ("wb".to_string(), "a".to_string(), "甲".to_string()),
    ];
    let got = r.freq_batch(&keys);
    let want: Vec<Option<FreqRec>> = keys.iter().map(|(s, c, t)| r.freq(s, c, t)).collect();
    assert_eq!(got, want);
    assert!(got[0].is_some() && got[1].is_none() && got[2].is_some());
}

#[test]
fn paged_listing_delete_and_clear() {
    let (_d, r) = open("page");
    for c in ["a", "b", "c", "d"] {
        r.record_freq("py", c, "字").unwrap();
    }
    let (page, total) = r.list_freq_paged("py", "", 1, 2);
    assert_eq!(total, 4);
    assert_eq!(page.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), vec!["b", "c"]);
    r.delete_freq("py", "b", "字").unwrap();
    assert_eq!(r.list_freq_paged("py", "", 0, 0).1, 3);
    assert_eq!(r.clear_freq("py").unwrap(), 3);
    assert_eq!(r.list_freq_paged("py", "", 0, 0).1, 0);
}

/// 对照 freq_jsonl_roundtrip_merge_max：导入同键取 max。
#[test]
fn import_merges_max() {
    let (_d, r) = open("import");
    r.record_freq("py", "a", "甲").unwrap();
    let mine = r.freq("py", "a", "甲").unwrap();
    let rows = vec![
        ("a".to_string(), "甲".to_string(), FreqRec { count: 9, last_used: 1 }),
        ("b".to_string(), "乙".to_string(), FreqRec { count: 2, last_used: 5 }),
    ];
    assert_eq!(r.import_freq_rows("py", &rows).unwrap(), 2);
    assert_eq!(
        r.freq("py", "a", "甲").unwrap(),
        FreqRec { count: 9, last_used: mine.last_used }
    );
    assert_eq!(r.freq("py", "b", "乙").unwrap(), FreqRec { count: 2, last_used: 5 });
}

#[test]
fn assoc_history_orders_by_count_then_recent() {
    let (_d, r) = open("assoc");
    r.record_assoc_pick("py", "天", "气").unwrap();
    r.record_assoc_pick("py", "天", "空").unwrap();
    r.record_assoc_pick("py", "天", "空").unwrap();
    r.record_assoc_pick("py", "", "空").unwrap();
    let h = r.assoc_history("py", "天", 0);
    assert_eq!(h.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["空", "气"]);
    assert_eq!(r.assoc_history("py", "天", 1).len(), 1);
    assert_eq!(r.clear_assoc("py").unwrap(), 2);
    assert!(r.assoc_history("py", "天", 0).is_empty());
}

/// Review Focus 1：联想上文是中文，折回成快照、重开后仍可精确查到。
#[test]
fn assoc_context_non_ascii_survives_fold() {
    let (d, r) = open("assoc_fold");
    r.record_assoc_pick("py", "今天天气", "很好").unwrap();
    r.record_assoc_pick("py", "今天", "是").unwrap();
    r.data().fold(Target::Assoc, "py").unwrap();
    drop(r);
    let r = Runtime::new(Arc::new(
        UserData::open(Dirs {
            state_dir: d,
            dir_id: "dir".into(),
        })
        .unwrap(),
    ));
    assert_eq!(r.assoc_history("py", "今天天气", 0)[0].0, "很好");
    assert_eq!(r.assoc_history("py", "今天", 0)[0].0, "是");
    let mut n = 0;
    r.for_each_assoc("py", &mut |_, _, _| {
        n += 1;
        true
    });
    assert_eq!(n, 2);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --test runtime_freq 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`FreqRec` / `record_freq` 未定义。

- [ ] **Step 3: 实现 `src/runtime/freq.rs`**

```rust
//! 词频与联想历史（对照 `wind-store/src/freq.rs`、`assoc_history.rs`）。两者都是
//! `(code, text) → (count, last_used)`：词频的 code 是码，联想的 code 是上文。

use super::Runtime;
use crate::record::{OpKind, Payload, Target};
use crate::snapshot::SnapshotEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreqRec {
    pub count: u32,
    /// unix 秒
    pub last_used: i64,
}

fn rec(e: &SnapshotEntry) -> FreqRec {
    FreqRec {
        count: e.count,
        last_used: e.last_used,
    }
}

impl Runtime {
    fn incr(&self, target: Target, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        if code.is_empty() || text.is_empty() {
            return Ok(());
        }
        self.ud
            .apply(target, schema, OpKind::Incr, code, text, Payload::Count(1), 0)?;
        Ok(())
    }

    fn clear_target(&self, target: Target, schema: &str) -> anyhow::Result<usize> {
        let mut n = 0;
        self.ud.fold_with(target, schema, &mut |v| {
            n = v.len();
            Vec::new()
        })?;
        Ok(n)
    }

    /// 同键取 max（count 与 last_used 各自取）；新键 created_at = last_used。
    fn import_max(
        &self,
        target: Target,
        schema: &str,
        rows: &[(String, String, FreqRec)],
    ) -> anyhow::Result<usize> {
        self.ud.fold_with(target, schema, &mut |v| {
            let mut map: std::collections::BTreeMap<(String, String), SnapshotEntry> = v
                .into_iter()
                .map(|e| ((e.code.clone(), e.text.clone()), e))
                .collect();
            for (code, text, r) in rows {
                if code.is_empty() || text.is_empty() {
                    continue;
                }
                let e = map
                    .entry((code.clone(), text.clone()))
                    .or_insert_with(|| SnapshotEntry {
                        code: code.clone(),
                        text: text.clone(),
                        weight: 0,
                        boundary: 0,
                        count: 0,
                        created_at: r.last_used,
                        last_used: 0,
                        flags: 0,
                    });
                e.count = e.count.max(r.count);
                e.last_used = e.last_used.max(r.last_used);
            }
            map.into_values().collect()
        })?;
        Ok(rows.len())
    }

    pub fn record_freq(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        self.incr(Target::Freq, schema, code, text)
    }

    pub fn freq(&self, schema: &str, code: &str, text: &str) -> Option<FreqRec> {
        self.ud.get(Target::Freq, schema, code, text).map(|e| rec(&e))
    }

    pub fn freq_batch(&self, keys: &[(String, String, String)]) -> Vec<Option<FreqRec>> {
        keys.iter().map(|(s, c, t)| self.freq(s, c, t)).collect()
    }

    pub fn for_each_freq(
        &self,
        schema: &str,
        prefix: &str,
        f: &mut dyn FnMut(&str, &str, FreqRec) -> bool,
    ) {
        self.ud
            .scan(Target::Freq, schema, prefix, &mut |e| f(&e.code, &e.text, rec(&e)));
    }

    pub fn list_freq_paged(
        &self,
        schema: &str,
        prefix: &str,
        offset: usize,
        limit: usize,
    ) -> (Vec<(String, String, FreqRec)>, usize) {
        let mut page = Vec::new();
        let mut total = 0;
        self.for_each_freq(schema, prefix, &mut |c, t, r| {
            if total >= offset && (limit == 0 || page.len() < limit) {
                page.push((c.to_string(), t.to_string(), r));
            }
            total += 1;
            true
        });
        (page, total)
    }

    pub fn delete_freq(&self, schema: &str, code: &str, text: &str) -> anyhow::Result<()> {
        if self.ud.get(Target::Freq, schema, code, text).is_some() {
            self.ud
                .apply(Target::Freq, schema, OpKind::Delete, code, text, Payload::None, 0)?;
        }
        Ok(())
    }

    pub fn clear_freq(&self, schema: &str) -> anyhow::Result<usize> {
        self.clear_target(Target::Freq, schema)
    }

    pub fn import_freq_rows(
        &self,
        schema: &str,
        rows: &[(String, String, FreqRec)],
    ) -> anyhow::Result<usize> {
        self.import_max(Target::Freq, schema, rows)
    }

    pub fn record_assoc_pick(&self, schema: &str, context: &str, word: &str) -> anyhow::Result<()> {
        if schema.is_empty() {
            return Ok(());
        }
        self.incr(Target::Assoc, schema, context, word)
    }

    pub fn assoc_history(&self, schema: &str, context: &str, limit: usize) -> Vec<(String, FreqRec)> {
        let mut out: Vec<(String, FreqRec)> = self
            .ud
            .exact(Target::Assoc, schema, context)
            .into_iter()
            .map(|e| {
                let r = rec(&e);
                (e.text, r)
            })
            .collect();
        out.sort_by(|a, b| {
            (b.1.count, b.1.last_used)
                .cmp(&(a.1.count, a.1.last_used))
                .then_with(|| a.0.cmp(&b.0))
        });
        if limit > 0 {
            out.truncate(limit);
        }
        out
    }

    pub fn for_each_assoc(&self, schema: &str, f: &mut dyn FnMut(&str, &str, FreqRec) -> bool) {
        self.ud
            .scan(Target::Assoc, schema, "", &mut |e| f(&e.code, &e.text, rec(&e)));
    }

    pub fn clear_assoc(&self, schema: &str) -> anyhow::Result<usize> {
        self.clear_target(Target::Assoc, schema)
    }

    pub fn import_assoc_rows(
        &self,
        schema: &str,
        rows: &[(String, String, FreqRec)],
    ) -> anyhow::Result<usize> {
        self.import_max(Target::Assoc, schema, rows)
    }
}
```

`runtime/mod.rs` 加 `mod freq;` 与 `pub use freq::FreqRec;`。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`，`runtime_freq` 6 条。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/runtime/freq.rs wind_input/crates/wind-userdata/tests/runtime_freq.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 词频与联想历史 API（对拍 wind-store freq / assoc_history）
EOF
```

---

### Task 8: 两个层与 `StoreTempLayer` / `StoreDraftLayer` 对齐

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/layer.rs`
- Test: `layer.rs` tests 模块

**Interfaces:**
- Consumes: Task 1 的层改写；Task 6 的 `draft_alive`；`UserData::{exact, scan}`。
- Produces：
  - `UserDataTempLayer`：`name()` 返回 `"temp:{schema}"`；`entries_digest()` 返回 `Some("temp:{schema}|enumerates-nothing")`（与 `StoreTempLayer` 同理由：不覆盖 `for_each_entry`）；`search_pattern` 照 `store_layer.rs::pattern_from_records`。
  - `pub struct UserDataDraftLayer`；`UserDataDraftLayer::new(ud: Arc<UserData>, schema: &str, ttl_secs: i64)`；`name()` = `"draft:{schema}"`；`layer_type()` = `LayerType::Draft`；`search` 只精确、过滤过期、候选 `weight 0` + `is_draft = true`；`search_prefix` 恒空；简拼与通配走 trait 默认（空）。

- [ ] **Step 1: 写失败测试（layer.rs tests 末尾）**

```rust
    fn open_ud(name: &str) -> Arc<UserData> {
        let d = std::env::temp_dir().join(format!("wind_layer8_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Arc::new(
            UserData::open(Dirs {
                state_dir: d,
                dir_id: "dir".into(),
            })
            .unwrap(),
        )
    }

    #[test]
    fn temp_layer_identity_matches_store_layer() {
        let ud = open_ud("ident");
        let l = UserDataTempLayer::new(ud.clone(), "py");
        assert_eq!(l.name(), "temp:py");
        assert_eq!(l.entries_digest().as_deref(), Some("temp:py|enumerates-nothing"));
        ud.apply(Target::Temp, "py", OpKind::Upsert, "nihao", "你好", Payload::Weight(5), 0b101)
            .unwrap();
        let c = &l.search("nihao", 0)[0];
        assert!(c.meta.is_temp_dict && !c.is_prefix);
        assert_eq!((c.natural_order, c.boundary, c.meta.raw_weight), (0, 0b101, 5));
    }

    /// 对照 store_layers_answer_pattern_queries_and_drafts_stay_out：字面前缀扫描后过滤，等长档在前。
    #[test]
    fn temp_layer_answers_pattern_queries() {
        let ud = open_ud("pattern");
        for (code, text) in [("abcd", "甲"), ("abxd", "乙"), ("abcde", "丙"), ("zbcd", "丁")] {
            ud.apply(Target::Temp, "wb", OpKind::Upsert, code, text, Payload::Weight(1), 0)
                .unwrap();
        }
        ud.fold(Target::Temp, "wb").unwrap();
        let l = UserDataTempLayer::new(ud, "wb");
        assert_eq!(names(l.search_pattern("ab?d", '?', 10, false)), vec!["甲", "乙"]);
        let with_prefix = l.search_pattern("ab?d", '?', 10, true);
        assert_eq!(names(with_prefix.clone()), vec!["甲", "乙", "丙"], "等长档在前");
        assert!(with_prefix[2].is_prefix);
        assert!(l.search_pattern("", '?', 10, true).is_empty());
        assert!(l.search_pattern("ab?d", '?', 0, true).is_empty());
    }

    /// 对照 drafts_never_surface_through_prefix_queries 与草稿候选形态。
    #[test]
    fn draft_layer_is_exact_only_sinks_and_respects_ttl() {
        let ud = open_ud("draft");
        let mut old = crate::engine::Op::new(Target::Draft, "wb", OpKind::Upsert, "ggtt", "旧", Payload::Weight(0), 0);
        old.ts_ms = 1_000;
        ud.apply_batch(&[old]).unwrap();
        ud.apply(Target::Draft, "wb", OpKind::Upsert, "ggtt", "盖伦", Payload::Weight(0), 0)
            .unwrap();
        let l = UserDataDraftLayer::new(ud, "wb", 86_400);
        assert_eq!(l.name(), "draft:wb");
        assert_eq!(l.layer_type(), LayerType::Draft);
        let hits = l.search("ggtt", 0);
        assert_eq!(names(hits.clone()), vec!["盖伦"], "过期的不出");
        assert!(hits[0].is_draft && hits[0].weight == 0);
        assert!(l.search_prefix("ggt", 0).is_empty());
        assert!(l.search_abbrev("g", 0).is_empty());
    }
```

（`names` 是该模块已有的辅助函数；若其签名与此不同，按它的实际签名调用。`LayerType` 若未实现 `PartialEq`，改用 `matches!(l.layer_type(), LayerType::Draft)`。）

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata layer:: 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`UserDataDraftLayer` 未定义；修掉后 `temp_layer_identity_matches_store_layer` 因 name 为 `userdata-temp` 而 FAIL、`temp_layer_answers_pattern_queries` 因默认空实现而 FAIL。

- [ ] **Step 3: 实现**

`UserDataTempLayer` 加字段 `name: String`（`new` 里 `format!("temp:{schema}")`），`name()` 返回 `&self.name`，并加：

```rust
    /// 本层不覆盖 `for_each_entry`，对派生索引的贡献恒为「无」，给稳定摘要（同 `StoreTempLayer`）。
    fn entries_digest(&self) -> Option<String> {
        Some(format!("{}|enumerates-nothing", self.name))
    }

    /// 字面前缀扫描（不带 limit——先截断再过滤会整批落空）→ 过滤 → 等长档在前排序截断。
    fn search_pattern(
        &self,
        pattern: &str,
        wildcard: char,
        limit: usize,
        with_prefix: bool,
    ) -> Vec<Candidate> {
        if pattern.is_empty() || limit == 0 {
            return Vec::new();
        }
        let n = pattern.chars().count();
        let lit = wind_dict::layer::literal_prefix(pattern, wildcard);
        let mut v = Vec::new();
        self.ud.scan(Target::Temp, &self.schema, lit, &mut |e| {
            if wind_dict::layer::pattern_matches(pattern, wildcard, &e.code, with_prefix) {
                let longer = e.code.chars().count() > n;
                v.push(Self::candidate(&e.code, &e.text, e.weight, e.boundary, longer));
            }
            true
        });
        v.sort_by(|a, b| wind_dict::layer::cmp_pattern(n, a, b));
        v.truncate(limit);
        v
    }
```

草稿层：

```rust
/// 草稿层（对照 `wind-dict/src/store_layer.rs::StoreDraftLayer`）：只响应精确查询，查询时按
/// TTL 过滤，候选 weight 0 + `is_draft` 恒沉底。`search_prefix` 恒空、简拼与通配走 trait 默认
/// （空）——理由见 `StoreDraftLayer` 文档，**不要顺手实现它们**。
pub struct UserDataDraftLayer {
    ud: Arc<UserData>,
    schema: String,
    name: String,
    ttl_secs: i64,
}

impl UserDataDraftLayer {
    pub fn new(ud: Arc<UserData>, schema: &str, ttl_secs: i64) -> Self {
        Self {
            ud,
            schema: schema.to_string(),
            name: format!("draft:{schema}"),
            ttl_secs,
        }
    }
}

impl DictLayer for UserDataDraftLayer {
    fn name(&self) -> &str {
        &self.name
    }

    fn layer_type(&self) -> LayerType {
        LayerType::Draft
    }

    fn search(&self, code: &str, limit: usize) -> Vec<Candidate> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let cands: Vec<Candidate> = self
            .ud
            .exact(Target::Draft, &self.schema, code)
            .into_iter()
            .filter(|e| crate::runtime::draft_alive(e.last_used, now, self.ttl_secs))
            .map(|e| Candidate {
                text: e.text,
                code: code.to_string(),
                // weight 0：不参与比较，也不让 merge_search 的「继承更高 weight」抬别层的权
                weight: 0,
                is_draft: true,
                ..Default::default()
            })
            .collect();
        Self::sort_trunc(cands, limit)
    }

    fn search_prefix(&self, _prefix: &str, _limit: usize) -> Vec<Candidate> {
        Vec::new()
    }
}
```

`sort_trunc` 从 `UserDataTempLayer` 的关联函数挪成模块级 `fn sort_trunc(...)`，两个层共用（把调用处 `Self::sort_trunc` 改成 `sort_trunc`）。

- [ ] **Step 4: 运行确认通过**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result"`
Expected: 全部 `ok`。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata/src/layer.rs <<'EOF'
feat(userdata): 临时词层与 StoreTempLayer 对齐，新增草稿层

name / entries_digest / 通配查询与 redb 版一致；草稿层只精确、按 TTL 过滤、恒沉底。
EOF
```

---

### Task 9: 文档与收尾

**Files:**
- Modify: `wind_input/crates/wind-userdata/AGENTS.md`（Key Files 加 `src/runtime/`、三个新测试文件；Working In This Directory 加「叠加语义唯一」「批量走 fold_with」「时间单位」「后台折回」四条）
- Modify: `docs/design/user-data-storage-redesign.md`（§3.6 generation 一句、§10 P1 行拆成 P1a / P1b 并标 P1a 已实现）

- [ ] **Step 1: 改文档**

`AGENTS.md` 的 Key Files 表追加：

```markdown
| `src/runtime/` | 运行时领域 API `Runtime`：临时词（`temp.rs`）、草稿（`draft.rs`）、词频 + 联想历史（`freq.rs`），语义逐条对照 `wind-store` 同名函数；不依赖 `wind-store` |
| `tests/runtime_{temp,draft,freq}.rs` | 领域 API 与 `wind-store` 的行为对拍 |
```

`layer.rs` 一行改为：`UserDataTempLayer`（与 `StoreTempLayer` 同名同摘要、含通配）与 `UserDataDraftLayer`（只精确、TTL、恒沉底）。

Working In This Directory 追加：

```markdown
- **叠加语义只有一份**：`engine::apply_ops`（表见 P1a 计划 Task 1）。读、折回、层都经 `overlay_entries` 调它——别在别处再写一份 Upsert/Delete 的 match。
- **批量操作走 `fold_with`**：清空、导入、草稿过期 / 淘汰都是「旧快照 + delta → policy → 新快照」原子换段，水位随之推进；不要逐条写 Delete。按键路径的单条写走 `apply` / `apply_batch`。
- **时间单位**：sidecar 与领域 API 是 unix 秒（同 `wind-store`）；日志 `ts_ms` 是毫秒。
- **后台折回**：写入方在写完后调 `spawn_maintain()`（单飞，只读时不起）；阈值 `FOLD_THRESHOLD` 条、日志 `JOURNAL_MAX_BYTES`。
```

设计文档 §3.6 把「`generation` 不同的记录一律忽略。」改为：

```markdown
- `generation`：P1 起批量操作（清空、导入、还原、迁移）一律走 `fold_with`——新快照内嵌 `max_seq = upto` 原子换入即推进水位，被取代的旧记录不再回放，不需要按代过滤。字段保留，留给日后「整目录文件级替换」式还原。
```

§10 表中 P1 一行拆为两行：

```markdown
| P1a | 引擎补强（时间、叠加唯一、delta 二分、fold_with、零条目快照、state 版本、压实与后台折回）；`Runtime` 四组领域 API；临时词层 / 草稿层 | **已实现**（计划 `user-data-storage-plan-p1a.md`）：领域 API 与 `wind-store` 对拍测试全绿 |
| P1b | 接线：协调器写路径、`EngineManager` 换层、设置页 IPC、备份 / 导出、联想索引改走 `Runtime`；redb → 引擎一次性迁移；`format_version` → 2；退役 5 张表 | 19 万词库简拼召回不劣于基线；按键路径不再碰 `Store` Mutex |
```

- [ ] **Step 2: 全量测试与 clippy**

Run: `cd wind_input && TMPDIR=<独立临时目录> cargo test -p wind-userdata --no-fail-fast 2>&1 | grep -E "FAILED|test result" && TMPDIR=<独立临时目录> cargo clippy -p wind-userdata --all-targets 2>&1 | grep -E "^(warning|error)" | grep -v "wind-dict"`
Expected: 测试全部 `ok`；`wind-userdata` 无 clippy 警告（顺手修 M8：`snapshot.rs` 里 `push_abbrev_sorted(&k, codes.into_iter())` 去掉 `.into_iter()`）。

- [ ] **Step 3: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata docs/design/user-data-storage-redesign.md <<'EOF'
docs(userdata): P1a 文档与阶段状态；P1 拆为 P1a / P1b
EOF
```
