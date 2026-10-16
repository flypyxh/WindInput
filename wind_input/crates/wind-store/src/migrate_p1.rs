//! P1 迁移（设计 §7.2）：redb 的 temp_words / draft_words / freq / assoc_history 四张表一次性
//! 迁入运行时引擎。按 (目标, 方案) 用 `fold_with` 把旧表行与引擎已有条目**合并**（见
//! [`merge_legacy`]）：重跑幂等，且上一轮迁移失败后照常运行写进引擎的数据不会被旧表盖掉。
//! 全部完成后写 `state/migrated-p1`。旧表只读保留到 P4，不删、不再写。

use crate::Store;
use crate::store::{ASSOC_HIST, DRAFT_WORDS, FREQ, TEMP_WORDS};
use std::collections::BTreeMap;
use std::path::Path;
use wind_userdata::record::Target;
use wind_userdata::snapshot::SnapshotEntry;

pub const MIGRATED_P1: &str = "migrated-p1";

type Rows = BTreeMap<String, Vec<SnapshotEntry>>;

fn entry(
    code: &str,
    text: &str,
    weight: i32,
    count: u32,
    created_at: i64,
    last_used: i64,
    boundary: u64,
) -> SnapshotEntry {
    SnapshotEntry {
        code: code.to_string(),
        text: text.to_string(),
        weight,
        boundary,
        count,
        created_at,
        last_used,
        flags: 0,
        order: 0,
    }
}

/// 旧表行与引擎里已有的条目合并。两边都有的键：以引擎为准（它更新），但 count 取较大、
/// created_at 取较早、last_used 取较晚——上一轮迁移失败时，没迁成的目标在引擎里只有那一轮
/// 新累计的小计数，不能盖掉旧表的历史计数。只在一边的照收。重跑幂等。
fn merge_legacy(legacy: Vec<SnapshotEntry>, existing: Vec<SnapshotEntry>) -> Vec<SnapshotEntry> {
    let mut map: BTreeMap<(String, String), SnapshotEntry> = legacy
        .into_iter()
        .map(|e| ((e.code.clone(), e.text.clone()), e))
        .collect();
    for e in existing {
        let key = (e.code.clone(), e.text.clone());
        let merged = match map.remove(&key) {
            Some(old) => SnapshotEntry {
                count: e.count.max(old.count),
                created_at: if old.created_at > 0 {
                    old.created_at.min(e.created_at)
                } else {
                    e.created_at
                },
                last_used: e.last_used.max(old.last_used),
                ..e
            },
            None => e,
        };
        map.insert(key, merged);
    }
    map.into_values().collect()
}

/// 旧表一行 `(code, text, value)` 的解码器。
type LegacyDecode<'a> = dyn Fn(&str, &str, &[u8]) -> Option<SnapshotEntry> + 'a;

impl Store {
    /// 读旧表 `table` 全部行，按方案分组；`decode(code, text, value) -> Option<SnapshotEntry>`。
    fn legacy_rows(
        &self,
        table: redb::TableDefinition<&str, &[u8]>,
        decode: &LegacyDecode,
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
                    crate::freq::dec_freq(v)
                        .map(|r| entry(c, t, 0, r.count, r.last_used, r.last_used, 0))
                })?,
            ),
            (
                Target::Assoc,
                self.legacy_rows(ASSOC_HIST, &|c, t, v| {
                    crate::freq::dec_freq(v)
                        .map(|r| entry(c, t, 0, r.count, r.last_used, r.last_used, 0))
                })?,
            ),
        ];
        let ud = self.runtime().data().clone();
        let mut total = 0usize;
        for (target, by_schema) in jobs {
            for (schema, rows) in by_schema {
                total += rows.len();
                let rows = std::cell::RefCell::new(Some(rows));
                ud.fold_with(target, &schema, &mut |existing| {
                    merge_legacy(rows.borrow_mut().take().unwrap_or_default(), existing)
                })?;
            }
        }
        wind_userdata::state::atomic_write(&marker, b"1\n")?;
        tracing::info!("P1 迁移完成：{total} 条运行时数据从 redb 迁入引擎");
        Ok(true)
    }
}

impl Store {
    /// 旧表里某方案的行数 `(临时词, 词频, 草稿)`，供 `examples/p1_migrate_check.rs` 与迁移结果比对。
    #[doc(hidden)]
    pub fn legacy_counts(&self, schema: &str) -> anyhow::Result<(usize, usize, usize)> {
        let n = |table, dec: &LegacyDecode| {
            self.legacy_rows(table, dec)
                .map(|r| r.get(schema).map_or(0, |v| v.len()))
        };
        Ok((
            n(TEMP_WORDS, &|c, t, v| {
                crate::user_words::dec_val(v).map(|(w, k, ca, b)| entry(c, t, w, k, ca, ca, b))
            })?,
            n(FREQ, &|c, t, v| {
                crate::freq::dec_freq(v)
                    .map(|r| entry(c, t, 0, r.count, r.last_used, r.last_used, 0))
            })?,
            n(DRAFT_WORDS, &|c, t, v| {
                crate::draft_words::dec_draft(v).map(|ca| entry(c, t, 0, 0, ca, ca, 0))
            })?,
        ))
    }
}

