//! 性能形状探针（`cargo test --release -- --ignored --nocapture`）。不进常规测试：只打印耗时，
//! 供改动前后对比；断言只守住数量级（防回退成 O(全表) 物化）。
use std::sync::Arc;
use std::time::Instant;
use wind_userdata::engine::{Dirs, Op, UserData};
use wind_userdata::record::{OpKind, Payload, Target};
use wind_userdata::runtime::Runtime;

fn open(name: &str) -> Runtime {
    let d = std::env::temp_dir().join(format!("wind_perf_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Runtime::new(Arc::new(
        UserData::open(Dirs {
            state_dir: d,
            dir_id: "dir".into(),
        })
        .unwrap(),
    ))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn needs_maintain_with_large_delta() {
    let r = open("nm");
    let ops: Vec<Op> = (0..6000)
        .map(|i| {
            Op::new(
                Target::Temp,
                "py",
                OpKind::Upsert,
                &format!("c{i}"),
                "字",
                Payload::Weight(1),
                0,
            )
        })
        .collect();
    r.data().apply_batch(&ops).unwrap();
    let t = Instant::now();
    for _ in 0..100 {
        assert!(!r.data().needs_maintain_with(10_000, u64::MAX));
    }
    let per = ms(t) / 100.0;
    println!("needs_maintain (delta 6000): {per:.3} ms/次");
    assert!(per < 0.1, "needs_maintain 退回到逐条扫 delta");
}

#[test]
#[ignore]
fn scan_and_evict_gate_with_large_snapshot() {
    let r = open("scan");
    let n = 190_000;
    r.data()
        .fold_with(Target::Temp, "py", &mut |_| {
            (0..n)
                .map(|i| wind_userdata::snapshot::SnapshotEntry {
                    code: format!("c{i:07}"),
                    text: "字".into(),
                    weight: 1,
                    boundary: 0,
                    count: 1,
                    created_at: 1,
                    last_used: 1,
                    flags: 0,
                    order: 0,
                })
                .collect()
        })
        .unwrap();
    r.learn_temp("py", "zz", "新", 1, 0).unwrap();
    let t = Instant::now();
    assert_eq!(r.count_temp("py"), n + 1);
    println!("count_temp ({n}): {:.1} ms", ms(t));
    let t = Instant::now();
    assert_eq!(r.evict_temp("py", 500_000).unwrap(), 0);
    let gate = ms(t);
    println!("evict_temp 未超上限: {gate:.3} ms");
    assert!(gate < 1.0, "evict_temp 未超上限却扫了全表");
    let t = Instant::now();
    let (page, total) = r.list_freq_paged("py", "", 0, 20);
    println!(
        "list_freq_paged 空表: {:.3} ms ({} / {total})",
        ms(t),
        page.len()
    );
    let t = Instant::now();
    let mut got = 0;
    r.for_each_temp("py", "", &mut |_| {
        got += 1;
        got < 20
    });
    let first = ms(t);
    println!("for_each_temp 前 20 条: {first:.3} ms");
    assert!(first < 5.0, "scan 退回到先物化整表");
}
