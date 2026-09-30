//! 彩色字形光栅：CBDT / sbix 位图、COLR v0 / v1、OpenType-SVG → 预乘 BGRA 位图。
//!
//! - **位图**（Noto Color Emoji 的 CBDT、Apple 的 sbix）：取最接近目标字号的 strike，PNG 用
//!   已有的 `image` 解码，在预乘空间里按目标字号重采样——缩小用面积平均（box），放大用双线性，
//!   不会糊成马赛克也不会在透明边缘出黑边。
//! - **COLR**（Twemoji Mozilla 是 v0，Noto-COLRv1 是 v1）：ttf-parser 的 `paint_color_glyph`
//!   逐层回调，这里把它译成一段 SVG 再交给已有的 resvg 光栅（与 usvg 自己处理 COLR 的做法
//!   同构）。v1 的扫掠渐变（sweep）SVG 表达不了，按色标均值填纯色；线性渐变的第三个控制点
//!   （x2, y2）不参与，混合模式只认 SVG 有的那些。
//! - **OpenType-SVG**：文档直接交 resvg；一份文档装多个字形时按 `id="glyph<N>"` 取节点。
//!
//! 不跟随文字颜色：COLR 的前景色层（调色板索引 0xFFFF）按黑色画。

use std::fmt::Write as _;

use resvg::{tiny_skia, usvg};
use ttf_parser::{GlyphId, RasterImageFormat, RgbaColor, Transform, colr};

use super::store::FaceData;
use super::{GlyphBmp, MAX_GLYPH_DIM};

/// 光栅 `gid` 的彩色字形；该字形没有彩色数据（或解不开）时返回 `None`，调用方退回轮廓。
pub(super) fn render(f: &FaceData, gid: u16, ppem: f32) -> Option<GlyphBmp> {
    let g = GlyphId(gid);
    let t = f.face.tables();
    if t.colr.is_some()
        && f.face.is_color_glyph(g)
        && let Some(b) = colr_glyph(f, g, ppem)
    {
        return Some(b);
    }
    if t.svg.is_some()
        && let Some(b) = svg_glyph(f, g, ppem)
    {
        return Some(b);
    }
    if t.cbdt.is_some() || t.sbix.is_some() {
        return raster_glyph(f, g, ppem);
    }
    None
}

// ── 位图 ──────────────────────────────────────────────────────────────────────────

