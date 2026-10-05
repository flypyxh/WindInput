//! 快捷输入历史（成员 `quick_input.history`，t46）：在快捷输入里上屏过的字面文本，
//! 下次按前缀补全。设计见 `src/quick_history.rs` 模块文档。
//!
//! ⚠️ 依赖 `build_dev/data` 真实词库（快捷输入的成员方案要能加载）；缺失时**静默跳过**
//! （判据是耗时 0.00s）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};
use wind_store::Store;
use wind_store::completion::CompletionKind;

const VK_SEMICOLON: u32 = 0xBA;
const VK_SPACE: u32 = 0x20;
const VK_RETURN: u32 = 0x0D;
const VK_OEM_MINUS: u32 = 0xBD;
const VK_OEM_PLUS: u32 = 0xBB;
const VK_OEM_PERIOD: u32 = 0xBE;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_data() -> bool {
    let d = data_dir();
    ["wubi86", "pinyin", "english"]
        .iter()
        .all(|s| d.join(format!("schemas/{s}.schema.toml")).exists())
}

macro_rules! skip_without_data {
    () => {
        if !has_data() {
            eprintln!("跳过：缺 build_dev 词库");
            return;
        }
    };
}

fn key(k: u32, modifiers: u32) -> KeyEventData {
    KeyEventData {
        key_code: k,
        scan_code: 0,
        modifiers,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

fn store_at(tag: &str) -> Arc<Store> {
    let path = std::env::temp_dir().join(format!(
        "wind_quick_history_{}_{tag}.redb",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    Arc::new(Store::open(&path).unwrap())
}

/// 五笔主方案（`;` 空码进快捷输入）；`history` 控制是否把历史成员加进 `quick_mix`。
fn open(tag: &str, history: bool) -> (Arc<Coordinator>, Arc<Store>) {
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into(), "pinyin".into(), "english".into()];
    c.schema.active = "wubi86".into();
    c.input.default.chinese_mode = true;
    c.schema.english.frequency.enabled = true;
    if history {
        let members = &mut c.schema.mix_modes[0].members;
        let at = members
            .iter()
            .position(|m| m == wind_quick_input::MEMBER_REPEAT)
            .map_or(members.len(), |i| i + 1);
        members.insert(at, wind_quick_input::MEMBER_HISTORY.to_string());
    }
    let store = store_at(tag);
    let coord = Coordinator::new_headless_with_store(c, Some(&data_dir()), store.clone());
    (coord, store)
}

/// 逐键敲入：小写字母 / 数字原样，大写字母带 Shift，`_` `$` `{` `}` `.` `+` 按美式键位。
fn type_str(coord: &Coordinator, s: &str) {
    for ch in s.chars() {
        let (vk, m) = match ch {
            'a'..='z' | '0'..='9' => ((ch.to_ascii_uppercase() as u32) & 0xFF, 0),
            'A'..='Z' => (ch as u32, MOD_SHIFT),
            '_' => (VK_OEM_MINUS, MOD_SHIFT),
            '+' => (VK_OEM_PLUS, MOD_SHIFT),
            '.' => (VK_OEM_PERIOD, 0),
            '$' => (u32::from(b'4'), MOD_SHIFT),
            '{' => (0xDB, MOD_SHIFT),
            '}' => (0xDD, MOD_SHIFT),
            _ => panic!("type_str 不认识 {ch:?}"),
        };
        coord.handle_key_event(&key(vk, m));
    }
}

fn enter_quick(coord: &Coordinator) {
    coord.handle_key_event(&key(VK_SEMICOLON, 0));
    assert_eq!(
        coord.debug_active_mode(),
        Some("mix"),
        "前提：`;` 进快捷输入"
    );
}

fn committed(act: &KeyAction) -> Option<String> {
    match act {
        KeyAction::InsertText { text, .. } => Some(text.clone()),
        _ => None,
    }
}

fn count_of(store: &Store, text: &str) -> u32 {
    store
        .get_completion(CompletionKind::QuickHistory, text)
        .unwrap()
        .map_or(0, |r| r.count)
}

fn history_len(store: &Store) -> usize {
    store
        .list_completions(CompletionKind::QuickHistory, "", 0, 0)
        .unwrap()
        .1
}

/// 回车上屏的自由输入原文进历史，下次打前缀（小写、文本透镜）就能补出来。
#[test]
fn literal_commit_is_remembered_and_recalled() {
    skip_without_data!();
    let (coord, store) = open("recall", true);
    enter_quick(&coord);
    type_str(&coord, "reset_time");
    let act = coord.handle_key_event(&key(VK_RETURN, 0));
    assert_eq!(committed(&act).as_deref(), Some("reset_time"), "前提");
    assert_eq!(
        count_of(&store, "reset_time"),
        1,
        "回车上屏的原文应记进历史"
    );

    enter_quick(&coord);
    type_str(&coord, "res");
    let texts = coord.debug_page_texts();
    assert!(
        texts.iter().any(|t| t == "reset_time"),
        "打 res 应补出历史 reset_time，实际 {texts:?}"
    );
}

/// 带符号的缓冲（Free 透镜）：历史排首位，空格上屏它并次数 +1；回车恒上屏原文。
#[test]
fn free_lens_puts_history_first_and_enter_keeps_raw() {
    skip_without_data!();
    let (coord, store) = open("free", true);
    store
        .record_completion(CompletionKind::QuickHistory, "${reset_time}")
        .unwrap();

    enter_quick(&coord);
    type_str(&coord, "${re");
    let texts = coord.debug_page_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("${reset_time}"),
        "Free 透镜里历史应置首，实际 {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "${re"),
        "原文仍在候选里: {texts:?}"
    );
    let act = coord.handle_key_event(&key(VK_SPACE, 0));
    assert_eq!(committed(&act).as_deref(), Some("${reset_time}"));
    assert_eq!(count_of(&store, "${reset_time}"), 2, "选中历史应次数 +1");

    enter_quick(&coord);
    type_str(&coord, "${re");
    let act = coord.handle_key_event(&key(VK_RETURN, 0));
    assert_eq!(
        committed(&act).as_deref(),
        Some("${re"),
        "回车恒上屏原文，不被历史顶掉"
    );
}

/// 存储区分大小写、匹配不区分：两条都补得出来。
#[test]
fn match_ignores_case_but_storage_keeps_both() {
    skip_without_data!();
    let (coord, store) = open("case", true);
    for t in ["RESET_TIME", "reset_time"] {
        store
            .record_completion(CompletionKind::QuickHistory, t)
            .unwrap();
    }
    enter_quick(&coord);
    type_str(&coord, "res");
    let texts = coord.debug_page_texts();
    for t in ["RESET_TIME", "reset_time"] {
        assert!(texts.iter().any(|x| x == t), "缺 {t}: {texts:?}");
    }
}

/// 历史文本不经词库特殊语法展开：含 `{…}` 的串原样出，不被改写也不被丢掉。
#[test]
fn history_text_is_not_expanded_as_template() {
    skip_without_data!();
    let (coord, store) = open("tmpl", true);
    store
        .record_completion(CompletionKind::QuickHistory, "ab{clip()}")
        .unwrap();
    enter_quick(&coord);
    type_str(&coord, "ab");
    let texts = coord.debug_page_texts();
    assert!(
        texts.iter().any(|t| t == "ab{clip()}"),
        "历史文本应原样出现，实际 {texts:?}"
    );
}

/// 没把历史成员加进列表：什么都不记、什么都不补（出厂即如此）。
#[test]
fn nothing_happens_without_the_member() {
    skip_without_data!();
    let (coord, store) = open("off", false);
    enter_quick(&coord);
    type_str(&coord, "reset_time");
    coord.handle_key_event(&key(VK_RETURN, 0));
    assert_eq!(history_len(&store), 0);
}

/// 数字透镜的产出（计算结果、数字原文）不进历史。
#[test]
fn numeric_lens_is_not_remembered() {
    skip_without_data!();
    let (coord, store) = open("num", true);
    enter_quick(&coord);
    type_str(&coord, "1+2");
    coord.handle_key_event(&key(VK_SPACE, 0));
    enter_quick(&coord);
    type_str(&coord, "12.5");
    coord.handle_key_event(&key(VK_RETURN, 0));
    assert_eq!(history_len(&store), 0, "计算 / 数字不该进历史");
}

/// 英文词库词不进历史（英文词只记英文词频，t46 楼主说英文不需要）。
#[test]
fn english_dictionary_word_is_not_remembered() {
    skip_without_data!();
    let (coord, store) = open("en", true);
    enter_quick(&coord);
    type_str(&coord, "Hello");
    let texts = coord.debug_page_texts();
    assert_eq!(texts.first().map(String::as_str), Some("Hello"), "前提");
    coord.handle_key_event(&key(VK_SPACE, 0));
    assert_eq!(history_len(&store), 0, "词库词不该进历史");
}

/// 容量上限：超出后裁掉，刚写入的那条保住。
#[test]
fn history_is_pruned_to_the_cap() {
    skip_without_data!();
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into(), "pinyin".into(), "english".into()];
    c.schema.active = "wubi86".into();
    c.input.default.chinese_mode = true;
    c.schema.mix_modes[0]
        .members
        .push(wind_quick_input::MEMBER_HISTORY.to_string());
    c.schema.quick_input.history_max = 2;
    let store = store_at("cap");
    let coord = Coordinator::new_headless_with_store(c, Some(&data_dir()), store.clone());
    for w in ["aa_one", "bb_two", "cc_three"] {
        enter_quick(&coord);
        type_str(&coord, w);
        coord.handle_key_event(&key(VK_RETURN, 0));
    }
    assert_eq!(history_len(&store), 2);
    assert_eq!(count_of(&store, "cc_three"), 1, "刚写入的那条不能被裁掉");
}

/// 带配置钩子的 `open`：历史成员恒加在 `quick_input.repeat` 之后。
fn open_with(tag: &str, edit: impl FnOnce(&mut Config)) -> (Arc<Coordinator>, Arc<Store>) {
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into(), "pinyin".into(), "english".into()];
    c.schema.active = "wubi86".into();
    c.input.default.chinese_mode = true;
    let members = &mut c.schema.mix_modes[0].members;
    let at = members
        .iter()
        .position(|m| m == wind_quick_input::MEMBER_REPEAT)
        .map_or(members.len(), |i| i + 1);
    members.insert(at, wind_quick_input::MEMBER_HISTORY.to_string());
    edit(&mut c);
    let store = store_at(tag);
    let coord = Coordinator::new_headless_with_store(c, Some(&data_dir()), store.clone());
    (coord, store)
}

/// 文本透镜（纯小写字母）回车上屏的原文不记：它本身就是拼音 / 英文的合法编码，记进来
/// 下次打前缀时会顶掉拼音首选。
#[test]
fn text_lens_raw_commit_is_not_remembered() {
    skip_without_data!();
    let (coord, store) = open("textraw", true);
    enter_quick(&coord);
    type_str(&coord, "nihao");
    coord.handle_key_event(&key(VK_RETURN, 0));
    assert_eq!(history_len(&store), 0, "纯小写原文不该进历史");
}

/// 标点顶屏高亮的历史候选：次数 +1，且**不**往任何方案写词频（它没有编码）。
#[test]
fn punct_top_commit_of_history_writes_no_freq() {
    skip_without_data!();
    let (coord, store) = open_with("punct", |c| {
        c.schema.mix_modes[0].free_input = wind_config::config::FreeInputMode::Off;
        c.schema.codetable.frequency.enabled = true;
        c.schema.pinyin.frequency.enabled = true;
        c.schema.english.frequency.enabled = true;
    });
    store
        .record_completion(CompletionKind::QuickHistory, "reset_time")
        .unwrap();
    enter_quick(&coord);
    type_str(&coord, "res");
    assert_eq!(
        coord.debug_page_texts().first().map(String::as_str),
        Some("reset_time"),
        "前提：历史排在成员列表里它的位置（文本透镜下 repeat 之后即首位）"
    );
    let act = coord.handle_key_event(&key(0xBC, 0)); // `,`
    let out = committed(&act).unwrap_or_default();
    assert!(out.starts_with("reset_time"), "应顶屏历史，实际 {act:?}");
    assert_eq!(count_of(&store, "reset_time"), 2, "顶屏也算选中一次");
    for schema in ["wubi86", "pinyin", "english"] {
        let (rows, _) = store.list_freq_paged(schema, "", 0, 0).unwrap();
        assert!(
            !rows.iter().any(|(_, text, _)| text == "reset_time"),
            "历史候选不该记进 {schema} 的词频: {rows:?}"
        );
    }
}

/// 历史置首时大小写档位键仍然生效（改动前 Free 透镜首位恒是原文，档位键可用）。
#[test]
fn case_cycle_key_still_works_when_history_is_first() {
    skip_without_data!();
    let (coord, store) = open_with("cycle", |c| {
        c.input.english_case_cycle_key = "tab".into();
    });
    store
        .record_completion(CompletionKind::QuickHistory, "Help_me")
        .unwrap();
    enter_quick(&coord);
    type_str(&coord, "Hel");
    let before = coord.debug_page_texts();
    assert_eq!(before.first().map(String::as_str), Some("Help_me"), "前提");
    coord.handle_key_event(&key(0x09, 0)); // Tab
    let after = coord.debug_page_texts();
    assert_ne!(
        before, after,
        "档位键应改写英文段的大小写，实际未变: {after:?}"
    );
    assert_eq!(
        after.first().map(String::as_str),
        Some("Help_me"),
        "历史仍在首位"
    );
}
