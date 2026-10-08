//! 编码手填成简拼的用户词不得在简拼输入下被强制置顶（GH#177）。
//!
//! 现场：拼音方案下用户词库加 `rq` → `$Y年$M月$D日`（权重 0），打 `rq` 时它恒居首选，
//! 而系统简拼词「日期」「人群」被压在后面。与 `$` 无关：引擎 step 6 按「编码 == 输入」
//! 查用户词，字面码 `rq` 被当成**全拼精确命中**放进完整匹配层；而系统里 `rq` 的候选全在
//! 简拼层（`is_abbrev`，层级排序里最沉的一层）。层级是硬闸门，权重 0 也照样压过一切。
//!
//! 修复后：输入本身被引擎当作简拼（`AbbrevMatcher::is_abbreviation`），且用户词编码就是
//! 这串击键时，该用户词归入简拼层，与系统简拼候选按权重 / 词频同层竞争。
//!
//! 反向对照：全拼码用户词（`riqi`）不受影响，仍在完整匹配层按自身权重排。
//!
//! 位次在 **Coordinator 层**取（协调器会按消费长度等再排一次，引擎位次 ≠ 界面位次）。
//!
//! ⚠️ 词典缺失时整族静默跳过（判据是**耗时 0.00s**，不是通过条数）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_pinyin() -> bool {
    data_dir()
        .join("schemas/pinyin/cn_dicts/base.dict.yaml")
        .exists()
}

fn key_event(key_code: u32) -> KeyEventData {
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

fn type_code(coord: &Coordinator, s: &str) {
    for c in s.chars() {
        coord.handle_key_event(&key_event((c.to_ascii_uppercase() as u32) & 0xFF));
    }
}

fn config(schema: &str) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec![schema.into()];
    cfg.schema.active = schema.into();
    cfg.input.default.chinese_mode = true;
    cfg
}

fn coordinator(tag: &str, words: &[(&str, &str, i32)]) -> Arc<Coordinator> {
    coordinator_for("pinyin", tag, words)
}

/// 用户词一律写在 `pinyin` 名下：双拼与全拼共用拼音用户词库（存的是全拼码）。
fn coordinator_for(schema: &str, tag: &str, words: &[(&str, &str, i32)]) -> Arc<Coordinator> {
    let store_path = std::env::temp_dir().join(format!(
        "wind_e2e_uw_abbrev_code_{tag}_{}.redb",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&store_path);
    let store = Arc::new(wind_store::Store::open(&store_path).unwrap());
    for (code, text, weight) in words {
        store
            .add_user_word("pinyin", code, text, *weight, 0)
            .expect("写入用户词");
    }
    Coordinator::new_headless_with_store(config(schema), Some(&data_dir()), store)
}

/// 简拼码用户词（文本不在系统库）不再夺首选；系统简拼高频词「日期」排在它前面。
#[test]
fn abbrev_coded_user_word_does_not_take_top() {
    if !has_pinyin() {
        return;
    }
    let coord = coordinator("plain", &[("rq", "测试简码词", 0)]);
    type_code(&coord, "rq");
    let texts = coord.debug_page_texts();
    let riqi = texts.iter().position(|t| t == "日期");
    let uw = texts.iter().position(|t| t == "测试简码词");
    assert!(
        riqi.is_some(),
        "`rq` 首页应有系统简拼词「日期」，实际: {texts:?}"
    );
    assert_ne!(uw, Some(0), "简拼码用户词不应置顶，实际: {texts:?}");
    // 用户词权重 0，可能被挤出首页（None）——那也满足「不在日期之前」。
    if let Some(u) = uw {
        assert!(
            riqi.unwrap() < u,
            "「日期」应排在用户词之前，实际: {texts:?}"
        );
    }
}

/// 现场原样：`$` 模板文本。展开后的文本含「年」「月」「日」，不在首位。
#[test]
fn abbrev_coded_template_user_word_does_not_take_top() {
    if !has_pinyin() {
        return;
    }
    let coord = coordinator("template", &[("rq", "$Y年$M月$D日", 0)]);
    type_code(&coord, "rq");
    let texts = coord.debug_page_texts();
    let is_date = |t: &String| t.contains('年') && t.contains('月') && t.ends_with('日');
    assert!(
        !texts.first().is_some_and(is_date),
        "`$Y年$M月$D日` 简拼码用户词不应置顶，实际: {texts:?}"
    );
    let riqi = texts.iter().position(|t| t == "日期");
    assert!(riqi.is_some(), "`rq` 首页应有「日期」，实际: {texts:?}");
    if let Some(u) = texts.iter().position(is_date) {
        assert!(
            riqi.unwrap() < u,
            "「日期」应排在模板词之前，实际: {texts:?}"
        );
    }
}

/// 简拼层内仍按权重竞争：高权重的简拼码用户词可以排到系统简拼词前面
/// （降层不是废掉，它与「日期」是同层对手）。
#[test]
fn abbrev_coded_user_word_competes_by_weight_within_abbrev_layer() {
    if !has_pinyin() {
        return;
    }
    let coord = coordinator("heavy", &[("rq", "测试简码词", 2_000_000_000)]);
    type_code(&coord, "rq");
    let texts = coord.debug_page_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("测试简码词"),
        "高权重简拼码用户词应在简拼层内胜出，实际: {texts:?}"
    );
}

