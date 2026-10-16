//! 用户词 API 与 wind-store/src/user_words.rs 的行为对拍（文本即真源：每条末尾折回、重开再验）。
use std::path::PathBuf;
use std::sync::Arc;
use wind_userdata::engine::{Dirs, TextBinding, UserData};
use wind_userdata::layer::UserDataUserLayer;
use wind_userdata::record::Target;
use wind_userdata::runtime::{Runtime, USER_CODE_MAX_CHARS, USER_TEXT_MAX_CHARS, UserWord};

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("wind_rtu_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Env { dir }
    }

    /// 打开引擎并把 `schemas` 各自绑定到 `user/<schema>/user.dict.yaml`。
    fn open(&self, schemas: &[&str]) -> Runtime {
        let ud = Arc::new(
            UserData::open(Dirs {
                state_dir: self.dir.join("state"),
                dir_id: "dir".into(),
            })
            .unwrap(),
        );
        for s in schemas {
            ud.bind_text(
                Target::User,
                s,
                TextBinding {
                    path: self.dir.join("user").join(s).join("user.dict.yaml"),
                    name: format!("{s}.user"),
                    lowercase: false,
                    syllable_codes: *s == "py",
                    format: wind_userdata::engine::TextFormat::Words,
                },
            )
            .unwrap();
        }
        Runtime::new(ud)
    }
}

fn flush(r: &Runtime, schema: &str) {
    r.data().fold(Target::User, schema).unwrap();
}

/// 对照 test_add_get_user_word：重复添加取 max 权重。
#[test]
fn add_get_and_max_weight() {
    let env = Env::new("addget");
    let r = env.open(&["wb"]);
    r.add_user_word("wb", "a", "工", 100, 0).unwrap();
    r.add_user_word("wb", "a", "戈", 50, 0).unwrap();
    r.add_user_word("wb", "a", "工", 200, 0).unwrap();
    r.add_user_word("wb", "a", "工", 10, 0).unwrap();
    flush(&r, "wb");
    drop(r);
    let r = env.open(&["wb"]);
    let mut got: Vec<(String, i32)> = r
        .user_words("wb", "a")
        .into_iter()
        .map(|w| (w.text, w.weight))
        .collect();
    got.sort();
    assert_eq!(got, vec![("工".into(), 200), ("戈".into(), 50)]);
}

/// 对照 add_user_word_rejects_text_over_max_chars / _code_over_max_chars。
#[test]
fn rejects_overlong_text_and_code() {
    let env = Env::new("maxlen");
    let r = env.open(&["py"]);
    r.add_user_word("py", "abc", &"汉".repeat(USER_TEXT_MAX_CHARS), 100, 0)
        .expect("恰好等于上限应写入成功");
    let err = r
        .add_user_word("py", "abd", &"汉".repeat(USER_TEXT_MAX_CHARS + 1), 100, 0)
        .expect_err("超过上限应被拒绝");
    assert!(err.to_string().contains("过长"), "{err}");
    assert!(r.user_words("py", "abd").is_empty());
    let long_code = "a".repeat(USER_CODE_MAX_CHARS + 1);
    assert!(r.add_user_word("py", &long_code, "你好", 1, 0).is_err());
    assert!(
        r.add_user_word("py", &"a".repeat(USER_CODE_MAX_CHARS), "你好", 1, 0)
            .is_ok()
    );
}

/// 对照 test_prefix_remove_update。
#[test]
fn prefix_remove_update() {
    let env = Env::new("prefix");
    let r = env.open(&["wb"]);
    r.add_user_word("wb", "ab", "阿", 10, 0).unwrap();
    r.add_user_word("wb", "abc", "啊", 20, 0).unwrap();
    r.add_user_word("wb", "x", "西", 30, 0).unwrap();
    flush(&r, "wb");
    let mut pre = Vec::new();
    r.for_each_user_word("wb", "ab", &mut |w| {
        pre.push(w.code.clone());
        true
    });
    assert_eq!(pre, vec!["ab", "abc"]);
    assert!(r.update_user_word_weight("wb", "ab", "阿", 99).unwrap());
    assert!(
        !r.update_user_word_weight("wb", "ab", "缺", 1).unwrap(),
        "不存在不新建"
    );
    r.remove_user_word("wb", "x", "西").unwrap();
    r.remove_user_word("wb", "x", "无此词").unwrap();
    flush(&r, "wb");
    drop(r);
    let r = env.open(&["wb"]);
    assert_eq!(r.user_word("wb", "ab", "阿").unwrap().weight, 99);
    assert!(r.user_words("wb", "ab").iter().all(|w| w.text != "缺"));
    assert!(r.user_word("wb", "x", "西").is_none());
    assert_eq!(r.count_user_words("wb"), 2);
}

