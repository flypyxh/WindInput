//! 英文混入策略（`complete_min_length` / `max_count` / 上屏否决口径）在**真实词库**下的验证：
//! 码表方案（`[schema.codetable.english_merge]`，可经方案级覆盖）与混输（`[schema.mix]`
//! 的 `english_*`）同一套口径。
//!
//! 位次（`position`）在协调器定位，守门测试在 `wind-coordinator` 的 `english_placement_tests`。

use std::path::PathBuf;
use wind_candidate::{Candidate, CandidateSource};
use wind_config::Config;
use wind_engine::EngineManager;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn ready(schema: &str) -> bool {
    let dir = data_dir();
    let ok = dir.join(format!("schemas/{schema}.schema.toml")).exists()
        && dir.join("schemas/english/en.dict.yaml").exists();
    if !ok {
        eprintln!("跳过：{schema} 方案或英文词库不存在");
    }
    ok
}

fn make_config(schema: &str) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec![schema.to_string()];
    cfg.schema.active = schema.to_string();
    cfg
}

fn english(r: &wind_engine::ConvertResult) -> Vec<&Candidate> {
    r.candidates
        .iter()
        .filter(|c| c.source == CandidateSource::English)
        .collect()
}

fn has_chinese(r: &wind_engine::ConvertResult) -> bool {
    r.candidates
        .iter()
        .any(|c| c.source != CandidateSource::English)
}

fn wubi(f: impl FnOnce(&mut Config)) -> EngineManager {
    let mut cfg = make_config("wubi86");
    cfg.schema.codetable.english_merge.enable = true;
    f(&mut cfg);
    EngineManager::new(&cfg, Some(&data_dir()))
}

/// ① 出厂口径：中文侧有候选时 3 / 4 码只出完整单词，`gith` 不补 github（现状不变）。
#[test]
fn codetable_default_only_exact_words() {
    if !ready("wubi86") {
        return;
    }
    let mgr = wubi(|_| {});
    let r = mgr.convert("gith", 50);
    assert!(has_chinese(&r), "前提：五笔 gith 有中文候选");
    assert!(
        english(&r).is_empty(),
        "出厂不补前缀，实际 {:?}",
        english(&r)
    );
    let r3 = mgr.convert("git", 50);
    assert!(has_chinese(&r3), "前提：五笔 git 有中文候选");
    assert!(
        english(&r3).iter().all(|c| c.code == "git"),
        "3 码只收完整单词：{:?}",
        english(&r3)
    );
}

/// ② `complete_min_length = 4`：`gith` 中文侧有候选也补出 github；3 码不补。
/// ③ `max_count` 限条数。
#[test]
fn codetable_complete_min_length_and_max_count() {
    if !ready("wubi86") {
        return;
    }
    let mgr = wubi(|c| c.schema.codetable.english_merge.complete_min_length = 4);
    let r = mgr.convert("gith", 50);
    assert!(has_chinese(&r), "前提：五笔 gith 有中文候选");
    let en = english(&r);
    assert!(
        en.iter().any(|c| c.text.eq_ignore_ascii_case("github")),
        "gith 应补出 github，实际 {en:?}"
    );
    assert!(en.len() <= 2, "出厂上限 2 条，实际 {}", en.len());
    assert!(
        english(&mgr.convert("git", 50))
            .iter()
            .all(|c| c.code == "git"),
        "3 码不到补全门槛"
    );

    let one = wubi(|c| {
        c.schema.codetable.english_merge.complete_min_length = 4;
        c.schema.codetable.english_merge.max_count = 1;
    });
    assert_eq!(english(&one.convert("gith", 50)).len(), 1, "max_count = 1");
}

/// ⑤ 漏洞：开满码自动上屏时，英文词的 4 码前缀（此刻无精确命中、列表里没有英文）须被
/// 英文否决。`abse` 是五笔唯一全码「荫棚」，也是 absent / absence 的前缀。
#[test]
fn codetable_block_commit_vetoes_prefix_at_full_code() {
    if !ready("wubi86") {
        return;
    }
    let build = |block: bool| {
        wubi(|c| {
            c.schema.codetable.auto_commit_at_full = true;
            c.schema.codetable.english_merge.block_commit = block;
        })
    };
    assert!(
        build(false).convert("abse", 50).should_commit,
        "前提：关掉否决时 abse 满码上屏，否则测不到否决"
    );
    let guarded = build(true).convert("abse", 50);
    assert!(
        english(&guarded).is_empty(),
        "前提：出厂口径下 abse 不显示英文（无精确命中、不补前缀）"
    );
    assert!(
        !guarded.should_commit,
        "abse 是 absent 的前缀，满码上屏须被否决，否则 absent 敲不完"
    );
    // 否决须叠「英文确有词条」：英文里没有以它为前缀的满码（`aqqa`）照常上屏。
    assert!(
        build(false).convert("aqqa", 50).should_commit,
        "前提：aqqa 满码上屏，否则这条反向断言测不到东西"
    );
    assert!(build(true).convert("aqqa", 50).should_commit);
}

