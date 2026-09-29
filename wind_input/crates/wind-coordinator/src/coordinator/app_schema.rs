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
//!
//! # 锁序（本模块的函数被焦点线程、`app-schema-load` 后台线程、设置页 RPC、菜单线程并发调用）
//!
//! 1. [`AppSchemaState::switch_lock`] 是**最外层**：「比对/检查 + 轻量切换」整段持有它。
//!    ⛔ 持有任何别的锁时不得去取它；持有它期间可以取下面任何一把。
//! 2. `Coordinator::schema_toggle_origin` 与 [`AppSchemaState::parked_toggle`] 是**叶子锁**：
//!    两者**永不嵌套**——一边 take 出值、放锁，再取另一边写入。曾经一段先 origin 后 parked、
//!    另一段先 parked 后 origin，两条线程各走一段即死锁。
//! 3. `remembered` → `state_writer` 的内部锁：写记忆表时持 `remembered` 调 `schedule`
//!    （见 [`Coordinator::remember_app_schema`]）；`schedule` 只取它自己的 `pending` 锁，
//!    写盘闭包在 writer 线程上跑、不碰 `remembered`，故不成环。
//! 4. 其余（`global` / `pending_load` / `warned_invalid`）只在单独一句里取、取完即放。

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
    /// 自动切换的互斥：[`Coordinator::apply_app_schema_on_focus`] 的「比较 + 切换」与
    /// [`Coordinator::finish_deferred_app_schema`] 的「检查意图代际 + 切换」都整段持有它。
    ///
    /// 没有它时后者是 check-then-act：后台线程比对代际通过之后、真正切换之前，焦点线程
    /// 可以完成下一次切入（代际 +1、切到新目标），随后后台线程把方案切回**旧**目标。
    /// 锁序：最外层，见模块文档。
    switch_lock: Mutex<()>,
    /// 已报过「固定 id 不在 available」的 `(进程名, id)`：每次切入/手切都会问规则，不去重
    /// 就是同一条 WARN 刷满日志。available 热重载后仍不在的组合也不再重报（配置没变）。
    warned_invalid: Mutex<std::collections::HashSet<(String, String)>>,
}

impl AppSchemaState {
    pub(crate) fn new(global: String, remembered: HashMap<String, String>) -> Self {
        Self {
            global: Mutex::new(global),
            remembered: Mutex::new(remembered),
            intent_gen: AtomicU64::new(0),
            parked_toggle: Mutex::new(None),
            pending_load: Mutex::new(None),
            switch_lock: Mutex::new(()),
            warned_invalid: Mutex::new(std::collections::HashSet::new()),
        }
    }
}