/// 对照 clear_user_words_only_target_schema。
#[test]
fn clear_only_target_schema() {
    let env = Env::new("clear");
    let r = env.open(&["wb", "py"]);
    r.add_user_word("wb", "a", "工", 1, 0).unwrap();
    r.add_user_word("wb", "b", "了", 1, 0).unwrap();
    r.add_user_word("py", "ni", "你", 1, 0b1).unwrap();
    assert_eq!(r.clear_user_words("wb").unwrap(), 2);
    assert_eq!(r.count_user_words("wb"), 0);
    assert_eq!(r.count_user_words("py"), 1);
    drop(r);
    let r = env.open(&["wb", "py"]);
    assert_eq!(r.count_user_words("wb"), 0, "清空落进了文本");
}

fn row(code: &str, text: &str, weight: i32) -> UserWord {
    UserWord {
        code: code.into(),
        text: text.into(),
        weight,
        boundary: 0,
        order: 0,
    }
}

/// 对照 import_user_words_merges_max_weight + reimport_keeps_existing_order。
#[test]
fn import_merges_max_and_keeps_existing_order() {
    let env = Env::new("import");
    let r = env.open(&["wb"]);
    r.add_user_word("wb", "a", "工", 100, 0).unwrap();
    assert_eq!(
        r.import_user_rows(
            "wb",
            &[row("a", "工", 30), row("abc", "甲", 5), row("abc", "乙", 5)]
        )
        .unwrap(),
        wind_userdata::runtime::UserImportCounts {
            added: 2,
            updated: 0,
            unchanged: 1
        }
    );
    assert_eq!(
        r.user_word("wb", "a", "工").unwrap().weight,
        100,
        "Merge 取 max"
    );
    let order_of = |r: &Runtime| -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = r
            .user_words("wb", "abc")
            .into_iter()
            .map(|w| (w.text, w.order))
            .collect();
        v.sort();
        v
    };
    let before = order_of(&r);
    assert!(
        before.iter().all(|(_, o)| *o > 0),
        "导入即落文本，order = 行号"
    );
    r.import_user_rows("wb", &[row("abc", "甲", 900), row("abc", "乙", 900)])
        .unwrap();
    assert!(r.user_words("wb", "abc").iter().all(|w| w.weight == 900));
    assert_eq!(order_of(&r), before, "重复导入不改变既有词条的行位置");
}

/// 对照 no_boundary_words_are_recalled_by_first_char_only（Review Focus 5）。
#[test]
fn user_layer_recalls_unknown_boundary_by_first_char() {
    use wind_dict::DictLayer;
    let env = Env::new("abbrev");
    let r = env.open(&["py"]);
    r.add_user_word("py", "xianning", "西安宁", 100, 0).unwrap();
    r.add_user_word("py", "nihao", "你好", 100, 0b101).unwrap();
    flush(&r, "py");
    drop(r);
    let r = env.open(&["py"]);
    let l = UserDataUserLayer::new(r.data().clone(), "py");
    let texts =
        |ab: &str| -> Vec<String> { l.search_abbrev(ab, 0).into_iter().map(|c| c.text).collect() };
    assert!(
        texts("xan").contains(&"西安宁".to_string()),
        "无边界词经首字符兜底组召回"
    );
    assert_eq!(texts("nh"), vec!["你好"]);
    assert!(texts("zg").is_empty());
    assert!(
        r.user_word("py", "xianning", "西安宁").unwrap().boundary == 0,
        "写回再解析仍是 0"
    );
}

/// 新词折回前后同码候选的入库顺序一致（新词排在已有词之后）。
#[test]
fn natural_order_is_stable_across_fold() {
    use wind_dict::DictLayer;
    let env = Env::new("norder");
    let r = env.open(&["py"]);
    r.add_user_word("py", "ni", "你", 1, 0b1).unwrap();
    r.add_user_word("py", "ni", "泥", 1, 0b1).unwrap();
    flush(&r, "py");
    r.add_user_word("py", "ni", "尼", 1, 0b1).unwrap();
    let l = UserDataUserLayer::new(r.data().clone(), "py");
    let order = |l: &UserDataUserLayer| -> Vec<String> {
        let mut v: Vec<(i32, String)> = l
            .search("ni", 0)
            .into_iter()
            .map(|c| (c.natural_order, c.text))
            .collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    };
    let pre = order(&l);
    assert_eq!(pre.last().map(String::as_str), Some("尼"), "新词排最后");
    flush(&r, "py");
    assert_eq!(order(&l), pre);
}

