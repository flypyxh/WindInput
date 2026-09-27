//! 候选悬停提示气泡：悬停候选时显示协调器按段列表渲染好的 [`TooltipDoc`]。
//!
//! 与 Go 版本 `wind_input/internal/ui/tooltip.go` 对齐（简化版）。
//! 深色圆角小气泡 + DirectWrite 文本。
//!
//! # 右键命中
//!
//! 整块文本仍是**一个**叶节点（`TooltipDoc::to_plain_text` 画出来的那串），外观与段列表
//! 引入前逐像素相同。命中不靠逐行布局，而是量出文本块的矩形（与 `View` 绘制同一套定位
//! 公式），再按行数均分——渲染器的行距钉成 UNIFORM（见 `text::dwrite` 的
//! `create_layout_with`），每行等高。点中的行经 `TooltipDoc::hit_at_line` 换算回
//! `(段, 原始行)`，随 `RequestTooltipMenu` 交给协调器。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;

use crate::manager::UiEvent;
use crate::sys::{
    GetCursorPos, HWND, LPARAM, LRESULT, POINT, WM_MOUSELEAVE, WM_MOUSEMOVE, WM_RBUTTONDOWN, WPARAM,
};
use crate::text::dwrite::TextRenderer;
use crate::view::{Align, Edges, View, ViewImage, ViewLayer};
use crate::window::{LayeredWindow, WindowMouse};
use wind_ui_types::{TooltipDoc, TooltipHit};

/// 当前显示内容的命中信息（`Tooltip` 写、`TooltipMouse` 读）。
#[derive(Default)]
struct HitState {
    doc: TooltipDoc,
    /// 气泡属于当前页第几个候选（页内下标）。
    candidate: i32,
    /// 文本块在窗口客户区里的矩形（最近一次渲染）。
    text_box: Option<TextBox>,
}

/// 文本块矩形 + 行数。
#[derive(Debug, Clone, Copy, PartialEq)]
struct TextBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    lines: usize,
}

impl TextBox {
    /// 客户区坐标 → 第几行（从 0 起）。落在文本块外（内边距、阴影扩边）为 `None`。
    fn line_at(&self, x: i32, y: i32) -> Option<usize> {
        let (x, y) = (x as f32, y as f32);
        if self.lines == 0
            || x < self.x
            || x >= self.x + self.w
            || y < self.y
            || y >= self.y + self.h
        {
            return None;
        }
        let line_h = self.h / self.lines as f32;
        Some((((y - self.y) / line_h) as usize).min(self.lines - 1))
    }
}

/// `WM_*BUTTON*` 的 lParam → 客户区坐标（低 / 高 16 位，有符号）。
fn client_point(lparam: LPARAM) -> (i32, i32) {
    let v = lparam.0;
    (
        (v & 0xFFFF) as u16 as i16 as i32,
        ((v >> 16) & 0xFFFF) as u16 as i16 as i32,
    )
}

/// 鼠标跟踪器：检测鼠标是否悬停在 tooltip 上（WM_MOUSELEAVE 触发时直接隐藏窗口）；
/// 右键弹出菜单（按段 / 按行复制、上屏，复制全部，截图此窗口）。
struct TooltipMouse {
    /// 仅 Windows 读取（TrackMouseEvent / ShowWindow）；其它平台无 Win32 消息泵。
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: HWND,
    mouse_over: Rc<Cell<bool>>,
    tracking: bool,
    /// 回送协调器的鼠标事件通道（右键请求菜单）。
    events: Sender<UiEvent>,
    /// 菜单打开期间抑制 WM_MOUSELEAVE 自动隐藏：右键弹出菜单后鼠标会移到菜单窗口上，
    /// 触发 WM_MOUSELEAVE，若不抑制 tooltip 会当场消失，菜单就指向一个已不存在的窗口。
    suppress_hide: Rc<Cell<bool>>,
    hits: Rc<RefCell<HitState>>,
}

