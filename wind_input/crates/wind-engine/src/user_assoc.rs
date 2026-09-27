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

/// 一条命中：词、它的一个记录码（供查 FREQ）、层内排序键与使用次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserHit<'a> {
    pub text: &'a str,
    pub code: &'a str,
    /// 层内排序键（降序）：用户词 = 权重；临时词 = `count << 32 | created_at`。
    pub rank: i64,
    /// store 记录的使用次数（临时词的门槛看它；用户词仅作参考）。
    pub count: u32,
}

/// 建表输入的一行：(词, 码, 排序键, 次数)。
type Row = (String, String, i64, u32);

/// 一层（用户词或临时词）按文本排序的紧凑表。
#[derive(Default)]
struct TextTable {
    /// 全部「词 + 码」首尾相接。
    buf: String,
    /// 按文本字节序升序、文本唯一。
    entries: Vec<Entry>,
}

struct Entry {
    off: u32,
    text_len: u16,
    code_len: u16,
    rank: i64,
    count: u32,
}

impl TextTable {
    /// `rows` 可重复、可乱序；同文本多码时取排序键最大的那条（次数取和）。
    fn build(mut rows: Vec<Row>) -> Self {
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)));
        let mut t = TextTable::default();
        for (text, code, rank, count) in rows {
            if text.len() > u16::MAX as usize || code.len() > u16::MAX as usize {
                continue;
            }
            if let Some(last) = t.entries.last_mut()
                && t.buf[last.off as usize..last.off as usize + last.text_len as usize] == *text
            {
                last.count = last.count.saturating_add(count);
                continue;
            }
            t.entries.push(Entry {
                off: t.buf.len() as u32,
                text_len: text.len() as u16,
                code_len: code.len() as u16,
                rank,
                count,
            });
            t.buf.push_str(&text);
            t.buf.push_str(&code);
        }
        t
    }

    fn hit(&self, e: &Entry) -> UserHit<'_> {
        let (o, tl, cl) = (e.off as usize, e.text_len as usize, e.code_len as usize);
        UserHit {
            text: &self.buf[o..o + tl],
            code: &self.buf[o + tl..o + tl + cl],
            rank: e.rank,
            count: e.count,
        }
    }

    /// 以 `prefix` 开头且严格更长、且过 `keep` 的词，按排序键降序（同键按文本）取前 `limit` 条。
    fn with_prefix(
        &self,
        prefix: &str,
        limit: usize,
        keep: impl Fn(&UserHit<'_>) -> bool,
    ) -> Vec<UserHit<'_>> {
        let start = self.entries.partition_point(|e| self.hit(e).text < prefix);
        let mut hits: Vec<UserHit<'_>> = self.entries[start..]
            .iter()
            .map(|e| self.hit(e))
            .take_while(|h| h.text.starts_with(prefix))
            .filter(|h| h.text.len() > prefix.len() && keep(h))
            .collect();
        hits.sort_by(|a, b| b.rank.cmp(&a.rank).then(a.text.cmp(b.text)));
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
    ///
    /// ★ 临时词的排序键以 **count** 打头：临时词权重是写入时的定值（自动造词恒为
    /// `LEARN_ADD_WEIGHT`），按权重排等于按字典序排。
    pub fn build(store: &wind_store::Store, data_schema: &str) -> Self {
        let generation = store.words_generation();
        let (mut user, mut temp): (Vec<Row>, Vec<Row>) = (Vec::new(), Vec::new());
        let multi = |t: &str| t.chars().nth(1).is_some();
        let r = store
            .for_each_user_word(data_schema, "", &mut |w| {
                if multi(w.text) {
                    user.push((w.text.into(), w.code.into(), w.weight as i64, w.count));
                }
                true
            })
            .and_then(|_| {
                store.for_each_temp_word(data_schema, "", &mut |w| {
                    if multi(w.text) {
                        let rank = ((w.count as i64) << 32) | (w.created_at & 0xFFFF_FFFF);
                        temp.push((w.text.into(), w.code.into(), rank, w.count));
                    }
                    true
                })
            });
        if let Err(e) = r {
            tracing::warn!("联想用户词索引：读 store 失败 schema={data_schema}: {e}");
        }
        UserAssocIndex {
            data_schema: data_schema.to_string(),
            generation,
            user: TextTable::build(user),
            temp: TextTable::build(temp),
        }
    }

    pub fn user_with_prefix(&self, prefix: &str, limit: usize) -> Vec<UserHit<'_>> {
        self.user.with_prefix(prefix, limit, |_| true)
    }

    /// 临时词；`keep` 按次数分档（个人档 / 补位档）。
    pub fn temp_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
        keep: impl Fn(&UserHit<'_>) -> bool,
    ) -> Vec<UserHit<'_>> {
        self.temp.with_prefix(prefix, limit, keep)
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

    fn texts<'a>(v: Vec<UserHit<'a>>) -> Vec<(&'a str, i64)> {
        v.iter().map(|h| (h.text, h.rank)).collect()
    }

    fn row(t: &str, rank: i64, count: u32) -> Row {
        (t.into(), "c".into(), rank, count)
    }

    #[test]
    fn prefix_is_strict_sorted_by_rank_and_deduped() {
        let t = TextTable::build(vec![
            row("荷载", 5, 1),
            row("荷", 99, 1),
            row("荷载效应", 7, 1),
            row("荷载", 9, 2),
            row("花卉", 100, 1),
            row("荷包", 1, 1),
        ]);
        assert_eq!(
            texts(t.with_prefix("荷", 10, |_| true)),
            vec![("荷载", 9), ("荷载效应", 7), ("荷包", 1)]
        );
        assert_eq!(
            t.with_prefix("荷", 10, |_| true)[0].count,
            3,
            "同文本多码次数取和"
        );
        assert_eq!(
            texts(t.with_prefix("荷载", 10, |_| true)),
            vec![("荷载效应", 7)]
        );
        assert_eq!(texts(t.with_prefix("荷", 1, |_| true)), vec![("荷载", 9)]);
        assert!(t.with_prefix("荷", 10, |h| h.count > 5).is_empty());
        assert!(t.with_prefix("无", 10, |_| true).is_empty());
    }
}
