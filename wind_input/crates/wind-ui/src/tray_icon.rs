//! Linux 托盘 / 面板的模式图标：**按模式主字运行时渲染**，同 Windows 语言栏图标。
//!
//! 主字就是 Windows 语言栏用的那一份（`Coordinator::mode_icon_label`：有效中文取方案
//! `icon_label`，英文取 `[ui.labels] english`，大写锁定取 `caps_lock`），所以切方案、自定义方案、
//! 改 `[ui.labels]` 都会反映到托盘上。服务进程按（状态, 主字）渲染各尺寸 PNG，写进用户图标目录
//! `$XDG_DATA_HOME/icons/hicolor/<N>x<N>/apps/`（XDG 图标主题规范的用户基目录，GTK / Qt / Fcitx5
//! 自己的 `IconTheme` 都在这里找 hicolor 的子目录），图标名 [`icon_name`] 由 addon 按同一规则算出、
//! 经 `subModeIcon` 交给 Fcitx5。设计与宿主行为见 `docs/design/linux-port.md` §5d。
//!
//! 图形：带底色的圆角方块 + 白字（底色按状态：中文蓝、英文灰、大写锁定橙），深浅面板都立得住。
//! 与 [`crate::langbar_icon::IconRenderer`] 不是一种图（那边透明底、任务栏明暗两套字色、Light 字重），
//! 借的是它的两条策略：**按墨迹盒居中**；**小字号按「会不会发虚」定字重**——这里底色是实心色块、
//! 白字要从色块里抠出来，Light 在 16px 上只剩一层灰雾，故取 Bold。
//!
//! 清晰度（本仓 Linux 文本后端没有 hinting，直接栅格会把 16px「英」的笔画间隙糊成灰带）：
//! - 16px 的常用单字（`PIXEL_16` 表：中 英 A 拼 五 笔 双）用手绘点阵，每一笔落整像素；
//! - 其余按字形轮廓栅格，在 ±½ 像素 × 字号 ±4% 内挑半透明像素最少的一版（粗粒度对格）；
//! - 字形直接按目标尺寸栅格，不画大图再缩；底板圆角超采样。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use ttf_parser::{Face, OutlineBuilder};

/// 图标的状态档，决定底色。与主字无关：英文方案（主字「英」）在中文模式下是 [`Self::Chinese`]，
/// 同 Windows 按 `effective_chinese` 取中文那一格颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// 有效中文（中文模式且大写锁定未开）。
    Chinese,
    /// 英文模式。
    English,
    /// 大写锁定（无论中英）。
    Caps,
}

impl Variant {
    /// 图标名里的状态段。**与 addon 的 `ModeIndicator` 对齐**。
    pub fn tag(self) -> &'static str {
        match self {
            Variant::Chinese => "zh",
            Variant::English => "en",
            Variant::Caps => "caps",
        }
    }

    /// 底色（RGB）。
    pub fn color(self) -> [u8; 3] {
        match self {
            // 应用图标的品牌蓝。
            Variant::Chinese => [0x2F, 0x86, 0xE6],
            // 石板灰：与蓝一眼可分，白字在上面对比仍够。
            Variant::English => [0x5E, 0x6B, 0x78],
            // 橙：三态里最「吵」的一个——大写锁定是最容易忘了关的状态。
            Variant::Caps => [0xE0, 0x8A, 0x1E],
        }
    }
}

/// 运行时图标的名字：`<前缀><状态>-<主字编码>`。
///
/// - 前缀按变体分（[`icon_prefix`]）：正式版与 dev 版共用 `~/.local/share/icons/hicolor`，名字不分
///   的话启动清理（[`TrayIcons::prune`]）会互删对方的图标。
/// - 主字编码：UTF-8 的小写十六进制；超过 [`MAX_HEX_LABEL_BYTES`] 字节改成 `h` + FNV-1a 64 位散列的
///   16 位十六进制（文件名上限 255 字节，`[ui.labels]` 可以配很长；十六进制恒为偶数位、只含 0-9a-f，
///   `h` 开头不会与之混淆）。
///
/// **这是与 addon 的契约**（`HostUi.cpp` 的 `dynamicIconName`，`host_ui_test` 读本文件核对同一组样例）。
/// 用十六进制而不是主字本身：图标名要进 kimpanel 以冒号分段的属性串、进文件名，只用 ASCII 最稳。
pub fn icon_name(dev: bool, variant: Variant, label: &str) -> String {
    let mut s = format!("{}{}-", icon_prefix(dev), variant.tag());
    if label.len() > MAX_HEX_LABEL_BYTES {
        s.push_str(&format!("h{:016x}", fnv1a64(label.as_bytes())));
        return s;
    }
    for b in label.as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 运行时图标的前缀：正式版 `windinput-lbl-`（保持原名，已装机用户磁盘上的运行时图标照认），
/// dev 版 `windinput-lbl-dev-`。清理时据此认领自己的文件（见 [`TrayIcons::prune`]）。
pub fn icon_prefix(dev: bool) -> &'static str {
    if dev {
        "windinput-lbl-dev-"
    } else {
        "windinput-lbl-"
    }
}

