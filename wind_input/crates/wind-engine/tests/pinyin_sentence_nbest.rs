//! 整句 N-best：`sentence_count`（露几条）与 `sentence_max_count`（算几条）。
//!
//! 起因是可验证性：整句只出一条、赢者通吃，S2（用户词进整句词图）赢了才看得见、
//! 输了什么线索都没有。
//!
//! 本文件只管**引擎侧**：出几条、名次对不对、池子装了什么。把整句块摆到候选最前是
//! 协调器的事（`place_sentence_block`），不在这里测。
//!
//! 自带 wdat 夹具，不依赖 `build_dev/data`。

use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};

/// `you|gai|lun|ma` 四段，中间两段各有两个字可选 ⇒ 至少 4 种互不相同的整句。
fn dict(tag: &str) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_nbest_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // ⚠️ 同一个 code 只能 add 一次（writer 不去重，DAT 构建假设编码唯一）。
    for (code, entries) in [
        ("you", vec![("有", 500_000)]),
        ("ma", vec![("吗", 300_000)]),
        ("gai", vec![("该", 80_000), ("盖", 20_000)]),
        ("lun", vec![("论", 60_000), ("轮", 30_000)]),
    ] {
        w.add_with_boundary(
            code.into(),
            entries
                .into_iter()
                .enumerate()
                .map(|(i, (t, wt))| (t.into(), wt, i as u32, 0b1))
                .collect(),
        );
    }
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

fn engine(tag: &str, count: u8, max: u8) -> PinyinEngine {
    let cfg = PyConfig {
        sentence_count: count,
        sentence_max_count: max,
        ..Default::default()
    };
    PinyinEngine::new(cfg, dict(tag))
}

/// (名次, 文本)
type Ranked = Vec<(u8, String)>;

/// (候选里的整句按名次, 池子里的整句按名次)
fn sentences(e: &PinyinEngine, input: &str) -> (Ranked, Ranked) {
    let r = e.convert(input, 100).expect("convert");
    let mut shown: Ranked = r
        .candidates
        .iter()
        .filter(|c| c.sentence_rank > 0)
        .map(|c| (c.sentence_rank, c.text.clone()))
        .collect();
    shown.sort();
    let pool = r
        .sentence_pool
        .iter()
        .map(|c| (c.sentence_rank, c.text.clone()))
        .collect();
    (shown, pool)
}

/// 出厂 (1, 1)：只一条整句、池子为空 —— 与 N-best 之前逐位相同的那条路。
///
/// 池子为空是协调器判「没有可切换的整句」的依据，切换键据此不吃键。
#[test]
fn factory_default_is_single_sentence_and_empty_pool() {
    let (shown, pool) = sentences(&engine("default", 1, 1), "yougailunma");
    assert_eq!(shown.len(), 1, "出厂只露一条: {shown:?}");
    assert_eq!(shown[0].0, 1);
    assert!(pool.is_empty(), "出厂池子必须为空: {pool:?}");
}

/// 露 3 算 3：候选里有 3 条**互不相同**的整句，名次 1..=3，池子与之一致。
#[test]
fn shows_ranked_distinct_alternatives() {
    let (shown, pool) = sentences(&engine("three", 3, 3), "yougailunma");
    assert_eq!(
        shown.iter().map(|x| x.0).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "名次连续: {shown:?}"
    );
    let mut texts: Vec<&String> = shown.iter().map(|x| &x.1).collect();
    texts.dedup();
    assert_eq!(texts.len(), 3, "按文本去重后仍是 3 条: {shown:?}");
    assert_eq!(shown[0].1, "有该论吗", "最优解不变（权重最高的那条）");
    assert_eq!(pool, shown, "露的 == 算的时，池子就是候选里那几条");
}

/// 算得多、露得少：池子装满 K 条，候选里只露前 N 条。
#[test]
fn pool_holds_more_than_shown() {
    let (shown, pool) = sentences(&engine("pool", 2, 4), "yougailunma");
    assert_eq!(shown.len(), 2, "露 2: {shown:?}");
    assert_eq!(pool.len(), 4, "算 4: {pool:?}");
    assert_eq!(&pool[..2], &shown[..], "池子前 N 条就是露出来的那几条");
}

/// 算的少于露的：按露的抬。配置写反了不该让用户看到的条数比自己要的还少。
#[test]
fn max_below_count_is_lifted() {
    let (shown, pool) = sentences(&engine("lifted", 3, 1), "yougailunma");
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert_eq!(pool.len(), 3, "{pool:?}");
}

/// 上限 8：配到 200 也只算 8 条（解码代价随 K 线性增长）。
#[test]
fn counts_are_capped() {
    let cfg = PyConfig {
        sentence_count: 200,
        sentence_max_count: 200,
        ..Default::default()
    };
    assert_eq!(cfg.sentence_counts(), (8, 8));
    // 0 也不该把整句关掉——那是 use_smart_compose 的活。
    let cfg0 = PyConfig {
        sentence_count: 0,
        sentence_max_count: 0,
        ..Default::default()
    };
    assert_eq!(cfg0.sentence_counts(), (1, 1));
}