impl TooltipMouse {
    #[cfg(windows)]
    fn arm_leave(&self) {
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            };
            let mut t = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut t);
        }
    }
    #[cfg(not(windows))]
    fn arm_leave(&self) {}
}

impl WindowMouse for TooltipMouse {
    fn on_message(
        &mut self,
        _hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_MOUSEMOVE => {
                self.mouse_over.set(true);
                if !self.tracking {
                    self.tracking = true;
                    self.arm_leave();
                }
                None
            }
            WM_MOUSELEAVE => {
                self.mouse_over.set(false);
                self.tracking = false;
                // 鼠标离开时直接隐藏（对齐 Go TooltipWindow WM_MOUSELEAVE 行为）；
                // 菜单打开期间抑制——鼠标离开是移向菜单窗口，不是真正离开。
                if !self.suppress_hide.get() {
                    #[cfg(windows)]
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
                        let _ = ShowWindow(self.hwnd, SW_HIDE);
                    }
                }
                None
            }
            WM_RBUTTONDOWN => {
                self.suppress_hide.set(true);
                let (sx, sy) = unsafe {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    (p.x, p.y)
                };
                let (cx, cy) = client_point(lparam);
                let hits = self.hits.borrow();
                let hit: Option<TooltipHit> = hits
                    .text_box
                    .and_then(|b| b.line_at(cx, cy))
                    .and_then(|i| hits.doc.hit_at_line(i));
                let _ = self.events.send(UiEvent::RequestTooltipMenu {
                    x: sx,
                    y: sy,
                    candidate: hits.candidate,
                    hit,
                    doc_fingerprint: hits.doc.fingerprint(),
                });
                None
            }
            _ => None,
        }
    }
}

const FONT_PX: f32 = 13.0;
const BG: [u8; 4] = [60, 60, 64, 240]; // 深灰底（RGBA）
const FG: [u8; 4] = [240, 240, 245, 255];

/// 提示气泡窗口
pub struct Tooltip {
    window: LayeredWindow,
    renderer: TextRenderer,
    scale: f32,
    visible: bool,
    bg: [u8; 4],
    fg: [u8; 4],
    /// 主题位图背景 + z 层（jidian tooltip 吃九宫格 panel + 角标水印）。
    bg_image: Option<ViewImage>,
    layers: Vec<ViewLayer>,
    /// 主题配置的软投影 / 边框 / 圆角（与候选窗一致化）。
    shadow: Option<crate::view::SoftShadow>,
    border: Option<([u8; 4], f32)>,
    radius: Option<f32>,
    /// 已应用主题（DPI 变化时按新缩放重解析几何）。
    theme: Option<wind_theme::Resolved>,
    /// 鼠标是否正悬停在 tooltip 上（由 TooltipMouse 更新）。
    /// hide() 遇到此标志时推迟隐藏，待 WM_MOUSELEAVE 自动触发后真正隐藏。
    mouse_over: Rc<Cell<bool>>,
    /// 右键菜单是否打开中（与 TooltipMouse 共享，供 set_menu_open 写入）。
    suppress_hide: Rc<Cell<bool>>,
    /// 当前内容的命中信息（与 TooltipMouse 共享）。
    hits: Rc<RefCell<HitState>>,
}

impl Tooltip {
    pub fn new(events: Sender<UiEvent>) -> Result<Self, String> {
        let scale = dpi_scale();
        let window = LayeredWindow::create(None, 120, 40, "WindInputTooltip")?;
        let renderer = TextRenderer::new("Microsoft YaHei UI", FONT_PX * scale)?;
        let mouse_over = Rc::new(Cell::new(false));
        let suppress_hide = Rc::new(Cell::new(false));
        let hits = Rc::new(RefCell::new(HitState::default()));
        // 注册鼠标跟踪：鼠标进入 tooltip 时保持可见；WM_MOUSELEAVE 触发时自动隐藏；右键弹出菜单。
        window.register_mouse(Rc::new(RefCell::new(TooltipMouse {
            hwnd: window.hwnd(),
            mouse_over: mouse_over.clone(),
            tracking: false,
            events,
            suppress_hide: suppress_hide.clone(),
            hits: hits.clone(),
        })));
        Ok(Self {
            window,
            renderer,
            scale,
            visible: false,
            bg: BG,
            fg: FG,
            bg_image: None,
            layers: Vec::new(),
            shadow: None,
            border: None,
            radius: None,
            theme: None,
            mouse_over,
            suppress_hide,
            hits,
        })
    }

