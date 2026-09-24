//! 混合整句（②b）里简拼段抢走前一全拼音节韵尾的读法要扣分（`CODA_STEAL_PENALTY`）。
//!
//! `ningbr`（宁波人）里 `ning` 是完整音节，简拼节点却能从它中间的 `n` 起头，读成
//! ni + n|g + b|r，组出「你能够比如」压过「宁波人」。
//!
//! 取值与评测依据见常量文档（pinyin_eval 界面序 E 类 74.40% → 76.20%）。
//!
//! 自带 wdat 夹具，权重取 `cn_dicts` 真实值（理由见 `pinyin_mixed_abbrev.rs` 的夹具说明）。

use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};

/// `with_ningbo`：是否放进「宁」「宁波人」。不放时，抢韵尾是组句的唯一读法。
fn fixture(tag: &str, with_ningbo: bool) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_coda_steal_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // (code, text, weight, boundary)，boundary = 各音节起始字节位
    // ning|bo|ren → 位 0/4/6；nen|gou 的 boundary 同理按字节位。
    let mut words = vec![
        ("ni", "你", 492_791, 0b1u64),
        ("nenggou", "能够", 117_534, 0b10001),
        ("biru", "比如", 250_303, 0b101),
    ];
    if with_ningbo {
        // 「宁」不可少：真实词库里保住 `ning` 的整句路径（宁 + 比如）正是靠它存在的。惩罚后
        // Viterbi 改走这条，而它过不了质量闸门 ⇒ 整句不出、「宁波人」登顶。缺了它，抢韵尾
        // 就成了唯一路径，测的是一个真实词库里不存在的局面。
        words.push(("ning", "宁", 1_192, 0b1));
        words.push(("ningboren", "宁波人", 60, 0b1010001));
    }
    words.sort_by(|a, b| a.0.cmp(b.0));
    for (code, text, weight, boundary) in &words {
        w.add_with_boundary(
            (*code).into(),
            vec![((*text).into(), *weight, 0, *boundary)],
        );
    }
    // 简拼索引存全拼码。
    w.add_abbrev("ng".into(), vec![("nenggou".into(), 117_534)]);
    w.add_abbrev("br".into(), vec![("biru".into(), 250_303)]);
    if with_ningbo {
        w.add_abbrev("nbr".into(), vec![("ningboren".into(), 60)]);
    }
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

/// **界面序**候选文本：与协调器同一个比较器排序、同文去重（同 `pinyin_eval` 的界面序）。
///
/// 不能看引擎序：整句不出之后，引擎序首位是只吃掉 `ning` 的部分候选「宁」，界面上协调器以
/// 消费长度为首键，吃满整串的「宁波人」才是首选。
fn texts(tag: &str, with_ningbo: bool) -> Vec<String> {
    let input = "ningbr";
    let mut c = PinyinEngine::new(PyConfig::default(), fixture(tag, with_ningbo))
        .convert(input, 300)
        .unwrap()
        .candidates;
    c.sort_by(|a, b| wind_candidate::candidate_display_order(a, b, false, false, input));
    let mut seen = std::collections::HashSet::new();
    c.into_iter()
        .map(|c| c.text)
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// 用户把 `ning` 打全了：要的是「宁波人」，不是「你能够比如」。
#[test]
fn coda_steal_loses_to_word_keeping_the_syllable() {
    let t = texts("with", true);
    assert_eq!(t.first().map(String::as_str), Some("宁波人"), "{t:?}");
}

/// 是**罚**不是禁：词库里没有任何保住 `ning` 的读法时，整句照样组得出。
#[test]
fn coda_steal_still_composes_when_it_is_the_only_one() {
    let t = texts("without", false);
    assert!(t.contains(&"你能够比如".to_string()), "{t:?}");
}
