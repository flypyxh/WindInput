//! 混输超码长归属：拼音的**混合简拼整句**吃满整串时，拼音必须主张这个串。
//!
//! `pinyin_claims_overflow` 的第二条是「拼音首选解释了整串」，实现上取 `convert(input, 1)`
//! 的第一条。`3934d5cc` 让 ②b 整句带上 `is_abbrev`（与整串简拼词同层按权重竞争）之后，
//! 引擎序里它排到了所有非简拼候选之后：`wobzd` 的第一条变成只吃 `wo` 的「我」，认领判假；
//! 恰好前 4 码 `wobz` 又是码表全码 ⇒ `codetable_owns_overflow` 把整串判给码表：码表候选
//! 「测」回到首位，顶码判定也随之归码表。
//!
//! 只在混输开了 `schema.mix.enable_pinyin_abbrev`（出厂关）时可达；本文件直接构造
//! `PinyinEngine`，其 `Config::default()` 的 `enable_abbrev` 为 true。
//!
//! 拼音侧用 wdat 夹具：简拼索引只有 wdat 词典才有（`CachedDict::Memory` 返回空）。

use std::sync::Arc;
use wind_dict::cached::CachedDict;
use wind_dict::codetable::CodetableDict;
use wind_dict::datformat::WdatWriter;
use wind_dict::{DictManager, SystemDictLayer};
use wind_engine::codetable::{CodeTableEngine, CommitOptions};
use wind_engine::mixed::{MixConfig, MixedEngine};
use wind_engine::pinyin::Config as PinyinConfig;
use wind_engine::{Engine, PinyinEngine};

/// 五笔侧：`wobz` 是精确全码（「前 N 码是精确全码」那一条成立，变量只剩拼音认不认领）。
fn wubi() -> Box<dyn Engine> {
    let mut d = CodetableDict::empty();
    d.merge_single("wobz".into(), "测".into(), 900, 0);
    let dm = DictManager::new();
    dm.register_layer(Box::new(SystemDictLayer::new(CachedDict::Memory(d), "sys")));
    Box::new(CodeTableEngine::new(
        4,
        CommitOptions {
            top_code_commit: true,
            ..Default::default()
        },
        Arc::new(dm),
    ))
}

/// 拼音侧：权重取 `cn_dicts` 真实值。
fn pinyin() -> PinyinEngine {
    let dir = std::env::temp_dir().join("wind_mixed_overflow_abbrev_sentence");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // 按码排序加入（DAT 构建假设编码有序唯一）。bu|zhi|dao → 位 0/2/5
    w.add_with_boundary(
        "buzhidao".into(),
        vec![("不知道".into(), 62_492, 0, 0b100101)],
    );
    w.add_with_boundary("wo".into(), vec![("我".into(), 889_049, 0, 0b1)]);
    w.add_abbrev("bzd".into(), vec![("buzhidao".into(), 62_492)]);
    w.write(&wdat).unwrap();
    let dict = CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具");
    PinyinEngine::new(PinyinConfig::default(), dict)
}

fn mixed() -> MixedEngine {
    MixedEngine::new(
        wubi(),
        Some(Box::new(pinyin())),
        None,
        MixConfig {
            pinyin_only_overflow: true,
            ..Default::default()
        },
    )
}

#[test]
fn mixed_abbrev_sentence_keeps_pinyin_claim_on_overflow() {
    // 前提：拼音单独就能组出吃满整串的混合整句（否则测的是别的分支）。
    let py = pinyin().convert("wobzd", 50).unwrap().candidates;
    assert!(
        py.iter().any(|c| c.is_sentence && c.text == "我不知道"),
        "前提：拼音应有混合整句「我不知道」，实际 {:?}",
        py.iter().map(|c| &c.text).collect::<Vec<_>>()
    );

    let got: Vec<String> = mixed()
        .convert("wobzd", 20)
        .unwrap()
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect();
    // 拼音主张了整串 ⇒ `pinyin_only_overflow` 下超码长只出拼音（与英文），码表不得认领。
    // 认领失败时「测」会回到首位——码表认领同时决定顶码（`handle_top_code`），用户打
    // `wobzd` 想要「我不知道」，得到的却是按五笔前 4 码顶出的「测」。
    assert!(
        !got.contains(&"测".to_string()),
        "混合整句吃满整串，拼音应主张该串、码表不得认领，实际 {got:?}"
    );
    assert!(got.contains(&"我不知道".to_string()), "{got:?}");
}