    /// DPI 动态化：按显示点所在显示器实时取缩放，变化则更新字号并按新缩放重解析主题几何。
    fn ensure_scale(&mut self, x: i32, y: i32) {
        let sc = crate::dpi::scale_for_point(x, y);
        if (sc - self.scale).abs() > 0.01 {
            self.scale = sc;
            self.renderer.set_base_size(FONT_PX * sc);
            if let Some(t) = self.theme.clone() {
                self.set_theme(&t);
            }
        }
    }

    /// 加载拆字字根字体（PUA 字根字符渲染）。`family` 为 DWrite 家族名。失败仅日志，不影响普通提示。
    pub fn set_chaizi_font(&mut self, path: &str, family: &str) {
        if let Err(e) = self.renderer.set_chaizi_font(path, family) {
            tracing::warn!("加载拆字字根字体失败 ({path}): {e}");
        }
    }

    /// 应用主题（tooltip 底色/文字色 + 位图背景/层）。
    pub fn set_theme(&mut self, theme: &wind_theme::Resolved) {
        self.theme = Some(theme.clone());
        // palette 兜底 → tooltip 节点覆盖（节点色已在 resolve 阶段合成 palette 默认）。
        self.bg = theme.color("tooltip_bg", BG);
        self.fg = theme.color("tooltip_text", FG);
        if let Some(node) = &theme.views.tooltip {
            if let Some(c) = node.bg_color {
                self.bg = c;
            }
            if let Some(c) = node.text_color {
                self.fg = c;
            }
            let s = self.scale;
            self.bg_image = crate::theme_assets::rv_image(theme, node.bg_image.as_ref(), s);
            self.layers = crate::theme_assets::rv_layers(theme, &node.layers, s);
            self.shadow = crate::view::SoftShadow::build(
                node.shadow_offset_x,
                node.shadow_offset_y,
                node.shadow_blur,
                node.shadow_spread,
                node.shadow_spread_offset_x,
                node.shadow_spread_offset_y,
                node.shadow_color,
                s,
            );
            self.border = node.border_color.map(|c| {
                (
                    c,
                    node.border_width
                        .map(|d| d.resolve(s, 0.0))
                        .unwrap_or(s)
                        .max(1.0),
                )
            });
            self.radius = node.border_radius.map(|d| d.resolve(s, 0.0));
        } else {
            self.bg_image = None;
            self.layers = Vec::new();
            self.shadow = None;
            self.border = None;
            self.radius = None;
        }
    }