/// 往旧表直接写行（模拟老版本写的库）。测试与跨 crate 测试用，生产代码不调。
#[doc(hidden)]
impl Store {
    #[allow(clippy::too_many_arguments)] // 参数与旧表字段一一对应
    pub fn legacy_put_temp(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        w: i32,
        count: u32,
        ca: i64,
        b: u64,
    ) -> anyhow::Result<()> {
        self.legacy_put(
            TEMP_WORDS,
            schema,
            code,
            text,
            &crate::user_words::enc_val(w, count, ca, b),
        )
    }
    pub fn legacy_put_draft(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        ca: i64,
    ) -> anyhow::Result<()> {
        self.legacy_put(DRAFT_WORDS, schema, code, text, &ca.to_le_bytes())
    }
    pub fn legacy_put_freq(
        &self,
        schema: &str,
        code: &str,
        text: &str,
        count: u32,
        last_used: i64,
    ) -> anyhow::Result<()> {
        self.legacy_put(
            FREQ,
            schema,
            code,
            text,
            &crate::freq::enc_freq(count, last_used),
        )
    }
    pub fn legacy_put_assoc(
        &self,
        schema: &str,
        ctx: &str,
        word: &str,
        count: u32,
        last_used: i64,
    ) -> anyhow::Result<()> {
        self.legacy_put(
            ASSOC_HIST,
            schema,
            ctx,
            word,
            &crate::freq::enc_freq(count, last_used),
        )
    }
    fn legacy_put(
        &self,
        table: redb::TableDefinition<&str, &[u8]>,
        schema: &str,
        code: &str,
        text: &str,
        val: &[u8],
    ) -> anyhow::Result<()> {
        let key = crate::user_words::enc_key(schema, code, text);
        self.with_db(|db| {
            let txn = db.begin_write()?;
            txn.open_table(table)?.insert(key.as_str(), val)?;
            txn.commit()?;
            Ok(())
        })
    }
}

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
        s.legacy_put_temp("py", "nihao", "你好", 800, 3, 1_700_000_000, 0b101)
            .unwrap();
        s.legacy_put_temp("py", "xian", "西安", 500, 1, 1_700_000_001, 0b101)
            .unwrap();
        s.legacy_put_draft("wb", "ggtt", "盖伦", crate::draft_words::now_for_test())
            .unwrap();
        s.legacy_put_freq("py", "nihao", "你好", 7, 1_700_000_100)
            .unwrap();
        s.legacy_put_assoc("py", "天", "气", 2, 1_700_000_200)
            .unwrap();
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
        assert_eq!(
            (w[0].weight, w[0].count, w[0].created_at, w[0].boundary),
            (800, 3, 1_700_000_000, 0b101)
        );
        use wind_dict::DictLayer;
        let l = crate::StoreTempLayer::new(s.clone(), "py");
        assert!(
            l.search_abbrev("xa", 0).iter().any(|c| c.text == "西安"),
            "简拼可召回"
        );
        assert_eq!(s.search_drafts("wb", "ggtt", 86_400).unwrap(), vec!["盖伦"]);
        assert_eq!(
            s.get_freq("py", "nihao", "你好")
                .unwrap()
                .map(|r| (r.count, r.last_used)),
            Some((7, 1_700_000_100))
        );
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
        assert!(
            crate::store::derived_state_dir(&p)
                .join(super::MIGRATED_P1)
                .is_file()
        );
        assert_eq!(s.count_temp_words("py").unwrap(), 0);
    }

    /// 审查 I1：迁移失败（某目标折回不了）后照常运行，这一轮的写入不得在下次重做时被旧表覆盖。
    #[test]
    fn failed_migration_keeps_writes_made_before_retry() {
        let p = legacy_db("fail");
        let state = crate::store::derived_state_dir(&p);
        std::fs::create_dir_all(&state).unwrap();
        // 普通文件占住 wb 的状态子目录：草稿 wb 的折回建不了目录 ⇒ 迁移失败
        std::fs::write(state.join("wb"), b"x").unwrap();
        let s = Store::open(&p).unwrap();
        assert!(!s.migrated_p1(), "迁移失败：标记不写");
        s.learn_temp_word("py", "xinci", "新词", 800, 0).unwrap();
        s.learn_temp_word("py", "nihao", "你好", 800, 0b101)
            .unwrap();
        drop(s);
        std::fs::remove_file(state.join("wb")).unwrap();
        let s = Store::open(&p).unwrap();
        assert!(s.migrated_p1());
        assert_eq!(
            s.get_temp_word("py", "xinci", "新词").unwrap(),
            Some(1),
            "失败那轮学的词还在"
        );
        assert_eq!(
            s.get_temp_word("py", "nihao", "你好").unwrap(),
            Some(4),
            "失败那轮的 count+1 不被旧表的 3 覆盖"
        );
        assert_eq!(
            s.search_drafts("wb", "ggtt", 86_400).unwrap(),
            vec!["盖伦"],
            "重做补上失败的目标"
        );
        assert_eq!(s.get_freq("py", "nihao", "你好").unwrap().unwrap().count, 7);
    }
}