fn raster_glyph(f: &FaceData, g: GlyphId, ppem: f32) -> Option<GlyphBmp> {
    let img = f
        .face
        .glyph_raster_image(g, ppem.ceil().clamp(1.0, u16::MAX as f32) as u16)?;
    let (sw, sh, src) = match img.format {
        RasterImageFormat::PNG => {
            let rgba = image::load_from_memory_with_format(img.data, image::ImageFormat::Png)
                .ok()?
                .into_rgba8();
            let (w, h) = rgba.dimensions();
            let mut v = rgba.into_raw();
            premultiply(&mut v);
            (w as usize, h as usize, v)
        }
        RasterImageFormat::BitmapPremulBgra32 => {
            let (w, h) = (img.width as usize, img.height as usize);
            let mut v = img.data.get(..w * h * 4)?.to_vec();
            v.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
            (w, h, v)
        }
        _ => return None,
    };
    if sw == 0 || sh == 0 {
        return None;
    }
    let s = ppem / img.pixels_per_em.max(1) as f32;
    // `x` / `y` 是位图左下角相对字形原点的偏移（strike 像素，y 向上）。
    let x0 = img.x as f32 * s;
    let y0 = -(img.y as f32 + sh as f32) * s;
    let left = x0.floor() as i32;
    let top = y0.floor() as i32;
    let w = (x0 + sw as f32 * s).ceil() as i32 - left;
    let h = (y0 + sh as f32 * s).ceil() as i32 - top;
    if w <= 0 || h <= 0 || w > MAX_GLYPH_DIM || h > MAX_GLYPH_DIM {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let data = resample(
        &src,
        (sw, sh),
        s,
        (x0 - left as f32, y0 - top as f32),
        (w, h),
    );
    finish(left, top, w, h, data)
}

fn premultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

/// 一个轴上的重采样权重：目标第 `d` 个像素取哪些源像素、各占多少。`off` 是源图起点在
/// 目标网格里的小数偏移。缩小（`s < 1`）按面积平均，权重和为 1（图边缘处小于 1——超出
/// 源图的部分当透明）；放大按双线性。
fn axis_weights(src: usize, dst: usize, s: f32, off: f32) -> Vec<Vec<(usize, f32)>> {
    (0..dst)
        .map(|d| {
            let mut v = Vec::new();
            if s < 1.0 {
                let a = (d as f32 - off) / s;
                let b = (d as f32 + 1.0 - off) / s;
                let i0 = a.floor().max(0.0) as usize;
                let i1 = (b.ceil().max(0.0) as usize).min(src);
                for i in i0..i1 {
                    let cov = b.min(i as f32 + 1.0) - a.max(i as f32);
                    if cov > 0.0 {
                        v.push((i, cov * s));
                    }
                }
            } else {
                let u = (d as f32 + 0.5 - off) / s - 0.5;
                let i0 = u.floor();
                let t = u - i0;
                for (i, wt) in [(i0, 1.0 - t), (i0 + 1.0, t)] {
                    if i >= 0.0 && (i as usize) < src && wt > 0.0 {
                        v.push((i as usize, wt));
                    }
                }
            }
            v
        })
        .collect()
}

/// 预乘 RGBA 可分离重采样（先横后纵）。在预乘空间里滤波，透明边缘不会被未预乘的色值
/// 染黑。
fn resample(
    src: &[u8],
    (sw, sh): (usize, usize),
    s: f32,
    (ox, oy): (f32, f32),
    (w, h): (usize, usize),
) -> Vec<u8> {
    let wx = axis_weights(sw, w, s, ox);
    let wy = axis_weights(sh, h, s, oy);
    let mut tmp = vec![0f32; sh * w * 4];
    for y in 0..sh {
        let row = &src[y * sw * 4..(y + 1) * sw * 4];
        for (x, ws) in wx.iter().enumerate() {
            let acc = &mut tmp[(y * w + x) * 4..(y * w + x) * 4 + 4];
            for &(i, wt) in ws {
                for c in 0..4 {
                    acc[c] += row[i * 4 + c] as f32 * wt;
                }
            }
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for (y, ws) in wy.iter().enumerate() {
        for x in 0..w {
            let mut acc = [0f32; 4];
            for &(j, wt) in ws {
                let p = &tmp[(j * w + x) * 4..(j * w + x) * 4 + 4];
                for c in 0..4 {
                    acc[c] += p[c] * wt;
                }
            }
            let o = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let a = (acc[3] + 0.5).clamp(0.0, 255.0) as u8;
            o[3] = a;
            for c in 0..3 {
                // 舍入可能让色值比 alpha 多 1，钳住以保持合法预乘。
                o[c] = ((acc[c] + 0.5).clamp(0.0, 255.0) as u8).min(a);
            }
        }
    }
    out
}

// ── COLR ─────────────────────────────────────────────────────────────────────────

fn colr_glyph(f: &FaceData, g: GlyphId, ppem: f32) -> Option<GlyphBmp> {
    let mut p = ColrToSvg {
        face: &f.face,
        body: String::with_capacity(4096),
        path: String::new(),
        next_id: 0,
        transform: Transform::default(),
        outline_transform: Transform::default(),
        stack: Vec::new(),
    };
    f.face
        .paint_color_glyph(g, 0, RgbaColor::new(0, 0, 0, 255), &mut p)?;
    // 字体坐标 y 向上：整体翻转一次，之后 resvg 看到的就是普通的 y 向下 SVG。
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><g transform="matrix(1 0 0 -1 0 0)">{}</g></svg>"#,
        p.body
    );
    let tree = usvg::Tree::from_str(&doc, &usvg::Options::default()).ok()?;
    render_svg(&tree, None, ppem / f.upem)
}

/// ttf-parser 的 COLR 回调 → SVG 片段（移植自 usvg 0.44 `text/colr.rs` 的 `GlyphPainter`）。
struct ColrToSvg<'f> {
    face: &'f ttf_parser::Face<'static>,
    body: String,
    /// 当前轮廓的路径数据（字体单位）。
    path: String,
    next_id: usize,
    transform: Transform,
    /// 取轮廓时生效的变换：轮廓按它画，渐变要换算进它的坐标系。
    outline_transform: Transform,
    stack: Vec<Transform>,
}

fn matrix(t: Transform) -> String {
    format!("matrix({} {} {} {} {} {})", t.a, t.b, t.c, t.d, t.e, t.f)
}

fn rgb(c: RgbaColor) -> String {
    format!("rgb({},{},{})", c.red, c.green, c.blue)
}

/// `outline⁻¹ · paint`：COLR 的渐变坐标在「当前变换」下，而路径在「取轮廓时的变换」下。
fn gradient_transform(outline: Transform, paint: Transform) -> Transform {
    let o = tiny_skia::Transform::from_row(
        outline.a, outline.b, outline.c, outline.d, outline.e, outline.f,
    );
    let p = tiny_skia::Transform::from_row(paint.a, paint.b, paint.c, paint.d, paint.e, paint.f);
    let r = o.invert().unwrap_or_default().pre_concat(p);
    Transform {
        a: r.sx,
        b: r.ky,
        c: r.kx,
        d: r.sy,
        e: r.tx,
        f: r.ty,
    }
}

fn spread(e: colr::GradientExtend) -> &'static str {
    match e {
        colr::GradientExtend::Pad => "pad",
        colr::GradientExtend::Repeat => "repeat",
        colr::GradientExtend::Reflect => "reflect",
    }
}

