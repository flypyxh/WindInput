//! 状态提示气泡：切换中英/标点/全半角/方案时短暂显示当前状态。
//!
//! 与 Go 版本的 showModeIndicator / CmdStatusShow 对齐（简化版）。
//! 统一到 View 盒模型 + DirectWrite：深色半透明圆角底 + 居中白字，约 1 秒后自动隐藏。

use crate::manager::UiEvent;
use crate::text::dwrite::TextRenderer;
use crate::view::{Align, Edges, View, ViewImage, ViewLayer};
use crate::window::LayeredWindow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use wind_ui_types::{StatusTipAnchor, StatusTipPlacement};

/// 状态提示气泡的鼠标处理器：左键拖动移动位置，右键请求功能菜单。
/// 抓取偏移模型（同 `input_diag_hud::DragState`）：按下时记录光标−窗口左上偏移，
/// 拖动时用该偏移换算新窗口左上，钳制到工作区后 `SetWindowPos`。
struct StatusTipMouse {
    hwnd: crate::sys::HWND,
    events: Sender<UiEvent>,
    /// 是否正在拖动（`WM_LBUTTONDOWN` → true，`WM_LBUTTONUP` → false）。
    dragging: bool,
    /// 按下时光标屏幕坐标与窗口左上角的偏移，拖动时保持该偏移。
    grab_dx: i32,
    grab_dy: i32,
    /// 阴影左/上扩边（由 `show`/`show_fixed` 每次渲染后同步），供换算内容左上坐标。
    margin: (i32, i32),
    /// 拖动中最近一次落定的窗口左上坐标（`WM_MOUSEMOVE` 写入）。
    drag_pin: Option<(i32, i32)>,
    /// 光标是否在气泡窗口内（`WM_MOUSEMOVE` 置 true / `WM_MOUSELEAVE` 置 false）。
    mouse_over: bool,
    /// 上次**物理**光标屏幕坐标——过滤气泡自身出现/移动到光标下方引起的伪 `WM_MOUSEMOVE`。
    /// 与候选窗 `CandidateMouse::last_cursor` 同构，见 [`StatusTipMouse::reset_hover`]。
    last_cursor: (i32, i32),
    /// 是否已注册过 `WM_MOUSELEAVE` 追踪（一次性，收到 LEAVE 后需重新注册）。
    leave_armed: bool,
    /// 本气泡的右键菜单是否打开中。
    menu_open: bool,
}

impl StatusTipMouse {
    /// 交互进行中：拖动 / 光标悬停其上 / 右键菜单打开。
    /// 临时模式的自动隐藏在此期间必须顺延，否则气泡会在用户正操作它时凭空消失。
    fn interacting(&self) -> bool {
        self.dragging || self.mouse_over || self.menu_open
    }

