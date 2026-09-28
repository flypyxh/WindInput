//! 按应用方案（compat.toml `[[apps]] schema`，C0-7 / C3-3，GH#80）。
//!
//! 设计见 `docs/design/per-app-overrides.md`「① ② 按应用方案」。三件事：
//!
//! 1. **切入**：焦点跨进程切入时（与 initial_mode 同一个 `crossed` 判据）按规则算目标方案，
//!    与当前不同就**轻量切换**——只换引擎与方案绑定的运行期资源，不写 `schema.active`、
//!    不归位中文、不弹状态气泡。见 [`Coordinator::apply_app_schema_on_focus`]。
//! 2. **手切分流**：所有手切入口汇到 `finish_user_schema_switch`，在那里问
//!    [`Coordinator::route_manual_schema_switch`]：规则应用内只改本应用的方案（`@remember`
//!    另记记忆表），不写盘；无规则应用照旧改全局。
//! 3. **全局方案**的来源是 [`AppSchemaState::global`]，**不是** `rt().config.schema.active`：
//!    手切写的是磁盘上的 config.toml，内存里的 config 不刷新（没有文件监视，只在
//!    `reload_user_config` 时整份重读），读它拿到的是启动时的旧值。
//!
//! ⛔ 本模块的任何函数都不得进 `get_current_mode`（DLL 同步阻塞路径）：切方案可能要构建
//! 引擎，冷方案还要起后台线程。

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use wind_config::app_compat::AppSchema;

/// 按应用方案的运行时态。独立成结构体：五个量只服务这一个功能，散在 `Coordinator` 的
/// 百来个字段里既难找也难看出它们之间的约束。
pub(crate) struct AppSchemaState {
    /// 「全局方案」：最近一次**全局手切**（无规则应用里手切）的结果，启动取引擎的活跃方案，
    /// `reload_user_config` 重建方案集后取引擎新的活跃方案（即磁盘上的 `schema.active`）。
    ///
    /// ⚠ 不能读 `rt().config.schema.active`，理由见模块文档第 3 条。
    global: Mutex<String>,
    /// `@remember` 应用的记忆表（进程名小写 → 方案 id），`state.toml` `app_schemas` 的内存镜像。
    remembered: Mutex<HashMap<String, String>>,
    /// 切方案意图的代际：每次焦点切入重算、每次手切都 +1。后台冷加载完成时比对它——
    /// 不等 ⇒ 期间焦点已离开或用户已手切，这次延后切换作废。
    ///
    /// 用代际而不是比对 pid / 进程名：「切走又切回同一个应用」在 pid 上与「从未离开」同形，
    /// 而那期间用户可能已经在别处手切过全局方案。
    intent_gen: AtomicU64,
    /// 被自动切换挂起的往返热键去程记录：`(送达的方案 id, 记录)`。见
    /// [`Coordinator::switch_schema_light`] 里「往返记录」一段。
    parked_toggle: Mutex<Option<(String, SchemaToggleOrigin)>>,
    /// 最近一次冷方案后台加载的线程句柄，只供测试等它完成（生产不 join）。
    pending_load: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AppSchemaState {
    pub(crate) fn new(global: String, remembered: HashMap<String, String>) -> Self {
        Self {
            global: Mutex::new(global),
            remembered: Mutex::new(remembered),
            intent_gen: AtomicU64::new(0),
            parked_toggle: Mutex::new(None),
            pending_load: Mutex::new(None),
        }
    }
}

/// 经 available 校验后的按应用方案规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppSchemaRule {
    Fixed(String),
    Remember,
}

impl Coordinator {
    /// 该进程**生效中**的方案规则：未配、或固定的 id 不在当时的 `schema.available` 内
    /// ⇒ `None`（视为未配置并 WARN）。
    ///
    /// available 校验放在这里而不是解析层：available 可热重载，按当时的值判才能随之生效；
    /// 且启动预热只覆盖 available，放行任意 id 等于允许焦点路径上同步冷构建。
    pub(crate) fn app_schema_rule(&self, proc: &str) -> Option<AppSchemaRule> {
        if proc.is_empty() {
            return None;
        }
        let raw = {
            let table = self.app_compat.lock().unwrap_or_else(|e| e.into_inner());
            match table.get_rule(proc)?.app_schema()? {
                AppSchema::Remember => return Some(AppSchemaRule::Remember),
                AppSchema::Fixed(id) => id.to_string(),
            }
        };
        if self.engine_mgr.available_schemas().contains(&raw) {
            Some(AppSchemaRule::Fixed(raw))
        } else {
            warn!(
                "compat.toml: {proc} 的 schema = \"{raw}\" 不在 schema.available 中，按未配置处理"
            );
            None
        }
    }

