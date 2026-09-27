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

fn setup(id: &str) -> (EngineManager, Arc<wind_store::Store>) {
    let dir = std::env::temp_dir().join(format!("wind_text_codes_it_{id}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_wbx(&dir.join("schemas"), id);
    let sp = dir.join("store.redb");
    let store = Arc::new(wind_store::Store::open(&sp).unwrap());
    let mut cfg = Config::default();
    cfg.schema.available = vec![id.into()];
    cfg.schema.active = id.into();
    let m =
        EngineManager::with_store_override(&cfg, Some(&dir), Some(store.clone()), None::<PathBuf>);
    (m, store)
}

#[test]
fn display_is_none_until_system_layer_ready() {
    let (m, _) = setup("zz_tc_ready");
    assert_eq!(
        m.word_codes_display("zz_tc_ready", "工"),
        None,
        "没就绪 ≠ 查不到"
    );
    assert!(m.prewarm_text_codes("zz_tc_ready"));
    assert_eq!(
        m.word_codes_display("zz_tc_ready", "工").as_deref(),
        Some("a/aaaa")
    );
    assert_eq!(
        m.word_codes_display("zz_tc_ready", "无").as_deref(),
        Some("")
    );
}

/// ★ 用户在该方案里加的编码进入显示；而加词去重用的 `word_codes_in` 仍只看系统层。
#[test]
fn user_codes_join_display_but_not_word_codes_in() {
    let (m, store) = setup("zz_tc_user");
    store
        .add_user_word("zz_tc_user", "gggg", "工", 0, 0)
        .unwrap();
    store
        .add_user_word("zz_tc_user", "zzzz", "嗨", 0, 0)
        .unwrap();
    m.prewarm_text_codes("zz_tc_user");
    assert_eq!(
        m.word_codes_display("zz_tc_user", "工").as_deref(),
        Some("a/aaaa/gggg")
    );
    assert_eq!(
        m.word_codes_display("zz_tc_user", "嗨").as_deref(),
        Some("zzzz"),
        "系统没有的字也能查到"
    );
    assert_eq!(
        m.word_codes_in("zz_tc_user", "工").as_deref(),
        Some("a/aaaa"),
        "去重口径不变"
    );
}

#[test]
fn view_any_code_walks_both_layers() {
    let (m, store) = setup("zz_tc_any");
    store
        .add_user_word("zz_tc_any", "zzzz", "工", 0, 0)
        .unwrap();
    m.prewarm_text_codes("zz_tc_any");
    let v = m.text_codes("zz_tc_any");
    assert!(v.any_code("工", &mut |c| c.starts_with('a')), "系统层");
    assert!(v.any_code("工", &mut |c| c.starts_with('z')), "用户层");
    assert!(!v.any_code("工", &mut |c| c.starts_with('q')));
}
