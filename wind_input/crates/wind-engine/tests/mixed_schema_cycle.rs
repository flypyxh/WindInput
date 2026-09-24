//! 混输方案的 primary/secondary 引用成环时，构建必须**失败返回**，不能无限递归栈溢出。
//!
//! `read_schema` 合并 override 走无白名单的 `merge_toml`，一个畸形 override（或导入的方案包）
//! 就能让混输引用自己、或两个方案互指。`build_engine` 的混输分支是递归构建子引擎的，没有环检测
//! 时这是一条无底递归：Rust 栈溢出直接 abort **整个进程**（比 A1-9 的死锁更糟——服务直接没了）。
//!
//! 判据：成环方案 `ensure_schema` 返回 false（该方案不可用），同进程里未成环的方案照常可用。
//! 回归时的表现是整个测试二进制 abort（`has overflowed its stack`），不是断言红。
//!
//! ⚠️ 词库缺失时静默跳过（同 `english_engine_is_shared.rs` 的约定）。

use std::path::{Path, PathBuf};
use wind_config::Config;
use wind_engine::EngineManager;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_data() -> bool {
    let s = data_dir().join("schemas");
    ["wubi86_pinyin", "wubi86", "pinyin"]
        .iter()
        .all(|id| s.join(format!("{id}.schema.toml")).exists())
}

/// 每个用例一个独立 override 目录（带进程号 + 用例名：并发会话同跑时不互删）。
fn override_dir(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let ov = std::env::temp_dir().join(format!("wind_mixcycle_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ov);
    std::fs::create_dir_all(&ov).unwrap();
    for (id, body) in files {
        std::fs::write(ov.join(format!("{id}.toml")), body).unwrap();
    }
    ov
}

fn manager(ov: &Path) -> EngineManager {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86_pinyin".into(), "wubi86".into(), "pinyin".into()];
    cfg.schema.active = "wubi86_pinyin".into();
    EngineManager::with_store_override(&cfg, Some(&data_dir()), None, Some(ov.to_path_buf()))
}

#[test]
fn mixed_primary_referencing_itself_is_rejected_not_overflowed() {
    if !has_data() {
        eprintln!("跳过：缺少方案文件");
        return;
    }
    let ov = override_dir(
        "self",
        &[(
            "wubi86_pinyin",
            "[engine.mixed]\nprimary_schema = \"wubi86_pinyin\"\n",
        )],
    );
    let mgr = manager(&ov);
    assert!(
        !mgr.ensure_schema("wubi86_pinyin"),
        "自引用的混输方案应不可用"
    );
    // 不连累别的方案。
    assert!(mgr.ensure_schema("pinyin"));
}

#[test]
fn mixed_secondary_referencing_itself_is_rejected() {
    if !has_data() {
        eprintln!("跳过：缺少方案文件");
        return;
    }
    let ov = override_dir(
        "secondary",
        &[(
            "wubi86_pinyin",
            "[engine.mixed]\nsecondary_schema = \"wubi86_pinyin\"\n",
        )],
    );
    let mgr = manager(&ov);
    assert!(!mgr.ensure_schema("wubi86_pinyin"));
}

/// 间接环：wubi86_pinyin → wubi86（被 override 成混输）→ wubi86_pinyin。
#[test]
fn two_mixed_schemas_referencing_each_other_are_rejected() {
    if !has_data() {
        eprintln!("跳过：缺少方案文件");
        return;
    }
    let ov = override_dir(
        "mutual",
        &[(
            "wubi86",
            "[engine]\ntype = \"mixed\"\n\n[engine.mixed]\nprimary_schema = \"wubi86_pinyin\"\n",
        )],
    );
    let mgr = manager(&ov);
    assert!(!mgr.ensure_schema("wubi86_pinyin"));
    assert!(!mgr.ensure_schema("wubi86"));
    assert!(mgr.ensure_schema("pinyin"));
}

/// 反向护栏：出厂的 wubi86_pinyin（无环）照常可建——环检测不能误伤正常混输。
#[test]
fn acyclic_mixed_schema_still_builds() {
    if !has_data() {
        eprintln!("跳过：缺少方案文件");
        return;
    }
    let ov = override_dir("ok", &[]);
    let mgr = manager(&ov);
    assert!(mgr.ensure_schema("wubi86_pinyin"));
}
