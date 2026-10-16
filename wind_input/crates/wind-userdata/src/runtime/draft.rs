//! 草稿（对照 `wind-store/src/draft_words.rs`）。有效期基准是 `last_used`：每次写入都是
//! Upsert，叠加语义里 Upsert 刷新 last_used，等同 redb 版「重写即续期」。

use super::{Runtime, now_secs};
use crate::engine::Op;
use crate::record::{OpKind, Payload, Target};
use crate::snapshot::SnapshotEntry;

const D: Target = Target::Draft;

/// 一次删除超过这么多条才整段重建；以下逐条记 Delete。
const BULK_DELETE_MAX: usize = 1024;

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

    /// 删掉若干草稿：少量走 Delete 记录（按键节奏下每批落库后都会调，不能每次整段重建），
    /// 大量（启动清理、长时间未用后首次清理）才走 `fold_with` 一次换段。
    fn drop_drafts(
        &self,
        schema: &str,
        doomed: &[(String, String)],
        keep: &mut dyn FnMut(&SnapshotEntry) -> bool,
    ) -> anyhow::Result<()> {
        if doomed.len() <= BULK_DELETE_MAX {
            let ops: Vec<Op> = doomed
                .iter()
                .map(|(c, t)| Op::new(D, schema, OpKind::Delete, c, t, Payload::None, 0))
                .collect();
            self.ud.apply_batch(&ops)?;
        } else {
            self.ud.fold_with(D, schema, &mut |v| {
                v.into_iter().filter(|e| keep(e)).collect()
            })?;
        }
        Ok(())
    }

    /// 清过期草稿；`schema = None` 扫所有方案。返回 (删, 留)。没有过期的方案不写任何东西。
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
            let mut total = 0;
            let mut doomed = Vec::new();
            self.ud.scan(D, &s, "", &mut |e| {
                total += 1;
                if !draft_alive(e.last_used, now, ttl_secs) {
                    doomed.push((e.code, e.text));
                }
                true
            });
            kept += total - doomed.len();
            if doomed.is_empty() {
                continue;
            }
            deleted += doomed.len();
            self.drop_drafts(&s, &doomed, &mut |e| {
                draft_alive(e.last_used, now, ttl_secs)
            })?;
        }
        Ok((deleted, kept))
    }

    /// 超出 `max_keep` 则按 last_used 升序淘汰。条数上界不超限就直接返回（不扫描）。
    pub fn evict_drafts(&self, schema: &str, max_keep: usize) -> anyhow::Result<usize> {
        if self.ud.upper_bound(D, schema) <= max_keep {
            return Ok(0);
        }
        let mut all: Vec<(i64, String, String)> = Vec::new();
        self.ud.scan(D, schema, "", &mut |e| {
            all.push((e.last_used, e.code, e.text));
            true
        });
        if all.len() <= max_keep {
            return Ok(0);
        }
        all.sort_by_key(|a| a.0);
        let n = all.len() - max_keep;
        let cutoff = all[n - 1].0;
        let doomed: Vec<(String, String)> = all.drain(..n).map(|(_, c, t)| (c, t)).collect();
        let doomed_set: std::collections::BTreeSet<&(String, String)> = doomed.iter().collect();
        self.drop_drafts(schema, &doomed, &mut |e| {
            e.last_used > cutoff || !doomed_set.contains(&(e.code.clone(), e.text.clone()))
        })?;
        Ok(n)
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
        let mut ops = vec![Op::new(
            D,
            schema,
            OpKind::Delete,
            code,
            text,
            Payload::None,
            0,
        )];
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
        self.ud.clear(D, schema)
    }
}