    /// 渲染到 BGRA Vec（离屏化，不依赖 LayeredWindow）。
    /// 返回 `(bgra, w, h, cw, ch, ml, mt, mr, mb, has_shadow)`。
    fn render_to_bgra(
        &mut self,
        text: &str,
    ) -> (Vec<u8>, u32, u32, u32, u32, u32, u32, u32, u32, bool) {
        let s = self.scale;
        let mut tip = View::leaf(text, self.fg)
            .bg(self.bg)
            .pad(Edges::xy(8.0 * s, 4.0 * s))
            .text_align(Align::Center);
        if let Some((bc, bw)) = self.border {
            tip = tip.border(bc, bw);
        }
        tip.corner_radius = self.radius.unwrap_or(5.0 * s);
        if let Some(img) = &self.bg_image {
            tip = tip.bg_image(img.clone());
        }
        if !self.layers.is_empty() {
            tip = tip.layers(self.layers.clone());
        }
        let (ml, mt, mr, mb) = self
            .shadow
            .as_ref()
            .map(|sh| sh.margins())
            .unwrap_or((0, 0, 0, 0));
        tip.layout(ml as f32, mt as f32, &self.renderer);
        let (w_f, h_f) = tip.measured_size();
        self.hits.borrow_mut().text_box = Some(text_box(&self.renderer, text, ml, mt, w_f, h_f));
        let cw = (w_f.ceil() as u32).max(24);
        let ch = (h_f.ceil() as u32).max(20);
        let w = cw + ml + mr;
        let h = ch + mt + mb;
        let n = (w * h * 4) as usize;
        let mut buf = vec![0u8; n];
        if let Some(sh) = &self.shadow {
            sh.paint(
                &mut buf,
                w,
                h,
                ml as f32,
                mt as f32,
                cw as f32,
                ch as f32,
                tip.corner_radius,
            );
        }
        tip.paint(&mut buf, w, h, &self.renderer);
        let has_shadow = self.shadow.is_some();
        (buf, w, h, cw, ch, ml, mt, mr, mb, has_shadow)
    }

    /// 渲染文本到窗口缓冲，返回内容尺寸和阴影 margin。
    /// 返回 `(cw, ch, ml, mt, mr, mb)`；失败返回 None（text 为空时调用方已拦截）。
    fn render_to_window(&mut self, text: &str) -> (u32, u32, u32, u32, u32, u32) {
        let (buf, w, h, cw, ch, ml, mt, mr, mb, _) = self.render_to_bgra(text);
        self.window.resize(w, h);
        {
            let wbuf = self.window.buffer_mut();
            wbuf[..(w * h * 4) as usize].copy_from_slice(&buf);
        }
        let _ = self.window.update();
        (cw, ch, ml, mt, mr, mb)
    }

    /// 横排模式：在候选行下方显示提示，下方不足时上翻到候选行上方。
    /// `anchor_top`/`anchor_bottom` 为候选行的屏幕上/下边界。
    ///
    /// `candidate` 是气泡所属候选的页内下标，右键时随菜单请求带回协调器。
    pub fn show(
        &mut self,
        doc: &TooltipDoc,
        candidate: i32,
        x: i32,
        anchor_top: i32,
        anchor_bottom: i32,
    ) {
        let text = self.set_doc(doc, candidate);
        if text.is_empty() {
            self.hide();
            return;
        }
        self.ensure_scale(x, anchor_bottom);
        let (cw, ch, ml, mt, ..) = self.render_to_window(&text);
        // 内容盒按工作区钳位（下方优先，不足上翻到候选行上方）；窗口原点 = 内容锚点 − 左/上 margin。
        let (px, py) = clamp_to_work_area(x, anchor_top, anchor_bottom, cw, ch);
        self.window.show(px - ml as i32, py - mt as i32);
        self.visible = true;
    }

    /// 竖排模式：在候选窗右侧显示提示，右侧空间不足时改显示在左侧。
    /// `win_left`/`win_right` 为候选窗左右边界（含阴影）屏幕坐标。
    /// `row_top`/`row_bottom` 为悬停候选行的屏幕上/下边界，tooltip 纵向对齐候选行。
    #[allow(clippy::too_many_arguments)]
    pub fn show_beside(
        &mut self,
        doc: &TooltipDoc,
        candidate: i32,
        win_left: i32,
        win_right: i32,
        row_top: i32,
        row_bottom: i32,
    ) {
        let text = self.set_doc(doc, candidate);
        if text.is_empty() {
            self.hide();
            return;
        }
        self.ensure_scale(win_right, row_top);
        let (cw, ch, ml, mt, ..) = self.render_to_window(&text);
        let (px, py) = clamp_beside(win_left, win_right, row_top, row_bottom, cw, ch);
        self.window.show(px - ml as i32, py - mt as i32);
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if self.mouse_over.get() {
            // 鼠标正悬停在 tooltip 上，不立即隐藏；WM_MOUSELEAVE 触发后 TooltipMouse 会自动隐藏窗口。
            return;
        }
        if self.visible {
            self.window.hide();
            self.visible = false;
        }
    }

