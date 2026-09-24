//! 联想两处缺口（论坛 t185）：
//!
//! - **A**：词语联想只查系统词库的反查索引，用户词库 / 临时词在联想里永远不出现。
//! - **B**：满码唯一自动上屏（`auto_commit_at_full`）走 `InputOutcome::AutoCommit` 出口，
//!   绕过了 `commit_selected` 末尾的联想接线 ⇒ 开了自动上屏的五笔用户单字上屏后没有联想。
//!
//! 夹具取楼主真机场景：wubi86，`awsk` = 「荷」（全码唯一），用户词「荷载」（`awfa`，
//! 系统词库里该码只有「花卉」）。
//!
//! 词典缺失时自动跳过（`build_dev/data` 不存在时整族静默跳过而计数照绿，判据是耗时）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn dict_ready() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}

fn press(c: &Coordinator, vk: u32) -> KeyAction {
    c.handle_key_event(&KeyEventData {
        key_code: vk,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    })
}

/// 逐字母敲，返回最后一键的动作（满码自动上屏就发生在那一键上）。
fn type_code(c: &Coordinator, code: &str) -> KeyAction {
    let mut last = KeyAction::Consumed;
    for ch in code.chars() {
        last = press(c, (ch.to_ascii_uppercase() as u32) & 0xFF);
    }
    last
}

fn committed(act: &KeyAction) -> Option<&str> {
    match act {
        KeyAction::InsertText { text, .. } => Some(text),
        KeyAction::CommitThenDeferComposition { commit_text, .. } => Some(commit_text),
        _ => None,
    }
}

/// 上屏动作是否**带了占位组合**——联想态能收到后续按键的命门。
fn opened_composition(act: &KeyAction) -> bool {
    match act {
        KeyAction::CommitThenDeferComposition {
            deferred_composition,
            ..
        } => !deferred_composition.is_empty(),
        KeyAction::InsertText {
            new_composition, ..
        } => new_composition.as_deref().is_some_and(|c| !c.is_empty()),
        _ => false,
    }
}

fn coord(
    tag: &str,
    kind: &str,
    at_full: bool,
    seed: impl FnOnce(&wind_store::Store),
) -> (Arc<Coordinator>, Arc<wind_store::Store>) {
    let store_path = std::env::temp_dir().join(format!("wind_assoc_user_{tag}.redb"));
    let _ = std::fs::remove_file(&store_path);
    let store = Arc::new(wind_store::Store::open(&store_path).unwrap());
    seed(&store);
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.input.symbol.smart_mode = false;
    cfg.input.association.kind = kind.to_string();
    cfg.input.association.mode = "continuous".to_string();
    cfg.schema.codetable.auto_commit_at_full = at_full;
    let c = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), store.clone());
    // 生产由启动后的预热线程建索引（系统反查索引 + 用户词文本索引），headless 不跑那个线程。
    c.prewarm_indexes();
    (c, store)
}

/// ★ B：满码唯一自动上屏之后要进联想态（与手动选词上屏一致）。
#[test]
fn auto_commit_at_full_enters_assoc() {
    if !dict_ready() {
        eprintln!("!!! 跳过：build_dev 词库不存在");
        return;
    }
    let (c, _) = coord("b_on", "word", true, |_| {});
    let act = type_code(&c, "awsk");
    assert_eq!(
        committed(&act),
        Some("荷"),
        "前提：awsk 满码唯一自动上屏「荷」"
    );
    assert!(
        opened_composition(&act),
        "自动上屏后该挂占位组合进联想态，实得 {act:?}"
    );
    let texts = c.debug_assoc_texts();
    assert!(
        !texts.is_empty() && texts.iter().all(|t| t.starts_with('荷')),
        "联想候选应是以「荷」开头的词，实得 {texts:?}"
    );
}

/// B 的反向对照：联想关着时自动上屏照旧纯上屏，不挂占位组合。
#[test]
fn auto_commit_at_full_respects_assoc_off() {
    if !dict_ready() {
        return;
    }
    let (c, _) = coord("b_off", "off", true, |_| {});
    let act = type_code(&c, "awsk");
    assert_eq!(committed(&act), Some("荷"));
    assert!(!opened_composition(&act), "联想关着不该挂占位组合：{act:?}");
    assert!(c.debug_assoc_texts().is_empty());
}

/// ★ A：用户词参与词语联想，且排在系统词之前。
#[test]
fn user_word_joins_word_assoc_ahead_of_system() {
    if !dict_ready() {
        return;
    }
    let (c, _) = coord("a_user", "word", false, |s| {
        s.add_user_word("wubi86", "awfa", "荷载", 0, 0).unwrap();
    });
    type_code(&c, "awsk");
    let act = press(&c, 0x20);
    assert_eq!(committed(&act), Some("荷"));
    let texts = c.debug_assoc_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("荷载"),
        "用户词应排在联想首位，实得 {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "荷花"),
        "系统词仍在联想里（排在用户词之后），实得 {texts:?}"
    );
}

/// ★ A：运行中新增的用户词不必重启就能进联想（写代次过期 → 后台重建）。
///
/// 过期时本次联想先用旧索引（不在按键线程上重建），重建完成后下一次上屏即可见。
#[test]
fn user_word_added_at_runtime_joins_word_assoc() {
    if !dict_ready() {
        return;
    }
    let (c, store) = coord("a_fresh", "word", false, |_| {});
    store.add_user_word("wubi86", "awfa", "荷载", 0, 0).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        press(&c, 0x1B); // Esc 退出上一轮联想
        type_code(&c, "awsk");
        press(&c, 0x20);
        if c.debug_assoc_texts().first().map(String::as_str) == Some("荷载") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "新增用户词 5 秒内仍未进联想：{:?}",
            c.debug_assoc_texts()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// ★ A：临时词也参与（排在系统词之后补位）。
#[test]
fn temp_word_joins_word_assoc() {
    if !dict_ready() {
        return;
    }
    let (c, _) = coord("a_temp", "word", false, |s| {
        s.add_user_word("wubi86", "awfa", "荷载", 0, 0).unwrap();
        s.learn_temp_word("wubi86", "awfx", "荷载组合", 10, 0)
            .unwrap();
    });
    // 选「荷载」：awfa 下候选〔花卉 / 荷载〕里找它的位置。
    type_code(&c, "awfa");
    let idx = c
        .debug_page_texts()
        .iter()
        .position(|t| t == "荷载")
        .expect("前提：awfa 下有用户词「荷载」");
    let act = press(&c, 0x31 + idx as u32);
    assert_eq!(committed(&act), Some("荷载"));
    let texts = c.debug_assoc_texts();
    assert!(
        texts.iter().any(|t| t == "荷载组合"),
        "临时词应出现在联想里，实得 {texts:?}"
    );
}
