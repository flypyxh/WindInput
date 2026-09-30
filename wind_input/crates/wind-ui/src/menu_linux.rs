//! Linux 外部宿主（Fcitx5 addon）的自绘菜单：主菜单 / 候选右键菜单。
//!
//! **复用 Windows 的 [`PopupMenu`] 整套**（级联状态机、视图树、主题、定位、命中测试、增量重绘），
//! 只换掉两头：
//!
//! - **出**：`PopupMenu` 的窗口在非 Windows 上是只持缓冲区的 mock。每次状态变化后把各级的
//!   像素写进该级自己的 SHM 段（`endpoint::overlay_shm_name`，`_MN0`…），推一帧
//!   `CMD_OVERLAY_FRAME`（kind = `OVERLAY_KIND_MENU + 级`，落位 `EXACT`）交 addon 贴图。
//!   落位在这边算完（工作区由 addon 随「打开菜单」请求报来），addon 原样摆放——命中测试
//!   按这个位置做，两边不能各算各的。
//! - **进**：Windows 菜单窗口自己收鼠标消息；这里改由 addon 抓住指针后把原始事件报上来
//!   （`CMD_MENU_POINTER` → 协调器 → [`UiCommand::MenuPointer`](crate::manager::UiCommand)）。
//!   键盘照旧由协调器 `forward_menu_key` 转发 `MenuKey`。
//!
//! 关闭与 `menu_open` 的对齐见 [`MenuHost::key`] / [`MenuHost::pointer`] 的自愈说明。

use wind_bridge::HostRenderSink;
use wind_bridge::shared_memory_posix::PosixSharedMemory;
use wind_ipc::codec::{OverlayFrameMeta, encode_overlay_frame};
use wind_ipc::protocol::overlay::{OVERLAY_KIND_MENU, OVERLAY_MENU_LEVELS, OVERLAY_PLACE_EXACT};
use wind_ipc::protocol::{MAX_SHARED_RENDER_SIZE, SharedRenderHeader};

use crate::manager::{MenuAnchor, MenuItemSpec, MenuPointerEvent, UiEvent};
use crate::popup_menu::{LevelKey, PopupMenu, set_host_work_area};

const LEVELS: usize = OVERLAY_MENU_LEVELS as usize;

pub(crate) struct MenuHost {
    menu: Option<PopupMenu>,
    events: std::sync::mpsc::Sender<UiEvent>,
    suffix: String,
    shms: Vec<Option<PosixSharedMemory>>,
    /// 各级上次推给宿主的内容（`None` = 该级没在显示）。
    pushed: Vec<Option<LevelKey>>,
    /// 最近一次指针位置：`MenuAnchor` 的 `i32::MIN` 哨兵（「更多…」弹主菜单）取它，
    /// 对位 Windows 的 `GetCursorPos`。
    last_pointer: Option<(i32, i32)>,
}

impl MenuHost {
    pub(crate) fn new(events: std::sync::mpsc::Sender<UiEvent>, suffix: String) -> Self {
        let menu = PopupMenu::new(events.clone())
            .map_err(|e| tracing::warn!("菜单渲染器创建失败: {e}"))
            .ok();
        Self {
            menu,
            events,
            suffix,
            shms: (0..LEVELS).map(|_| None).collect(),
            pushed: vec![None; LEVELS],
            last_pointer: None,
        }
    }

    pub(crate) fn set_theme(&mut self, sink: &dyn HostRenderSink, t: &wind_theme::Resolved) {
        if let Some(m) = &mut self.menu {
            m.set_theme(t);
            m.repaint();
        }
        self.sync(sink);
    }

    pub(crate) fn set_work_area(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
        let valid = right > left && bottom > top;
        set_host_work_area(valid.then_some((left, top, right, bottom)));
    }

    pub(crate) fn show(
        &mut self,
        sink: &dyn HostRenderSink,
        items: Vec<MenuItemSpec>,
        anchor: MenuAnchor,
    ) {
        let anchor = if anchor.x == i32::MIN || anchor.y == i32::MIN {
            // 宿主从没报过指针位置（键盘选的「更多…」且此前没动过鼠标）：落到工作区左上，
            // 总比 mock 的 GetCursorPos 给的 (0,0) 可控——两者此时其实同一个点。
            let (x, y) = self.last_pointer.unwrap_or((0, 0));
            MenuAnchor::at_point(x, y)
        } else {
            anchor
        };
        let Some(m) = &mut self.menu else {
            // 画不出来就当场关掉，否则协调器的 menu_open 一直为真、方向键回车全被吞。
            let _ = self.events.send(UiEvent::MenuClose);
            return;
        };
        m.show(items, anchor);
        self.sync(sink);
    }

