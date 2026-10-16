//! P4a 迁移（设计 §7.2）：redb 剩下的运行期数据迁成用户数据目录 `data/` 下的文件——快捷格式 /
//! 常用字 / 字符集用户层 / 补全历史 / 每日统计 / 统计元数据 / `phrase_sys_hash`（P5 起全在 `data/`）。每一类目标文件已经
//! 存在且非空 ⇒ 这一类跳过（文件是真源，别拿旧表盖掉它）。全部做完写 `migrated-p4`；没有标记就
//! 整体重做（跳过规则让重做幂等）。旧表只读保留到 P4b。

use crate::Store;
use crate::common_chars::CommonCharRecord;
use crate::quick_format::QuickFormatRecord;
use crate::stats::{DailyStats, StatsMeta};
use crate::store::{CHARSET_USER, COMMON_CHARS, META, QUICK_FORMAT, STATS_DAILY};
use std::path::Path;

pub const MIGRATED_P4: &str = "migrated-p4";

fn nonempty_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0)
}

impl Store {
    pub(crate) fn migrate_p4_if_needed(&self, state_dir: &Path) -> anyhow::Result<bool> {
        let marker = state_dir.join(MIGRATED_P4);
        if marker.is_file() {
            return Ok(false);
        }
        use redb::ReadableTable;
        // 快捷格式
        if !nonempty_file(&self.quick_format_path()) {
            let rows: Vec<(String, QuickFormatRecord)> = self.with_db(|db| {
                let txn = db.begin_read()?;
                let t = txn.open_table(QUICK_FORMAT)?;
                let mut out = Vec::new();
                for item in t.iter()? {
                    let (k, v) = item?;
                    if let Ok(r) = serde_json::from_slice::<QuickFormatRecord>(v.value()) {
                        out.push((k.value().to_string(), r));
                    }
                }
                Ok(out)
            })?;
            if !rows.is_empty() {
                self.import_quick_format_rows(rows, true)?;
            }
        }
        // 常用字
        if !nonempty_file(&self.common_chars_path()) {
            let items: Vec<(String, Option<bool>)> = self.with_db(|db| {
                let txn = db.begin_read()?;
                let t = txn.open_table(COMMON_CHARS)?;
                let mut out = Vec::new();
                for item in t.iter()? {
                    let (k, v) = item?;
                    if let Ok(r) = serde_json::from_slice::<CommonCharRecord>(v.value())
                        && !k.value().is_empty()
                    {
                        out.push((k.value().to_string(), Some(r.common)));
                    }
                }
                Ok(out)
            })?;
            if !items.is_empty() {
                self.apply_common_char_overrides(&items)?;
            }
        }
        // 字符集用户层（逐个 key：文件已在的跳过）
        let docs: Vec<(String, String)> = self.with_db(|db| {
            let txn = db.begin_read()?;
            let t = txn.open_table(CHARSET_USER)?;
            let mut out = Vec::new();
            for item in t.iter()? {
                let (k, v) = item?;
                if !k.value().is_empty() {
                    out.push((
                        k.value().to_string(),
                        String::from_utf8_lossy(v.value()).into_owned(),
                    ));
                }
            }
            Ok(out)
        })?;
        for (key, text) in docs {
            match self.charset_path(&key) {
                Ok(p) if !nonempty_file(&p) => self.set_charset_doc(&key, &text)?,
                Ok(_) => {}
                Err(_) => tracing::warn!("P4 迁移跳过文件名不合法的字符类"),
            }
        }
        // 补全历史
        if !nonempty_file(&self.completion_path()) {
            let rows = self.legacy_completion_rows()?;
            if !rows.is_empty() {
                let jsonl: String = rows
                    .into_iter()
                    .map(|(k, t, r)| {
                        serde_json::json!({
                            "kind": k.as_str(), "text": t,
                            "count": r.count, "last_used": r.last_used,
                        })
                        .to_string()
                            + "\n"
                    })
                    .collect();
                self.import_completions_jsonl(&jsonl)?;
            }
        }
        // 每日统计与元数据：按日合并、已有的日以文件为准（不是整类跳过）——按月逐文件写，中途失败
        // 后重试要能补齐余下的月份
        let (days, meta) = self.legacy_stats()?;
        if !days.is_empty() {
            let jsonl: String = days
                .into_iter()
                .map(|(d, st)| serde_json::json!({ "date": d, "stat": st }).to_string() + "\n")
                .collect();
            self.import_stats_jsonl(&jsonl, false)?;
        }
        if let Some(m) = meta
            && !nonempty_file(&self.stats_meta_path())
        {
            self.put_stats_meta(&m)?;
        }
        // 系统短语哈希
        if self.phrase_sys_hash()?.is_none()
            && let Some(h) = self.legacy_phrase_sys_hash()?
        {
            self.set_phrase_sys_hash(&h)?;
        }
        wind_userdata::state::atomic_write(&marker, b"1\n")?;
        tracing::info!("P4a 迁移完成：覆盖表与小文件已从 redb 迁出");
        Ok(true)
    }