/// 主字按十六进制编码的字节上限，超了改用散列（见 [`icon_name`]）。32 字节 = 10 个汉字，远超
/// 标签宽度上限（2）——只有手配的超长 `[ui.labels]` 会走到散列。
pub const MAX_HEX_LABEL_BYTES: usize = 32;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `stem` 是不是这个前缀名下、本模块产出的图标名（`<前缀><zh|en|caps>-<十六进制 | h散列>`）。
/// 正式版前缀是 dev 前缀的前缀：dev 的名字在正式版看来状态段是 `dev`，不认——两个变体各删各的。
fn owns(prefix: &str, stem: &str) -> bool {
    let Some(rest) = stem.strip_prefix(prefix) else {
        return false;
    };
    let Some((tag, enc)) = rest.split_once('-') else {
        return false;
    };
    let hex = |t: &str| {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    matches!(tag, "zh" | "en" | "caps")
        && (hex(enc)
            || enc
                .strip_prefix('h')
                .is_some_and(|h| h.len() == 16 && hex(h)))
}

/// 渲染的尺寸档（hicolor 的 `<N>x<N>`）。托盘常见 16/22/24，HiDPI 取 32/48，64 给面板大图标。
///
/// **按从小到大写**，最后写的是 [`LAST_SIZE`]：addon 以它在不在判断这一组是否写完。
pub const SIZES: &[usize] = &[16, 22, 24, 32, 48, 64];
/// 一组图标里最后写出的尺寸，见 [`SIZES`]。
pub const LAST_SIZE: usize = 64;

/// 用户图标目录下的 hicolor 根：`$XDG_DATA_HOME/icons/hicolor`（缺省 `~/.local/share`）。
///
/// 按 XDG 规范：`XDG_DATA_HOME` 为空或不是绝对路径时视同未设。addon 按同一规则算（在同一会话、
/// 同一环境下，服务多由 addon 拉起，环境一致）。
pub fn user_hicolor_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("icons/hicolor"))
}

/// 一张图标：`n×n` 非预乘 RGBA。
pub struct Icon {
    pub n: usize,
    pub rgba: Vec<u8>,
}

/// 圆角半径 / 边长，与应用图标的圆角接近。
pub const RADIUS: f32 = 0.22;

/// 字身（em）/ 边长。小尺寸里每一像素都得给字形，大尺寸留出呼吸。
///
/// 纯拉丁标签的 em 里字面只占约 0.7，同一个比例下比汉字小一圈，故另给一列。
fn em_ratio(n: usize, latin: bool) -> f32 {
    match (n, latin) {
        (0..=16, false) => 0.82,
        (0..=24, false) => 0.80,
        (_, false) => 0.76,
        (0..=24, true) => 0.98,
        (_, true) => 0.94,
    }
}

/// 多字标签（`En`、双汉字截断前的宽标签）排不下时，墨迹宽度的上限 / 边长。
const MAX_INK_WIDTH: f32 = 0.84;

/// 16px 档的手绘点阵。`#` 全白、`.` 底色；按整像素居中贴进画布。
///
/// 只收常用单字：出厂方案的标签与英文 / 大写的出厂标签。别的字走字形栅格（见模块头）。
pub const PIXEL_16: &[(char, &[&str])] = &[
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

/// 手绘点阵 → 覆盖度蒙版（整像素居中）。只有 16px、单字、表里有时给出。
fn pixel_mask(label: &str, n: usize) -> Option<Vec<f32>> {
    let mut chars = label.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if n != 16 {
        return None;
    }
    let rows = PIXEL_16.iter().find(|p| p.0 == ch)?.1;
    let h = rows.len();
    let w = rows[0].chars().count();
    let (x0, y0) = ((n - w) / 2, (n - h) / 2);
    let mut m = vec![0.0f32; n * n];
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            if c == '#' {
                m[(y0 + y) * n + x0 + x] = 1.0;
            }
        }
    }
    Some(m)
}