/// 补齐的音节边界写进文本：折回后仍在，再加同一词不再产生操作。
#[test]
fn filled_boundary_is_persisted() {
    let env = Env::new("bfill");
    let r = env.open(&["py"]);
    r.add_user_word("py", "nihao", "你好", 5, 0).unwrap();
    flush(&r, "py");
    r.add_user_word("py", "nihao", "你好", 5, 0b101).unwrap();
    flush(&r, "py");
    assert_eq!(r.user_word("py", "nihao", "你好").unwrap().boundary, 0b101);
    r.add_user_word("py", "nihao", "你好", 5, 0b101).unwrap();
    assert_eq!(r.data().pending(Target::User, "py"), 0);
}

/// 导入分类：新键 added；权重更大或补边界 updated；其余 unchanged。预览判据与落盘一致。
#[test]
fn import_classifies_rows_like_wind_store() {
    use wind_userdata::runtime::{RowEffect, UserImportCounts};
    let env = Env::new("classify");
    let r = env.open(&["py"]);
    r.add_user_word("py", "nihao", "你好", 100, 0).unwrap();
    r.add_user_word("py", "ta", "他", 50, 0b1).unwrap();
    let rows = [
        row("nihao", "你好", 10),
        UserWord {
            boundary: 0b101,
            ..row("nihao", "你好", 10)
        },
        row("ta", "他", 80),
        row("xin", "新", 1),
        row("xin", "新", 5),
    ];
    assert_eq!(r.user_row_effect("py", &rows[0]), RowEffect::Unchanged);
    assert_eq!(r.user_row_effect("py", &rows[1]), RowEffect::Updated);
    assert_eq!(r.user_row_effect("py", &rows[3]), RowEffect::Added);
    let c = r.import_user_rows("py", &rows[1..]).unwrap();
    assert_eq!(
        c,
        UserImportCounts {
            added: 1,
            updated: 3,
            unchanged: 0
        },
        "同批第二次出现的新键按「已存在」判定：权重更大 ⇒ updated"
    );
    assert_eq!(r.user_word("py", "nihao", "你好").unwrap().boundary, 0b101);
    assert_eq!(r.user_word("py", "xin", "新").unwrap().weight, 5);
}

/// 审查 I1：大批导入线性——6 万条一次导入（含折回进文本）不能是平方级。
#[test]
fn large_import_is_not_quadratic() {
    let env = Env::new("bigimport");
    let r = env.open(&["wb"]);
    let rows: Vec<UserWord> = (0..60_000)
        .map(|i| row(&format!("c{i:05}"), &format!("词{i:05}"), 1))
        .collect();
    let t = std::time::Instant::now();
    let c = r.import_user_rows("wb", &rows).unwrap();
    assert!(
        t.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(c.added, 60_000);
    assert_eq!(r.count_user_words("wb"), 60_000);
}

/// 审查 I3 / I4：文本隔离（头部坏）时导入整体拒绝、不留暂存；清空（用户显式的整体替换）照样成功。
#[test]
fn isolated_text_rejects_import_but_clear_still_replaces() {
    let env = Env::new("isolated");
    let r = env.open(&["wb"]);
    r.add_user_word("wb", "a", "工", 1, 0).unwrap();
    flush(&r, "wb");
    let f = env.dir.join("user").join("wb").join("user.dict.yaml");
    std::fs::write(&f, "---\nname: [\n...\n").unwrap();
    r.add_user_word("wb", "b", "了", 1, 0).unwrap(); // 隔离期间的写入：留在 delta
    assert!(r.import_user_rows("wb", &[row("c", "以", 1)]).is_err());
    assert!(r.user_word("wb", "c", "以").is_none(), "导入失败不留暂存");
    r.clear_user_words("wb").unwrap();
    assert_eq!(r.count_user_words("wb"), 0);
    assert!(
        !std::fs::read_to_string(&f).unwrap().contains("name: ["),
        "整文件替换"
    );
    drop(r);
    let r = env.open(&["wb"]);
    assert_eq!(r.count_user_words("wb"), 0, "隔离期间暂存的写入不复活");
}

/// 审查 I2：方案 id 是路径分量，不能越出状态 / 文本目录。
#[test]
fn unsafe_schema_ids_are_rejected() {
    let env = Env::new("schemaid");
    let r = env.open(&[]);
    for bad in ["../evil", "a/b", "a\\b", "..", ".", "", "x:y", " x", "x."] {
        assert!(r.add_user_word(bad, "a", "工", 1, 0).is_err(), "{bad:?}");
    }
    assert!(!env.dir.join("evil").exists());
    assert!(r.add_user_word("wubi86_pinyin", "a", "工", 1, 0).is_ok());
    assert!(r.add_user_word("我的方案", "a", "工", 1, 0).is_ok());
}
