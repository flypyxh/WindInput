//! Linux 托盘 / 面板模式图标的运行时渲染（`ext_presenter` 的 Linux 形态）。
//!
//! 对位 Windows 的 [`super::langbar_icon`]：那边把当前状态渲染进共享内存、DLL 的 `GetIcon` 去取；
//! 这边把（状态, 主字）渲染成 PNG 写进用户图标目录（`wind_ui::tray_icon`），addon 按同一规则算出
//! 图标名交给 Fcitx5。**顺序由数据依赖保证**：[`Coordinator::build_status`] 在返回之前确保本状态
//! 用得到的图标已在盘上，于是任何一帧带着新标签的状态帧到达 addon 时，文件必然已经写完。
//!
//! 设计与宿主行为见 `docs/design/linux-port.md` §5d。

use super::*;
use wind_ui::tray_icon::{TrayIcons, Variant, user_hicolor_dir};

/// 进程级单例：用户图标目录是进程外的共享资源，一个进程一份缓存足够。
/// 内层 `None` = 拿不到用户目录（没有 `HOME`），此后全部跳过、addon 退回种子图标。
static TRAY: std::sync::OnceLock<std::sync::Mutex<TrayState>> = std::sync::OnceLock::new();

struct TrayState {
    icons: Option<TrayIcons>,
    /// 已经报过警告的图标名：失败（缺字体、目录不可写）每次状态构建都会重试，只记一次日志。
    warned: std::collections::HashSet<String>,
    /// 启动那一轮（全部可用方案预渲染 + 清理）做过没有。
    primed: bool,
}

impl Coordinator {
    /// 本状态可能用到的托盘图标：当前方案标签（中文）、英文标签、大写锁定标签。
    ///
    /// 三张都要，不只当前这一张：addon 在本端判定大写锁定翻转（不等服务端的帧），那一刻要的
    /// 图标服务端还没为它构建过状态。
    ///
    /// `zh_label`：调用方已算出的有效中文态主字（`build_status` 在有效中文态下的 `icon_label`
    /// 就是它），给了就不再读一遍方案文件；`None` 时自己算。
    pub(super) fn ensure_tray_icons(&self, zh_label: Option<&str>) {
        let (zh, en, caps) = {
            let rt = self.rt();
            (
                zh_label.map_or_else(|| self.chinese_icon_label(), str::to_string),
                rt.config.ui.labels.english_label(),
                rt.config.ui.labels.caps_label(),
            )
        };
        let cell = TRAY.get_or_init(|| {
            std::sync::Mutex::new(TrayState {
                icons: user_hicolor_dir()
                    .map(|root| TrayIcons::new(root, wind_config::variant::is_dev())),
                warned: Default::default(),
                primed: false,
            })
        });
        let first = {
            let Ok(mut st) = cell.lock() else {
                return;
            };
            for (v, label) in [
                (Variant::Chinese, zh),
                (Variant::English, en),
                (Variant::Caps, caps),
            ] {
                Self::ensure_one(&mut st, v, &label);
            }
            !std::mem::replace(&mut st.primed, true)
        };
        if first {
            // 其余方案的图标与清理放后台：本状态要的三组上面已同步备好（数据依赖），其余的只为
            // 日后切方案时文件早已在盘上。全部同步做的话，方案多、字体缓存冷时第一次构建状态
            // 能到几百毫秒，而 addon 等每个响应只有 2 秒（超时即熔断）。
            let wanted = self.all_tray_labels();
            let spawned = std::thread::Builder::new()
                .name("tray-icons".into())
                .spawn(move || Self::prime_tray_icons(cell, &wanted));
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "托盘图标后台预渲染线程起不来，其余方案的图标等用到时再画");
            }
        }
    }

    /// 有效中文态的主字：方案 `icon_label`，未配为「中」（与 [`Self::mode_icon_label`] 同一回落）。
    fn chinese_icon_label(&self) -> String {
        let id = self.engine_mgr.active_schema_id();
        let lbl = self.engine_mgr.schema_icon_label(&id);
        if lbl.is_empty() {
            "中".to_string()
        } else {
            lbl
        }
    }

    /// 全部可用方案的中文态主字 + 英文 / 大写标签。
    fn all_tray_labels(&self) -> Vec<(Variant, String)> {
        let rt = self.rt();
        let mut out: Vec<(Variant, String)> = self
            .engine_mgr
            .available_schemas()
            .iter()
            .map(|id| {
                let l = self.engine_mgr.schema_icon_label(id);
                (Variant::Chinese, if l.is_empty() { "中".into() } else { l })
            })
            .collect();
        out.push((Variant::English, rt.config.ui.labels.english_label()));
        out.push((Variant::Caps, rt.config.ui.labels.caps_label()));
        out
    }

    /// 启动一轮（后台线程）：把全部可用方案的图标先写出来，再删掉不在其中的旧运行时图标。
    ///
    /// 先写是为了宿主的图标主题缓存：GTK 的 `GtkIconTheme` 按目录 mtime 最多每 5 秒重扫一次，
    /// 新文件刚写出就被引用可能要等一轮；启动时写好，日常切方案引用的都是早已存在的文件。
    /// 清理只在这里做：运行中新出现的标签（改了方案标签、加了方案）只增不删，下次启动再收——
    /// 数量上限就是「可用方案数 + 2」，不会无限增长。只删本变体的（正式版与 dev 版各管各的）。
    ///
    /// 每画一组拿一次锁、画完就放：前台构建状态至多等一组（几十毫秒），不等整轮。
    fn prime_tray_icons(cell: &std::sync::Mutex<TrayState>, wanted: &[(Variant, String)]) {
        let t0 = std::time::Instant::now();
        let lock = || cell.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(icons) = lock().icons.as_ref()
            && let Err(e) = icons.create_dirs()
        {
            tracing::warn!(error = %e, "托盘图标目录建不起来，addon 将退回随包的种子图标");
        }
        let mut keep = std::collections::HashSet::new();
        for (v, label) in wanted {
            let mut st = lock();
            if Self::ensure_one(&mut st, *v, label)
                && let Some(icons) = st.icons.as_ref()
            {
                keep.insert(icons.name(*v, label));
            }
        }
        let mut st = lock();
        if let Some(icons) = st.icons.as_mut() {
            let removed = icons.prune(&keep);
            tracing::info!(
                dir = %icons.root().display(),
                icons = keep.len(),
                removed,
                elapsed_ms = t0.elapsed().as_millis() as u64,
                "托盘图标已就绪"
            );
        }
    }

    /// 返回这一张是否就绪。
    fn ensure_one(st: &mut TrayState, v: Variant, label: &str) -> bool {
        let Some(icons) = st.icons.as_mut() else {
            return false;
        };
        match icons.ensure(v, label) {
            Ok(r) => r.is_some(),
            Err(e) => {
                if st.warned.insert(icons.name(v, label)) {
                    tracing::warn!(error = %e, label, "托盘图标渲染失败，addon 将退回随包的种子图标");
                }
                false
            }
        }
    }
}
