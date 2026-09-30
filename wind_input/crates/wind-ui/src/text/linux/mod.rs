//! 文本渲染后端（Linux 外部宿主形态，`feature = "linux-host"`）。
//!
//! 与 Windows 版 `text/dwrite.rs`、macOS 版 `text/coretext.rs` 对外契约逐方法对齐：
//! 颜色 `[u8;4]` 是 `[R, G, B, A]`；`buf` 是预乘 BGRA、已含背景、原地做 source-over 叠加
//! （与 CoreText 的 CGBitmapContext 同一语义：透明底上画字，字形像素的 alpha 随之升高）。
//!
//! 管线：fontconfig（运行期 `dlopen`）选字、排回退链 → ttf-parser 读 cmap / 前进宽度 /
//! `kern` / 轮廓 → ab_glyph_rasterizer 出 A8 覆盖度 → 按色合成进缓冲区。
//! 移植自姊妹仓 wind-ui-rust `src/text/linux/`（同一作者，MIT/Apache-2.0），去掉了本仓用
//! 不上的自动折行与斜体，换成本仓的 `TextStyle` / `FontPlan` / 拆字字根契约。
//!
//! # 取舍（已知限制）
//!
//! 走「小依赖」路线：不链接 HarfBuzz / FreeType（C 库，构建机要装 -dev 包），编译期不需要
//! 任何系统库。代价写在明处：
//! - **不做复杂文字整形**：没有连字、阿拉伯文连写、印度系字形重排、蒙古文变形；从左到右
//!   逐字排。中文 / 西文 / 日文 / 韩文候选足够。ZWJ 组合 emoji 拆成单个 emoji 排。
//! - **无 hinting**：灰度抗锯齿 + 4 档亚像素横向定位，观感接近 macOS 而非 ClearType。
//! - 字距只读旧式 `kern` 表；只放在 GPOS 里的字距不生效（CJK 字体几乎都只有 GPOS，
//!   对汉字无影响）。
//! - 字体缺粗体字面时合成（水平加粗）。
//! - **彩色 emoji 画不出来**：Noto Color Emoji（CBDT）、sbix 这类纯位图字体不当候选
//!   （`store::FaceData::has_outlines`）。链上若有带轮廓的单色 emoji 字体（Noto Emoji、
//!   Symbola 等）就用它画单色字形；一个都没有时落回主字体的 `.notdef`（方框）——与
//!   DirectWrite / CoreText 对无字形字符的表现一致，不静默吞字。
//!
//! 测量与绘制走同一条排版路径（[`TextRenderer::layout`]），宽度由构造保证一致。

mod fontconfig;
mod store;

use std::cell::RefCell;
use std::collections::HashMap;

use ab_glyph_rasterizer::{Point as RPoint, Rasterizer, point};

use super::dwrite::{ColorRun, TextMetrics, TextStyle, measure_key, utf16_runs};
use super::script::{FontPlan, ScriptClass, font_runs};
use store::{ChainId, FaceId};

/// 亚像素横向定位的档数（字形位图按笔位小数部分分 4 档各存一份）。
const SUBPIXEL_PHASES: u8 = 4;
/// 字形缓存每一代的条目上限（两代轮换，见 [`GlyphCache::get`]）。
const GLYPH_GEN_MAX: usize = 4096;
/// 测量缓存容量上限，满则整体清空（理由同 dwrite 的 `MEASURE_CACHE_CAP`）。
const MEASURE_CACHE_CAP: usize = 4096;
/// 单个字形位图的边长上限（像素）。超过这个量级的不是 UI 文本，直接不画。
const MAX_GLYPH_DIM: i32 = 2048;