    /// 注册一次性 `WM_MOUSELEAVE` 通知（光标移出窗口时收到）。
    fn arm_leave(&mut self) {
        if self.leave_armed {
            return;
        }
        self.leave_armed = true;
        #[cfg(windows)]
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

    /// 物理移动门控：`cur` 与基线不同才算「用户真的动了鼠标」，此时更新基线并返回 true。
    ///
    /// 抽成独立方法是为了能脱离 Win32 消息与真实光标做守门测试，见本文件 `hover_gate_tests`。
    fn accept_move(&mut self, cur: (i32, i32)) -> bool {
        if cur == self.last_cursor {
            return false;
        }
        self.last_cursor = cur;
        true
    }

    /// 复位悬停状态，并**以当前物理光标位重建基线**。
    ///
    /// 两个调用点，职责不同，缺一不可（与候选窗 `CandidateMouse::reset_hover` 同构）：
    /// - [`StatusTip::mark_visible`]（不可见 → 可见）：**基线在这里才有意义**。判据问的是
    ///   「气泡出现之后鼠标动没动」，基准就必须取自气泡出现那一刻。
    /// - [`StatusTip::hide`]：清掉悬停残留。隐藏时系统未必投出 `WM_MOUSELEAVE`，
    ///   不清则 `mouse_over` 一直挂着 true，之后每次显示都被判成「交互中」而永不自动隐藏；
    ///   `leave_armed` 同样残留会让 [`Self::arm_leave`] 一直早退，`WM_MOUSELEAVE` 再不会来。
    ///   它顺带采的那次基线到下次显示时多半已过时，**不能当作基线的来源**。
    fn reset_hover(&mut self) {
        self.mouse_over = false;
        self.leave_armed = false;
        self.last_cursor = cursor_screen();
    }
}

impl crate::window::WindowMouse for StatusTipMouse {
    fn on_message(
        &mut self,
        _hwnd: crate::sys::HWND,
        msg: u32,
        _wparam: crate::sys::WPARAM,
        _lparam: crate::sys::LPARAM,
    ) -> Option<crate::sys::LRESULT> {
        use crate::sys::{
            GetWindowRect, HWND_TOPMOST, IDC_ARROW, IDC_SIZEALL, LRESULT, LoadCursorW, RECT,
            ReleaseCapture, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetCapture, SetCursor,
            SetWindowPos, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSELEAVE, WM_MOUSEMOVE,
            WM_RBUTTONDOWN, WM_SETCURSOR, clamp_to_work_area,
        };
        match msg {
            WM_MOUSELEAVE => {
                // 光标移出气泡：解除"交互中"，临时模式的自动隐藏随后重新计时。
                self.mouse_over = false;
                self.leave_armed = false;
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let (mx, my) = cursor_screen();
                let (wx, wy) = window_origin(self.hwnd);
                self.grab_dx = mx - wx;
                self.grab_dy = my - wy;
                self.dragging = true;
                unsafe {
                    SetCapture(self.hwnd);
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                // 物理移动门控：气泡自身出现在光标下方、或状态刷新时挪到光标下方，Windows
                // 一样会投 WM_MOUSEMOVE（消息语义是「鼠标与本窗口的相对位置变了」，不是
                // 「用户动了鼠标」），但此时物理光标屏幕坐标不变 → 忽略。
                // 缺这一层，气泡只要弹在静止的鼠标指针上就被判成「悬停中」，
                // `interacting()` 恒真、自动隐藏被无限顺延，气泡常显不消失。
                if !self.accept_move(cursor_screen()) {
                    return Some(LRESULT(0));
                }
                let cur = self.last_cursor;
                // 悬停即视为交互中：光标停在气泡上时不该被自动隐藏抽走。
                self.mouse_over = true;
                self.arm_leave();
                if self.dragging {
                    let (mx, my) = cur;
                    let nx = mx - self.grab_dx;
                    let ny = my - self.grab_dy;
                    let (w, h) = {
                        let mut r = RECT::default();
                        unsafe {
                            if GetWindowRect(self.hwnd, &mut r).is_ok() {
                                ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
                            } else {
                                (0, 0)
                            }
                        }
                    };
                    let (cx, cy) = clamp_to_work_area(nx, ny, w, h);
                    unsafe {
                        let _ = SetWindowPos(
                            self.hwnd,
                            HWND_TOPMOST,
                            cx,
                            cy,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                    self.drag_pin = Some((cx, cy));
                    return Some(LRESULT(0));
                }
                None
            }
            WM_LBUTTONUP => {
                if self.dragging {
                    self.dragging = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    let (wx, wy) = window_origin(self.hwnd);
                    let x = wx + self.margin.0;
                    let y = wy + self.margin.1;
                    let _ = self.events.send(UiEvent::StatusTipMoved { x, y });
                    return Some(LRESULT(0));
                }
                None
            }
            WM_RBUTTONDOWN => {
                let (mx, my) = cursor_screen();
                let _ = self
                    .events
                    .send(UiEvent::RequestStatusMenu { x: mx, y: my });
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                unsafe {
                    let cur = if self.dragging {
                        IDC_SIZEALL
                    } else {
                        IDC_ARROW
                    };
                    if let Ok(c) = LoadCursorW(None, cur) {
                        SetCursor(c);
                    }
                }
                Some(LRESULT(1))
            }
            _ => None,
        }
    }
}

/// 取鼠标屏幕坐标（失败回退 (0,0)）。
fn cursor_screen() -> (i32, i32) {
    let mut pt = crate::sys::POINT::default();
    unsafe {
        let _ = crate::sys::GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

/// 取窗口左上角屏幕坐标（失败回退 (0,0)）。
fn window_origin(hwnd: crate::sys::HWND) -> (i32, i32) {
    let mut r = crate::sys::RECT::default();
    unsafe {
        let _ = crate::sys::GetWindowRect(hwnd, &mut r);
    }
    (r.left, r.top)
}

/// 状态提示气泡窗口
pub struct StatusTip {
    window: LayeredWindow,
    renderer: TextRenderer,
    scale: f32,
    bg: [u8; 4],
    fg: [u8; 4],
    /// 主题位图背景（如 jidian status 的九宫格 panel）+ z 层水印。
    bg_image: Option<ViewImage>,
    layers: Vec<ViewLayer>,
    /// 主题配置的软投影 / 边框 / 圆角（与候选窗一致化）。
    shadow: Option<crate::view::SoftShadow>,
    border: Option<([u8; 4], f32)>,
    radius: Option<f32>,
    /// 已应用主题（DPI 变化时按新缩放重解析几何）。
    theme: Option<wind_theme::Resolved>,
    /// 基准字号（逻辑像素）：跟随主题 behavior.font_size（+ status 节点偏移）。
    base_logical: f32,
    /// 拖动 + 右键菜单处理器（`show`/`show_fixed` 每次渲染后同步其 margin）。
    mouse: Rc<RefCell<StatusTipMouse>>,
    /// 气泡当前是否可见。只为识别「不可见 → 可见」这一沿，好在那一刻重采悬停基线，
    /// 见 [`StatusTip::mark_visible`]。用 `Cell` 是因为 [`StatusTip::hide`] 取 `&self`。
    visible: Cell<bool>,
}

impl StatusTip {
    /// 无主题时的兜底字号（逻辑像素），与候选窗主题默认一致。
    const DEFAULT_FONT_PX: f32 = 18.0;

    pub fn new(events: Sender<UiEvent>) -> Result<Self, String> {
        let scale = Self::dpi_scale();
        let window = LayeredWindow::create(None, 200, 80, "WindInputStatusTip")?;
        let mouse = Rc::new(RefCell::new(StatusTipMouse {
            hwnd: window.hwnd(),
            events,
            dragging: false,
            grab_dx: 0,
            grab_dy: 0,
            margin: (0, 0),
            drag_pin: None,
            mouse_over: false,
            // 构造初值只是占位：首次显示必经 `mark_visible` 重采基线（窗口此刻不可见）。
            last_cursor: (i32::MIN, i32::MIN),
            leave_armed: false,
            menu_open: false,
        }));
        window.register_mouse(mouse.clone());
        let renderer = TextRenderer::new("Microsoft YaHei UI", Self::DEFAULT_FONT_PX * scale)?;
        Ok(Self {
            window,
            renderer,
            scale,
            bg: [40, 40, 40, 235],
            fg: [245, 245, 245, 255],
            bg_image: None,
            layers: Vec::new(),
            shadow: None,
            border: None,
            radius: None,
            theme: None,
            base_logical: Self::DEFAULT_FONT_PX,
            mouse,
            visible: Cell::new(false),
        })
    }

    /// 标记气泡已显示；仅在**不可见 → 可见**这一沿重采悬停基线。
    ///
    /// 基线要回答的是「气泡出现之后鼠标动没动」，基准就只能取自气泡出现那一刻：
    /// - 取自 `hide()`（上一次显示结束）必然过时——这期间用户多半移动过鼠标，气泡再弹出时
    ///   系统投来的进入消息坐标与陈旧基线不同 → 被判成「用户真实移动了鼠标」；
    /// - 取构造初值则进程内第一次显示必不相等，同样误判。
    ///
    /// 反过来，**已可见时不得重采**：连续切换状态时气泡只更新内容/位置，用户可能正悬停其上，
    /// 此时重采基线会连同 `mouse_over` 一起清掉，把真实悬停也抹平。
    fn mark_visible(&self) {
        if !self.visible.get() {
            self.mouse.borrow_mut().reset_hover();
        }
        self.visible.set(true);
    }

    /// DPI 动态化：按显示点所在显示器实时取缩放，变化则更新字号并按新缩放重解析主题几何。
    fn ensure_scale(&mut self, x: i32, y: i32) {
        let sc = crate::dpi::scale_for_point(x, y);
        if (sc - self.scale).abs() > 0.01 {
            self.scale = sc;
            self.renderer.set_base_size(self.base_logical * sc);
            if let Some(t) = self.theme.clone() {
                self.set_theme(&t);
            }
        }
    }

    /// 应用主题（状态气泡底色/文字色 + 位图背景/层）。
    pub fn set_theme(&mut self, theme: &wind_theme::Resolved) {
        self.theme = Some(theme.clone());
        // 先取 palette 兜底，再让 status 节点覆盖。节点色在 resolve 阶段已是
        // 「主题显式值 ⊕ palette 默认」的合成结果（build(n, tk("status_bg"), …)），
        // 与候选窗/菜单保持同一套优先级；节点缺席才落回 token。
        self.bg = theme.color("status_bg", self.bg);
        self.fg = theme.color("status_text", self.fg);
        if let Some(node) = &theme.views.status {
            if let Some(c) = node.bg_color {
                self.bg = c;
            }
            if let Some(c) = node.text_color {
                self.fg = c;
            }
        }
        // 尺寸跟随主题：基准 = behavior.font_size（+ status 节点相对偏移），弃用硬编码。
        let node_off = theme
            .views
            .status
            .as_ref()
            .map(|n| n.font_size)
            .unwrap_or(0.0);
        self.base_logical = (theme.behavior.font_size as f32 + node_off).max(8.0);
        self.renderer.set_base_size(self.base_logical * self.scale);
        if let Some(node) = &theme.views.status {
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

    /// 渲染气泡到 BGRA Vec（离屏化，不依赖 LayeredWindow）。
    /// 返回 `(bgra, w, h, cw, ch, ml, mt, has_shadow)`。
    fn render_bubble_to_bgra(
        &mut self,
        text: &str,
    ) -> (Vec<u8>, u32, u32, u32, u32, u32, u32, bool) {
        let s = self.scale;
        let mut tip = View::leaf(text, self.fg)
            .bg(self.bg)
            .pad(Edges::xy(10.0 * s, 5.0 * s))
            .text_align(Align::Center);
        if let Some((bc, bw)) = self.border {
            tip = tip.border(bc, bw);
        }
        tip.corner_radius = self
            .radius
            .unwrap_or((self.renderer.measure_text("国").height + 10.0 * s) * 0.3);
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
        let cw = (w_f.ceil() as u32).max(32);
        let ch = (h_f.ceil() as u32).max(24);
        let w = cw + ml + mr;
        let h = ch + mt + mb;
        let mut buf = vec![0u8; (w * h * 4) as usize];
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
        (buf, w, h, cw, ch, ml, mt, self.shadow.is_some())
    }

    /// 渲染气泡到窗口缓冲并 update。返回 (内容宽 cw, 内容高 ch, 左 margin ml, 上 margin mt)。
    /// 供 show(跟随光标) 与 show_fixed(固定坐标) 复用，只在定位上分叉。
    fn render_bubble(&mut self, text: &str) -> (u32, u32, u32, u32) {
        let (buf, w, h, cw, ch, ml, mt, _) = self.render_bubble_to_bgra(text);
        self.window.resize(w, h);
        {
            let wbuf = self.window.buffer_mut();
            wbuf[..(w * h * 4) as usize].copy_from_slice(&buf);
        }
        if let Err(e) = self.window.update() {
            tracing::warn!("StatusTip update failed: {}", e);
        }
        (cw, ch, ml, mt)
    }

    /// 显示提示文本：左对齐于光标、默认在光标下方（下方不足则上翻），加用户偏移。
    /// `cy` 为光标底端，`caret_h` 为光标高度（上翻定位用）。
    pub fn show(&mut self, text: &str, cx: i32, cy: i32, caret_h: i32, off_x: i32, off_y: i32) {
        self.ensure_scale(cx, cy);
        let s = self.scale;
        let (cw, ch, ml, mt) = self.render_bubble(text);
        self.mouse.borrow_mut().margin = (ml as i32, mt as i32);
        // 拖动中：跳过重新定位，避免状态刷新把窗口拽回去（拖动本身已用 SetWindowPos 定位）。
        let m = self.mouse.borrow();
        if m.dragging && m.drag_pin.is_some() {
            return;
        }
        drop(m);
        // 左对齐于光标、默认光标下方（下方不足上翻），叠加用户偏移；按工作区钳位。
        // 左对齐而非居中：气泡宽度随 items 勾选项与方案名长度变化，居中会让左边缘随文本
        // 长短左右横跳；左对齐把左边缘钉在 caret 上，且与候选窗 place_window 同基准。
        let gap = (4.0 * s).round() as i32;
        let x = cx + off_x;
        let y = cy + gap + off_y;
        let (px, py) = clamp_below_or_above(x, y, cw, ch, cy, caret_h, gap);
        // 基线采样刻意排在 `show` **之前**：窗口出现不会移动鼠标，两处取值物理上相同，
        // 但排在前面就完全不依赖「`show` 内部不会泵到 WM_MOUSEMOVE」这个前提。
        self.mark_visible();
        // 内容锚点 − 左/上 margin，阴影向四周溢出。
        self.window.show(px - ml as i32, py - mt as i32);
    }

    /// 固定坐标显示（position_mode=fixed）：(fx,fy) 为内容左上屏幕坐标，不随光标。
    /// `caret_*` 只在 (fx,fy)==(0,0)（从未设定过位置）时用于选屏，见 [`fixed_anchor`]。
    pub fn show_fixed(&mut self, text: &str, fx: i32, fy: i32, caret_x: i32, caret_y: i32) {
        let (probe_x, probe_y) = anchor_probe(fx, fy, caret_x, caret_y);
        self.ensure_scale(probe_x, probe_y);
        let (cw, ch, ml, mt) = self.render_bubble(text);
        self.mouse.borrow_mut().margin = (ml as i32, mt as i32);
        // 拖动中：跳过重新定位，避免状态刷新把窗口拽回去（拖动本身已用 SetWindowPos 定位）。
        let m = self.mouse.borrow();
        if m.dragging && m.drag_pin.is_some() {
            return;
        }
        drop(m);
        let (ax, ay) = fixed_anchor(fx, fy, caret_x, caret_y, cw, ch);
        // 同 `show`：基线采样排在 `window.show` 之前。固定位置模式尤其需要——气泡每次都弹在
        // 同一坐标，鼠标停在那儿时若无门控，之后每一次提示都不会自动消失。
        self.mark_visible();
        // 内容锚点 − 左/上 margin，阴影向四周溢出。
        self.window.show(ax - ml as i32, ay - mt as i32);
    }

    /// 锚点显示（C2-33 / GH#148）：前台窗口所在屏的工作区 / 前台窗口边框上的锚点，不跟随光标。
    /// `caret_*` 只在取不到前台窗口时用于选屏，见 [`anchor_frames`]。
    pub fn show_anchor(&mut self, text: &str, anchor: StatusTipAnchor, caret_x: i32, caret_y: i32) {
        let frames = anchor_frames(caret_x, caret_y);
        let (probe_x, probe_y) = anchor_probe_point(frames, caret_x, caret_y);
        // ★ 顺序契约同 `show_fixed`：scale → 尺寸 → 落点。
        self.ensure_scale(probe_x, probe_y);
        let margin = (ANCHOR_MARGIN_DP * self.scale).round() as i32;
        let (cw, ch, ml, mt) = self.render_bubble(text);
        self.mouse.borrow_mut().margin = (ml as i32, mt as i32);
        let m = self.mouse.borrow();
        if m.dragging && m.drag_pin.is_some() {
            return;
        }
        drop(m);
        let (ax, ay) = anchor_target(frames, anchor, (cw, ch), margin, caret_x, caret_y);
        self.mark_visible();
        self.window.show(ax - ml as i32, ay - mt as i32);
    }

    /// 按 [`StatusTipPlacement`] 分派到三种本地显示。`(x, y, caret_h)` 为光标。
    pub fn show_placed(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        caret_h: i32,
        placement: StatusTipPlacement,
    ) {
        match placement {
            StatusTipPlacement::Caret { offset_x, offset_y } => {
                self.show(text, x, y, caret_h, offset_x, offset_y)
            }
            StatusTipPlacement::Fixed { x: fx, y: fy } => self.show_fixed(text, fx, fy, x, y),
            StatusTipPlacement::Anchor(a) => self.show_anchor(text, a, x, y),
        }
    }

    /// 将当前渲染帧保存为 PNG 文件（截图用）。
    pub fn capture_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        self.window.capture_to_file(path)
    }

    /// 将当前渲染帧复制到剪贴板（截图用）。
    pub fn capture_to_clipboard(&self) -> Result<(), String> {
        self.window.capture_to_clipboard()
    }

    /// 用户是否正在与气泡交互（拖动 / 悬停 / 右键菜单打开）。
    /// 临时模式的自动隐藏须在此期间顺延——否则用户正拖着它、或菜单还开着，气泡就消失了。
    pub fn interacting(&self) -> bool {
        self.mouse.borrow().interacting()
    }

    /// 标记本气泡的右键菜单开/关（打开期间抑制自动隐藏）。
    pub fn set_menu_open(&self, open: bool) {
        self.mouse.borrow_mut().menu_open = open;
    }

    /// 当前气泡**内容左上**屏幕坐标（窗口左上 + 阴影扩边）。
    /// 供「固定位置」开关把当前实际位置落盘成 custom_x/custom_y。
    pub fn content_origin(&self) -> (i32, i32) {
        let m = self.mouse.borrow();
        let (wx, wy) = window_origin(m.hwnd);
        (wx + m.margin.0, wy + m.margin.1)
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

    /// 返回状态提示窗口句柄（截图用）。
    #[cfg(windows)]
    pub fn hwnd(&self) -> windows::Win32::Foundation::HWND {
        self.window.hwnd()
    }

    pub fn hide(&self) {
        self.window.hide();
        self.visible.set(false);
        // 悬停残留必须在这里清：窗口隐藏时系统未必投出 WM_MOUSELEAVE，不清则 `mouse_over`
        // 一直为 true，之后每次显示都被判成「交互中」而永不自动隐藏。
        self.mouse.borrow_mut().reset_hover();
    }

    /// host-render：将状态气泡渲染到 BGRA buffer 并计算屏幕坐标（光标下方/上方）。
    /// 返回 `(bgra, w, h, screen_x, screen_y, software_shadow)`；text 为空返回 None。
    #[cfg(windows)]
    pub fn render_frame(
        &mut self,
        text: &str,
        cx: i32,
        cy: i32,
        caret_h: i32,
        off_x: i32,
        off_y: i32,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        if text.is_empty() {
            return None;
        }
        self.ensure_scale(cx, cy);
        let s = self.scale;
        let (buf, w, h, cw, ch, ml, mt, has_shadow) = self.render_bubble_to_bgra(text);
        let gap = (4.0 * s).round() as i32;
        // 与 `show` 同一定位公式（左对齐于光标），两条渲染路径必须一致。
        let x = cx + off_x;
        let y = cy + gap + off_y;
        let (px, py) = clamp_below_or_above(x, y, cw, ch, cy, caret_h, gap);
        Some((buf, w, h, px - ml as i32, py - mt as i32, has_shadow))
    }

    /// host-render：固定坐标模式，(fx, fy) 为内容左上屏幕坐标。
    /// 与 [`Self::show_fixed`] 同一套锚点解析（[`fixed_anchor`]），两条渲染路径必须一致。
    /// 返回 `(bgra, w, h, screen_x, screen_y, software_shadow)`；text 为空返回 None。
    #[cfg(windows)]
    pub fn render_frame_fixed(
        &mut self,
        text: &str,
        fx: i32,
        fy: i32,
        caret_x: i32,
        caret_y: i32,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        if text.is_empty() {
            return None;
        }
        let (probe_x, probe_y) = anchor_probe(fx, fy, caret_x, caret_y);
        self.ensure_scale(probe_x, probe_y);
        let (buf, w, h, cw, ch, ml, mt, has_shadow) = self.render_bubble_to_bgra(text);
        let (ax, ay) = fixed_anchor(fx, fy, caret_x, caret_y, cw, ch);
        Some((buf, w, h, ax - ml as i32, ay - mt as i32, has_shadow))
    }

    /// host-render：锚点模式。与 [`Self::show_anchor`] 同一套落点解析，两条渲染路径必须一致。
    #[cfg(windows)]
    pub fn render_frame_anchor(
        &mut self,
        text: &str,
        anchor: StatusTipAnchor,
        caret_x: i32,
        caret_y: i32,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        if text.is_empty() {
            return None;
        }
        let frames = anchor_frames(caret_x, caret_y);
        let (probe_x, probe_y) = anchor_probe_point(frames, caret_x, caret_y);
        self.ensure_scale(probe_x, probe_y);
        let margin = (ANCHOR_MARGIN_DP * self.scale).round() as i32;
        let (buf, w, h, cw, ch, ml, mt, has_shadow) = self.render_bubble_to_bgra(text);
        let (ax, ay) = anchor_target(frames, anchor, (cw, ch), margin, caret_x, caret_y);
        Some((buf, w, h, ax - ml as i32, ay - mt as i32, has_shadow))
    }

    /// host-render：按 [`StatusTipPlacement`] 分派，与 [`Self::show_placed`] 一一对应。
    #[cfg(windows)]
    pub fn render_frame_placed(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        caret_h: i32,
        placement: StatusTipPlacement,
    ) -> Option<(Vec<u8>, u32, u32, i32, i32, bool)> {
        match placement {
            StatusTipPlacement::Caret { offset_x, offset_y } => {
                self.render_frame(text, x, y, caret_h, offset_x, offset_y)
            }
            StatusTipPlacement::Fixed { x: fx, y: fy } => {
                self.render_frame_fixed(text, fx, fy, x, y)
            }
            StatusTipPlacement::Anchor(a) => self.render_frame_anchor(text, a, x, y),
        }
    }

    /// Linux 外部宿主：渲染位图 + 落位规则，落位由 addon 按工作区做（服务端拿不到屏幕几何）。
    /// 三种定位与 [`Self::show_placed`] 一一对应，公式同源（见 `overlay_linux` 的纯函数）。
    /// `(x, caret_bottom, caret_h)` 为光标；text 为空返回 None。
    #[cfg(all(target_os = "linux", ext_presenter))]
    pub(crate) fn render_overlay(
        &mut self,
        text: &str,
        x: i32,
        caret_bottom: i32,
        caret_h: i32,
        placement: StatusTipPlacement,
    ) -> Option<crate::overlay_linux::Overlay> {
        use crate::overlay_linux as ol;
        if text.is_empty() {
            return None;
        }
        self.ensure_scale(x, caret_bottom);
        let s = self.scale;
        let (buf, w, h, cw, ch, ml, mt, has_shadow) = self.render_bubble_to_bgra(text);
        let place = match placement {
            StatusTipPlacement::Caret { offset_x, offset_y } => ol::status_caret_place(
                x,
                caret_bottom,
                caret_h,
                (offset_x, offset_y),
                ch,
                (4.0 * s).round() as i32,
            ),
            StatusTipPlacement::Fixed { x: fx, y: fy } => {
                ol::status_fixed_place(fx, fy, x, caret_bottom)
            }
            StatusTipPlacement::Anchor(a) => ol::Place::Anchor {
                anchor: ol::status_anchor_code(a),
                margin: (ANCHOR_MARGIN_DP * s).round() as i32,
            },
        };
        Some(ol::Overlay {
            buf,
            width: w,
            height: h,
            content_x: ml,
            content_y: mt,
            content_w: cw,
            content_h: ch,
            software_shadow: has_shadow,
            place,
        })
    }
}

/// 锚点距参照矩形边缘的留白（逻辑像素，按目标屏 DPI 缩放）。
const ANCHOR_MARGIN_DP: f32 = 16.0;

/// 锚点的参照矩形：`(工作区, 前台窗口边框)`，均为 `(left, top, right, bottom)`。
type AnchorFrames = Option<((i32, i32, i32, i32), Option<(i32, i32, i32, i32)>)>;

/// 取锚点的参照矩形（平台层）：前台窗口所在屏的工作区 + 前台窗口可见边框。
/// 取不到前台窗口时退到**光标所在屏**的工作区（无窗口边框 ⇒ 窗口锚点降级为屏幕锚点）；
/// 连工作区都查不到时返回 `None`，调用方按光标点夹回。非 Windows 恒 `None`（macOS 的气泡在
/// `.app` 侧定位，不走本模块）。
#[cfg_attr(not(windows), allow(unused_variables))]
fn anchor_frames(caret_x: i32, caret_y: i32) -> AnchorFrames {
    #[cfg(windows)]
    {
        if let Some(g) = wind_keys::foreground::foreground_window_geometry() {
            return Some((g.work_area, g.frame));
        }
        crate::sys::work_area_at(caret_x, caret_y).map(|w| (w, None))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// `ensure_scale` 的探测点：取工作区中心（锚点都落在这块屏上），查不到时退回光标。
fn anchor_probe_point(frames: AnchorFrames, caret_x: i32, caret_y: i32) -> (i32, i32) {
    match frames {
        Some(((l, t, r, b), _)) => ((l + r) / 2, (t + b) / 2),
        None => (caret_x, caret_y),
    }
}

/// 锚点落点（内容左上）：有参照矩形走纯函数 [`place_anchor`]，否则按光标点夹回工作区。
fn anchor_target(
    frames: AnchorFrames,
    anchor: StatusTipAnchor,
    size: (u32, u32),
    margin: i32,
    caret_x: i32,
    caret_y: i32,
) -> (i32, i32) {
    match frames {
        Some((work, frame)) => place_anchor(anchor, work, frame, size, margin),
        None => crate::sys::clamp_to_work_area(caret_x, caret_y, size.0, size.1),
    }
}

/// 锚点 → 气泡内容左上角（纯几何，未夹回）。`target` 为参照矩形 `(left, top, right, bottom)`，
/// `margin` 为距参照边缘的留白（居中两档不用它）。
///
/// 窗口锚点与同名屏幕锚点公式相同，差别只在调用方给的参照矩形。
fn anchor_origin(
    target: (i32, i32, i32, i32),
    size: (u32, u32),
    margin: i32,
    anchor: StatusTipAnchor,
) -> (i32, i32) {
    use StatusTipAnchor as A;
    let (l, t, r, b) = target;
    let (w, h) = (size.0 as i32, size.1 as i32);
    let center = ((l + r) / 2 - w / 2, (t + b) / 2 - h / 2);
    match anchor {
        A::ScreenCenter | A::WindowCenter => center,
        A::ScreenTopLeft => (l + margin, t + margin),
        A::ScreenTopRight => (r - margin - w, t + margin),
        A::ScreenBottomLeft | A::WindowBottomLeft => (l + margin, b - margin - h),
        A::ScreenBottomRight => (r - margin - w, b - margin - h),
    }
}

/// 锚点定位的完整纯函数：选参照矩形 → [`anchor_origin`] → 夹回工作区。
///
/// - 窗口锚点拿不到窗口边框（`frame = None`，如最小化）⇒ 降级为同位置的屏幕锚点；
/// - 最后一律夹回 `work`：窗口可能一半在屏外（拖出屏幕、跨屏），锚点跟着出去气泡就看不见了。
fn place_anchor(
    anchor: StatusTipAnchor,
    work: (i32, i32, i32, i32),
    frame: Option<(i32, i32, i32, i32)>,
    size: (u32, u32),
    margin: i32,
) -> (i32, i32) {
    let (anchor, target) = match (anchor.is_window(), frame) {
        (true, Some(f)) => (anchor, f),
        (true, None) => (anchor.screen_equivalent(), work),
        (false, _) => (anchor, work),
    };
    let (x, y) = anchor_origin(target, size, margin, anchor);
    crate::sys::clamp_rect_in_bounds(x, y, size.0, size.1, work)
}

/// `custom_x/custom_y` 的「从未设定」哨兵。配置默认即 0，而 (0,0) 恰是**主显示器**左上角
/// —— 固定位置模式下这正是「气泡永远在主屏」的另一条通路：用户刚打开开关还没拖过，
/// 气泡就钉死在主屏角上，且此前 fixed 路径完全不做屏幕钳制，副屏拔掉后再也拖不回来。
const FIXED_UNSET: (i32, i32) = (0, 0);

/// `ensure_scale` 的探测点：未设定固定位置时，DPI 必须按**光标所在屏**取，
/// 否则会拿主屏缩放去排版一个即将落在副屏的气泡（★ 顺序契约：scale → 尺寸 → 落点）。
fn anchor_probe(fx: i32, fy: i32, caret_x: i32, caret_y: i32) -> (i32, i32) {
    if (fx, fy) == FIXED_UNSET {
        (caret_x, caret_y)
    } else {
        (fx, fy)
    }
}

/// 固定位置模式的**内容左上**锚点。
///
/// - 已设定：原样使用用户拖定的绝对坐标（「固定」就该是固定，不跟随焦点换屏）。
/// - 未设定（(0,0) 哨兵）：落到**光标所在屏**，而不是主屏左上角。
///
/// 两种情况都过一次 [`crate::sys::clamp_to_work_area`]：它按落点反查显示器、不预设原点，
/// 所以既不会把副屏的合法负坐标拽回主屏，又能在副屏被拔掉后把气泡拉回可见区域
/// （否则那对绝对坐标已不属于任何显示器，气泡不可见也就无法再拖动纠正）。
fn fixed_anchor(fx: i32, fy: i32, caret_x: i32, caret_y: i32, cw: u32, ch: u32) -> (i32, i32) {
    let (ax, ay) = if (fx, fy) == FIXED_UNSET {
        (caret_x, caret_y)
    } else {
        (fx, fy)
    };
    crate::sys::clamp_to_work_area(ax, ay, cw, ch)
}

/// [`clamp_below_or_above`] 的纯几何内核：在给定工作区 `bounds` 内定位气泡。
///
/// 抽出来是为了可测——原实现整段埋在 `#[cfg(windows)]` + unsafe 里，Linux CI 上一行都跑不到。
/// 要锁住的性质是**坐标不得假设桌面原点为 (0,0)**：主显示器左上角才是虚拟桌面原点，
/// 摆在主屏左侧/上方的副屏工作区坐标为负，任何 `max(0)` 都会把气泡推回主屏
/// （见本模块测试 `left_monitor_negative_x_survives`）。
// 非 Windows 下唯一的调用者在 `#[cfg(windows)]` 分支里，非 test 构建会判其为 dead_code。
#[cfg_attr(not(windows), allow(dead_code))]
fn place_below_or_above(
    x: i32,
    y_below: i32,
    size: (u32, u32),
    caret_y: i32,
    caret_h: i32,
    gap: i32,
    bounds: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (bl, bt, br, bb) = bounds;
    let (wi, hi) = (size.0 as i32, size.1 as i32);
    let (mut nx, mut ny) = (x, y_below);
    // 下方放不下 → 上翻到光标上方；上方也放不下则贴工作区下沿。
    if ny + hi > bb {
        let above = caret_y - caret_h.max(0) - hi - gap;
        ny = if above >= bt { above } else { bb - hi };
    }
    if nx + wi > br {
        nx = br - wi;
    }
    if nx < bl {
        nx = bl;
    }
    if ny < bt {
        ny = bt;
    }
    (nx, ny)
}

/// 把气泡钳制在光标所在显示器工作区：默认 (x, y_below)；下方放不下则上翻到光标上方
/// （光标顶端 = caret_y - caret_h）；左右越界贴边。返回内容盒左上屏幕坐标。
///
/// 显示器按 **caret 点**反查（`MONITOR_DEFAULTTONEAREST`），故天然跟随光标所在屏；
/// 查不到工作区时原样返回，**不得**再做任何以 0 为下界的兜底钳制。
#[cfg_attr(not(windows), allow(unused_variables))]
fn clamp_below_or_above(
    x: i32,
    y_below: i32,
    w: u32,
    h: u32,
    caret_y: i32,
    caret_h: i32,
    gap: i32,
) -> (i32, i32) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        };
        unsafe {
            let pt = POINT { x, y: caret_y };
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let wa = mi.rcWork;
                return place_below_or_above(
                    x,
                    y_below,
                    (w, h),
                    caret_y,
                    caret_h,
                    gap,
                    (wa.left, wa.top, wa.right, wa.bottom),
                );
            }
        }
    }
    (x, y_below)
}

impl StatusTip {
    /// 系统 DPI 缩放因子
    fn dpi_scale() -> f32 {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::Graphics::Gdi::*;
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
}

#[cfg(test)]
mod hover_gate_tests {
    //! 悬停防抖：**「收到 WM_MOUSEMOVE」不等于「用户动了鼠标」**。
    //!
    //! 该消息的语义是「鼠标与本窗口的相对位置变了」，气泡自己弹到静止的指针下方同样满足。
    //! 少了物理坐标门控，气泡一弹在鼠标上就被判成悬停中，`interacting()` 恒真，
    //! 主循环每轮把自动隐藏时刻顺延满一份 —— 表现为气泡常显不消失。

    use super::StatusTipMouse;

    fn mouse_at(baseline: (i32, i32)) -> StatusTipMouse {
        // 接收端就地丢弃：本组测试只验状态迁移，不发任何 UiEvent。
        let (tx, _) = std::sync::mpsc::channel();
        StatusTipMouse {
            hwnd: crate::sys::HWND::default(),
            events: tx,
            dragging: false,
            grab_dx: 0,
            grab_dy: 0,
            margin: (0, 0),
            drag_pin: None,
            mouse_over: false,
            last_cursor: baseline,
            leave_armed: false,
            menu_open: false,
        }
    }

    /// 气泡出现/移动到静止指针下方：坐标与基线相同 → 不算移动。
    #[test]
    fn synthetic_move_at_same_cursor_is_rejected() {
        let mut m = mouse_at((640, 480));
        assert!(!m.accept_move((640, 480)));
    }

    /// 反向对照：真实移动必须放行，并把基线推进到新位置。
    #[test]
    fn real_move_is_accepted_and_advances_baseline() {
        let mut m = mouse_at((640, 480));
        assert!(m.accept_move((641, 480)));
        assert_eq!(m.last_cursor, (641, 480));
        // 放行后停在原地不动的后续消息又该被挡住。
        assert!(!m.accept_move((641, 480)));
    }

    /// `reset_hover` 清悬停残留：隐藏时系统未必投 WM_MOUSELEAVE，不清则下次显示直接被判交互中。
    #[test]
    fn reset_hover_clears_over_and_leave_arm() {
        let mut m = mouse_at((0, 0));
        m.mouse_over = true;
        m.leave_armed = true;
        assert!(m.interacting(), "反向对照：悬停中确实算交互");
        m.reset_hover();
        assert!(!m.mouse_over);
        assert!(
            !m.leave_armed,
            "不清则 arm_leave 永远早退，LEAVE 再也不会来"
        );
        assert!(!m.interacting());
    }

    /// 拖动与右键菜单不受门控影响，仍各自独立地维持「交互中」。
    #[test]
    fn drag_and_menu_still_hold_interacting() {
        let mut m = mouse_at((0, 0));
        m.dragging = true;
        assert!(m.interacting());
        m.dragging = false;
        m.menu_open = true;
        assert!(m.interacting());
    }
}

#[cfg(test)]
mod place_tests {
    //! 气泡落点的纯几何：**坐标不得假设桌面原点为 (0,0)**。
    //!
    //! 此前 `clamp_below_or_above` 出口处有一行 `(nx.max(0), ny.max(0))`，把上面按
    //! `MonitorFromPoint` + `rcWork` 算对的结果又拍回非负象限。摆在主屏左侧/上方的副屏
    //! 坐标整块为负，于是气泡被吸到主屏边缘——多显示器下的「气泡永远在主屏」。
    //! 同一病灶本仓在工具栏上踩过一次（`corner_in_work_area` 内明令禁止 `max(0)`）。
    use super::place_below_or_above;

    /// 主屏（虚拟桌面原点）与摆在它**左侧**的副屏。副屏工作区 x 全为负数——
    /// 这正是 `max(0)` 会把气泡整块吸回主屏的那种布局。
    const MAIN: (i32, i32, i32, i32) = (0, 0, 2560, 1368);
    const LEFT: (i32, i32, i32, i32) = (-1920, 0, 0, 1040);
    /// 摆在主屏**上方**的副屏：y 为负。
    const TOP: (i32, i32, i32, i32) = (0, -1080, 1920, -40);

    const TIP: (u32, u32) = (120, 34);
    const GAP: i32 = 4;

    /// ★ 左侧副屏：负的 x/y 必须原样保留，绝不能被钳到 0（那会跳回主屏）。
    #[test]
    fn left_monitor_negative_x_survives() {
        let (x, y) = place_below_or_above(-1200, 500, TIP, 480, 20, GAP, LEFT);
        assert_eq!((x, y), (-1200, 500), "副屏内的合法负坐标不该被改写");
    }

    /// ★ 上方副屏：整块屏的 y 都是负的，上翻后仍须落在该屏内。
    /// caret 必须取该屏内的坐标（-60），下方放不下（-56 + 34 > -40）故上翻。
    #[test]
    fn top_monitor_negative_y_survives() {
        let caret_y = -60;
        let (x, y) = place_below_or_above(300, caret_y + GAP, TIP, caret_y, 20, GAP, TOP);
        assert_eq!(x, 300);
        assert_eq!(y, caret_y - 20 - 34 - GAP, "应上翻到光标上方，且保持负坐标");
        assert!(
            y >= TOP.1 && y + TIP.1 as i32 <= TOP.3,
            "必须留在上方副屏内"
        );
    }

    /// 左侧副屏右边界是 0：贴右边缘时结果为负，同样不该被 `max(0)` 吃掉。
    #[test]
    fn left_monitor_right_edge_clamps_to_negative() {
        let (x, _) = place_below_or_above(-60, 500, TIP, 480, 20, GAP, LEFT);
        assert_eq!(x, -120, "右溢出应回拉到 right - w = -120，而非 0");
    }

    /// 主屏常规回归：下方放得下就用下方，坐标原样。
    #[test]
    fn main_monitor_below_unchanged() {
        let (x, y) = place_below_or_above(800, 600, TIP, 580, 20, GAP, MAIN);
        assert_eq!((x, y), (800, 600));
    }

    /// 主屏底部：下方放不下 → 上翻到光标上方。
    #[test]
    fn main_monitor_flips_above_near_bottom() {
        let caret_y = 1360;
        let (_, y) = place_below_or_above(800, 1364, TIP, caret_y, 20, GAP, MAIN);
        assert_eq!(y, caret_y - 20 - 34 - GAP);
    }

    /// 上下都放不下（工作区比气泡还矮）时贴下沿，且左上角保持可见。
    #[test]
    fn degenerate_short_work_area_pins_to_bottom() {
        let short = (0, 0, 1920, 30);
        let (_, y) = place_below_or_above(100, 20, TIP, 10, 10, GAP, short);
        assert_eq!(y, short.1, "上翻也放不下 → 贴下沿后再被上边界兜回 top");
    }
}

#[cfg(test)]
mod anchor_tests {
    //! 锚点几何（C2-33 / GH#148）：参照矩形 → 气泡左上角，含边距与夹回。
    use super::{anchor_origin, place_anchor};
    use wind_ui_types::StatusTipAnchor as A;

    /// 主屏工作区（任务栏在底部，40px）。
    const WORK: (i32, i32, i32, i32) = (0, 0, 2560, 1400);
    /// 左侧副屏：整块负坐标。
    const LEFT_WORK: (i32, i32, i32, i32) = (-1920, 0, 0, 1040);
    const TIP: (u32, u32) = (120, 34);
    const M: i32 = 16;

    #[test]
    fn screen_anchors_on_work_area() {
        assert_eq!(anchor_origin(WORK, TIP, M, A::ScreenCenter), (1220, 683));
        assert_eq!(anchor_origin(WORK, TIP, M, A::ScreenTopLeft), (16, 16));
        assert_eq!(
            anchor_origin(WORK, TIP, M, A::ScreenTopRight),
            (2560 - 16 - 120, 16)
        );
        assert_eq!(
            anchor_origin(WORK, TIP, M, A::ScreenBottomLeft),
            (16, 1400 - 16 - 34)
        );
        assert_eq!(
            anchor_origin(WORK, TIP, M, A::ScreenBottomRight),
            (2560 - 16 - 120, 1400 - 16 - 34)
        );
    }

    /// 窗口锚点用窗口边框作参照，而不是工作区。
    #[test]
    fn window_anchors_use_frame() {
        let frame = (400, 300, 1400, 900);
        assert_eq!(
            place_anchor(A::WindowCenter, WORK, Some(frame), TIP, M),
            (900 - 60, 600 - 17)
        );
        assert_eq!(
            place_anchor(A::WindowBottomLeft, WORK, Some(frame), TIP, M),
            (416, 900 - 16 - 34)
        );
    }

    /// 屏幕锚点不看窗口边框——即便给了也不用。
    #[test]
    fn screen_anchor_ignores_frame() {
        let frame = (400, 300, 1400, 900);
        assert_eq!(
            place_anchor(A::ScreenTopLeft, WORK, Some(frame), TIP, M),
            (16, 16)
        );
    }

    /// 边距参与计算：边距为 0 时贴边。
    #[test]
    fn margin_is_applied() {
        assert_eq!(anchor_origin(WORK, TIP, 0, A::ScreenTopLeft), (0, 0));
        assert_eq!(
            anchor_origin(WORK, TIP, 0, A::ScreenBottomRight),
            (2560 - 120, 1400 - 34)
        );
        assert_ne!(
            anchor_origin(WORK, TIP, 0, A::ScreenBottomRight),
            anchor_origin(WORK, TIP, M, A::ScreenBottomRight)
        );
    }

    /// 拿不到窗口边框 ⇒ 降级为同位置的屏幕锚点。
    #[test]
    fn window_anchor_without_frame_degrades_to_screen() {
        assert_eq!(
            place_anchor(A::WindowCenter, WORK, None, TIP, M),
            anchor_origin(WORK, TIP, M, A::ScreenCenter)
        );
        assert_eq!(
            place_anchor(A::WindowBottomLeft, WORK, None, TIP, M),
            anchor_origin(WORK, TIP, M, A::ScreenBottomLeft)
        );
    }

    /// 窗口一半拖出屏幕：锚点落在屏外的部分夹回工作区。
    #[test]
    fn window_anchor_outside_work_area_is_clamped() {
        let frame = (-600, 1200, 400, 1800);
        let (x, y) = place_anchor(A::WindowBottomLeft, WORK, Some(frame), TIP, M);
        assert_eq!((x, y), (0, 1400 - 34), "左、下越界都应夹回工作区内");
    }

    /// 副屏负坐标原样保留，不被拉回主屏（同 place_tests 的同名性质）。
    #[test]
    fn negative_work_area_survives() {
        assert_eq!(
            place_anchor(A::ScreenTopLeft, LEFT_WORK, None, TIP, M),
            (-1920 + 16, 16)
        );
        assert_eq!(
            place_anchor(A::ScreenBottomRight, LEFT_WORK, None, TIP, M),
            (-16 - 120, 1040 - 16 - 34)
        );
    }
}
