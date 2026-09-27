//! 字体 × 字重的**真机**探针：候选窗按 `ui.font.family` / `ui.font.weight` 排字时，
//! DirectWrite 实际用了哪个 face、有没有做粗体模拟、墨量（可见粗细）变了多少。
//!
//! 走的是候选窗同一条路径（看板 A2-1）：
//! 1. 字体名过 `font_resolve::resolve_font_name`（`family_exists` / `find_face` 由真实
//!    `TextRenderer` 提供，与 `CandidateWindow::resolve_font` 同源）；
//! 2. `set_font_family(解析出的 family)` + `set_default_weight(用户字重非 0 ? 用户字重 : 名字字重)`，
//!    与 `CandidateWindow::apply_default_weight` 同一优先级；
//! 3. 排版用 `TextRenderer::probe_glyph_runs`（即 `draw` 的同一个 `create_layout`），
//!    墨量用 `draw` 真画到白底位图上数出来。
//!
//! 不开窗口、不建管道、不碰服务：SSH（session 0）里直接跑。
//!
//! ```text
//! cargo xwin build --release --target x86_64-pc-windows-msvc -p wind-ui --example font_weight_probe
//! font_weight_probe.exe [--fonts 名1,名2,…] [--weights 100,400,…] [--text 文本]
//! ```
//!
//! 输出 TSV，列：请求字体、请求字重、解析来源、解析 family、名字字重、生效默认字重、
//! 各段实际字体（family/face/weight/模拟，`|` 分隔）、墨量。墨量 = Σ(255 − 绿通道)/255，
//! 约等于被墨覆盖的像素数；同一字体不同字重的墨量相同 ⇒ 画面上看不出区别。

#[cfg(windows)]
fn main() {
    use wind_ui::text::dwrite::{TextRenderer, TextStyle};
    use wind_ui::text::font_resolve::resolve_font_name;

    const DEFAULT_FONTS: &[&str] = &[
        "汉仪中圆B5",
        "黑体",
        "宋体",
        "微软雅黑",
        "等线",
        "Noto Sans SC",
        "Noto Serif SC",
        "Source Han Serif SC",
        "思源宋体 SemiBold",
    ];
    const DEFAULT_WEIGHTS: &[i32] = &[100, 300, 400, 500, 600, 700, 900];
    const SIZE: f32 = 32.0;

    let mut fonts: Vec<String> = DEFAULT_FONTS.iter().map(|s| s.to_string()).collect();
    let mut weights: Vec<i32> = DEFAULT_WEIGHTS.to_vec();
    let mut text = "字重测试永和".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().unwrap_or_default();
        match a.as_str() {
            "--fonts" => fonts = v.split(',').map(|s| s.trim().to_string()).collect(),
            "--weights" => weights = v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
            "--text" => text = v,
            _ => {
                eprintln!("未知参数 {a}；用法见源码头注释");
                std::process::exit(2);
            }
        }
    }

    println!("请求字体\t请求字重\t解析来源\t解析family\t名字字重\t生效字重\t实际字体\t墨量");
    for name in &fonts {
        for &w in &weights {
            let mut tr = match TextRenderer::new("Microsoft YaHei", SIZE) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("TextRenderer::new 失败：{e}");
                    std::process::exit(1);
                }
            };
            let r = resolve_font_name(name, |n| tr.family_exists(n), |n| tr.find_face(n));
            tr.set_font_family(&r.family);
            let eff = if w > 0 { w.clamp(1, 950) } else { r.weight };
            tr.set_default_weight(eff);
            let ts = TextStyle::new(SIZE);
            let runs = match tr.probe_glyph_runs(&text, &ts) {
                Ok(runs) => runs
                    .iter()
                    .map(|g| {
                        let sim = match (g.bold_sim, g.oblique_sim) {
                            (true, true) => "+粗斜模拟",
                            (true, false) => "+粗模拟",
                            (false, true) => "+斜模拟",
                            (false, false) => "",
                        };
                        format!("{}/{}/{}{}×{}", g.family, g.face, g.weight, sim, g.glyphs)
                    })
                    .collect::<Vec<_>>()
                    .join(" | "),
                Err(e) => format!("失败：{e}"),
            };
            let (bw, bh) = (400u32, 64u32);
            let mut buf = vec![255u8; (bw * bh * 4) as usize];
            let ink = match tr.draw(&mut buf, bw, bh, 4.0, 8.0, &text, &ts, [0, 0, 0, 255]) {
                Ok(()) => {
                    let s: u64 = buf.chunks_exact(4).map(|p| 255 - p[1] as u64).sum();
                    format!("{:.0}", s as f64 / 255.0)
                }
                Err(e) => format!("失败：{e}"),
            };
            println!(
                "{name}\t{w}\t{:?}\t{}\t{}\t{eff}\t{runs}\t{ink}",
                r.source, r.family, r.weight
            );
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("font_weight_probe 只在 Windows 上有意义（DirectWrite）");
}