/// 文本渲染器（Linux：fontconfig + ttf-parser + ab_glyph_rasterizer）。
pub struct TextRenderer {
    /// 全局字族（`ui.font.family`）。
    family: String,
    /// 基准字号（View 叶子未显式指定字号时回退）。
    font_size: f32,
    /// 全局默认字重；0 = 常规 400。
    default_weight: i32,
    /// 全局字体方案：默认链 + 按脚本的字体指派，两半都实现（见 [`Self::families_for`]）。
    plan: FontPlan,
    /// 拆字字根字体：私用区码位优先取它的字形。
    chaizi: Option<FaceId>,
    /// (叶子字族, 字重, 脚本类) → 字体仓库里的链。字体仓库本身进程级共享、永不清；
    /// 这里只是省掉每次排版都拼一遍族名表去查仓库。
    chains: RefCell<HashMap<ChainKey, ChainId>>,
    /// 测量缓存（键见 `dwrite::measure_key`）。渲染器级状态（字族 / 方案 / 字重 / 字根）
    /// 不在键里，任何一项变更都整表清空。
    measures: RefCell<HashMap<u64, TextMetrics>>,
    glyphs: RefCell<GlyphCache>,
}

/// [`TextRenderer::chains`] 的键：(叶子字族, 字重, 脚本类)。
type ChainKey = (Option<String>, u16, Option<ScriptClass>);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: FaceId,
    gid: u16,
    /// 字号 × 64（26.6 定点），避免拿浮点做键。
    ppem64: u32,
    phase: u8,
    bold: bool,
}

/// 一个字形的 A8 覆盖度位图。`left`/`top` 是位图左上角相对「笔位整数列 / 基线行」的偏移。
struct GlyphBmp {
    left: i32,
    top: i32,
    w: usize,
    h: usize,
    data: Vec<u8>,
}

/// 两代轮换的字形缓存：到上限时当前代降为上一代，命中上一代的提升回来，只有两代都没
/// 碰过的才真正淘汰——常用字多的界面不会在同一帧里一边清表一边重光栅。
#[derive(Default)]
struct GlyphCache {
    cur: HashMap<GlyphKey, Option<GlyphBmp>>,
    old: HashMap<GlyphKey, Option<GlyphBmp>>,
}

impl GlyphCache {
    fn get(&mut self, key: GlyphKey) -> Option<&GlyphBmp> {
        if !self.cur.contains_key(&key) {
            if self.cur.len() >= GLYPH_GEN_MAX {
                self.old = std::mem::take(&mut self.cur);
            }
            let bmp = match self.old.remove(&key) {
                Some(b) => b,
                None => store::with(|st| rasterize(st.face(key.face), key)),
            };
            self.cur.insert(key, bmp);
        }
        self.cur.get(&key).and_then(|b| b.as_ref())
    }
}

/// 排好的一个字符。
struct Shaped {
    /// `None` = 不绘制（控制字符、零宽格式字符）。
    glyph: Option<(FaceId, u16)>,
    adv: f32,
    /// 按字形**所属字体**判的合成粗体：回退字体可能有真粗体而主字体没有。
    bold: bool,
    /// 该字符在整串里的 UTF-16 下标（分段着色按它取色，同 [`utf16_runs`] 口径）。
    u16_idx: u32,
}

/// 一段文本的排版结果。
struct Layout {
    lines: Vec<Vec<Shaped>>,
    /// 行盒高度（基准字体的 ascent + descent + line gap）。
    line_h: f32,
    /// 行盒顶到基线。
    ascent: f32,
    ppem: f32,
}

impl Layout {
    fn width(&self) -> f32 {
        self.lines
            .iter()
            .map(|l| l.iter().map(|g| g.adv).sum::<f32>())
            .fold(0.0, f32::max)
    }
}

/// 私用区：BMP PUA 与补充私用区 A/B（同 dwrite 的 `pua_runs` 三段）。
fn is_pua(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF_0000..=0x10_FFFD)
}

/// 零宽格式字符（ZWJ、变体选择符等）：不画、不占宽。字体通常没有它们的字形，
/// 不拦下来就会各自画出一个 `.notdef` 方框。
fn is_invisible_format(c: char) -> bool {
    matches!(c as u32, 0x200B..=0x200F | 0x2060..=0x2064 | 0xFE00..=0xFE0F | 0xFEFF)
}

impl TextRenderer {
    /// 创建文本渲染器。字体在首次排版时才解析（fontconfig 初始化要几十毫秒，不压在构造上）。
    pub fn new(font_family: &str, font_size: f32) -> Result<Self, String> {
        Ok(Self {
            family: font_family.to_string(),
            font_size,
            default_weight: 0,
            plan: FontPlan::default(),
            chaizi: None,
            chains: RefCell::new(HashMap::new()),
            measures: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(GlyphCache::default()),
        })
    }

