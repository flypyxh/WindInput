//! 焦点窗口上下文：`compat.toml` 规则按窗口类名 / 标题匹配的协调器侧（P2，设计见
//! `docs/design/compat-window-match.md`「服务端判定」）。
//!
//! 三件事都收在这里，调用点只认这几个入口：
//! - **记账**：`client_token → 最近一次 focus_gained 的窗口`（[`FocusWindow`]）。ime_activated、
//!   按 token 推 DLL 配置都沿用这张表；按 pid 查的路径取该 pid 最近一次获焦的那个窗口。
//! - **解析**：有窗口上下文走 `AppCompat::resolve`，没有退回按进程名（`get_rule`）——两者在窗口
//!   为空时结果相同，后者只是省掉一次合成与缓存键的分配。
//! - **跨越判据**：进入类字段（初始中英 / 标点 / 方案）只在「pid 变了，或命中的带窗口条件的进入类
//!   规则集合变了」时重算；带标题条件的规则不进这个集合，故**仅标题变化永不重算**。
//!
//! 标题由 P3 上报（FocusGained 变长段），在那之前 [`FocusWindow::title`] 恒为空串，带 title 的规则
//! 一律不命中；代码路径已按「有标题」写好，P3 只需改 [`FocusWindow::of_focus`] 一处。

use super::*;
use std::hash::{Hash, Hasher};
use wind_config::app_compat::{AppCompatRule, ResolvedRule, RuleId, WindowCtx};

/// 一个焦点窗口的身份：顶层窗口类名 + 标题。拿不到的项为空串，带对应条件的规则一律不命中
/// （「不知道是哪个窗口」不能套窗口规则）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FocusWindow {
    pub(crate) class: String,
    pub(crate) title: String,
}

impl FocusWindow {
    /// 从 FocusGained 取。⚠ 标题要等 P3（DLL 上报 + 协议）——届时只改这一处。
    pub(crate) fn of_focus(data: &FocusData) -> Self {
        FocusWindow {
            class: data.window_class.clone(),
            title: String::new(),
        }
    }

    /// 只有类名（DLL 同步路径 `get_current_mode` 只带类名）。
    pub(crate) fn of_class(class: &str) -> Self {
        FocusWindow {
            class: class.to_string(),
            title: String::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.class.is_empty() && self.title.is_empty()
    }

    /// `ActiveCompat` 的缓存键分量：(类名小写, 标题) 的哈希。规则按类名不区分大小写匹配，
    /// 类名大小写不同的两次焦点必然解析出同一结果，不必重算。
    pub(crate) fn cache_hash(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.class.to_lowercase().hash(&mut h);
        self.title.hash(&mut h);
        h.finish()
    }

    pub(crate) fn ctx<'a>(&'a self, process: &'a str) -> WindowCtx<'a> {
        WindowCtx {
            process,
            class: &self.class,
            title: &self.title,
        }
    }
}

/// token 窗口表的上限：客户端断开时 bridge 不带 token，表靠容量淘汰最旧的一条。
const FOCUS_WINDOWS_CAP: usize = 256;

/// `client_token → 最近一次 focus_gained 的窗口`，外加 `active_compat` 当前按哪个窗口解析。
#[derive(Debug, Default)]
pub(crate) struct FocusWindows {
    /// 值的第二项是记录序号，按 pid 查时取序号最大（最近获焦）的那条。
    by_token: HashMap<u64, (FocusWindow, u64)>,
    seq: u64,
    /// `active_compat` 当前那份规则是按哪个窗口解析的（与 `ActiveCompat::window_hash` 同步写）。
    active: FocusWindow,
}

/// 「初始模式该按谁算」：上一次**真正参与**初始模式决策的宿主，见 `Coordinator::mode_scope`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModeScope {
    pub(crate) pid: u32,
    /// 该宿主是否配了初始状态规则（`initial_mode` / `initial_punct`）。
    pub(crate) has_rule: bool,
    /// 命中的带窗口条件的进入类规则（不含带标题条件的），见 [`entry_window_key`]。
    pub(crate) entry_windows: Vec<RuleId>,
    /// 推进本记录的那次焦点的窗口：规则表重载后按它重算 `entry_windows`
    /// （[`Coordinator::refresh_mode_scope_after_reload`]）。不能取「该 pid 最近获焦的窗口」：
    /// 作用域外的过渡窗口（explorer 任务栏）不推进本记录，却会成为最近的那个。
    pub(crate) window: FocusWindow,
}

/// 进入类跨越判据用的规则集合：`entry_window_matches` 去掉带标题条件的。
///
/// 去掉标题规则 = 「仅标题变化永不重算初始模式」（设计稿定稿决策）：同一个窗口的标题随浏览器
/// 标签页、编辑器文件不断变化，拿它当跨越会把用户手切的中英态反复冲掉。代价是标题规则里的进入类
/// 字段只在 pid / 类名集合跨越时随之生效。
pub(crate) fn entry_window_key(r: &ResolvedRule) -> Vec<RuleId> {
    r.entry_window_matches
        .iter()
        .filter(|id| id.title.is_empty())
        .cloned()
        .collect()
}

