//! 反查模式端到端（docs/design/codetable-reverse-mode.md §3）。真实数据：build_dev/data 的 wubi86 / wubi86_pinyin。
//! ⚠️ 数据缺失时整族静默跳过而计数照绿，判据是耗时（≥1s）与输出里有没有「跳过」。

use std::path::PathBuf;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

const VK_BACKSLASH: u32 = 0xDC;
const VK_ESCAPE: u32 = 0x1B;
const VK_NEXT: u32 = 0x22;
const VK_BACK: u32 = 0x08;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}
fn dict_ready() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}
/// 走生产出口 `handle_key_event_policed`（`wind-bridge` server 调的就是它）：放宽态的失效
/// （`expire_scope_override`）挂在这个出口上，裸 `handle_key_event` 绕过它，测不到。
fn key(c: &Coordinator, vk: u32, shift: bool) {
    c.handle_key_event_policed(&KeyEventData {
        key_code: vk,
        scan_code: 0,
        modifiers: if shift { MOD_SHIFT } else { 0 },
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    });
}
fn letters(c: &Coordinator, s: &str) {
    for ch in s.chars() {
        key(c, ch.to_ascii_uppercase() as u32, false);
    }
}

/// 五笔 86 + `\` 绑反查 + 模式开。通配主开关默认关（反查不看它）。
fn wubi_rev() -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.input.reverse.enabled = true;
    cfg.keys
        .key_actions
        .insert("backslash".into(), "reverse".into());
    cfg.input.scope_relax.page_end_key = false;
    cfg
}
fn with_filter(mut cfg: Config, mode: &str) -> Config {
    cfg.input.filter_mode = mode.into();
    cfg.input.rare_phrase = "keep".into();
    cfg
}
fn rev_triples(cfg: Config, code: &str) -> Vec<(String, String, String)> {
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, code);
    c.debug_candidate_triples()
}
fn page_until_more_than(c: &Coordinator, than: usize) -> usize {
    for _ in 0..400 {
        let n = c.debug_candidate_count();
        if n > than {
            return n;
        }
        let (cur, _, total) = c.debug_page_info();
        if cur + 1 >= total && !c.debug_has_more() {
            return n;
        }
        key(c, VK_NEXT, false);
    }
    c.debug_candidate_count()
}

/// spec §3.2 + codetable-wildcard §11：首批 100、`has_more`、翻到边界 ×2 扩充。缺 Task 18 时红在扩充。
#[test]
fn reverse_azzz_first_batch_100_and_expands() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(with_filter(wubi_rev(), "gb18030"), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "azzz");
    assert_eq!(c.debug_candidate_count(), 100);
    assert!(c.debug_has_more());
    let grown = page_until_more_than(&c, 100);
    assert!(grown > 100, "应扩充：{grown}");
    assert_eq!(c.debug_active_mode(), Some("reverse"), "翻页不退模式");
}

/// §11 契约 1：智能档把反查结果整份当一组，与常用字档相同。
#[test]
fn reverse_smart_matches_general() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let all = rev_triples(with_filter(wubi_rev(), "gb18030"), "hanz");
    let general = rev_triples(with_filter(wubi_rev(), "general"), "hanz");
    let smart = rev_triples(with_filter(wubi_rev(), "smart"), "hanz");
    assert!(all.len() > general.len(), "前置：han? 有生僻字可滤");
    assert_eq!(smart, general);
}

/// 智能档 + 末页放宽：反查模式里 `input_buffer` 恒空，放宽须看 `special_buffer`。
fn smart_relax() -> Config {
    let mut cfg = with_filter(wubi_rev(), "smart");
    cfg.input.scope_relax.page_end_key = true;
    cfg
}

/// 在反查模式里一路向后翻，直到放宽生效或确实翻不动。返回是否放宽了。
fn page_until_relaxed(c: &Coordinator) -> bool {
    for _ in 0..50 {
        key(c, VK_NEXT, false);
        if c.debug_scope_relaxed() {
            return true;
        }
    }
    false
}

/// 翻到末页再按一次 ⇒ 被智能档滤掉的反查生僻字追加在末尾，原有顺序不动、模式不退；
/// 放宽态在本次按键结束后仍在（`expire_scope_override` 看的是 `special_buffer`，不是恒空的
/// `input_buffer`——看错了会在同一键里静默撤销）。缺 Task 18 时红在「应放宽」。
#[test]
fn reverse_page_end_relax_appends_filtered() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let pair = |t: &(String, String, String)| (t.0.clone(), t.1.clone());
    let all: Vec<(String, String)> = rev_triples(with_filter(wubi_rev(), "gb18030"), "hanz")
        .iter()
        .map(pair)
        .collect();
    let c = Coordinator::new_headless(smart_relax(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "hanz");
    let before: Vec<(String, String)> = c.debug_candidate_triples().iter().map(pair).collect();
    assert!(all.len() > before.len(), "前置：hanz 有被滤的生僻字");
    assert!(!c.debug_has_more(), "前置：一批取完");
    assert!(page_until_relaxed(&c), "末页再按应放宽");
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    let after: Vec<(String, String)> = c.debug_candidate_triples().iter().map(pair).collect();
    assert_eq!(&after[..before.len()], &before[..], "原有候选顺序不动");
    let mut appended = after[before.len()..].to_vec();
    appended.sort();
    let mut expected: Vec<(String, String)> =
        all.into_iter().filter(|p| !before.contains(p)).collect();
    expected.sort();
    assert_eq!(appended, expected, "被滤的反查生僻字全部追加在末尾");

    // 放宽期间改码（退格）：缓冲非空 ⇒ 放宽态保持，同主路「找生僻字常要改几次编码」。
    key(&c, VK_BACK, false);
    assert!(c.debug_scope_relaxed(), "缓冲非空时放宽保持");
    assert_eq!(c.debug_preedit(), "\\han");
}

