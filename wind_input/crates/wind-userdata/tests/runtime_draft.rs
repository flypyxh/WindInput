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
    let mut op = Op::new(
        Target::Draft,
        schema,
        OpKind::Upsert,
        code,
        text,
        Payload::Weight(0),
        0,
    );
    op.ts_ms = secs * 1000;
    r.data().apply_batch(&[op]).unwrap();
}

const DAY: i64 = 86_400;

/// 对照 drafts_are_found_by_exact_code_only。
#[test]
fn drafts_are_found_by_exact_code_only() {
    let r = rt("exact");
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())])
        .unwrap();
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
    assert_eq!(
        r.search_drafts("wb", "aaaa", 0).len(),
        2,
        "ttl 0 = 永不过期"
    );
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
    assert_eq!(
        r.purge_expired_drafts(Some("wb"), 0).unwrap(),
        (0, 0),
        "ttl 0 不清"
    );
}

/// 对照 promote_draft_to_temp：草稿删除、临时词新建 count 1；已有临时词则 count+1。
#[test]
fn promote_draft_moves_it_into_temp() {
    let r = rt("promote");
    assert!(
        !r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0)
            .unwrap(),
        "没有草稿"
    );
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())])
        .unwrap();
    let g = r.words_gen("wb");
    assert!(
        r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0)
            .unwrap()
    );
    assert!(!r.has_draft("wb", "ggtt", "盖伦", 0));
    let t = r.temp_word("wb", "ggtt", "盖伦").unwrap();
    assert_eq!((t.weight, t.count), (800, 1));
    assert!(r.words_gen("wb") > g);
    r.add_drafts("wb", &[("ggtt".into(), "盖伦".into())])
        .unwrap();
    r.promote_draft_to_temp("wb", "ggtt", "盖伦", 800, 0)
        .unwrap();
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

/// 审查 I1：没东西可删时不折回（不重建整段）；少量删除走 Delete 记录，不整段重写。
#[test]
fn purge_and_evict_do_not_rebuild_for_small_changes() {
    let r = rt("cheap");
    for c in ["a", "b", "c"] {
        r.add_drafts("wb", &[(c.into(), "字".into())]).unwrap();
    }
    let pending = || r.data().pending(Target::Draft, "wb");
    assert_eq!(pending(), 3);
    assert_eq!(r.purge_expired_drafts(Some("wb"), DAY).unwrap(), (0, 3));
    assert_eq!(pending(), 3, "无过期：不折回");
    assert_eq!(r.evict_drafts("wb", 10).unwrap(), 0);
    assert_eq!(pending(), 3, "未超上限：不折回");
    assert_eq!(r.evict_drafts("wb", 2).unwrap(), 1);
    assert_eq!(pending(), 4, "少量淘汰：一条 Delete，不整段重建");
    assert_eq!(r.count_drafts("wb"), 2);
}
