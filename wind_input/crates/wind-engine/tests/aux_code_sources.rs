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

/// `direct`（直接辅助码）的生效判据：`enabled` 是总闸、只认双拼、方案 tri-state 覆盖全局。
/// 见 `docs/design/aux-code-direct.md` §7。
#[test]
fn direct_requires_enabled_and_shuangpin() {
    let dir = std::env::temp_dir().join(format!("wind_aux_direct_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
    std::fs::write(schemas.join("aux_code/t.txt"), "李=mz\n").unwrap();
    let write = |id: &str, scheme: &str, aux: &str| {
        std::fs::write(
            schemas.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"{id}\"\n[engine]\ntype = \"pinyin\"\n\
                 [engine.pinyin]\nscheme = \"{scheme}\"\n\
                 [engine.aux_code]\nfiles = [\"aux_code/t.txt\"]\n{aux}"
            ),
        )
        .unwrap();
    };
    write("sp_on", "shuangpin", "enabled = true\ndirect = true\n");
    write(
        "sp_off_total",
        "shuangpin",
        "enabled = false\ndirect = true\n",
    );
    write("sp_follow", "shuangpin", "enabled = true\n");
    write("sp_veto", "shuangpin", "enabled = true\ndirect = false\n");
    write("qp_on", "", "enabled = true\ndirect = true\n");
    let manager = |global_direct: bool| {
        let mut cfg = Config::default();
        cfg.schema.available = vec!["sp_on".into()];
        cfg.schema.active = "sp_on".into();
        cfg.schema.pinyin.aux_code.direct = global_direct;
        EngineManager::with_store_override(&cfg, Some(&dir), None, None::<PathBuf>)
    };
    let m = manager(false);
    assert!(
        m.aux_code_settings_of("sp_on").direct,
        "双拼 + enabled + direct"
    );
    assert!(
        !m.aux_code_settings_of("sp_off_total").direct,
        "enabled 是总闸：关了 direct 也不生效"
    );
    assert!(
        !m.aux_code_settings_of("sp_follow").direct,
        "方案不写 = 跟随全局（出厂关）"
    );
    assert!(
        !m.aux_code_settings_of("qp_on").direct,
        "全拼写了 direct 也不生效"
    );
    assert!(
        m.aux_code_settings_of("qp_on").enabled,
        "全拼的 direct 不生效不连累 enabled"
    );
    let m = manager(true);
    assert!(m.aux_code_settings_of("sp_follow").direct, "跟随全局开");
    assert!(
        !m.aux_code_settings_of("sp_veto").direct,
        "方案显式关压过全局开"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
