//! 词语联想的**用户词文本索引**：按文本前缀取用户词 / 临时词（论坛 t185）。
//!
//! # 为什么要单独建一份
//!
//! 系统词的联想走反查索引（`.dict.yaml` 构建，词 → 编码），store 层的用户词与临时词
//! 不在里面。而 store 的键是 `schema\0code\0text`——按**编码**有序，按文本前缀查只能
//! 全表扫；用户词库实测可达十九万条，联想跑在上屏的同步链路上，扫不起。
//!
//! 于是在内存里按**文本**排一份（紧凑布局：一整块文本 + 定长条目），前缀查询是二分 +
//! 顺序走，与反查索引同形。
//!
//! # 何时过期
//!
//! store 的写代次（[`wind_store::Store::words_generation`]）一变就过期。过期时**本次
//! 照用旧索引**，另起后台线程重建（单飞）——绝不在按键线程上扫表。代价是刚写入的词
//! 要到重建完成后的下一次上屏才进联想（毫秒级到百毫秒级），换来按键路径零扫描。
//!
//! 只收两字及以上的词：联想要的是「上文的严格延长」，上文至少一个字，单字永远轮不到。

use std::sync::{Arc, Mutex};

/// 一层（用户词或临时词）按文本排序的紧凑表。
#[derive(Default)]
struct TextTable {
    /// 全部词文本首尾相接。
    buf: String,
    /// (在 `buf` 中的起点, 字节长, 权重)，按文本字节序升序、文本唯一。
    entries: Vec<(u32, u32, i32)>,
}

impl TextTable {
    /// `words` 可重复、可乱序；同文本多码时取最大权重。
    fn build(mut words: Vec<(String, i32)>) -> Self {
        words.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        words.dedup_by(|b, a| a.0 == b.0);
        let mut t = TextTable {
            buf: String::with_capacity(words.iter().map(|w| w.0.len()).sum()),
            entries: Vec::with_capacity(words.len()),
        };
        for (text, w) in words {
            t.entries.push((t.buf.len() as u32, text.len() as u32, w));
            t.buf.push_str(&text);
        }
        t
    }

    fn text(&self, e: &(u32, u32, i32)) -> &str {
        &self.buf[e.0 as usize..(e.0 + e.1) as usize]
    }

    /// 以 `prefix` 开头且严格更长的词，按权重降序（同权按文本）取前 `limit` 条。
    fn with_prefix(&self, prefix: &str, limit: usize) -> Vec<(&str, i32)> {
        let start = self.entries.partition_point(|e| self.text(e) < prefix);
        let mut hits: Vec<(&str, i32)> = self.entries[start..]
            .iter()
            .map(|e| (self.text(e), e.2))
            .take_while(|(t, _)| t.starts_with(prefix))
            .filter(|(t, _)| t.len() > prefix.len())
            .collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        hits.truncate(limit);
        hits
    }
}

/// 某个数据方案（store 归属 id）的用户词 + 临时词文本索引。
pub struct UserAssocIndex {
    data_schema: String,
    /// 建索引**之前**读到的写代次。先读后扫：扫描期间若有写入，代次已前进，下次必判过期。
    generation: u64,
    user: TextTable,
    temp: TextTable,
}

impl UserAssocIndex {
    /// 全量扫 store 建索引（**会扫整张用户词表**，只在后台线程 / 预热 / 测试里调）。
    pub fn build(store: &wind_store::Store, data_schema: &str) -> Self {
        let generation = store.words_generation();
        let (mut user, mut temp) = (Vec::new(), Vec::new());
        let keep = |v: &mut Vec<(String, i32)>, w: wind_store::user_words::UserWordView<'_>| {
            if w.text.chars().nth(1).is_some() {
                v.push((w.text.to_string(), w.weight));
            }
            true
        };
        let r = store
            .for_each_user_word(data_schema, "", &mut |w| keep(&mut user, w))
            .and_then(|_| store.for_each_temp_word(data_schema, "", &mut |w| keep(&mut temp, w)));
        if let Err(e) = r {
            tracing::warn!("联想用户词索引：读 store 失败 schema={data_schema}: {e}");
        }
        let (user, temp) = (TextTable::build(user), TextTable::build(temp));
        UserAssocIndex {
            data_schema: data_schema.to_string(),
            generation,
            user,
            temp,
        }
    }

    pub fn user_with_prefix(&self, prefix: &str, limit: usize) -> Vec<(&str, i32)> {
        self.user.with_prefix(prefix, limit)
    }

    pub fn temp_with_prefix(&self, prefix: &str, limit: usize) -> Vec<(&str, i32)> {
        self.temp.with_prefix(prefix, limit)
    }
}

/// 缓存槽：只留一份（当前联想方案的），外加「后台重建进行中」单飞标记。
#[derive(Default)]
pub(crate) struct UserAssocSlot {
    index: Option<Arc<UserAssocIndex>>,
    building: bool,
}

pub(crate) type SharedSlot = Arc<Mutex<UserAssocSlot>>;

/// 取可用索引（可能略旧）；缺失或过期时起一次后台重建（已有在建则不重复起）。
///
/// 方案不符时返回 `None`（不拿别的方案的用户词顶替）。
pub(crate) fn get_or_refresh(
    slot: &SharedSlot,
    store: &Arc<wind_store::Store>,
    data_schema: &str,
) -> Option<Arc<UserAssocIndex>> {
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    let current = g
        .index
        .as_ref()
        .filter(|i| i.data_schema == data_schema)
        .cloned();
    let fresh = current
        .as_ref()
        .is_some_and(|i| i.generation == store.words_generation());
    if !fresh && !g.building {
        g.building = true;
        let (slot2, store2, schema2) = (slot.clone(), store.clone(), data_schema.to_string());
        let spawned = std::thread::Builder::new()
            .name("user-assoc-index".into())
            .spawn(move || {
                let idx = Arc::new(UserAssocIndex::build(&store2, &schema2));
                let mut g = slot2.lock().unwrap_or_else(|e| e.into_inner());
                g.index = Some(idx);
                g.building = false;
            });
        if let Err(e) = spawned {
            tracing::warn!("联想用户词索引：起重建线程失败: {e}");
            g.building = false;
        }
    }
    current
}

/// 阻塞地建好并放进槽（预热 / 测试用）。已是最新则不重建。
pub(crate) fn prewarm(slot: &SharedSlot, store: &wind_store::Store, data_schema: &str) -> bool {
    {
        let g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.index.as_ref().is_some_and(|i| {
            i.data_schema == data_schema && i.generation == store.words_generation()
        }) {
            return false;
        }
    }
    let idx = Arc::new(UserAssocIndex::build(store, data_schema));
    slot.lock().unwrap_or_else(|e| e.into_inner()).index = Some(idx);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_strict_sorted_by_weight_and_deduped() {
        let t = TextTable::build(vec![
            ("荷载".into(), 5),
            ("荷".into(), 99),
            ("荷载效应".into(), 7),
            ("荷载".into(), 9),
            ("花卉".into(), 100),
            ("荷包".into(), 1),
        ]);
        assert_eq!(
            t.with_prefix("荷", 10),
            vec![("荷载", 9), ("荷载效应", 7), ("荷包", 1)]
        );
        assert_eq!(t.with_prefix("荷载", 10), vec![("荷载效应", 7)]);
        assert_eq!(t.with_prefix("荷", 1), vec![("荷载", 9)]);
        assert!(t.with_prefix("无", 10).is_empty());
    }
}
