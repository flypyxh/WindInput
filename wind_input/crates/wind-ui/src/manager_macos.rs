//! macOS host-render forwarder：把协调器下发的 `UiCommand` 转成 push 帧。
//!
//! Windows 侧 `ui_thread` 直接驱动 LayeredWindow 呈现；macOS 侧无进程内窗口，
//! 候选/工具栏/提示统一光栅化进 POSIX SHM，再经 push 管道通知 .app 端取帧呈现。
//! 本模块在 `ext_presenter` 下编译：macOS，以及 Linux 的 `linux-host` 形态（宿主是 Fcitx5 addon）。
//! 软键盘、全局热键这两块是 macOS 专属（AppKit / Carbon），Linux 不做，命令落 `other` 臂。
//!
//! 状态气泡 / Toast / 悬停提示两平台分道：macOS `.app` 原生渲染，这里只发文本 + 配色；
//! Linux addon 不排字，这里光栅化后走 `CMD_OVERLAY_FRAME`（见 `crate::overlay_linux`）。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use crate::candidate_window::{CandidateWindow, CandidateWindowConfig};
use crate::manager::{UiCommand, UiEvent};
#[cfg(target_os = "macos")]
use crate::softkeyboard_host_macos as sk;
use crate::toast::{ToastKind, ToastPosition};
use wind_bridge::HostRenderSink;
use wind_bridge::shared_memory_posix::PosixSharedMemory;
use wind_ipc::codec::*;
use wind_ipc::protocol::*;
use wind_ui_types::TooltipDoc;
#[cfg(target_os = "macos")]
use wind_ui_types::{StatusTipAnchor, StatusTipPlacement};

const SHM_MAX: usize = MAX_SHARED_RENDER_SIZE;

/// 气泡锚点 → wire 编码（[`wind_ipc::protocol::status_anchor`]）。
#[cfg(target_os = "macos")]
fn status_anchor_code(a: StatusTipAnchor) -> i32 {
    use StatusTipAnchor as A;
    match a {
        A::ScreenCenter => status_anchor::SCREEN_CENTER,
        A::ScreenTopLeft => status_anchor::SCREEN_TOP_LEFT,
        A::ScreenTopRight => status_anchor::SCREEN_TOP_RIGHT,
        A::ScreenBottomLeft => status_anchor::SCREEN_BOTTOM_LEFT,
        A::ScreenBottomRight => status_anchor::SCREEN_BOTTOM_RIGHT,
        A::WindowCenter => status_anchor::WINDOW_CENTER,
        A::WindowBottomLeft => status_anchor::WINDOW_BOTTOM_LEFT,
    }
}

/// 把 `Rgba` 编成 wire 用的 `#RRGGBBAA`（Swift `NSColor(windHex:)` 认 6/8 位）。
fn hex(c: wind_theme::Rgba) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

/// 取「palette token 兜底 → 视图节点覆盖」的底色/文字色，与 Windows 侧各窗口
/// `set_theme` 的优先级一致（节点色在 resolve 阶段已合成 token 默认）。
fn node_colors(
    theme: &wind_theme::Resolved,
    bg_token: &str,
    fg_token: &str,
    node: Option<&wind_theme::RvNode>,
    fallback: (wind_theme::Rgba, wind_theme::Rgba),
) -> (String, String) {
    let mut bg = theme.color(bg_token, fallback.0);
    let mut fg = theme.color(fg_token, fallback.1);
    if let Some(n) = node {
        if let Some(c) = n.bg_color {
            bg = c;
        }
        if let Some(c) = n.text_color {
            fg = c;
        }
    }
    (hex(bg), hex(fg))
}

/// 把截图的慢活扔到后台线程。
///
/// **forwarder 线程绝不能在这里阻塞**：它按序处理**全部** `UiCommand`（候选更新、主题、
/// 工具栏…），卡住多久，输入法就有多久不响应。而截图这条路上的三件事都不快：
///   · PNG 编码（候选窗 Retina 下上千像素宽）；
///   · 写临时文件；
///   · `copy_bgra_to_clipboard` 在 macOS 要 **spawn 一个 `osascript` 进程**（几十到
///     几百毫秒）——服务进程不链接 AppKit，拿不到 NSPasteboard，只能走这条外部路。
/// 三者叠起来实测能让候选窗肉眼可见地冻住。
///
/// 每次截图起一个线程（而不是常驻工作线程）是刻意的：截图是用户偶发动作，频率以分钟计，
/// 一个线程的创建成本完全淹没在上面那几十毫秒里，不值得为它引入一条队列和它的生命周期。
fn spawn_screenshot_work(tag: &'static str, work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new()
        .name(format!("windinput-screenshot-{tag}"))
        .spawn(work)
    {
        // 起不了线程（资源耗尽）→ 本次截图丢弃。**不能退回同步执行**：`Builder::spawn`
        // 已经把闭包吃掉了，这里拿不回来；何况系统连线程都起不出来时，更不该由 forwarder
        // 线程去扛一个几百毫秒的 osascript。记一条 warn，用户会发现"截了没反应"时有据可查。
        tracing::warn!("截图线程创建失败({tag}): {e}，本次截图已放弃");
    }
}

