//! 切回本输入法（IME_ACTIVATED）必须清掉上一个实例留下的密码态（t197）。
//!
//! 切换输入法不会重发 OnSetFocus：TSF 新实例激活后若服务端仍沿用旧的 `password_suppress`，
//! 图标会一直显「英」。DLL 随后会补发 input_state_report 覆盖，本用例只钉服务端这一半。
//!
//! 夹具全部自造（headless、无词库），不会静默跳过。

use wind_bridge::handler::{FocusData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;

/// IS_DEFAULT + IS_PASSWORD：t197 楼主 HUD 上的掩码。
const MASK_T197: u64 = 0x8000_0001;
const PID: u32 = 4243;

fn token() -> u64 {
    (PID as u64) << 32 | 1
}

fn focus(coord: &Coordinator, mask: u64) {
    coord.handle_focus_gained(&FocusData {
        x: 100,
        y: 100,
        height: 20,
        composition_start_x: 0,
        composition_start_y: 0,
        client_token: token(),
        input_scope_mask: mask,
        disabled: false,
        reason: 2,
        caret_source: 0,
        bundle_id: String::new(),
        window_class: String::new(),
        window_title: String::new(),
    });
}

#[test]
fn ime_activated_drops_stale_password_suppress() {
    let coord = Coordinator::new_headless(Config::default(), None);
    let (enabled, _) = coord.debug_password_suppress();
    assert!(enabled, "前置条件：出厂应开启密码框强制英文");

    focus(&coord, MASK_T197);
    assert_eq!(
        coord.debug_password_suppress(),
        (true, true),
        "前置条件：密码 scope 应触发抑制"
    );

    coord.handle_ime_activated(token());
    assert_eq!(
        coord.debug_password_suppress(),
        (true, false),
        "切回输入法后旧的密码抑制必须清零，等 DLL 补发的读数再定"
    );
}
