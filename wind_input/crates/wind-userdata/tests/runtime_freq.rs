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
    assert_eq!(
        page.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
        vec!["b", "c"]
    );
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
        (
            "a".to_string(),
            "甲".to_string(),
            FreqRec {
                count: 9,
                last_used: 1,
            },
        ),
        (
            "b".to_string(),
            "乙".to_string(),
            FreqRec {
                count: 2,
                last_used: 5,
            },
        ),
    ];
    assert_eq!(r.import_freq_rows("py", &rows).unwrap(), 2);
    assert_eq!(
        r.freq("py", "a", "甲").unwrap(),
        FreqRec {
            count: 9,
            last_used: mine.last_used
        }
    );
    assert_eq!(
        r.freq("py", "b", "乙").unwrap(),
        FreqRec {
            count: 2,
            last_used: 5
        }
    );
}

#[test]
fn assoc_history_orders_by_count_then_recent() {
    let (_d, r) = open("assoc");
    r.record_assoc_pick("py", "天", "气").unwrap();
    r.record_assoc_pick("py", "天", "空").unwrap();
    r.record_assoc_pick("py", "天", "空").unwrap();
    r.record_assoc_pick("py", "", "空").unwrap();
    let h = r.assoc_history("py", "天", 0);
    assert_eq!(
        h.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        vec!["空", "气"]
    );
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