/// 字形轮廓 → 目标，坐标经 `(x·s + dx, −y·s + dy)` 变换（字体 y 向上、位图 y 向下）。
/// 由 [`Raster`] 与生成器的 SVG 输出共用。
pub trait PathSink {
    fn line(&mut self, a: Point, b: Point);
    fn quad(&mut self, a: Point, c: Point, b: Point);
    fn cubic(&mut self, a: Point, c1: Point, c2: Point, b: Point);
    fn begin(&mut self, _p: Point) {}
    fn end(&mut self) {}
}

struct Outline<'a, S: PathSink> {
    sink: &'a mut S,
    s: f32,
    dx: f32,
    dy: f32,
    last: Point,
    start: Point,
}

impl<S: PathSink> Outline<'_, S> {
    fn map(&self, x: f32, y: f32) -> Point {
        point(x * self.s + self.dx, -y * self.s + self.dy)
    }
}

impl<S: PathSink> OutlineBuilder for Outline<'_, S> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = self.map(x, y);
        self.start = self.last;
        self.sink.begin(self.last);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.sink.line(self.last, p);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.map(x1, y1), self.map(x, y));
        self.sink.quad(self.last, c, p);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y));
        self.sink.cubic(self.last, c1, c2, p);
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.sink.line(self.last, self.start);
            self.last = self.start;
        }
        self.sink.end();
    }
}

struct Raster(Rasterizer);

impl PathSink for Raster {
    fn line(&mut self, a: Point, b: Point) {
        self.0.draw_line(a, b);
    }
    fn quad(&mut self, a: Point, c: Point, b: Point) {
        self.0.draw_quad(a, c, b);
    }
    fn cubic(&mut self, a: Point, c1: Point, c2: Point, b: Point) {
        self.0.draw_cubic(a, c1, c2, b);
    }
}

/// 标签在 `n×n` 画布里的排布：每个字形的（字形号, 笔位 x，字体单位），加整体缩放与平移。
pub struct Placement {
    glyphs: Vec<(ttf_parser::GlyphId, f32)>,
    s: f32,
    dx: f32,
    dy: f32,
}

impl Placement {
    /// 按排布把轮廓送进 `sink`。
    pub fn outline<S: PathSink>(&self, face: &Face, sink: &mut S) {
        for &(gid, pen) in &self.glyphs {
            let mut o = Outline {
                sink: &mut *sink,
                s: self.s,
                dx: self.dx + pen * self.s,
                dy: self.dy,
                last: point(0.0, 0.0),
                start: point(0.0, 0.0),
            };
            face.outline_glyph(gid, &mut o);
        }
    }
}

/// 排一次：`k` 是对 em 比例的微调倍率，`nudge` 是在墨迹居中点上的偏移（像素）。
pub fn place(
    face: &Face,
    label: &str,
    n: usize,
    k: f32,
    nudge: (f32, f32),
) -> Result<Placement, String> {
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for ch in label.chars() {
        let gid = face
            .glyph_index(ch)
            .ok_or_else(|| format!("字体里没有「{ch}」"))?;
        if let Some(bb) = face.glyph_bounding_box(gid) {
            x0 = x0.min(pen + bb.x_min as f32);
            x1 = x1.max(pen + bb.x_max as f32);
            y0 = y0.min(bb.y_min as f32);
            y1 = y1.max(bb.y_max as f32);
        }
        glyphs.push((gid, pen));
        pen += face.glyph_hor_advance(gid).unwrap_or(0) as f32;
    }
    if x0 > x1 {
        return Err(format!("「{label}」没有轮廓"));
    }
    let latin = label.is_ascii();
    let mut s = n as f32 * em_ratio(n, latin) * k / face.units_per_em() as f32;
    // 宽标签按墨迹宽度回缩（同 langbar_icon：判据是实测宽度，不是字符数）。
    let max_w = n as f32 * MAX_INK_WIDTH;
    if (x1 - x0) * s > max_w {
        s = max_w / (x1 - x0);
    }
    let (w, h) = ((x1 - x0) * s, (y1 - y0) * s);
    Ok(Placement {
        glyphs,
        s,
        dx: (n as f32 - w) / 2.0 - x0 * s + nudge.0,
        dy: (n as f32 - h) / 2.0 + y1 * s + nudge.1,
    })
}