impl ColrToSvg<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn stops(&mut self, stops: impl Iterator<Item = colr::ColorStop>) {
        for s in stops {
            let _ = write!(
                self.body,
                r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                s.stop_offset,
                rgb(s.color),
                s.color.alpha as f32 / 255.0
            );
        }
    }

    fn fill_path(&mut self, fill: &str, opacity: f32) {
        let _ = write!(
            self.body,
            r#"<path fill="{fill}" fill-opacity="{opacity}" transform="{}" d="{}"/>"#,
            matrix(self.outline_transform),
            self.path
        );
    }

    fn clip_with(&mut self, d: &str) {
        let id = self.id("c");
        let _ = write!(
            self.body,
            r#"<clipPath id="{id}"><path transform="{}" d="{d}"/></clipPath><g clip-path="url(#{id})">"#,
            matrix(self.outline_transform)
        );
    }
}

impl ttf_parser::OutlineBuilder for ColrToSvg<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.path, "M{x} {y}");
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.path, "L{x} {y}");
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.path, "Q{x1} {y1} {x} {y}");
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.path, "C{x1} {y1} {x2} {y2} {x} {y}");
    }
    fn close(&mut self) {
        self.path.push('Z');
    }
}

impl colr::Painter<'static> for ColrToSvg<'_> {
    fn outline_glyph(&mut self, glyph_id: GlyphId) {
        self.path.clear();
        let face = self.face;
        face.outline_glyph(glyph_id, self);
        self.outline_transform = self.transform;
    }

    fn paint(&mut self, paint: colr::Paint<'static>) {
        let coords = self.face.variation_coordinates();
        match paint {
            colr::Paint::Solid(c) => self.fill_path(&rgb(c), c.alpha as f32 / 255.0),
            colr::Paint::LinearGradient(lg) => {
                let id = self.id("g");
                let gt = gradient_transform(self.outline_transform, self.transform);
                let _ = write!(
                    self.body,
                    r#"<linearGradient id="{id}" x1="{}" y1="{}" x2="{}" y2="{}" gradientUnits="userSpaceOnUse" spreadMethod="{}" gradientTransform="{}">"#,
                    lg.x0,
                    lg.y0,
                    lg.x1,
                    lg.y1,
                    spread(lg.extend),
                    matrix(gt)
                );
                self.stops(lg.stops(0, coords));
                self.body.push_str("</linearGradient>");
                self.fill_path(&format!("url(#{id})"), 1.0);
            }
            colr::Paint::RadialGradient(rg) => {
                let id = self.id("g");
                let gt = gradient_transform(self.outline_transform, self.transform);
                let _ = write!(
                    self.body,
                    r#"<radialGradient id="{id}" cx="{}" cy="{}" r="{}" fr="{}" fx="{}" fy="{}" gradientUnits="userSpaceOnUse" spreadMethod="{}" gradientTransform="{}">"#,
                    rg.x1,
                    rg.y1,
                    rg.r1,
                    rg.r0,
                    rg.x0,
                    rg.y0,
                    spread(rg.extend),
                    matrix(gt)
                );
                self.stops(rg.stops(0, coords));
                self.body.push_str("</radialGradient>");
                self.fill_path(&format!("url(#{id})"), 1.0);
            }
            colr::Paint::SweepGradient(sg) => {
                // SVG 没有扫掠渐变：取色标均值填纯色，好过整层消失。
                let (mut n, mut acc) = (0u32, [0u32; 4]);
                for s in sg.stops(0, coords) {
                    let c = s.color;
                    for (a, v) in acc.iter_mut().zip([c.red, c.green, c.blue, c.alpha]) {
                        *a += v as u32;
                    }
                    n += 1;
                }
                if n > 0 {
                    let [r, g, b, a] = acc.map(|v| (v / n) as u8);
                    self.fill_path(&rgb(RgbaColor::new(r, g, b, a)), a as f32 / 255.0);
                }
            }
        }
    }

