//! 按应用方案 `@remember` 的记忆表经 `state.toml` 跨重启恢复（C0-7 / C3-3，GH#80）。
//!
//! 单开测试二进制：要把本机状态目录（`state.toml` 所在）重定向到临时目录，而那是进程级的
//! 一次性设置（便携标记 + `WIND_INSTALL_ROOT`，同 `app_schema_per_app.rs`）。
//!
//! 「重启」= 再构造一个协调器：它在构造时从 `state.toml` 读回记忆表。第一个协调器不会被
//! drop（后台线程持有 `Arc`），故落盘靠 `StateWriter` 的防抖到期，这里轮询等文件出现。
//!
//! ⚠️ 本用例**不依赖 `build_dev/data`**：方案全部自造。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wind_bridge::handler::{FocusData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ui_types::MenuCmd;

fn setup(root: &Path) {
    let schemas = root.join("data/schemas");
    for id in ["za", "zb"] {
        std::fs::create_dir_all(schemas.join(id)).unwrap();
        std::fs::write(
            schemas.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"{id}\"\n\
                 [engine]\ntype = \"codetable\"\n\
                 [engine.codetable]\nmax_code_length = 4\n\
                 [[dictionaries]]\nid = \"main\"\npath = \"{id}/{id}.dict.yaml\"\ndefault = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            schemas.join(format!("{id}/{id}.dict.yaml")),
            format!("---\nname: {id}\nversion: \"1\"\n...\n甲\ta\n"),
        )
        .unwrap();
    }
    std::fs::write(root.join(wind_config::variant::PORTABLE_MARKER_NAME), "").unwrap();
    let user = root.join("userdata");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("config.toml"),
        "[schema]\nactive = \"za\"\navailable = [\"za\", \"zb\"]\n",
    )
    .unwrap();
    std::fs::write(
        user.join("compat.toml"),
        "[[apps]]\nprocess = \"chat.exe\"\nschema = \"@remember\"\n",
    )
    .unwrap();
}

fn make(user: &Path) -> Arc<Coordinator> {
    let data = Config::data_dir();
    Coordinator::new_headless_with_ui_at(
        Config::load(data.as_deref()).unwrap(),
        data.as_deref(),
        Some(user),
    )
    .0
}

fn focus(c: &Coordinator, pid: u32, proc: &str) {
    c.handle_focus_gained(&FocusData {
        x: 100,
        y: 100,
        height: 20,
        composition_start_x: 0,
        composition_start_y: 0,
        client_token: (pid as u64) << 32 | 1,
        input_scope_mask: 0,
        disabled: false,
        reason: 2,
        caret_source: 0,
        bundle_id: proc.to_string(),
        window_class: String::new(),
    });
    c.debug_wait_app_schema_load();
}

#[test]
fn remembered_schema_survives_restart() {
    // ⚠️ 目录名带 pid：多 worktree / 多会话并行跑测试时固定名会互删夹具。
    let root = std::env::temp_dir().join(format!("wind_app_schema_state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    setup(&root);
    // SAFETY: 本文件仅此一个测试，env 在任何 OnceLock 初始化之前设置，无并发读者。
    unsafe {
        std::env::set_var("WIND_INSTALL_ROOT", &root);
    }
    let user = root.join("userdata");
    let state = root.join("localdata/state.toml");
    assert_eq!(
        Config::user_config_dir(),
        Some(user.clone()),
        "前置条件：用户目录须已重定向"
    );
    assert_eq!(
        Config::state_dir(),
        Some(root.join("localdata")),
        "前置条件：本机状态目录须已重定向，否则会改掉开发者本机的 state.toml"
    );

    let c = make(&user);
    focus(&c, 1, "plain.exe");
    focus(&c, 2, "chat.exe");
    assert_eq!(c.active_schema_id(), "za", "无记录 ⇒ 全局");
    c.debug_run_menu_cmd(MenuCmd::SchemaSelect(1));
    assert_eq!(c.active_schema_id(), "zb");

    // 等 StateWriter 防抖落盘。
    let deadline = Instant::now() + Duration::from_secs(10);
    let persisted = loop {
        let text = std::fs::read_to_string(&state).unwrap_or_default();
        if text.contains("chat.exe") || Instant::now() > deadline {
            break text;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let v: toml::Value = toml::from_str(&persisted).expect("state.toml 应可解析");
    assert_eq!(
        v.get("app_schemas")
            .and_then(|t| t.get("chat.exe"))
            .and_then(|s| s.as_str()),
        Some("zb"),
        "记忆表应写进 state.toml：\n{persisted}"
    );

    // ── 「重启」：新协调器从 state.toml 读回记忆表 ──
    let c2 = make(&user);
    focus(&c2, 1, "plain.exe");
    assert_eq!(c2.active_schema_id(), "za");
    focus(&c2, 2, "chat.exe");
    assert_eq!(c2.active_schema_id(), "zb", "重启后切入应恢复记忆的方案");

    // ── 记录的方案已不在 available ⇒ 当没记过（回落全局），但不清理记录 ──
    std::fs::write(&state, "[app_schemas]\n\"chat.exe\" = \"gone\"\n").unwrap();
    let c3 = make(&user);
    focus(&c3, 1, "plain.exe");
    focus(&c3, 2, "chat.exe");
    assert_eq!(c3.active_schema_id(), "za", "记录的方案不可用 ⇒ 全局");
    assert_eq!(
        c3.debug_app_schema_state()
            .1
            .get("chat.exe")
            .map(String::as_str),
        Some("gone"),
        "不主动清理：方案可能被重新启用"
    );

    drop((c, c2, c3));
    let _ = std::fs::remove_dir_all(&root);
}
