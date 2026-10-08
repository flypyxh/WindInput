//! 「应用独立配置 → 密码框强制英文」菜单（A2-37 / t197）：写盘到用户层 compat.toml、
//! 重载规则表、当前焦点立即解除抑制、规则压过全局。
//!
//! 走真实的菜单分派与焦点事件，所以要写盘：用户目录经 `WIND_DATADIR_CONF` 重定向到临时目录，
//! 一个进程只能重定向一次，故单开一个测试二进制（同 `password_force_english_persist.rs`）。
//!
//! 焦点进程名经 `FocusData::bundle_id` 注入（macOS 通路；Linux 下 `process_name` 恒空，
//! 这是 host 上唯一够得着 `pid_names` 的公开入口）。
//!
//! ⚠️ 本用例**不依赖 `build_dev/data`**：夹具全部自造，不会静默跳过。

use std::path::Path;
use wind_bridge::handler::{FocusData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ui_types::MenuCmd;

/// IS_DEFAULT + IS_PASSWORD：t197 楼主那个输入框报出的 InputScope 掩码。
const MASK_T197: u64 = 0x8000_0001;
const PID: u32 = 4242;
const PROC: &str = "misreport.exe";

/// 用户层 compat.toml 里该进程的 `password_force_english`；无文件/无规则/未写均为 `None`。
fn user_rule(user: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(user.join("compat.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).unwrap();
    v.get("apps")?
        .as_array()?
        .iter()
        .find(|r| r.get("process").and_then(|p| p.as_str()) == Some(PROC))?
        .get("password_force_english")?
        .as_bool()
}

fn focus(coord: &Coordinator) {
    coord.handle_focus_gained(&FocusData {
        x: 100,
        y: 100,
        height: 20,
        composition_start_x: 0,
        composition_start_y: 0,
        client_token: (PID as u64) << 32 | 1,
        input_scope_mask: MASK_T197,
        disabled: false,
        reason: 2,
        caret_source: 0,
        bundle_id: PROC.to_string(),
        window_class: String::new(),
        window_title: String::new(),
    });
}

#[test]
fn per_app_password_force_english_menu_writes_and_applies() {
    // ⚠️ 目录名带 pid：多 worktree / 多会话并行跑测试时固定名会互删夹具。
    let tmp = std::env::temp_dir().join(format!("wind_coord_pw_perapp-{}", std::process::id()));
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

    // 带用户目录构造：compat 规则的菜单写回要 `compat_dirs.1`，`new_headless` 给的是 None。
    let make = || {
        Coordinator::new_headless_with_ui_at(
            Config::load(Config::data_dir().as_deref()).unwrap(),
            None,
            Some(&user),
        )
        .0
    };
    let coord = make();
    focus(&coord);
    assert_eq!(
        coord.debug_password_suppress(),
        (true, true),
        "前置条件：出厂全局开、无规则 → 密码位强制英文"
    );

    // ── 本应用「关」：写盘 false，当前抑制立即解除；全局开关不动 ──
    coord.debug_run_menu_cmd(MenuCmd::PasswordForceEnglishRule(2));
    assert_eq!(user_rule(&user), Some(false), "应写入用户层 compat.toml");
    assert_eq!(
        coord.debug_password_suppress(),
        (true, false),
        "全局仍开，但本应用的抑制应立即解除"
    );
    // 下一次诊断上报也不再置位（规则已进运行时表，不是只改了当前态）。
    coord.handle_input_state_report(PID, false, 2, MASK_T197);
    assert_eq!(coord.debug_password_suppress(), (true, false));

    // ── 「跟随全局」：规则从文件里清掉，下一次上报回到全局（开）──
    coord.debug_run_menu_cmd(MenuCmd::PasswordForceEnglishRule(0));
    assert_eq!(user_rule(&user), None, "跟随全局 = 清掉该字段");
    coord.handle_input_state_report(PID, false, 2, MASK_T197);
    assert_eq!(coord.debug_password_suppress(), (true, true));

    // ── 全局关 + 本应用「开」：规则压过全局 ──
    coord.debug_run_menu_cmd(MenuCmd::TogglePasswordSuppress);
    assert_eq!(
        coord.debug_password_suppress(),
        (false, false),
        "全局关掉，无规则 → 解除"
    );
    coord.debug_run_menu_cmd(MenuCmd::PasswordForceEnglishRule(1));
    assert_eq!(user_rule(&user), Some(true));
    coord.handle_input_state_report(PID, false, 2, MASK_T197);
    assert_eq!(
        coord.debug_password_suppress(),
        (false, true),
        "本应用规则开应压过全局关"
    );

    // ── 「重启」：规则从用户层 compat.toml 读回 ──
    drop(coord);
    let coord = make();
    focus(&coord);
    assert_eq!(
        coord.debug_password_suppress(),
        (false, true),
        "重启后规则仍生效"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
