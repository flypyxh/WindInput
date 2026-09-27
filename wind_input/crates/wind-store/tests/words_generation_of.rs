//! 按方案的写代次：一个方案的写入不该让别的方案的内存索引过期（见 `Store::words_generation_of`）。
use wind_store::Store;

fn open(tag: &str) -> Store {
    let p = std::env::temp_dir().join(format!(
        "wind_store_gen_of_{tag}_{}.redb",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    Store::open(&p).unwrap()
}

/// ★ 在拼音里造词不能让五笔的索引过期——否则辅助码引用五笔时，每次上屏都要后台重扫五笔用户词。
#[test]
fn write_in_one_schema_does_not_touch_another() {
    let s = open("iso");
    let wb0 = s.words_generation_of("wb");
    let py0 = s.words_generation_of("pinyin");
    s.add_user_word("pinyin", "nihao", "你好", 0, 0).unwrap();
    assert_eq!(
        s.words_generation_of("wb"),
        wb0,
        "拼音写入不该推进五笔的代次"
    );
    assert_ne!(
        s.words_generation_of("pinyin"),
        py0,
        "拼音自己的代次必须前进"
    );
    s.learn_temp_word("wb", "aaaa", "工", 0, 0).unwrap();
    assert_ne!(
        s.words_generation_of("wb"),
        wb0,
        "临时词写入同样推进本方案代次"
    );
}

/// 备份还原换了整个文件，哪个方案的索引都不可信。
#[test]
fn resume_invalidates_every_schema() {
    let s = open("resume");
    let wb0 = s.words_generation_of("wb");
    let py0 = s.words_generation_of("pinyin");
    s.pause().unwrap();
    s.resume().unwrap();
    assert_ne!(s.words_generation_of("wb"), wb0);
    assert_ne!(s.words_generation_of("pinyin"), py0);
}

/// 全局代次保持原语义：任何方案的结构写入都 +1（词语联想仍靠它）。
#[test]
fn global_generation_still_moves_on_any_write() {
    let s = open("global");
    let g0 = s.words_generation();
    s.add_user_word("wb", "aaaa", "工", 0, 0).unwrap();
    assert_ne!(s.words_generation(), g0);
}