    /// 记下当前内容（命中换算要用），返回要画的纯文本。
    fn set_doc(&mut self, doc: &TooltipDoc, candidate: i32) -> String {
        let mut h = self.hits.borrow_mut();
        h.doc = doc.clone();
        h.candidate = candidate;
        h.text_box = None;
        doc.to_plain_text()
    }

    /// 将当前渲染帧保存为 PNG 文件（截图用）。
    pub fn capture_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        self.window.capture_to_file(path)
    }

    /// 将当前渲染帧复制到剪贴板（截图用）。
    pub fn capture_to_clipboard(&self) -> Result<(), String> {
        self.window.capture_to_clipboard()
    }

    /// 窗口当前是否可见（查询 Win32 IsWindowVisible）。
    pub fn is_visible(&self) -> bool {
        #[cfg(windows)]
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.window.hwnd()).as_bool()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// 设置右键菜单打开状态：开启时抑制 WM_MOUSELEAVE 自动隐藏；关闭时若鼠标已不在
    /// tooltip 上则立即隐藏（避免菜单关掉后 tooltip 永久赖着不走）。
    pub fn set_menu_open(&mut self, open: bool) {
        self.suppress_hide.set(open);
        if !open && !self.mouse_over.get() {
            self.hide();
        }
    }

    /// 横排 host-render：渲染到 BGRA buffer + 计算屏幕坐标，不操作 LayeredWindow。
    /// 返回 `(bgra, w, h, screen_x, screen_y, software_shadow)`；text 为空返回 None。
    #[cfg(windows)]
    pub fn render_frame(
        &mut self,
        doc: &TooltipDoc,
        candidate: i32,
        x: i32,
        anchor_top: i32,
        anchor_bottom: i32,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        let text = self.set_doc(doc, candidate);
        if text.is_empty() {
            return None;
        }
        self.ensure_scale(x, anchor_bottom);
        let (buf, w, h, cw, ch, ml, mt, _mr, _mb, has_shadow) = self.render_to_bgra(&text);
        let (px, py) = clamp_to_work_area(x, anchor_top, anchor_bottom, cw, ch);
        Some((buf, w, h, px - ml as i32, py - mt as i32, has_shadow))
    }

    /// 竖排 host-render：渲染到 BGRA buffer + 计算候选窗右侧/左侧坐标，不操作 LayeredWindow。
    #[cfg(windows)]
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_beside(
        &mut self,
        doc: &TooltipDoc,
        candidate: i32,
        win_left: i32,
        win_right: i32,
        row_top: i32,
        row_bottom: i32,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        let text = self.set_doc(doc, candidate);
        if text.is_empty() {
            return None;
        }
        self.ensure_scale(win_right, row_top);
        let (buf, w, h, cw, ch, ml, mt, _mr, _mb, has_shadow) = self.render_to_bgra(&text);
        let (px, py) = clamp_beside(win_left, win_right, row_top, row_bottom, cw, ch);
        Some((buf, w, h, px - ml as i32, py - mt as i32, has_shadow))
    }
}

/// 文本块在窗口里的矩形：与 `View::paint` 画叶节点文本的定位公式同一套——内容盒内
/// 水平居中（`Align::Center`）、垂直居中，左右 / 上下内边距对称，故内边距不必出现在式中。
/// `(ml, mt)` 是叶节点的排布原点（软阴影扩边），`(w_f, h_f)` 是叶节点测得尺寸。
fn text_box(renderer: &TextRenderer, text: &str, ml: u32, mt: u32, w_f: f32, h_f: f32) -> TextBox {
    let m = renderer.measure_text(text);
    let (x0, y0) = (ml as f32, mt as f32);
    TextBox {
        x: (x0 + (w_f - m.width) * 0.5).max(x0),
        y: (y0 + (h_f - m.height) * 0.5).max(y0),
        w: m.width,
        h: m.height,
        lines: text.split('\n').count(),
    }
}