/// 编码一帧 Toast。**自由函数而非方法**：截图那条路要在后台线程发 Toast（见
/// `TakeScreenshot` 分支），那边只拿得到 `sink` 与两个配色串的克隆，碰不到 `&self`。
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "linux", ext_presenter), allow(dead_code))]
fn toast_frame(
    bg: &str,
    fg: &str,
    text: &str,
    position: ToastPosition,
    kind: ToastKind,
    duration_ms: i32,
    accent_override: Option<[u8; 4]>,
) -> Vec<u8> {
    let pos = match position {
        ToastPosition::Center => "center",
        ToastPosition::TopCenter => "top_center",
        ToastPosition::BottomCenter => "bottom_center",
        ToastPosition::TopLeft => "top_left",
        ToastPosition::TopRight => "top_right",
        ToastPosition::BottomLeft => "bottom_left",
        ToastPosition::BottomRight => "bottom_right",
    };
    // accent 取 ToastKind 对应强调色（与 toast.rs ToastKind::accent 一致）；
    // `ui.toast(color=…)` 给了自定义色则压过它。帧里 accent 本就是 hex 串，
    // 故本平台与 Windows 一样支持自定义色，**不必降级**。
    // alpha 段丢弃：.app 侧按 6 位 hex 解析，多出的两位会被整串判为非法而落回内置色。
    let custom = accent_override.map(|c| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
    let accent = match (&custom, kind) {
        (Some(c), _) => c.as_str(),
        (None, ToastKind::Info) => "#409EFF",
        (None, ToastKind::Success) => "#52C46E",
        (None, ToastKind::Error) => "#F56C6C",
    };
    encode_toast_show("", text, bg, fg, accent, pos, duration_ms, 0)
}

/// macOS 侧提示类窗口的配色快照。.app 原生渲染 tooltip / 状态气泡 / Toast，
/// 拿不到 `Resolved`，故在此把主题求值成 hex 串随帧下发；空串 = .app 用内置默认。
#[derive(Default, Clone)]
#[cfg_attr(all(target_os = "linux", ext_presenter), allow(dead_code))]
struct TipColors {
    tooltip_bg: String,
    tooltip_fg: String,
    status_bg: String,
    status_fg: String,
    toast_bg: String,
    toast_fg: String,
}

pub struct Forwarder {
    win: CandidateWindow,
    shm: Option<PosixSharedMemory>,
    sink: Arc<dyn HostRenderSink>,
    suffix: String,
    /// 提示类窗口配色（`SetTheme` 时求值一次，随 show 帧下发）。
    tips: TipColors,
    /// 拆字字根字体绝对路径（`SetTooltipChaiziFont` 下发）。缺它则 .app 侧
    /// PUA 字根渲染成方框——对齐 Windows 64a2b50 修的同一问题。
    #[cfg_attr(all(target_os = "linux", ext_presenter), allow(dead_code))]
    chaizi_font: String,
    /// 回协调器的事件通道（全局热键触发等）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ev_tx: Sender<UiEvent>,
    /// 候选窗当前是否有帧在显示。外观类命令（主题/字号/布局…）只在**显示中**才重推帧。
    visible: bool,
    /// 最近一帧随附的 hover tooltip。重推时须一并带上，否则换主题会把气泡弄丢。
    ///
    /// ★ 存结构化的 `TooltipDoc` 而不是算好颜色的结果：换主题 / 换明暗时 `handle` 用它重推
    /// 当前帧（`affects_appearance` 分支），每次推送都按**当前**主题现算分段颜色——存颜色的话，
    /// 重推出去的气泡仍是旧主题的颜色。
    last_tip: Option<Arc<TooltipDoc>>,
    /// 当前主题（`SetTheme` 留一份），推气泡时现算分段颜色用。
    theme: Option<Box<wind_theme::Resolved>>,
    /// Linux：状态气泡 / Toast / tooltip 的光栅浮层（macOS 由 `.app` 原生渲染，无此字段）。
    #[cfg(all(target_os = "linux", ext_presenter))]
    overlays: crate::overlay_linux::Overlays,
}

impl Forwarder {
    pub fn new(ev_tx: Sender<UiEvent>, sink: Arc<dyn HostRenderSink>, suffix: String) -> Self {
        // CandidateWindow 在非 Windows 是纯光栅 mock，不产生鼠标事件；共用同一 tx 即可。
        let win = CandidateWindow::new(CandidateWindowConfig::default(), ev_tx.clone())
            .expect("create candidate window (mock/raster host)");
        #[cfg(all(target_os = "linux", ext_presenter))]
        let overlays = crate::overlay_linux::Overlays::new(ev_tx.clone(), suffix.clone());
        Self {
            win,
            shm: None,
            sink,
            suffix,
            tips: TipColors::default(),
            chaizi_font: String::new(),
            ev_tx,
            visible: false,
            last_tip: None,
            theme: None,
            #[cfg(all(target_os = "linux", ext_presenter))]
            overlays,
        }
    }

    fn ensure_shm(&mut self) -> Option<&mut PosixSharedMemory> {
        if self.shm.is_none() {
            match PosixSharedMemory::create(&wind_bridge::endpoint::shm_name(&self.suffix), SHM_MAX)
            {
                Ok(s) => self.shm = Some(s),
                Err(e) => {
                    tracing::warn!("create SHM failed: {}", e);
                    return None;
                }
            }
        }
        self.shm.as_mut()
    }

    /// 该命令是否只改**外观**而不改内容。
    ///
    /// 这类命令在 Windows 上由窗口自己重绘，macOS 却是「渲染在服务进程、像素经 SHM 推给
    /// `.app`」——不主动重推一帧，已经显示着的候选窗就停在旧样子，直到下一次按键 / 鼠标
    /// 悬停触发 `UpdateCandidates` 才更新。表现为「菜单里换了主题要把鼠标移到候选项上才生效」。
    fn affects_appearance(cmd: &UiCommand) -> bool {
        matches!(
            cmd,
            UiCommand::SetTheme(_)
                | UiCommand::SetCandidateLayout { .. }
                | UiCommand::SetCandidateTextFamily(_)
                | UiCommand::SetPreeditEmbedded(_)
                | UiCommand::SetCandidateFontSize(_)
                | UiCommand::SetCandidateFont { .. }
                | UiCommand::SetCandidateMinSize { .. }
                | UiCommand::SetCandidateFlipWhenAbove(_)
                | UiCommand::SetCandidateSwapWhenAbove(_)
                | UiCommand::SetPagerInPreedit(_)
                | UiCommand::SetCandidateOffset { .. }
                | UiCommand::SetCandidateShadow(_)
                | UiCommand::SetPagerDisplay { .. }
                | UiCommand::SetPageNumberDisplay { .. }
        )
    }

    pub fn handle(&mut self, cmd: UiCommand) {
        let repaint = Self::affects_appearance(&cmd);
        self.handle_inner(cmd);
        // 只在显示中才重推：不可见时重推会把一个空窗口推上屏。
        if repaint && self.visible {
            let tip = self.last_tip.clone();
            self.push_current_frame(tip);
        }
    }

    fn handle_inner(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::UpdateCandidates {
                preedit,
                preedit_caret,
                preedit_host_owned,
                mode_label,
                candidates,
                selected,
                hover,
                page,
                total_pages,
                caret_x,
                caret_y,
                caret_height,
                caret_valid,
                fixed,
                fixed_x,
                fixed_y,
            } => {
                tracing::debug!(
                    "forwarder UpdateCandidates: n={} preedit={:?} caret=({},{},{}) valid={}",
                    candidates.len(),
                    preedit,
                    caret_x,
                    caret_y,
                    caret_height,
                    caret_valid
                );
                // hover tooltip（反查码在 CandidateItem.tooltip）。
                let tip = if hover >= 0 {
                    candidates
                        .get(hover as usize)
                        .map(|c| c.tooltip.clone())
                        .filter(|d| !d.sections.is_empty())
                } else {
                    None
                };
                // 编码区归宿主自绘时（`preedit_display = app_inline`），候选窗不重复画一遍
                // ——数据恒下发、显示与否看这个标志，与 Windows 侧 `manager.rs` 同一判据。
                let preedit = if preedit_host_owned {
                    String::new()
                } else {
                    preedit
                };
                self.win.update(
                    &preedit,
                    preedit_caret,
                    &mode_label,
                    candidates,
                    selected,
                    hover,
                    page,
                    total_pages,
                );
                self.win
                    .set_position(caret_x, caret_y, caret_height, caret_valid);
                // 固定位置：坐标由服务进程算定（`render_frame` 走 place_fixed 分支），
                // 帧里带 FLAG_ABSOLUTE_POS 告诉 `.app` 照搬、别再按光标翻转。
                self.win
                    .set_fixed_position(fixed.then_some((fixed_x, fixed_y)));
                self.push_current_frame(tip);
            }
            UiCommand::HideCandidates => self.hide_frame(),
            UiCommand::UpdateToolbar(s) => {
                let mut flags = STATUS_TOOLBAR_VISIBLE;
                if s.chinese_mode {
                    flags |= STATUS_CHINESE_MODE;
                }
                if s.full_width {
                    flags |= STATUS_FULL_WIDTH;
                }
                if s.chinese_punct {
                    flags |= STATUS_CHINESE_PUNCT;
                }
                let mode = if s.chinese_mode { 1 } else { 0 };
                self.sink
                    .push_frame(&encode_mode_status(flags, mode, &s.icon_label));
            }
            UiCommand::HideToolbar => {
                self.sink.push_frame(&encode_mode_status(0, 0, ""));
            }
            UiCommand::ShowStatusTip {
                text,
                x,
                y,
                caret_height,
                duration_ms,
                placement,
            } => {
                // Linux：光栅化成浮层帧，落位（含锚点）交 addon 按工作区做。
                #[cfg(all(target_os = "linux", ext_presenter))]
                self.overlays.show_status(
                    &*self.sink,
                    &text,
                    x,
                    y,
                    caret_height,
                    duration_ms,
                    placement,
                );
                #[cfg(target_os = "macos")]
                {
                    // wire 传屏幕 (x,y) + 锚点编码；跟随光标 / 固定坐标在此算定最终 (x,y)。
                    // 跟随光标时 y 是 caret 顶端，须 +caret_height 落到 caret 底端下方，否则气泡
                    // 贴在 caret 顶端盖住输入位（与候选窗 render_frame 的 y+caret_height 对齐）。
                    // 锚点由 `.app` 在焦点所在屏上落位（前台窗口 / 屏幕几何只有那边拿得到），
                    // 此时 (x,y) 只是选屏参考，照跟随光标的算法给出光标底端。
                    let (fx, fy, anchor) = match placement {
                        StatusTipPlacement::Fixed { x: px, y: py } => (px, py, status_anchor::NONE),
                        StatusTipPlacement::Caret { offset_x, offset_y } => (
                            x + offset_x,
                            y + offset_y + caret_height,
                            status_anchor::NONE,
                        ),
                        StatusTipPlacement::Anchor(a) => {
                            (x, y + caret_height, status_anchor_code(a))
                        }
                    };
                    self.sink.push_frame(&encode_status_show(
                        &text,
                        &self.tips.status_bg,
                        &self.tips.status_fg,
                        fx,
                        fy,
                        duration_ms as i32,
                        anchor,
                    ));
                }
            }
            UiCommand::HideStatusTip => {
                #[cfg(target_os = "macos")]
                self.sink.push_frame(&encode_status_hide());
                #[cfg(all(target_os = "linux", ext_presenter))]
                self.overlays.hide(
                    &*self.sink,
                    wind_ipc::protocol::overlay::OVERLAY_KIND_STATUS,
                );
            }
            UiCommand::ShowToast {
                text,
                position,
                kind,
                duration_ms,
                accent,
            } => self.push_toast(&text, position, kind, duration_ms as i32, accent),
            UiCommand::SetTheme(t) => {
                // 提示类窗口在 .app 侧原生渲染，配色须在此求值成 hex 随帧下发；
                // 兜底值与各自 Windows 实现的编译期默认逐字一致，避免两端观感分叉。
                let (tooltip_bg, tooltip_fg) = node_colors(
                    &t,
                    "tooltip_bg",
                    "tooltip_text",
                    t.views.tooltip.as_ref(),
                    ([60, 60, 64, 240], [240, 240, 245, 255]),
                );
                let (status_bg, status_fg) = node_colors(
                    &t,
                    "status_bg",
                    "status_text",
                    t.views.status.as_ref(),
                    ([40, 40, 40, 235], [245, 245, 245, 255]),
                );
                let (toast_bg, toast_fg) = node_colors(
                    &t,
                    "toast_bg",
                    "toast_text",
                    t.views.toast.as_ref(),
                    ([44, 44, 48, 240], [240, 240, 245, 255]),
                );
                self.tips = TipColors {
                    tooltip_bg,
                    tooltip_fg,
                    status_bg,
                    status_fg,
                    toast_bg,
                    toast_fg,
                };
                // 面板是惰性建窗的，主题得在本模块之外留一份底，理由见 `SkCmd::Theme`。
                #[cfg(target_os = "macos")]
                self.push_softkeyboard(sk::SkCmd::Theme(t.clone()));
                #[cfg(all(target_os = "linux", ext_presenter))]
                self.overlays.set_theme(&t);
                self.theme = Some(t.clone());
                self.win.set_theme(*t);
            }
            UiCommand::SetCandidateTextFamily(f) => self.win.set_text_family_override(&f),
            UiCommand::SetCandidateLayout {
                vertical,
                rotated,
                upright,
            } => self.win.set_orientation(vertical, rotated, upright),
            UiCommand::SetPreeditEmbedded(v) => self.win.set_preedit_embedded(v),
            UiCommand::SetCandidateFontSize(s) => self.win.set_font_size_override(s),
            // 两步顺序同 Windows 侧 manager：链首必须是刚设进去的那个字族。
            // ⚠️ CoreText 后端只实现了回退链那一半，脚本指派尚未实现（见 coretext.rs 的
            // `plan` 字段说明）——这里照常下发，是为了让两平台的**配置通路**一致，
            // 补上 macOS 侧渲染时不必再改接线。
            UiCommand::SetCandidateFont {
                family,
                fallback,
                scripts,
                weight,
            } => {
                self.win.set_font_family(&family);
                self.win.set_font_plan(&family, &fallback, &scripts);
                self.win.set_font_weight(weight);
            }
            UiCommand::SetCandidateMinSize {
                width_horizontal,
                width_vertical,
                height_horizontal,
                height_vertical,
                rows,
            } => self.win.set_min_size(
                width_horizontal,
                width_vertical,
                height_horizontal,
                height_vertical,
                rows,
            ),
            UiCommand::SetTooltipDelay(d) => self.win.set_tooltip_delay(d),
            UiCommand::SetCandidateFlipWhenAbove(v) => self.win.set_flip_when_above(v),
            UiCommand::SetCandidateSwapWhenAbove(v) => self.win.set_swap_preedit_when_above(v),
            UiCommand::SetCandidateOffset { x, y } => self.win.set_user_offset(x, y),
            UiCommand::SetCandidateShadow(on) => self.win.set_theme_shadow(on),
            UiCommand::SetPagerInPreedit(v) => self.win.set_pager_in_preedit(v),
            UiCommand::SetPagerDisplay { h, v } => self.win.set_pager_display(h, v),
            UiCommand::SetPageNumberDisplay { h, v } => self.win.set_page_number_display(h, v),
            UiCommand::SetTooltipChaiziFont { path, family } => {
                self.chaizi_font = path.clone();
                self.win.set_chaizi_font(&path, &family)
            }
            // 全局热键：macOS 走 Carbon；Linux 无对应通路（Wayland 无标准全局热键），落 `other` 臂。
            #[cfg(target_os = "macos")]
            UiCommand::RegisterGlobalHotkeys(entries) => {
                // 只入队 + 唤醒主线程；真正的 Carbon 注册在主线程做（见该模块头「线程约定」）。
                crate::global_hotkey_macos::apply(entries, self.ev_tx.clone());
            }
            // 「截图所有窗口」：截**我们自己渲染的**每一个可见浮窗。
            //
            // 候选窗的像素在本进程（光栅化后经 SHM 推下去），就地截；状态气泡 / 悬停提示 /
            // Toast 是 `.app` 侧的 NSPanel，转成一次下行请求由那边截。
            //
            // **右键菜单不截**：Windows 上它是我们自绘的窗口（`popup_menu.rs`），macOS 上却是
            // 原生 NSMenu——要截它只能走 `CGWindowListCreateImage`，那需要「屏幕录制」授权
            // （见 `PanelCapture` 的说明）。为一张菜单截图换一项更敏感的授权不划算，跳过。
            UiCommand::TakeScreenshot { dir } => {
                let dir = std::path::PathBuf::from(&dir);
                let ts = crate::screenshot::timestamp();
                // 取像素要 `&self`（读候选窗当前帧），必须在本线程；**其余全部挪走**，
                // 理由见 `spawn_screenshot_work`。
                let shot = self.capture_candidate();
                let sink = Arc::clone(&self.sink);
                spawn_screenshot_work("take", move || {
                    // 本进程能截的那部分先做完，结果随请求一起带下去，由 `.app` 原样回传，
                    // 好让协调器把两边的数量合成**一条** Toast（而不是各弹各的）。
                    let (mut saved, mut clipboard) = (0usize, false);
                    if let Some((buf, w, h)) = shot {
                        let path = dir.join(format!("candidate_{ts}.png"));
                        match crate::screenshot::save_bgra_to_png(&buf, w, h, &path) {
                            Ok(()) => {
                                tracing::info!("Screenshot saved: {:?}", path);
                                saved += 1;
                                // 存盘同时进剪贴板（对齐 Windows）：截完直接能粘贴，省去翻目录。
                                // 剪贴板失败不影响"已存盘"这个既成事实，只在文案里说明。
                                match crate::screenshot::copy_bgra_to_clipboard(&buf, w, h) {
                                    Ok(()) => clipboard = true,
                                    Err(e) => tracing::warn!("截图进剪贴板失败: {e}"),
                                }
                            }
                            Err(e) => tracing::warn!("截图存盘失败: {e}"),
                        }
                    }
                    let items: Vec<serde_json::Value> = ["status_tip", "tooltip", "toast"]
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "target": t,
                                "path": dir.join(format!("{t}_{ts}.png")).to_string_lossy(),
                            })
                        })
                        .collect();
                    // 请求在本线程发出（而不是先发再算）：`already*` 要带上真实结果，
                    // 而 `.app` 那侧本就是异步的，晚几十毫秒不可见。
                    sink.push_frame(&encode_ext(
                        ext_kind::SHOT_PANEL,
                        serde_json::json!({
                            "mode": "all",
                            "dir": dir.to_string_lossy(),
                            "already": saved,
                            "already_clipboard": clipboard,
                            "items": items,
                        })
                        .to_string()
                        .as_bytes(),
                    ));
                });
            }
            UiCommand::ScreenshotCandidateToClipboard => {
                let shot = self.capture_candidate();
                let sink = Arc::clone(&self.sink);
                #[cfg(target_os = "macos")]
                let (bg, fg) = (self.tips.toast_bg.clone(), self.tips.toast_fg.clone());
                #[cfg(all(target_os = "linux", ext_presenter))]
                let toast = self.overlays.toast_handle();
                spawn_screenshot_work("clip", move || {
                    let (msg, kind) = match shot {
                        Some((buf, w, h)) => {
                            match crate::screenshot::copy_bgra_to_clipboard(&buf, w, h) {
                                Ok(()) => {
                                    tracing::info!("Candidate screenshot copied to clipboard");
                                    ("候选窗口已截图到剪贴板".to_string(), ToastKind::Success)
                                }
                                Err(e) => {
                                    tracing::warn!("Screenshot to clipboard failed: {e}");
                                    (format!("截图到剪贴板失败：{e}"), ToastKind::Error)
                                }
                            }
                        }
                        None => ("候选窗口未显示，无法截图".to_string(), ToastKind::Info),
                    };
                    #[cfg(target_os = "macos")]
                    sink.push_frame(&toast_frame(
                        &bg,
                        &fg,
                        &msg,
                        ToastPosition::BottomRight,
                        kind,
                        3000,
                        None,
                    ));
                    #[cfg(all(target_os = "linux", ext_presenter))]
                    toast.show(&*sink, &msg, ToastPosition::BottomRight, kind, 3000, None);
                });
            }
            UiCommand::CopyTooltipText(text) => {
                // 文本由协调器按原始行拼好随命令下发，复制无需 .app 参与。
                let (msg, kind) = match Some(text.as_str()).filter(|s| !s.is_empty()) {
                    Some(t) => {
                        crate::popup_menu::set_clipboard_text(t);
                        ("提示内容已复制".to_string(), ToastKind::Success)
                    }
                    None => ("提示内容为空，无法复制".to_string(), ToastKind::Info),
                };
                self.push_result_toast(&msg, kind);
            }
            // 状态气泡 / 悬停提示的截图：**像素不在本进程**（这两者是 `.app` 侧的原生
            // NSPanel，服务端只下发文本与配色），故转成一次下行请求由那边动手。
            // 文件名与随后的 Toast 文案仍留在服务端决定，与 Windows 逐字一致。
            UiCommand::ScreenshotStatusTip { dir } => self.request_panel_shot("status_tip", &dir),
            UiCommand::ScreenshotTooltip { dir } => self.request_panel_shot("tooltip", &dir),
            // 协调器把定位方式切到 fixed 时问「你现在在哪」，好把当前位置落盘成 custom_x/y
            // ——否则窗口会跳到上次保存（往往是 0,0）的坐标。
            //
            // 两者都转成一次**下行询问**而不是在本进程记账：浮窗是 `.app` 侧的原生 NSPanel，
            // 服务进程推下去的只是建议落点，实际位置还会被那边的屏幕钳制 / 下方放不下时上翻 /
            // 用户本次组合内的拖动落位改掉。答案经上行 `pos.*` 回来（见 `handle_ext`）。
            UiCommand::ReportCandidatePos => {
                self.sink
                    .push_frame(&encode_ext(ext_kind::POS_CANDIDATE_QUERY, b""));
            }
            UiCommand::ReportStatusTipPos => {
                self.sink
                    .push_frame(&encode_ext(ext_kind::POS_STATUS_TIP_QUERY, b""));
            }
            // ── 软键盘面板 ──
            //
            // 与其余浮窗相反，它**不走 SHM + `.app` 那条路**：面板由服务进程自己开窗
            // （macOS 上唯一一处），理由见 `crate::mac_panel` 模块头。这里只负责把命令
            // 从本工作线程转运到主线程——AppKit 碰不得工作线程。
            // 软键盘仅 macOS（服务进程自开 NSPanel）；Linux 不做，落 `other` 臂。
            #[cfg(target_os = "macos")]
            UiCommand::ShowSoftKeyboard {
                pages,
                current,
                keys,
                send_keys,
                // 多屏摆放依据,macOS 上用不到:协调器的 `focus_monitor()` 在
                // `#[cfg(not(target_os = "windows"))]` 下恒返回 `None`,于是
                // `softkeyboard_placement()` 走早返回分支,这两个字段恒是 `None`。
                // 面板位置由 `mac_panel` 自己定,不经这条路。
                //
                // ⚠️ 刻意逐个写出而不用 `..`:用 `..` 的话,将来 `ShowSoftKeyboard`
                //    再加 macOS 也该处理的字段会被**静默忽略**——而本行当初漏掉这
                //    两个字段正是同一类事故(编译期报错才把它暴露出来)。显式列出,
                //    下次加字段时这里会再次编译失败,强制过一遍 review。
                anchor: _,
                work_area: _,
            } => self.push_softkeyboard(sk::SkCmd::Show {
                pages,
                current,
                keys,
                send_keys,
            }),
            #[cfg(target_os = "macos")]
            UiCommand::HideSoftKeyboard => self.push_softkeyboard(sk::SkCmd::Hide),
            #[cfg(target_os = "macos")]
            UiCommand::SoftKeyboardKeyState { slot, down } => {
                self.push_softkeyboard(sk::SkCmd::KeyState { slot, down })
            }
            #[cfg(target_os = "macos")]
            UiCommand::SoftKeyboardLayer { shift } => {
                self.push_softkeyboard(sk::SkCmd::Layer { shift })
            }
            UiCommand::OpenPath(path) => crate::manager::open_path(&path),
            UiCommand::OpenApp { path, args } => crate::manager::open_app(&path, &args),
            UiCommand::Shutdown => {}
            UiCommand::CopyToClipboard(text) => crate::popup_menu::set_clipboard_text(&text),
            // 其余未接的变体（截图族 / 输入诊断 HUD / 拖动落点回报 / 候选右键菜单键盘
            // 导航 / 工具栏位置）见 wind_macos/AGENTS.md「与 Windows 的功能差距」表。
            // 新接一个就从那张表里划掉一行。
            other => {
                tracing::debug!("forwarder: 暂未处理 {:?}", std::mem::discriminant(&other));
            }
        }
    }

    /// 把一条软键盘命令转运给主线程宿主。
    #[cfg(target_os = "macos")]
    fn push_softkeyboard(&self, cmd: sk::SkCmd) {
        sk::apply(cmd, &self.ev_tx);
    }

    /// 推一条 toast：macOS 发文本给 `.app` 原生渲染，Linux 光栅化成浮层帧。
    fn push_toast(
        &mut self,
        text: &str,
        position: ToastPosition,
        kind: ToastKind,
        duration_ms: i32,
        accent: Option<[u8; 4]>,
    ) {
        #[cfg(all(target_os = "linux", ext_presenter))]
        self.overlays
            .show_toast(&*self.sink, text, position, kind, duration_ms, accent);
        #[cfg(target_os = "macos")]
        self.sink.push_frame(&toast_frame(
            &self.tips.toast_bg,
            &self.tips.toast_fg,
            text,
            position,
            kind,
            duration_ms,
            accent,
        ));
    }

    /// 截图/复制类操作的结果反馈：右下角 toast，3 秒（与 Windows 侧同一形态）。
    fn push_result_toast(&mut self, text: &str, kind: ToastKind) {
        self.push_toast(text, ToastPosition::BottomRight, kind, 3000, None);
    }

    /// 取候选窗当前帧的像素（BGRA + 尺寸）。窗口未显示时返回 `None`。
    ///
    /// macOS 的候选窗像素本来就在服务进程里（我们光栅化后经 SHM 推给 `.app`），故截图
    /// 无需 `.app` 参与——直接把同一份 buffer 编码存盘即可。这也是为什么状态气泡 / 悬停
    /// 提示的截图**做不了**：那两者是 `.app` 侧原生 NSPanel，像素不在本进程。
    fn capture_candidate(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        if !self.visible {
            return None;
        }
        let f = self.win.render_frame()?;
        Some((f.buf, f.width, f.height))
    }

    /// 请 `.app` 截某个原生浮窗存盘。文件名在此定（与 Windows 侧同一格式），
    /// 结果经上行 `shot.result` 回来由协调器弹 Toast（见 `Coordinator::handle_ext`）。
    fn request_panel_shot(&self, target: &str, dir: &std::path::Path) {
        let path = dir.join(format!("{target}_{}.png", crate::screenshot::timestamp()));
        self.send_shot_request(serde_json::json!({
            "mode": "single",
            "items": [{ "target": target, "path": path.to_string_lossy() }],
        }));
    }

    /// 下发截图请求。`.app` 只负责截 `items` 里的每一项，其余字段**原样回传**——
    /// 文案所需的上下文（数量、目录、候选是否已进剪贴板）因此不必在任何一边留状态。
    fn send_shot_request(&self, body: serde_json::Value) {
        self.sink.push_frame(&encode_ext(
            ext_kind::SHOT_PANEL,
            body.to_string().as_bytes(),
        ));
    }

    /// 按 `win` 的当前状态渲染一帧并推给 `.app`（像素走 SHM，元数据走 push 管道）。
    ///
    /// 内容更新（`UpdateCandidates`）与纯外观变更（换主题/字号…）共用此路径——后者若不
    /// 走这里重推一帧，显示中的候选窗就会停在旧样子。
    fn push_current_frame(&mut self, tip: Option<Arc<TooltipDoc>>) {
        match self.win.render_frame() {
            Some(f) => {
                let (sx, sy, w, h, scale, soft, absolute) = (
                    f.screen_x,
                    f.screen_y,
                    f.width,
                    f.height,
                    f.scale,
                    f.software_shadow,
                    f.absolute_pos,
                );
                // 翻页器命中矩形的内部 tag(HOVER_PAGE_PREV/NEXT=100000/100001)重映射为
                // Swift/Go 约定的 -1(上页)/-2(下页)，否则 100000>=0 会被 .app 误当候选选中
                // (index 100000) → 翻页失效；候选 tag(>=0)原样。对齐 Go forwarder_darwin。
                let rects: Vec<(i32, i32, i32, i32, i32)> = f
                    .hit_rects
                    .iter()
                    .map(|(i, r)| {
                        let wire = if *i == crate::manager::HOVER_PAGE_PREV {
                            -1
                        } else if *i == crate::manager::HOVER_PAGE_NEXT {
                            -2
                        } else {
                            *i
                        };
                        (wire, r.x as i32, r.y as i32, r.w as i32, r.h as i32)
                    })
                    .collect();
                let buf = f.buf;
                // 先写 SHM 像素并取 seq；shm 建失败则整帧放弃——
                // 不能只推命中矩形/tooltip 而无底帧，否则 .app 拿到无像素的命中区（不一致）。
                let seq = match self.ensure_shm() {
                    Some(shm) => shm.write_frame(sx, sy, w, h, &buf),
                    None => return,
                };
                let mut flags =
                    SharedRenderHeader::FLAG_VISIBLE | SharedRenderHeader::FLAG_CONTENT_READY;
                if soft {
                    flags |= SharedRenderHeader::FLAG_SOFTWARE_SHADOW;
                }
                if absolute {
                    flags |= SharedRenderHeader::FLAG_ABSOLUTE_POS;
                }
                self.sink.push_frame(&encode_host_render_frame(
                    seq,
                    sx,
                    sy,
                    w,
                    h,
                    flags,
                    scale.round().max(1.0) as u32,
                ));
                self.sink.push_frame(&encode_candidate_rects(&rects));
                // Linux：tooltip 在本进程光栅化，坐标按候选窗建议落点算（addon 平移到实际位置）。
                // 悬停目标取自候选窗自己的 hover 状态，与 `tip` 同源（都来自本帧 `hover`）。
                #[cfg(all(target_os = "linux", ext_presenter))]
                {
                    let ov = self.win.render_tooltip_overlay(sx, sy);
                    self.overlays.show_tooltip(&*self.sink, ov);
                }
                #[cfg(target_os = "macos")]
                match &tip {
                    Some(doc) => {
                        let (text, runs) = self.tooltip_payload(doc);
                        self.sink.push_frame(&encode_tooltip_show(
                            &text,
                            &self.tips.tooltip_bg,
                            &self.tips.tooltip_fg,
                            &self.chaizi_font,
                            &runs,
                        ))
                    }
                    None => self.sink.push_frame(&encode_tooltip_hide()),
                }
                self.visible = true;
                self.last_tip = tip;
                tracing::debug!(
                    "forwarder pushed host-render frame seq={} {}x{} at ({},{}) scale={}",
                    seq,
                    w,
                    h,
                    sx,
                    sy,
                    scale
                );
            }
            None => {
                tracing::debug!("forwarder render_frame=None → hide");
                self.hide_frame();
            }
        }
    }

    /// 气泡的纯文本 + 按当前主题现算的分段颜色（UTF-16 区间，直接当 `NSRange`）。
    ///
    /// 正文色兜底与 `SetTheme` 里下发的 `tooltip_fg` 同源（palette `tooltip_text` → 节点文字色，
    /// 编译期默认同 `node_colors` 的兜底）；等于正文色的区间已丢弃，出厂下恒为空。
    #[cfg(target_os = "macos")]
    fn tooltip_payload(&self, doc: &TooltipDoc) -> (String, Vec<TooltipColorRun>) {
        let styled = doc.to_styled();
        let runs = match &self.theme {
            Some(t) => {
                let default_node = wind_theme::RvNode::default();
                let node = t.views.tooltip.as_ref().unwrap_or(&default_node);
                let runs = crate::span_runs::color_runs(
                    t,
                    node,
                    true,
                    wind_theme::TextState::Normal,
                    t.color("tooltip_text", [240, 240, 245, 255]),
                    &styled,
                );
                crate::text::dwrite::utf16_runs(styled.as_str(), &runs)
                    .into_iter()
                    .map(|(start, len, rgba)| TooltipColorRun { start, len, rgba })
                    .collect()
            }
            None => Vec::new(),
        };
        (styled.into_string(), runs)
    }

    fn hide_frame(&mut self) {
        // 无论 SHM 建没建起来都得落 visible=false：否则外观类命令会对着一个已经隐藏的
        // 候选窗重推帧，把它又推回屏幕上。
        self.visible = false;
        self.last_tip = None;
        #[cfg(all(target_os = "linux", ext_presenter))]
        self.overlays.hide(
            &*self.sink,
            wind_ipc::protocol::overlay::OVERLAY_KIND_TOOLTIP,
        );
        if let Some(shm) = self.shm.as_mut() {
            let seq = shm.write_hidden();
            self.sink
                .push_frame(&encode_host_render_frame(seq, 0, 0, 0, 0, 0, 1));
        }
    }
}

