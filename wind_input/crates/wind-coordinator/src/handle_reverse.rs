//! 反查模式（`ModeKind::Reverse`，docs/design/codetable-reverse-mode.md §3）。
//!
//! 同生僻字模式先例：缓冲 / 光标 / 退格 / 选词 / 退出复用 special 族
//! （`special_buffer` + `handle_special_key` + `exit_special_mode`），本文件只管
//! 「进入」与「候选怎么来」。候选走 `EngineManager::convert_reverse`（与行内通配同一个
//! `wildcard_query` 内核、不看 `wildcard` 主开关、只查主码表），再过常用字判定与检索范围
//! 过滤；**不**自动上屏、**不**做词频重排、**不**吃候选调整（计划裁决 12：通配串不是任何
//! 码位，读侧本就查不中）。

use crate::coordinator::{Coordinator, State};
use crate::pipeline::ModeKind;
use tracing::debug;
use wind_bridge::handler::KeyAction;
use wind_dict::WILDCARD_SLOT;
use wind_keys::keymap;

/// 反查缓冲 → 查询模式串：缓冲里**每一个**通配键（含首位）都换成槽位字符。
///
/// 与行内通配（`wildcard_pattern`）的差别正是「含首位、不看让位规则」——想在首位通配
/// 就进反查模式（spec §3.3）。缓冲里没有通配键时原样返回，引擎按字面等长 + 前缀查。
pub(crate) fn reverse_pattern(buffer: &str, key: char) -> String {
    buffer
        .chars()
        .map(|c| if c == key { WILDCARD_SLOT } else { c })
        .collect()
}

impl Coordinator {
    /// 反查模式此刻能不能进：总开关 `input.reverse.enabled` 开，且活跃方案有反查通配键
    /// （码表 / 混输的主码表才有；拼音等引擎给 `None`）。
    ///
    /// 门卫没过时调用方返回 `None` 让触发键落普通输入——绝不吞键（同 `enter_bound_action`）。
    pub(crate) fn reverse_mode_available(&self) -> bool {
        self.rt().config.input.reverse.enabled && self.engine_mgr.active_reverse_key().is_some()
    }

    /// 这个键是否绑了反查动词（方案 `[key_actions]` → 全局 → `z_key_action` 三层链）。
    #[allow(dead_code)] // Task 19（首键冲突体检 / z 夺取通路）接线
    pub(crate) fn is_reverse_trigger(&self, key_code: u32) -> bool {
        matches!(
            self.bound_action_for(key_code),
            Some(wind_config::BoundAction::Reverse)
        )
    }

    /// 进入反查模式。字段组与 `enter_rare_char_mode` 同一组（本模式也没有宿主方案，
    /// `special_id` / `overlay_spec` 恒为默认值 ⇒ 布局与注释模板跟随全局或本模式配置）。
    pub(crate) fn enter_reverse_mode(&self, state: &mut State, key_code: u32) -> KeyAction {
        state.input_buffer.clear();
        state.candidates.clear();
        state.active = Some(ModeKind::Reverse);
        state.special_id = 0;
        state.overlay_spec = None;
        state.special_buffer.clear();
        state.special_cursor = 0;
        state.special_prefix = keymap::vk_to_prefix_char_with_letters(key_code)
            .map(|c| c.to_string())
            .unwrap_or_default();
        state.scope_relaxed = false;
        self.update_reverse_candidates(state);
        self.notify_ui_update(state);
        let display = state.preedit.clone();
        debug!("Entered reverse mode");
        self.reverse_entry_composition(key_code, display)
    }

    /// 顶掉当前半成品并进入反查模式（同 `commit_and_enter_rare_char_mode`）。
    pub(crate) fn commit_and_enter_reverse_mode(
        &self,
        state: &mut State,
        key_code: u32,
    ) -> KeyAction {
        if let Some(act) = self.top_commit_command_guard(state) {
            return act;
        }
        let committed = self.take_committed_with_highlight(state);
        let enter = self.enter_reverse_mode(state, key_code);
        match committed {
            Some(text) => {
                let new_comp = match &enter {
                    KeyAction::UpdateComposition { text, .. } => text.clone(),
                    _ => state.preedit.clone(),
                };
                self.commit_then_new_composition(text, new_comp)
            }
            None => enter,
        }
    }

    /// 缓冲变了之后的候选刷新：视图复位 + 首批（`WILDCARD_INITIAL_LIMIT`，同行内通配 §11）。
    pub(crate) fn update_reverse_candidates(&self, state: &mut State) {
        self.reset_candidate_view(state);
        self.build_reverse_candidates(state, crate::wildcard::WILDCARD_INITIAL_LIMIT);
    }