/// 反向对照：全拼码用户词行为不变——高权重时仍居首选（完整匹配层，不被降到简拼层）。
#[test]
fn full_pinyin_coded_user_word_unchanged() {
    if !has_pinyin() {
        return;
    }
    let coord = coordinator("full", &[("riqi", "测试全拼词", 2_000_000_000)]);
    type_code(&coord, "riqi");
    let texts = coord.debug_page_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("测试全拼词"),
        "全拼码高权重用户词应居首选，实际: {texts:?}"
    );
}

/// 击键是**未打完的音节**（`sh` 是 shi/shang… 的前缀）时不降层。
///
/// `is_abbreviation` 对 `sh`/`zh` 也判真，但它们的主解释是全拼：有近 300 条前缀补全
/// （是 / 上…），简拼层沉在前缀层、子短语层之后。实测（权重 0 的同码用户词）：
/// - GH#177 修复前：全表第 8 位（第 2 页首），在完整匹配层；
/// - 若无条件降层：**整条消失**（沉到候选上限之外被截断）——用户配的词打不出来；
/// - 限定降层后：回到第 8 位。
///
/// 断言取层级不变量而非写死位次：用户词在、且排在同串的系统**简拼**词之前（`s|h` 的
/// 「时候」、`z|h` 的「之后」——两音节词在单音节输入下只能经简拼召回，位于全表 260 位
/// 往后）——即仍在完整匹配层。
#[test]
fn syllable_prefix_coded_user_word_keeps_position() {
    if !has_pinyin() {
        return;
    }
    for (code, abbrev_word) in [("sh", "时候"), ("zh", "之后")] {
        let coord = coordinator(&format!("prefix_{code}"), &[(code, "测试声母词", 0)]);
        type_code(&coord, code);
        let all: Vec<String> = coord
            .debug_candidate_triples()
            .into_iter()
            .map(|t| t.0)
            .collect();
        let uw = all.iter().position(|t| t == "测试声母词");
        let sys = all.iter().position(|t| t == abbrev_word);
        let head = &all[..all.len().min(20)];
        assert!(
            uw.is_some(),
            "`{code}` 是音节前缀，同码用户词必须仍可达，前 20: {head:?}"
        );
        assert!(
            sys.is_some(),
            "`{code}` 下应有系统简拼词「{abbrev_word}」，前 20: {head:?}"
        );
        assert!(
            uw < sys,
            "`{code}` 同码用户词应留在完整匹配层、排在简拼词「{abbrev_word}」之前，\
             用户词位次 {uw:?}，简拼词位次 {sys:?}，前 20: {head:?}"
        );
    }
}

/// 双拼：比对基准必须是**原始击键**（`abbr_query`），不是转换后的全拼（`query`）。
///
/// 小鹤 `hd` = h + ai → `hai`。击键 `hd` 按全拼读是纯简拼（`is_abbreviation` 真、也不是
/// 任何音节前缀），而用户词码 `hai` 恰是转换结果——它是**全拼精确命中**，必须留在完整
/// 匹配层按权重居首。若误比 `query`，`hai == query` 成立、它会被降进简拼层，
/// 沉到「还」「海」这些精确单字之后。
#[test]
fn shuangpin_full_pinyin_coded_user_word_not_demoted() {
    if !has_pinyin() {
        return;
    }
    let coord = coordinator_for(
        "shuangpin",
        "sp_hai",
        &[("hai", "测试双拼词", 2_000_000_000)],
    );
    type_code(&coord, "hd");
    let texts = coord.debug_page_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("测试双拼词"),
        "双拼 `hd`(hai) 下码为 `hai` 的高权重用户词应居首，实际: {texts:?}"
    );
}