    fn push_clip(&mut self) {
        let d = std::mem::take(&mut self.path);
        self.clip_with(&d);
        self.path = d;
    }

    fn push_clip_box(&mut self, b: colr::ClipBox) {
        let d = format!(
            "M{} {}L{} {}L{} {}L{} {}Z",
            b.x_min, b.y_min, b.x_max, b.y_min, b.x_max, b.y_max, b.x_min, b.y_max
        );
        self.clip_with(&d);
    }

    fn pop_clip(&mut self) {
        self.body.push_str("</g>");
    }

    fn push_layer(&mut self, mode: colr::CompositeMode) {
        use colr::CompositeMode as M;
        let blend = match mode {
            M::Screen => "screen",
            M::Overlay => "overlay",
            M::Darken => "darken",
            M::Lighten => "lighten",
            M::ColorDodge => "color-dodge",
            M::ColorBurn => "color-burn",
            M::HardLight => "hard-light",
            M::SoftLight => "soft-light",
            M::Difference => "difference",
            M::Exclusion => "exclusion",
            M::Multiply => "multiply",
            M::Hue => "hue",
            M::Saturation => "saturation",
            M::Color => "color",
            M::Luminosity => "luminosity",
            // Porter-Duff 那几种 SVG 的 mix-blend-mode 表达不了，按普通叠加。
            _ => "normal",
        };
        let _ = write!(
            self.body,
            r#"<g style="mix-blend-mode:{blend};isolation:isolate">"#
        );
    }

    fn pop_layer(&mut self) {
        self.body.push_str("</g>");
    }

    fn push_transform(&mut self, t: Transform) {
        self.stack.push(self.transform);
        self.transform = Transform::combine(self.transform, t);
    }

    fn pop_transform(&mut self) {
        if let Some(t) = self.stack.pop() {
            self.transform = t;
        }
    }
}

// ── OpenType-SVG ─────────────────────────────────────────────────────────────────

fn svg_glyph(f: &FaceData, g: GlyphId, ppem: f32) -> Option<GlyphBmp> {
    let doc = f.face.glyph_svg_image(g)?;
    // `from_data` 认 gzip 压缩（SVGZ）的文档。
    let tree = usvg::Tree::from_data(doc.data, &usvg::Options::default()).ok()?;
    let k = ppem / f.upem;
    if doc.start_glyph_id == doc.end_glyph_id {
        return render_svg(&tree, None, k);
    }
    let node = tree.node_by_id(&format!("glyph{}", g.0))?;
    render_svg(&tree, Some(node), k)
}