/// 轻量切换时对未上屏编码的处置，见 [`Coordinator::switch_schema_light`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingInput {
    /// 只丢弃，不上屏、不推任何东西给宿主。**焦点切入**用：此刻 active token 已指向新宿主，
    /// 缓冲里若还有旧宿主的编码，按 `commit_on_switch` 上屏就会把 A 应用的码打进 B 应用。
    Discard,
    /// 与手切同一条策略（`keys.commit_on_switch`：开则上屏原码，关则丢弃）。只给**冷方案
    /// 加载完成**那条路用：那时焦点仍在该应用（意图代际未变），缓冲属于它。
    CommitPerPolicy,
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
            let first = self
                .app_schema
                .warned_invalid
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((proc.to_string(), raw.clone()));
            if first {
                warn!(
                    "compat.toml: {proc} 的 schema = \"{raw}\" 不在 schema.available 中，按未配置处理"
                );
            }
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
    ///
    /// 同步切换时缓冲**只丢弃**（[`PendingInput::Discard`]）：焦点切入那一刻 active token 已是
    /// 新宿主，上屏会把旧应用的码打进新应用。菜单改规则、热重载对齐也走这里，那两处缓冲
    /// 本就已被菜单/重载清掉，丢弃与上屏同效。
    pub(crate) fn apply_app_schema_on_focus(&self, proc: &str) {
        // 「比较 + 切换」整段互斥，见 `AppSchemaState::switch_lock`。代际 +1 也放在锁内：
        // 后台线程持锁检查时看到的代际与它随后切换时一致。
        let _switch = self
            .app_schema
            .switch_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let generation = self.app_schema.intent_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let target = self.app_schema_target(proc);
        if target.is_empty() || target == self.engine_mgr.active_schema_id() {
            return;
        }
        if self.engine_mgr.is_loaded(&target) {
            self.switch_schema_light(&target, proc, PendingInput::Discard);
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
    ///
    /// 「检查 + 切换」整段持 `switch_lock`：否则检查通过后、切换之前焦点线程可以完成下一次
    /// 切入，本函数再把方案切回已作废的旧目标。
    pub(crate) fn finish_deferred_app_schema(&self, generation: u64, proc: &str, target: &str) {
        let _switch = self
            .app_schema
            .switch_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.app_schema.intent_gen.load(Ordering::SeqCst) != generation {
            debug!("按应用方案: {target} 加载完成，但焦点已离开 {proc} 或期间手切过，不切换");
            return;
        }
        if self.app_schema_target(proc) != target {
            return;
        }
        self.switch_schema_light(target, proc, PendingInput::CommitPerPolicy);
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
    ///
    /// ⚠ 两段都是「从一把锁 take 出值、放锁，再取另一把写入」，`schema_toggle_origin` 与
    /// `parked_toggle` **永不同时持有**（锁序见模块文档）。
    ///
    /// `pending` 决定未上屏编码怎么处置，见 [`PendingInput`]。
    pub(crate) fn switch_schema_light(&self, target: &str, proc: &str, pending: PendingInput) {
        let from = self.engine_mgr.active_schema_id();
        let generation = self.engine_mgr.schema_generation();
        let still_valid = self
            .schema_toggle_origin
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_if(|r| r.landing.generation == generation);
        if let Some(rec) = still_valid {
            *self
                .app_schema
                .parked_toggle
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some((from.clone(), rec));
        }
        // 编码处置必须早于 switch_schema：上屏记账取的是切换前的方案。
        let commit = match pending {
            PendingInput::CommitPerPolicy => self.take_input_on_schema_switch(),
            PendingInput::Discard => {
                self.discard_input_on_schema_switch();
                SwitchCommit::default()
            }
        };
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
        let resumed = self
            .app_schema
            .parked_toggle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_if(|(id, _)| id == target);
        if let Some((_, mut rec)) = resumed {
            rec.landing.generation = self.engine_mgr.schema_generation();
            *self
                .schema_toggle_origin
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(rec);
        }
        info!("按应用方案: {proc} {from} -> {target}");
    }

    /// 丢弃未上屏的编码与一切独占模式缓冲，不上屏、不记上屏统计、不推任何东西给宿主。
    /// 与失焦清输入同一组动作（`reset_exclusive_modes` 已含主缓冲、候选、preedit、逐步
    /// 转换前缀）。旧宿主里残留的组合由它自己的 DLL 在失焦时结束，不归这里管。
    fn discard_input_on_schema_switch(&self) {
        self.terminate_auto_phrase("switch_schema");
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.exit_assoc(&mut state, crate::handle_assoc::AssocExit::ModeSwitch);
        self.reset_exclusive_modes(&mut state);
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
    ///
    /// ★ **持 `remembered` 锁调 `schedule`**：快照与登记必须是同一个原子步骤。放锁后再
    /// 登记的话，两条线程并发时后取的（更新的）快照可能先登记、先取的（更旧的）后登记，
    /// 而 `schedule` 是同种覆盖语义 ⇒ 落盘的是旧表。持锁保证「登记顺序 = 快照顺序」。
    /// 不成环：`schedule` 只取 writer 自己的 `pending` 锁，写盘闭包在 writer 线程上跑。
    pub(crate) fn remember_app_schema(&self, proc: &str, schema_id: &str) {
        let mut m = self
            .app_schema
            .remembered
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        m.insert(proc.to_string(), schema_id.to_string());
        let snapshot = m.clone();
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
        fixture_with(tag, |_| {})
    }

    /// 同 [`fixture`]，另可改配置。
    fn fixture_with(tag: &str, tweak: impl FnOnce(&mut Config)) -> (Arc<Coordinator>, PathBuf) {
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
        tweak(&mut cfg);
        let c = Coordinator::new_headless_with_override(
            cfg,
            Some(&dir.join("data")),
            Some(dir.join("override")),
        );
        let rules = vec![wind_config::app_compat::AppCompatRule {
            process: "code.exe".into(),
            schema: Some("zb".into()),
            ..Default::default()
        }];
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

    /// 把 zb 暖好（冷加载一次）再回到 za：之后切入 code.exe 走同步轻量切换。
    fn warm_zb_then_back_to_za(c: &Arc<Coordinator>) {
        c.apply_app_schema_on_focus("code.exe");
        c.debug_wait_app_schema_load();
        c.apply_app_schema_on_focus("plain.exe");
        assert_eq!(c.engine_mgr.active_schema_id(), "za", "前置：回到全局 za");
        assert!(c.engine_mgr.is_loaded("zb"), "前置：zb 已暖");
    }

    /// 等 `cond` 成立，最多 2s。**只用来给旧实现的反例留出显现的时间**：各测试在新实现下的
    /// 通过与否不取决于它等了多久（见各用例的说明），故不是靠时序碰运气的测试。
    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if cond() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// ★ 锁序：`schema_toggle_origin` 与 `parked_toggle` 永不同时持有。
    ///
    /// 复现旧实现的死锁形状：一条线程持着 `parked_toggle`（旧实现后段的持锁形态），另一条
    /// 线程跑 `switch_schema_light`。旧实现前段 take 走记录后**仍持着 origin** 去等 parked，
    /// 于是 origin 在主线程放掉 parked 之前永远取不到——主线程若此时去取 origin 就是死锁；
    /// 这里用 `try_lock` 观察而不真的去等。新实现 take 完即放锁：只要后台线程被调度到，
    /// 主线程必然看到「origin 可取且记录已被取走」，与等待时长无关。
    #[test]
    fn toggle_record_locks_are_never_held_together() {
        let (c, dir) = fixture("locks");
        warm_zb_then_back_to_za(&c);
        *c.schema_toggle_origin.lock().unwrap() = Some(SchemaToggleOrigin {
            origin: "zb".into(),
            landing: ToggleLanding {
                generation: c.engine_mgr.schema_generation(),
                chinese_mode: true,
                caps_lock: false,
            },
            trigger_vk: 0,
        });
        let parked = c.app_schema.parked_toggle.lock().unwrap();
        let c2 = c.clone();
        let h = std::thread::spawn(move || {
            c2.switch_schema_light("zb", "code.exe", PendingInput::Discard)
        });
        let released = wait_until(|| match c.schema_toggle_origin.try_lock() {
            Ok(g) => g.is_none(),
            Err(std::sync::TryLockError::WouldBlock) => false,
            Err(std::sync::TryLockError::Poisoned(e)) => panic!("origin 锁中毒: {e}"),
        });
        drop(parked);
        h.join().unwrap();
        assert!(
            released,
            "switch_schema_light 持着 schema_toggle_origin 去等 parked_toggle——与另一段的反向锁序构成死锁"
        );
        assert_eq!(c.engine_mgr.active_schema_id(), "zb");
        assert!(
            c.app_schema
                .parked_toggle
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|(id, _)| id == "za"),
            "仍有效的去程记录被挂起到来源方案 za 名下"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 冷方案延后切换的「检查 + 切换」与焦点切入互斥。
    ///
    /// 主线程持 `switch_lock` 扮演「焦点线程正处在一次切入的比较 + 切换中」，后台线程带着
    /// 此刻仍有效的代际来做延后切换；随后焦点线程完成这次切入（代际 +1，目标仍是 za）。
    /// 旧实现不等锁：代际检查早已通过、直接切到 zb——把方案切回一个已作废的目标。
    /// 新实现被锁挡住，拿到锁时代际已变，放弃。无论后台线程何时被调度，新实现的结论都
    /// 相同（它只可能在主线程放锁之后做检查）。
    #[test]
    fn deferred_switch_cannot_interleave_with_a_focus_switch() {
        let (c, dir) = fixture("race");
        warm_zb_then_back_to_za(&c);
        let stale = c.app_schema.intent_gen.load(Ordering::SeqCst);
        let focus_in_progress = c.app_schema.switch_lock.lock().unwrap();
        let c2 = c.clone();
        let h = std::thread::spawn(move || c2.finish_deferred_app_schema(stale, "code.exe", "zb"));
        // 旧实现下后台线程不等锁，这段时间里就跑完了；新实现下它一直卡在锁上。
        let _ = wait_until(|| h.is_finished());
        // 焦点线程完成这次切入：代际 +1（目标 za = 当前，无需切）。
        c.app_schema.intent_gen.fetch_add(1, Ordering::SeqCst);
        drop(focus_in_progress);
        h.join().unwrap();
        assert_eq!(
            c.engine_mgr.active_schema_id(),
            "za",
            "延后切换的检查与切换之间插进了一次焦点切入，不得再切回旧目标"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 焦点切入触发的轻量切换**只丢弃**缓冲：那一刻 active token 已是新宿主，按
    /// `commit_on_switch` 上屏会把旧应用的码打进新应用。上屏语义只留给冷方案加载完成那条路。
    #[test]
    fn focus_switch_discards_pending_input_but_deferred_switch_commits() {
        let (c, dir) = fixture_with("discard", |cfg| cfg.keys.commit_on_switch = true);
        warm_zb_then_back_to_za(&c);
        let cap = c.push_server.attach_capture_client((42u64 << 32) | 1);
        let commit_a = wind_ipc::codec::encode_commit_text("a", None, false, true, false);
        let clear = wind_ipc::codec::encode_clear_composition();

        c.state.lock().unwrap().input_buffer = "a".into();
        c.apply_app_schema_on_focus("code.exe");
        assert_eq!(c.engine_mgr.active_schema_id(), "zb");
        let got: Vec<Vec<u8>> = cap.try_iter().collect();
        assert!(
            !got.contains(&commit_a),
            "焦点切入不得把旧缓冲按 commit_on_switch 上屏到新宿主"
        );
        assert!(!got.contains(&clear), "也不该给新宿主推清组合");
        assert!(
            c.state.lock().unwrap().input_buffer.is_empty(),
            "缓冲必须丢弃"
        );

        // 对照：冷方案加载完成（焦点仍在该应用）那条路照 commit_on_switch 上屏——
        // 否则上面几条断言在「一律不上屏」的实现下也会绿。
        c.apply_app_schema_on_focus("plain.exe");
        c.state.lock().unwrap().input_buffer = "a".into();
        let _ = cap.try_iter().count();
        let current = c.app_schema.intent_gen.load(Ordering::SeqCst);
        c.finish_deferred_app_schema(current, "code.exe", "zb");
        assert_eq!(c.engine_mgr.active_schema_id(), "zb");
        let got: Vec<Vec<u8>> = cap.try_iter().collect();
        assert!(
            got.contains(&commit_a),
            "冷方案加载完成时缓冲属于该应用，按策略上屏"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 固定 id 不在 available：同一 `(进程名, id)` 只 WARN 一次（每次切入/手切都会问规则）。
    #[test]
    fn invalid_fixed_id_is_reported_once_per_process_and_id() {
        let (c, dir) = fixture("warn");
        let rules = ["code.exe", "vim.exe"]
            .into_iter()
            .map(|p| wind_config::app_compat::AppCompatRule {
                process: p.into(),
                schema: Some("gone".into()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        *c.app_compat.lock().unwrap() = wind_config::app_compat::AppCompat::from_rules(rules);
        for _ in 0..3 {
            assert_eq!(
                c.app_schema_rule("code.exe"),
                None,
                "不在 available ⇒ 未配置"
            );
        }
        let reported = || c.app_schema.warned_invalid.lock().unwrap().len();
        assert_eq!(reported(), 1, "同一 (进程, id) 只报一次");
        assert_eq!(c.app_schema_rule("vim.exe"), None);
        assert_eq!(reported(), 2, "换一个进程要另报");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
