//! UDS / SHM 端点路径（macOS 与 Linux 共用）
//!
//! 与 Go `internal/bridge/endpoint_darwin.go` 及 Swift `ProtocolTypes.swift`
//! 的 `BridgeEndpoints` 对齐。
use std::path::PathBuf;

/// 把**管道后缀**（`""` / `"_dev"`，见 `wind_config::variant::pipe_suffix`）映射成
/// **变体后缀**（`""` / `"Dev"`）。
///
/// 这两种后缀是两套命名风格，混用会让 dev 变体的两端各说各话：
/// 调用方一路传下来的是管道风格的 `_dev`（它给 `wind_input_ctrl_dev` 这类管道名用），
/// 但 Application Support 下的目录名与 SHM 名走的是变体风格 `Dev` ——
/// Swift `BridgeEndpoints.variantSuffix`、`wind_config::variant::app_dir_name()`
/// 和 `scripts/mac/dev.sh` 三处都是 `WindInputDev`。
///
/// 曾经此处直接把 `_dev` 拼进目录名，于是服务把 socket bind 到 `WindInput_dev/`，
/// 而 .app 去 `WindInputDev/` 连——dev 变体的 bridge/SHM 全程握不上手。
/// 改名须三处同步。
///
/// 未知后缀（测试/临时场景的 `_debug` 等）原样透传：它们不是正式变体，
/// 静默改名只会让排查现场更难看懂。
fn variant_suffix(pipe_suffix: &str) -> &str {
    match pipe_suffix {
        "_dev" => "Dev",
        other => other,
    }
}

/// runtime 目录：env 覆盖 → macOS: ~/Library/Application Support/WindInput{变体后缀} / Linux: $XDG_RUNTIME_DIR/WindInput{变体后缀} → /tmp/wind_input{管道后缀}
///
/// 两段刻意用不同风格的后缀：Application Support 段是**面向用户的应用目录名**
/// （与 .app、dev.sh、用户配置目录同名），/tmp 段是无 HOME 时的兜底，
/// 沿用管道风格的 snake 命名。
pub fn runtime_dir(suffix: &str) -> PathBuf {
    if let Ok(env) = std::env::var("WIND_INPUT_RUNTIME_DIR")
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    // Linux（外部宿主形态）：socket 属运行时状态，放 `$XDG_RUNTIME_DIR`（tmpfs、仅本用户可访问、
    // 登出即清）；不放 HOME 下——那里没有 `Library/Application Support`，且 socket 不该落盘。
    // 目录名沿用变体风格（`WindInput` / `WindInputDev`），与 macOS 和 addon 侧一致。
    #[cfg(all(target_os = "linux", ext_presenter))]
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR")
        && !rt.is_empty()
    {
        return PathBuf::from(rt).join(format!("WindInput{}", variant_suffix(suffix)));
    }
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        let dir = variant_suffix(suffix);
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(format!("WindInput{dir}"));
    }
    PathBuf::from(format!("/tmp/wind_input{suffix}"))
}

pub fn request_socket_path(suffix: &str) -> PathBuf {
    runtime_dir(suffix).join("bridge.sock")
}

pub fn push_socket_path(suffix: &str) -> PathBuf {
    runtime_dir(suffix).join("bridge_push.sock")
}

/// POSIX shm 名（须以 '/' 开头，长度 <=31）。
///
/// 后缀与 socket 目录同源（`variant_suffix`），对齐 Swift
/// `CandidatePanelHost` 的 `"/WindInput_SHM\(BridgeEndpoints.variantSuffix)"`；
/// 不一致的话 dev 变体开出的是两段互不相干的共享内存，候选框永远拿不到帧。
///
/// Linux 外部宿主形态带 uid：`/WindInput{变体后缀}.<uid>`（见 [`shm_name_for_uid`]）。
/// macOS 不变（Swift 写死了名字，且 `/dev/shm` 式的全系统共用问题在那边由沙箱规避）。
pub fn shm_name(suffix: &str) -> String {
    #[cfg(all(target_os = "linux", ext_presenter))]
    return shm_name_for_uid(suffix, current_uid());
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    {
        let suffix = variant_suffix(suffix);
        let name = format!("/WindInput_SHM{suffix}");
        debug_assert!(name.len() <= 31, "shm name too long: {name}");
        name
    }
}

/// Linux 的 SHM 名：`/WindInput{变体后缀}.<uid>`，与 addon `Bridge.cpp::shmNameForUid` 同一规则
/// （两侧单测钉同一组样例）。
///
/// 为什么带 uid：POSIX shm 落在全系统共用的 `/dev/shm`。不带的话同机第二个用户的服务
/// 建不了段（上一个用户的同名段 `shm_unlink` 得 EPERM、`O_EXCL` 得 EEXIST），候选窗 / 气泡 /
/// 菜单全失效；别的用户还能抢先建个同名段喂假帧（addon 另按属主拒收）。
///
/// 长度：`/WindInput`(10) + `Dev`(3) + `.`(1) + uid（u32 至多 10 位）+ 层后缀（至多 `_MN5` 4）
/// = 28 ≤ 31。31 是 macOS 的 PSHMNAMLEN（Linux 实际上限是 NAME_MAX 255），两平台守同一个数。
/// 测试用的未知后缀原样透传（`_debug` 6 字节时最长 31，仍不越界）。
#[cfg(all(target_os = "linux", ext_presenter))]
pub fn shm_name_for_uid(suffix: &str, uid: u32) -> String {
    let name = format!("/WindInput{}.{uid}", variant_suffix(suffix));
    debug_assert!(name.len() + "_MN5".len() <= 31, "shm name too long: {name}");
    name
}

