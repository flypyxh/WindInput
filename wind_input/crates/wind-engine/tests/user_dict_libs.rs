//! 用户分库（`data/<数据方案>/dicts/*.dict.yaml`）登记为方案的扩展词库：id `user:<名>`、码表档位
//! 与用户词同档（-3）、开关取 `dicts.toml`、拼音族共用 `data/pinyin/`。自造夹具，不依赖 build_dev/data。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use wind_config::Config;
use wind_engine::EngineManager;
use wind_store::Store;

struct Fx {
    dir: PathBuf,
    id: String,
    store: Arc<Store>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        if let Some(cache) = Config::cache_dir() {
            let _ = std::fs::remove_dir_all(cache.join(&self.id));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fx {
    fn new(tag: &str, schema_toml: &dyn Fn(&str) -> String, main: &str) -> Fx {
        let id = format!("zz_udl_{tag}_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("wind_udl_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("schemas");
        std::fs::create_dir_all(s.join(&id)).unwrap();
        std::fs::write(s.join(format!("{id}.schema.toml")), schema_toml(&id)).unwrap();
        std::fs::write(s.join(format!("{id}/main.dict.yaml")), main).unwrap();
        let store = Arc::new(Store::open(dir.join("u.redb")).unwrap());
        Fx { dir, id, store }
    }

    fn manager(&self) -> EngineManager {
        let mut cfg = Config::default();
        cfg.schema.available = vec![self.id.clone()];
        cfg.schema.active = self.id.clone();
        EngineManager::with_store_override(
            &cfg,
            Some(&self.dir),
            Some(self.store.clone()),
            Some(self.dir.join("ov")),
        )
    }

    fn lib(&self, ds: &str, name: &str) -> PathBuf {
        wind_store::user_dicts::lib_path(self.store.user_text_dir(), ds, name)
    }
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn texts(m: &EngineManager, id: &str, input: &str) -> Vec<String> {
    m.convert_with(id, input, 20)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

fn codetable_toml(id: &str) -> String {
    format!(
        "[schema]\nid = \"{id}\"\nname = \"测\"\n[engine]\ntype = \"codetable\"\n\
         [engine.codetable]\nmax_code_length = 4\n\
         [[dictionaries]]\nid = \"main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n"
    )
}

const CT_MAIN: &str =
    "---\nname: main\ncolumns: [text, code, weight]\n...\n工\ta\t100\n式\taa\t10\n";

#[test]
fn codetable_lib_registers_ahead_of_system_and_follows_switch() {
    let fx = Fx::new("ct", &codetable_toml, CT_MAIN);
    let id = fx.id.clone();
    write(
        &fx.lib(&id, "mine"),
        "---\nname: 我的词\n...\n戈\ta\t100\n自造\tzz\t1\n",
    );
    let m = fx.manager();

    let s = m.schema_merged(&id).unwrap();
    let d = s
        .dictionaries
        .iter()
        .find(|d| d.id == "user:mine")
        .expect("分库登记为方案词库");
    assert_eq!(
        (d.label.as_str(), d.base_order, d.enabled),
        ("我的词", -3, Some(true))
    );
    assert!(m.schema_base(&id).unwrap().dictionaries.len() == 1);

    assert_eq!(
        texts(&m, &id, "a")[..2],
        ["戈", "工"],
        "同权时分库排在系统库前"
    );

    fx.store
        .set_user_dict_enabled(&id, &id, "mine", false)
        .unwrap();
    m.set_dict_enabled_live(&id, "user:mine", false);
    assert!(
        !texts(&m, &id, "a").contains(&"戈".to_string()),
        "关掉即摘层"
    );
    assert!(texts(&m, &id, "zz").is_empty());
    let s = m.schema_merged(&id).unwrap();
    assert_eq!(
        s.dictionaries
            .iter()
            .find(|d| d.id == "user:mine")
            .unwrap()
            .enabled,
        Some(false)
    );

    fx.store
        .set_user_dict_enabled(&id, &id, "mine", true)
        .unwrap();
    m.set_dict_enabled_live(&id, "user:mine", true);
    assert_eq!(texts(&m, &id, "a")[0], "戈", "重新打开");
}

#[test]
fn reload_detects_changed_libs_and_rebuilds() {
    let fx = Fx::new("reload", &codetable_toml, CT_MAIN);
    let id = fx.id.clone();
    let lib = fx.lib(&id, "mine");
    write(&lib, "---\nname: m\n...\n戈\ta\t100\n");
    let m = fx.manager();
    assert!(texts(&m, &id, "b").is_empty());
    assert!(m.user_dicts_changed().is_empty(), "没改不报");

    write(&lib, "---\nname: m\n...\n戈\ta\t100\n新词\tb\t5\n");
    assert_eq!(m.user_dicts_changed(), std::slice::from_ref(&id));
    m.invalidate_user_dicts(&id);
    assert_eq!(texts(&m, &id, "b"), ["新词"]);
    assert!(m.user_dicts_changed().is_empty());

    // 新放进来的分库：出厂开
    write(&fx.lib(&id, "second"), "---\nname: s\n...\n第二\tc\t1\n");
    assert_eq!(m.user_dicts_changed(), std::slice::from_ref(&id));
    m.invalidate_user_dicts(&id);
    assert_eq!(texts(&m, &id, "c"), ["第二"]);
}

fn pinyin_toml(id: &str) -> String {
    format!(
        "[schema]\nid = \"{id}\"\nname = \"拼\"\n[engine]\ntype = \"pinyin\"\n\
         [[dictionaries]]\nid = \"main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"rime_pinyin\"\ndefault = true\n"
    )
}

#[test]
fn pinyin_libs_live_under_the_folded_data_schema() {
    let fx = Fx::new(
        "py",
        &pinyin_toml,
        "---\nname: main\n...\n你好\tni hao\t100\n后\thou\t100\n",
    );
    let id = fx.id.clone();
    write(
        &fx.lib("pinyin", "cy"),
        "---\nname: 成语\n...\n厚积薄发\thou ji bo fa\t5\n",
    );
    let m = fx.manager();
    assert_eq!(m.data_schema_id(&id), "pinyin");
    let s = m.schema_merged(&id).unwrap();
    let d = s.dictionaries.iter().find(|d| d.id == "user:cy").unwrap();
    assert_eq!(d.dict_type, "rime_pinyin");
    assert_eq!(texts(&m, &id, "houjibofa")[0], "厚积薄发");

    // 开关按方案 id 记（不是数据方案）
    fx.store
        .set_user_dict_enabled("pinyin", &id, "cy", false)
        .unwrap();
    m.set_dict_enabled_live(&id, "user:cy", false);
    assert!(!texts(&m, &id, "houjibofa").contains(&"厚积薄发".to_string()));
}

#[test]
fn libs_of_other_data_schemas_are_not_registered() {
    let fx = Fx::new("other", &codetable_toml, CT_MAIN);
    write(&fx.lib("pinyin", "cy"), "---\nname: c\n...\n戈\ta\t100\n");
    let m = fx.manager();
    let s = m.schema_merged(&fx.id).unwrap();
    assert!(s.dictionaries.iter().all(|d| !d.id.starts_with("user:")));
}

/// 按键路径上取混输拼音子方案不碰用户分库：开关文件坏了也不在这里读出问题来。
#[test]
fn mixed_secondary_lookup_does_not_touch_user_dicts() {
    let fx = Fx::new(
        "py2",
        &pinyin_toml,
        "---\nname: main\n...\n你好\tni hao\t100\n",
    );
    let id = fx.id.clone();
    write(
        &fx.lib("pinyin", "cy"),
        "---\nname: 成语\n...\n厚\thou\t5\n",
    );
    let m = fx.manager();
    let meta = wind_store::user_dicts::meta_path(fx.store.user_text_dir(), "pinyin");
    write(&meta, "not = [valid");
    wind_store::issues::clear(&meta);
    assert_eq!(m.mixed_secondary_schema(&id), None);
    assert!(
        wind_store::issues::list_under(fx.store.user_text_dir()).is_empty(),
        "不读开关文件"
    );
}
