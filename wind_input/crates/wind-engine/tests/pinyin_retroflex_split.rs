//! 混合整句（②b）里 zh/ch/sh 被拆成两个声母的读法要扣分（`RETROFLEX_SPLIT_PENALTY`）。
//!
//! 简拼节点按「一个字母一个声母」读，`sh` 会被读成 s|h：「生活」(sheng|huo) 的简拼恰好
//! 是 `sh`，于是 `shzhe` 组出「生活这」、压过用户要的「试着」。
//!
//! 取值与评测依据见常量文档（pinyin_eval 界面序 E 类 64.40% → 74.40%）。
//!
//! 自带 wdat 夹具，权重取 `cn_dicts` 真实值（理由见 `pinyin_mixed_abbrev.rs` 的夹具说明）。

use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};

fn fixture(tag: &str) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_retroflex_split_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // (code, [(text, weight, boundary)])，boundary = 各音节起始字节位
    for (code, entries) in [
        ("shizhe", vec![("试着", 6_110, 0b1001u64)]),
        ("shenghuo", vec![("生活", 66_451, 0b100001)]),
        ("zhe", vec![("这", 323_192, 0b1), ("着", 43_529, 0b1)]),
        ("zuihou", vec![("最后", 207_726, 0b1001)]),
        ("hai", vec![("还", 206_072, 0b1)]),
    ] {
        w.add_with_boundary(
            code.into(),
            entries
                .into_iter()
                .enumerate()
                .map(|(i, (t, wt, b))| (t.into(), wt, i as u32, b))
                .collect(),
        );
    }
    // 简拼索引存全拼码：`sh` → 生活（s|h）、`sz` → 试着、`zh` → 最后（z|h）。
    w.add_abbrev("sh".into(), vec![("shenghuo".into(), 66_451)]);
    w.add_abbrev("sz".into(), vec![("shizhe".into(), 6_110)]);
    w.add_abbrev("zh".into(), vec![("zuihou".into(), 207_726)]);
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

fn texts(input: &str, tag: &str) -> Vec<String> {
    PinyinEngine::new(PyConfig::default(), fixture(tag))
        .convert(input, 100)
        .unwrap()
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

/// 用户反馈同类现场：`shzhe` 要的是「试着」（sh|zhe），不是「生活这」（s|h + zhe）。
#[test]
fn split_reading_loses_to_retroflex_word() {
    let t = texts("shzhe", "shzhe");
    assert_eq!(t.first().map(String::as_str), Some("试着"), "{t:?}");
}

/// 是**罚**不是禁：只有拆开这一种读法时，整句照样组得出（「最后」的简拼就是 `zh`）。
#[test]
fn split_reading_still_composes_when_it_is_the_only_one() {
    let t = texts("zhhai", "zhhai");
    assert!(t.contains(&"最后还".to_string()), "{t:?}");
}
