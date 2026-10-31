//! 用户词与主库 `default_weight` 的位次（GH#248）：两类用户词的权重来源不同——
//! **手动加词** 1200（`ADD_WORD_WEIGHT`，wind-coordinator），**自动造词晋升** 1000
//! （`wind_store::PROMOTED_WEIGHT`）。主库 `default_weight` 覆盖整库权重后，1100 的主库
//! 恰好压过晋升词、被手动词压过——「自造词为什么排后面」的回答钉在这里：
//! 不是排序 bug，是两个出厂默认值不同（调整入口：词库管理里改该词权重）。
//!
//! 自造夹具，不依赖 build_dev/data。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use wind_config::Config;
use wind_engine::EngineManager;

fn uid(tag: &str) -> String {
    format!("zz_uww_{tag}_{}", std::process::id())
}

struct Cleanup {
    id: String,
    dir: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(cache) = Config::cache_dir() {
            let _ = std::fs::remove_dir_all(cache.join(&self.id));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 主库两码重码（立法 / 立法院）+ 邻码（立场），`default_weight` 由用例给定。
fn write_fixture(dir: &Path, id: &str, main_weight: i32) {
    let s = dir.join("schemas");
    std::fs::create_dir_all(s.join(id)).unwrap();
    let toml = format!(
        "[schema]\nid = \"{id}\"\nname = \"测影\"\n[engine]\ntype = \"codetable\"\n\
         [engine.codetable]\nmax_code_length = 4\n\
         [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\ndefault_weight = {main_weight}\n"
    );
    std::fs::write(s.join(format!("{id}.schema.toml")), toml).unwrap();
    let dict = "---\nname: main\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n\
                uuif\t立法\t10\nuuif\t立法院\t20\nuuik\t立场\t30\n";
    std::fs::write(s.join(format!("{id}/main.dict.yaml")), dict).unwrap();
}

fn setup(
    tag: &str,
    main_weight: i32,
    user_weight: i32,
    user_text: &str,
) -> (EngineManager, Cleanup) {
    let id = uid(tag);
    let dir = std::env::temp_dir().join(format!("wind_uww_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_fixture(&dir, &id, main_weight);
    let store = Arc::new(wind_store::Store::open(dir.join("user_data.db")).unwrap());
    store
        .add_user_word(&id, "uuif", user_text, user_weight, 0)
        .unwrap();
    let mut cfg = Config::default();
    cfg.schema.available = vec![id.clone()];
    cfg.schema.active = id.clone();
    let m = EngineManager::with_store_override(&cfg, Some(&dir), Some(store), Some(dir.join("ov")));
    (m, Cleanup { id, dir })
}

fn texts(m: &EngineManager, input: &str) -> Vec<String> {
    m.convert(input, 20)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

/// 手动加词（1200）压过 `default_weight = 1100` 的主库——`ADD_WORD_WEIGHT` 的
/// 「略高于归一化中位」承诺在码表侧成立（此前只在英文侧有 `RANK_WEIGHT_MAX` 断言）。
#[test]
fn manual_add_word_1200_beats_main_dict_1100() {
    let (m, _g) = setup("manual", 1100, 1200, "自造词甲");
    assert_eq!(texts(&m, "uuif")[0], "自造词甲");
}

/// 自动造词晋升（1000）被 1100 的主库压过：**设计如此**。`PROMOTED_WEIGHT = 1000`
/// 对齐「系统词库归一化中位 1000」，不覆盖手动加词的更高权重；主库被用户调到 1100
/// 时晋升词沉到其后是预期行为。要反超请在词库管理里改该词权重（或主库降到 1000 以下）。
/// 改 `PROMOTED_WEIGHT` 或这条相对关系前先过本用例。
#[test]
fn promoted_word_1000_sits_below_main_dict_1100_by_design() {
    let (m, _g) = setup("promoted", 1100, 1000, "自造词乙");
    assert_eq!(
        texts(&m, "uuif"),
        vec![
            "立法".to_string(),
            "立法院".to_string(),
            "自造词乙".to_string()
        ]
    );
}

/// 同一现象的另一面：主库降到 800，晋升词（1000）照常反超——排序本身按权重工作，
/// 位次变化只来自两侧权重，别再往「层序 / 用户词被降权」方向排查。
#[test]
fn promoted_word_1000_beats_main_dict_800() {
    let (m, _g) = setup("low", 800, 1000, "自造词丙");
    assert_eq!(texts(&m, "uuif")[0], "自造词丙");
}

/// 用户词与主库词条**同文同码**：合成一条、权重取高者（`composite.rs` 同码取 max），
/// 用户层的归属标记保留（右键删除仍删得到用户词）。
#[test]
fn same_text_same_code_takes_higher_weight() {
    let (m, _g) = setup("same", 1100, 1200, "立法");
    assert_eq!(
        texts(&m, "uuif"),
        vec!["立法".to_string(), "立法院".to_string()]
    );
}