/// 放宽后 Esc：放宽 / 翻页位全部复位，退出后主路径的下一次组码不处于放宽态。
#[test]
fn reverse_exit_after_relax_leaves_no_residue() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(smart_relax(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "hanz");
    assert!(page_until_relaxed(&c), "前置：已放宽");
    key(&c, VK_ESCAPE, false);
    assert_eq!(c.debug_active_mode(), None);
    assert!(!c.debug_scope_relaxed());
    assert!(!c.debug_has_more());
    letters(&c, "a");
    assert_eq!(c.debug_input_buffer(), "a", "主路径照常组码");
    assert!(!c.debug_scope_relaxed(), "主路径组码不继承放宽");
}

/// 扩充后 Esc：`has_more` 复位；退出后主路径翻页不会拿残留位去扩充。
#[test]
fn reverse_exit_after_expand_leaves_no_residue() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(with_filter(wubi_rev(), "gb18030"), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    letters(&c, "azzz");
    assert!(page_until_more_than(&c, 100) > 100, "前置：已扩充");
    key(&c, VK_ESCAPE, false);
    assert_eq!(c.debug_active_mode(), None);
    assert!(!c.debug_has_more());
    assert_eq!(c.debug_candidate_count(), 0);
    // 页码本身随 special 族一样留在退出前的值（候选已空、无可见影响）；主路径下一次装填
    // 经 `reset_candidate_view` 归零，这里锁的是「主路径从第 1 页开始」。
    letters(&c, "a");
    assert_eq!(c.debug_input_buffer(), "a");
    assert_eq!(c.debug_page_info().0, 0, "主路径从第 1 页开始");
}

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

#[test]
fn reverse_symbol_trigger_enters_only_when_enabled() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let c = Coordinator::new_headless(wubi_rev(), Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    let mut off = wubi_rev();
    off.input.reverse.enabled = false;
    let c = Coordinator::new_headless(off, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), None, "总开关关：绑了键也不进");
}

/// ★ Review Focus 3：字母触发键是本方案首码（活码前缀）⇒ 让位作正常码；符号键不让位；
/// 冲突体检认得反查键。
#[test]
fn reverse_letter_trigger_yields_to_live_code_prefix() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.keys.key_actions.insert("a".into(), "reverse".into());
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    letters(&c, "a");
    assert_eq!(c.debug_active_mode(), None, "a 是五笔活码，让位");
    assert_eq!(c.debug_input_buffer(), "a");
    key(&c, VK_ESCAPE, false);
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), Some("reverse"), "符号键不让位");

    let mut conflict = wubi_rev();
    conflict.schema.codetable.input_chars = "a-y\\".into();
    let c = Coordinator::new_headless(conflict, Some(&data_dir()));
    let rep = c.code_char_conflicts();
    assert!(
        rep.iter()
            .any(|(ch, owners)| *ch == '\\' && owners.contains(&"反查模式触发键")),
        "{rep:?}"
    );
}

/// spec §3.1：非码表 / 混输方案让位。
#[test]
fn reverse_yields_in_pinyin_schema() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.available = vec!["pinyin".into(), "wubi86".into()];
    cfg.schema.active = "pinyin".into();
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    key(&c, VK_BACKSLASH, false);
    assert_eq!(c.debug_active_mode(), None);
}

/// GH#146 同构：`z_key_action = "reverse"` 在出厂 `zz*` 短语下经 z 夺取进入。
#[test]
fn z_key_action_reverse_enters_via_z_fallback() {
    if !dict_ready() {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    let mut cfg = wubi_rev();
    cfg.schema.codetable.z_key_action = "reverse".into();
    let c = Coordinator::new_headless(cfg, Some(&data_dir()));
    c.debug_install_phrases(zz_phrases());
    letters(&c, "z");
    assert_eq!(c.debug_active_mode(), None, "首键 z 让位给 zz* 活码");
    letters(&c, "uia");
    assert_eq!(c.debug_active_mode(), Some("reverse"));
    assert_eq!(c.debug_preedit(), "zuia");
    assert!(
        !c.debug_all_candidate_texts().is_empty(),
        "uia 的等长 + 前缀结果"
    );
}
