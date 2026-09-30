//! 开发期生成器：Linux 托盘图标的**种子**（`wind_linux/data/icons/hicolor/*/apps/windinput-{zh,en,caps}.*`）
//! 与肉眼验收用的放大预览。
//!
//! ```text
//! cd wind_input
//! cargo run -p wind-ui --features linux-host --example gen_tray_icons -- \
//!     --font NotoSansCJKsc-Bold.otf [--out <hicolor 目录>] [--preview <目录>]
//! ```
//!
//! 托盘图标的主路径是服务端按模式主字**运行时渲染**（[`wind_ui::tray_icon`]，同 Windows 语言栏）；
//! 这里产出的三张是随包安装的种子：服务还没写出运行时图标（刚装好、服务没起来、用户图标目录
//! 不可写）时 addon 退回它们。渲染走同一个 [`wind_ui::tray_icon::render`]，种子与运行时图标同一个样子。
//!
//! 种子入库，构建与打包都不跑本生成器。字体用 Noto Sans CJK SC **Bold**
//! （`github.com/notofonts/noto-cjk` 的 `Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Bold.otf`），只在
//! 生成时用一次，SVG 里是路径。预览（`--preview`）另画几个运行时才会出现的主字（方案标签、
//! 自定义标签「虎」、两字母「En」），用来看没有手绘点阵的字在 16px 下的样子。

#[cfg(all(target_os = "linux", feature = "linux-host"))]
fn main() {
    if let Err(e) = tray::main() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "linux", feature = "linux-host")))]
fn main() {
    eprintln!("gen_tray_icons 只在 Linux 上以 --features linux-host 运行");
    std::process::exit(2);
}

#[cfg(all(target_os = "linux", feature = "linux-host"))]
mod tray {
    use ab_glyph_rasterizer::Point;
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use ttf_parser::Face;
    use wind_ui::tray_icon::{self, Icon, PathSink, Variant};

    /// 种子：图标名 → 状态 + 主字。**名字是与 addon 的契约**（`HostUi.cpp` 的 `seedIconName`）。
    const SEEDS: &[(&str, Variant, &str)] = &[
        ("windinput-zh", Variant::Chinese, "中"),
        ("windinput-en", Variant::English, "英"),
        ("windinput-caps", Variant::Caps, "A"),
    ];

    /// 只进预览、不落盘的样例：运行时才会出现的主字。
    const PREVIEW_ONLY: &[(Variant, &str)] = &[
        (Variant::Chinese, "拼"),
        (Variant::Chinese, "五"),
        (Variant::Chinese, "笔"),
        (Variant::Chinese, "双"),
        (Variant::Chinese, "虎"),
        (Variant::English, "En"),
    ];

    pub fn main() -> Result<(), String> {
        let mut font_path: Option<PathBuf> = None;
        let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../wind_linux/data/icons/hicolor");
        let mut preview: Option<PathBuf> = None;
        let mut preview_from: Option<PathBuf> = None;
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            let mut val = || args.next().ok_or_else(|| format!("{a} 缺参数"));
            match a.as_str() {
                "--font" => font_path = Some(val()?.into()),
                "--out" => out = val()?.into(),
                "--preview" => preview = Some(val()?.into()),
                "--preview-from" => preview_from = Some(val()?.into()),
                _ => return Err(format!("未知参数 {a}")),
            }
        }
        if let Some(root) = preview_from {
            let dir = preview.ok_or("--preview-from 要配 --preview <输出目录>")?;
            return preview_runtime(&root, &dir);
        }
        let font_path = font_path.ok_or("需要 --font <NotoSansCJKsc-Bold.otf>")?;
        let data =
            std::fs::read(&font_path).map_err(|e| format!("读 {}: {e}", font_path.display()))?;
        let face = Face::parse(&data, 0).map_err(|e| format!("解析字体: {e}"))?;

        let mut rendered: Vec<(String, usize, Icon)> = Vec::new();
        let mut svgs: Vec<String> = Vec::new();
        for &(name, variant, label) in SEEDS {
            for &n in tray_icon::SIZES {
                let icon = tray_icon::render(&face, variant, label, n)?;
                tray_icon::write_atomic(
                    &out.join(format!("{n}x{n}/apps/{name}.png")),
                    &tray_icon::encode_png(&icon)?,
                )?;
                rendered.push((name.to_string(), n, icon));
            }
            let svg = svg(&face, variant, label)?;
            tray_icon::write_atomic(
                &out.join(format!("scalable/apps/{name}.svg")),
                svg.as_bytes(),
            )?;
            svgs.push(svg);
        }
        println!("已写入 {}", out.canonicalize().unwrap_or(out).display());

