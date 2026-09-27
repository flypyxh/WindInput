//! `[engine.aux_code].files` 的两种条目：文件路径与 `schema:<id>` 方案引用。

use std::path::PathBuf;
use wind_config::Config;
use wind_engine::{AuxSource, EngineManager};

fn setup(tag: &str, files: &str) -> (EngineManager, PathBuf) {
    let dir = std::env::temp_dir().join(format!("wind_aux_src_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
    std::fs::write(schemas.join("aux_code/t.txt"), "李=mz\n").unwrap();
    std::fs::write(
        schemas.join("pinyin.schema.toml"),
        format!(
            "[schema]\nid = \"pinyin\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n\
             [engine.aux_code]\nfiles = {files}\nenabled = true\n"
        ),
    )
    .unwrap();
    std::fs::write(
        schemas.join("wbx.schema.toml"),
        "[schema]\nid = \"wbx\"\nname = \"测五\"\n[engine]\ntype = \"codetable\"\n",
    )
    .unwrap();
    std::fs::write(
        schemas.join("en.schema.toml"),
        "[schema]\nid = \"en\"\nname = \"英\"\n[engine]\ntype = \"english\"\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".into()];
    cfg.schema.active = "pinyin".into();
    (
        EngineManager::with_store_override(&cfg, Some(&dir), None, None::<PathBuf>),
        schemas,
    )
}

/// 顺序即优先级；坏条目（不存在 / 非码表 / 自己 / 空 id）逐条跳过，其余照常。
#[test]
fn schema_entries_are_resolved_in_order_and_bad_ones_skipped() {
    let (m, schemas) = setup(
        "mix",
        r#"["schema:wbx", "schema:nope", "schema:en", "schema:pinyin", "schema: ", "aux_code/t.txt"]"#,
    );
    let s = m.aux_code_settings();
    assert_eq!(
        s.sources,
        vec![
            AuxSource::Schema("wbx".into()),
            AuxSource::File(schemas.join("aux_code/t.txt")),
        ]
    );
    assert_eq!(s.schema_sources().collect::<Vec<_>>(), vec!["wbx"]);
}

#[test]
fn plain_file_entries_behave_as_before() {
    let (m, schemas) = setup("file", r#"["aux_code/t.txt", "aux_code/missing.txt"]"#);
    assert_eq!(
        m.aux_code_settings().sources,
        vec![AuxSource::File(schemas.join("aux_code/t.txt"))]
    );
}