/// 焦点是否「切进来」（进入类字段该不该重算的前一半判据，另一半见 `should_reapply_initial`）。
///
/// pid 变了，或同一进程内命中的带窗口条件的进入类规则集合变了。pid 未知（0）一律不算。
pub(crate) fn entry_crossed(old: &ModeScope, new_pid: u32, new_entry: &[RuleId]) -> bool {
    new_pid != 0 && (old.pid != new_pid || old.entry_windows != new_entry)
}

impl Coordinator {
    /// 记下 `token` 这次获焦的窗口；与它上一次记录（没有 = 空窗口）不同时返回 true。
    ///
    /// 返回值驱动 DLL 配置重推：DLL 的占位字符 / 密码抑制 / 英文配对是按 token 持有的一份值，
    /// 同一 token（同一 TSF 实例）换到另一个窗口时规则可能变了，不重推 DLL 就一直用旧窗口的值。
    pub(crate) fn note_focus_window(&self, token: u64, win: FocusWindow) -> bool {
        if token == 0 {
            return false;
        }
        let mut t = self.focus_windows.lock().unwrap_or_else(|e| e.into_inner());
        t.seq += 1;
        let seq = t.seq;
        // 没有记录 ≡ 空窗口：握手时推过去的就是按空窗口算的值。
        let changed = match t.by_token.get(&token) {
            Some((prev, _)) => *prev != win,
            None => !win.is_empty(),
        };
        t.by_token.insert(token, (win, seq));
        if t.by_token.len() > FOCUS_WINDOWS_CAP
            && let Some(oldest) = t
                .by_token
                .iter()
                .min_by_key(|(_, (_, s))| *s)
                .map(|(k, _)| *k)
        {
            t.by_token.remove(&oldest);
        }
        changed
    }

    /// `token` 最近一次获焦的窗口；没记录过返回空窗口（不匹配任何窗口规则）。
    pub(crate) fn focus_window_of_token(&self, token: u64) -> FocusWindow {
        self.focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_token
            .get(&token)
            .map(|(w, _)| w.clone())
            .unwrap_or_default()
    }

    /// `token` 的窗口；它从没获焦过（只发了 ime_activated 的新实例）则退回该 pid 最近获焦的窗口。
    ///
    /// 只给「焦点应用是谁」的两处用（`update_active_compat` 的 active 槽、`apply_initial_mode`）。
    /// 推给 DLL 的配置**不**用它：那是按 token 持有的值，没获焦过的 token 不该套别的实例的窗口。
    pub(crate) fn focus_window_of_token_or_pid(&self, token: u64) -> FocusWindow {
        if let Some((w, _)) = self
            .focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_token
            .get(&token)
        {
            return w.clone();
        }
        self.focus_window_of_pid((token >> 32) as u32)
    }

    /// 清掉 `pid` 名下的全部窗口记录（PID 复用：上一任进程的窗口不属于新进程）。
    /// 唯一调用方 `revalidate_pid_name` 同此 cfg。
    #[cfg(any(windows, test))]
    pub(crate) fn forget_focus_windows_of_pid(&self, pid: u32) {
        self.focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_token
            .retain(|k, _| (*k >> 32) as u32 != pid);
    }

    /// `pid` 最近一次获焦的窗口（它名下各 token 里最新的那条）；没记录过返回空窗口。
    ///
    /// 给只知道 pid 的路径用（输入诊断上报、UIElement 推断、按 pid 的推送判定）。对刚获焦的
    /// 那个 token，它与 [`Self::focus_window_of_token`] 必然相同——密码抑制的两侧（服务端
    /// `apply_input_diag` 按 pid、推给 DLL 的按 token）因此对焦点实例算出同一个值。
    pub(crate) fn focus_window_of_pid(&self, pid: u32) -> FocusWindow {
        if pid == 0 {
            return FocusWindow::default();
        }
        self.focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .by_token
            .iter()
            .filter(|(k, _)| (**k >> 32) as u32 == pid)
            .max_by_key(|(_, (_, s))| *s)
            .map(|(_, (w, _))| w.clone())
            .unwrap_or_default()
    }

    /// `active_compat` 当前按哪个窗口解析（焦点应用的窗口上下文）。
    pub(crate) fn active_focus_window(&self) -> FocusWindow {
        self.focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .clone()
    }

    /// 只供 `update_active_compat` / 测试同步写：与 `ActiveCompat::window_hash` 同步。
    pub(super) fn set_active_focus_window(&self, win: FocusWindow) {
        self.focus_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active = win;
    }

    /// 按 (进程名, 窗口) 解析规则后交给 `f`。**规则查表的唯一入口**（[[commit_newline]] 与
    /// HostRender 白名单除外，它们按设计不支持窗口条件）。
    ///
    /// 进程名为空 ⇒ `None`（空名不吃通配，口径同 `AppCompat::get_rule`）。窗口为空 ⇒ 按进程名
    /// 查（不分配、不合成），与 `resolve` 空窗口的结果相同。
    pub(crate) fn with_compat_rule<R>(
        &self,
        process: &str,
        win: &FocusWindow,
        f: impl FnOnce(Option<&AppCompatRule>) -> R,
    ) -> R {
        if process.is_empty() {
            return f(None);
        }
        let table = self.app_compat.lock().unwrap_or_else(|e| e.into_inner());
        if win.is_empty() {
            return f(table.get_rule(process));
        }
        let resolved = table.resolve(&win.ctx(process));
        drop(table);
        f(resolved.rule.as_ref())
    }

