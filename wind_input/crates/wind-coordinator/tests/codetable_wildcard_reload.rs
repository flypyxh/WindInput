//! 设置里开 / 关通配后热重载（`reload_user_config`），透传集必须**当场**跟上。
//!
//! 透传集里的键空缓冲时 C++ 不吃，首位符号通配键（`/`）若还留在集合里就到不了 core。
//! 热重载先建配置快照、后 `reload_from_config` 刷新引擎管理器——快照若读管理器手里的全局
//! 码表设置，拿到的是**上一版**开关：开了 `/` 仍透传，关了 `/` 仍被吃。
//!
//! 要写盘：用户目录经 `WIND_DATADIR_CONF`、安装根经 `WIND_INSTALL_ROOT` 重定向，一个进程
//! 只能重定向一次，故单开一个测试二进制（同 `password_force_english_persist.rs`）。
//!
//! ⚠️ 安装根指向 `build_dev`（`data_dir()` = `build_dev/data`）；缺数据时静默跳过。

use std::path::PathBuf;
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_host::KeyProbe;

const VK_SLASH: u32 = 0xBF;

fn build_dev() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev")
}

fn set(path: &[&str], v: toml::Value) {
    Config::set_user_value(path, v).unwrap();
}

#[test]
fn toggling_wildcard_via_reload_updates_passthrough_set() {
    if !build_dev()
        .join("data/schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
    {
        eprintln!("跳过：五笔词库不存在");
        return;
    }
    // ⚠️ 目录名带 pid：多 worktree / 多会话并行跑测试时固定名会互删夹具。
    let tmp = std::env::temp_dir().join(format!("wind_coord_wc_reload-{}", std::process::id()));
    let user = tmp.join("UserData");
    let conf = tmp.join("datadir.conf");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(&conf, user.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: 本文件仅此一个测试，env 在任何 OnceLock 初始化之前设置，无并发读者。
    unsafe {
        std::env::set_var("WIND_DATADIR_CONF", &conf);
        std::env::set_var("WIND_INSTALL_ROOT", build_dev());
    }
    assert_eq!(
        Config::user_config_dir(),
        Some(user.clone()),
        "前置条件：用户目录须已重定向，否则本测试会读写真实用户配置"
    );

    let arr = |v: &[&str]| toml::Value::Array(v.iter().map(|s| (*s).into()).collect());
    set(&["schema", "available"], arr(&["wubi86"]));
    set(&["schema", "active"], "wubi86".into());
    set(&["schema", "codetable", "wildcard"], false.into());
    set(&["schema", "codetable", "wildcard_key"], "/".into());

    let data = Config::data_dir();
    let coord = Coordinator::new_headless(Config::load(data.as_deref()).unwrap(), data.as_deref());
    assert!(
        !coord.should_handle_key(&KeyProbe::new(VK_SLASH)),
        "前置：通配关闭时 `/` 透传"
    );

    // ── 设置里打开通配：热重载后 `/` 立即不再透传 ──
    set(&["schema", "codetable", "wildcard"], true.into());
    // 返回值是「加载失败」（成功为 false），见 `reload_user_config` 末尾。
    assert!(!coord.reload_user_config(), "热重载应成功");
    assert!(
        coord.should_handle_key(&KeyProbe::new(VK_SLASH)),
        "开启通配并热重载后，空缓冲的 `/` 必须送到服务端"
    );

    // ── 再关掉：`/` 恢复透传 ──
    set(&["schema", "codetable", "wildcard"], false.into());
    // 返回值是「加载失败」（成功为 false），见 `reload_user_config` 末尾。
    assert!(!coord.reload_user_config(), "热重载应成功");
    assert!(
        !coord.should_handle_key(&KeyProbe::new(VK_SLASH)),
        "关闭通配并热重载后，`/` 应恢复透传"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