/// 把 SVG（用户坐标 = 字体单位、y 向下、原点在字形原点）按比例 `k` 光栅成位图，位图范围
/// 取内容的包围盒。`node` = 只画文档里的这一个节点（连同祖先的变换）。
fn render_svg(tree: &usvg::Tree, node: Option<&usvg::Node>, k: f32) -> Option<GlyphBmp> {
    let bbox = match node {
        Some(n) => n.abs_layer_bounding_box()?,
        None => tree.root().abs_layer_bounding_box(),
    };
    let left = (bbox.left() * k).floor() as i32 - 1;
    let top = (bbox.top() * k).floor() as i32 - 1;
    let w = (bbox.right() * k).ceil() as i32 + 1 - left;
    let h = (bbox.bottom() * k).ceil() as i32 + 1 - top;
    if w <= 0 || h <= 0 || w > MAX_GLYPH_DIM || h > MAX_GLYPH_DIM {
        return None;
    }
    let mut pm = tiny_skia::Pixmap::new(w as u32, h as u32)?;
    let ts = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, -left as f32, -top as f32);
    match node {
        None => resvg::render(tree, ts, &mut pm.as_mut()),
        Some(n) => {
            // `resvg::render_node` 只套节点自己的变换、并把节点包围盒挪到原点；把祖先变换与
            // 那一步平移补回去，节点就画在它在整份文档里的位置上。
            let local = match n {
                usvg::Node::Group(g) => g.transform(),
                _ => tiny_skia::Transform::identity(),
            };
            let parents = n
                .abs_transform()
                .pre_concat(local.invert().unwrap_or_default());
            let t = ts.pre_concat(parents).pre_translate(bbox.x(), bbox.y());
            resvg::render_node(n, t, &mut pm.as_mut())?;
        }
    }
    finish(left, top, w as usize, h as usize, pm.take())
}

// ── 收尾 ─────────────────────────────────────────────────────────────────────────

/// 预乘 RGBA → 预乘 BGRA（缓冲区的内存序），裁掉四周全透明的行列。全透明返回 `None`。
fn finish(left: i32, top: i32, w: usize, h: usize, mut data: Vec<u8>) -> Option<GlyphBmp> {
    data.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    let alpha = |x: usize, y: usize| data[(y * w + x) * 4 + 3];
    let rows: Vec<usize> = (0..h)
        .filter(|&y| (0..w).any(|x| alpha(x, y) != 0))
        .collect();
    let (&y0, &y1) = (rows.first()?, rows.last()?);
    let x0 = (0..w).find(|&x| (y0..=y1).any(|y| alpha(x, y) != 0))?;
    let x1 = (0..w)
        .rev()
        .find(|&x| (y0..=y1).any(|y| alpha(x, y) != 0))?;
    let (nw, nh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in y0..=y1 {
        out.extend_from_slice(&data[(y * w + x0) * 4..(y * w + x1 + 1) * 4]);
    }
    Some(GlyphBmp {
        left: left + x0 as i32,
        top: top + y0 as i32,
        w: nw,
        h: nh,
        color: true,
        data: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_weights_cover_each_target_pixel_once() {
        // 4 → 2：每个目标像素各取两个源像素、各半。
        let w = axis_weights(4, 2, 0.5, 0.0);
        assert_eq!(w[0], vec![(0, 0.5), (1, 0.5)]);
        assert_eq!(w[1], vec![(2, 0.5), (3, 0.5)]);
    }

    #[test]
    fn downscale_keeps_opaque_solid_opaque_and_edges_soft() {
        let src = [255u8, 0, 0, 255].repeat(8 * 8);
        let out = resample(&src, (8, 8), 0.5, (0.25, 0.0), (5, 4));
        // 内部像素仍是不透明纯红；左右边缘因起点偏 1/4 像素而半透明。
        let px = |x: usize, y: usize| &out[(y * 5 + x) * 4..(y * 5 + x) * 4 + 4];
        assert_eq!(px(2, 1), &[255, 0, 0, 255]);
        assert!(px(0, 1)[3] > 0 && px(0, 1)[3] < 255, "{:?}", px(0, 1));
        for p in out.chunks(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
        }
    }

    #[test]
    fn upscale_is_bilinear_not_nearest() {
        // 1×2 的黑白两像素放大到 1×4：中间要出现过渡值，而不是 0/255 两档。
        let src = [0u8, 0, 0, 255, 255, 255, 255, 255];
        let out = resample(&src, (2, 1), 2.0, (0.0, 0.0), (4, 1));
        let reds: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert!(reds.iter().any(|&r| r > 20 && r < 235), "{reds:?}");
        assert!(reds.windows(2).all(|w| w[0] <= w[1]), "应单调：{reds:?}");
    }
}