fn glyph_mask(face: &Face, n: usize, p: &Placement) -> Vec<f32> {
    let mut r = Raster(Rasterizer::new(n, n));
    p.outline(face, &mut r);
    let mut m = vec![0.0f32; n * n];
    r.0.for_each_pixel_2d(|x, y, a| m[y as usize * n + x as usize] = a.clamp(0.0, 1.0));
    m
}

/// 半透明像素占墨量的比例：越小，笔画越「压格」、越实。除以墨量是因为还要搜字号——
/// 不归一的话，字越小半透明像素的绝对量越少，搜索会一路偏向把字缩小。
fn fuzz(m: &[f32]) -> f32 {
    let ink: f32 = m.iter().sum();
    m.iter().map(|&a| a.min(1.0 - a)).sum::<f32>() / ink.max(1.0)
}

/// 在墨迹居中点附近 ±½ 像素内按 ⅛ 像素步进、字号 ±4% 内分五档，挑最实的一版。
///
/// 只挪落点压不住「笔画间距与像素格不成整数比」的糊（22px「中」的竖画跨两列、「笔」的几道横
/// 挤成灰带）；字号微调几个百分点，肉眼看不出大小差，却能让间距落到整数格上。
fn best_glyph_mask(face: &Face, label: &str, n: usize) -> Result<Vec<f32>, String> {
    let mut best: Option<(f32, Vec<f32>)> = None;
    for k in [0.96, 0.98, 1.0, 1.02, 1.04] {
        for ix in -4..4 {
            for iy in -4..4 {
                let p = place(face, label, n, k, (ix as f32 / 8.0, iy as f32 / 8.0))?;
                let m = glyph_mask(face, n, &p);
                let f = fuzz(&m);
                if best.as_ref().is_none_or(|b| f < b.0) {
                    best = Some((f, m));
                }
            }
        }
    }
    Ok(best.expect("搜索范围非空").1)
}

/// 字形栅格在这个尺寸及以下做一次对比度拉伸，见 [`sharpen`]。
const SHARPEN_MAX: usize = 16;

/// 把半透明像素往 0 / 1 推：`(a − 0.3) / 0.4` 截到 [0, 1]。
///
/// 16px 下没有手绘点阵的字（自定义方案标签「虎」、两字母「En」）栅格出来笔画都跨两列，整字是
/// 一层灰雾；拉开对比后笔画实、间隙清。代价是斜笔画更锯齿、极细的笔画可能断——在 16px 的实心色块
/// 上，锯齿远比灰雾好认。22px 起字身够大，不拉。
fn sharpen(m: &mut [f32]) {
    for a in m.iter_mut() {
        *a = ((*a - 0.3) / 0.4).clamp(0.0, 1.0);
    }
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

/// 渲染一张 `n×n` 图标。
pub fn render(face: &Face, variant: Variant, label: &str, n: usize) -> Result<Icon, String> {
    let glyph = match pixel_mask(label, n) {
        Some(m) => m,
        None => {
            let mut m = best_glyph_mask(face, label, n)?;
            if n <= SHARPEN_MAX {
                sharpen(&mut m);
            }
            m
        }
    };
    let plate = plate_mask(n);
    let bg = variant.color();
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

/// PNG 编码。
pub fn encode_png(icon: &Icon) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(icon.n as u32, icon.n as u32, icon.rgba.clone())
        .ok_or("位图尺寸不符")?;
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("编码 PNG: {e}"))?;
    Ok(bytes)
}

/// 原子写：先写同目录临时文件再 rename，读的一方（图标主题扫目录）只会看到完整文件或没有。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("路径没有父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("icon"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("改名 {}: {e}", path.display())
    })
}

/// 运行时图标缓存：同一（状态, 主字）只渲染一次。
///
/// 进程内记已确认存在的名字；首次遇到时看磁盘上那组是否齐全（上次运行写过就不重画）。
pub struct TrayIcons {
    root: PathBuf,
    /// dev 变体：图标名前缀不同（见 [`icon_prefix`]）。
    dev: bool,
    /// 已确认齐全的图标名。
    ready: HashSet<String>,
    /// 字体按「覆盖这些字」查过的结果缓存：（覆盖的字符串, 字体数据, 集合序号）。
    fonts: Vec<(String, std::sync::Arc<Vec<u8>>, u32)>,
}