    /// 字族 / 方案 / 字重 / 字根变更后：链与测量都要重算。字形缓存按 (face, gid) 键，
    /// 与这些设置无关，保留。
    fn invalidate(&mut self) {
        self.chains.get_mut().clear();
        self.measures.get_mut().clear();
    }

    /// 基准字号（View 叶子未显式指定字号时回退）。
    pub fn base_size(&self) -> f32 {
        self.font_size
    }

    /// 更新基准字号（DPI 动态变化时调用）。字号在测量键里，无需清缓存。
    pub fn set_base_size(&mut self, size: f32) {
        self.font_size = size;
    }

    /// 切换全局字族（`ui.font.family` 变更时调用）。
    pub fn set_font_family(&mut self, font_family: &str) {
        if self.family != font_family {
            self.family = font_family.to_string();
            self.invalidate();
        }
    }

    /// 换字体方案。默认链与脚本指派都生效（与 DirectWrite 侧一致，比 CoreText 侧多实现了
    /// 脚本指派那一半）。
    pub fn set_font_plan(&mut self, plan: FontPlan) {
        if self.plan != plan {
            self.plan = plan;
            self.invalidate();
        }
    }

    /// 当前字体方案。
    pub fn font_plan(&self) -> &FontPlan {
        &self.plan
    }

    /// 系统字体集里有没有这个家族名（fontconfig `FcFontList`，大小写不敏感）。
    /// `None` = 查不了（名字为空，或 fontconfig 不可用）。通用族名（`sans-serif` 等）
    /// 恒为 `Some(true)`：它们是别名，一定能匹配出字体。
    ///
    /// 不在渲染热路径：调用点都是换字体 / 换方案 / 换主题这类低频事件。
    pub fn family_exists(&self, family: &str) -> Option<bool> {
        let name = family.trim();
        if name.is_empty() {
            return None;
        }
        if store::is_generic_family(name) {
            return Some(true);
        }
        fontconfig::family_exists(name)
    }