    /// 当前的全局方案，见 [`AppSchemaState::global`]。
    pub(crate) fn global_schema(&self) -> String {
        self.app_schema
            .global
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 记忆表里该进程的方案；记录的 id 已不在 available ⇒ 当没记过（不清理：方案可能被重新启用）。
    fn remembered_schema(&self, proc: &str) -> Option<String> {
        let id = self
            .app_schema
            .remembered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(proc)
            .cloned()?;
        self.engine_mgr
            .available_schemas()
            .contains(&id)
            .then_some(id)
    }

    /// 焦点在该进程时应当使用的方案：固定 → 规则值；`@remember` → 记忆表，无记录用全局；
    /// 无规则 → 全局。
    pub(crate) fn app_schema_target(&self, proc: &str) -> String {
        match self.app_schema_rule(proc) {
            Some(AppSchemaRule::Fixed(id)) => id,
            Some(AppSchemaRule::Remember) => self
                .remembered_schema(proc)
                .unwrap_or_else(|| self.global_schema()),
            None => self.global_schema(),
        }
    }

    /// 焦点跨进程切入（或菜单改了规则、热重载重建了方案集）时，把活跃方案对齐到该进程的
    /// 目标方案。调用方负责「何时该算」（`crossed` 判据在 `handle_focus_gained`）。
    ///
    /// 目标方案没加载好（开机预热未完）⇒ **不阻塞**：保持当前方案，后台加载，完成时若
    /// 意图代际未变（焦点没离开、用户没手切）再切。焦点路径上同步构建一个大词库会卡住宿主。
    pub(crate) fn apply_app_schema_on_focus(&self, proc: &str) {
        let generation = self.app_schema.intent_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let target = self.app_schema_target(proc);
        if target.is_empty() || target == self.engine_mgr.active_schema_id() {
            return;
        }
        if self.engine_mgr.is_loaded(&target) {
            self.switch_schema_light(&target, proc);
            return;
        }
        let Some(weak) = self.self_weak.get().cloned() else {
            return;
        };
        debug!("按应用方案: {proc} 的目标 {target} 尚未加载，保持当前方案、后台加载");
        let proc = proc.to_string();
        let spawned = std::thread::Builder::new()
            .name("app-schema-load".into())
            .spawn(move || {
                let Some(c) = weak.upgrade() else {
                    return;
                };
                if !c.engine_mgr.ensure_schema(&target) {
                    warn!("按应用方案: {target} 加载失败，{proc} 保持当前方案");
                    return;
                }
                c.finish_deferred_app_schema(generation, &proc, &target);
            });
        match spawned {
            Ok(h) => {
                *self
                    .app_schema
                    .pending_load
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(h);
            }
            Err(e) => warn!("按应用方案: 起后台加载线程失败: {e}"),
        }
    }

    /// 冷方案加载完成后的收尾：意图代际未变才切，且按**此刻**的规则重算目标——加载期间
    /// 菜单可能改过规则，拿旧目标会切到一个已被撤销的方案。
    pub(crate) fn finish_deferred_app_schema(&self, generation: u64, proc: &str, target: &str) {
        if self.app_schema.intent_gen.load(Ordering::SeqCst) != generation {
            debug!("按应用方案: {target} 加载完成，但焦点已离开 {proc} 或期间手切过，不切换");
            return;
        }
        if self.app_schema_target(proc) != target {
            return;
        }
        self.switch_schema_light(target, proc);
    }

    /// **轻量切换**：换引擎 + 方案绑定的运行期同步，是 `finish_user_schema_switch` 去掉
    /// 「写 `schema.active` / 归位中文与取消大写 / 状态气泡」之后的部分。
    ///
    /// 不归位中文：中英态归 initial_mode 那套管，两者正交；自动切换强行改中英会与用户在
    /// 该应用里配的 `initial_mode = "english"` 打架。不弹气泡：焦点气泡另有
    /// `ui.status.show_on_focus` 管，再弹一次就是每次切应用都闪。
    ///
    /// # 往返记录（`toggle_schema:<id>`）
    ///
    /// 引擎切方案会让 `schema_generation` +1，往返键的去程记录随之失效。对手切这是想要的
    /// （期间用别的方式切过方案 ⇒ 来源作废），对自动切换则是误伤：在 A 应用按往返键去了
    /// 英文、切到一个固定五笔的应用再切回 A，A 会被自动切回英文，但再按往返键却没反应。
    /// 故自动切走时把仍有效的记录**挂起**，之后某次自动切换又落回同一个方案时按新代际
    /// 恢复；任何手切都丢弃挂起的记录（见 [`Self::route_manual_schema_switch`]）。
    fn switch_schema_light(&self, target: &str, proc: &str) {
        let from = self.engine_mgr.active_schema_id();
        {
            let generation = self.engine_mgr.schema_generation();
            let mut origin = self
                .schema_toggle_origin
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if origin
                .as_ref()
                .is_some_and(|r| r.landing.generation == generation)
                && let Some(rec) = origin.take()
            {
                *self
                    .app_schema
                    .parked_toggle
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some((from.clone(), rec));
            }
        }
        // 与手切同一条编码处置（上屏记账取的是切换前的方案，故必须早于 switch_schema）。
        // 焦点切入时缓冲通常已空，这里主要照顾「冷方案加载完成时用户正在打字」的那一刻。
        let commit = self.take_input_on_schema_switch();
        if !self.engine_mgr.switch_schema(target) {
            self.push_switch_commit(&commit);
            return;
        }
        self.sync_chaizi_assets();
        self.invalidate_aux_code_table();
        self.notify_ui_hide();
        self.push_state_update();
        self.notify_toolbar();
        self.push_switch_commit(&commit);
        {
            let mut parked = self
                .app_schema
                .parked_toggle
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if parked.as_ref().is_some_and(|(id, _)| id == target)
                && let Some((_, mut rec)) = parked.take()
            {
                rec.landing.generation = self.engine_mgr.schema_generation();
                *self
                    .schema_toggle_origin
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(rec);
            }
        }
        info!("按应用方案: {proc} {from} -> {target}");
    }

    /// 手切收尾的分流（`finish_user_schema_switch` 调）：返回 `true` = 当前焦点应用有方案
    /// 规则，本次手切只属于该应用，调用方**不得**写 `schema.active`；`false` = 全局手切，
    /// 全局方案随之更新，调用方照旧持久化。
    ///
    /// 固定应用的手切是临时覆盖：不记任何东西，离开再回来由规则重算、自然恢复规则值。
    pub(crate) fn route_manual_schema_switch(&self, schema_id: &str) -> bool {
        // 手切就是新的意图：作废在途的冷加载（否则加载完会把用户刚切的方案切掉），
        // 丢弃挂起的往返记录（它描述的是手切之前的往返）。
        self.app_schema.intent_gen.fetch_add(1, Ordering::SeqCst);
        self.app_schema
            .parked_toggle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let proc = self.active_process_name();
        match self.app_schema_rule(&proc) {
            Some(AppSchemaRule::Fixed(_)) => {
                info!("按应用方案: {proc} 内手切到 {schema_id}（临时，不写 schema.active）");
                true
            }
            Some(AppSchemaRule::Remember) => {
                self.remember_app_schema(&proc, schema_id);
                info!("按应用方案: {proc} 内手切到 {schema_id}，记入记忆表");
                true
            }
            None => {
                *self
                    .app_schema
                    .global
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = schema_id.to_string();
                false
            }
        }
    }

    /// 写记忆表并经 `state_writer` 落盘。闭包带**整张表**（覆盖语义，见 `state_writer` 模块文档）。
    pub(crate) fn remember_app_schema(&self, proc: &str, schema_id: &str) {
        let snapshot = {
            let mut m = self
                .app_schema
                .remembered
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            m.insert(proc.to_string(), schema_id.to_string());
            m.clone()
        };
        self.state_writer.schedule("app_schemas", move |rs| {
            rs.app_schemas = snapshot.clone();
        });
    }

    /// 该进程在记忆表里有没有**有效**记录（菜单设 `@remember` 时用来决定要不要播种）。
    pub(crate) fn has_remembered_schema(&self, proc: &str) -> bool {
        self.remembered_schema(proc).is_some()
    }

    /// `reload_user_config` 重建方案集之后调：引擎的活跃方案此刻已被重置为磁盘上的
    /// `schema.active`，即最新的全局方案——据此更新 [`AppSchemaState::global`]，再把当前
    /// 焦点应用对齐回它的目标方案。
    ///
    /// 不做后一步的话，焦点在固定五笔的应用里时，设置页随便保存一项方案段的设置，
    /// 活跃方案就被冲回全局，直到切走再切回来。
    pub(crate) fn resync_app_schema_after_reload(&self) {
        *self
            .app_schema
            .global
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.engine_mgr.active_schema_id();
        let proc = self.active_process_name();
        self.apply_app_schema_on_focus(&proc);
    }

    /// 设置页「设为当前」（`schema.setActive` RPC）直接切了引擎：那是全局意图，全局方案跟随。
    /// 不跟随的话，下一次焦点切入无规则应用会把它切回旧的全局方案。
    pub(crate) fn note_global_schema_set(&self, schema_id: &str) {
        self.app_schema.intent_gen.fetch_add(1, Ordering::SeqCst);
        *self
            .app_schema
            .global
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = schema_id.to_string();
    }

    /// 测试用：等最近一次冷方案后台加载（及其延后切换）结束。
    #[doc(hidden)]
    pub fn debug_wait_app_schema_load(&self) {
        let h = self
            .app_schema
            .pending_load
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(h) = h {
            let _ = h.join();
        }
    }

    /// 测试用：当前的全局方案与记忆表快照。
    #[doc(hidden)]
    pub fn debug_app_schema_state(&self) -> (String, HashMap<String, String>) {
        (
            self.global_schema(),
            self.app_schema
                .remembered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 两个自造码表方案 za / zb 的数据目录 + 空 override 目录（不碰真实用户目录）。
    fn fixture(tag: &str) -> (Arc<Coordinator>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("wind_app_schema_unit_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schemas = dir.join("data/schemas");
        for id in ["za", "zb"] {
            std::fs::create_dir_all(schemas.join(id)).unwrap();
            std::fs::write(
                schemas.join(format!("{id}.schema.toml")),
                format!(
                    "[schema]\nid = \"{id}\"\nname = \"{id}\"\n\
                     [engine]\ntype = \"codetable\"\n\
                     [engine.codetable]\nmax_code_length = 4\n\
                     [[dictionaries]]\nid = \"main\"\npath = \"{id}/{id}.dict.yaml\"\ndefault = true\n"
                ),
            )
            .unwrap();
            std::fs::write(
                schemas.join(format!("{id}/{id}.dict.yaml")),
                format!("---\nname: {id}\nversion: \"1\"\n...\n甲\ta\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.join("override")).unwrap();
        let mut cfg = Config::default();
        cfg.schema.available = vec!["za".into(), "zb".into()];
        cfg.schema.active = "za".into();
        let c = Coordinator::new_headless_with_override(
            cfg,
            Some(&dir.join("data")),
            Some(dir.join("override")),
        );
        let mut rules = Vec::new();
        wind_config::app_compat::set_schema(&mut rules, "code.exe", Some("zb".into()));
        *c.app_compat.lock().unwrap() = wind_config::app_compat::AppCompat::from_rules(rules);
        (c, dir)
    }

    /// 冷方案不在焦点路径上同步构建：切入时起后台加载，完成后才切。
    #[test]
    fn cold_target_loads_in_background() {
        let (c, dir) = fixture("cold");
        assert!(
            !c.engine_mgr.is_loaded("zb"),
            "前置条件：headless 不预热，zb 是冷的"
        );
        c.apply_app_schema_on_focus("code.exe");
        assert!(
            c.app_schema.pending_load.lock().unwrap().is_some(),
            "冷方案必须交给后台线程，而不是在焦点路径上同步构建"
        );
        c.debug_wait_app_schema_load();
        assert_eq!(
            c.engine_mgr.active_schema_id(),
            "zb",
            "加载完成且焦点未离开 ⇒ 切过去"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 加载完成时焦点已离开（或期间手切过）⇒ 不切；意图代际未变才切。
    #[test]
    fn deferred_switch_is_dropped_when_intent_moved_on() {
        let (c, dir) = fixture("stale");
        c.apply_app_schema_on_focus("code.exe");
        c.debug_wait_app_schema_load();
        // 回到起点：焦点落到无规则应用（目标 = 全局 za）。
        c.apply_app_schema_on_focus("plain.exe");
        assert_eq!(c.engine_mgr.active_schema_id(), "za");

        // 模拟「切入 code.exe 时 zb 还冷、加载期间焦点又切走了」。
        let stale = c.app_schema.intent_gen.load(Ordering::SeqCst);
        c.apply_app_schema_on_focus("plain.exe");
        c.finish_deferred_app_schema(stale, "code.exe", "zb");
        assert_eq!(
            c.engine_mgr.active_schema_id(),
            "za",
            "焦点已离开，延后切换必须作废"
        );

        // 对照：代际未变 ⇒ 照切（否则上一条断言在「永远不切」的实现下也会绿）。
        let current = c.app_schema.intent_gen.load(Ordering::SeqCst);
        c.finish_deferred_app_schema(current, "code.exe", "zb");
        assert_eq!(c.engine_mgr.active_schema_id(), "zb");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
