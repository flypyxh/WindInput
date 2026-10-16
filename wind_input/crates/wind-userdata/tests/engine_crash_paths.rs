//! 引擎级崩溃 / 并发路径（设计 §11）。
use wind_userdata::engine::{Dirs, UserData};
use wind_userdata::record::{OpKind, Payload, Target};

fn dirs(name: &str) -> Dirs {
    let d = std::env::temp_dir().join(format!("wind_ud_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Dirs {
        state_dir: d,
        dir_id: "dir".into(),
    }
}

fn texts(ud: &UserData, code: &str) -> Vec<String> {
    ud.query_exact(Target::Temp, "py", code)
        .into_iter()
        .map(|(t, _w)| t)
        .collect()
}

#[test]
fn write_is_visible_immediately_and_survives_reopen() {
    let d = dirs("vis");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
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
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "写后立即可读");
    drop(ud);
    let ud = UserData::open(d).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"], "重开后由日志回放可读");
}

#[test]
fn fold_then_reopen_reads_from_snapshot_without_journal_replay() {
    let d = dirs("fold");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
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
        Payload::Weight(3),
        0b101,
    )
    .unwrap();
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
    ud.fold(Target::Temp, "py").unwrap();
    assert_eq!(
        texts(&ud, "nihao"),
        vec!["你好"],
        "折回后 delta 清空、快照里是叠加后的结果"
    );
    assert_eq!(ud.view().delta.len(), 0);
    drop(ud);
    let ud = UserData::open(d).unwrap();
    assert_eq!(texts(&ud, "nihao"), vec!["你好"]);
    assert_eq!(ud.view().delta.len(), 0, "已折回的记录不再回放");
}

/// Review Focus 2：state 缺失 ⇒ 以快照内嵌水位为准，只回放更大的 seq，不重复应用。
#[test]
fn state_missing_replays_only_beyond_snapshot_watermark() {
    let d = dirs("wm");
    let ud = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        "a",
        "甲",
        Payload::Weight(1),
        0,
    )
    .unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Incr,
        "a",
        "甲",
        Payload::Count(1),
        0,
    )
    .unwrap();
    ud.fold(Target::Temp, "py").unwrap();
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Incr,
        "a",
        "甲",
        Payload::Count(1),
        0,
    )
    .unwrap(); // 折回后一条
    drop(ud);
    std::fs::remove_file(d.state_dir.join("journal/state")).unwrap();
    let ud = UserData::open(d).unwrap();
    assert_eq!(
        ud.get(Target::Temp, "py", "a", "甲")
            .map(|e| e.count)
            .unwrap_or(0),
        2,
        "快照里 1 + 日志里只回放折回后的 1；重复回放会得 3"
    );
}

/// Review Focus 4：折回期间新到的操作保留。
#[test]
fn fold_keeps_ops_that_arrived_during_fold() {
    let d = dirs("race");
    let ud = std::sync::Arc::new(UserData::open(d).unwrap());
    for i in 0..500 {
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            &format!("c{i}"),
            "x",
            Payload::Weight(1),
            0,
        )
        .unwrap();
    }
    let w = {
        let ud = ud.clone();
        std::thread::spawn(move || {
            for i in 500..600 {
                ud.apply(
                    Target::Temp,
                    "py",
                    OpKind::Upsert,
                    &format!("c{i}"),
                    "x",
                    Payload::Weight(1),
                    0,
                )
                .unwrap();
            }
        })
    };
    ud.fold(Target::Temp, "py").unwrap();
    w.join().unwrap();
    for i in 0..600 {
        assert_eq!(
            ud.query_exact(Target::Temp, "py", &format!("c{i}")).len(),
            1,
            "c{i} 丢了"
        );
    }
}

#[test]
fn second_instance_is_readonly() {
    let d = dirs("ro");
    let a = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap();
    let b = UserData::open(d).unwrap();
    assert!(!a.readonly() && b.readonly());
    assert!(
        b.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "a",
            "甲",
            Payload::Weight(1),
            0
        )
        .is_err()
    );
}

fn up(ud: &UserData, code: &str, text: &str) {
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Upsert,
        code,
        text,
        Payload::Weight(1),
        0,
    )
    .unwrap();
}

fn reopen(d: &Dirs) -> UserData {
    UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: d.dir_id.clone(),
    })
    .unwrap()
}