    /// 按 (进程名, 窗口) 解析，连同命中的窗口规则集合一起返回（跨越判据要用）。
    pub(crate) fn resolve_compat(
        &self,
        process: &str,
        win: &FocusWindow,
    ) -> std::sync::Arc<ResolvedRule> {
        if process.is_empty() {
            return Default::default();
        }
        self.app_compat
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(&win.ctx(process))
    }

    /// 焦点应用（`active_compat.pid` + 它的窗口）的规则。
    pub(crate) fn with_active_compat_rule<R>(
        &self,
        f: impl FnOnce(Option<&AppCompatRule>) -> R,
    ) -> R {
        let name = self.active_process_name();
        let win = self.active_focus_window();
        self.with_compat_rule(&name, &win, f)
    }

    /// pid → 进程名：先读缓存，没有再反查（推送路径用，**不得**用在 DLL 同步阻塞路径上）。
    pub(super) fn proc_name_or_lookup(&self, pid: u32) -> String {
        if pid == 0 {
            return String::new();
        }
        let cached = self
            .pid_names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .cloned();
        cached.unwrap_or_else(|| process_name(pid))
    }

    /// 指定 token（推给它的 DLL 配置）的规则：该 token 自己的窗口；没获焦过 = 无窗口上下文，
    /// 不匹配窗口规则。
    pub(crate) fn with_token_compat_rule<R>(
        &self,
        token: u64,
        f: impl FnOnce(Option<&AppCompatRule>) -> R,
    ) -> R {
        let name = self.proc_name_or_lookup((token >> 32) as u32);
        let win = self.focus_window_of_token(token);
        self.with_compat_rule(&name, &win, f)
    }

    /// 指定 pid 的规则：该 pid 最近一次获焦的窗口；没获焦过 = 不匹配窗口规则。
    pub(crate) fn with_pid_compat_rule<R>(
        &self,
        pid: u32,
        f: impl FnOnce(Option<&AppCompatRule>) -> R,
    ) -> R {
        let name = self.proc_name_or_lookup(pid);
        let win = self.focus_window_of_pid(pid);
        self.with_compat_rule(&name, &win, f)
    }

    /// 规则表重载后按新表重算 `mode_scope.entry_windows`（窗口取推进它的那次焦点）。
    ///
    /// 不重算的话，记录里留着按旧表算的集合，重载后在**同一个窗口**里的下一次 focus_gained
    /// 就会因「集合变了」被当成切进来，把用户手切的中英态按初始规则冲掉。
    /// `has_rule` 不动：它的刷新沿用既有口径（菜单改初始规则时刷 `active_compat`）。
    pub(crate) fn refresh_mode_scope_after_reload(&self) {
        let (pid, win) = {
            let m = self.mode_scope.lock().unwrap_or_else(|e| e.into_inner());
            (m.pid, m.window.clone())
        };
        if pid == 0 {
            return;
        }
        let name = self.cached_proc_name((pid as u64) << 32);
        let entry = entry_window_key(&self.resolve_compat(&name, &win));
        let mut m = self.mode_scope.lock().unwrap_or_else(|e| e.into_inner());
        if m.pid == pid {
            m.entry_windows = entry;
        }
    }

    /// 菜单写回（P4 之前只写纯进程键）前的提示：焦点窗口的生效值与「只按进程名」解析的值不同，
    /// 说明当前值来自带窗口条件的规则，写回纯进程键可能被它压住、看起来「点了没反应」。
    /// 只打 warn（不含标题文本，标题按用户数据处理），返回是否命中，行为不变。
    pub(crate) fn warn_if_menu_writeback_shadowed<T: PartialEq>(
        &self,
        what: &str,
        pick: impl Fn(Option<&AppCompatRule>) -> T,
    ) -> bool {
        let name = self.active_process_name();
        let win = self.active_focus_window();
        if name.is_empty() || win.is_empty() {
            return false;
        }
        let shadowed = self.with_compat_rule(&name, &win, &pick)
            != self.with_compat_rule(&name, &FocusWindow::default(), &pick);
        if shadowed {
            tracing::warn!(
                "{what}: 当前窗口（class={:?} title_len={}）的生效值来自带窗口条件的规则，\
                 菜单只写回 {name} 的进程规则，可能被窗口规则压住而不生效",
                win.class,
                win.title.chars().count()
            );
        }
        shadowed
    }

