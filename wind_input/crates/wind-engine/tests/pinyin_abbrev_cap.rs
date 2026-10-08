//! 简拼缩写上限 `schema.pinyin.abbrev.max_syllables`（GH#180）。
//!
//! 数的是**缩写掉的音节**（只打了声母的段），不是候选字数：`hlyb` 缩 4 个、`nhao` 缩 1 个、
//! `bzdhaobuhao` 缩 3 个。每条支路（纯简拼 / 混合简拼 / 简拼整句 / 前缀回退 / 用户层）
//! 各数各的，这里逐条钉住；`max = 0` 不限，与改动前一致。

use std::path::PathBuf;

use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::EngineManager;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};

fn fixture(tag: &str) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_abbrev_cap_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");

    let mut w = WdatWriter::new();
    w.add_with_boundary("nihao".into(), vec![("你好".into(), 5328, 0, 0b101)]);
    w.add_with_boundary("nihaoma".into(), vec![("你好吗".into(), 166, 0, 0b100101)]);
    w.add_with_boundary(
        "buzhidao".into(),
        vec![("不知道".into(), 62492, 0, 0b100101)],
    );
    w.add_with_boundary("haobuhao".into(), vec![("好不好".into(), 20000, 0, 0b1001)]);
    w.add_with_boundary("hao".into(), vec![("好".into(), 90000, 0, 0b1)]);
    w.add_with_boundary("bu".into(), vec![("不".into(), 90000, 0, 0b1)]);
    // he|le|yi|bei
    w.add_with_boundary(
        "heleyibei".into(),
        vec![("喝了一杯".into(), 3000, 0, 0b1010101)],
    );
    w.add_abbrev("nh".into(), vec![("nihao".into(), 9000)]);
    w.add_abbrev("nhm".into(), vec![("nihaoma".into(), 2000)]);
    w.add_abbrev("bzd".into(), vec![("buzhidao".into(), 7000)]);
    w.add_abbrev("hbh".into(), vec![("haobuhao".into(), 5000)]);
    w.add_abbrev("hlyb".into(), vec![("heleyibei".into(), 3000)]);
    w.write(&wdat).unwrap();

    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

fn engine(tag: &str, max: usize) -> PinyinEngine {
    let cfg = PyConfig {
        abbrev_max_syllables: max,
        ..PyConfig::default()
    };
    PinyinEngine::new(cfg, fixture(tag))
}

fn texts(e: &PinyinEngine, input: &str) -> Vec<String> {
    e.convert(input, 50)
        .map(|r| r.candidates.into_iter().map(|c| c.text).collect())
        .unwrap_or_default()
}

fn has(t: &[String], w: &str) -> bool {
    t.iter().any(|s| s == w)
}

/// 纯简拼：缩写数 = 字母数。上限 3 挡 4 字母的 `hlyb`，放 2 字母的 `nh`。
#[test]
fn plain_abbrev_over_cap_is_dropped() {
    let e = engine("plain_cap3", 3);
    let t = texts(&e, "hlyb");
    assert!(
        !has(&t, "喝了一杯"),
        "上限 3 时 hlyb（缩 4 个）不该出「喝了一杯」: {t:?}"
    );
    let t = texts(&e, "nh");
    assert!(
        has(&t, "你好"),
        "上限 3 时 nh（缩 2 个）应照常出「你好」: {t:?}"
    );
}

/// 边界取闭区间：缩写数恰好等于上限时放行。
#[test]
fn plain_abbrev_at_cap_is_kept() {
    let t = texts(&engine("plain_cap4", 4), "hlyb");
    assert!(
        has(&t, "喝了一杯"),
        "上限 4 时 hlyb（缩 4 个）应出「喝了一杯」: {t:?}"
    );
}

/// 0 = 不限：与没有这个设置时一样。
#[test]
fn zero_means_unlimited() {
    let t = texts(&engine("plain_cap0", 0), "hlyb");
    assert!(
        has(&t, "喝了一杯"),
        "上限 0 不限，hlyb 应出「喝了一杯」: {t:?}"
    );
}

/// 混合简拼数的是声母段：`nihm` = ni + h + m 缩 2 个，`nhm` 纯简拼缩 3 个。
/// 上限 2 时前者在、后者不在——同一个词，按用户实际缩了多少决定。
#[test]
fn mixed_abbrev_counts_initial_segments_only() {
    let e = engine("mixed_cap2", 2);
    let t = texts(&e, "nihm");
    assert!(
        has(&t, "你好吗"),
        "nihm 只缩 2 个，上限 2 应出「你好吗」: {t:?}"
    );
    let t = texts(&e, "nhm");
    assert!(
        !has(&t, "你好吗"),
        "nhm 缩 3 个，上限 2 不该出「你好吗」: {t:?}"
    );
}

/// 混合模式里声母段超了上限，这条模式就不参与召回：`hlybei` = [h][l][y][bei] 缩 3 个。
#[test]
fn mixed_pattern_over_cap_is_dropped() {
    let t = texts(&engine("mixedpat_cap3", 3), "hlybei");
    assert!(
        has(&t, "喝了一杯"),
        "上限 3 时 hlybei（缩 3 个）应出「喝了一杯」: {t:?}"
    );
    let t = texts(&engine("mixedpat_cap2", 2), "hlybei");
    assert!(
        !has(&t, "喝了一杯"),
        "上限 2 时 hlybei（缩 3 个）不该出「喝了一杯」: {t:?}"
    );
}

