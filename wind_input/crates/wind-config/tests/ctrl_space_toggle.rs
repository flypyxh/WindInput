//! GH#172：Ctrl+空格中英切换的开关（`keys.ctrl_space_toggle`）及其「有效值」判定。
//!
//! 有效值 = 开关开着 **且** 「全局自定义按键」（`keys.key_actions`）里没有把 Ctrl+空格
//! 绑成 `none`。后一条让「在自定义按键里把 Ctrl+空格 设为不启用」真的关得掉它——此前那只
//! 删了一条本不存在的绑定，切换照旧（报障原话：「设为不启用也禁不掉」）。

use wind_config::Config;

#[test]
fn ships_enabled() {
    let cfg = Config::default();
    assert!(
        cfg.keys.ctrl_space_toggle,
        "出厂必须开：老用户的 Ctrl+空格 不能因升级失灵"
    );
    assert!(cfg.keys.ctrl_space_toggle_effective());
}

#[test]
fn missing_key_in_toml_defaults_to_enabled() {
    // 老配置里没有这个键 ⇒ serde 默认值必须是 true，而不是 bool 的 false。
    let cfg: Config = toml::from_str("[keys]\ncommit_on_switch = true\n").unwrap();
    assert!(cfg.keys.ctrl_space_toggle);
}

#[test]
fn switch_off_disables() {
    let mut cfg = Config::default();
    cfg.keys.ctrl_space_toggle = false;
    assert!(!cfg.keys.ctrl_space_toggle_effective());
}

#[test]
fn key_actions_none_disables_regardless_of_spelling() {
    for key in ["ctrl+space", "Ctrl+Space", " control + space "] {
        for verb in ["none", "NONE", ""] {
            let mut cfg = Config::default();
            cfg.keys
                .key_actions
                .insert(key.to_string(), verb.to_string());
            assert!(
                !cfg.keys.ctrl_space_toggle_effective(),
                "key_actions[{key:?}] = {verb:?} 应关掉内置 Ctrl+空格 切换"
            );
        }
    }
}

#[test]
fn key_actions_other_bindings_do_not_disable() {
    // 绑成真动作（如 toggle_mode）不关：系统热键在 keystroke sink 之下就翻 compartment，
    // 若此时也拒绝，Ctrl+空格 在启用了系统热键的机器上就什么都不做了。
    let mut cfg = Config::default();
    cfg.keys
        .key_actions
        .insert("ctrl+space".into(), "toggle_mode".into());
    assert!(cfg.keys.ctrl_space_toggle_effective());

    // 别的键绑 none 不相干。
    let mut cfg = Config::default();
    cfg.keys
        .key_actions
        .insert("ctrl+shift+space".into(), "none".into());
    cfg.keys
        .key_actions
        .insert("shift+space".into(), "none".into());
    assert!(cfg.keys.ctrl_space_toggle_effective());
}