/// 成词级（`block_commit_on_word`）：前缀级关掉时，英文词的前缀不拦满码，整串是英文单词
/// 才拦。`abse` 是 absent 的前缀；`addr` 本身是英文单词、也是五笔唯一全码「莽原」。
#[test]
fn codetable_word_level_veto() {
    if !ready("wubi86") {
        return;
    }
    let build = |word: bool| {
        wubi(|c| {
            c.schema.codetable.auto_commit_at_full = true;
            c.schema.codetable.english_merge.block_commit = false;
            c.schema.codetable.english_merge.block_commit_on_word = word;
        })
    };
    assert!(
        build(false).convert("addr", 50).should_commit,
        "前提：两级都关时 addr 满码上屏"
    );
    assert!(
        !build(true).convert("addr", 50).should_commit,
        "addr 是英文单词，成词级应否决"
    );
    assert!(
        build(true).convert("abse", 50).should_commit,
        "abse 只是英文词前缀，成词级不该否决"
    );
}

/// ⑦ 方案级 `[engine.codetable.english_merge]` 逐字段覆盖全局。
#[test]
fn codetable_schema_override_applies() {
    if !ready("wubi86") {
        return;
    }
    let mut cfg = make_config("wubi86");
    cfg.schema.codetable.english_merge.enable = true;
    let ov = std::env::temp_dir().join("wind_english_merge_policy_ov");
    let _ = std::fs::remove_dir_all(&ov);
    std::fs::create_dir_all(&ov).unwrap();
    std::fs::write(
        ov.join("wubi86.toml"),
        "[engine.codetable.english_merge]\ncomplete_min_length = 4\n",
    )
    .unwrap();
    let mgr = EngineManager::with_store_override(&cfg, Some(&data_dir()), None, Some(ov));
    assert!(
        english(&mgr.convert("gith", 50))
            .iter()
            .any(|c| c.text.eq_ignore_ascii_case("github")),
        "方案里写的 complete_min_length 应生效"
    );
}

fn mixed(f: impl FnOnce(&mut Config)) -> EngineManager {
    let mut cfg = make_config("wubi86_pinyin");
    cfg.schema.mix.enable_english = true;
    f(&mut cfg);
    EngineManager::new(&cfg, Some(&data_dir()))
}

/// ⑥ 混输同口径：默认 `gith` 不补；`english_complete_min_length = 4` 时补；上限生效。
#[test]
fn mixed_complete_min_length_and_max_count() {
    if !ready("wubi86_pinyin") {
        return;
    }
    let r = mixed(|_| {}).convert("gith", 50);
    assert!(has_chinese(&r), "前提：混输 gith 有中文候选");
    assert!(english(&r).is_empty(), "默认不补：{:?}", english(&r));

    let mgr = mixed(|c| c.schema.mix.english_complete_min_length = 4);
    let en_r = mgr.convert("gith", 50);
    let en = english(&en_r);
    assert!(
        en.iter().any(|c| c.text.eq_ignore_ascii_case("github")),
        "gith 应补出 github，实际 {en:?}"
    );
    assert!(en.len() <= 2);

    let one = mixed(|c| {
        c.schema.mix.english_complete_min_length = 4;
        c.schema.mix.english_max_count = 1;
    });
    assert_eq!(english(&one.convert("gith", 50)).len(), 1);
}

/// ⑥ 混输的英文守护同样按前缀否决：`abse`（absent 的前缀）满码不得把中文顶上屏。
#[test]
fn mixed_block_on_english_vetoes_prefix_at_full_code() {
    if !ready("wubi86_pinyin") {
        return;
    }
    let build = |block: bool| {
        mixed(|c| {
            c.schema.codetable.auto_commit_at_full = true;
            c.schema.mix.auto_commit_block_on_english = block;
        })
    };
    assert!(
        build(false).convert("abse", 50).should_commit,
        "前提：关掉英文守护时混输 abse 满码上屏，否则测不到否决"
    );
    assert!(
        !build(true).convert("abse", 50).should_commit,
        "混输开英文守护时 abse 满码上屏须被否决"
    );
}
