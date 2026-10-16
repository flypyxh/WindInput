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
    assert_eq!(
        r.temp_word("py", "xian", "先").unwrap().weight,
        TEMP_WORD_MAX_WEIGHT
    );
    r.learn_temp("py", "xian", "先", 1, 0b1).unwrap();
    assert_eq!(
        r.temp_word("py", "xian", "先").unwrap().boundary,
        0b1,
        "旧为 0 时补上"
    );
    r.learn_temp("py", "xian", "先", 1, 0b101).unwrap();
    assert_eq!(
        r.temp_word("py", "xian", "先").unwrap().boundary,
        0b1,
        "已有则沿用"
    );
}

/// 对照 test_increment_if_exists。
#[test]
fn increment_only_when_present() {
    let r = rt("incr");
    assert_eq!(
        r.increment_temp_if_exists("py", "a", "甲").unwrap(),
        (false, 0)
    );
    assert!(r.temp_word("py", "a", "甲").is_none(), "不创建");
    r.learn_temp("py", "a", "甲", 10, 0).unwrap();
    assert_eq!(
        r.increment_temp_if_exists("py", "a", "甲").unwrap(),
        (true, 2)
    );
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
    let mut a = wind_userdata::engine::Op::new(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "a",
        "旧",
        Payload::Weight(5),
        0,
    );
    a.ts_ms = 1_000_000;
    let mut ai = wind_userdata::engine::Op::new(
        Target::Temp,
        "py",
        OpKind::Incr,
        "a",
        "旧",
        Payload::Count(1),
        0,
    );
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
    assert!(
        r.temp_word("py", "a", "旧").is_none(),
        "count 相同先删最老的"
    );
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
            &[
                row("xian", "先", 50, 7, 0b1),
                row("xian", "西安", 99_999, 2, 0b101),
            ],
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
    assert_eq!(
        r.schemas().into_iter().collect::<Vec<_>>(),
        vec!["py".to_string()]
    );
}
