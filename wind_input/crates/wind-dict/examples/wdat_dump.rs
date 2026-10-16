//! 把一份 wdat 的主表与简拼区逐条导出为 TSV（排错 / 对拍用）。
//! 用法：`cargo run -p wind-dict --example wdat_dump -- <file.wdat> [abbrev]`
use wind_dict::datformat::WdatReader;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("wdat path");
    let mode = std::env::args().nth(2).unwrap_or_default();
    let r = WdatReader::open(&path)?;
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    use std::io::Write;
    if mode == "abbrev" {
        // 简拼区：按游标无法遍历（独立 DAT），用 for_each 不可得；这里穷举 a-z 组合不现实，
        // 改为从主表反推：对每个 code 的多音节条目取首字母组合，再查简拼区并打印命中。
        let mut seen = std::collections::BTreeSet::new();
        for (code, leaf) in r.codes() {
            for (_, _, _, b) in r.leaf_entries(leaf) {
                if b.count_ones() >= 2 {
                    let ab: String = code
                        .bytes()
                        .enumerate()
                        .take(64)
                        .filter(|(i, _)| b & (1u64 << i) != 0)
                        .map(|(_, c)| c as char)
                        .collect();
                    seen.insert(ab);
                }
            }
        }
        for ab in seen {
            for e in r.search_abbrev(&ab, 100_000) {
                writeln!(out, "{ab}\t{}\t{}\t{}", e.text, e.weight, e.order)?;
            }
        }
    } else {
        for (code, leaf) in r.codes() {
            for (t, w, o, b) in r.leaf_entries(leaf) {
                writeln!(out, "{code}\t{t}\t{w}\t{o}\t{b:#x}")?;
            }
        }
    }
    Ok(())
}