    /// 按 `limit` 重建反查候选，**不动页码 / 高亮**（翻页扩充复用）；返回引擎条数。
    ///
    /// `has_more` 看引擎条数而非可见条数（spec codetable-wildcard §11 契约 3）：新增的一批
    /// 被检索范围整批滤掉时，可见条数不变而引擎后面仍可能有常用字。
    pub(crate) fn build_reverse_candidates(&self, state: &mut State, limit: usize) -> usize {
        state.candidates.clear();
        state.preedit = format!("{}{}", state.special_prefix, state.special_buffer);
        state.candidate_limit = limit;
        let key = match self.engine_mgr.active_reverse_key() {
            Some(k) if !state.special_buffer.is_empty() => k,
            _ => {
                state.has_more = false;
                return 0;
            }
        };
        let buffer = state.special_buffer.clone();
        let pattern = reverse_pattern(&buffer, key);
        let result = self
            .engine_mgr
            .convert_reverse(&buffer, &pattern, limit)
            .unwrap_or_default();
        let engine_count = result.candidates.len();
        // 词条内特殊语法（`$SS` / `$CC` / `{..}`）统一展开，同主路通配（`build_candidates`）。
        let mut cands = self.finalize_candidates(result.candidates, &buffer);
        self.mark_common(&mut cands);
        // 通配结果带 `is_wildcard`，§11 的整组常用字过滤在 `filter_candidates` 里自动成立。
        self.apply_filter(state, &mut cands);
        state.candidates = cands;
        state.has_more = engine_count >= limit;
        engine_count
    }
}

#[cfg(test)]
mod tests {
    use super::reverse_pattern;
    use crate::coordinator::Coordinator;
    use std::path::PathBuf;
    use std::sync::Arc;
    use wind_bridge::handler::{KeyEventData, MessageHandler};
    use wind_config::Config;
    use wind_ipc::protocol::{EVENT_KEY_DOWN, MOD_SHIFT};

    const VK_BACKSLASH: u32 = 0xDC;
    const VK_ESCAPE: u32 = 0x1B;
    const VK_SLASH: u32 = 0xBF;

