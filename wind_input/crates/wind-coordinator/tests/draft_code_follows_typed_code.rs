//! GH#181①：自动造词的草稿码必须跟着**用户实际打的码**走。
//!
//! # 现场（小鹤音形，「好嘞」）
//!
//! 用户打 `hc`「好」、`lw`「嘞」上屏，回打 `hclw` 召不回「好嘞」——草稿落在了 `hcle` 下。
//! 根因：草稿的码由单字全码表重新推，「嘞」有 `le…`/`lw…` 两条同长全码，全码表按权重挑了
//! `le…`，而用户打的是 `lw`。
//!
//! # 本文件用真实 wubi86 复现同一机制
//!
//! 出厂词库里「彧」有两条同长全码 `akge`(1247) / `gkgy`(978)，全码表挑 `akge`。用户打
//! `gkgy` 上屏「彧」、再打 `xnn` 上屏「幻」，按五笔二字词公式（AaAbBaBb）草稿应落在
//! `gk`+`xn` = `gkxn` 下；修复前落在 `akxn` 下，回打 `gkxn` 永远召不回。
//!
//! 缺 `build_dev/data` 时静默跳过。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;
use wind_store::Store;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_schemas() -> bool {
    data_dir().join("schemas/wubi86.schema.toml").exists()
}

fn key(key_code: u32) -> KeyEventData {
    KeyEventData {
        key_code,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

fn committed_text(a: &KeyAction) -> Option<&str> {
    match a {
        KeyAction::InsertText { text, .. } | KeyAction::InsertTextWithCursor { text, .. } => {
            Some(text)
        }
        KeyAction::CommitAndHoldComposition { commit_text, .. }
        | KeyAction::CommitThenDeferComposition { commit_text, .. } => Some(commit_text),
        _ => None,
    }
}

/// 打 `code`，再按数字键选中 `want` 上屏（4 码唯一自动上屏也认）。
fn type_and_pick(coord: &Coordinator, code: &str, want: &str) {
    for ch in code.chars() {
        let a = coord.handle_key_event_policed(&key(ch.to_ascii_uppercase() as u32));
        if let Some(t) = committed_text(&a) {
            assert_eq!(t, want, "{code} 自动上屏的不是「{want}」");
            return;
        }
    }
    let items = coord.candidate_window(0, 9).items;
    let pos = items
        .iter()
        .position(|t| t == want)
        .unwrap_or_else(|| panic!("{code} 的首页候选里没有「{want}」: {items:?}"));
    let a = coord.handle_key_event_policed(&key(0x31 + pos as u32));
    assert_eq!(
        committed_text(&a),
        Some(want),
        "选第 {} 个候选应上屏「{want}」，实际 {a:?}",
        pos + 1
    );
}

fn wait_draft(store: &Store, code: &str) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let found = store.search_drafts("wubi86", code, 0).unwrap_or_default();
        if !found.is_empty() || std::time::Instant::now() >= deadline {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn draft_code_uses_the_code_the_user_actually_typed() {
    if !has_schemas() {
        eprintln!("跳过：缺少 schema");
        return;
    }
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.codetable.auto_phrase.enabled = true;

    let db = std::env::temp_dir().join("wind_draft_typed_code_gkxn.redb");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(&db).unwrap());
    let coord = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), Arc::clone(&store));
    coord.prewarm_indexes();

    type_and_pick(&coord, "gkgy", "彧");
    type_and_pick(&coord, "xnn", "幻");

    let found = wait_draft(&store, "gkxn");
    let stale = store.search_drafts("wubi86", "akxn", 0).unwrap_or_default();
    let _ = std::fs::remove_file(&db);
    assert!(
        found.iter().any(|t| t == "彧幻"),
        "用户打 gkgy 上屏「彧」，草稿应落在 gkxn 下；实际 gkxn -> {found:?}，akxn -> {stale:?}"
    );
    assert!(
        !stale.iter().any(|t| t == "彧幻"),
        "不该再按全码表的 akge 落到 akxn 下：{stale:?}"
    );
}

/// 〇（U+3007）不是 `is_han`，曾一上屏就让滑窗断流，「二〇」这类年份 / 编号词永远造不出来。
/// wubi86 给了 〇 码 `llll`，草稿应按「二 fg + 〇 ll」落在 `fgll` 下。
#[test]
fn draft_window_keeps_ling_and_encodes_it() {
    if !has_schemas() {
        eprintln!("跳过：缺少 schema");
        return;
    }
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.codetable.auto_phrase.enabled = true;

    let db = std::env::temp_dir().join("wind_draft_ling_fgll.redb");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(Store::open(&db).unwrap());
    let coord = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), Arc::clone(&store));
    coord.prewarm_indexes();

    type_and_pick(&coord, "fg", "二");
    type_and_pick(&coord, "llll", "〇");

    let found = wait_draft(&store, "fgll");
    let _ = std::fs::remove_file(&db);
    assert!(
        found.iter().any(|t| t == "二〇"),
        "「二」「〇」连续上屏，草稿应有 fgll -> 二〇；实际 {found:?}"
    );
}
