//! 码表逆切分的端到端验证：从按键入口走到候选窗显示与上屏。
//!
//! 引擎单测（`codetable/engine.rs` 的 `split_*`）证明的是组合候选的构造；本文件证明的是
//! 两件只有走完协调器才看得见的事：
//!
//! - **段候选吃候选调整**（论坛 t231）：`EngineManager` 真的把 store 注入了引擎。用户在
//!   段码上删掉的词不得被拼进组合——引擎单测里是手工 `with_segment_shadow`，漏接线照样绿；
//! - **次选只显示后段**（论坛 t231 / t11）：显示与上屏在协调器出口分叉，首条组合由
//!   显示出口 `cand_display_text` 判定为整串；
//! - **段内置顶**活过协调器的重排（引擎单测只看得到引擎序，审查实测曾被按权重排回）。
//!
//! 现场 `aaab`：五笔 86 里是空码，`aa`（式/戒…）与 `ab`（节/蒸…）都有重码。
//! 逆切分出厂关，经方案级 override 打开。
//!
//! ⚠️ 依赖 `build_dev/data` 真实词库；缺失时**静默跳过**（判据是耗时 0.00s）。

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

fn has_data() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}

fn key(k: u32) -> KeyEventData {
    KeyEventData {
        key_code: k,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

fn press(coord: &Coordinator, code: &str) {
    for c in code.chars() {
        coord.handle_key_event(&key(u32::from(c.to_ascii_uppercase())));
    }
}

/// 每个用例独立的 store 与 override 目录（并发写同一文件会撕裂）。
/// `extra` 追加到 `[engine.codetable]` 段。
fn coord_with(tag: &str, extra: &str) -> (Arc<Coordinator>, Arc<Store>) {
    let root = std::env::temp_dir().join(format!("wind_split_e2e_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    let ov = root.join("overrides");
    std::fs::create_dir_all(&ov).unwrap();
    std::fs::write(
        ov.join("wubi86.toml"),
        format!("[engine.codetable]\nsplit_input = true\n{extra}"),
    )
    .unwrap();
    let store = Arc::new(Store::open(root.join("user_data.db")).unwrap());
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into()];
    c.schema.active = "wubi86".into();
    c.input.default.chinese_mode = true;
    c.keys.select_char_keys = vec!["comma_period".into()];
    let coord = Coordinator::new_headless_with_store_override(
        c,
        Some(&data_dir()),
        store.clone(),
        Some(ov),
    );
    (coord, store)
}

const INPUT: &str = "aaab";

#[test]
fn alt_candidates_show_back_segment_but_commit_whole() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    let (coord, _store) = coord_with("alt", "");
    press(&coord, INPUT);
    let texts = coord.debug_page_texts();
    let shown = coord.debug_page_display_texts();
    assert!(
        texts.len() >= 2,
        "前提：`ab` 有重码，应有 ≥2 条组合，实际 {texts:?}"
    );
    assert_eq!(shown[0], texts[0], "首选（组合区）显示整串");
    assert!(
        shown[1] != texts[1] && texts[1].ends_with(shown[1].as_str()),
        "次选只显示后段：显示 {:?}，整串 {:?}",
        shown[1],
        texts[1]
    );
    // 选 ② 上屏的是**整串**，不是显示的那半截。
    match coord.handle_key_event(&key(u32::from(b'2'))) {
        KeyAction::InsertText { text, .. } => assert_eq!(text, texts[1]),
        other => panic!("选 ② 应上屏整串，实际 {other:?}"),
    }
}

/// 反向对照：`split_alt_display = "full"` ⇒ 显示与上屏同形（证明分叉来自这个开关）。
#[test]
fn full_display_keeps_whole_text() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    let (coord, _store) = coord_with("full", "split_alt_display = \"full\"\n");
    press(&coord, INPUT);
    let texts = coord.debug_page_texts();
    assert!(texts.len() >= 2, "前提：应有 ≥2 条组合，实际 {texts:?}");
    assert_eq!(coord.debug_page_display_texts(), texts);
    // ★ 前提对照：同一现场在默认档下**确实**分叉——否则 `aaab` 哪天不再走切分（词库加了
    // 条目），本条会因为「根本没有组合候选」而假绿。
    let (back, _store) = coord_with("full_ctl", "");
    press(&back, INPUT);
    assert_ne!(
        back.debug_page_display_texts()[1],
        back.debug_page_texts()[1]
    );
}

/// 论坛 t231：段码上被删除的词不再参与组合。
#[test]
fn deleted_segment_word_is_not_composed() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    let (coord, store) = coord_with("del", "");
    press(&coord, INPUT);
    let before = coord.debug_page_texts();
    let shown = coord.debug_page_display_texts();
    assert!(before.len() >= 2, "前提：应有 ≥2 条组合，实际 {before:?}");
    // 删掉后段 `ab` 的次选（即 ② 显示的那个词）。
    let victim = shown[1].clone();
    store.delete_shadow("wubi86", "ab", &victim).unwrap();

    coord.handle_key_event(&key(0x1B)); // Esc 清缓冲
    press(&coord, INPUT);
    let after = coord.debug_page_texts();
    assert!(
        !after.iter().any(|t| t == &before[1]),
        "`ab` 上删掉的「{victim}」仍被拼进组合：{after:?}"
    );
    assert_eq!(after.first(), before.first(), "首选不受影响");
}

/// 段内置顶要活过协调器的重排：在 `ab` 上把次选置顶，组合首选随之换成它。
#[test]
fn pinned_segment_word_leads_composition() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    let (coord, store) = coord_with("pin", "");
    press(&coord, INPUT);
    let before = coord.debug_page_texts();
    let shown = coord.debug_page_display_texts();
    assert!(before.len() >= 2, "前提：应有 ≥2 条组合，实际 {before:?}");
    store
        .pin_shadow("wubi86", "ab", &shown[1], None, 0)
        .unwrap();

    coord.handle_key_event(&key(0x1B));
    press(&coord, INPUT);
    let after = coord.debug_page_texts();
    assert_eq!(
        after[0], before[1],
        "`ab` 上置顶的「{}」应成为组合首选",
        shown[1]
    );
    // 首条换了人，显示整串的也跟着换（首条判定在显示出口，不认引擎给的位置）。
    assert_eq!(coord.debug_page_display_texts()[0], after[0]);
}

/// 以词定字按**所见**取字：高亮「② 蒸」时按 `,` 上屏「式蒸」的前段 + 「蒸」，而不是整串
/// 第一个字「式」。
#[test]
fn select_char_on_alt_uses_shown_back_segment() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    const VK_COMMA: u32 = 0xBC;
    const VK_DOWN: u32 = 0x28;
    let (coord, _store) = coord_with("selchar", "");
    press(&coord, INPUT);
    let texts = coord.debug_page_texts();
    let shown = coord.debug_page_display_texts();
    let front = texts[1]
        .strip_suffix(shown[1].as_str())
        .expect("前提：② 只显示后段");
    coord.handle_key_event(&key(VK_DOWN));
    let first_shown: String = shown[1].chars().take(1).collect();
    match coord.handle_key_event(&key(VK_COMMA)) {
        KeyAction::InsertText { text, .. } => assert_eq!(text, format!("{front}{first_shown}")),
        other => panic!("以词定字应上屏，实际 {other:?}"),
    }
}