fn dpi_scale() -> f32 {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSY, ReleaseDC};
        unsafe {
            let hdc = GetDC(HWND::default());
            let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
            ReleaseDC(HWND::default(), hdc);
            if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 }
        }
    }
    #[cfg(not(windows))]
    {
        1.0
    }
}

/// 竖排模式：tooltip 显示在候选窗**右侧**（空间不足时改左侧），纵向对齐悬停候选行。
/// `win_left`/`win_right` 为候选窗左右边界（含阴影）；`row_top`/`row_bottom` 为候选行上下边界。
#[cfg_attr(not(windows), allow(unused_variables, unused_mut))]
fn clamp_beside(
    win_left: i32,
    win_right: i32,
    row_top: i32,
    _row_bottom: i32,
    w: u32,
    h: u32,
) -> (i32, i32) {
    let gap = 4;
    let (mut px, mut py) = (win_right + gap, row_top);
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        };
        unsafe {
            let pt = POINT {
                x: win_right,
                y: row_top,
            };
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let wa = mi.rcWork;
                let (wi, hi) = (w as i32, h as i32);
                // 右侧放不下则改左侧
                if px + wi > wa.right {
                    px = win_left - gap - wi;
                }
                // 左侧也越界则贴左边
                if px < wa.left {
                    px = wa.left;
                }
                // 纵向：对齐候选行顶，下方越界时上移
                if py + hi > wa.bottom {
                    py = wa.bottom - hi;
                }
                if py < wa.top {
                    py = wa.top;
                }
                return (px, py);
            }
        }
    }
    (px, py)
}

