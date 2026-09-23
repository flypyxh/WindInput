//! 混合简拼整句（step ②b）与整串简拼词**同层**按权重竞争。
//!
//! 用户反馈 `zhge` → 首选是「之后个」、要的「这个」在第 2。整句是把 `zh` 读成 z|h 两个
//! 声母拼出来的（之后 + 个），权重 3263，比「这个」555006 低两个数量级 —— 它能排第一，
//! 是因为 ②b 整句此前**不标 `is_abbrev`**，落在全拼层，被 `cmp_match_layers` 的
//! 「全拼优先于简拼」整层顶在所有简拼候选之上，权重根本不参与比较。
//!
//! 当初不标的理由是「标了会沉进简拼层，被前缀回退的部分候选压在下面」。现在前缀回退
//! 的候选都带 `is_partial`（同层内排在完整匹配之后），协调器又以消费长度为首键，
//! 那条顾虑已不成立。真实词库界面层实测（2026-09-23）：短简拼 5063 条首选命中
//! 68.48% → 85.50%；长串混合整句（pinyin_eval D 类 1000 条）逐条零差异。
//!
//! 自带 wdat 夹具，权重取 `cn_dicts` 真实值（理由见 `pinyin_mixed_abbrev.rs` 的夹具说明）。

use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};

fn fixture(tag: &str) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_mixed_sentence_layer_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // (code, [(text, weight, order, boundary)])，boundary = 各音节起始字节位
    for (code, entries) in [
        ("zhege", vec![("这个", 555_006, 0b1001u64)]),
        ("zhengge", vec![("整个", 79_081, 0b100001)]),
        ("zhihou", vec![("之后", 249_628, 0b1001)]),
        ("ge", vec![("个", 215_733, 0b1)]),
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
    // 简拼索引存全拼码：`zh` → 之后（z|h 两个声母），`zg` → 这个 / 整个。
    w.add_abbrev("zh".into(), vec![("zhihou".into(), 249_628)]);
    w.add_abbrev(
        "zg".into(),
        vec![("zhege".into(), 555_006), ("zhengge".into(), 79_081)],
    );
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

#[test]
fn mixed_sentence_competes_with_abbrev_words_by_weight() {
    let e = PinyinEngine::new(PyConfig::default(), fixture("zhge"));
    let r = e.convert("zhge", 100).unwrap().candidates;
    let texts: Vec<&str> = r.iter().map(|c| c.text.as_str()).collect();

    // 前提：②b 整句确实产出了（否则本用例测的是空气）。
    let sentence = r
        .iter()
        .find(|c| c.is_sentence)
        .unwrap_or_else(|| panic!("前提：应有 ②b 整句，实际: {texts:?}"));
    assert_eq!(
        sentence.text, "之后个",
        "前提：整句按 z|h|ge 组出，实际: {texts:?}"
    );
    assert!(sentence.is_abbrev, "②b 整句须在简拼层");

    assert_eq!(
        texts[0], "这个",
        "同层按权重，「这个」应压过「之后个」: {texts:?}"
    );
}
