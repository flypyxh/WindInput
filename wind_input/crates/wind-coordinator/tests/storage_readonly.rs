//! 设计 §7.1 降级保护：meta.toml 版本高于本版本 ⇒ 只读模式，store 不打开，RPC 能查到原因。
use wind_config::Config;
use wind_coordinator::Coordinator;

#[test]
fn newer_meta_version_puts_coordinator_in_readonly_mode() {
    // ⚠️ 目录名带 pid：多会话并行跑测试时固定名会互删夹具。
    let root = std::env::temp_dir().join(format!("wind_ro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data/schemas")).unwrap();
    std::fs::create_dir_all(root.join("userdata")).unwrap();
    std::fs::create_dir_all(root.join("localdata")).unwrap();
    std::fs::write(root.join(wind_config::variant::PORTABLE_MARKER_NAME), "").unwrap();
    // SAFETY: 本文件仅此一个测试，env 在任何 OnceLock 初始化之前设置，无并发读者。
    unsafe {
        std::env::set_var("WIND_INSTALL_ROOT", &root);
    }
    std::fs::write(
        root.join("userdata/meta.toml"),
        "format_version = 99\ndir_id = \"x\"\n",
    )
    .unwrap();
    let user = Config::user_config_dir().expect("用户目录须已重定向");
    assert_eq!(user, root.join("userdata"), "前置条件：用户目录须已重定向");

    let data = Config::data_dir();
    let (c, _rx) = Coordinator::new_headless_with_ui_at(
        Config::load(data.as_deref()).unwrap(),
        data.as_deref(),
        Some(&user),
    );
    let reason = c.storage_readonly_reason().expect("高版本 meta ⇒ 只读");
    assert!(reason.contains("99"), "原因里要带上对方的版本号: {reason}");
    // RPC `system.storageStatus` 的形状在 wind-webdata 的测试里验（trait 在那边才可调）
    assert!(
        !user.join("userdata.redb").exists(),
        "只读模式不得创建 redb"
    );
    assert_eq!(
        std::fs::read_to_string(user.join("meta.toml")).unwrap(),
        "format_version = 99\ndir_id = \"x\"\n",
        "只读模式一字不改 meta.toml"
    );
    let _ = std::fs::remove_dir_all(&root);
}