/// 钳位 tooltip 到工作区：默认候选行下方（anchor_bottom + gap）；下方放不下则上翻到候选行
/// **上方**（anchor_top − gap − h，让出整行高度避免遮挡候选）；左右越界贴边。
#[cfg_attr(not(windows), allow(unused_variables, unused_mut))]
fn clamp_to_work_area(x: i32, anchor_top: i32, anchor_bottom: i32, w: u32, h: u32) -> (i32, i32) {
    let gap = 2;
    let (mut nx, mut ny) = (x, anchor_bottom + gap);
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        };
        unsafe {
            let pt = POINT {
                x,
                y: anchor_bottom,
            };
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let wa = mi.rcWork;
                let (wi, hi) = (w as i32, h as i32);
                // 下方放不下 → 上翻到候选行上方（让出整行高度，不遮候选）
                if ny + hi > wa.bottom {
                    ny = (anchor_top - gap - hi).max(wa.top);
                }
                if nx + wi > wa.right {
                    nx = wa.right - wi;
                }
                if nx < wa.left {
                    nx = wa.left;
                }
                if ny < wa.top {
                    ny = wa.top;
                }
                return (nx, ny);
            }
        }
    }
    (nx, ny)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wind_ui_types::{TooltipLine, TooltipSection};

    fn doc() -> TooltipDoc {
        let line = |t: &str, raw| TooltipLine {
            text: t.to_string(),
            raw,
        };
        TooltipDoc {
            sections: vec![
                TooltipSection {
                    title: Some("完整原文".into()),
                    inline: false,
                    // 一条原始行折成两条显示行
                    lines: vec![line("一二三四五", 0), line("六七", 0)],
                },
                TooltipSection {
                    title: Some("拼音".into()),
                    inline: false,
                    lines: vec![line("你：nǐ", 0), line("好：hǎo/hào", 1)],
                },
            ],
        }
    }

    fn tooltip() -> Tooltip {
        let (tx, _rx) = std::sync::mpsc::channel();
        Tooltip::new(tx).expect("创建气泡窗口（非 Windows 下是内存桩）")
    }

    /// ★ 外观零回归的约束在「画的是同一串文本」：渲染路径没改（仍是 `render_to_bgra(&str)`
    /// 画一个叶节点），故只要 `TooltipDoc` 扁平化出的文本与旧版逐字节相同，像素就相同。
    /// 像素本身不在这里比——同一串文本自己比自己证明不了任何事。
    #[test]
    fn doc_flattens_to_the_legacy_string() {
        let legacy = "[完整原文]\n一二三四五\n六七\n[拼音]\n你：nǐ\n好：hǎo/hào";
        assert_eq!(doc().to_plain_text(), legacy);
    }

    /// 命中换算「按行数均分」的前提：多行文本的高度 = 行数 × 单行高度，含 CJK 与 emoji 行
    /// （渲染器行距钉成 UNIFORM，回退字体再高也不撑高行框）。只有真 DirectWrite 能验证；
    /// mock 后端的高度本就是按行数算的。
    #[cfg(windows)]
    #[test]
    fn multiline_height_is_lines_times_line_height() {
        let r = TextRenderer::new("Microsoft YaHei UI", FONT_PX).expect("DirectWrite");
        let one = r.measure_text("A").height;
        for text in [
            "A\n你好\n😀",
            "😀\nA",
            "你\n好\n吗\n👨\u{200D}👩\u{200D}👧",
            "[拼音]\n你：nǐ\n好：hǎo/hào",
        ] {
            let n = text.split('\n').count() as f32;
            let h = r.measure_text(text).height;
            assert!(
                (h - one * n).abs() < 0.5,
                "{text:?}: 高 {h}，应为 {n} × {one}"
            );
        }
    }

    /// 命中换算：标题行 / 内容行 / 折行后的第二条显示行 / 内边距。
    ///
    /// 在非 Windows（mock 渲染器）下只验证换算的算术：矩形、按行均分、行号到 `(段, 原始行)`
    /// 的映射。「每行真的等高」由上面的 `multiline_height_is_lines_times_line_height` 在
    /// Windows 上兜底。
    #[test]
    fn hit_maps_client_point_to_section_and_raw_line() {
        let mut t = tooltip();
        let text = t.set_doc(&doc(), 3);
        let _ = t.render_to_bgra(&text);
        let h = t.hits.borrow();
        let b = h.text_box.expect("渲染后应记下文本块");
        assert_eq!(b.lines, 6);
        let line_h = b.h / 6.0;
        let at = |i: usize| {
            let y = (b.y + line_h * (i as f32 + 0.5)) as i32;
            let x = (b.x + b.w * 0.5) as i32;
            b.line_at(x, y).and_then(|l| h.doc.hit_at_line(l))
        };
        let hit = |section, raw_line| Some(TooltipHit { section, raw_line });
        assert_eq!(at(0), hit(0, None), "标题行");
        assert_eq!(at(1), hit(0, Some(0)));
        assert_eq!(at(2), hit(0, Some(0)), "折行的第二条显示行指回同一原始行");
        assert_eq!(at(3), hit(1, None));
        assert_eq!(at(5), hit(1, Some(1)));
        // 内边距：文本块上方、左侧都不算命中。
        assert_eq!(b.line_at(b.x as i32, (b.y - 1.0) as i32), None);
        assert_eq!(b.line_at((b.x - 1.0) as i32, (b.y + 1.0) as i32), None);
        assert_eq!(b.line_at(b.x as i32, (b.y + b.h + 1.0) as i32), None);
        assert_eq!(h.candidate, 3);
    }

    #[test]
    fn client_point_is_signed_16_bit_pairs() {
        assert_eq!(client_point(LPARAM((20 << 16) | 10)), (10, 20));
        assert_eq!(client_point(LPARAM(0xFFFF_FFFF)), (-1, -1));
    }
}
