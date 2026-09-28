//! 「应用独立配置 → 状态提示位置」菜单与气泡拖动落盘分流（C2-33 / GH#148）：
//! 有规则写规则（compat.toml），无规则写全局（config.toml），读取侧规则压过全局。
//!
//! 走真实的菜单分派、焦点事件与 UI 反向事件，所以要写盘：用户目录经 `WIND_DATADIR_CONF`
//! 重定向到临时目录，一个进程只能重定向一次，故单开一个测试二进制（同
//! `password_force_english_per_app.rs`）。焦点进程名经 `FocusData::bundle_id` 注入。
//!
//! ⚠️ 本用例**不依赖 `build_dev/data`**：夹具全部自造，不会静默跳过。

use std::path::Path;
use std::sync::mpsc::Receiver;
use wind_bridge::handler::{FocusData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ui_types::{MenuCmd, StatusTipAnchor, StatusTipPlacement, UiCommand, UiEvent};

const PID: u32 = 4343;
const PROC: &str = "illustrator.exe";

/// 用户层 compat.toml 里该进程规则的某个键；无文件/无规则/未写均为 `None`。
fn rule_key(user: &Path, key: &str) -> Option<toml::Value> {
    let text = std::fs::read_to_string(user.join("compat.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).unwrap();
    v.get("apps")?
        .as_array()?
        .iter()
        .find(|r| r.get("process").and_then(|p| p.as_str()) == Some(PROC))?
        .get(key)
        .cloned()
}

/// 用户层 config.toml 里 `[ui.status]` 的某个键。
fn global_key(user: &Path, key: &str) -> Option<toml::Value> {
    let text = std::fs::read_to_string(user.join("config.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).unwrap();
    v.get("ui")?.get("status")?.get(key).cloned()
}

fn focus(coord: &Coordinator) {
    coord.handle_focus_gained(&FocusData {
        x: 100,
        y: 100,
        height: 20,
        composition_start_x: 0,
        composition_start_y: 0,
        client_token: (PID as u64) << 32 | 1,
        input_scope_mask: 0,
        disabled: false,
        reason: 2,
        caret_source: 0,
        bundle_id: PROC.to_string(),
        window_class: String::new(),
    });
}

/// 触发一次状态气泡（切标点必弹），取最后一条 ShowStatusTip 的定位。
fn tip_placement(coord: &Coordinator, rx: &Receiver<UiCommand>) -> Option<StatusTipPlacement> {
    let _: Vec<_> = rx.try_iter().collect();
    coord.debug_run_menu_cmd(MenuCmd::TogglePunct);
    rx.try_iter()
        .filter_map(|c| match c {
            UiCommand::ShowStatusTip { placement, .. } => Some(placement),
            _ => None,
        })
        .last()
}

fn int(v: Option<toml::Value>) -> Option<i64> {
    v.and_then(|v| v.as_integer())
}

fn string(v: Option<toml::Value>) -> Option<String> {
    v.and_then(|v| v.as_str().map(str::to_string))
}

#[test]
fn per_app_status_position_menu_and_drag_routing() {
    // ⚠️ 目录名带 pid：多 worktree / 多会话并行跑测试时固定名会互删夹具。
    let tmp = std::env::temp_dir().join(format!("wind_coord_status_pos-{}", std::process::id()));
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

    let make = || {
        Coordinator::new_headless_with_ui_at(
            Config::load(Config::data_dir().as_deref()).unwrap(),
            None,
            Some(&user),
        )
    };
    let (coord, rx) = make();
    focus(&coord);
    assert!(
        matches!(
            tip_placement(&coord, &rx),
            Some(StatusTipPlacement::Caret { .. })
        ),
        "前置条件：出厂跟随光标"
    );

    // ── 无规则：气泡菜单「固定位置」+ 拖动 → 写全局，compat.toml 不动 ──
    coord.debug_run_menu_cmd(MenuCmd::StatusTogglePinned);
    assert_eq!(
        string(global_key(&user, "position_mode")).as_deref(),
        Some("fixed")
    );
    coord.inject_ui_event(UiEvent::StatusTipMoved { x: 300, y: 400 });
    assert_eq!(int(global_key(&user, "custom_x")), Some(300));
    assert_eq!(int(global_key(&user, "custom_y")), Some(400));
    assert_eq!(rule_key(&user, "status_position_mode"), None);
    assert_eq!(
        tip_placement(&coord, &rx),
        Some(StatusTipPlacement::Fixed { x: 300, y: 400 })
    );

    // ── 本应用「固定（取当前位置）」：写规则并请 UI 报位置；回报落到规则，全局不动 ──
    let _: Vec<_> = rx.try_iter().collect();
    coord.debug_run_menu_cmd(MenuCmd::StatusPositionRule(2));
    assert_eq!(
        string(rule_key(&user, "status_position_mode")).as_deref(),
        Some("fixed")
    );
    assert!(
        rx.try_iter()
            .any(|c| matches!(c, UiCommand::ReportStatusTipPos)),
        "固定应取气泡当前位置：要请 UI 回报"
    );
    coord.inject_ui_event(UiEvent::StatusTipMoved { x: 700, y: 800 });
    assert_eq!(int(rule_key(&user, "status_x")), Some(700));
    assert_eq!(int(rule_key(&user, "status_y")), Some(800));
    assert_eq!(
        int(global_key(&user, "custom_x")),
        Some(300),
        "有规则时拖动不得改全局坐标"
    );
    assert_eq!(
        tip_placement(&coord, &rx),
        Some(StatusTipPlacement::Fixed { x: 700, y: 800 }),
        "规则压过全局，且坐标取规则自己的"
    );

    // ── 本应用锚点（StatusAnchor::ALL[0] = 屏幕中央）：规则改成锚点、坐标清零 ──
    coord.debug_run_menu_cmd(MenuCmd::StatusPositionRule(3));
    assert_eq!(
        string(rule_key(&user, "status_position_mode")).as_deref(),
        Some("screen_center")
    );
    assert_eq!(rule_key(&user, "status_x"), None, "非 fixed 不留坐标");
    assert_eq!(
        tip_placement(&coord, &rx),
        Some(StatusTipPlacement::Anchor(StatusTipAnchor::ScreenCenter))
    );
    // 锚点模式下拖动是临时的：不落盘。
    coord.inject_ui_event(UiEvent::StatusTipMoved { x: 1, y: 2 });
    assert_eq!(rule_key(&user, "status_x"), None);

    // ── 坐标不可用时：本应用「不显示」、再换成窗口左下角（ALL[6]）──
    coord.debug_run_menu_cmd(MenuCmd::StatusFallbackRule(2));
    assert_eq!(
        string(rule_key(&user, "status_fallback_position")).as_deref(),
        Some("hide")
    );
    coord.debug_run_menu_cmd(MenuCmd::StatusFallbackRule(9));
    assert_eq!(
        string(rule_key(&user, "status_fallback_position")).as_deref(),
        Some("window_bottom_left")
    );

    // ── 气泡菜单「恢复默认位置」：有规则 ⇒ 改规则为跟随光标，全局仍是 fixed ──
    coord.debug_run_menu_cmd(MenuCmd::StatusResetPosition);
    assert_eq!(
        string(rule_key(&user, "status_position_mode")).as_deref(),
        Some("follow_caret")
    );
    assert_eq!(
        string(global_key(&user, "position_mode")).as_deref(),
        Some("fixed")
    );

    // ── 「重启」：规则从用户层 compat.toml 读回 ──
    drop(coord);
    let (coord, rx) = make();
    focus(&coord);
    assert!(
        matches!(
            tip_placement(&coord, &rx),
            Some(StatusTipPlacement::Caret { .. })
        ),
        "重启后规则（跟随光标）仍压过全局 fixed"
    );

    // ── 两项都「跟随全局」：规则整条消失，回到全局 fixed ──
    coord.debug_run_menu_cmd(MenuCmd::StatusPositionRule(0));
    coord.debug_run_menu_cmd(MenuCmd::StatusFallbackRule(0));
    assert_eq!(rule_key(&user, "process"), None, "空壳规则应被剔除");
    assert_eq!(
        tip_placement(&coord, &rx),
        Some(StatusTipPlacement::Fixed { x: 300, y: 400 })
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
