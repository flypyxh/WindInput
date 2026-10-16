//! P2 迁移等价检查（设计 §11）：把一份 userdata.redb **副本**用 Store::open 打开（触发迁移到派生
//! 目录 `<redb>.user/`），逐方案逐条比对旧 `USER_WORDS` 表与迁移后读回的 (文本, 权重, 边界)，
//! 同码多词的相对先后也要一致。
//! 用法：cargo run --release -p wind-store --example p2_migrate_check -- <userdata.redb 副本>
//! 不带参数：合成一份 19 万词的旧库做演练（含边界未知的单字）。
fn main() -> anyhow::Result<()> {
    let p = match std::env::args().nth(1) {
        Some(p) => std::path::PathBuf::from(p),
        None => synth()?,
    };
    let _ = std::fs::remove_dir_all(wind_store::store::derived_state_dir(&p));
    let _ = std::fs::remove_dir_all(wind_store::store::derived_text_dir(&p));
    let t = std::time::Instant::now();
    let s = wind_store::Store::open(&p)?;
    println!("打开 + 迁移: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    assert!(s.migrated_p2(), "迁移未完成");
    let (mut bad, mut tolerated) = (0usize, 0usize);
    for (schema, rows) in s.legacy_user_rows()? {
        let mut by_code: std::collections::BTreeMap<String, Vec<(String, i32, u64)>> =
            Default::default();
        for r in &rows {
            by_code
                .entry(r.code.clone())
                .or_default()
                .push((r.text.clone(), r.weight, r.boundary));
        }
        for (code, want) in &by_code {
            let mut got = s.get_user_words(&schema, code)?;
            got.sort_by_key(|r| r.order);
            let got: Vec<(String, i32, u64)> = got
                .into_iter()
                .map(|r| (r.text, r.weight, r.boundary))
                .collect();
            // 边界：旧表单音节 0b1 只对单个汉字保真，其余退化为 0；反过来单个汉字旧边界 0
            // 读回 0b1（解析时补的单音节边界）。两者都属已知，单独计
            let ok = got.len() == want.len()
                && got.iter().zip(want).all(|(g, w)| {
                    g.0 == w.0
                        && g.1 == w.1
                        && (g.2 == w.2
                            || (w.2 == 0b1 && g.2 == 0)
                            || (w.2 == 0 && g.2 == 0b1 && g.0.chars().count() == 1))
                });
            tolerated += got
                .iter()
                .zip(want)
                .filter(|(g, w)| g.0 == w.0 && g.2 != w.2)
                .count();
            if !ok {
                bad += 1;
                if bad <= 5 {
                    println!("不一致 {schema}/{code}: 旧 {want:?} 新 {got:?}");
                }
            }
        }
        println!("{schema}: {} 条 / {} 个码", rows.len(), by_code.len());
    }
    println!("边界按已知规则变化的词: {tolerated}");
    println!("不一致的码: {bad}");
    if bad > 0 {
        std::process::exit(1);
    }
    println!("OK");
    Ok(())
}

/// 合成旧库：19 万词，2–4 音节轮换，带边界未知、同码多词、# 开头与含 tab 的文本。
fn synth() -> anyhow::Result<std::path::PathBuf> {
    let p = std::env::temp_dir().join("wind_p2_migrate_synth.redb");
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_dir_all(wind_store::store::derived_state_dir(&p));
    let _ = std::fs::remove_dir_all(wind_store::store::derived_text_dir(&p));
    let s = wind_store::Store::open(&p)?;
    let l = b"abcdefghijklmnopqrstuvwxyz";
    let t = std::time::Instant::now();
    for i in 0..190_000u32 {
        let syl = 2 + (i as usize % 3);
        let mut x = i as usize;
        let (mut code, mut b) = (String::new(), 0u64);
        for _ in 0..syl {
            b |= 1 << code.len();
            code.push(l[x % 26] as char);
            code.push('i');
            x /= 26;
        }
        let b = if i % 7 == 0 { 0 } else { b };
        let text = match i % 1000 {
            1 => format!("#词{i}"),
            2 => format!("词\t{i}"),
            _ => format!("词{i}"),
        };
        s.legacy_put_user("pinyin", &code, &text, (i % 500) as i32, b, i / 3)?;
    }
    // 单个汉字边界未知（旧库 0）：迁移后读回 0b1
    for (i, ch) in ["我", "你", "他"].iter().enumerate() {
        s.legacy_put_user("pinyin", "zz", ch, i as i32, 0, 190_000 + i as u32)?;
    }
    println!("合成旧库 19 万词: {:.1} s", t.elapsed().as_secs_f64());
    drop(s);
    let _ = std::fs::remove_dir_all(wind_store::store::derived_state_dir(&p));
    let _ = std::fs::remove_dir_all(wind_store::store::derived_text_dir(&p));
    Ok(p)
}
