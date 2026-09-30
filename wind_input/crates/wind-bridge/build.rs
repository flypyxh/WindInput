fn main() {
    emit_platform_aliases();
}

/// 平台语义别名（三个 crate 的 build.rs 各存一份，改动须同步：wind-ui / wind-bridge / wind-coordinator）。
///
/// - `ext_presenter`：呈现层由**外部宿主进程**承担（macOS `.app` / Linux Fcitx5 addon）——服务进程只
///   光栅化，窗口、菜单、设置入口、按键合成都归宿主。macOS 恒为真；Linux 须开 `linux-host`
///   feature，默认关，使 Linux 开发机上的 `cargo test` 仍走 mock 文本，行为与原先一致。
/// - `mock_text`：文本后端是 mock（等宽近似）。含文本的布局测试据此门控，取代原先的
///   `not(windows), not(target_os = "macos")`——Linux 开 `linux-host` 后文本是真的，数值不再确定。
fn emit_platform_aliases() {
    println!("cargo:rustc-check-cfg=cfg(ext_presenter)");
    println!("cargo:rustc-check-cfg=cfg(mock_text)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let linux_host = os == "linux" && std::env::var_os("CARGO_FEATURE_LINUX_HOST").is_some();
    if os == "macos" || linux_host {
        println!("cargo:rustc-cfg=ext_presenter");
    }
    if os != "windows" && os != "macos" && !linux_host {
        println!("cargo:rustc-cfg=mock_text");
    }
}
