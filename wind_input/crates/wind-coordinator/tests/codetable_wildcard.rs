//! 码表通配输入（万能键）端到端验证（设计见 docs/design/codetable-wildcard.md §7）。
//!
//! 每条「开启通配」用例都配一条「关闭时同一操作」的对照——只测正向的话，
//! 通配整个没接线、或某键本就这么表现，用例一样会绿。
//!
//! 「开启」一律经 `Config` 走 `build_engine` 的折叠（不直接构造引擎）：`build_engine`
//! 若仍传 `wildcard: None`，`active_wildcard_key()` 恒 `None`，下面的进缓冲用例全红。
//!
//! ⚠️ `build_dev/data` 不存在时整族静默跳过而计数照绿，判据是耗时（正常 ≥1s）
//! 与输出里有没有「跳过」。

use std::path::PathBuf;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_host::KeyProbe;
use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn dict_ready() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}

fn key_event(key_code: u32, modifiers: u32) -> KeyEventData {
    KeyEventData {
        key_code,
        scan_code: 0,
        modifiers,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

/// 字母键（VK = 大写 ASCII）。符号键用 [`press_vk`]。
fn press(coord: &Coordinator, s: &str) -> KeyAction {
    let mut last = KeyAction::Consumed;
    for c in s.chars() {
        debug_assert!(
            c.is_ascii_alphabetic(),
            "符号键的 VK 与字符不同，请用 press_vk"
        );
        last = coord.handle_key_event(&key_event((c.to_ascii_uppercase() as u32) & 0xFF, 0));
    }
    last
}

fn press_vk(coord: &Coordinator, vk: u32, shift: bool) -> KeyAction {
    coord.handle_key_event(&key_event(vk, if shift { MOD_SHIFT } else { 0 }))
}

const VK_SLASH: u32 = 0xBF; // `/`，Shift 即 `?`
const VK_SEMICOLON: u32 = 0xBA; // `;`（出厂次选键）

fn committed(a: &KeyAction) -> Option<&str> {
    match a {
        KeyAction::InsertText { text, .. } => Some(text.as_str()),
        _ => None,
    }
}

fn wubi(wildcard: bool, key: &str) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.codetable.wildcard = wildcard;
    cfg.schema.codetable.wildcard_key = key.into();
    cfg
}

/// 真机出厂 `zz*` 标点短语的最小替身（同 input_flow.rs 的 `zz_system_phrases`）。
fn zz_phrases() -> Vec<wind_phrase::PhraseSeed> {
    let seed = |code: &str, text: &str| wind_phrase::PhraseSeed {
        code: code.into(),
        text: text.into(),
        weight: 0,
        position: 0,
        is_system: true,
        category: String::new(),
    };
    vec![seed("zzbd", "、"), seed("zzsz", "…")]
}

// ─────────────────────────── 进缓冲裁决（Task 10） ───────────────────────────

/// 非首位字母通配键进缓冲，即使它不在 `input_chars`（`a-y`）里。
/// 对照：关闭时同一操作按非码元字母处置（顶屏高亮候选再出 `z`）。
#[test]
fn wildcard_letter_enters_buffer_outside_input_chars() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.codetable.input_chars = "a-y".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "a");
    assert!(
        !coord.debug_all_candidate_texts().is_empty(),
        "前置：a 在五笔 86 下应有真实候选"
    );
    press(&coord, "z");
    assert_eq!(coord.debug_input_buffer(), "az");

    let mut off = wubi(false, "z");
    off.schema.codetable.input_chars = "a-y".into();
    let coord = Coordinator::new_headless(off, Some(&data_dir()));
    press(&coord, "a");
    let act = press(&coord, "z");
    assert!(
        committed(&act).is_some_and(|t| t.ends_with('z')),
        "对照：关闭时 z 是非码元，实际 {act:?}"
    );
}

/// 26 码元方案配符号通配键：组码中 `?` 进缓冲，不走标点流水线。
/// 对照：关闭时 `?` 不进缓冲。
#[test]
fn symbol_wildcard_enters_buffer_instead_of_punct() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "?"), Some(&data_dir()));
    press(&coord, "a");
    let act = press_vk(&coord, VK_SLASH, true);
    assert_eq!(coord.debug_input_buffer(), "a?");
    assert!(committed(&act).is_none(), "通配键不应顶屏：{act:?}");

    let coord = Coordinator::new_headless(wubi(false, "?"), Some(&data_dir()));
    press(&coord, "a");
    press_vk(&coord, VK_SLASH, true);
    assert_ne!(coord.debug_input_buffer(), "a?", "对照：关闭时 ? 不进缓冲");
}

/// 空缓冲下未绑定任何功能的符号通配键作通配进缓冲（首位一行的「全无绑定」分支）。
/// 对照：关闭时同一键走标点流水线，缓冲保持为空。
#[test]
fn unbound_symbol_wildcard_enters_buffer_at_lead() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "?"), Some(&data_dir()));
    press_vk(&coord, VK_SLASH, true);
    assert_eq!(coord.debug_input_buffer(), "?");

    let coord = Coordinator::new_headless(wubi(false, "?"), Some(&data_dir()));
    press_vk(&coord, VK_SLASH, true);
    assert_eq!(coord.debug_input_buffer(), "", "对照：关闭时 ? 走标点");
}