    /// 焦点换了窗口（同一 token 的窗口上下文变了）时，给该 token 重推按规则现算的 DLL 配置。
    ///
    /// 清单与 `revalidate_pid_name` 的补推一致：这三项都是「按目标客户端现算」的。握手推的值按
    /// 空窗口算（那时还没有焦点），故首次获焦带来类名时同样要推（`Chrome_WidgetWin_*` 的占位
    /// 字符就靠这一下才到 DLL）。
    pub(crate) fn repush_window_scoped_dll_config(&self, token: u64) {
        if token == 0 {
            return;
        }
        self.push_password_suppress_config(token);
        self.push_english_pair_config(token);
        self.push_composition_placeholder_config(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(class: &str, title: &str) -> RuleId {
        RuleId {
            process: "a.exe".into(),
            class: class.into(),
            title: title.into(),
        }
    }

    #[test]
    fn entry_key_drops_title_rules_so_title_only_changes_never_cross() {
        let r = ResolvedRule {
            entry_window_matches: vec![id("c", ""), id("", "t*"), id("c", "t*")],
            ..Default::default()
        };
        assert_eq!(entry_window_key(&r), vec![id("c", "")]);
    }

    #[test]
    fn entry_crossed_on_pid_change_or_entry_set_change_only() {
        let old = ModeScope {
            pid: 7,
            has_rule: false,
            entry_windows: vec![id("c", "")],
            ..Default::default()
        };
        assert!(entry_crossed(&old, 8, &[id("c", "")]), "pid 变了");
        assert!(
            entry_crossed(&old, 7, &[]),
            "同进程，进入类窗口规则集合变了"
        );
        assert!(!entry_crossed(&old, 7, &[id("c", "")]), "同进程同集合");
        assert!(!entry_crossed(&old, 0, &[]), "pid 未知不算");
    }

    #[test]
    fn noting_a_window_reports_change_against_the_tokens_previous_window() {
        let c = Coordinator::new_headless(Config::default(), None);
        let t = (5u64 << 32) | 1;
        assert!(
            !c.note_focus_window(t, FocusWindow::default()),
            "首次且空窗口 ≡ 握手时的假设"
        );
        assert!(c.note_focus_window(t, FocusWindow::of_class("A")));
        assert!(!c.note_focus_window(t, FocusWindow::of_class("A")));
        assert!(c.note_focus_window(t, FocusWindow::of_class("B")));
        assert!(
            c.note_focus_window(t, FocusWindow::default()),
            "回到空窗口也是变化"
        );
        assert!(
            !c.note_focus_window(0, FocusWindow::of_class("A")),
            "token 0 不记"
        );
    }

    #[test]
    fn pid_window_is_the_latest_focused_token_of_that_pid() {
        let c = Coordinator::new_headless(Config::default(), None);
        c.note_focus_window((5u64 << 32) | 1, FocusWindow::of_class("A"));
        c.note_focus_window((5u64 << 32) | 2, FocusWindow::of_class("B"));
        c.note_focus_window((6u64 << 32) | 1, FocusWindow::of_class("C"));
        assert_eq!(c.focus_window_of_pid(5).class, "B");
        c.note_focus_window((5u64 << 32) | 1, FocusWindow::of_class("A2"));
        assert_eq!(c.focus_window_of_pid(5).class, "A2");
        assert!(c.focus_window_of_pid(9).is_empty());
        assert_eq!(c.focus_window_of_token((5u64 << 32) | 2).class, "B");
        assert!(c.focus_window_of_token((9u64 << 32) | 2).is_empty());
    }

    #[test]
    fn token_table_is_capped_by_evicting_the_oldest() {
        let c = Coordinator::new_headless(Config::default(), None);
        for i in 0..(FOCUS_WINDOWS_CAP as u64 + 1) {
            c.note_focus_window((1u64 << 32) | i, FocusWindow::of_class("X"));
        }
        assert!(
            c.focus_window_of_token(1u64 << 32).is_empty(),
            "最旧的被淘汰"
        );
        assert_eq!(
            c.focus_window_of_token((1u64 << 32) | FOCUS_WINDOWS_CAP as u64)
                .class,
            "X"
        );
    }

    // ── 协调器层：焦点事件 → 窗口上下文 → 规则 ───────────────────────────────

    use wind_config::app_compat::AppCompat;

    /// 走生产加载路径（`load_layered`）：带 class / title 的规则只能从 TOML 来。
    fn load_rules(tag: &str, text: &str) -> AppCompat {
        let d = std::env::temp_dir().join(format!("wind_cwm_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(wind_config::app_compat::COMPAT_FILE_NAME), text).unwrap();
        let c = AppCompat::load_layered(Some(&d), None, None);
        let _ = std::fs::remove_dir_all(&d);
        c
    }

    fn coord_with_rules(tag: &str, text: &str) -> Arc<Coordinator> {
        let mut cfg = Config::default();
        cfg.input.default.chinese_mode = true;
        let c = Coordinator::new_headless(cfg, None);
        *c.app_compat.lock().unwrap() = load_rules(tag, text);
        c
    }

    fn focus(token: u64, class: &str) -> FocusData {
        FocusData {
            x: 0,
            y: 0,
            height: 0,
            composition_start_x: 0,
            composition_start_y: 0,
            client_token: token,
            input_scope_mask: 0,
            disabled: false,
            reason: 0,
            caret_source: 0,
            bundle_id: String::new(),
            window_class: class.into(),
        }
    }

    fn chinese(c: &Coordinator) -> bool {
        c.state.lock().unwrap().chinese_mode
    }

    fn placeholder_msg(kind: u8) -> Vec<u8> {
        wind_ipc::codec::encode_sync_config(
            wind_ipc::protocol::CONFIG_KEY_COMPOSITION_PLACEHOLDER,
            &wind_ipc::codec::encode_composition_placeholder_value(kind),
        )
    }

    /// 收到的占位字符推送：`None` = 没推，`Some(kind)` = 推了一次（0 空格 / 1 ZWSP / 2 盲文空白）。
    fn pushed_placeholder(cap: &std::sync::mpsc::Receiver<Vec<u8>>) -> Option<u8> {
        let got: Vec<Vec<u8>> = cap.try_iter().collect();
        let kinds: Vec<u8> = (0u8..=2)
            .filter(|&k| got.contains(&placeholder_msg(k)))
            .collect();
        match kinds.as_slice() {
            [] => None,
            [k] => Some(*k),
            _ => panic!("同一轮收到多种值：{kinds:?}"),
        }
    }

    const AHK: &str = r#"
[[apps]]
process = "AutoHotkey.exe"
class = "AHK_A"
caret_offset_x = 5
initial_mode = "english"

[[apps]]
process = "AutoHotkey.exe"
class = "AHK_B"
caret_offset_x = 9
initial_mode = "chinese"
"#;

    /// 论坛 t270：AutoHotkey 同一进程的两个 GUI 窗口（类名不同）各自命中自己的规则——持续类
    /// （caret_offset）随焦点即时刷新，进入类（initial_mode）在两窗口间切换时也重算。
    #[test]
    fn ahk_two_windows_of_one_process_each_hit_their_own_rule() {
        let c = coord_with_rules("ahk", AHK);
        c.pid_names
            .lock()
            .unwrap()
            .insert(300, "autohotkey.exe".into());
        let t = (300u64 << 32) | 1;

        c.handle_focus_gained(&focus(t, "AHK_A"));
        assert_eq!(c.active_compat.lock().unwrap().caret_offset_x, 5);
        assert!(!chinese(&c), "A 窗口 initial_mode = english");

        c.handle_focus_gained(&focus(t, "AHK_B"));
        assert_eq!(
            c.active_compat.lock().unwrap().caret_offset_x,
            9,
            "同 pid 换窗口，持续类字段必须刷新（缓存键含窗口）"
        );
        assert!(
            chinese(&c),
            "B 窗口 initial_mode = chinese：命中集合变了 ⇒ 重算"
        );

        c.handle_focus_gained(&focus(t, "AHK_A"));
        assert!(!chinese(&c), "回到 A 再按 A 的规则");

        // 停在 A 里手切中文，再在 A 内换输入框（同窗口的 focus_gained）：不重算，尊重手切。
        c.state.lock().unwrap().chinese_mode = true;
        c.handle_focus_gained(&focus(t, "AHK_A"));
        assert!(chinese(&c), "同窗口内换焦点不重算初始模式");
    }

    /// DLL 同步路径（`get_current_mode`）跑在重型段记账之前：它必须用自己带来的类名判跨越、
    /// 算初始模式，而不是 token 表里的上一个窗口——否则首键会按上一个窗口的模式处理。
    #[test]
    fn get_current_mode_uses_the_class_it_was_given() {
        let c = coord_with_rules("ahk_sync", AHK);
        c.pid_names
            .lock()
            .unwrap()
            .insert(300, "autohotkey.exe".into());
        let t = (300u64 << 32) | 1;
        c.handle_focus_gained(&focus(t, "AHK_A"));
        assert!(!chinese(&c));
        let (cn, _, _) = c.get_current_mode(t, "AHK_B");
        assert!(cn, "同步回传必须已按 B 的规则（中文）");
    }

    /// GH#175：出厂规则换成类名通配后，Edge 焦点下协调器自己发的占位与推给 DLL 的兜底占位
    /// 都是 ZWSP；同一 token 换到非 Chromium 窗口时持续类字段刷新、DLL 配置重推。
    #[test]
    fn chromium_class_wildcard_gives_zwsp_and_repushes_on_window_change() {
        let c = coord_with_rules(
            "chromium",
            "[[apps]]\nclass = \"Chrome_WidgetWin_*\"\ncomposition_placeholder = \"zwsp\"\n",
        );
        c.pid_names.lock().unwrap().insert(42, "msedge.exe".into());
        let t = (42u64 << 32) | 1;
        let cap = c.push_server.attach_capture_client(t);

        // 握手：还没有焦点窗口 ⇒ 类名规则不命中 ⇒ 空格。
        c.push_composition_placeholder_config(t);
        assert_eq!(pushed_placeholder(&cap), Some(0));

        c.handle_focus_gained(&focus(t, "Chrome_WidgetWin_1"));
        assert_eq!(
            c.composition_placeholder(),
            "\u{200B}",
            "协调器自己发的占位"
        );
        assert_eq!(
            pushed_placeholder(&cap),
            Some(1),
            "首次带类名获焦必须重推：握手那份是按空窗口算的"
        );

        c.handle_focus_gained(&focus(t, "Chrome_WidgetWin_1"));
        assert_eq!(pushed_placeholder(&cap), None, "窗口没变不重推");

        c.handle_focus_gained(&focus(t, "#32770"));
        assert_eq!(c.composition_placeholder(), COMPOSITION_PLACEHOLDER);
        assert_eq!(
            pushed_placeholder(&cap),
            Some(0),
            "换到非 Chromium 窗口重推空格"
        );
    }

    /// GH#175 第三档：`blank`（U+2800）——协调器自己发的占位与推给 DLL 的档位（2）都要跟上。
    /// 钉钉在线表格一类「靠出现内容进入编辑态」的宿主用它（ZWSP 被当成没内容）。
    #[test]
    fn blank_rule_gives_braille_blank_and_pushes_kind_2() {
        let c = coord_with_rules(
            "blank",
            "[[apps]]\nprocess = \"DingTalk.exe\"\ncomposition_placeholder = \"blank\"\n",
        );
        c.pid_names
            .lock()
            .unwrap()
            .insert(42, "dingtalk.exe".into());
        let t = (42u64 << 32) | 1;
        let cap = c.push_server.attach_capture_client(t);

        c.push_composition_placeholder_config(t);
        assert_eq!(pushed_placeholder(&cap), Some(2), "blank 下发值 = 2");

        c.handle_focus_gained(&focus(t, "StandardFrame_DingTalk"));
        assert_eq!(
            c.composition_placeholder(),
            "\u{2800}",
            "协调器自己发的占位"
        );
    }

    /// 进入类字段只在「命中的带窗口条件的**进入类**规则集合」变了时重算：只管占位的类名规则
    /// 命中与否（VS Code 里弹出原生对话框）不算切进来，进程规则配的 initial_mode 不得冲掉手切。
    #[test]
    fn non_entry_window_rules_do_not_make_a_same_process_switch_count_as_entering() {
        let c = coord_with_rules(
            "entry",
            r#"
[[apps]]
class = "Chrome_WidgetWin_*"
composition_placeholder = "zwsp"

[[apps]]
process = "Code.exe"
initial_mode = "english"
"#,
        );
        c.pid_names.lock().unwrap().insert(400, "code.exe".into());
        let t = (400u64 << 32) | 1;
        c.handle_focus_gained(&focus(t, "Chrome_WidgetWin_1"));
        assert!(!chinese(&c), "切进 Code.exe 套 initial_mode");
        c.state.lock().unwrap().chinese_mode = true; // 用户手切中文
        c.handle_focus_gained(&focus(t, "#32770"));
        assert!(
            chinese(&c),
            "对话框：窗口规则集合变了，但进入类集合没变 ⇒ 不重算"
        );
        c.handle_focus_gained(&focus(t, "Chrome_WidgetWin_1"));
        assert!(chinese(&c), "回到主窗口同理");
    }

    /// 仅标题变化永不重算初始模式：带标题条件的规则不进跨越集合。标题要等 P3 上报，这里直接
    /// 在解析层钉判据（`focus_gained` 的跨越就是 `entry_crossed(entry_window_key(resolve))`）。
    #[test]
    fn title_only_change_never_counts_as_entering() {
        let c = coord_with_rules(
            "title",
            r#"
[[apps]]
process = "app.exe"
title = "Doc*"
initial_mode = "english"

[[apps]]
process = "app.exe"
class = "Main"
title = "Doc*"
initial_punct = "english"
"#,
        );
        let a = FocusWindow {
            class: "Main".into(),
            title: "Doc 1".into(),
        };
        let b = FocusWindow {
            class: "Main".into(),
            title: "Other".into(),
        };
        let ra = c.resolve_compat("app.exe", &a);
        let rb = c.resolve_compat("app.exe", &b);
        assert!(
            ra.rule.as_ref().and_then(|r| r.initial_mode).is_some()
                && rb.rule.as_ref().and_then(|r| r.initial_mode).is_none(),
            "前置：标题规则确实只命中 A"
        );
        assert_ne!(ra.entry_window_matches, rb.entry_window_matches);
        let scope = ModeScope {
            pid: 5,
            has_rule: true,
            entry_windows: entry_window_key(&ra),
            ..Default::default()
        };
        assert!(
            !entry_crossed(&scope, 5, &entry_window_key(&rb)),
            "同进程仅标题不同 ⇒ 不算切进来"
        );
    }

    /// ime_activated 不带窗口信息：沿用该 token 最近一次 focus_gained 的窗口上下文。
    #[test]
    fn ime_activated_reuses_the_tokens_last_focus_window() {
        let c = coord_with_rules("ime", AHK);
        c.pid_names
            .lock()
            .unwrap()
            .insert(300, "autohotkey.exe".into());
        c.pid_names
            .lock()
            .unwrap()
            .insert(100, "notepad.exe".into());
        let t = (300u64 << 32) | 1;
        c.handle_focus_gained(&focus(t, "AHK_B"));
        c.handle_focus_gained(&focus((100u64 << 32) | 1, "Notepad"));
        assert_eq!(c.active_compat.lock().unwrap().caret_offset_x, 0);
        c.handle_ime_activated(t);
        assert_eq!(
            c.active_compat.lock().unwrap().caret_offset_x,
            9,
            "回到 t：按它上次的窗口（AHK_B）解析"
        );
        assert!(chinese(&c), "初始模式同样按 AHK_B 的规则");
    }

    /// 没有窗口上下文（从未 focus_gained 的 token、按 pid 推送的后台客户端）⇒ 只按进程名，
    /// 窗口规则一律不命中。
    #[test]
    fn without_window_context_falls_back_to_process_rules_only() {
        let c = coord_with_rules(
            "nowin",
            r#"
[[apps]]
process = "AutoHotkey.exe"
caret_offset_x = 3

[[apps]]
process = "AutoHotkey.exe"
class = "AHK_A"
caret_offset_x = 5
composition_placeholder = "zwsp"
"#,
        );
        c.pid_names
            .lock()
            .unwrap()
            .insert(300, "autohotkey.exe".into());
        let t = (300u64 << 32) | 7;
        c.handle_ime_activated(t);
        assert_eq!(
            c.active_compat.lock().unwrap().caret_offset_x,
            3,
            "进程规则照常生效，窗口规则不命中"
        );
        assert_eq!(
            c.composition_placeholder_for_token(t),
            wind_config::app_compat::PlaceholderChar::Space
        );
        // 同一 pid 的另一个 token 获焦过 A：按 pid 查的路径取它，按 token 查的仍只认自己的窗口。
        let t2 = (300u64 << 32) | 8;
        c.handle_focus_gained(&focus(t2, "AHK_A"));
        assert_eq!(
            c.composition_placeholder_for_token(t),
            wind_config::app_compat::PlaceholderChar::Space,
            "推给 DLL 的按 token：t 自己没获焦过"
        );
        // 焦点应用那一侧（active 槽）：t 没有自己的窗口记录 ⇒ 退回该 pid 最近获焦的窗口。
        c.handle_focus_gained(&focus((100u64 << 32) | 1, "Notepad"));
        c.handle_ime_activated(t);
        assert_eq!(
            c.active_compat.lock().unwrap().caret_offset_x,
            5,
            "只发 ime_activated 的 token 沿用本进程最近获焦的窗口（AHK_A）"
        );
        assert_eq!(
            c.composition_placeholder_for_token(t2),
            wind_config::app_compat::PlaceholderChar::Zwsp
        );
        assert!(
            c.with_pid_compat_rule(300, |r| r.and_then(|r| r.composition_placeholder))
                .is_some(),
            "按 pid：取该 pid 最近获焦的窗口"
        );
    }

    /// `[[commit_newline]]` 不支持窗口条件：焦点落在带类名的窗口上，查的仍是进程那一条。
    #[test]
    fn commit_newline_stays_per_process() {
        let c = coord_with_rules(
            "newline",
            r#"
[[apps]]
class = "OpusApp"
caret_offset_x = 1

[[commit_newline]]
process = "WINWORD.EXE"
style = "cr"
"#,
        );
        c.pid_names
            .lock()
            .unwrap()
            .insert(500, "winword.exe".into());
        c.handle_focus_gained(&focus((500u64 << 32) | 1, "OpusApp"));
        assert_eq!(c.active_compat.lock().unwrap().caret_offset_x, 1);
        assert_eq!(
            c.app_compat
                .lock()
                .unwrap()
                .commit_newline_for(&c.active_process_name()),
            Some(wind_config::app_compat::NewlineStyle::Cr)
        );
    }

    /// 新实例只发 ime_activated、从没 focus_gained：不得把窗口上下文清空——Chromium 的占位、
    /// 窗口规则的初始模式都要沿用本进程最近获焦的窗口（相对按进程名的出厂规则不回退）。
    #[test]
    fn ime_activated_on_a_fresh_token_falls_back_to_the_pids_window() {
        let c = coord_with_rules(
            "fresh",
            r#"
[[apps]]
class = "Chrome_WidgetWin_*"
composition_placeholder = "zwsp"

[[apps]]
process = "msedge.exe"
class = "Chrome_WidgetWin_*"
initial_mode = "english"
"#,
        );
        c.pid_names.lock().unwrap().insert(42, "msedge.exe".into());
        c.pid_names
            .lock()
            .unwrap()
            .insert(100, "notepad.exe".into());
        c.handle_focus_gained(&focus((42u64 << 32) | 1, "Chrome_WidgetWin_1"));
        c.handle_focus_gained(&focus((100u64 << 32) | 1, "Notepad"));
        c.state.lock().unwrap().chinese_mode = true;
        let fresh = (42u64 << 32) | 9;
        c.handle_ime_activated(fresh);
        assert_eq!(
            c.composition_placeholder(),
            "\u{200B}",
            "占位沿用 pid 的窗口"
        );
        assert!(!chinese(&c), "初始模式同样按 pid 的窗口解析");
        assert_eq!(
            c.composition_placeholder_for_token(fresh),
            wind_config::app_compat::PlaceholderChar::Space,
            "推给 DLL 的仍只认 token 自己的窗口"
        );
    }

    fn pfe_msg(enabled: bool) -> Vec<u8> {
        wind_ipc::codec::encode_sync_config(
            wind_ipc::protocol::CONFIG_KEY_PASSWORD_SUPPRESS,
            &wind_ipc::codec::encode_password_suppress_value(enabled),
        )
    }

    fn suppressed(c: &Coordinator) -> bool {
        c.password_suppress
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    const PASSWORD_MASK: u64 = 1 << 31;

    /// 同一 token 换窗口时密码抑制**只降不升**：DLL 那份门控还是按旧窗口算的、新值的重推还在
    /// 路上，服务端当场置位就是 core 抑制而 DLL 照吃（违反 core.suppress ⊆ C++.suppress）。
    /// 置位等 DLL 下一次 input_state_report。首次带类名获焦（握手按空窗口算）同理。
    #[test]
    fn window_change_never_raises_password_suppress_before_the_dll_has_the_new_value() {
        let c = coord_with_rules(
            "pfe",
            r#"
[[apps]]
process = "bank.exe"
password_force_english = false

[[apps]]
process = "bank.exe"
class = "Login"
password_force_english = true
"#,
        );
        c.pid_names.lock().unwrap().insert(700, "bank.exe".into());
        let t = (700u64 << 32) | 1;
        let cap = c.push_server.attach_capture_client(t);
        let login = || FocusData {
            input_scope_mask: PASSWORD_MASK,
            ..focus(t, "Login")
        };

        // 首次带类名获焦：DLL 握手值按空窗口（false），新窗口要 true ⇒ 先不置位，只重推。
        c.handle_focus_gained(&login());
        assert!(!suppressed(&c), "DLL 还没收到 true 之前服务端不得抑制");
        let got: Vec<Vec<u8>> = cap.try_iter().collect();
        assert!(got.contains(&pfe_msg(true)), "新值已重推给 DLL");
        // DLL 收到新值后上报诊断 ⇒ 此时才置位。
        c.handle_input_state_report(700, false, 2, PASSWORD_MASK);
        assert!(suppressed(&c), "DLL 下一次上报时置位");

        // 窗口没变的 focus_gained 照常当场置位（DLL 那份已是同一窗口的值）。
        c.password_suppress
            .store(false, std::sync::atomic::Ordering::Relaxed);
        c.handle_focus_gained(&login());
        assert!(suppressed(&c), "同窗口照常置位");

        // 降的方向照常当场生效：换到规则为 false 的窗口。
        c.handle_focus_gained(&FocusData {
            input_scope_mask: PASSWORD_MASK,
            ..focus(t, "Main")
        });
        assert!(!suppressed(&c), "解除方向不必等 DLL");
    }

    /// PID 复用：校正出进程换了，就清掉上一任记下的窗口，新进程不得按旧窗口匹配窗口规则。
    #[test]
    fn pid_reuse_forgets_the_previous_processes_windows() {
        let c = Coordinator::new_headless(Config::default(), None);
        c.pid_names.lock().unwrap().insert(700, "old.exe".into());
        c.note_focus_window((700u64 << 32) | 1, FocusWindow::of_class("OldWin"));
        c.note_focus_window((701u64 << 32) | 1, FocusWindow::of_class("Other"));
        c.revalidate_pid_name(700, "old.exe");
        assert_eq!(c.focus_window_of_pid(700).class, "OldWin", "名字没变不清");
        c.revalidate_pid_name(700, "new.exe");
        assert!(c.focus_window_of_pid(700).is_empty(), "进程换了 ⇒ 清掉");
        assert_eq!(
            c.focus_window_of_pid(701).class,
            "Other",
            "别的 pid 不受影响"
        );
    }

    /// 规则表重载要按新表重算 `mode_scope.entry_windows`：否则同一个窗口里下一次 focus_gained
    /// 会因「集合变了」被当成切进来，把用户手切的中英态冲掉。
    #[test]
    fn compat_reload_refreshes_the_entry_window_set() {
        let dir = std::env::temp_dir().join(format!("wind_cwm_reload_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(wind_config::app_compat::COMPAT_FILE_NAME);
        std::fs::write(
            &file,
            "[[apps]]\nprocess = \"app.exe\"\ninitial_mode = \"english\"\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.input.default.chinese_mode = true;
        let c = Coordinator::new_headless(cfg, Some(&dir));
        c.pid_names.lock().unwrap().insert(800, "app.exe".into());
        let t = (800u64 << 32) | 1;
        c.handle_focus_gained(&focus(t, "Win"));
        assert!(!chinese(&c), "前置：切进来套 initial_mode");
        c.state.lock().unwrap().chinese_mode = true; // 用户手切中文

        std::fs::write(
            &file,
            "[[apps]]\nprocess = \"app.exe\"\ninitial_mode = \"english\"\n\n\
             [[apps]]\nprocess = \"app.exe\"\nclass = \"Win\"\ninitial_punct = \"english\"\n",
        )
        .unwrap();
        c.reload_app_compat();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            c.mode_scope.lock().unwrap().entry_windows,
            vec![RuleId {
                process: "app.exe".into(),
                class: "win".into(),
                title: String::new(),
            }],
            "重载后按新表重算"
        );
        c.handle_focus_gained(&focus(t, "Win"));
        assert!(chinese(&c), "同一个窗口：不算切进来，手切保留");
    }

    /// 菜单写回只写纯进程键：当前生效值来自窗口规则时提示（P4 前只打 warn，行为不变）。
    #[test]
    fn menu_writeback_is_flagged_when_a_window_rule_supplies_the_value() {
        let c = coord_with_rules(
            "menuwarn",
            r#"
[[apps]]
process = "app.exe"
candidate_position_mode = "follow_caret"

[[apps]]
process = "app.exe"
class = "Pinned"
candidate_position_mode = "fixed"
"#,
        );
        c.pid_names.lock().unwrap().insert(900, "app.exe".into());
        let pick = |r: Option<&wind_config::app_compat::AppCompatRule>| {
            r.and_then(|r| r.candidate_position_mode)
        };
        c.handle_focus_gained(&focus((900u64 << 32) | 1, "Pinned"));
        assert!(c.warn_if_menu_writeback_shadowed("t", pick));
        c.handle_focus_gained(&focus((900u64 << 32) | 1, "Plain"));
        assert!(!c.warn_if_menu_writeback_shadowed("t", pick));
    }
}
