//! 未启用扩展词库的通配 / 注释反查（reverse-mode spec §4），自造夹具，不依赖 build_dev/data。

use std::path::{Path, PathBuf};
use wind_config::Config;
use wind_engine::EngineManager;

fn uid(tag: &str) -> String {
    format!("zz_ldd_{tag}_{}", std::process::id())
}

pub struct Cleanup {
    pub id: String,
    pub dir: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(cache) = Config::cache_dir() {
            let _ = std::fs::remove_dir_all(cache.join(&self.id));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 主库：工 a / 立法 uuif；已启用扩展 `_ext`：甘蓝菜 aaae；未启用扩展 `_xz`：门头沟区 uuia。
/// `_xz` 的权重刻意高于主库同前缀的「立法」：「已启用排前」的断言因此只能靠未启用标记成立，
/// 不会被权重巧合蒙过去。
/// `xz_file = false` ⇒ `_xz` 的 path 指向不存在的文件（缺失处理）。`with_xz = false` ⇒ 不声明 `_xz`。
fn write_fixture(dir: &Path, id: &str, with_xz: bool, xz_file: bool) {
    let s = dir.join("schemas");
    std::fs::create_dir_all(s.join(id)).unwrap();
    let mut toml = format!(
        "[schema]\nid = \"{id}\"\nname = \"测影\"\n[engine]\ntype = \"codetable\"\n\
         [engine.codetable]\nmax_code_length = 4\n\
         [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n\
         [[dictionaries]]\nid = \"{id}_ext\"\npath = \"{id}/ext.dict.yaml\"\ntype = \"rime_codetable\"\n\
         default_enabled = true\nbase_order = 1\n"
    );
    if with_xz {
        toml += &format!(
            "[[dictionaries]]\nid = \"{id}_xz\"\npath = \"{id}/xz.dict.yaml\"\ntype = \"rime_codetable\"\n\
             default_enabled = false\nbase_order = 3\ndefault_weight = 500\n"
        );
    }
    std::fs::write(s.join(format!("{id}.schema.toml")), toml).unwrap();
    let dict = |name: &str, body: &str| {
        format!(
            "---\nname: {name}\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n{body}"
        )
    };
    std::fs::write(
        s.join(format!("{id}/main.dict.yaml")),
        dict("main", "a\t工\t100\nuuif\t立法\t10\n"),
    )
    .unwrap();
    std::fs::write(
        s.join(format!("{id}/ext.dict.yaml")),
        dict("ext", "aaae\t甘蓝菜\t50\n"),
    )
    .unwrap();
    if with_xz && xz_file {
        std::fs::write(
            s.join(format!("{id}/xz.dict.yaml")),
            dict("xz", "uuia\t门头沟区\t1000\n"),
        )
        .unwrap();
    }
}

fn setup(tag: &str, on: bool, with_xz: bool, xz_file: bool) -> (EngineManager, String, Cleanup) {
    let id = uid(tag);
    let dir = std::env::temp_dir().join(format!("wind_ldd_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    let guard = Cleanup {
        id: id.clone(),
        dir: dir.clone(),
    };
    write_fixture(&dir, &id, with_xz, xz_file);
    (manager(&dir, &id, on), id, guard)
}

fn manager(dir: &Path, id: &str, on: bool) -> EngineManager {
    let mut cfg = Config::default();
    cfg.schema.available = vec![id.into()];
    cfg.schema.active = id.into();
    cfg.schema.codetable.wildcard = true;
    cfg.schema.codetable.lookup_disabled_dicts = on;
    EngineManager::with_store_override(&cfg, Some(dir), None, Some(dir.join("ov")))
}

fn slot(p: &str) -> String {
    p.replace('?', &wind_dict::WILDCARD_SLOT.to_string())
}

fn wc_texts(m: &EngineManager, input: &str, pat: &str) -> Vec<String> {
    m.convert_wildcard(input, &slot(pat), 50)
        .unwrap_or_default()
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn wildcard_sees_disabled_extra_only_when_switch_on() {
    let (on, _id, _g) = setup("on", true, true, true);
    assert_eq!(wc_texts(&on, "uuiz", "uui?"), ["立法", "门头沟区"]);
    let (off, _id2, _g2) = setup("off", false, true, true);
    assert_eq!(wc_texts(&off, "uuiz", "uui?"), ["立法"]);
}

/// ★ Review Focus 1（接线层）：开关开着，普通 convert 照旧看不到未启用库。
#[test]
fn plain_convert_ignores_disabled_extra_with_switch_on() {
    let (m, _id, _g) = setup("plain", true, true, true);
    for input in ["uuia", "uui"] {
        assert!(
            m.convert(input, 50)
                .candidates
                .iter()
                .all(|c| c.text != "门头沟区"),
            "{input}"
        );
    }
}

/// 热禁用已启用扩展库 ⇒ 普通候选消失、通配经影子层仍可见。
#[test]
fn live_disable_moves_extra_into_lookup() {
    let (m, id, _g) = setup("live", true, true, true);
    assert!(
        m.convert("aaae", 20)
            .candidates
            .iter()
            .any(|c| c.text == "甘蓝菜")
    );
    assert!(m.set_dict_enabled_live(&id, &format!("{id}_ext"), false));
    assert!(
        m.convert("aaae", 20)
            .candidates
            .iter()
            .all(|c| c.text != "甘蓝菜")
    );
    assert!(wc_texts(&m, "aaaz", "aaa?").contains(&"甘蓝菜".to_string()));
}

/// spec §4.3：未启用库文件缺失 ⇒ 跳过（warn），通配照常出主库结果。
#[test]
fn missing_disabled_file_is_skipped() {
    let (m, _id, _g) = setup("missing", true, true, false);
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法"]);
}

/// 影子层只收未启用的扩展库——主库、已启用扩展库的命中恒不带未启用标记，
/// 只有 `_xz` 的带。（`(text, code)` 去重也会遮住误收；判据的直接证明在 manager 单元测试。）
#[test]
fn only_disabled_extra_hits_carry_the_flag() {
    let (m, _id, _g) = setup("flag", true, true, true);
    let flags = |pat: &str, input: &str| -> Vec<(String, bool)> {
        m.convert_wildcard(input, &slot(pat), 50)
            .unwrap_or_default()
            .candidates
            .into_iter()
            .map(|c| (c.text, c.from_disabled_dict))
            .collect()
    };
    assert_eq!(
        flags("uui?", "uuiz"),
        [("立法".to_string(), false), ("门头沟区".to_string(), true)]
    );
    assert_eq!(flags("aaa?", "aaaz"), [("甘蓝菜".to_string(), false)]);
}

/// 等后台预热线程把影子层建完（最多 10 秒）；返回最终的建表次数。
fn wait_disabled_loaded(m: &EngineManager, id: &str) -> Option<usize> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let n = m.disabled_dicts_load_count(id);
        if n != Some(0) || std::time::Instant::now() >= deadline {
            return n;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// 开关开：引擎建好后影子层由后台线程预热，首次通配不再在按键线程上读盘（只建一遍）。
#[test]
fn disabled_layers_warm_in_background_when_switch_on() {
    let (m, id, _g) = setup("warm_on", true, true, true);
    assert!(m.prewarm_schema(&id));
    assert_eq!(wait_disabled_loaded(&m, &id), Some(1), "后台预热应已建好");
    assert!(!m.prewarm_disabled_dicts(&id), "已建好，不再建");
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法", "门头沟区"]);
    assert_eq!(
        m.disabled_dicts_load_count(&id),
        Some(1),
        "首次通配复用预热结果"
    );
}

/// 开关关：引擎不挂影子层，预热无从谈起（不起线程、不读盘）。
#[test]
fn switch_off_never_warms_disabled_layers() {
    let (m, id, _g) = setup("warm_off", false, true, true);
    assert!(m.prewarm_schema(&id));
    assert_eq!(m.disabled_dicts_load_count(&id), None);
    assert!(!m.prewarm_disabled_dicts(&id));
    assert_eq!(wc_texts(&m, "uuiz", "uui?"), ["立法"]);
}

/// ★ Review Focus 4：开关开着，注释 / 悬停 / 查重一律只认已启用词库；不存在能带出未启用库编码的路径。
#[test]
fn comment_reverse_ignores_disabled_extra_even_when_on() {
    let (m, id, _g) = setup("cmt", true, true, true);
    assert!(m.prewarm_reverse_index(&id));
    assert_eq!(m.codetable_reverse_hint("门头沟区").as_deref(), Some(""));
    assert_eq!(m.word_codes_display(&id, "门头沟区").as_deref(), Some(""));
    assert_eq!(m.word_codes_in(&id, "门头沟区").as_deref(), Some(""));
    assert_eq!(m.codetable_reverse_hint("立法").as_deref(), Some("uuif"));
}

/// 缓存目录里的反查索引文件只有常规那一份 `<id>.wridx`，不另建含未启用库的变体文件。
#[test]
fn no_variant_index_file_is_written() {
    let (m, id, _g) = setup("nofile", true, true, true);
    assert!(m.prewarm_reverse_index(&id));
    let _ = m.codetable_reverse_hint("立法");
    if let Some(dir) = Config::cache_dir().map(|c| c.join(&id)) {
        let names: Vec<String> = std::fs::read_dir(&dir)
            .map(|it| {
                it.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let regular = format!("{id}.wridx");
        assert!(
            names
                .iter()
                .filter(|n| n.ends_with(".wridx"))
                .all(|n| *n == regular),
            "{names:?}"
        );
    }
}
