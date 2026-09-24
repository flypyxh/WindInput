//! 角标色值拆键（`auto`/`#RRGGBBAA` → `""`/`#RRGGBB` + `alpha_light|dark`）在**两条写盘
//! 路径**上都落盘为新形态，且不丢 8 位写法里的不透明度。
//!
//! 为什么要测写盘路径而不只测 `load()`：两条路径都会整表写回用户文件。若写回前不迁移，
//! 旧写法原样留在文件里——下次 load 仍能迁移，看似无害；但设置端读到的用户层就是旧形态，
//! 而且 `prune_user_config` 一旦改动别的键就会把这份旧形态固化下去。
//!
//! 为什么必须是集成测试（独立进程）：用户目录经 `WIND_DATADIR_CONF` 重定向，OnceLock
//! 进程内只认一次。
//!
//! ⚠️ 全文件仅此一个 `#[test]`：多个测试在同一二进制里并行会争抢环境变量与 OnceLock。
//! ⚠️ `WIND_DATADIR_CONF` 必须设，否则会真写用户的 `%APPDATA%\WindInput\config.toml`。

use std::path::Path;
use wind_config::Config;

fn write_at(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

const OLD_BADGES: &str = "[ui.langbar]\nbadge = \"dot\"\n\n\
[[ui.langbar.badges]]\nstate = \"punct_cn\"\ncolor_light = \"#2288E080\"\ncolor_dark = \"auto\"\n";

/// 断言用户文件里那条角标已是新形态：色值 6 位 / 空串，alpha 拆到独立键。
fn assert_new_form(text: &str, which: &str) {
    let v: toml::Value = toml::from_str(text).expect("写回的内容必须是合法 TOML");
    let b = &v["ui"]["langbar"]["badges"][0];
    assert_eq!(
        b["color_light"].as_str(),
        Some("#2288E0"),
        "{which}：8 位色值须截成 6 位\n{text}"
    );
    assert_eq!(
        b["color_dark"].as_str(),
        Some(""),
        "{which}：auto 须迁为空串\n{text}"
    );
    let a = b
        .get("alpha_light")
        .and_then(toml::Value::as_float)
        .unwrap_or_else(|| panic!("{which}：8 位写法的不透明度不得丢失\n{text}"));
    assert!((a - 128.0 / 255.0).abs() < 1e-6, "{which}：alpha={a}");
    assert!(
        b.get("alpha_dark").is_none(),
        "{which}：auto 那一侧没写过不透明度，不该凭空冒出 alpha_dark\n{text}"
    );
}

#[test]
fn badge_color_migration_lands_on_disk_via_prune_and_set_user_value() {
    let tmp = std::env::temp_dir().join("wind_badge_migration_e2e");
    let root = tmp.join("install");
    let user = tmp.join("UserData");
    let conf = tmp.join("datadir.conf");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(&conf, user.to_string_lossy().as_bytes()).unwrap();

    // SAFETY: 本文件仅此一个测试，env 在任何 OnceLock 初始化之前设置，无并发读者。
    unsafe {
        std::env::set_var("WIND_DATADIR_CONF", &conf);
        std::env::set_var("WIND_INSTALL_ROOT", &root);
    }
    assert_eq!(
        Config::user_config_dir(),
        Some(user.clone()),
        "前置条件：用户目录须已重定向，否则本测试会读写真实 %APPDATA%"
    );
    let file = user.join("config.toml");

    // ── 一、prune_user_config：只有待迁移的旧写法，也必须落盘 ───────────────
    write_at(&user, "config.toml", OLD_BADGES);
    let n = Config::prune_user_config().unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        n >= 1,
        "迁移本身就是一处改动，须计数并写盘（n={n}）\n{text}"
    );
    assert_new_form(&text, "prune_user_config");
    assert_eq!(Config::prune_user_config().unwrap(), 0, "prune 必须幂等");

    // ── 二、set_user_value：改一个无关键，整表写回时旧写法一并迁掉 ─────────
    write_at(&user, "config.toml", OLD_BADGES);
    Config::set_user_value(&["schema", "active"], toml::Value::String("wubi98".into())).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("wubi98"), "本次修改要落盘\n{text}");
    assert_new_form(&text, "set_user_value");

    let _ = std::fs::remove_dir_all(&tmp);
}