#[cfg(all(target_os = "linux", ext_presenter))]
fn current_uid() -> u32 {
    // SAFETY: getuid 无参数、总是成功。
    unsafe { libc::getuid() }
}

/// Linux 光栅浮层（`CMD_OVERLAY_FRAME`）各层的 SHM 名：候选窗那段 + 层后缀。一层一段，
/// 候选窗与气泡同时在屏上时互不覆盖。后缀与 Windows host-render（`host_render_windows.rs`
/// 的 `KIND_SUFFIXES`）同形；Toast 那层 Windows 没有。未知层返回 `None`。
pub fn overlay_shm_name(suffix: &str, kind: u32) -> Option<String> {
    Some(format!("{}{}", shm_name(suffix), overlay_tail(kind)?))
}

/// 层后缀：`_TIP` / `_STS` / `_TST` / `_MN<k>`。
fn overlay_tail(kind: u32) -> Option<String> {
    use wind_ipc::protocol::overlay::*;
    let tail = match kind {
        OVERLAY_KIND_TOOLTIP => "_TIP".to_string(),
        OVERLAY_KIND_STATUS => "_STS".to_string(),
        OVERLAY_KIND_TOAST => "_TST".to_string(),
        // 菜单每级一段：`_MN0`、`_MN1`…
        k if (OVERLAY_KIND_MENU..OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS).contains(&k) => {
            format!("_MN{}", k - OVERLAY_KIND_MENU)
        }
        _ => return None,
    };
    Some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 串行化所有动 env 的测试（env 是进程全局，默认并行会互扰）
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 保存并在 Drop 时恢复指定 env 变量（panic-safe）
    struct EnvRestore {
        keys: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }
    impl EnvRestore {
        fn capture(keys: &[&'static str]) -> Self {
            Self {
                keys: keys.iter().map(|k| (*k, std::env::var_os(k))).collect(),
            }
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (k, v) in &self.keys {
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    #[test]
    fn runtime_dir_honors_env_override() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = EnvRestore::capture(&["WIND_INPUT_RUNTIME_DIR", "HOME"]);
        unsafe { std::env::set_var("WIND_INPUT_RUNTIME_DIR", "/tmp/wind_test_rt") };
        assert_eq!(
            runtime_dir(""),
            std::path::PathBuf::from("/tmp/wind_test_rt")
        );
        assert_eq!(
            request_socket_path(""),
            std::path::PathBuf::from("/tmp/wind_test_rt/bridge.sock")
        );
        assert_eq!(
            push_socket_path(""),
            std::path::PathBuf::from("/tmp/wind_test_rt/bridge_push.sock")
        );
    }

    /// Linux 外部宿主形态：socket 在 `$XDG_RUNTIME_DIR/WindInput[Dev]`，且绝不落到 HOME 下。
    #[cfg(all(target_os = "linux", ext_presenter))]
    #[test]
    fn linux_runtime_dir_uses_xdg_runtime_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = EnvRestore::capture(&["WIND_INPUT_RUNTIME_DIR", "XDG_RUNTIME_DIR", "HOME"]);
        unsafe { std::env::remove_var("WIND_INPUT_RUNTIME_DIR") };
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            runtime_dir(""),
            std::path::PathBuf::from("/run/user/1000/WindInput")
        );
        assert_eq!(
            request_socket_path("_dev"),
            std::path::PathBuf::from("/run/user/1000/WindInputDev/bridge.sock")
        );
        // 没有 XDG_RUNTIME_DIR 时退到 /tmp，而不是 HOME 下的 macOS 式路径。
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        assert_eq!(
            runtime_dir("_dev"),
            std::path::PathBuf::from("/tmp/wind_input_dev")
        );
    }

    // 以下三条断言 macOS 式的 HOME 路径，Linux 外部宿主形态不适用。
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn runtime_dir_tmp_fallback_with_suffix() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = EnvRestore::capture(&["WIND_INPUT_RUNTIME_DIR", "HOME"]);
        unsafe { std::env::remove_var("WIND_INPUT_RUNTIME_DIR") };
        unsafe { std::env::remove_var("HOME") };
        assert_eq!(
            runtime_dir("_debug"),
            std::path::PathBuf::from("/tmp/wind_input_debug")
        );
        assert_eq!(
            request_socket_path("_debug"),
            std::path::PathBuf::from("/tmp/wind_input_debug/bridge.sock")
        );
        assert_eq!(
            push_socket_path("_debug"),
            std::path::PathBuf::from("/tmp/wind_input_debug/bridge_push.sock")
        );
    }

    /// macOS（及不开 linux-host 的 Linux）：名字一字不变，Swift 那边写死的是同一个串。
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn shm_name_has_leading_slash_and_suffix() {
        assert_eq!(shm_name(""), "/WindInput_SHM");
        assert_eq!(shm_name("_debug"), "/WindInput_SHM_debug");
    }

    /// Linux 外部宿主：带 uid。样例与 addon `bridge_test`（`shmNameForUid`）逐字相同，
    /// 那边读本文件核对这几个字面量——改规则两侧一起改。
    #[cfg(all(target_os = "linux", ext_presenter))]
    #[test]
    fn linux_shm_name_carries_uid() {
        use wind_ipc::protocol::overlay::*;
        assert_eq!(shm_name_for_uid("", 1000), "/WindInput.1000");
        assert_eq!(
            format!(
                "{}{}",
                shm_name_for_uid("", 1000),
                overlay_tail(OVERLAY_KIND_STATUS).unwrap()
            ),
            "/WindInput.1000_STS"
        );
        let longest = format!(
            "{}{}",
            shm_name_for_uid("_dev", u32::MAX),
            overlay_tail(OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS - 1).unwrap()
        );
        assert_eq!(longest, "/WindInputDev.4294967295_MN5");
        assert!(longest.len() <= 31);
        let uid = unsafe { libc::getuid() };
        assert_eq!(shm_name("_dev"), format!("/WindInputDev.{uid}"));
        assert_eq!(
            overlay_shm_name("", OVERLAY_KIND_TOAST).as_deref(),
            Some(format!("/WindInput.{uid}_TST").as_str())
        );
    }

    /// dev 变体的目录名必须是 `WindInputDev`（变体风格），不是把管道后缀 `_dev`
    /// 直接拼上去的 `WindInput_dev`。三处对齐：Swift `BridgeEndpoints.runtimeDir`、
    /// `wind_config::variant::app_dir_name()`、`scripts/mac/dev.sh` 的 `APP_SUPPORT`。
    ///
    /// 这条曾经是真实故障：服务 bind 在 `WindInput_dev/bridge.sock`，
    /// .app 连 `WindInputDev/bridge.sock`，dev 变体的 IPC 从来没通过。
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn dev_runtime_dir_uses_app_dir_style_suffix() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = EnvRestore::capture(&["WIND_INPUT_RUNTIME_DIR", "HOME"]);
        unsafe { std::env::remove_var("WIND_INPUT_RUNTIME_DIR") };
        unsafe { std::env::set_var("HOME", "/Users/tester") };

        assert_eq!(
            runtime_dir("_dev"),
            std::path::PathBuf::from("/Users/tester/Library/Application Support/WindInputDev")
        );
        assert_eq!(
            request_socket_path("_dev"),
            std::path::PathBuf::from(
                "/Users/tester/Library/Application Support/WindInputDev/bridge.sock"
            )
        );
        assert_eq!(
            push_socket_path("_dev"),
            std::path::PathBuf::from(
                "/Users/tester/Library/Application Support/WindInputDev/bridge_push.sock"
            )
        );
    }

    /// 正式变体（空后缀）不受映射影响 —— 已装机用户的 socket 路径不能因这次对齐而变。
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn release_runtime_dir_unchanged() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = EnvRestore::capture(&["WIND_INPUT_RUNTIME_DIR", "HOME"]);
        unsafe { std::env::remove_var("WIND_INPUT_RUNTIME_DIR") };
        unsafe { std::env::set_var("HOME", "/Users/tester") };

        assert_eq!(
            runtime_dir(""),
            std::path::PathBuf::from("/Users/tester/Library/Application Support/WindInput")
        );
    }

    /// SHM 名与 socket 目录同源：dev 走变体风格，未知后缀原样透传。
    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn overlay_shm_name_appends_kind_suffix() {
        use wind_ipc::protocol::overlay::*;
        assert_eq!(
            overlay_shm_name("_dev", OVERLAY_KIND_STATUS).as_deref(),
            Some("/WindInput_SHMDev_STS")
        );
        assert_eq!(
            overlay_shm_name("", OVERLAY_KIND_TOOLTIP).as_deref(),
            Some("/WindInput_SHM_TIP")
        );
        assert_eq!(
            overlay_shm_name("", OVERLAY_KIND_TOAST).as_deref(),
            Some("/WindInput_SHM_TST")
        );
        assert_eq!(overlay_shm_name("", 0), None, "候选窗那段不归本函数");
        assert_eq!(
            overlay_shm_name("_dev", OVERLAY_KIND_MENU + 2).as_deref(),
            Some("/WindInput_SHMDev_MN2")
        );
        assert_eq!(
            overlay_shm_name("", OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS),
            None,
            "超出菜单级数上限"
        );
    }

    #[cfg(not(all(target_os = "linux", ext_presenter)))]
    #[test]
    fn shm_name_maps_dev_suffix_like_socket_dir() {
        assert_eq!(shm_name("_dev"), "/WindInput_SHMDev");
        // POSIX shm 名上限 31 字节，映射后不得越界。
        assert!(shm_name("_dev").len() <= 31);
    }
}