impl TrayIcons {
    /// `root` 是 hicolor 根（通常 [`user_hicolor_dir`]）；`dev` 是本进程的变体（决定图标名前缀）。
    pub fn new(root: PathBuf, dev: bool) -> Self {
        Self {
            root,
            dev,
            ready: HashSet::new(),
            fonts: Vec::new(),
        }
    }

    /// 本进程（变体）的图标名，见 [`icon_name`]。
    pub fn name(&self, variant: Variant, label: &str) -> String {
        icon_name(self.dev, variant, label)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn png_path(&self, name: &str, n: usize) -> PathBuf {
        self.root.join(format!("{n}x{n}/apps/{name}.png"))
    }

    /// 字体：Bold，覆盖标签全部字符。按标签缓存（标签集合很小）。
    fn font_for(&mut self, label: &str) -> Result<(std::sync::Arc<Vec<u8>>, u32), String> {
        if let Some((_, d, i)) = self.fonts.iter().find(|f| f.0 == label) {
            return Ok((d.clone(), *i));
        }
        let (path, index) = crate::text::linux::font_file_covering(label, 700)
            .ok_or_else(|| format!("没有字体覆盖「{label}」（fontconfig 不可用或缺 CJK 字体）"))?;
        let data = std::sync::Arc::new(
            std::fs::read(&path).map_err(|e| format!("读字体 {}: {e}", path.display()))?,
        );
        self.fonts.push((label.to_string(), data.clone(), index));
        Ok((data, index))
    }

    /// 建好全部尺寸档的 `apps` 目录。
    ///
    /// 要赶在宿主构建图标主题之前：Fcitx5 的 `IconTheme`（classicui 托盘）与 Qt 的 `QIconTheme`
    /// 在构造时把**不存在**的主题目录永久排除，KIconLoader 只为构造时已存在的子目录建索引——
    /// 目录晚一步出现，里面的文件就再也找不到，直到宿主重启。addon 在 Fcitx5 加载它时也建一次。
    pub fn create_dirs(&self) -> Result<(), String> {
        for &n in SIZES {
            let d = self.root.join(format!("{n}x{n}/apps"));
            std::fs::create_dir_all(&d).map_err(|e| format!("建目录 {}: {e}", d.display()))?;
        }
        Ok(())
    }

    /// 让 hicolor 根目录的 mtime 前进：建一个空文件再删掉。
    ///
    /// GTK 的 `GtkIconTheme` 与 GNOME Shell 的 `StIconTheme` 在加载主题时快照子目录里的文件名，
    /// 之后只看「搜索根」与「主题根」两层目录的 mtime 决定要不要重扫（至多每 5 秒一次）——往已存在的
    /// `48x48/apps/` 里加文件不改这两层的 mtime，新图标永远看不见。改 mtime 用建删文件而不是
    /// `utimensat`，免得为一次 touch 引 libc。失败只影响那两个宿主的刷新时机，忽略。
    fn touch_root(&self) {
        let probe = self
            .root
            .join(format!(".windinput-touch.{}", std::process::id()));
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
        }
    }

    /// 确保（状态, 主字）这一组图标已写在磁盘上，返回图标名。空标签不画（返回 `None`）。
    ///
    /// 写入按 [`SIZES`] 从小到大，最后一张是 [`LAST_SIZE`]——addon 据它判断「写完了」。
    pub fn ensure(&mut self, variant: Variant, label: &str) -> Result<Option<String>, String> {
        if label.is_empty() {
            return Ok(None);
        }
        let name = self.name(variant, label);
        // 进程内缓存命中也补查一下最后那张还在不在（一次 stat）：别的进程（另一个变体的旧版本、
        // 用户手动清理）可能删过它，缓存却以为还在，addon 就只能一直退回种子图标。
        if self.ready.contains(&name) && self.png_path(&name, LAST_SIZE).is_file() {
            return Ok(Some(name));
        }
        if !SIZES.iter().all(|&n| self.png_path(&name, n).is_file()) {
            let (data, index) = self.font_for(label)?;
            let face = Face::parse(&data, index).map_err(|e| format!("解析字体: {e}"))?;
            for &n in SIZES {
                let png = encode_png(&render(&face, variant, label, n)?)?;
                write_atomic(&self.png_path(&name, n), &png)?;
            }
            self.touch_root();
        }
        self.ready.insert(name.clone());
        Ok(Some(name))
    }

