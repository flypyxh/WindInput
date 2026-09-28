//! 「密码框强制英文」开关持久化（t197 / A2-37）：菜单切换写盘、配置启动即生效、热重载保持。
//!
//! 此前开关只在内存里，构造处硬编码 `true`——宿主把普通输入框误报成密码框的用户，每次重启
//! 服务都得再关一次。本用例走真实的菜单分派与 `reload_user_config`，所以要写盘：用户目录经
//! `WIND_DATADIR_CONF` 重定向到临时目录，一个进程只能重定向一次，故单开一个测试二进制。
//!
//! ⚠️ 本用例**不依赖 `build_dev/data`**：夹具全部自造，不会静默跳过。

use std::path::Path;
use wind_bridge::handler::MessageHandler;
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ui_types::MenuCmd;

/// IS_DEFAULT + IS_PASSWORD：t197 楼主那个输入框报出的 InputScope 掩码。
const MASK_T197: u64 = 0x8000_0001;

/// 用户层 config.toml 里 `input.password_force_english` 的写盘值；未写则 `None`。
fn user_value(user: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(user.join("config.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).unwrap();
    v.get("input")?.get("password_force_english")?.as_bool()
}

#[test]
fn password_force_english_survives_restart_and_reload() {
    // ⚠️ 目录名带 pid：多 worktree / 多会话并行跑测试时固定名会互删夹具。
    let tmp = std::env::temp_dir().join(format!("wind_coord_pw_persist-{}", std::process::id()));
    let root = tmp.join("install");
    let user = tmp.join("UserData");
    let conf = tmp.join("datadir.conf");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(&conf, user.to_string_lossy().as_bytes()).unwrap();

    // SAFETY: 本文件仅此一个测试，env 在任何 OnceLock 初始化之前设置，无并发读者。
    unsafe {
        std::env::set_var("WIND_DATADIR_CONF", &conf);
        std::env::set_var("WIND_INSTALL_ROOT", &root);
    }
    assert_eq!(
        Config::user_config_dir(),
        Some(user.clone()),
        "前置条件：用户目录须已重定向，否则本测试会读写真实用户配置"
    );

    // ── 出厂：开，密码位强制英文 ──
    let coord =
        Coordinator::new_headless(Config::load(Config::data_dir().as_deref()).unwrap(), None);
    coord.handle_input_state_report(1, false, 2, MASK_T197);
    assert_eq!(
        coord.debug_password_suppress(),
        (true, true),
        "出厂应开且生效"
    );

    // ── 菜单关掉：写盘 false，内存同步，当前抑制立即解除 ──
    coord.debug_run_menu_cmd(MenuCmd::TogglePasswordSuppress);
    assert_eq!(user_value(&user), Some(false), "菜单关掉应写盘");
    assert_eq!(
        coord.debug_password_suppress(),
        (false, false),
        "关掉后内存开关同步、已生效的抑制立即解除"
    );

    // ── 热重载：保持关闭（重载回灌的是配置值，不是构造时的出厂值）──
    coord.reload_user_config();
    coord.handle_input_state_report(1, false, 2, MASK_T197);
    assert_eq!(
        coord.debug_password_suppress(),
        (false, false),
        "热重载后应保持关闭"
    );

    // ── 「重启」：按同一份用户配置新建协调器，开关仍是关的 ──
    drop(coord);
    let coord =
        Coordinator::new_headless(Config::load(Config::data_dir().as_deref()).unwrap(), None);
    coord.handle_input_state_report(1, false, 2, MASK_T197);
    assert_eq!(
        coord.debug_password_suppress(),
        (false, false),
        "重启后应保持关闭（t197 的原始症状是这里自动勾回）"
    );

    // ── 菜单再打开：写盘 true，密码位重新生效 ──
    coord.debug_run_menu_cmd(MenuCmd::TogglePasswordSuppress);
    assert_eq!(user_value(&user), Some(true), "菜单打开应写盘");
    coord.handle_input_state_report(1, false, 2, MASK_T197);
    assert_eq!(coord.debug_password_suppress(), (true, true));

    // ── 设置端/手改文件后热重载：跟随配置关掉，并解除当前抑制 ──
    Config::set_user_value(
        &["input", "password_force_english"],
        toml::Value::Boolean(false),
    )
    .unwrap();
    coord.reload_user_config();
    assert_eq!(
        coord.debug_password_suppress(),
        (false, false),
        "外部改配置后热重载应跟随，且立即解除抑制"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