    /// 协调器转发的菜单键。
    ///
    /// **自愈**：菜单已经不在屏上（被别的路径收掉了）而协调器还在转发键，说明两边对「菜单开着」
    /// 的认识错开了——回一条 `MenuClose` 让协调器复位，否则 Esc 也关不掉一个看不见的菜单，
    /// 方向键 / 回车 / Esc 会一直被吞，直到超时兜底。
    pub(crate) fn key(&mut self, sink: &dyn HostRenderSink, vk: u32) {
        match &mut self.menu {
            Some(m) if m.is_visible() => m.on_key(vk),
            _ => {
                let _ = self.events.send(UiEvent::MenuClose);
                return;
            }
        }
        self.sync(sink);
    }

    /// 宿主报来的指针事件。自愈同 [`Self::key`]。
    pub(crate) fn pointer(
        &mut self,
        sink: &dyn HostRenderSink,
        event: MenuPointerEvent,
        x: i32,
        y: i32,
    ) {
        self.last_pointer = Some((x, y));
        match &mut self.menu {
            Some(m) if m.is_visible() => m.on_pointer(event, x, y),
            _ => {
                let _ = self.events.send(UiEvent::MenuClose);
                // 宿主那边可能还挂着上一帧：再补推一遍隐藏帧。
                self.sync(sink);
                return;
            }
        }
        self.sync(sink);
    }

    /// 协调器要求收菜单（`HideMenu`）。收掉的是**可见**菜单时回送 `MenuClose`：协调器有几条路
    /// （候选收起时收候选右键菜单，见 `notify_ui_hide`）是在持 `state` 锁时发的 `HideMenu`，
    /// 那里不能顺手复位 `menu_open`，靠这条回送收口。菜单已自己收起（点了条目 / Esc）时不回送，
    /// 免得把紧接着打开的下一个菜单（「更多…」）关掉；协调器那边重复收到也是幂等的。
    pub(crate) fn hide(&mut self, sink: &dyn HostRenderSink) {
        if let Some(m) = &mut self.menu
            && m.is_visible()
        {
            m.hide();
            let _ = self.events.send(UiEvent::MenuClose);
        }
        self.sync(sink);
    }

    /// 把各级的当前帧与上次推过的比对，变了的写 SHM + 推帧，消失的推隐藏帧。
    fn sync(&mut self, sink: &dyn HostRenderSink) {
        let frames = self
            .menu
            .as_ref()
            .map(|m| m.level_frames())
            .unwrap_or_default();
        for k in 0..LEVELS {
            let kind = OVERLAY_KIND_MENU + k as u32;
            match frames.get(k) {
                Some(f) => {
                    if self.pushed[k].as_ref() == Some(&f.key) {
                        continue;
                    }
                    let (x, y, w, h) = f.geom;
                    let bytes = (w * h * 4) as usize;
                    if f.buf.len() < bytes
                        || bytes > MAX_SHARED_RENDER_SIZE - SharedRenderHeader::SIZE
                    {
                        tracing::warn!("菜单第 {k} 级位图 {w}x{h} 无效或过大，丢弃");
                        continue;
                    }
                    let Some(shm) = ensure_shm(&mut self.shms[k], &self.suffix, kind) else {
                        continue;
                    };
                    let seq = shm.write_frame(0, 0, w, h, &f.buf[..bytes]);
                    let mut flags =
                        SharedRenderHeader::FLAG_VISIBLE | SharedRenderHeader::FLAG_CONTENT_READY;
                    if f.software_shadow {
                        flags |= SharedRenderHeader::FLAG_SOFTWARE_SHADOW;
                    }
                    sink.push_frame(&encode_overlay_frame(&OverlayFrameMeta {
                        kind,
                        seq,
                        width: w,
                        height: h,
                        flags,
                        place: OVERLAY_PLACE_EXACT,
                        x,
                        y,
                        alt_x: x,
                        alt_y: y,
                        content_w: w,
                        content_h: h,
                        ..Default::default()
                    }));
                    self.pushed[k] = Some(f.key.clone());
                }
                None => {
                    if self.pushed[k].take().is_none() {
                        continue;
                    }
                    let seq = self.shms[k].as_mut().map(|s| s.write_hidden()).unwrap_or(0);
                    sink.push_frame(&encode_overlay_frame(&OverlayFrameMeta {
                        kind,
                        seq,
                        ..Default::default()
                    }));
                }
            }
        }
    }
}