    /// 按 face 全名（fontconfig `fullname`，如「Noto Sans CJK SC Bold」）找它所属的家族名
    /// 与字重，给存量配置里的旧 GDI face name 兜底（见 `font_resolve`）。
    pub fn find_face(&self, name: &str) -> Option<(String, i32)> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        fontconfig::find_face(name)
    }

    /// 全局默认字重（`ui.font.weight`，或旧字体名里带的字重；0 = 常规）。
    pub fn set_default_weight(&mut self, weight: i32) {
        if self.default_weight != weight {
            self.default_weight = weight;
            self.invalidate();
        }
    }

    /// 加载拆字字根字体作私用区码位的首选字体。`_family` 是 DirectWrite 家族名（Windows
    /// 侧用），这里直接按文件加载，忽略。`path` 为空 = 撤掉字根字体。
    ///
    /// 先撤旧的再加载：加载失败的结局是「没有字根字体」而不是沿用上一个方案的（理由见
    /// `dwrite.rs` 的同名方法）。
    pub fn set_chaizi_font(&mut self, path: &str, _family: &str) -> Result<(), String> {
        self.chaizi = None;
        self.invalidate();
        if path.is_empty() {
            return Ok(());
        }
        let p = std::path::Path::new(path);
        self.chaizi = store::with(|st| st.load_file(p, 0));
        match self.chaizi {
            Some(_) => Ok(()),
            None => Err(format!("load chaizi font {path} failed")),
        }
    }

    /// 实际字重：叶子显式 > 全局默认 > 400。钳到 fontconfig / CSS 的合法区间。
    fn effective_weight(&self, ts: &TextStyle) -> u16 {
        let w = if ts.weight > 0 {
            ts.weight
        } else if self.default_weight > 0 {
            self.default_weight
        } else {
            400
        };
        w.clamp(1, 1000) as u16
    }

    /// 某个脚本归属的族名链。默认链：叶子字族 > 方案链首 > 全局字族，其后接方案默认链的
    /// 回退项——与 dwrite `create_layout` 的三层合成同序。具名类：方案给它指派的那条链
    /// （「指派决定这一段的 base family → 该段再各自走自己的回退链」，见 `FontPlan`）。
    fn families_for(&self, leaf: Option<&str>, class: Option<ScriptClass>) -> Vec<String> {
        if let Some(c) = class {
            let chain = self.plan.chain_for(Some(c));
            if !chain.is_empty() {
                return chain.to_vec();
            }
        }
        let base = leaf
            .or_else(|| self.plan.base_family())
            .unwrap_or(&self.family)
            .trim();
        let mut v = Vec::with_capacity(1 + self.plan.chain_for(None).len());
        if !base.is_empty() {
            v.push(base.to_string());
        }
        v.extend(self.plan.chain_for(None).iter().skip(1).cloned());
        v
    }

    fn chain_id(
        &self,
        st: &mut store::Store,
        leaf: Option<&str>,
        weight: u16,
        class: Option<ScriptClass>,
    ) -> ChainId {
        let key = (leaf.map(str::to_string), weight, class);
        if let Some(&id) = self.chains.borrow().get(&key) {
            return id;
        }
        let id = st.chain(&self.families_for(leaf, class), weight);
        self.chains.borrow_mut().insert(key, id);
        id
    }

    /// 排版：按 `\n` 硬换行（与 DirectWrite 的 NO_WRAP 同口径：不自动折行，硬换行照旧），
    /// 逐字选字、取前进宽度、同字体相邻字查 `kern`。
    fn layout(&self, text: &str, ts: &TextStyle) -> Layout {
        let ppem = ts.size.max(1.0);
        let weight = self.effective_weight(ts);
        let leaf = ts.family.map(str::trim).filter(|s| !s.is_empty());
        // 脚本指派：只有声明了指派的方案才切段（零配置短路，同 dwrite）。
        let classes: Option<Vec<Option<ScriptClass>>> =
            (!self.plan.declared().is_empty()).then(|| {
                let wide: Vec<u16> = text.encode_utf16().collect();
                let mut per = vec![None; wide.len()];
                for run in font_runs(&wide, self.plan.declared()) {
                    per[run.start..run.start + run.len].fill(run.class);
                }
                per
            });
        store::with(|st| {
            let base = self.chain_id(st, leaf, weight, None);
            let (ascent, descent, gap) = match st.primary(base) {
                Some(id) => {
                    let f = st.face(id);
                    let k = ppem / f.upem;
                    (f.ascent * k, f.descent * k, f.line_gap * k)
                }
                // 系统上一款字体都没有：按常见比例给度量，文字画不出来但布局不塌。
                None => (ppem * 0.8, ppem * 0.2, 0.0),
            };
            let mut lines = Vec::new();
            let mut u16_idx = 0u32;
            for (li, para) in text.split('\n').enumerate() {
                if li > 0 {
                    u16_idx += 1; // 被 split 吃掉的 '\n'
                }
                let mut line: Vec<Shaped> = Vec::with_capacity(para.len());
                for ch in para.chars() {
                    let idx = u16_idx;
                    u16_idx += ch.len_utf16() as u32;
                    if ch == '\t' {
                        // 制表符按 4 个空格宽：UI 文本里它几乎只用于对齐，没有制表位可言。
                        let w = st
                            .glyph_for(base, ' ')
                            .map_or(ppem * 0.25, |(f, g)| advance(st.face(f), g, ppem));
                        line.push(Shaped {
                            glyph: None,
                            adv: w * 4.0,
                            bold: false,
                            u16_idx: idx,
                        });
                        continue;
                    }
                    if ch.is_control() || is_invisible_format(ch) {
                        line.push(Shaped {
                            glyph: None,
                            adv: 0.0,
                            bold: false,
                            u16_idx: idx,
                        });
                        continue;
                    }
                    let chaizi_hit = self.chaizi.filter(|_| is_pua(ch)).and_then(|id| {
                        let g = st.face(id).face.glyph_index(ch)?;
                        (g.0 != 0).then_some((id, g.0))
                    });
                    let glyph = chaizi_hit.or_else(|| {
                        let class = classes
                            .as_ref()
                            .and_then(|c| c.get(idx as usize).copied().flatten());
                        let chain = match class {
                            Some(_) => self.chain_id(st, leaf, weight, class),
                            None => base,
                        };
                        st.glyph_for(chain, ch)
                    });
                    let (adv, bold) = match glyph {
                        Some((f, g)) => {
                            let fd = st.face(f);
                            (advance(fd, g, ppem), weight >= 600 && fd.weight <= 500)
                        }
                        None => (0.0, false),
                    };
                    line.push(Shaped {
                        glyph,
                        adv,
                        bold,
                        u16_idx: idx,
                    });
                }
                // 字距：同一字体里相邻两字查 `kern` 表，调整量并进前一个字的前进宽度。
                for i in 1..line.len() {
                    let (Some((fa, ga)), Some((fb, gb))) = (line[i - 1].glyph, line[i].glyph)
                    else {
                        continue;
                    };
                    if fa != fb {
                        continue;
                    }
                    let f = st.face(fa);
                    if let Some(k) = kerning(f, ga, gb) {
                        line[i - 1].adv += k as f32 * ppem / f.upem;
                    }
                }
                lines.push(line);
            }
            Layout {
                lines,
                line_h: ascent + descent + gap,
                ascent,
                ppem,
            }
        })
    }

    /// 测量文本。宽 = 最宽一行（含尾随空白，同 DirectWrite `widthIncludingTrailingWhitespace`），
    /// 高 = 行数 × 基准字体行高（行高不随行内回退字体起伏，同 dwrite 的 UNIFORM 行距约定）。
    pub fn measure(&self, text: &str, ts: &TextStyle) -> TextMetrics {
        if text.is_empty() {
            return TextMetrics {
                width: 0.0,
                height: ts.size * 1.2,
            };
        }
        let key = measure_key(text, ts);
        if let Some(m) = self.measures.borrow().get(&key) {
            return m.clone();
        }
        let lay = self.layout(text, ts);
        let height = lay.lines.len() as f32 * lay.line_h;
        let m = TextMetrics {
            width: lay.width(),
            height: if height > 0.0 { height } else { ts.size * 1.2 },
        };
        let mut c = self.measures.borrow_mut();
        if c.len() >= MEASURE_CACHE_CAP {
            c.clear();
        }
        c.insert(key, m.clone());
        m
    }

    /// 测量文本尺寸（用基准字号）。
    pub fn measure_text(&self, text: &str) -> TextMetrics {
        self.measure_text_sized(text, self.font_size)
    }

    /// 测量文本尺寸（指定字号，其余取默认）。
    pub fn measure_text_sized(&self, text: &str, size: f32) -> TextMetrics {
        self.measure(text, &TextStyle::new(size))
    }

    /// 绘制文本到预乘 BGRA 缓冲区。`x`/`y` 是文本左上角，基线在 `y + ascent`。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        buf: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: f32,
        y: f32,
        text: &str,
        ts: &TextStyle,
        color: [u8; 4],
    ) -> Result<(), String> {
        self.paint(buf, buf_width, buf_height, x, y, text, ts, |_| color)
    }

    /// 分段着色绘制。`runs` 为空（或全被判为非法区间）时就是 [`Self::draw`]。
    ///
    /// 排版只做一次、颜色只在合成时按字符取——字形位置与 [`Self::measure`] 同源，颜色边界
    /// 不会让宽度漂移。区间重叠时后者覆盖前者，同 DirectWrite 的 `SetDrawingEffect`。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_runs(
        &self,
        buf: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: f32,
        y: f32,
        text: &str,
        ts: &TextStyle,
        color: [u8; 4],
        runs: &[ColorRun],
    ) -> Result<(), String> {
        let spans = utf16_runs(text, runs);
        if spans.is_empty() {
            return self.draw(buf, buf_width, buf_height, x, y, text, ts, color);
        }
        self.paint(buf, buf_width, buf_height, x, y, text, ts, |i| {
            spans
                .iter()
                .rev()
                .find(|(s, l, _)| i >= *s && i < s + l)
                .map_or(color, |r| r.2)
        })
    }

    /// 绘制文本（用基准字号）。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        buf: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: f32,
        y: f32,
        text: &str,
        color: [u8; 4],
    ) -> Result<(), String> {
        self.draw_text_sized(
            buf,
            buf_width,
            buf_height,
            x,
            y,
            text,
            self.font_size,
            color,
        )
    }

    /// 绘制文本（指定字号，其余取默认）。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_sized(
        &self,
        buf: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: [u8; 4],
    ) -> Result<(), String> {
        self.draw(
            buf,
            buf_width,
            buf_height,
            x,
            y,
            text,
            &TextStyle::new(size),
            color,
        )
    }

    /// `draw` / `draw_runs` 的共同实现。`color_at(UTF-16 下标)` 给每个字符的颜色。
    #[allow(clippy::too_many_arguments)]
    fn paint(
        &self,
        buf: &mut [u8],
        buf_width: u32,
        buf_height: u32,
        x: f32,
        y: f32,
        text: &str,
        ts: &TextStyle,
        color_at: impl Fn(u32) -> [u8; 4],
    ) -> Result<(), String> {
        if text.is_empty() || buf_width == 0 || buf_height == 0 || ts.size <= 0.0 {
            return Ok(());
        }
        let (w, h) = (buf_width as usize, buf_height as usize);
        if buf.len() < w * h * 4 {
            return Err("buffer too small".into());
        }
        let lay = self.layout(text, ts);
        let ppem64 = (lay.ppem * 64.0).round() as u32;
        let mut cache = self.glyphs.borrow_mut();
        for (li, line) in lay.lines.iter().enumerate() {
            let line_top = y + li as f32 * lay.line_h;
            // 整行都在缓冲区外的直接跳过。
            if line_top > h as f32 || line_top + lay.line_h < 0.0 {
                continue;
            }
            let baseline = (line_top + lay.ascent).round() as i32;
            let mut pen = x;
            for g in line {
                if let Some((face, gid)) = g.glyph {
                    let color = color_at(g.u16_idx);
                    let fx = pen.floor();
                    if color[3] != 0 {
                        let phase =
                            (((pen - fx) * SUBPIXEL_PHASES as f32) as u8).min(SUBPIXEL_PHASES - 1);
                        let key = GlyphKey {
                            face,
                            gid,
                            ppem64,
                            phase,
                            bold: g.bold,
                        };
                        if let Some(bmp) = cache.get(key) {
                            blit(
                                buf,
                                w,
                                h,
                                bmp,
                                fx as i32 + bmp.left,
                                baseline + bmp.top,
                                color,
                            );
                        }
                    }
                }
                pen += g.adv;
            }
        }
        Ok(())
    }
}