/// 删掉 journal/（连同里面的 state）后，序号不能倒退到快照水位之下，否则新记录重开即丢。
#[test]
fn deleting_journal_dir_does_not_rewind_seq_below_snapshot() {
    let d = dirs("rmjnl");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    up(&ud, "b", "乙");
    ud.fold(Target::Temp, "py").unwrap();
    drop(ud);
    std::fs::remove_dir_all(d.state_dir.join("journal")).unwrap();
    let ud = reopen(&d);
    up(&ud, "c", "丙");
    up(&ud, "e", "戊");
    drop(ud);
    let ud = reopen(&d);
    assert_eq!(texts(&ud, "a"), vec!["甲"], "快照里的还在");
    assert_eq!(texts(&ud, "c"), vec!["丙"], "删目录后新写的重开可见");
    assert_eq!(texts(&ud, "e"), vec!["戊"]);
}

/// 快照真坏了（不是缺失）：不能信 state 的水位，要从日志整份回放该目标。
#[test]
fn corrupt_snapshot_falls_back_to_full_replay_of_that_target() {
    let d = dirs("corrupt");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    drop(ud);
    std::fs::write(d.state_dir.join("py").join("temp.meta"), b"WUDM").unwrap();
    let ud = reopen(&d);
    assert_eq!(texts(&ud, "a"), vec!["甲"]);
}

/// 只读实例（另一实例持锁）不得改动任何文件：日志尾巴可能是写者正在写的那一条。
#[test]
fn readonly_instance_never_writes_journal_or_device() {
    let d = dirs("rofiles");
    let w = reopen(&d);
    up(&w, "a", "甲");
    let jdir = d.state_dir.join("journal");
    let jpath = std::fs::read_dir(&jdir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "wjl"))
        .unwrap();
    // 模拟写者正写到一半的尾巴
    let mut bytes = std::fs::read(&jpath).unwrap();
    bytes.extend_from_slice(&[0x30, 0, 0, 0, 1, 2]);
    std::fs::write(&jpath, &bytes).unwrap();
    let device = std::fs::read(d.state_dir.join("device.toml")).unwrap();
    // 换了 dir_id 也不能换号封存
    let ro = UserData::open(Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: "other".into(),
    })
    .unwrap();
    assert!(ro.readonly());
    assert_eq!(std::fs::read(&jpath).unwrap(), bytes, "只读实例截了日志");
    assert_eq!(
        std::fs::read(d.state_dir.join("device.toml")).unwrap(),
        device
    );
    assert_eq!(texts(&ro, "a"), vec!["甲"], "只读也能读到写者的记录");
}

/// 换号封存：旧日志里未折回的记录仍可见，新日志接着旧号往下编，不混号。
#[test]
fn renewed_device_keeps_unfolded_records_and_continues_numbering() {
    let d = dirs("renew");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "b", "乙");
    up(&ud, "c", "丙");
    drop(ud);
    let d2 = Dirs {
        state_dir: d.state_dir.clone(),
        dir_id: "dir-B".into(),
    };
    let ud = reopen(&d2);
    assert_eq!(texts(&ud, "b"), vec!["乙"], "封存日志里未折回的仍可见");
    assert_eq!(texts(&ud, "c"), vec!["丙"]);
    let seq = ud
        .apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "e",
            "戊",
            Payload::Weight(1),
            0,
        )
        .unwrap();
    assert!(seq > 3, "新日志接着旧号编，实际 {seq}");
    drop(ud);
    let ud = reopen(&d2);
    for (c, t) in [("a", "甲"), ("b", "乙"), ("c", "丙"), ("e", "戊")] {
        assert_eq!(texts(&ud, c), vec![t], "{c}");
    }
}

/// Review Focus 3：清空（fold_with 返回空）后 state 丢失再重开，被清空的词不复活。
#[test]
fn clear_survives_state_loss() {
    let d = dirs("clear");
    let ud = reopen(&d);
    up(&ud, "a", "甲");
    ud.fold(Target::Temp, "py").unwrap();
    up(&ud, "b", "乙");
    ud.fold_with(Target::Temp, "py", &mut |_| Vec::new())
        .unwrap();
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

fn journal_records(d: &Dirs) -> usize {
    let jdir = d.state_dir.join("journal");
    std::fs::read_dir(&jdir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wjl"))
        .map(|p| {
            wind_userdata::journal::Journal::replay(&p, 0)
                .unwrap()
                .len()
        })
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
    ud.apply(
        Target::Freq,
        "py",
        OpKind::Incr,
        "a",
        "字",
        Payload::Count(1),
        0,
    )
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
    ud.apply(
        Target::Temp,
        "py",
        OpKind::Delete,
        "d",
        "丁",
        Payload::None,
        0,
    )
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