pub fn forwarder_thread(
    rx: Receiver<UiCommand>,
    ev_tx: Sender<UiEvent>,
    sink: Arc<dyn HostRenderSink>,
    suffix: String,
) {
    let mut fwd = Forwarder::new(ev_tx, sink, suffix);
    tracing::info!("macOS host-render forwarder started");
    // 不是裸 `for cmd in rx`：那样线程永远阻塞在 recv 上，背景图缓存的闲置回收
    // （`manager.rs` 的消息循环负责的那件事）在 macOS 就一次都不会发生。
    while let Some(cmd) = crate::view::recv_evicting_idle(&rx) {
        if matches!(cmd, UiCommand::Shutdown) {
            break;
        }
        fwd.handle(cmd);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::candidate_window::CandidateItem;
    use std::sync::{Arc, Mutex};

    struct CapSink(Arc<Mutex<Vec<Vec<u8>>>>);
    impl wind_bridge::HostRenderSink for CapSink {
        fn push_frame(&self, f: &[u8]) {
            self.0.lock().unwrap().push(f.to_vec());
        }
    }
    fn cmd_of(f: &[u8]) -> u16 {
        u16::from_le_bytes([f[2], f[3]])
    }
    fn item(t: &str) -> CandidateItem {
        CandidateItem {
            text: t.into(),
            code: String::new(),
            label: String::new(),
            tooltip: Default::default(),
            comment: Default::default(),
            comment_above: Default::default(),
            no_index: false,
        }
    }
    /// 事件通道的接收端在测试里不消费，但必须**持有**——drop 掉会让 forwarder 里的
    /// `ev_tx.send` 立刻报错。故连同 Forwarder 一起返回。
    fn mk(
        cap: Arc<Mutex<Vec<Vec<u8>>>>,
        suffix: &str,
    ) -> (Forwarder, std::sync::mpsc::Receiver<UiEvent>) {
        let (ev_tx, ev_rx) = std::sync::mpsc::channel();
        (
            Forwarder::new(ev_tx, Arc::new(CapSink(cap)), suffix.into()),
            ev_rx,
        )
    }

    #[test]
    fn update_candidates_emits_frame_and_rects() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t1");
        f.handle(UiCommand::UpdateCandidates {
            preedit: "a".into(),
            preedit_caret: 1,
            preedit_host_owned: false,
            mode_label: "".into(),
            candidates: vec![item("中"), item("国")],
            selected: 0,
            hover: -1,
            page: 1,
            total_pages: 1,
            caret_x: 100,
            caret_y: 200,
            caret_height: 20,
            caret_valid: true,
            fixed: false,
            fixed_x: 0,
            fixed_y: 0,
        });
        let v = cap.lock().unwrap();
        assert!(
            v.iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_HOST_RENDER_FRAME)
        );
        assert!(
            v.iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_CANDIDATE_RECTS)
        );
    }

    /// 造一条最小的 UpdateCandidates，供需要「先显示一帧」的用例复用。
    /// 取最后一帧 `CMD_HOST_RENDER_FRAME` 的 `(x, y, flags)`。
    fn last_render_frame(cap: &Arc<Mutex<Vec<Vec<u8>>>>) -> Option<(i32, i32, u32)> {
        let v = cap.lock().unwrap();
        let f = v
            .iter()
            .rev()
            .find(|f| cmd_of(f) == CMD_HOST_RENDER_FRAME)?;
        let p = &f[8..];
        Some((
            i32::from_le_bytes(p[4..8].try_into().unwrap()),
            i32::from_le_bytes(p[8..12].try_into().unwrap()),
            u32::from_le_bytes(p[20..24].try_into().unwrap()),
        ))
    }

    fn show_two_fixed(f: &mut Forwarder, fixed: bool, fx: i32, fy: i32) {
        f.handle(UiCommand::UpdateCandidates {
            preedit: "a".into(),
            preedit_caret: 1,
            preedit_host_owned: false,
            mode_label: "".into(),
            candidates: vec![item("中"), item("国")],
            selected: 0,
            hover: -1,
            page: 1,
            total_pages: 1,
            caret_x: 100,
            caret_y: 200,
            caret_height: 20,
            caret_valid: true,
            fixed,
            fixed_x: fx,
            fixed_y: fy,
        });
    }

    /// 固定位置模式：帧坐标就是配置里的 custom_x/y，且带 FLAG_ABSOLUTE_POS。
    ///
    /// 回归：这条路径此前被 `fixed: _` 显式丢弃——「候选窗固定位置」在 macOS 上整个是死的，
    /// 设置里改了没有任何反应。标志位不能省：`.app` 收到普通帧会自作主张做「下方放不下就
    /// 翻到光标上方」，固定点靠近屏幕底边时窗口会被莫名弹到顶上。
    #[test]
    fn fixed_position_frame_carries_absolute_flag_and_coords() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_fixed");
        show_two_fixed(&mut f, true, 640, 480);

        let (x, y, flags) = last_render_frame(&cap).expect("应推出 host render frame");
        assert_eq!((x, y), (640, 480), "固定位置必须原样下发，不再按光标推算");
        assert_ne!(
            flags & SharedRenderHeader::FLAG_ABSOLUTE_POS,
            0,
            "缺 FLAG_ABSOLUTE_POS，.app 会对固定位置套用光标翻转逻辑"
        );
    }

    /// 跟随光标模式不得置 FLAG_ABSOLUTE_POS——否则 `.app` 不再做上翻，光标贴屏幕底边时
    /// 候选窗被钳在底边糊住输入位。
    #[test]
    fn follow_caret_frame_has_no_absolute_flag() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_follow");
        show_two_fixed(&mut f, false, 640, 480);

        let (_, _, flags) = last_render_frame(&cap).expect("应推出 host render frame");
        assert_eq!(flags & SharedRenderHeader::FLAG_ABSOLUTE_POS, 0);
    }

    /// `ReportCandidatePos` / `ReportStatusTipPos`（协调器把定位方式切到 fixed 时问「你现在
    /// 在哪」）在 macOS 只能转成一次下行询问 —— 浮窗是 `.app` 侧的 NSPanel，服务进程推下去
    /// 的只是建议落点，实际位置还会被那边的屏幕钳制 / 上翻 / 拖动落位改掉。
    ///
    /// 回归：这里一度改用「记住最后一帧推的坐标」直接回答，省掉一次往返。那个值在窗口被
    /// 上翻或拖动过之后就是错的，落盘后窗口会摆到一个它从没出现过的地方。
    #[test]
    fn report_pos_sends_query_downstream() {
        for (cmd, want) in [
            (UiCommand::ReportCandidatePos, ext_kind::POS_CANDIDATE_QUERY),
            (
                UiCommand::ReportStatusTipPos,
                ext_kind::POS_STATUS_TIP_QUERY,
            ),
        ] {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, ev) = mk(cap.clone(), "_t_query");
            show_two_fixed(&mut f, true, 300, 400);
            cap.lock().unwrap().clear();
            while ev.try_recv().is_ok() {}

            f.handle(cmd);
            let v = cap.lock().unwrap();
            let ext = v
                .iter()
                .find(|f| cmd_of(f) == CMD_EXT)
                .expect("应发扩展信封问询");
            let (kind, body) = decode_ext(&ext[8..]).expect("信封应可解");
            assert_eq!(kind, want);
            assert!(body.is_empty(), "问询不带 body");
            assert!(
                ev.try_recv().is_err(),
                "不该就地编一个位置事件——答案要等 .app 回"
            );
        }
    }

    fn show_two(f: &mut Forwarder) {
        f.handle(UiCommand::UpdateCandidates {
            preedit: "a".into(),
            preedit_caret: 1,
            preedit_host_owned: false,
            mode_label: "".into(),
            candidates: vec![item("中"), item("国")],
            selected: 0,
            hover: -1,
            page: 1,
            total_pages: 1,
            caret_x: 100,
            caret_y: 200,
            caret_height: 20,
            caret_valid: true,
            fixed: false,
            fixed_x: 0,
            fixed_y: 0,
        });
    }

    #[test]
    fn theme_change_repaints_visible_candidates() {
        // 回归：换主题只改了 win 的配色却不重推帧，显示中的候选窗停在旧样子，
        // 要等下一次按键/鼠标悬停才更新（用户可见症状：菜单里换主题「不生效」）。
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_theme");
        show_two(&mut f);
        cap.lock().unwrap().clear();

        f.handle(UiCommand::SetTheme(Box::default()));
        let v = cap.lock().unwrap();
        assert!(
            v.iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_HOST_RENDER_FRAME),
            "换主题后必须重推一帧"
        );
    }

    #[test]
    fn theme_change_does_not_resurrect_hidden_candidates() {
        // 反向：候选窗已隐藏时换主题不得把它推回屏幕上。
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_theme2");
        show_two(&mut f);
        f.handle(UiCommand::HideCandidates);
        cap.lock().unwrap().clear();

        f.handle(UiCommand::SetTheme(Box::default()));
        assert!(
            cap.lock().unwrap().is_empty(),
            "隐藏状态下换主题不该推任何帧"
        );
    }

    /// 从抓到的帧里取出 toast 文本（CmdToastShow 的第二个长度前缀字段）。
    fn toast_texts(v: &[Vec<u8>]) -> Vec<String> {
        v.iter()
            .filter(|f| cmd_of(f) == wind_ipc::protocol::CMD_TOAST_SHOW)
            .filter_map(|f| {
                let p = &f[8..];
                let n0 = u32::from_le_bytes(p[0..4].try_into().ok()?) as usize;
                let off = 4 + n0;
                let n1 = u32::from_le_bytes(p[off..off + 4].try_into().ok()?) as usize;
                String::from_utf8(p[off + 4..off + 4 + n1].to_vec()).ok()
            })
            .collect()
    }

    /// 等扩展信封到达（截图请求由后台线程发出）。最多等 5 秒。
    fn wait_for_ext(cap: &Arc<Mutex<Vec<Vec<u8>>>>) -> Option<(String, serde_json::Value)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(v) = last_ext(cap) {
                return Some(v);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// 取最后一个扩展信封的 (kind, body-json)。
    fn last_ext(cap: &Arc<Mutex<Vec<Vec<u8>>>>) -> Option<(String, serde_json::Value)> {
        let v = cap.lock().unwrap();
        let f = v.iter().rev().find(|f| cmd_of(f) == CMD_EXT)?;
        let (kind, body) = decode_ext(&f[8..])?;
        Some((kind.to_string(), serde_json::from_slice(body).ok()?))
    }

    /// 候选窗没显示时不能悄悄存一张空图，也不能就地弹 Toast——文案要等 `.app` 那三个
    /// 浮窗的结果一起算（`already: 0` 随请求带下去）。
    #[test]
    fn screenshot_without_visible_candidates_saves_nothing_and_asks_app() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_shot0");
        let dir = std::env::temp_dir().join("windinput_test_shots_none");
        f.handle(UiCommand::TakeScreenshot {
            dir: dir.display().to_string(),
        });

        // 请求由后台线程发出，等它到达。
        let (kind, body) = wait_for_ext(&cap).expect("应下发截图请求");
        assert!(!dir.exists(), "不该建目录/落文件");
        assert!(
            toast_texts(&cap.lock().unwrap()).is_empty(),
            "不该就地弹 Toast：数量要与 .app 侧合并后只弹一条"
        );
        assert_eq!(kind, ext_kind::SHOT_PANEL);
        assert_eq!(body["mode"], "all");
        assert_eq!(body["already"], 0, "候选窗没截到");
        // 三个 `.app` 侧浮窗都要问；右键菜单**不在其列**（原生 NSMenu，截它要屏幕录制授权）。
        let targets: Vec<&str> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["target"].as_str().unwrap())
            .collect();
        assert_eq!(targets, ["status_tip", "tooltip", "toast"]);
    }

    /// 等目录里出现 `want` 个文件（截图已改为后台线程完成）。最多等 5 秒。
    fn wait_for_files(dir: &std::path::Path, want: usize) -> Vec<std::ffi::OsString> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let files: Vec<_> = std::fs::read_dir(dir)
                .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
                .unwrap_or_default();
            if files.len() >= want || std::time::Instant::now() >= deadline {
                return files;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn screenshot_saves_png_when_visible() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_shot1");
        show_two(&mut f);
        let dir = std::env::temp_dir().join(format!("windinput_test_shots_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        f.handle(UiCommand::TakeScreenshot {
            dir: dir.display().to_string(),
        });

        // 存盘已挪到后台线程（forwarder 线程不能被 PNG 编码 + osascript 卡住），
        // 故等它落盘；超时即判失败，不放过"永远不落盘"这种回归。
        let files = wait_for_files(&dir, 1);
        assert_eq!(files.len(), 1, "应存出一张 PNG，实际 {files:?}");
        let name = files[0].to_string_lossy().to_string();
        assert!(
            name.starts_with("candidate_") && name.ends_with(".png"),
            "{name}"
        );
        // 回归：timestamp() 曾在非 Windows 恒返回 "00000000_000000"，多张截图互相覆盖。
        assert!(
            !name.contains("00000000_000000"),
            "文件名须含真实时间戳，实际 {name}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 文本帧是 macOS `.app` 的契约；Linux 走光栅浮层帧，见 `linux_overlays` 同名用例。
    #[cfg(target_os = "macos")]
    #[test]
    fn copy_tooltip_text_reports_empty_when_no_tip() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_tip");
        show_two(&mut f); // hover=-1 → 无 tooltip
        cap.lock().unwrap().clear();
        f.handle(UiCommand::CopyTooltipText(String::new()));
        let texts = toast_texts(&cap.lock().unwrap());
        assert!(
            texts.iter().any(|t| t.contains("为空")),
            "应提示内容为空，实际 {texts:?}"
        );
    }

    #[test]
    fn hide_emits_hidden_frame() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t2");
        // 先显示一帧建 shm。
        f.handle(UiCommand::UpdateCandidates {
            preedit: "a".into(),
            preedit_caret: 1,
            preedit_host_owned: false,
            mode_label: "".into(),
            candidates: vec![item("中")],
            selected: 0,
            hover: -1,
            page: 1,
            total_pages: 1,
            caret_x: 10,
            caret_y: 20,
            caret_height: 20,
            caret_valid: true,
            fixed: false,
            fixed_x: 0,
            fixed_y: 0,
        });
        cap.lock().unwrap().clear();
        f.handle(UiCommand::HideCandidates);
        let v = cap.lock().unwrap();
        let hr = v
            .iter()
            .find(|x| cmd_of(x) == wind_ipc::protocol::CMD_HOST_RENDER_FRAME)
            .expect("hidden frame");
        // payload flags @ 帧 offset 8+20=28，VISIBLE 位应为 0。
        assert_eq!(u32::from_le_bytes(hr[28..32].try_into().unwrap()) & 0x1, 0);
    }

    #[test]
    fn update_toolbar_emits_mode_status() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t3");
        f.handle(UiCommand::UpdateToolbar(
            crate::toolbar::ToolbarState::default(),
        ));
        assert!(
            cap.lock()
                .unwrap()
                .iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_MODE_STATUS)
        );
    }

    // 文本帧是 macOS `.app` 的契约；Linux 走光栅浮层帧，见 `linux_overlays` 同名用例。
    #[cfg(target_os = "macos")]
    #[test]
    fn status_tip_fixed_overrides_coords() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t4");
        f.handle(UiCommand::ShowStatusTip {
            text: "中".into(),
            x: 10,
            y: 20,
            caret_height: 18,
            duration_ms: 1000,
            placement: StatusTipPlacement::Fixed { x: 500, y: 600 },
        });
        let v = cap.lock().unwrap();
        let fr = v
            .iter()
            .find(|x| cmd_of(x) == wind_ipc::protocol::CMD_STATUS_SHOW)
            .expect("status_show frame");
        // payload = textLen+text + bgLen + fgLen + x:i32 + y:i32 + dur:i32。
        // "中"=3 字节 → text 段 4+3=7；bg/fg 空各 4 字节 → x 从 payload offset 7+4+4=15 起；帧 +8。
        let off = 8 + 15;
        assert_eq!(
            i32::from_le_bytes(fr[off..off + 4].try_into().unwrap()),
            500
        ); // fixed_x
        assert_eq!(
            i32::from_le_bytes(fr[off + 4..off + 8].try_into().unwrap()),
            600
        ); // fixed_y
        assert_eq!(
            i32::from_le_bytes(fr[off + 12..off + 16].try_into().unwrap()),
            status_anchor::NONE
        );
    }

    // 文本帧是 macOS `.app` 的契约；Linux 走光栅浮层帧，见 `linux_overlays` 同名用例。
    #[cfg(target_os = "macos")]
    #[test]
    fn status_tip_non_fixed_applies_offset() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t5");
        f.handle(UiCommand::ShowStatusTip {
            text: "x".into(),
            x: 10,
            y: 20,
            caret_height: 0,
            duration_ms: 0,
            placement: StatusTipPlacement::Caret {
                offset_x: 3,
                offset_y: 4,
            },
        });
        let v = cap.lock().unwrap();
        let fr = v
            .iter()
            .find(|x| cmd_of(x) == wind_ipc::protocol::CMD_STATUS_SHOW)
            .unwrap();
        // "x"=1 → text 段 4+1=5；bg/fg 空 → x 从 payload offset 5+4+4=13 起；帧 +8。
        let off = 8 + 13;
        assert_eq!(i32::from_le_bytes(fr[off..off + 4].try_into().unwrap()), 13); // 10+3
        assert_eq!(
            i32::from_le_bytes(fr[off + 4..off + 8].try_into().unwrap()),
            24
        ); // 20+4
    }

    /// 锚点：编码随帧下发，(x,y) 是光标底端（选屏参考），不叠加用户偏移。
    #[cfg(target_os = "macos")]
    #[test]
    fn status_tip_anchor_is_encoded() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t5a");
        f.handle(UiCommand::ShowStatusTip {
            text: "x".into(),
            x: 10,
            y: 20,
            caret_height: 18,
            duration_ms: 0,
            placement: StatusTipPlacement::Anchor(StatusTipAnchor::WindowBottomLeft),
        });
        let v = cap.lock().unwrap();
        let fr = v
            .iter()
            .find(|x| cmd_of(x) == wind_ipc::protocol::CMD_STATUS_SHOW)
            .unwrap();
        let off = 8 + 13;
        assert_eq!(i32::from_le_bytes(fr[off..off + 4].try_into().unwrap()), 10);
        assert_eq!(
            i32::from_le_bytes(fr[off + 4..off + 8].try_into().unwrap()),
            38
        );
        assert_eq!(
            i32::from_le_bytes(fr[off + 12..off + 16].try_into().unwrap()),
            status_anchor::WINDOW_BOTTOM_LEFT
        );
    }

    // 文本帧是 macOS `.app` 的契约；Linux 走光栅浮层帧，见 `linux_overlays` 同名用例。
    #[cfg(target_os = "macos")]
    #[test]
    fn hide_status_tip_and_toast_and_toolbar_emit() {
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t6");
        f.handle(UiCommand::HideStatusTip);
        f.handle(UiCommand::ShowToast {
            text: "ok".into(),
            position: crate::toast::ToastPosition::Center,
            kind: crate::toast::ToastKind::Success,
            duration_ms: 2000,
            accent: None,
        });
        f.handle(UiCommand::HideToolbar);
        let v = cap.lock().unwrap();
        assert!(
            v.iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_STATUS_HIDE)
        );
        let toast = v
            .iter()
            .find(|x| cmd_of(x) == wind_ipc::protocol::CMD_TOAST_SHOW)
            .expect("toast frame");
        // position 段：title(空,4) message("ok",4+2=6) bg(4) fg(4) accent(#52C46E,4+7=11) position(...)
        // 校验 position 字符串 = "center"。
        let p = &toast[8..];
        let mut o = 0usize;
        let mut read = || {
            let n = u32::from_le_bytes(p[o..o + 4].try_into().unwrap()) as usize;
            let s = String::from_utf8(p[o + 4..o + 4 + n].to_vec()).unwrap();
            o += 4 + n;
            s
        };
        assert_eq!(read(), ""); // title
        assert_eq!(read(), "ok"); // message
        let _ = read(); // bg
        let _ = read(); // fg
        assert_eq!(read(), "#52C46E"); // accent (Success)
        assert_eq!(read(), "center"); // position
        assert!(
            v.iter()
                .any(|x| cmd_of(x) == wind_ipc::protocol::CMD_MODE_STATUS)
        ); // HideToolbar → mode_status
    }

    /// 气泡的分段颜色随帧下发，且按**推送时**的主题现算：换明暗后重推，颜色跟着换。
    #[cfg(target_os = "macos")]
    #[test]
    fn tooltip_runs_are_recomputed_on_theme_change() {
        use wind_ui_types::{SpanStyle, StyledText, TooltipDoc, TooltipLine, TooltipSection};
        let cap = Arc::new(Mutex::new(Vec::new()));
        let (mut f, _ev) = mk(cap.clone(), "_t_runs");
        // `error` 在气泡里先查 `tooltip_error`（§6.2 作用域查找），两档各给一个值。
        let theme = |dark: bool| {
            let mut palette = std::collections::HashMap::new();
            palette.insert("error".to_string(), [1, 1, 1, 255]);
            palette.insert(
                "tooltip_error".to_string(),
                if dark { [3, 3, 3, 255] } else { [2, 2, 2, 255] },
            );
            Box::new(wind_theme::Resolved {
                is_dark: dark,
                palette,
                ..Default::default()
            })
        };
        f.handle(UiCommand::SetTheme(theme(false)));
        // 「你：nǐ !」里 `nǐ` 带亮 / 暗两色的字面内联色，`!` 带名字写的内联色 `error`。
        let mut text = StyledText::from("你：");
        text.push(
            "nǐ",
            &SpanStyle {
                color: Some(std::sync::Arc::new(wind_theme::InlineColor::parse(
                    "#C00000/#FF8080",
                ))),
                ..Default::default()
            },
        );
        text.push(" ", &SpanStyle::default());
        text.push(
            "!",
            &SpanStyle {
                color: Some(std::sync::Arc::new(wind_theme::InlineColor::parse("error"))),
                ..Default::default()
            },
        );
        let mut cand = item("你");
        cand.tooltip = Arc::new(TooltipDoc {
            sections: vec![TooltipSection {
                title: None,
                inline: false,
                lines: vec![TooltipLine { text, raw: 0 }],
            }],
        });
        f.handle(UiCommand::UpdateCandidates {
            preedit: "a".into(),
            preedit_caret: 1,
            preedit_host_owned: false,
            mode_label: "".into(),
            candidates: vec![cand],
            selected: 0,
            hover: 0,
            page: 1,
            total_pages: 1,
            caret_x: 100,
            caret_y: 200,
            caret_height: 20,
            caret_valid: true,
            fixed: false,
            fixed_x: 0,
            fixed_y: 0,
        });
        // 取最后一帧 tooltip 的 runs 尾段：(start, len, rgba)。
        let last_runs = |cap: &Arc<Mutex<Vec<Vec<u8>>>>| -> Vec<(u32, u32, [u8; 4])> {
            let v = cap.lock().unwrap();
            let fr = v
                .iter()
                .rev()
                .find(|f| cmd_of(f) == CMD_TOOLTIP_SHOW)
                .expect("应推出气泡");
            let p = &fr[8..];
            let mut off = 0usize;
            for _ in 0..4 {
                let n = u32::from_le_bytes(p[off..off + 4].try_into().unwrap()) as usize;
                off += 4 + n;
            }
            let u = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap());
            (0..u(off) as usize)
                .map(|k| {
                    let b = off + 4 + k * 12;
                    (u(b), u(b + 4), p[b + 8..b + 12].try_into().unwrap())
                })
                .collect()
        };
        // 「你：」是 2 个 UTF-16 码元，`nǐ` 从 2 起、长 2。
        assert_eq!(
            last_runs(&cap),
            vec![(2, 2, [0xC0, 0, 0, 255]), (5, 1, [2, 2, 2, 255])],
            "名字写的颜色在气泡里取 tooltip_error 而不是 error"
        );
        f.handle(UiCommand::SetTheme(theme(true)));
        assert_eq!(
            last_runs(&cap),
            vec![(2, 2, [0xFF, 0x80, 0x80, 255]), (5, 1, [3, 3, 3, 255])],
            "换明暗重推的气泡必须按新主题现算颜色"
        );
    }

    /// Linux：状态气泡 / Toast / tooltip 光栅化成 `CMD_OVERLAY_FRAME`，不发 macOS 的文本帧。
    #[cfg(all(target_os = "linux", ext_presenter))]
    mod linux_overlays {
        use super::*;
        use wind_bridge::shared_memory_posix::PosixSharedMemory;
        use wind_ipc::protocol::overlay::*;
        use wind_ui_types::{StatusTipAnchor, StatusTipPlacement};

        /// 解出一帧浮层元数据（按 `encode_overlay_frame` 的偏移）。
        struct Meta {
            kind: u32,
            w: u32,
            h: u32,
            flags: u32,
            place: u32,
            x: i32,
            y: i32,
            alt_y: i32,
            anchor: u32,
            margin: i32,
            duration: i32,
        }

        fn overlays(cap: &Arc<Mutex<Vec<Vec<u8>>>>, kind: u32) -> Vec<Meta> {
            cap.lock()
                .unwrap()
                .iter()
                .filter(|f| cmd_of(f) == CMD_OVERLAY_FRAME)
                .map(|f| {
                    let p = &f[8..];
                    let u = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
                    let i = |o: usize| i32::from_le_bytes(p[o..o + 4].try_into().unwrap());
                    Meta {
                        kind: u(0),
                        w: u(8),
                        h: u(12),
                        flags: u(16),
                        place: u(20),
                        x: i(24),
                        y: i(28),
                        alt_y: i(36),
                        anchor: u(40),
                        margin: i(44),
                        duration: i(64),
                    }
                })
                .filter(|m| m.kind == kind)
                .collect()
        }

        fn visible(m: &Meta) -> bool {
            m.flags & SharedRenderHeader::FLAG_VISIBLE != 0
        }

        fn no_text_frames(cap: &Arc<Mutex<Vec<Vec<u8>>>>) {
            let v = cap.lock().unwrap();
            for c in [CMD_STATUS_SHOW, CMD_TOAST_SHOW, CMD_TOOLTIP_SHOW] {
                assert!(
                    !v.iter().any(|f| cmd_of(f) == c),
                    "Linux 不该发 macOS 文本帧 0x{c:04X}"
                );
            }
        }

        fn status(f: &mut Forwarder, placement: StatusTipPlacement) {
            f.handle(UiCommand::ShowStatusTip {
                text: "中".into(),
                x: 10,
                y: 20,
                caret_height: 18,
                duration_ms: 1500,
                placement,
            });
        }

        /// 跟随光标：首选点 = 光标底端 + 4 + 偏移，备选点 = 光标顶端上方；像素真在 SHM 里。
        #[test]
        fn status_tip_caret_is_rasterized_with_flip_points() {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo1");
            status(
                &mut f,
                StatusTipPlacement::Caret {
                    offset_x: 3,
                    offset_y: 4,
                },
            );
            no_text_frames(&cap);
            let m = overlays(&cap, OVERLAY_KIND_STATUS);
            assert_eq!(m.len(), 1);
            let m = &m[0];
            assert!(visible(m));
            assert_eq!(m.place, OVERLAY_PLACE_FLIP);
            assert_eq!((m.x, m.y), (13, 20 + 18 + 4 + 4));
            assert!(m.alt_y < 20, "备选点在光标顶端之上");
            assert_eq!(m.duration, 1500, "自动隐藏时长随帧下发、归 addon 计时");
            let name =
                wind_bridge::endpoint::overlay_shm_name("_lo1", OVERLAY_KIND_STATUS).unwrap();
            let shm = PosixSharedMemory::open_readonly(&name, MAX_SHARED_RENDER_SIZE).unwrap();
            let hdr = shm.read_header();
            assert_eq!((hdr.width, hdr.height), (m.w, m.h));
            let px = &shm.pixels()[..(m.w * m.h * 4) as usize];
            assert!(px.chunks(4).any(|p| p[3] > 0), "位图不能是全透明");
        }

        #[test]
        fn status_tip_fixed_and_anchor_placements() {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo2");
            status(&mut f, StatusTipPlacement::Fixed { x: 500, y: 600 });
            status(
                &mut f,
                StatusTipPlacement::Anchor(StatusTipAnchor::WindowBottomLeft),
            );
            let m = overlays(&cap, OVERLAY_KIND_STATUS);
            assert_eq!(m.len(), 2);
            assert_eq!(m[0].place, OVERLAY_PLACE_ABSOLUTE);
            assert_eq!((m[0].x, m[0].y), (500, 600));
            assert_eq!(m[1].place, OVERLAY_PLACE_ANCHOR);
            assert_eq!(
                m[1].anchor, OVERLAY_ANCHOR_BOTTOM_LEFT,
                "窗口锚点拿不到窗口边框，降级为屏幕锚点"
            );
            assert_eq!(m[1].margin, 16);
        }

        #[test]
        fn hide_status_tip_after_show_emits_hidden_overlay() {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo3");
            f.handle(UiCommand::HideStatusTip);
            assert!(
                overlays(&cap, OVERLAY_KIND_STATUS).is_empty(),
                "没显示过不发"
            );
            status(&mut f, StatusTipPlacement::Fixed { x: 1, y: 1 });
            f.handle(UiCommand::HideStatusTip);
            let m = overlays(&cap, OVERLAY_KIND_STATUS);
            assert_eq!(m.len(), 2);
            assert!(visible(&m[0]) && !visible(&m[1]));
        }

        #[test]
        fn toast_is_rasterized_with_anchor() {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo4");
            f.handle(UiCommand::ShowToast {
                text: "ok".into(),
                position: crate::toast::ToastPosition::BottomCenter,
                kind: crate::toast::ToastKind::Success,
                duration_ms: 2000,
                accent: None,
            });
            no_text_frames(&cap);
            let m = overlays(&cap, OVERLAY_KIND_TOAST);
            assert_eq!(m.len(), 1);
            assert!(visible(&m[0]));
            assert_eq!(m[0].place, OVERLAY_PLACE_ANCHOR);
            assert_eq!(m[0].anchor, OVERLAY_ANCHOR_BOTTOM_CENTER);
            assert_eq!((m[0].margin, m[0].duration), (12, 2000));
        }

        /// 复制类操作的结果反馈也走浮层（此前 Linux 上这条 Toast 被 addon 丢弃）。
        #[test]
        fn copy_tooltip_text_reports_empty_when_no_tip() {
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo5");
            f.handle(UiCommand::CopyTooltipText(String::new()));
            let m = overlays(&cap, OVERLAY_KIND_TOAST);
            assert_eq!(m.len(), 1);
            assert_eq!(m[0].anchor, OVERLAY_ANCHOR_BOTTOM_RIGHT);
        }

        /// 悬停候选有反查内容 → tooltip 层跟着候选窗走；候选窗隐藏时 tooltip 一起藏。
        #[test]
        fn hover_tooltip_follows_candidate_and_hides_with_it() {
            use wind_ui_types::{StyledText, TooltipDoc, TooltipLine, TooltipSection};
            let cap = Arc::new(Mutex::new(Vec::new()));
            let (mut f, _ev) = mk(cap.clone(), "_lo6");
            let mut cand = item("你");
            cand.tooltip = Arc::new(TooltipDoc {
                sections: vec![TooltipSection {
                    title: None,
                    inline: false,
                    lines: vec![TooltipLine {
                        text: StyledText::from("nǐ"),
                        raw: 0,
                    }],
                }],
            });
            let update = |f: &mut Forwarder, hover: i32| {
                f.handle(UiCommand::UpdateCandidates {
                    preedit: "ni".into(),
                    preedit_caret: 2,
                    preedit_host_owned: false,
                    mode_label: "".into(),
                    candidates: vec![cand.clone()],
                    selected: 0,
                    hover,
                    page: 1,
                    total_pages: 1,
                    caret_x: 100,
                    caret_y: 200,
                    caret_height: 20,
                    caret_valid: true,
                    fixed: false,
                    fixed_x: 0,
                    fixed_y: 0,
                })
            };
            update(&mut f, -1);
            assert!(
                overlays(&cap, OVERLAY_KIND_TOOLTIP).is_empty(),
                "无悬停不发"
            );
            update(&mut f, 0);
            no_text_frames(&cap);
            let (sx, sy, _) = last_render_frame(&cap).unwrap();
            let m = overlays(&cap, OVERLAY_KIND_TOOLTIP);
            assert_eq!(m.len(), 1);
            assert!(visible(&m[0]));
            assert_eq!(m[0].place, OVERLAY_PLACE_FOLLOW_CANDIDATE);
            assert_eq!(m[0].duration, 0, "tooltip 常驻到下一帧");
            assert!(
                m[0].x >= sx && m[0].y > sy,
                "横排在候选行下方：({},{}) vs 候选窗 ({sx},{sy})",
                m[0].x,
                m[0].y
            );
            f.handle(UiCommand::HideCandidates);
            let m = overlays(&cap, OVERLAY_KIND_TOOLTIP);
            assert!(!visible(m.last().unwrap()), "候选窗藏了 tooltip 也得藏");
        }
    }
}