fn advance(f: &store::FaceData, gid: u16, ppem: f32) -> f32 {
    f.face
        .glyph_hor_advance(ttf_parser::GlyphId(gid))
        .map_or(0.0, |a| a as f32 * ppem / f.upem)
}

fn kerning(f: &store::FaceData, a: u16, b: u16) -> Option<i16> {
    let kern = f.face.tables().kern?;
    kern.subtables
        .into_iter()
        .filter(|s| s.horizontal && !s.variable)
        .find_map(|s| s.glyphs_kerning(ttf_parser::GlyphId(a), ttf_parser::GlyphId(b)))
}

/// 光栅一个字形（未命中缓存时调用）。
fn rasterize(f: &store::FaceData, key: GlyphKey) -> Option<GlyphBmp> {
    let gid = ttf_parser::GlyphId(key.gid);
    let bb = f.face.glyph_bounding_box(gid)?;
    let ppem = key.ppem64 as f32 / 64.0;
    let k = ppem / f.upem;
    let dx = key.phase as f32 / SUBPIXEL_PHASES as f32;
    // 合成粗体的加宽量：字号的 1/24，限制在 [0.5, 2] 像素。
    let embolden = if key.bold {
        (ppem / 24.0).clamp(0.5, 2.0)
    } else {
        0.0
    };
    let x0 = bb.x_min as f32 * k + dx;
    let x1 = bb.x_max as f32 * k + dx;
    let y0 = -(bb.y_max as f32) * k;
    let y1 = -(bb.y_min as f32) * k;
    let left = x0.floor() as i32 - 1;
    let top = y0.floor() as i32 - 1;
    let w = x1.ceil() as i32 + 1 + embolden.ceil() as i32 - left;
    let h = y1.ceil() as i32 + 1 - top;
    if w <= 0 || h <= 0 || w > MAX_GLYPH_DIM || h > MAX_GLYPH_DIM {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mut b = OutlineToRaster {
        r: Rasterizer::new(w, h),
        k,
        dx,
        ox: left as f32,
        oy: top as f32,
        start: point(0.0, 0.0),
        cur: point(0.0, 0.0),
    };
    f.face.outline_glyph(gid, &mut b)?;
    let mut data = vec![0u8; w * h];
    b.r.for_each_pixel_2d(|x, y, a| {
        data[y as usize * w + x as usize] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    });
    if embolden > 0.0 {
        embolden_rows(&mut data, w, embolden);
    }
    Some(GlyphBmp {
        left,
        top,
        w,
        h,
        data,
    })
}

/// 水平加粗：每一行把覆盖度向右「涂抹」`amount` 像素（取最大值，不累加以免发糊）。
fn embolden_rows(data: &mut [u8], w: usize, amount: f32) {
    let whole = amount.floor() as usize;
    let frac = amount - whole as f32;
    let mut row = vec![0u8; w];
    for line in data.chunks_mut(w) {
        row.copy_from_slice(line);
        for x in 0..w {
            let mut v = row[x];
            for j in 1..=whole {
                if x >= j {
                    v = v.max(row[x - j]);
                }
            }
            if frac > 0.0 && x > whole {
                v = v.max((row[x - whole - 1] as f32 * frac) as u8);
            }
            line[x] = v;
        }
    }
}

/// ttf-parser 轮廓回调 → 光栅器。字体坐标 y 向上，位图 y 向下。
struct OutlineToRaster {
    r: Rasterizer,
    k: f32,
    dx: f32,
    ox: f32,
    oy: f32,
    start: RPoint,
    cur: RPoint,
}

impl OutlineToRaster {
    fn p(&self, x: f32, y: f32) -> RPoint {
        point(x * self.k + self.dx - self.ox, -y * self.k - self.oy)
    }
}

impl ttf_parser::OutlineBuilder for OutlineToRaster {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.start = p;
        self.cur = p;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.r.draw_line(self.cur, p);
        self.cur = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = self.p(x1, y1);
        let p = self.p(x, y);
        self.r.draw_quad(self.cur, c, p);
        self.cur = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.p(x1, y1);
        let c2 = self.p(x2, y2);
        let p = self.p(x, y);
        self.r.draw_cubic(self.cur, c1, c2, p);
        self.cur = p;
    }
    fn close(&mut self) {
        if self.cur != self.start {
            self.r.draw_line(self.cur, self.start);
        }
        self.cur = self.start;
    }
}

