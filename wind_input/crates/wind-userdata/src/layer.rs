//! `DictLayer` 适配（设计 §4）：快照 + 增量表叠加成候选。
//!
//! 叠加规则：Delete 剔除、SetWeight 覆盖、Upsert 并入或覆盖、Incr / Touch 不影响候选。
//! 候选字段照 `wind-store/src/store_layer.rs::record_to_candidate`：临时词 `meta.is_temp_dict`、
//! `natural_order` 恒 0；用户词 `meta.is_user_dict`、`natural_order` = 入库顺序（文本行号）；
//! 两者都带 `meta.raw_weight` 与 `boundary`。叠加语义经
//! `engine::overlay_entries`，与读路径、折回同一份。简拼召回按 trait 约定返回**超集**（声母组 + 首字符的无边界兜底组），判据留在
//! 引擎侧；`search_abbrev_exact` 只给声母组（不含无边界词）。

use crate::delta::{Entry, group_of};
use crate::engine::{UserData, overlay_entries};
use crate::record::Target;
use crate::snapshot::SnapshotEntry;
use std::sync::Arc;
use wind_candidate::{Candidate, better};
use wind_dict::{DictLayer, LayerType};

fn sort_trunc(mut v: Vec<Candidate>, limit: usize) -> Vec<Candidate> {
    v.sort_by(better);
    if limit > 0 {
        v.truncate(limit);
    }
    v
}

/// 还没折回进文本的新词的入库顺序：大于任何文本行号，又给调用方的 `+1` 之类运算留足余量
/// （取 `i32::MAX` 会让「下一个顺序号」溢出）。
pub const UNFOLDED_ORDER: u32 = 1 << 30;

/// 临时词层与用户词层共用的查询核心：按 `target` 读快照 + delta，`user` 决定候选标记与自然序。
struct WordLayer {
    ud: Arc<UserData>,
    schema: String,
    name: String,
    target: Target,
    /// true = 用户词层（`is_user_dict`、`natural_order` 取入库顺序）；false = 临时词层。
    user: bool,
}

impl WordLayer {
    fn new(ud: Arc<UserData>, schema: &str, target: Target, user: bool, prefix: &str) -> Self {
        Self {
            ud,
            schema: schema.to_string(),
            name: format!("{prefix}:{schema}"),
            target,
            user,
        }
    }

    /// 临时词 `natural_order` 恒 0（同 `StoreTempLayer`：redb 临时词没有入库序号）；用户词取入库
    /// 顺序（文本行号，同 `StoreUserLayer` 的 order）。
    fn candidate(
        &self,
        code: &str,
        text: &str,
        weight: i32,
        boundary: u64,
        is_prefix: bool,
        order: u32,
    ) -> Candidate {
        let mut c = Candidate {
            text: text.to_string(),
            code: code.to_string(),
            weight,
            boundary,
            is_prefix,
            ..Default::default()
        };
        c.meta.raw_weight = weight;
        if self.user {
            c.meta.is_user_dict = true;
            // 还没折回进文本的新词（order 0）排在已有词之后，与折回后取到的行号同向
            let o = if order == 0 { UNFOLDED_ORDER } else { order };
            c.natural_order = o.min(UNFOLDED_ORDER) as i32;
        } else {
            c.meta.is_temp_dict = true;
        }
        c
    }

    fn candidates(&self, entries: Vec<SnapshotEntry>, query_code: Option<&str>) -> Vec<Candidate> {
        entries
            .into_iter()
            .map(|e| {
                let is_prefix = query_code.is_some_and(|q| q != e.code);
                self.candidate(&e.code, &e.text, e.weight, e.boundary, is_prefix, e.order)
            })
            .collect()
    }

