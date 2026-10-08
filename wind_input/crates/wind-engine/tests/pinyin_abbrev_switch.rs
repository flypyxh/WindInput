//! 简拼开关 `schema.pinyin.abbrev.enabled`（GH#180）经 `EngineManager` 传到拼音引擎。
//!
//! 引擎侧 `enable_abbrev = false` 的行为（不出简拼、全拼照旧）由 `pinyin::tests::
//! abbrev_disabled_suppresses_abbrev_candidates_only` 与 `pinyin_mixed_abbrev` 守着；
//! 这里只钉接线，以及「补全不归简拼开关管」这条容易被误改的边界。

use std::path::PathBuf;

use wind_engine::EngineManager;

fn data_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data");
    p.join("schemas/pinyin/cn_dicts/base.dict.yaml")
        .exists()
        .then_some(p)
}

fn manager(
    dir: &std::path::Path,
    schema: &str,
    f: impl FnOnce(&mut wind_config::Config),
) -> EngineManager {
    let mut cfg = wind_config::Config::default();
    cfg.schema.available = vec![schema.to_string()];
    cfg.schema.active = schema.to_string();
    f(&mut cfg);
    EngineManager::new(&cfg, Some(dir))
}

fn texts(mgr: &EngineManager, schema: &str, input: &str) -> Vec<String> {
    mgr.convert_with(schema, input, 300)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

fn has(t: &[String], w: &str) -> bool {
    t.iter().any(|s| s == w)
}

fn off(c: &mut wind_config::Config) {
    c.schema.pinyin.abbrev.enabled = false;
}

/// `enabled = false` 传到了引擎：`nh` 不再出「你好」，全拼 `nihao` 照旧；出厂开着时照常出。
#[test]
fn manager_wires_enabled() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let on = texts(&manager(&dir, "pinyin", |_| {}), "pinyin", "nh");
    assert!(
        has(&on, "你好"),
        "出厂开着，nh 应出「你好」: {:?}",
        &on[..on.len().min(20)]
    );

    let mgr = manager(&dir, "pinyin", off);
    let t = texts(&mgr, "pinyin", "nh");
    assert!(
        !has(&t, "你好"),
        "关掉简拼后 nh 不该出「你好」: {:?}",
        &t[..t.len().min(20)]
    );
    let t = texts(&mgr, "pinyin", "nihao");
    assert!(
        has(&t, "你好"),
        "关掉简拼不影响全拼: {:?}",
        &t[..t.len().min(20)]
    );
}

/// 打完一个音节再补声母是补全不是简拼：关掉简拼后全拼 `haol`、双拼 `hcl`（小鹤）照常出「好了」。
#[test]
fn trailing_initial_completion_is_not_governed() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let t = texts(&manager(&dir, "pinyin", off), "pinyin", "haol");
    assert!(
        has(&t, "好了"),
        "关掉简拼后 haol 仍应出「好了」: {:?}",
        &t[..t.len().min(20)]
    );
    let t = texts(&manager(&dir, "shuangpin", off), "shuangpin", "hcl");
    assert!(
        has(&t, "好了"),
        "关掉简拼后小鹤 hcl 仍应出「好了」: {:?}",
        &t[..t.len().min(20)]
    );
}