/// 把覆盖度位图按 `[R,G,B,A]` 颜色以 source-over 合成进预乘 BGRA 缓冲，裁到缓冲区内。
fn blit(buf: &mut [u8], bw: usize, bh: usize, g: &GlyphBmp, x0: i32, y0: i32, color: [u8; 4]) {
    let gx0 = x0.max(0);
    let gy0 = y0.max(0);
    let gx1 = (x0 + g.w as i32).min(bw as i32);
    let gy1 = (y0 + g.h as i32).min(bh as i32);
    if gx0 >= gx1 || gy0 >= gy1 {
        return;
    }
    let [r, gg, b, a] = color.map(u32::from);
    for y in gy0..gy1 {
        let src_row = (y - y0) as usize * g.w;
        let dst_row = y as usize * bw * 4;
        for x in gx0..gx1 {
            let cov = g.data[src_row + (x - x0) as usize] as u32;
            if cov == 0 {
                continue;
            }
            let sa = (cov * a + 127) / 255;
            if sa == 0 {
                continue;
            }
            let inv = 255 - sa;
            let i = dst_row + x as usize * 4;
            let d = &mut buf[i..i + 4];
            // BGRA 内存序；源色按 sa 预乘，底色按 (1 - sa) 衰减。
            d[0] = ((b * sa + d[0] as u32 * inv + 127) / 255) as u8;
            d[1] = ((gg * sa + d[1] as u32 * inv + 127) / 255) as u8;
            d[2] = ((r * sa + d[2] as u32 * inv + 127) / 255) as u8;
            d[3] = ((sa * 255 + d[3] as u32 * inv + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests;