/// 非首位通配键与次选键冲突 ⇒ 让位（`;` 照常选第 2 个候选）且体检报冲突。
/// 同一键只报一条（`;` 的会话动作就是次选，不再另报成「会话键」）。
/// 对照：通配键 `z` 在出厂键位下无冲突。
#[test]
fn mid_composition_conflict_yields_and_is_reported() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, ";"), Some(&data_dir()));
    assert_eq!(
        coord.wildcard_conflicts(),
        vec!["次选键"],
        "应只报一条次选键冲突"
    );
    press(&coord, "a");
    let second = coord
        .debug_all_candidate_texts()
        .get(1)
        .cloned()
        .expect("a 至少两个候选");
    let act = press_vk(&coord, VK_SEMICOLON, false);
    assert_eq!(committed(&act), Some(second.as_str()), "让位给次选键");

    let coord = Coordinator::new_headless(wubi(true, "z"), Some(&data_dir()));
    assert!(
        coord.wildcard_conflicts().is_empty(),
        "对照：z 在出厂键位下无冲突"
    );
}

/// 通配键是**显式配置**的方案码元时，体检报「方案码元」。
/// 对照：同一键不在显式码元集里（未配 `input_chars`）时不报。
#[test]
fn wildcard_that_is_a_configured_code_char_is_reported() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "/");
    cfg.schema.codetable.input_chars = "a-z/".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    assert!(
        coord
            .wildcard_conflicts()
            .iter()
            .any(|o| o.starts_with("方案码元")),
        "{:?}",
        coord.wildcard_conflicts()
    );

    let coord = Coordinator::new_headless(wubi(true, "/"), Some(&data_dir()));
    assert!(
        !coord
            .wildcard_conflicts()
            .iter()
            .any(|o| o.starts_with("方案码元")),
        "对照：未显式配置 input_chars 时不报码元：{:?}",
        coord.wildcard_conflicts()
    );
}

/// 首位 z 已绑 `z_key_action` ⇒ 让位进模式，不作通配。
#[test]
fn leading_z_yields_to_z_key_action() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.available.push("pinyin".into());
    cfg.input.temp_pinyin.enabled = true;
    cfg.schema.codetable.z_key_action = "temp_pinyin".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "z");
    assert!(coord.debug_in_temp_pinyin(), "首位 z 应让位给 z_key_action");
}

/// 首位 z 是 `zz*` 短语活码 ⇒ 让位，且整轮按字面：`zzbd` 照出短语「、」。
/// 对照：关闭时同一操作候选相同。
#[test]
fn leading_z_phrase_code_stays_literal() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    for on in [true, false] {
        let coord = Coordinator::new_headless(wubi(on, "z"), Some(&data_dir()));
        coord.debug_install_phrases(zz_phrases());
        press(&coord, "zzbd");
        assert_eq!(coord.debug_input_buffer(), "zzbd", "wildcard={on}");
        assert!(
            coord.debug_all_candidate_texts().iter().any(|t| t == "、"),
            "wildcard={on}：zzbd 应出短语「、」，实际 {:?}",
            coord.debug_all_candidate_texts()
        );
    }
}

/// overlay（临时拼音）激活时通配不适用：`z` 在临拼里是拼音字母。
/// 通配键取的是**活跃方案**（五笔）的，overlay 用的是别的方案，协调器必须自己挡住。
#[test]
fn overlay_mode_ignores_wildcard() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi(true, "z");
    cfg.schema.available.push("pinyin".into());
    cfg.input.temp_pinyin.enabled = true;
    cfg.schema.codetable.z_key_action = "temp_pinyin".into();
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    press(&coord, "z");
    assert!(coord.debug_in_temp_pinyin(), "前置：z 进临拼");
    press(&coord, "zhong");
    assert!(coord.debug_in_temp_pinyin(), "仍在临拼");
    assert!(
        coord.debug_all_candidate_texts().iter().any(|t| t == "中"),
        "临拼 zhong 应出「中」，实际 {:?}",
        coord.debug_all_candidate_texts()
    );
}

/// 首位符号通配键不得在透传集里：空缓冲时 C++ 对透传集里的键不吃，`/` 到不了 core，
/// 首位通配静默失效。`should_handle_key` 读的正是推给 DLL 的那两份集合（`key_gate.rs`）。
/// 对照：关闭通配时 `/` 照旧透传（出厂中文标点表对 `/` 无映射）。
#[test]
fn lead_symbol_wildcard_key_is_not_passed_through() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let coord = Coordinator::new_headless(wubi(true, "/"), Some(&data_dir()));
    assert!(
        coord.should_handle_key(&KeyProbe::new(VK_SLASH)),
        "开启通配：空缓冲时 `/` 必须送到服务端"
    );
    press_vk(&coord, VK_SLASH, false);
    assert_eq!(coord.debug_input_buffer(), "/", "首位无绑定 ⇒ 作通配进缓冲");

    let coord = Coordinator::new_headless(wubi(false, "/"), Some(&data_dir()));
    assert!(
        !coord.should_handle_key(&KeyProbe::new(VK_SLASH)),
        "对照：关闭时 `/` 透传给宿主"
    );
}