    /// P4a 迁移是否已完成（标记 `migrated-p4` 在）。
    pub fn migrated_p4(&self) -> bool {
        self.state_dir().join(MIGRATED_P4).is_file()
    }

    // ───── 测试 / 探针：往旧表写（模拟升级前的库） ─────

    #[doc(hidden)]
    pub fn legacy_put_quick_format(
        &self,
        kind: &str,
        rec: &QuickFormatRecord,
    ) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(rec)?;
        self.legacy_put_raw(QUICK_FORMAT, kind, &bytes)
    }

    #[doc(hidden)]
    pub fn legacy_put_common_char(&self, ch: &str, common: bool) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(&CommonCharRecord { common })?;
        self.legacy_put_raw(COMMON_CHARS, ch, &bytes)
    }

    #[doc(hidden)]
    pub fn legacy_put_charset(&self, key: &str, text: &str) -> anyhow::Result<()> {
        self.legacy_put_raw(CHARSET_USER, key, text.as_bytes())
    }

    #[doc(hidden)]
    pub fn legacy_put_daily_stat(&self, date: &str, stat: &DailyStats) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(stat)?;
        self.legacy_put_raw(STATS_DAILY, date, &bytes)
    }

    #[doc(hidden)]
    pub fn legacy_put_stats_meta(&self, meta: &StatsMeta) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(meta)?;
        self.legacy_put_raw(META, "stats_meta", &bytes)
    }

    #[doc(hidden)]
    pub fn legacy_put_phrase_sys_hash(&self, h: &str) -> anyhow::Result<()> {
        self.meta_set("phrase_sys_hash", h)
    }

    fn legacy_put_raw(
        &self,
        table: redb::TableDefinition<&str, &[u8]>,
        key: &str,
        val: &[u8],
    ) -> anyhow::Result<()> {
        self.with_db(|db| {
            let txn = db.begin_write()?;
            txn.open_table(table)?.insert(key, val)?;
            txn.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use crate::completion::CompletionKind;
    use crate::quick_format::QuickFormatRecord;
    use crate::stats::{DailyStats, StatsMeta};

    fn db(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("wind_mp4_{name}_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(crate::store::derived_state_dir(&p));
        let _ = std::fs::remove_dir_all(crate::store::derived_text_dir(&p));
        p
    }

    /// 造旧库：P4 各表只在 redb 里；删掉新文件与标记，模拟升级前。
    fn legacy(name: &str) -> std::path::PathBuf {
        let p = db(name);
        {
            let s = Store::open(&p).unwrap();
            let mut qf = QuickFormatRecord::default();
            qf.add_entry("my", "yyyy");
            s.legacy_put_quick_format("date", &qf).unwrap();
            s.legacy_put_common_char("槮", true).unwrap();
            s.legacy_put_common_char("的", false).unwrap();
            s.legacy_put_charset("rare", "key: rare\n").unwrap();
            s.legacy_put_completions(&[(CompletionKind::UrlHistory, "example.com".into(), 3, 9)])
                .unwrap();
            s.legacy_put_daily_stat(
                "2026-10-14",
                &DailyStats {
                    chinese: 7,
                    ..Default::default()
                },
            )
            .unwrap();
            s.legacy_put_stats_meta(&StatsMeta {
                total_chars: 7,
                ..Default::default()
            })
            .unwrap();
            s.legacy_put_phrase_sys_hash("h0").unwrap();
        }
        let st = crate::store::derived_state_dir(&p);
        let tx = crate::store::derived_text_dir(&p);
        for f in [
            st.join(super::MIGRATED_P4),
            tx.join("completion.table.yaml"),
            tx.join("phrases.sys_hash"),
            tx.join("quick_format.toml"),
            tx.join("common_chars.yaml"),
        ] {
            let _ = std::fs::remove_file(f);
        }
        let _ = std::fs::remove_dir_all(tx.join("stats"));
        let _ = std::fs::remove_dir_all(tx.join("charsets"));
        p
    }

    /// 旧表各类数据迁成文件：快捷格式、常用字、字符集、补全、统计、meta、phrase_sys_hash。
    #[test]
    fn p4_tables_migrate_to_files() {
        let p = legacy("all");
        let s = Store::open(&p).unwrap();
        assert!(s.state_dir().join(super::MIGRATED_P4).is_file());
        assert_eq!(
            s.get_quick_format("date").unwrap().unwrap().added[0].id,
            "my"
        );
        assert_eq!(s.get_common_char_override("的").unwrap(), Some(false));
        assert_eq!(
            s.get_charset_doc("rare").unwrap().as_deref(),
            Some("key: rare\n")
        );
        let c = s
            .get_completion(CompletionKind::UrlHistory, "example.com")
            .unwrap()
            .unwrap();
        assert_eq!((c.count, c.last_used), (3, 9));
        assert!(s.completion_path().is_file());
        assert_eq!(s.get_daily_stat("2026-10-14").unwrap().chinese, 7);
        assert_eq!(s.get_stats_meta().unwrap().total_chars, 7);
        assert_eq!(s.phrase_sys_hash().unwrap().as_deref(), Some("h0"));
    }

    /// 用户目录里已有手写的 common_chars.yaml ⇒ 这一类跳过，不拿旧表盖掉它。
    #[test]
    fn p4_migration_keeps_existing_files() {
        let p = legacy("keep");
        let f = crate::store::derived_text_dir(&p).join("common_chars.yaml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "common: [甲]\nuncommon: []\n").unwrap();
        let s = Store::open(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "common: [甲]\nuncommon: []\n"
        );
        assert_eq!(s.get_common_char_override("的").unwrap(), None);
        assert_eq!(
            s.get_quick_format("date").unwrap().unwrap().added[0].id,
            "my",
            "别的类照迁"
        );
    }

    /// 审查 M-5（升为 Important）：统计迁移中途失败后重试，已写的月份保留、余下的补齐。
    #[test]
    fn partial_stats_migration_completes_on_retry() {
        let p = legacy("partial");
        {
            let s = Store::open(&p).unwrap();
            s.legacy_put_daily_stat(
                "2026-09-01",
                &DailyStats {
                    chinese: 3,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let st = crate::store::derived_state_dir(&p);
        let _ = std::fs::remove_file(st.join(super::MIGRATED_P4));
        let stats = crate::store::derived_text_dir(&p).join("stats");
        let _ = std::fs::remove_dir_all(&stats);
        // 上一轮只写成了 10 月（且本地这天已有新数据）
        std::fs::create_dir_all(&stats).unwrap();
        std::fs::write(
            stats.join("2026-10.toml"),
            "version = 1\n[days.2026-10-14]\nchinese = 99\n",
        )
        .unwrap();
        let s = Store::open(&p).unwrap();
        assert_eq!(
            s.get_daily_stat("2026-09-01").unwrap().chinese,
            3,
            "余下的月份补齐"
        );
        assert_eq!(
            s.get_daily_stat("2026-10-14").unwrap().chinese,
            99,
            "已有的日以文件为准"
        );
    }

    /// 审查 I-3：重试时已有的统计元数据（运行期间累加过）不被旧表倒回。
    #[test]
    fn retry_does_not_roll_back_existing_stats_meta() {
        let p = legacy("meta_retry");
        {
            let s = Store::open(&p).unwrap();
            s.legacy_put_stats_meta(&crate::stats::StatsMeta {
                total_chars: 7,
                ..Default::default()
            })
            .unwrap();
        }
        let st = crate::store::derived_state_dir(&p);
        let _ = std::fs::remove_file(st.join(super::MIGRATED_P4));
        let stats = crate::store::derived_text_dir(&p).join("stats");
        std::fs::create_dir_all(&stats).unwrap();
        std::fs::write(stats.join("meta.toml"), "total_chars = 500\n").unwrap();
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get_stats_meta().unwrap().total_chars, 500);
    }
}
