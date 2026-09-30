//! 开发期生成器：Linux 托盘 / 面板的模式图标（`wind_linux/data/icons/hicolor/*/apps/windinput-*.{png,svg}`）。
//!
//! ```text
//! cd wind_input
//! cargo run -p wind-ui --features linux-host --example gen_tray_icons -- \
//!     --font NotoSansCJKsc-Bold.otf [--out <hicolor 目录>] [--preview <目录>]
//! ```
//!
//! 产物入库，构建与打包都不跑它；只在改图标时跑一次。字体用 Noto Sans CJK SC **Bold**
//! （`github.com/notofonts/noto-cjk` 的 `Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Bold.otf`）：
//! 字形只在生成时用一次，SVG 里是路径，目标机装没装这款字体无关。
//!
//! 为什么是自己合成而不是直接调 [`wind_ui::langbar_icon::IconRenderer`]：那套是给 Windows 语言栏
//! 调的——透明底、任务栏明暗两套字色、Light 字重、主字字号 = 边长 − 2。托盘图标要的是「带底色
//! 的圆角方块 + 白字」，在深浅面板上都立得住，这是另一种图形。借用的是它的两条**策略**：
//! - **按墨迹盒居中**，不按行盒（`render_glyph_mask` 的「版面盒 ≠ 墨迹盒」）：本生成器直接取
//!   字形轮廓的包围盒，没有行盒这一层；
//! - **字重按字号分档**（`FONT_WEIGHT` / `FONT_WEIGHT_LATIN` 的判据是「小了会不会发虚」）：
//!   这里底色是实心色块、白字要从色块里「抠」出来，Light 在 16px 上只剩一层灰雾，故全档用 Bold。
//!
//! 清晰度的做法（`docs/design/linux-port.md` §5d 有放大图的结论）：
//! - 字形直接按目标尺寸栅格，不画大图再缩（缩小会把笔画边缘抹成灰边）；
//! - 文本后端没有 hinting，于是在 ±½ 像素内以 ⅛ 像素步进搜一个**最「实」的落点**（半透明像素
//!   最少）——横竖笔画尽量压在整像素格上，效果接近一次粗粒度的 hinting；
//! - 底板圆角超采样（否则是锯齿），字形不超采样。
//!
//! 图标表 [`ICONS`] 与 addon 的 `ModeIndicator`（`wind_linux/src/core/HostUi.cpp`）按名字一一对应，
//! `host_ui_test` 逐个核对文件在不在。

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
    use ab_glyph_rasterizer::{Point, Rasterizer, point};
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use ttf_parser::{Face, OutlineBuilder};

    /// 中文态底色：应用图标的品牌蓝。
    const BLUE: [u8; 3] = [0x2F, 0x86, 0xE6];
    /// 英文态底色：石板灰。与蓝一眼可分，白字在上面对比仍够。
    const GREY: [u8; 3] = [0x5E, 0x6B, 0x78];
    /// 大写锁定底色：橙。三态里最「吵」的一个——大写锁定是最容易忘了关的状态。
    const ORANGE: [u8; 3] = [0xE0, 0x8A, 0x1E];

    /// 图标名 → 字 + 底色。**名字是与 addon 的契约**（`HostUi.cpp` 的 `iconFor`）。
    ///
    /// 中文态按方案标签（`[schema] icon_label`）分：内置方案的标签是有限集，全部预生成；
    /// 自定义方案的标签在 addon 侧回落到 `windinput-zh`（见设计文档 §5d）。
    /// 英文方案（标签「英」）在中文模式下是**蓝底**「英」，与英文模式的灰底「英」不是一张图——
    /// 同 Windows：主字相同，主字色按 `effective_chinese` 取中文那一格。
    pub(crate) const ICONS: &[(&str, char, [u8; 3])] = &[
        ("windinput-zh", '中', BLUE),
        ("windinput-en", '英', GREY),
        ("windinput-caps", 'A', ORANGE),
        ("windinput-zh-pin", '拼', BLUE),
        ("windinput-zh-wu", '五', BLUE),
        ("windinput-zh-bi", '笔', BLUE),
        ("windinput-zh-shuang", '双', BLUE),
        ("windinput-zh-ying", '英', BLUE),
    ];

    /// 位图尺寸档（hicolor 的 `<N>x<N>`）。托盘常见 16/22/24，HiDPI 下取 32/48，64 给面板大图标。
    const SIZES: &[usize] = &[16, 22, 24, 32, 48, 64];

    /// 圆角半径 / 边长，与应用图标的圆角接近。
    const RADIUS: f32 = 0.22;

    /// 字身（em）/ 边长。按尺寸档给：小尺寸里每一像素都得给字形，大尺寸留出呼吸。
    ///
    /// 拉丁「A」的 em 里字面只占约 0.7，同一个 em 比例下比汉字小一圈，故另给一列。
    fn em_ratio(n: usize, latin: bool) -> f32 {
        match (n, latin) {
            (0..=16, false) => 0.82,
            (0..=24, false) => 0.80,
            (_, false) => 0.76,
            (0..=24, true) => 0.98,
            (_, true) => 0.94,
        }
    }

    pub fn main() -> Result<(), String> {
        let mut font_path: Option<PathBuf> = None;
        let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../wind_linux/data/icons/hicolor");
        let mut preview: Option<PathBuf> = None;
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            let mut val = || args.next().ok_or_else(|| format!("{a} 缺参数"));
            match a.as_str() {
                "--font" => font_path = Some(val()?.into()),
                "--out" => out = val()?.into(),
                "--preview" => preview = Some(val()?.into()),
                _ => return Err(format!("未知参数 {a}")),
            }
        }
        let font_path = font_path.ok_or("需要 --font <NotoSansCJKsc-Bold.otf>")?;
        let data =
            std::fs::read(&font_path).map_err(|e| format!("读 {}: {e}", font_path.display()))?;
        let face = Face::parse(&data, 0).map_err(|e| format!("解析字体: {e}"))?;

        let mut rendered: Vec<(&str, usize, Icon)> = Vec::new();
        let mut svgs: Vec<String> = Vec::new();
        for &(name, ch, bg) in ICONS {
            for &n in SIZES {
                let icon = render(&face, ch, bg, n)?;
                save_png(&out.join(format!("{n}x{n}/apps/{name}.png")), &icon)?;
                rendered.push((name, n, icon));
            }
            let svg = svg(&face, ch, bg)?;
            write(
                &out.join(format!("scalable/apps/{name}.svg")),
                svg.as_bytes(),
            )?;
            svgs.push(svg);
        }
        println!("已写入 {}", out.canonicalize().unwrap_or(out).display());

        if let Some(dir) = preview {
            write_previews(&dir, &rendered, &svgs)?;
            println!("预览图在 {}", dir.display());
        }
        Ok(())
    }

    /// 一张图标：`n×n` 非预乘 RGBA。
    pub(crate) struct Icon {
        n: usize,
        rgba: Vec<u8>,
    }

    /// 字形轮廓 → 光栅器，坐标经 `(x·s + dx, −y·s + dy)` 变换（字体 y 向上、位图 y 向下）。
    struct Raster {
        r: Rasterizer,
        s: f32,
        dx: f32,
        dy: f32,
        last: Point,
        start: Point,
    }

    impl Raster {
        fn map(&self, x: f32, y: f32) -> Point {
            point(x * self.s + self.dx, -y * self.s + self.dy)
        }
    }

    impl OutlineBuilder for Raster {
        fn move_to(&mut self, x: f32, y: f32) {
            self.last = self.map(x, y);
            self.start = self.last;
        }
        fn line_to(&mut self, x: f32, y: f32) {
            let p = self.map(x, y);
            self.r.draw_line(self.last, p);
            self.last = p;
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            let (c, p) = (self.map(x1, y1), self.map(x, y));
            self.r.draw_quad(self.last, c, p);
            self.last = p;
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            let (c1, c2, p) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y));
            self.r.draw_cubic(self.last, c1, c2, p);
            self.last = p;
        }
        fn close(&mut self) {
            if self.last != self.start {
                self.r.draw_line(self.last, self.start);
                self.last = self.start;
            }
        }
    }

    /// 字形在 `n×n` 画布里的摆放：缩放与平移（墨迹盒居中 + 微调偏移）。
    struct Placement {
        s: f32,
        dx: f32,
        dy: f32,
    }

    /// `k` 是对 [`em_ratio`] 的微调倍率（1.0 = 原值），见 [`best_placement`]。
    fn placement(
        face: &Face,
        ch: char,
        n: usize,
        k: f32,
        nudge: (f32, f32),
    ) -> Result<Placement, String> {
        let gid = face
            .glyph_index(ch)
            .ok_or_else(|| format!("字体里没有「{ch}」"))?;
        let bb = face
            .glyph_bounding_box(gid)
            .ok_or_else(|| format!("「{ch}」没有轮廓"))?;
        let em = n as f32 * em_ratio(n, ch.is_ascii()) * k;
        let s = em / face.units_per_em() as f32;
        let w = (bb.x_max - bb.x_min) as f32 * s;
        let h = (bb.y_max - bb.y_min) as f32 * s;
        Ok(Placement {
            s,
            dx: (n as f32 - w) / 2.0 - bb.x_min as f32 * s + nudge.0,
            dy: (n as f32 - h) / 2.0 + bb.y_max as f32 * s + nudge.1,
        })
    }

    fn glyph_mask(face: &Face, ch: char, n: usize, p: &Placement) -> Vec<f32> {
        let gid = face.glyph_index(ch).expect("placement 已查过");
        let mut r = Raster {
            r: Rasterizer::new(n, n),
            s: p.s,
            dx: p.dx,
            dy: p.dy,
            last: point(0.0, 0.0),
            start: point(0.0, 0.0),
        };
        face.outline_glyph(gid, &mut r);
        let mut m = vec![0.0f32; n * n];
        r.r.for_each_pixel_2d(|x, y, a| m[y as usize * n + x as usize] = a.clamp(0.0, 1.0));
        m
    }

    /// 半透明像素占墨量的比例：越小，笔画越「压格」、越实。
    ///
    /// 除以墨量是因为下面还搜字号：不归一的话，字越小半透明像素的绝对量越少，搜索会一路
    /// 偏向把字缩小。
    fn fuzz(m: &[f32]) -> f32 {
        let ink: f32 = m.iter().sum();
        m.iter().map(|&a| a.min(1.0 - a)).sum::<f32>() / ink.max(1.0)
    }

    /// 在墨迹居中点附近 ±½ 像素内按 ⅛ 像素步进、字号 ±4% 内分五档，挑最实的一版。
    ///
    /// 只挪落点压不住「笔画间距与像素格不成整数比」的那种糊（22px 的「中」竖画恰好跨两列、
    /// 「笔」的几道横挤成灰带）；字号微调几个百分点，肉眼看不出大小差，却能让间距落到整数格上。
    fn best_placement(face: &Face, ch: char, n: usize) -> Result<(Placement, Vec<f32>), String> {
        let mut best: Option<(f32, Placement, Vec<f32>)> = None;
        for k in [0.96, 0.98, 1.0, 1.02, 1.04] {
            for ix in -4..4 {
                for iy in -4..4 {
                    let p = placement(face, ch, n, k, (ix as f32 / 8.0, iy as f32 / 8.0))?;
                    let m = glyph_mask(face, ch, n, &p);
                    let f = fuzz(&m);
                    if best.as_ref().is_none_or(|b| f < b.0) {
                        best = Some((f, p, m));
                    }
                }
            }
        }
        let (_, p, m) = best.expect("搜索范围非空");
        Ok((p, m))
    }

    /// 圆角方块的覆盖度（4×4 超采样）。
    fn plate_mask(n: usize) -> Vec<f32> {
        let s = n as f32;
        let r = s * RADIUS;
        let inside = |x: f32, y: f32| {
            let cx = x.clamp(r, s - r);
            let cy = y.clamp(r, s - r);
            (x - cx).powi(2) + (y - cy).powi(2) <= r * r
        };
        const K: usize = 4;
        let mut m = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                let mut hit = 0;
                for sy in 0..K {
                    for sx in 0..K {
                        let fx = x as f32 + (sx as f32 + 0.5) / K as f32;
                        let fy = y as f32 + (sy as f32 + 0.5) / K as f32;
                        hit += inside(fx, fy) as usize;
                    }
                }
                m[y * n + x] = hit as f32 / (K * K) as f32;
            }
        }
        m
    }

    /// 16px 档的手绘点阵。`#` 全白、`+` 半白、`.` 底色；行宽一致，按整像素居中贴进画布。
    const PIXEL_16: &[(char, &[&str])] = &[
        (
            '中',
            &[
                ".....##.....",
                ".....##.....",
                ".##########.",
                ".#...##...#.",
                ".#...##...#.",
                ".#...##...#.",
                ".#...##...#.",
                ".##########.",
                ".....##.....",
                ".....##.....",
                ".....##.....",
                ".....##.....",
            ],
        ),
        (
            '英',
            &[
                "..#.....#..",
                "###########",
                "..#.....#..",
                ".....#.....",
                ".#########.",
                ".#...#...#.",
                ".#...#...#.",
                "###########",
                ".....#.....",
                "....#.#....",
                "..##...##..",
                "##.......##",
            ],
        ),
        (
            'A',
            &[
                "....###....",
                "...##.##...",
                "...##.##...",
                "..##...##..",
                "..##...##..",
                "..##...##..",
                ".##.....##.",
                ".#########.",
                ".##.....##.",
                "##.......##",
                "##.......##",
                "##.......##",
            ],
        ),
        (
            '五',
            &[
                ".#########.",
                "....#......",
                "....#......",
                "....#......",
                "....#......",
                "..########.",
                "...#.....#.",
                "...#.....#.",
                "...#.....#.",
                "...#.....#.",
                "...#.....#.",
                "###########",
            ],
        ),
        (
            '拼',
            &[
                "..#...#...#.",
                "..#....#.#..",
                "####.#######",
                "..#....#.#..",
                "..#....#.#..",
                "..##...#.#..",
                ".##..#######",
                "#.#....#.#..",
                "..#....#.#..",
                "..#...#..#..",
                "..#...#..#..",
                ".##..#...#..",
            ],
        ),
        (
            '笔',
            &[
                ".#.....#....",
                "#####.#####.",
                "..#.....#...",
                "............",
                "........###.",
                "..######....",
                ".....#......",
                ".##########.",
                ".....#......",
                ".....#......",
                ".....#....#.",
                "......#####.",
            ],
        ),
        (
            '双',
            &[
                "####..######",
                "...#.......#",
                "...#.......#",
                ".#.#..#...#.",
                "..#....#..#.",
                "..#.....##..",
                ".#.#....##..",
                ".#..#..#..#.",
                "#......#...#",
                "#.....#.....",
            ],
        ),
    ];

    /// 手绘点阵 → 覆盖度蒙版（整像素居中）。没有这个字的点阵时返回 `None`，走字形栅格。
    fn pixel_mask(ch: char, n: usize) -> Option<Vec<f32>> {
        if n != 16 {
            return None;
        }
        let rows = PIXEL_16.iter().find(|p| p.0 == ch)?.1;
        let h = rows.len();
        let w = rows[0].chars().count();
        let (x0, y0) = ((n - w) / 2, (n - h) / 2);
        let mut m = vec![0.0f32; n * n];
        for (y, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), w, "「{ch}」点阵行宽不一致");
            for (x, c) in row.chars().enumerate() {
                m[(y0 + y) * n + x0 + x] = match c {
                    '#' => 1.0,
                    '+' => 0.5,
                    _ => 0.0,
                };
            }
        }
        Some(m)
    }

    fn render(face: &Face, ch: char, bg: [u8; 3], n: usize) -> Result<Icon, String> {
        let glyph = match pixel_mask(ch, n) {
            Some(m) => m,
            None => best_placement(face, ch, n)?.1,
        };
        let plate = plate_mask(n);
        let mut rgba = vec![0u8; n * n * 4];
        for i in 0..n * n {
            let g = glyph[i];
            for c in 0..3 {
                rgba[i * 4 + c] = (bg[c] as f32 * (1.0 - g) + 255.0 * g).round() as u8;
            }
            // 字形都在底板里面（圆角外没有墨迹），alpha 只看底板。
            rgba[i * 4 + 3] = (plate[i] * 255.0).round() as u8;
        }
        Ok(Icon { n, rgba })
    }

    /// 字形轮廓 → SVG path 的 `d`。
    struct SvgPath {
        d: String,
        s: f32,
        dx: f32,
        dy: f32,
    }

    impl SvgPath {
        fn pt(&mut self, x: f32, y: f32) {
            let (x, y) = (x * self.s + self.dx, -y * self.s + self.dy);
            let _ = write!(self.d, "{} {}", num(x), num(y));
        }
    }

    fn num(v: f32) -> String {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    impl OutlineBuilder for SvgPath {
        fn move_to(&mut self, x: f32, y: f32) {
            self.d.push('M');
            self.pt(x, y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.d.push('L');
            self.pt(x, y);
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            self.d.push('Q');
            self.pt(x1, y1);
            self.d.push(' ');
            self.pt(x, y);
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            self.d.push('C');
            self.pt(x1, y1);
            self.d.push(' ');
            self.pt(x2, y2);
            self.d.push(' ');
            self.pt(x, y);
        }
        fn close(&mut self) {
            self.d.push('Z');
        }
    }

    /// 可缩放版：几何与 64px 档相同（同一套 em 比例与墨迹居中，不做 ⅛ 像素微调——矢量没有像素格）。
    fn svg(face: &Face, ch: char, bg: [u8; 3]) -> Result<String, String> {
        const N: usize = 64;
        let p = placement(face, ch, N, 1.0, (0.0, 0.0))?;
        let gid = face.glyph_index(ch).expect("placement 已查过");
        let mut path = SvgPath {
            d: String::new(),
            s: p.s,
            dx: p.dx,
            dy: p.dy,
        };
        face.outline_glyph(gid, &mut path);
        let r = num(N as f32 * RADIUS);
        Ok(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{N}\" height=\"{N}\" viewBox=\"0 0 {N} {N}\">\n  \
             <rect width=\"{N}\" height=\"{N}\" rx=\"{r}\" ry=\"{r}\" fill=\"#{:02X}{:02X}{:02X}\"/>\n  \
             <path d=\"{}\" fill=\"#fff\"/>\n</svg>\n",
            bg[0], bg[1], bg[2], path.d
        ))
    }

    fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("建目录 {}: {e}", d.display()))?;
        }
        std::fs::write(path, bytes).map_err(|e| format!("写 {}: {e}", path.display()))
    }

    fn save_png(path: &Path, icon: &Icon) -> Result<(), String> {
        let img = image::RgbaImage::from_raw(icon.n as u32, icon.n as u32, icon.rgba.clone())
            .ok_or("位图尺寸不符")?;
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("编码 PNG: {e}"))?;
        write(path, &bytes)
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
        rendered: &[(&str, usize, Icon)],
        svgs: &[String],
    ) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {}: {e}", dir.display()))?;
        let names: Vec<&str> = ICONS.iter().map(|i| i.0).collect();
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
