//! S2 补缺：**简拼整句**也认用户词，且**临时词**也算用户词。
//!
//! 现场（用户反馈）：「拜城县」系统词库里没有，用户手打上屏过一次 ⇒ 进了临时词库。
//! 之后打 `bcxrmzf`，整句给的是别的前缀 +「人民政府」，拜城县进不来。两道闸叠在一起：
//!
//! 1. `add_store_nodes` 只收**已晋升**的用户词，临时词被挡。可临时词本身就是「上屏过」
//!    的词——「用过即转正」那道质量闸在**草稿 → 临时**这一跳，不在临时 → 用户。
//!    滑窗杂词住在草稿层，那一层照旧不进图。
//! 2. 简拼整句（step ②b）上的用户词点查用的是全拼码，简拼段（`bcx`）是声母串，
//!    必然落空。系统词走 `add_abbrev_nodes` 查简拼索引，用户层一直缺这一半。
//!
//! 自带 wdat 夹具 + 真 redb store，不依赖 `build_dev/data`。

use std::sync::Arc;
use wind_dict::cached::CachedDict;
use wind_dict::datformat::WdatWriter;
use wind_engine::Engine;
use wind_engine::pinyin::{Config as PyConfig, PinyinEngine};
use wind_store::Store;

/// bai|cheng|xian → 位 0/3/8
const BCX_BOUNDARY: u64 = 0b100001001;

/// 系统词库：`bcx` 下只有「不出现」，`rmzf` 下是「人民政府」。**没有**「拜城县」。
fn sys_dict(tag: &str) -> CachedDict {
    let dir = std::env::temp_dir().join(format!("wind_s2abbr_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wdat = dir.join("t.wdat");
    let mut w = WdatWriter::new();
    // bu|chu|xian → 0/2/5
    w.add_with_boundary(
        "buchuxian".into(),
        vec![("不出现".into(), 517, 0, 0b100101)],
    );
    // ren|min|zheng|fu → 0/3/6/11
    w.add_with_boundary(
        "renminzhengfu".into(),
        vec![("人民政府".into(), 90_000, 0, 0b100001001001)],
    );
    w.add_abbrev("bcx".into(), vec![("buchuxian".into(), 517)]);
    w.add_abbrev("rmzf".into(), vec![("renminzhengfu".into(), 90_000)]);
    w.write(&wdat).unwrap();
    CachedDict::load_at(&dir.join("t.dict.yaml"), &wdat).expect("加载 wdat 夹具")
}

fn store(tag: &str) -> Arc<Store> {
    let p = std::env::temp_dir().join(format!("wind_s2abbr_{tag}.redb"));
    let _ = std::fs::remove_file(&p);
    Arc::new(Store::open(&p).unwrap())
}

fn engine(tag: &str, s: Arc<Store>, on: bool) -> PinyinEngine {
    let dm = wind_dict::manager::DictManager::new();
    dm.register_layer(Box::new(wind_dict::StoreUserLayer::new(
        s.clone(),
        "pinyin",
    )));
    dm.register_layer(Box::new(wind_dict::StoreTempLayer::new(s, "pinyin")));
    let cfg = PyConfig {
        sentence_uses_user_words: on,
        ..Default::default()
    };
    PinyinEngine::new(cfg, sys_dict(tag)).with_store_layers(Arc::new(dm))
}

fn sentence(e: &PinyinEngine, input: &str) -> Option<String> {
    e.convert(input, 100)
        .ok()?
        .candidates
        .into_iter()
        .find(|c| c.is_sentence)
        .map(|c| c.text)
}

/// 用户反馈的原样现场：临时词 + 纯简拼长串。
#[test]
fn temp_word_joins_abbrev_sentence() {
    let s = store("temp");
    // 出厂自动学词权重（coordinator `LEARN_ADD_WEIGHT`）。
    s.learn_temp_word("pinyin", "baichengxian", "拜城县", 800, BCX_BOUNDARY)
        .unwrap();

    let off = sentence(&engine("temp_off", s.clone(), false), "bcxrmzf");
    assert_eq!(
        off.as_deref(),
        Some("不出现人民政府"),
        "前提：开关关闭时整句只认系统词"
    );
    let on = sentence(&engine("temp_on", s, true), "bcxrmzf");
    assert_eq!(on.as_deref(), Some("拜城县人民政府"));
}

/// 已晋升的用户词同样要能走简拼段（第 2 道闸与临时/晋升无关）。
#[test]
fn promoted_word_joins_abbrev_sentence() {
    let s = store("user");
    s.add_user_word("pinyin", "baichengxian", "拜城县", 1200, BCX_BOUNDARY)
        .unwrap();
    let on = sentence(&engine("user_on", s, true), "bcxrmzf");
    assert_eq!(on.as_deref(), Some("拜城县人民政府"));
}

/// 简拼索引交回的是**超集**：`boundary == 0` 的词（手输码、旧版扁平导入）算不出声母串，
/// 按码首字母挂在「无边界组」里，查 `bcx` 时整组一并带回。它们不能进图——整句节点要求
/// 真值切分（同 `add_store_nodes` 约束 2）。
///
/// ⚠️ 本条先验证过可观测性：去掉 `add_store_abbrev_nodes` 里的切分校验，它会红。
#[test]
fn abbrev_store_node_rejects_words_without_boundary() {
    let s = store("nobound");
    s.learn_temp_word("pinyin", "baichengxian", "拜城县", 800, 0)
        .unwrap();
    let on = sentence(&engine("nobound_on", s, true), "bcxrmzf");
    assert_eq!(
        on.as_deref(),
        Some("不出现人民政府"),
        "无边界的词不该进简拼整句"
    );
}