/// 简拼整句（step 2b）按解码路径缩掉的音节数判：`bzdhaobuhao` 缩 3 个。
#[test]
fn mixed_sentence_counts_abbreviated_path_segments() {
    let t = texts(&engine("sent_cap3", 3), "bzdhaobuhao");
    assert!(
        has(&t, "不知道好不好"),
        "上限 3 时 bzdhaobuhao（缩 3 个）应组出整句: {t:?}"
    );
    let t = texts(&engine("sent_cap2", 2), "bzdhaobuhao");
    assert!(
        !has(&t, "不知道好不好"),
        "上限 2 时 bzdhaobuhao（缩 3 个）不该组出整句: {t:?}"
    );
}

/// 前缀回退（step 6.2）：`bzdha` 整串落空后退到 `bzd`，那段缩 3 个，同样受上限约束。
#[test]
fn prefix_fallback_respects_cap() {
    let t = texts(&engine("fallback_cap0", 0), "bzdha");
    assert!(
        has(&t, "不知道"),
        "不限时 bzdha 应退到 bzd 出「不知道」: {t:?}"
    );
    let t = texts(&engine("fallback_cap2", 2), "bzdha");
    assert!(
        !has(&t, "不知道"),
        "上限 2 时 bzd 段缩 3 个，不该出「不知道」: {t:?}"
    );
}

// ------------------------------------------------- 经 EngineManager 的接线（真实词库）

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

fn mgr_texts(mgr: &EngineManager, schema: &str, input: &str) -> Vec<String> {
    mgr.convert_with(schema, input, 300)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

/// `[schema.pinyin.abbrev] max_syllables` 传到了引擎：全拼方案 `hlyb` 的「喝了一杯」随之出没。
#[test]
fn manager_wires_max_syllables() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let on = mgr_texts(&manager(&dir, "pinyin", |_| {}), "pinyin", "hlyb");
    assert!(
        has(&on, "喝了一杯"),
        "出厂不限，hlyb 应出「喝了一杯」: {:?}",
        &on[..on.len().min(20)]
    );
    let capped = mgr_texts(
        &manager(&dir, "pinyin", |c| c.schema.pinyin.abbrev.max_syllables = 3),
        "pinyin",
        "hlyb",
    );
    assert!(
        !has(&capped, "喝了一杯"),
        "上限 3 时不该出「喝了一杯」: {:?}",
        &capped[..capped.len().min(20)]
    );
}

/// `enabled = false` 传到了引擎：`nh` 不再出「你好」，全拼 `nihao` 照旧。
#[test]
fn manager_wires_enabled() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let mgr = manager(&dir, "pinyin", |c| c.schema.pinyin.abbrev.enabled = false);
    let t = mgr_texts(&mgr, "pinyin", "nh");
    assert!(
        !has(&t, "你好"),
        "关掉简拼后 nh 不该出「你好」: {:?}",
        &t[..t.len().min(20)]
    );
    let t = mgr_texts(&mgr, "pinyin", "nihao");
    assert!(
        has(&t, "你好"),
        "关掉简拼不影响全拼: {:?}",
        &t[..t.len().min(20)]
    );
}

/// 整句里 `a` / `e` / `o` 作声母时同样计数：`agzyhaobuhao` 的「爱国主义」缩了 a、g、z、y
/// 4 个。只看字面（`a` 是音节）会漏数，上限 3 就被绕过（审查实测）。
#[test]
fn sentence_counts_vowel_initials() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let mgr = manager(&dir, "pinyin", |c| c.schema.pinyin.abbrev.max_syllables = 3);
    let t = mgr_texts(&mgr, "pinyin", "agzyhaobuhao");
    assert!(
        !t.iter().any(|s| s.starts_with("爱国主义")),
        "上限 3 时 agzyhaobuhao（缩 4 个）不该组出整句: {:?}",
        &t[..t.len().min(20)]
    );
    let open = mgr_texts(&manager(&dir, "pinyin", |_| {}), "pinyin", "agzyhaobuhao");
    assert!(
        open.iter().any(|s| s.starts_with("爱国主义")),
        "不限时应组出「爱国主义…」整句（否则上面那条测不到东西）: {:?}",
        &open[..open.len().min(20)]
    );
}

/// 模糊拼写不是缩写：开 in_ing 后 `wmtinbudong` 的 `tin`（听）是全拼，只缩了 w、m 两个。
#[test]
fn fuzzy_spelling_is_not_abbreviation() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev/data 下没有拼音词库");
        return;
    };
    let mgr = manager(&dir, "pinyin", |c| {
        c.schema.pinyin.abbrev.max_syllables = 2;
        c.schema.pinyin.fuzzy.enabled = true;
        c.schema.pinyin.fuzzy.in_ing = true;
    });
    let t = mgr_texts(&mgr, "pinyin", "wmtinbudong");
    assert!(
        has(&t, "我们听不懂"),
        "上限 2 时 wmtinbudong（缩 2 个）应组出「我们听不懂」: {:?}",
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
    let off = |c: &mut wind_config::Config| c.schema.pinyin.abbrev.enabled = false;
    let t = mgr_texts(&manager(&dir, "pinyin", off), "pinyin", "haol");
    assert!(
        has(&t, "好了"),
        "关掉简拼后 haol 仍应出「好了」: {:?}",
        &t[..t.len().min(20)]
    );
    let t = mgr_texts(&manager(&dir, "shuangpin", off), "shuangpin", "hcl");
    assert!(
        has(&t, "好了"),
        "关掉简拼后小鹤 hcl 仍应出「好了」: {:?}",
        &t[..t.len().min(20)]
    );
}
