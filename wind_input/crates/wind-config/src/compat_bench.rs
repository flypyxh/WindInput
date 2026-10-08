//! 兼容规则表的性能基准（`#[ignore]`，不进常规测试）。
//!
//! ```bash
//! cargo test --release -p wind-config -- --ignored bench --nocapture --test-threads=1
//! ```
//!
//! 规模 50 / 500 / 5000 条 `[[apps]]`，窗口规则占 10% / 50%；测三样，均取多轮中位数：
//! - **构建**：三层（出厂 / 定制 / 用户）原始文本 → 解析 → 清理 → 叠加 → 运行时表，即
//!   `AppCompat::load_layered` 去掉读文件的部分；
//! - **冷 resolve**：每次换一个标题，缓存键必不命中（含一次缓存插入）；
//! - **热 resolve**：同一上下文反复解析（缓存命中）。
//!
//! 窗口规则一半只写类名（不限进程，`process = "*"` 那一档，每次 resolve 都要比），
//! 一半是「进程名 + 类名」。

use crate::app_compat::{AppCompat, WindowCtx, sanitize_raw};
use crate::compat_overlay::{Raw, overlay_raw, parse_raw};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// 生成一层规则文本。`offset` 让定制层 / 用户层与出厂层的身份部分重叠（跨层叠加）、部分新增。
fn layer_text(n: usize, window_pct: usize, offset: usize) -> String {
    let mut s = String::from("[[apps]]\nprocess = \"*\"\ncomposition_placeholder = \"space\"\n\n");
    let windows = n * window_pct / 100;
    for i in 0..n {
        let k = i + offset;
        if i < windows {
            if i % 2 == 0 {
                let _ = writeln!(
                    s,
                    "[[apps]]\nclass = \"Chrome_WidgetWin_{k}*\"\ncomposition_placeholder = \"zwsp\"\n"
                );
            } else {
                let _ = writeln!(
                    s,
                    "[[apps]]\nprocess = \"app{k}.exe\"\nclass = \"Win{k}?Cls*\"\ninitial_mode = \"english\"\n"
                );
            }
        } else {
            let _ = writeln!(
                s,
                "[[apps]]\nprocess = \"app{k}.exe\"\ncaret_use_top = true\nfirst_show_mode = \"wait\"\ncaret_offset_x = {}\n",
                k % 7
            );
        }
    }
    s
}

/// 三层：出厂 n 条；定制层 n/10 条（与出厂错开一半）；用户层 n/10 条（与出厂错开 n/20）。
fn layers(n: usize, window_pct: usize) -> [String; 3] {
    [
        layer_text(n, window_pct, 0),
        layer_text((n / 10).max(1), window_pct, n / 2),
        layer_text((n / 10).max(1), window_pct, n / 20),
    ]
}

fn build(texts: &[String; 3]) -> AppCompat {
    let mut raw = Raw::default();
    for t in texts {
        let layer = parse_raw(t).expect("基准夹具必须是合法 TOML");
        raw = overlay_raw(raw, &sanitize_raw(&layer).0);
    }
    AppCompat::from_raw(&raw)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn fmt(d: Duration) -> String {
    let ns = d.as_nanos() as f64;
    if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} µs", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

fn run(n: usize, window_pct: usize) {
    let texts = layers(n, window_pct);
    let build_rounds = if n >= 5000 { 3 } else { 7 };
    let mut builds = Vec::new();
    let mut table = None;
    for _ in 0..build_rounds {
        let t = Instant::now();
        let c = build(&texts);
        builds.push(t.elapsed());
        table = Some(c);
    }
    let table = table.unwrap();

    // 焦点进程：一个纯进程规则的进程（出厂层最后一条，必是纯进程规则）。
    let process = format!("app{}.exe", n - 1);
    let class = "Chrome_WidgetWin_1";
    const COLD_PER_ROUND: usize = 200;
    const HOT_PER_ROUND: usize = 20_000;
    let mut cold = Vec::new();
    for round in 0..9 {
        let titles: Vec<String> = (0..COLD_PER_ROUND)
            .map(|i| format!("t{round}-{i}"))
            .collect();
        let t = Instant::now();
        for title in &titles {
            let r = table.resolve(&WindowCtx {
                process: &process,
                class,
                title,
            });
            std::hint::black_box(&r);
        }
        cold.push(t.elapsed() / COLD_PER_ROUND as u32);
    }
    let ctx = WindowCtx {
        process: &process,
        class,
        title: "fixed",
    };
    let mut hot = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        for _ in 0..HOT_PER_ROUND {
            std::hint::black_box(table.resolve(std::hint::black_box(&ctx)));
        }
        hot.push(t.elapsed() / HOT_PER_ROUND as u32);
    }
    let mut get = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        for _ in 0..HOT_PER_ROUND {
            std::hint::black_box(table.get_rule(std::hint::black_box(&process)));
        }
        get.push(t.elapsed() / HOT_PER_ROUND as u32);
    }
    println!(
        "BENCH n={n:>5} window={window_pct:>2}% | build {:>10} | cold resolve {:>10} | hot resolve {:>10} | get_rule {:>8}",
        fmt(median(builds)),
        fmt(median(cold)),
        fmt(median(hot)),
        fmt(median(get)),
    );
}

#[test]
#[ignore = "性能基准：cargo test --release -p wind-config -- --ignored bench --nocapture"]
fn bench_compat_build_and_resolve() {
    for n in [50, 500, 5000] {
        for pct in [10, 50] {
            run(n, pct);
        }
    }
}

/// 基准夹具自身的正确性：确保冷 / 热路径真的在解析有规则的表（不是在量空表）。
#[test]
fn bench_fixture_resolves_the_expected_rules() {
    let table = build(&layers(50, 50));
    let r = table.resolve(&WindowCtx {
        process: "app49.exe",
        class: "Chrome_WidgetWin_1",
        title: "x",
    });
    let rule = r.rule.as_ref().expect("纯进程规则必须命中");
    assert!(rule.caret_use_top);
    assert_eq!(
        r.window_matches.len(),
        0,
        "Chrome_WidgetWin_1 不命中 _0* 以外的模式"
    );
    let r = table.resolve(&WindowCtx {
        process: "app49.exe",
        class: "Chrome_WidgetWin_0x",
        title: "x",
    });
    assert_eq!(r.window_matches.len(), 1, "Chrome_WidgetWin_0* 命中");
}
