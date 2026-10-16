//! P1 迁移等价检查（设计 §11）：把一份 userdata.redb **副本**用 Store::open 打开（触发迁移到
//! 派生状态目录），逐方案比对旧表行与引擎读回的结果。
//! 用法：cargo run --release -p wind-store --example p1_migrate_check -- <userdata.redb 副本>
fn main() -> anyhow::Result<()> {
    let p = std::env::args()
        .nth(1)
        .expect("参数：userdata.redb 副本路径");
    let _ = std::fs::remove_dir_all(wind_store::store::derived_state_dir(std::path::Path::new(
        &p,
    )));
    let t = std::time::Instant::now();
    let s = wind_store::Store::open(&p)?;
    println!("打开 + 迁移: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    for schema in s.list_data_schemas()? {
        let (temp, freq, drafts) = (
            s.count_temp_words(&schema)?,
            s.list_freq_paged(&schema, "", 0, 0)?.1,
            s.count_drafts(&schema)?,
        );
        let legacy = s.legacy_counts(&schema)?;
        println!(
            "{schema}: temp {temp}/{} freq {freq}/{} drafts {drafts}/{}",
            legacy.0, legacy.1, legacy.2
        );
        assert_eq!((temp, freq, drafts), legacy, "{schema} 条数不一致");
    }
    println!("OK");
    Ok(())
}