    /// 删掉本变体名下、既不在 `keep` 里也不是本进程备好过的运行时图标（只认 [`owns`] 认领的文件：
    /// 另一个变体的、种子、别的应用的一概不碰）。返回删掉的文件数。
    ///
    /// 本进程备好过的（`ready`）一并保留：启动清理在后台跑，期间前台可能刚为新标签写出图标。
    pub fn prune(&mut self, keep: &HashSet<String>) -> usize {
        let prefix = icon_prefix(self.dev);
        let mut removed = 0;
        let Ok(dirs) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        for d in dirs.flatten() {
            let apps = d.path().join("apps");
            let Ok(files) = std::fs::read_dir(&apps) else {
                continue;
            };
            for f in files.flatten() {
                let file = f.file_name();
                let Some(file) = file.to_str() else {
                    continue;
                };
                let Some(stem) = file.strip_suffix(".png") else {
                    continue;
                };
                if owns(prefix, stem)
                    && !keep.contains(stem)
                    && !self.ready.contains(stem)
                    && std::fs::remove_file(f.path()).is_ok()
                {
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_name_is_ascii_and_matches_addon_samples() {
        // 与 wind_linux/tests/host_ui_test.cpp 的「运行时图标名」用例同一组样例（那边读本文件核对
        // 下面这些字面量）。正式版名字与改版前逐字相同：已装机用户磁盘上的运行时图标照认。
        assert_eq!(
            icon_name(false, Variant::Chinese, "拼"),
            "windinput-lbl-zh-e68bbc"
        );
        assert_eq!(
            icon_name(false, Variant::English, "英"),
            "windinput-lbl-en-e88bb1"
        );
        assert_eq!(
            icon_name(false, Variant::Caps, "A"),
            "windinput-lbl-caps-41"
        );
        assert_eq!(
            icon_name(false, Variant::English, "En"),
            "windinput-lbl-en-456e"
        );
        assert_eq!(
            icon_name(true, Variant::Chinese, "拼"),
            "windinput-lbl-dev-zh-e68bbc"
        );
        // 超长标签走散列：40 个「虎」（120 字节）。
        let long = "虎".repeat(40);
        assert_eq!(
            icon_name(false, Variant::Chinese, &long),
            "windinput-lbl-zh-h8b27ed0fc2ac336d"
        );
        // 32 字节恰好还是十六进制，33 字节起散列。
        assert_eq!(
            icon_name(false, Variant::Caps, &"a".repeat(32)),
            format!("windinput-lbl-caps-{}", "61".repeat(32))
        );
        assert!(
            icon_name(false, Variant::Caps, &"a".repeat(33)).starts_with("windinput-lbl-caps-h")
        );
    }

    /// 文件名不超过 255 字节，不论标签多长。
    #[test]
    fn icon_file_name_fits_name_max() {
        for n in [1, 10, 11, 32, 33, 100, 1000] {
            let name = icon_name(true, Variant::Caps, &"虎".repeat(n));
            assert!(name.len() + ".png".len() <= 255, "{n}: {}", name.len());
            assert!(name.is_ascii());
        }
    }

    /// 两个变体各认各的：正式版前缀是 dev 前缀的前缀，dev 的名字不能被正式版认领（反之亦然）；
    /// 种子与别的应用的文件谁都不认。
    #[test]
    fn owns_distinguishes_variants() {
        let rel = icon_prefix(false);
        let dev = icon_prefix(true);
        assert!(owns(rel, "windinput-lbl-zh-e68bbc"));
        assert!(owns(rel, "windinput-lbl-caps-h8b27ed0fc2ac336d"));
        assert!(!owns(rel, "windinput-lbl-dev-zh-e68bbc"));
        assert!(owns(dev, "windinput-lbl-dev-zh-e68bbc"));
        assert!(!owns(dev, "windinput-lbl-zh-e68bbc"));
        for foreign in [
            "windinput-zh",
            "windinput",
            "windinput-lbl-xx-41",
            "windinput-lbl-zh-",
            "windinput-lbl-zh-4G",
            "windinput-lbl-zh-h123",
        ] {
            assert!(!owns(rel, foreign), "{foreign}");
            assert!(!owns(dev, foreign), "{foreign}");
        }
    }

    #[test]
    fn pixel_art_rows_are_rectangular_and_fit() {
        for (ch, rows) in PIXEL_16 {
            let w = rows[0].chars().count();
            assert!(
                rows.iter().all(|r| r.chars().count() == w),
                "「{ch}」行宽不一致"
            );
            assert!(w <= 14 && rows.len() <= 14, "「{ch}」超出 16px 底板的内框");
        }
    }

    #[test]
    fn pixel_art_only_for_single_known_char_at_16() {
        assert!(pixel_mask("中", 16).is_some());
        assert!(pixel_mask("中", 22).is_none());
        assert!(pixel_mask("虎", 16).is_none());
        assert!(pixel_mask("En", 16).is_none());
    }

    #[test]
    fn user_dir_follows_xdg_data_home() {
        // 只读环境变量的纯函数难以隔离；这里只验结构：以 icons/hicolor 结尾。
        if let Some(d) = user_hicolor_dir() {
            assert!(d.ends_with("icons/hicolor"));
        }
    }

    /// 真渲染 + 落盘 + 清理（需要系统有 CJK 字体；本仓 Linux 形态的文字测试都有这个前提）。
    #[test]
    fn ensure_writes_every_size_once_and_prune_keeps_only_wanted() {
        let tmp = std::env::temp_dir().join(format!("wi-tray-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut t = TrayIcons::new(tmp.clone(), false);
        t.create_dirs().unwrap();
        assert!(tmp.join("48x48/apps").is_dir());
        let before = std::fs::metadata(&tmp).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let name = t.ensure(Variant::Chinese, "五").unwrap().unwrap();
        for &n in SIZES {
            let p = tmp.join(format!("{n}x{n}/apps/{name}.png"));
            let img = image::open(&p).expect("合法 PNG").to_rgba8();
            assert_eq!((img.width(), img.height()), (n as u32, n as u32));
        }
        // 写出新图标后主题根的 mtime 前进（GTK / GNOME Shell 据它重扫）。
        assert!(std::fs::metadata(&tmp).unwrap().modified().unwrap() > before);
        // 自定义标签（没有点阵）也画得出来，且不是空白：中心附近有白字像素。
        let custom = t.ensure(Variant::Chinese, "虎").unwrap().unwrap();
        let img = image::open(tmp.join(format!("24x24/apps/{custom}.png")))
            .unwrap()
            .to_rgba8();
        assert!(img.pixels().any(|p| p[0] > 200 && p[1] > 200 && p[2] > 200));
        // 空标签不画。
        assert_eq!(t.ensure(Variant::English, "").unwrap(), None);
        // 缓存命中但文件被别人删了：补查发现、重画（不会一直以为它在）。
        std::fs::remove_file(tmp.join(format!("64x64/apps/{name}.png"))).unwrap();
        assert_eq!(t.ensure(Variant::Chinese, "五").unwrap().unwrap(), name);
        assert!(tmp.join(format!("64x64/apps/{name}.png")).is_file());
        // 别人的文件不碰：种子、另一个变体（dev）的运行时图标。
        let foreign = tmp.join("16x16/apps/windinput-zh.png");
        std::fs::write(&foreign, b"x").unwrap();
        let other_variant = tmp.join("16x16/apps/windinput-lbl-dev-zh-e4ba94.png");
        std::fs::write(&other_variant, b"x").unwrap();
        // 本进程备好过的（ready）不在 keep 里也留着：启动清理在后台跑，期间前台可能刚写出新标签。
        let keep: HashSet<String> = [name.clone()].into_iter().collect();
        assert_eq!(t.prune(&keep), 0, "custom 是本进程备好的，不删");
        let mut fresh = TrayIcons::new(tmp.clone(), false);
        assert_eq!(fresh.prune(&keep), SIZES.len(), "新进程只保留 keep");
        assert!(tmp.join(format!("16x16/apps/{name}.png")).is_file());
        assert!(!tmp.join(format!("16x16/apps/{custom}.png")).exists());
        assert!(foreign.is_file());
        assert!(other_variant.is_file(), "正式版不删 dev 的图标");
        // dev 那边清理时同样不碰正式版的。
        let mut dev = TrayIcons::new(tmp.clone(), true);
        assert_eq!(dev.prune(&HashSet::new()), 1);
        assert!(!other_variant.exists());
        assert!(tmp.join(format!("16x16/apps/{name}.png")).is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
