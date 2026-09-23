//! 端到端守卫：开了模糊音，按模糊打法也要打得出**用户词**（论坛 t215 / A2-44）。
//!
//! 现场：c=ch 开启后，`caijiuduolian` 能出用户加的「菜就多练」，`chaijiuduolian` 不能。
//! 多打几遍也没用 —— 造词写库用的是规范码（`learn_code`），学进去的仍是
//! `caijiuduolian`。根因是 step 6 只拿用户敲的码查 store，系统词库那条路有
//! `lookup_with_fuzzy` 的变体展开，用户词这条路没有。
//!
//! 内联夹具的单测（`pinyin::tests::fuzzy_*user*`）钉的是召回本身；本文件钉的是
//! 「真实词库下这条候选排得到用户看得见的地方」——系统词库的整句、子短语、模糊单字
//! 都在同一串上竞争，内联夹具里系统词典是空的，测不出这一层。
//!
//! 词库不存在时跳过（判据是耗时 0.0x 秒）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wind_config::Config;
use wind_engine::EngineManager;
use wind_store::Store;

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

/// 用户词表为 `words`、模糊音开 c=ch（`fuzzy = false` 则全关）的引擎。
fn manager(dir: &Path, tag: &str, words: &[(&str, &str)], fuzzy: bool) -> EngineManager {
    let root = std::env::temp_dir().join(format!("wind_pinyin_fuzzy_user_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(Store::open(root.join("user_data.db")).expect("打开 store"));
    for (code, text) in words {
        store
            .add_user_word("pinyin", code, text, 1200, 0)
            .expect("写入用户词");
    }
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".to_string()];
    cfg.schema.active = "pinyin".to_string();
    cfg.schema.pinyin.fuzzy.enabled = fuzzy;
    cfg.schema.pinyin.fuzzy.ch_c = fuzzy;
    EngineManager::with_store_override(
        &cfg,
        Some(dir),
        Some(store),
        Some(root.join("schema_overrides")),
    )
}

fn rank(mgr: &EngineManager, input: &str, text: &str) -> Option<usize> {
    mgr.convert_with("pinyin", input, 50)
        .candidates
        .iter()
        .position(|c| c.text == text)
}

/// 用户原话里的那一串：翘舌打法出平舌码的用户词，且在首页（出厂每页 9 条）。
#[test]
fn retroflex_typing_recalls_flat_user_word_on_first_page() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev 拼音词库不存在");
        return;
    };
    const WORD: &str = "菜就多练";
    let words = [("caijiuduolian", WORD)];

    // 前提：系统词库自己组不出这个词，否则测到的不是用户词这条路。
    let base = manager(&dir, "base", &[], true);
    assert_eq!(
        rank(&base, "chaijiuduolian", WORD),
        None,
        "前提：无用户词时 chaijiuduolian 不得出「{WORD}」"
    );

    let mgr = manager(&dir, "on", &words, true);
    for input in ["chaijiuduolian", "chai'jiu'duo'lian", "caijiuduolian"] {
        let r = rank(&mgr, input, WORD);
        println!("[c=ch] {input} → 「{WORD}」 rank {r:?}");
        assert!(
            r.is_some_and(|i| i < 9),
            "{input} 应在首页出用户词「{WORD}」，实际 rank {r:?}"
        );
    }
}

/// 关模糊音：翘舌打法召不回（对照组，证明上一条的命中来自模糊路径），平舌照旧。
#[test]
fn fuzzy_off_keeps_user_word_exact_only() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：build_dev 拼音词库不存在");
        return;
    };
    const WORD: &str = "菜就多练";
    let mgr = manager(&dir, "off", &[("caijiuduolian", WORD)], false);
    assert_eq!(rank(&mgr, "chaijiuduolian", WORD), None);
    assert_eq!(rank(&mgr, "caijiuduolian", WORD), Some(0));
}