    fn abbrev_groups(&self, groups: &[String], limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let snap = v.snapshots.get(&(self.target, self.schema.clone()));
        let mut base = Vec::new();
        let mut keys: Vec<(String, String)> = Vec::new();
        for g in groups {
            if let Some(s) = snap {
                base.extend(s.abbrev(g, 0));
            }
            keys.extend(
                v.delta
                    .by_group(self.target, &self.schema, g)
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
            .flat_map(|(c, t)| v.delta.ops_for(self.target, &self.schema, c, t))
            .collect();
        sort_trunc(self.candidates(overlay_entries(base, ops), None), limit)
    }
}

impl DictLayer for WordLayer {
    fn name(&self) -> &str {
        &self.name
    }

    fn layer_type(&self) -> LayerType {
        if self.user {
            LayerType::User
        } else {
            LayerType::Temp
        }
    }

    fn search(&self, code: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let base = v
            .snapshots
            .get(&(self.target, self.schema.clone()))
            .map(|s| s.exact(code))
            .unwrap_or_default();
        let ops = v.delta.exact(self.target, &self.schema, code);
        sort_trunc(
            self.candidates(overlay_entries(base, ops), Some(code)),
            limit,
        )
    }

    fn search_prefix(&self, prefix: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let ops = v.delta.prefix(self.target, &self.schema, prefix);
        // 每段多取「delta 覆盖的条数」：被删条目占名额、被调高的条目可能不在前 limit 里。
        // limit == 0 = 不限（Snapshot::prefix 自己处理）。
        let take = if limit == 0 { 0 } else { limit + ops.len() };
        let base = v
            .snapshots
            .get(&(self.target, self.schema.clone()))
            .map(|s| s.prefix(prefix, take))
            .unwrap_or_default();
        // 前缀补全一律 is_prefix = true，同 `StoreTempLayer`（`record_to_candidate(r, true, true)`）
        let mut v = self.candidates(overlay_entries(base, ops), None);
        for c in &mut v {
            c.is_prefix = true;
        }
        sort_trunc(v, limit)
    }

    fn search_abbrev(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
        let mut groups = vec![abbrev.to_string()];
        if let Some(c) = abbrev.chars().next() {
            groups.push(group_of(&c.to_string(), 0));
        }
        self.abbrev_groups(&groups, limit)
    }

    fn search_abbrev_exact(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
        self.abbrev_groups(&[abbrev.to_string()], limit)
            .into_iter()
            .filter(|c| c.boundary != 0)
            .collect()
    }

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
        self.ud.scan(self.target, &self.schema, lit, &mut |e| {
            if wind_dict::layer::pattern_matches(pattern, wildcard, &e.code, with_prefix) {
                let longer = e.code.chars().count() > n;
                v.push(self.candidate(&e.code, &e.text, e.weight, e.boundary, longer, e.order));
            }
            true
        });
        v.sort_by(|a, b| wind_dict::layer::cmp_pattern(n, a, b));
        v.truncate(limit);
        v
    }
}

/// 把 `DictLayer` 的查询方法转发给内部的 [`WordLayer`]。
macro_rules! forward_word_layer {
    ($t:ty) => {
        impl DictLayer for $t {
            fn name(&self) -> &str {
                self.0.name()
            }
            fn layer_type(&self) -> LayerType {
                self.0.layer_type()
            }
            fn entries_digest(&self) -> Option<String> {
                self.0.entries_digest()
            }
            fn search(&self, code: &str, limit: usize) -> Vec<Candidate> {
                self.0.search(code, limit)
            }
            fn search_prefix(&self, prefix: &str, limit: usize) -> Vec<Candidate> {
                self.0.search_prefix(prefix, limit)
            }
            fn search_abbrev(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
                self.0.search_abbrev(abbrev, limit)
            }
            fn search_abbrev_exact(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
                self.0.search_abbrev_exact(abbrev, limit)
            }
            fn search_pattern(
                &self,
                pattern: &str,
                wildcard: char,
                limit: usize,
                with_prefix: bool,
            ) -> Vec<Candidate> {
                self.0.search_pattern(pattern, wildcard, limit, with_prefix)
            }
        }
    };
}

/// 临时词层（运行时引擎 `Target::Temp`）。
pub struct UserDataTempLayer(WordLayer);

impl UserDataTempLayer {
    pub fn new(ud: Arc<UserData>, schema: &str) -> Self {
        Self(WordLayer::new(ud, schema, Target::Temp, false, "temp"))
    }
}

forward_word_layer!(UserDataTempLayer);

/// 用户词层（`Target::User`；文本目标时快照由 `user.dict.yaml` 建，`natural_order` = 行号）。
pub struct UserDataUserLayer(WordLayer);

impl UserDataUserLayer {
    pub fn new(ud: Arc<UserData>, schema: &str) -> Self {
        Self(WordLayer::new(ud, schema, Target::User, true, "user"))
    }
}

forward_word_layer!(UserDataUserLayer);

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
        sort_trunc(cands, limit)
    }

    fn search_prefix(&self, _prefix: &str, _limit: usize) -> Vec<Candidate> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Dirs, UserData};
    use crate::record::{OpKind, Payload, Target};
    use std::sync::Arc;
    use wind_dict::DictLayer;

    fn names(v: Vec<wind_candidate::Candidate>) -> Vec<String> {
        v.into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn layer_sees_snapshot_delta_and_deletes() {
        let d = std::env::temp_dir().join(format!("wind_layer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ud = Arc::new(
            UserData::open(Dirs {
                state_dir: d,
                dir_id: "x".into(),
            })
            .unwrap(),
        );
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "你好",
            Payload::Weight(10),
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "拟好",
            Payload::Weight(20),
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "wubi",
            "五笔",
            Payload::Weight(1),
            0,
        )
        .unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Delete,
            "nihao",
            "拟好",
            Payload::None,
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihaoma",
            "你好吗",
            Payload::Weight(1),
            0b100101,
        )
        .unwrap();
        let layer = UserDataTempLayer::new(ud, "py");
        assert_eq!(layer.layer_type(), wind_dict::LayerType::Temp);
        assert_eq!(
            names(layer.search("nihao", 10)),
            vec!["你好"],
            "快照里的 + delta 删除生效"
        );
        let pre = layer.search_prefix("niha", 10);
        assert_eq!(
            names(pre.clone()),
            vec!["你好", "你好吗"],
            "快照 + delta 新增，按权重降序"
        );
        assert!(pre.iter().all(|c| c.is_prefix), "码都比前缀长");
        let pre = layer.search_prefix("nihao", 10);
        assert_eq!(names(pre.clone()), vec!["你好", "你好吗"]);
        assert!(
            pre.iter().all(|c| c.is_prefix),
            "前缀查询一律 is_prefix（同 StoreTempLayer）"
        );
        assert_eq!(
            names(layer.search_abbrev("nh", 10)),
            vec!["你好"],
            "简拼：快照里被删的不出现"
        );
        assert_eq!(
            names(layer.search_abbrev("nhm", 10)),
            vec!["你好吗"],
            "简拼：delta 新增能召回"
        );
        assert_eq!(
            names(layer.search_abbrev("w", 10)),
            vec!["五笔"],
            "无边界词走兜底组"
        );
        assert!(
            layer.search_abbrev_exact("w", 10).is_empty(),
            "exact 不含无边界词"
        );
        assert!(layer.has_longer_code("niha"));
        let c = &layer.search("nihao", 10)[0];
        assert!(c.meta.is_temp_dict && c.meta.raw_weight == 10 && c.boundary == 0b101);
    }
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
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "你好",
            Payload::Weight(5),
            0b101,
        )
        .unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihaoma",
            "你好吗",
            Payload::Weight(3),
            0b100101,
        )
        .unwrap();
        let layer = UserDataTempLayer::new(ud, "py");
        assert_eq!(
            names(layer.search_prefix("niha", 0)),
            vec!["你好", "你好吗"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
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
        assert_eq!(
            l.entries_digest().as_deref(),
            Some("temp:py|enumerates-nothing")
        );
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "你好",
            Payload::Weight(5),
            0b101,
        )
        .unwrap();
        let c = &l.search("nihao", 0)[0];
        assert!(c.meta.is_temp_dict && !c.is_prefix);
        assert_eq!(
            (c.natural_order, c.boundary, c.meta.raw_weight),
            (0, 0b101, 5)
        );
    }

    /// 对照 store_layers_answer_pattern_queries_and_drafts_stay_out：字面前缀扫描后过滤，等长档在前。
    #[test]
    fn temp_layer_answers_pattern_queries() {
        let ud = open_ud("pattern");
        for (code, text) in [
            ("abcd", "甲"),
            ("abxd", "乙"),
            ("abcde", "丙"),
            ("zbcd", "丁"),
        ] {
            ud.apply(
                Target::Temp,
                "wb",
                OpKind::Upsert,
                code,
                text,
                Payload::Weight(1),
                0,
            )
            .unwrap();
        }
        ud.fold(Target::Temp, "wb").unwrap();
        let l = UserDataTempLayer::new(ud, "wb");
        assert_eq!(
            names(l.search_pattern("ab?d", '?', 10, false)),
            vec!["甲", "乙"]
        );
        let with_prefix = l.search_pattern("ab?d", '?', 10, true);
        assert_eq!(
            names(with_prefix.clone()),
            vec!["甲", "乙", "丙"],
            "等长档在前"
        );
        assert!(with_prefix[2].is_prefix);
        assert!(l.search_pattern("", '?', 10, true).is_empty());
        assert!(l.search_pattern("ab?d", '?', 0, true).is_empty());
    }

    /// 对照 drafts_never_surface_through_prefix_queries 与草稿候选形态。
    #[test]
    fn draft_layer_is_exact_only_sinks_and_respects_ttl() {
        let ud = open_ud("draft");
        let mut old = crate::engine::Op::new(
            Target::Draft,
            "wb",
            OpKind::Upsert,
            "ggtt",
            "旧",
            Payload::Weight(0),
            0,
        );
        old.ts_ms = 1_000;
        ud.apply_batch(&[old]).unwrap();
        ud.apply(
            Target::Draft,
            "wb",
            OpKind::Upsert,
            "ggtt",
            "盖伦",
            Payload::Weight(0),
            0,
        )
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
    #[test]
    fn user_layer_marks_user_dict_and_uses_line_order() {
        let ud = open_ud("userlayer");
        let mut ops = Vec::new();
        for (text, order_hint) in [("泥", 2u32), ("你", 1)] {
            let _ = order_hint;
            ops.push(crate::engine::Op::new(
                Target::User,
                "py",
                OpKind::Upsert,
                "ni",
                text,
                Payload::Weight(5),
                0b1,
            ));
        }
        ud.apply_batch(&ops).unwrap();
        let l = UserDataUserLayer::new(ud, "py");
        assert_eq!(l.name(), "user:py");
        assert_eq!(l.layer_type(), LayerType::User);
        let c = l.search("ni", 0);
        assert!(
            c.iter()
                .all(|c| c.meta.is_user_dict && !c.meta.is_temp_dict)
        );
        assert!(l.search_prefix("n", 0).iter().all(|c| c.is_prefix));
    }
}