        if let Some(dir) = preview {
            for &(variant, label) in PREVIEW_ONLY {
                let name = tray_icon::icon_name(variant, label);
                for &n in tray_icon::SIZES {
                    rendered.push((
                        name.clone(),
                        n,
                        tray_icon::render(&face, variant, label, n)?,
                    ));
                }
            }
            write_previews(&dir, &rendered, &svgs)?;
            println!("预览图在 {}", dir.display());
        }
        Ok(())
    }

    /// 给一个 hicolor 根里服务端写出的运行时图标（`windinput-lbl-*`）出同样的放大 / 面板预览：
    /// 真机上验「用那台机器的字体渲染出来是什么样」。
    fn preview_runtime(root: &Path, dir: &Path) -> Result<(), String> {
        let apps = root.join("16x16/apps");
        let mut names: Vec<String> = std::fs::read_dir(&apps)
            .map_err(|e| format!("读 {}: {e}", apps.display()))?
            .flatten()
            .filter_map(|f| f.file_name().to_str().map(str::to_string))
            .filter_map(|f| f.strip_suffix(".png").map(str::to_string))
            .filter(|n| n.starts_with(tray_icon::DYNAMIC_PREFIX))
            .collect();
        names.sort();
        let mut rendered = Vec::new();
        for name in names {
            for &n in tray_icon::SIZES {
                let p = root.join(format!("{n}x{n}/apps/{name}.png"));
                let img = image::open(&p)
                    .map_err(|e| format!("读 {}: {e}", p.display()))?
                    .to_rgba8();
                rendered.push((
                    name.clone(),
                    n,
                    Icon {
                        n,
                        rgba: img.into_raw(),
                    },
                ));
            }
        }
        write_previews(dir, &rendered, &[])?;
        println!("预览图在 {}", dir.display());
        Ok(())
    }

    /// 字形轮廓 → SVG path 的 `d`。
    struct SvgPath(String);

    fn num(v: f32) -> String {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    impl SvgPath {
        fn pt(&mut self, p: Point) {
            let _ = write!(self.0, "{} {}", num(p.x), num(p.y));
        }
    }

    impl PathSink for SvgPath {
        fn begin(&mut self, p: Point) {
            self.0.push('M');
            self.pt(p);
        }
        fn line(&mut self, _a: Point, b: Point) {
            self.0.push('L');
            self.pt(b);
        }
        fn quad(&mut self, _a: Point, c: Point, b: Point) {
            self.0.push('Q');
            self.pt(c);
            self.0.push(' ');
            self.pt(b);
        }
        fn cubic(&mut self, _a: Point, c1: Point, c2: Point, b: Point) {
            self.0.push('C');
            self.pt(c1);
            self.0.push(' ');
            self.pt(c2);
            self.0.push(' ');
            self.pt(b);
        }
        fn end(&mut self) {
            self.0.push('Z');
        }
    }

    /// 可缩放版：几何与 64px 档相同（同一套 em 比例与墨迹居中，不做亚像素对格——矢量没有像素格）。
    fn svg(face: &Face, variant: Variant, label: &str) -> Result<String, String> {
        const N: usize = 64;
        let p = tray_icon::place(face, label, N, 1.0, (0.0, 0.0))?;
        let mut path = SvgPath(String::new());
        p.outline(face, &mut path);
        let r = num(N as f32 * tray_icon::RADIUS);
        let [cr, cg, cb] = variant.color();
        Ok(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{N}\" height=\"{N}\" viewBox=\"0 0 {N} {N}\">\n  \
             <rect width=\"{N}\" height=\"{N}\" rx=\"{r}\" ry=\"{r}\" fill=\"#{cr:02X}{cg:02X}{cb:02X}\"/>\n  \
             <path d=\"{}\" fill=\"#fff\"/>\n</svg>\n",
            path.0
        ))
    }

    /// 在 `sheet` 的 `(x0, y0)` 处以 `k` 倍最近邻贴一张图标，按 alpha 合到底色上。
    fn blit(sheet: &mut image::RgbaImage, icon: &Icon, x0: u32, y0: u32, k: u32) {
        for y in 0..icon.n {
            for x in 0..icon.n {
                let i = (y * icon.n + x) * 4;
                let a = icon.rgba[i + 3] as f32 / 255.0;
                for yy in 0..k {
                    for xx in 0..k {
                        let (px, py) = (x0 + x as u32 * k + xx, y0 + y as u32 * k + yy);
                        let under = *sheet.get_pixel(px, py);
                        let mut c = under;
                        for ch in 0..3 {
                            c[ch] = (icon.rgba[i + ch] as f32 * a + under[ch] as f32 * (1.0 - a))
                                .round() as u8;
                        }
                        sheet.put_pixel(px, py, c);
                    }
                }
            }
        }
    }

    /// 肉眼验收用：`zoom-<N>.png`（各图标在该尺寸下最近邻放大 8 倍，棋盘格底看得出透明边）、
    /// `panel-1x.png`（浅 / 深两条面板上按 1× 并排，另附一份 3 倍最近邻放大的 `panel-3x.png`）、
    /// `svg-128.png`（SVG 经 resvg 按 128px 栅格，验路径与底板没画歪）。
    fn write_previews(
        dir: &Path,
        rendered: &[(String, usize, Icon)],
        svgs: &[String],
    ) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {}: {e}", dir.display()))?;
        let mut names: Vec<&str> = Vec::new();
        for r in rendered {
            if !names.contains(&r.0.as_str()) {
                names.push(&r.0);
            }
        }
        let find = |name: &str, n: usize| {
            rendered
                .iter()
                .find(|r| r.0 == name && r.1 == n)
                .map(|r| &r.2)
                .expect("每个名字每个尺寸都渲染过")
        };
        for &n in &[16usize, 22, 24, 32] {
            // 放大后每格约 200px，一行 4 格：图片查看器缩小整张图时像素格仍分得清。
            let k = (200 / n as u32).max(4);
            const COLS: u32 = 4;
            let cell = n as u32 * k + 16;
            let rows = (names.len() as u32).div_ceil(COLS);
            let mut sheet = image::RgbaImage::new(cell * COLS, cell * rows);
            for (x, y, p) in sheet.enumerate_pixels_mut() {
                let dark = ((x / 16) + (y / 16)) % 2 == 0;
                *p = if dark {
                    image::Rgba([200, 200, 200, 255])
                } else {
                    image::Rgba([235, 235, 235, 255])
                };
            }
            for (i, name) in names.iter().enumerate() {
                let (cx, cy) = (i as u32 % COLS, i as u32 / COLS);
                blit(&mut sheet, find(name, n), cx * cell + 8, cy * cell + 8, k);
            }
            sheet
                .save(dir.join(format!("zoom-{n}.png")))
                .map_err(|e| format!("写预览: {e}"))?;
        }
        // 1× 面板模拟：每行一个尺寸（16/22/24），左半浅色面板、右半深色面板。
        let rows = [16usize, 22, 24];
        let gap = 8u32;
        let half = names.len() as u32 * (24 + gap) + gap;
        let row_h = 24 + 2 * gap;
        let mut panel = image::RgbaImage::new(half * 2, row_h * rows.len() as u32);
        for (x, _, p) in panel.enumerate_pixels_mut() {
            *p = if x < half {
                image::Rgba([0xF2, 0xF2, 0xF2, 255])
            } else {
                image::Rgba([0x2B, 0x2B, 0x2E, 255])
            };
        }
        for (ri, &n) in rows.iter().enumerate() {
            for side in 0..2u32 {
                for (i, name) in names.iter().enumerate() {
                    let x = side * half + gap + i as u32 * (24 + gap) + (24 - n as u32) / 2;
                    let y = ri as u32 * row_h + gap + (24 - n as u32) / 2;
                    blit(&mut panel, find(name, n), x, y, 1);
                }
            }
        }
        panel
            .save(dir.join("panel-1x.png"))
            .map_err(|e| format!("写预览: {e}"))?;
        let big = image::imageops::resize(
            &panel,
            panel.width() * 3,
            panel.height() * 3,
            image::imageops::FilterType::Nearest,
        );
        big.save(dir.join("panel-3x.png"))
            .map_err(|e| format!("写预览: {e}"))?;

        if svgs.is_empty() {
            return Ok(());
        }
        const S: u32 = 128;
        let mut sheet = image::RgbaImage::from_pixel(
            (S + 8) * svgs.len() as u32 + 8,
            S + 16,
            image::Rgba([0xF2, 0xF2, 0xF2, 255]),
        );
        for (i, svg) in svgs.iter().enumerate() {
            let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
                .map_err(|e| format!("解析 SVG: {e}"))?;
            let mut pm = resvg::tiny_skia::Pixmap::new(S, S).ok_or("建 pixmap")?;
            let k = S as f32 / tree.size().width();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(k, k),
                &mut pm.as_mut(),
            );
            // tiny-skia 是预乘 RGBA，blit 要非预乘。
            let mut rgba = Vec::with_capacity((S * S * 4) as usize);
            for px in pm.pixels() {
                let c = px.demultiply();
                rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
            }
            let icon = Icon {
                n: S as usize,
                rgba,
            };
            blit(&mut sheet, &icon, 8 + i as u32 * (S + 8), 8, 1);
        }
        sheet
            .save(dir.join("svg-128.png"))
            .map_err(|e| format!("写预览: {e}"))?;
        Ok(())
    }
}
