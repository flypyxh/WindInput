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
//! ⚠️ 用例场景已从 `zhge` 换成 `sran`：`RETROFLEX_SPLIT_PENALTY` 落地后，「之后个」这种把 zh
//! 拆成 z|h 的整句过不了质量闸门、根本不再产出，`zhge` 于是测不到「整句与简拼词同层」这件事
//! （它的首选由 `pinyin_retroflex_split.rs` 管）。`sran` 的「虽然按钮」全是单字母声母、不涉及
//! 翘舌，整句照常产出，正好只剩层级这一个变量。
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
        ("suiran", vec![("虽然", 195_193, 0b1001u64)]),
        ("anniu", vec![("按钮", 7_109, 0b101)]),
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
    // 简拼索引存全拼码：`sr` → 虽然、`an` → 按钮（a|n 两个声母）。
    w.add_abbrev("sr".into(), vec![("suiran".into(), 195_193)]);
    w.add_abbrev("an".into(), vec![("anniu".into(), 7_109)]);
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

/// `sran`：整句「虽然按钮」（sr + an）与整串混合简拼词「虽然」（s + ran）同层，按权重。
#[test]
fn mixed_sentence_competes_with_abbrev_words_by_weight() {
    let e = PinyinEngine::new(PyConfig::default(), fixture("sran"));
    let r = e.convert("sran", 100).unwrap().candidates;
    let texts: Vec<&str> = r.iter().map(|c| c.text.as_str()).collect();

    // 前提：②b 整句确实产出了（否则本用例测的是空气）。
    let sentence = r
        .iter()
        .find(|c| c.is_sentence)
        .unwrap_or_else(|| panic!("前提：应有 ②b 整句，实际: {texts:?}"));
    assert_eq!(
        sentence.text, "虽然按钮",
        "前提：整句按 s|r + a|n 组出，实际: {texts:?}"
    );
    assert!(sentence.is_abbrev, "②b 整句须在简拼层");

    assert_eq!(
        texts[0], "虽然",
        "同层按权重，「虽然」应压过「虽然按钮」: {texts:?}"
    );
}