    struct Cleanup {
        id: String,
        dir: PathBuf,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(cache) = Config::cache_dir() {
                let _ = std::fs::remove_dir_all(cache.join(&self.id));
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 码表 `ab 甲 / ac 乙 / bb 丁`；`\` 绑反查、模式开。
    fn coord(tag: &str, tweak: impl FnOnce(&mut Config)) -> (Arc<Coordinator>, Cleanup) {
        let id = format!("zz_rev_{tag}_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("wind_rev_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("schemas");
        std::fs::create_dir_all(s.join(&id)).unwrap();
        std::fs::write(
            s.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"反\"\n[engine]\ntype = \"codetable\"\n\
                 [engine.codetable]\nmax_code_length = 4\n\
                 [[dictionaries]]\nid = \"{id}_m\"\npath = \"{id}/m.dict.yaml\"\ntype = \"rime_codetable\"\ndefault = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            s.join(format!("{id}/m.dict.yaml")),
            "---\nname: m\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\nab\t甲\t30\nac\t乙\t20\nbb\t丁\t10\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.available = vec![id.clone()];
        cfg.schema.active = id.clone();
        cfg.input.default.chinese_mode = true;
        cfg.input.reverse.enabled = true;
        cfg.keys
            .key_actions
            .insert("backslash".into(), "reverse".into());
        tweak(&mut cfg);
        (
            Coordinator::new_headless(cfg, Some(&dir)),
            Cleanup { id, dir },
        )
    }

    fn key(c: &Coordinator, vk: u32, shift: bool) {
        c.handle_key_event(&KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: if shift { MOD_SHIFT } else { 0 },
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        });
    }

    fn letters(c: &Coordinator, s: &str) {
        for ch in s.chars() {
            key(c, ch.to_ascii_uppercase() as u32, false);
        }
    }

    #[test]
    fn reverse_pattern_marks_every_key_position() {
        let s = wind_dict::WILDCARD_SLOT;
        assert_eq!(reverse_pattern("zbz", 'z'), format!("{s}b{s}"), "含首位");
        assert_eq!(
            reverse_pattern("ab", 'z'),
            "ab",
            "无通配键 ⇒ 字面等长 + 前缀"
        );
    }

    /// ★ Review Focus 5：进 → 打码出候选 → Esc，模式态与翻页 / 放宽位全部复位，主路径随后照常。
    #[test]
    fn enter_and_exit_reverse_mode_clears_state() {
        let (c, _g) = coord("exit", |_| {});
        key(&c, VK_BACKSLASH, false);
        assert_eq!(c.debug_active_mode(), Some("reverse"));
        letters(&c, "zb");
        let tri = c.debug_candidate_triples();
        assert_eq!(
            tri.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>(),
            ["甲", "丁"],
            "首位通配：?b"
        );
        assert!(
            tri.iter().all(|(_, code, comment)| code == comment),
            "注释是全码"
        );
        key(&c, VK_ESCAPE, false);
        assert_eq!(c.debug_active_mode(), None);
        assert!(c.debug_preedit().is_empty());
        assert_eq!(c.debug_candidate_count(), 0);
        assert!(!c.debug_has_more());
        assert!(!c.debug_scope_relaxed());
        letters(&c, "a");
        assert_eq!(c.debug_input_buffer(), "a", "主路径照常组码");
    }

    /// 符号通配键：模式内先于选词 / 标点顶屏进缓冲。
    #[test]
    fn reverse_mode_symbol_wildcard_key_enters_buffer() {
        let (c, _g) = coord("sym", |cfg| cfg.schema.codetable.wildcard_key = "?".into());
        key(&c, VK_BACKSLASH, false);
        key(&c, VK_SLASH, true); // ?
        letters(&c, "b");
        assert_eq!(c.debug_preedit(), "\\?b");
        assert_eq!(c.debug_all_candidate_texts(), ["甲", "丁"]);
    }

    /// 选中上屏候选文本并退出（不上屏编码）。
    #[test]
    fn reverse_selecting_commits_candidate_and_exits() {
        let (c, _g) = coord("sel", |_| {});
        key(&c, VK_BACKSLASH, false);
        letters(&c, "zb");
        key(&c, 0x20, false); // 空格
        assert_eq!(c.debug_active_mode(), None);
    }

    fn press(c: &Coordinator, vk: u32) -> wind_bridge::handler::KeyAction {
        c.handle_key_event(&KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: 0,
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        })
    }

    fn inserted(act: &wind_bridge::handler::KeyAction) -> Option<&str> {
        match act {
            wind_bridge::handler::KeyAction::InsertText { text, .. } => Some(text),
            // 顶字重开：先上屏、组合区延后重建（`commit_then_new_composition`）。
            wind_bridge::handler::KeyAction::CommitThenDeferComposition { commit_text, .. } => {
                Some(commit_text)
            }
            _ => None,
        }
    }

    /// 选中上屏的是**候选文本**（不是编码），且模式随之退出。
    #[test]
    fn reverse_space_commits_highlighted_text() {
        let (c, _g) = coord("txt", |_| {});
        key(&c, VK_BACKSLASH, false);
        letters(&c, "zb");
        let act = press(&c, 0x20);
        assert_eq!(inserted(&act), Some("甲"));
        assert_eq!(c.debug_active_mode(), None);
    }

    /// 出厂 `input.reverse.enabled = false`：绑了键也不进入，触发键落普通流程。
    #[test]
    fn reverse_disabled_does_not_enter() {
        let (c, _g) = coord("off", |cfg| cfg.input.reverse.enabled = false);
        key(&c, VK_BACKSLASH, false);
        assert_eq!(c.debug_active_mode(), None);
    }

    /// 缓冲非空时按触发键：顶屏高亮候选再进入（commit-and-enter 通路）。
    #[test]
    fn reverse_trigger_with_pending_input_commits_then_enters() {
        let (c, _g) = coord("top", |_| {});
        letters(&c, "ab");
        let act = press(&c, VK_BACKSLASH);
        assert_eq!(inserted(&act), Some("甲"));
        assert_eq!(c.debug_active_mode(), Some("reverse"));
        assert_eq!(c.debug_preedit(), "\\");
    }

    /// ★ 裁决 13：退出时翻页 / 放宽三位一并复位。上面的端到端用例候选不足一批、放宽也没触发，
    /// 三位本就是假，测不出复位——这里先把它们置脏再退出。
    #[test]
    fn exit_resets_paging_and_relax_bits() {
        let (c, _g) = coord("bits", |_| {});
        key(&c, VK_BACKSLASH, false);
        letters(&c, "zb");
        {
            let mut st = c.state.lock().unwrap();
            assert_eq!(st.candidate_limit, crate::wildcard::WILDCARD_INITIAL_LIMIT);
            st.has_more = true;
            st.scope_relaxed = true;
        }
        key(&c, VK_ESCAPE, false);
        let st = c.state.lock().unwrap();
        assert!(!st.has_more && !st.scope_relaxed);
        assert_eq!(st.candidate_limit, 0);
        assert!(st.special_buffer.is_empty() && st.special_prefix.is_empty());
    }
}