fn ensure_shm<'a>(
    slot: &'a mut Option<PosixSharedMemory>,
    suffix: &str,
    kind: u32,
) -> Option<&'a mut PosixSharedMemory> {
    if slot.is_none() {
        let name = wind_bridge::endpoint::overlay_shm_name(suffix, kind)?;
        match PosixSharedMemory::create(&name, MAX_SHARED_RENDER_SIZE) {
            Ok(s) => *slot = Some(s),
            Err(e) => {
                tracing::warn!("create menu SHM {name} failed: {e}");
                return None;
            }
        }
    }
    slot.as_mut()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::{MenuCmd, MenuKind};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, channel};

    struct Cap(Mutex<Vec<Vec<u8>>>);
    impl HostRenderSink for Cap {
        fn push_frame(&self, f: &[u8]) {
            self.0.lock().unwrap().push(f.to_vec());
        }
    }
    impl Cap {
        fn new() -> Self {
            Self(Mutex::new(Vec::new()))
        }
        /// 取出累计的帧，解成 (kind, 可见, x, y, w, h)。
        fn take(&self) -> Vec<(u32, bool, i32, i32, u32, u32)> {
            let u = |f: &[u8], o: usize| u32::from_le_bytes(f[8 + o..12 + o].try_into().unwrap());
            std::mem::take(&mut *self.0.lock().unwrap())
                .iter()
                .map(|f| {
                    (
                        u(f, 0),
                        u(f, 16) & SharedRenderHeader::FLAG_VISIBLE != 0,
                        u(f, 24) as i32,
                        u(f, 28) as i32,
                        u(f, 8),
                        u(f, 12),
                    )
                })
                .collect()
        }
    }

    fn leaf(label: &str, cmd: MenuCmd) -> MenuItemSpec {
        MenuItemSpec::leaf(label, MenuKind::Command(cmd), true, false)
    }

    /// 两项顶层菜单，第二项带一个两项的子菜单。
    fn items() -> Vec<MenuItemSpec> {
        vec![
            leaf("全角", MenuCmd::ToggleWidth),
            MenuItemSpec::submenu(
                "主题",
                vec![
                    leaf("跟随系统", MenuCmd::ReloadConfig),
                    leaf("浅色", MenuCmd::OpenSettings),
                ],
            ),
        ]
    }

    fn host() -> (MenuHost, Receiver<UiEvent>, Cap) {
        let (tx, rx) = channel();
        // SHM 名带进程号与线程号：并发测试不撞段。
        let suffix = format!(
            "_tm{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        )
        .replace(['(', ')', ' '], "")
        .replace("ThreadId", "");
        let mut h = MenuHost::new(tx, suffix);
        h.set_work_area(0, 0, 1280, 800);
        (h, rx, Cap::new())
    }

    fn events(rx: &Receiver<UiEvent>) -> Vec<String> {
        rx.try_iter().map(|e| format!("{e:?}")).collect()
    }

    #[test]
    fn show_pushes_one_exact_frame_at_anchor_and_hide_pushes_hidden() {
        let (mut h, _rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(100, 200));
        let f = cap.take();
        assert_eq!(f.len(), 1, "只有顶层一级：{f:?}");
        let (kind, vis, x, y, w, hh) = f[0];
        assert_eq!(kind, OVERLAY_KIND_MENU);
        assert!(vis && w > 0 && hh > 0);
        assert_eq!((x, y), (100, 200), "无投影时窗口左上即锚点");
        h.hide(&cap);
        assert_eq!(
            events(&_rx),
            vec!["MenuClose".to_string()],
            "收可见菜单回送 MenuClose"
        );
        let f = cap.take();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].0, f[0].1), (OVERLAY_KIND_MENU, false));
        h.hide(&cap);
        assert!(cap.take().is_empty(), "已隐藏的级不重复推隐藏帧");
    }

    #[test]
    fn anchor_near_bottom_right_is_clamped_into_host_work_area() {
        let (mut h, _rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(1270, 790));
        let (_, _, x, y, w, hh) = cap.take()[0];
        assert!(
            x + w as i32 <= 1280 && y + hh as i32 <= 800,
            "@({x},{y}) {w}x{hh}"
        );
    }

    /// 悬停：鼠标移到第一项 → 重推一帧（高亮）；移到带子菜单的项 → 第 1 级出现在右侧；
    /// 再移到菜单外 → 只灭高亮，子菜单保留。
    #[test]
    fn hover_repaints_and_opens_submenu_to_the_right() {
        let (mut h, _rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(100, 100));
        let (_, _, x, y, w, hh) = cap.take()[0];
        let row = |i: i32| y + 4 + (hh as i32 - 8) * (2 * i + 1) / 4;
        h.pointer(&cap, MenuPointerEvent::Move, x + 20, row(0));
        let f = cap.take();
        assert_eq!(f.len(), 1, "高亮只重推顶层：{f:?}");
        h.pointer(&cap, MenuPointerEvent::Move, x + 20, row(0));
        assert!(cap.take().is_empty(), "同一项上移动不重推");
        h.pointer(&cap, MenuPointerEvent::Move, x + 20, row(1));
        let f = cap.take();
        let sub = f.iter().find(|t| t.0 == OVERLAY_KIND_MENU + 1).copied();
        let (_, vis, sx, _, _, _) = sub.unwrap_or_else(|| panic!("子菜单没出来：{f:?}"));
        assert!(vis && sx >= x + w as i32 - 10, "子菜单在右侧：x={sx}");
        h.pointer(&cap, MenuPointerEvent::Move, 5, 5);
        let f = cap.take();
        assert!(f.iter().all(|t| t.1), "移出菜单只灭高亮、不收子菜单：{f:?}");
    }

    #[test]
    fn click_on_leaf_reports_action_and_closes() {
        let (mut h, rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(100, 100));
        let (_, _, x, y, _, hh) = cap.take()[0];
        h.pointer(&cap, MenuPointerEvent::LeftPress, x + 20, y + hh as i32 / 4);
        let ev = events(&rx);
        assert!(
            ev.iter()
                .any(|e| e.contains("MenuAction") && e.contains("ToggleWidth")),
            "{ev:?}"
        );
        let f = cap.take();
        assert!(f.iter().any(|t| !t.1), "点选后菜单收起：{f:?}");
    }

    #[test]
    fn press_outside_or_right_press_closes_with_menu_close_event() {
        for ev in [
            MenuPointerEvent::LeftPress,
            MenuPointerEvent::RightPress,
            MenuPointerEvent::OtherPress,
        ] {
            let (mut h, rx, cap) = host();
            h.show(&cap, items(), MenuAnchor::at_point(100, 100));
            cap.take();
            h.pointer(&cap, ev, 900, 700);
            assert!(events(&rx).iter().any(|e| e == "MenuClose"), "{ev:?}");
            assert!(cap.take().iter().any(|t| !t.1), "{ev:?} 后菜单收起");
        }
    }

    #[test]
    fn keyboard_navigates_and_escape_closes() {
        let (mut h, rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(100, 100));
        cap.take();
        h.key(&cap, 0x28); // Down → 第一项高亮
        assert_eq!(cap.take().len(), 1);
        h.key(&cap, 0x28); // Down → 「主题」
        h.key(&cap, 0x27); // Right → 展开子菜单
        assert!(
            cap.take()
                .iter()
                .any(|t| t.0 == OVERLAY_KIND_MENU + 1 && t.1),
            "→ 展开子菜单"
        );
        h.key(&cap, 0x1B);
        assert!(events(&rx).iter().any(|e| e == "MenuClose"));
        let f = cap.take();
        assert_eq!(
            f.iter().filter(|t| !t.1).count(),
            2,
            "两级都推隐藏帧：{f:?}"
        );
    }

    /// 两端对「菜单开着」的认识错开时自愈：菜单不在屏上还收到键 / 指针，回 MenuClose。
    #[test]
    fn input_for_an_invisible_menu_reports_close() {
        let (mut h, rx, cap) = host();
        h.key(&cap, 0x28);
        assert_eq!(events(&rx), vec!["MenuClose".to_string()]);
        h.pointer(&cap, MenuPointerEvent::Move, 1, 1);
        assert_eq!(events(&rx), vec!["MenuClose".to_string()]);
        assert!(cap.take().is_empty());
    }

    #[test]
    fn more_item_anchor_sentinel_uses_last_pointer() {
        let (mut h, _rx, cap) = host();
        h.show(&cap, items(), MenuAnchor::at_point(100, 100));
        h.pointer(&cap, MenuPointerEvent::Move, 300, 400);
        h.hide(&cap);
        cap.take();
        h.show(&cap, items(), MenuAnchor::at_point(i32::MIN, i32::MIN));
        let (_, _, x, y, _, _) = cap.take()[0];
        assert_eq!((x, y), (300, 400));
    }
}
