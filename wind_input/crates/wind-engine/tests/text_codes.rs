//! 按词查编码统一入口的接线验证：系统层（反查索引）+ 用户层（用户词），见
//! `docs/design/text-code-lookup.md`。夹具自造一个小码表方案，不依赖 build_dev/data。

use std::path::PathBuf;
use std::sync::Arc;
use wind_config::Config;
use wind_engine::EngineManager;

/// 造一个码表方案，工=a/aaaa、攻=atyy、公=wcu。`id` 由调用方指定：反查索引磁盘缓存路径按
/// 方案 id 落在进程外共享目录（`Config::cache_dir`），并发跑测试时同名会撞同一份缓存文件 /
/// build_guard 标记，故每个测试必须用自己独有的 id。
pub fn write_wbx(schemas: &std::path::Path, id: &str) {
    std::fs::create_dir_all(schemas.join(id)).unwrap();
    std::fs::write(
        schemas.join(format!("{id}.schema.toml")),
        format!(
            "[schema]\nid = \"{id}\"\nname = \"测五\"\n[engine]\ntype = \"codetable\"\n\
             [engine.codetable]\nmax_code_length = 4\n\
             [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/{id}.dict.yaml\"\n\
             type = \"rime_codetable\"\ndefault = true\n"
        ),
    )
    .unwrap();
    std::fs::write(
        schemas.join(format!("{id}/{id}.dict.yaml")),
        "---\nname: wbx\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n\
         a\t工\t100\naaaa\t工\t90\natyy\t攻\t80\nwcu\t公\t80\n",
    )
    .unwrap();
}

/// 本进程内唯一的方案 id：反查索引的磁盘缓存按方案 id 落在进程外的共享目录
/// （`Config::cache_dir`），固定 id 会让并行 / 先后几次测试互相读到对方的缓存。
fn uid(tag: &str) -> String {
    format!("zz_tc_{tag}_{}", std::process::id())
}

/// 测试结束（含 panic）时清掉夹具目录与该方案在共享缓存根下的产物（`<cache>/<id>/`）。
/// 不清的话每跑一次测试就在真实缓存目录里多留一个 `zz_*` 目录，无上限增长。
pub struct Cleanup {
    id: String,
    dir: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(cache) = Config::cache_dir() {
            let _ = std::fs::remove_dir_all(cache.join(&self.id));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 夹具：码表方案 `id`（主码表）+ 一个 `pinyin` 方案，后者的 `[engine.aux_code].files`
/// 由 `pinyin_aux_files`（TOML 数组字面量）给定，空串 = 不写 aux_code 段。
fn setup_with(
    id: &str,
    pinyin_aux_files: &str,
) -> (EngineManager, Arc<wind_store::Store>, Cleanup) {
    let dir = std::env::temp_dir().join(format!("wind_text_codes_it_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    let guard = Cleanup {
        id: id.to_string(),
        dir: dir.clone(),
    };
    write_wbx(&dir.join("schemas"), id);
    let aux = if pinyin_aux_files.is_empty() {
        String::new()
    } else {
        format!("[engine.aux_code]\nfiles = {pinyin_aux_files}\nenabled = true\n")
    };
    std::fs::write(
        dir.join("schemas/pinyin.schema.toml"),
        format!("[schema]\nid = \"pinyin\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n{aux}"),
    )
    .unwrap();
    let sp = dir.join("store.redb");
    let store = Arc::new(wind_store::Store::open(&sp).unwrap());
    let mut cfg = Config::default();
    cfg.schema.available = vec![id.into()];
    cfg.schema.active = id.into();
    let m =
        EngineManager::with_store_override(&cfg, Some(&dir), Some(store.clone()), None::<PathBuf>);
    (m, store, guard)
}

fn setup(id: &str) -> (EngineManager, Arc<wind_store::Store>, Cleanup) {
    setup_with(id, "")
}

#[test]
fn display_is_none_until_system_layer_ready() {
    let id = uid("ready");
    let (m, _, _g) = setup(&id);
    assert_eq!(m.word_codes_display(&id, "工"), None, "没就绪 ≠ 查不到");
    assert!(m.prewarm_text_codes(&id));
    assert_eq!(m.word_codes_display(&id, "工").as_deref(), Some("a/aaaa"));
    assert_eq!(m.word_codes_display(&id, "无").as_deref(), Some(""));
}

/// ★ 用户在该方案里加的编码进入显示；而加词去重用的 `word_codes_in` 仍只看系统层。
#[test]
fn user_codes_join_display_but_not_word_codes_in() {
    let id = uid("user");
    let (m, store, _g) = setup(&id);
    store.add_user_word(&id, "gggg", "工", 0, 0).unwrap();
    store.add_user_word(&id, "zzzz", "嗨", 0, 0).unwrap();
    m.prewarm_text_codes(&id);
    assert_eq!(
        m.word_codes_display(&id, "工").as_deref(),
        Some("a/aaaa/gggg")
    );
    assert_eq!(
        m.word_codes_display(&id, "嗨").as_deref(),
        Some("zzzz"),
        "系统没有的字也能查到"
    );
    assert_eq!(
        m.word_codes_in(&id, "工").as_deref(),
        Some("a/aaaa"),
        "去重口径不变"
    );
}

#[test]
fn view_any_code_walks_both_layers() {
    let id = uid("any");
    let (m, store, _g) = setup(&id);
    store.add_user_word(&id, "zzzz", "工", 0, 0).unwrap();
    m.prewarm_text_codes(&id);
    let v = m.text_codes(&id);
    assert!(v.any_code("工", &mut |c| c.starts_with('a')), "系统层");
    assert!(v.any_code("工", &mut |c| c.starts_with('z')), "用户层");
    assert!(!v.any_code("工", &mut |c| c.starts_with('q')));
}

/// ★ 主码表提示取「最长码里的系统码」：同长时系统码胜出，用户码只在系统没有时才出。
/// 曾经直接取合并后列表的最后一个——用户给「工」加个同长的 gggg，提示就从 aaaa 变成 gggg。
#[test]
fn codetable_hint_prefers_system_code_at_same_length() {
    let id = uid("hint");
    let (m, store, _g) = setup(&id);
    store.add_user_word(&id, "gggg", "工", 0, 0).unwrap();
    store.add_user_word(&id, "zzzz", "嗨", 0, 0).unwrap();
    m.prewarm_text_codes(&id);
    assert_eq!(m.codetable_reverse_hint("工").as_deref(), Some("aaaa"));
    assert_eq!(
        m.codetable_reverse_hint("嗨").as_deref(),
        Some("zzzz"),
        "只有用户码时取用户码"
    );
}

/// ★ 辅助码「在用」的方案来源含**临拼目标方案**引用的：五笔方案下从临拼进辅助码时，
/// 码表配在拼音方案里，只看活跃方案的话它既不预热也不被反查索引护栏钉住。
#[test]
fn aux_code_schemas_in_use_includes_temp_pinyin_target() {
    let id = uid("inuse");
    let (m, _, _g) = setup_with(&id, &format!(r#"["schema:{id}"]"#));
    assert!(m.ensure_schema(&id), "活跃码表方案可加载");
    assert!(
        m.aux_code_settings().sources.is_empty(),
        "活跃的码表方案自己没配辅助码"
    );
    assert_eq!(m.aux_code_schemas_in_use(), vec![id.clone()]);
}
