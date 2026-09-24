//! 稀有标准音节（`dia` 嗲 / `sei` 塞 / `cei` 𤭢）能被全拼打出，且补表不改变易歧义串的既有首选。
//!
//! 标准音节表曾缺这三个音节：切分器不认 ⇒ `sei`/`cei` 切不动（`se` + 残码 `i`），「塞(sēi)」
//! 「𤭢」全拼打不出；`dia` 被切成 `di|a`，「嗲」只靠整码点查碰巧命中，切分本身是错的。
//!
//! **必须用真实词库**（build_dev/data）：要验的是「词库真值读音能被切分器走到」，内存词典测不到。
//! 词库缺失时跳过。

use std::path::PathBuf;
use wind_config::Config;
use wind_engine::EngineManager;

fn data_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("build_dev")
        .join("data");
    p.join("schemas/pinyin/cn_dicts/base.dict.yaml")
        .exists()
        .then_some(p)
}

fn manager(dir: &std::path::Path) -> EngineManager {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".to_string()];
    cfg.schema.active = "pinyin".to_string();
    EngineManager::new(&cfg, Some(dir))
}

/// 界面序（与协调器同一个比较器、同文去重），见 `pinyin_eval.rs` 的「两套位次」。
fn ui_texts(mgr: &EngineManager, input: &str) -> Vec<String> {
    let mut c = mgr.convert_with("pinyin", input, 300).candidates;
    c.sort_by(|a, b| wind_candidate::candidate_display_order(a, b, false, false, input));
    let mut seen = std::collections::HashSet::new();
    c.retain(|x| seen.insert(x.text.clone()));
    c.into_iter().map(|c| c.text).collect()
}

#[test]
fn rare_syllables_type_their_characters() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：拼音词库不存在");
        return;
    };
    let mgr = manager(&dir);

    // `sei`/`cei` 补表前切不动（`se` + 残码 `i`），首选是「色」「侧」。
    for (input, want) in [("dia", "嗲"), ("sei", "塞"), ("cei", "𤭢")] {
        let got = ui_texts(&mgr, input);
        assert_eq!(
            got.first().map(String::as_str),
            Some(want),
            "`{input}` 首选应为「{want}」，实际前 5：{:?}",
            &got[..got.len().min(5)]
        );
        let r = mgr.convert_with("pinyin", input, 10);
        assert_eq!(
            r.completed_syllables,
            [input],
            "`{input}` 应切成一个完整音节（残码 {:?}）",
            r.partial_syllable
        );
    }
}

/// `dia` 补表前被切成 `di|a`：整词「嗲」靠的是整码点查碰巧命中，音节切分（step ① 的查询码、
/// 子短语、晋升校验的边界）全是错的。
///
/// ⚠️ 不断言 `diasheng` 的整句 / 组合区：词库没有「嗲声」，整句由 Viterbi 在 `di|a|sheng` 与
/// `dia|sheng` 间按字频裁决（「地阿胜」胜出，组合区跟随首选显示 `di'a'sheng`），补表不改变它。
#[test]
fn dia_segments_as_one_syllable_inside_longer_input() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：拼音词库不存在");
        return;
    };
    let mgr = manager(&dir);

    let r = mgr.convert_with("pinyin", "diasheng", 10);
    assert_eq!(r.completed_syllables, ["dia", "sheng"]);

    let r = mgr.convert_with("pinyin", "diashengdiaqi", 10);
    assert_eq!(r.completed_syllables, ["dia", "sheng", "dia", "qi"]);
    assert_eq!(
        ui_texts(&mgr, "diashengdiaqi").first().map(String::as_str),
        Some("嗲声嗲气")
    );
}

/// `dia` 让 `diaN…`/`diao…` 多出 `dia|…` 切法；这些串的既有首选不得被它改变。
/// 首选均为补表前的实测值；补表前后对拍的全量结论见 `syllable.rs` 表尾注。
#[test]
fn ambiguous_strings_keep_their_top_choice() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：拼音词库不存在");
        return;
    };
    let mgr = manager(&dir);

    for (input, want) in [
        ("diao", "掉"),
        ("dian", "点"),
        ("diaoyu", "钓鱼"),
        ("diannao", "电脑"),
        ("dianying", "电影"),
        ("diaocha", "调查"),
        ("dianhua", "电话"),
        ("didian", "地点"),
        ("shei", "谁"),
    ] {
        let got = ui_texts(&mgr, input);
        assert_eq!(
            got.first().map(String::as_str),
            Some(want),
            "`{input}` 首选变了，前 5：{:?}",
            &got[..got.len().min(5)]
        );
    }
}
