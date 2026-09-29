//! 码表通配输入（万能键）的按键裁决与冲突体检。
//! 设计见 `docs/design/codetable-wildcard.md` §3.3–§3.4、§5.3。
//!
//! # 为什么「哪几位是通配」由协调器定，而不是引擎按内容自判
//!
//! 缓冲本身不带标记：首位让位后进缓冲的字面 `z`（`zz*` 短语的活码）与作通配的 `z` 同形。
//! 故裁决按**同一套规则**从缓冲内容重算，不记标志位——理由同 `buffer_has_literal_symbol`：
//! 缓冲会被退格、光标中插改写，来源标志迟早与内容对不上，而字符与规则不会。规则必须
//! **与历史无关**（`z_key_repeat` 取配置开关而非「有没有上屏历史」），否则同一串缓冲
//! 前后两次重算会得出不同结论。

use crate::coordinator::{Coordinator, State};
use crate::key_convert::{punct_char, punct_source_vk};
use tracing::warn;
use wind_keys::keymap;

/// 通配键此刻的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WildcardDecision {
    /// 让位：按它原本的身份走（码元 / 模式 / 选词 / 翻页 / 标点），与关闭通配时逐键相同。
    Yield,
    /// 作通配进缓冲。
    Enter,
}

/// 通配键 → (主键盘 VK, 是否需要 Shift)。字母恒无 Shift（缓冲恒存小写）。
fn wildcard_key_vk(key: char) -> Option<(u32, bool)> {
    if key.is_ascii_lowercase() {
        return Some((keymap::VK_A + (key as u32 - 'a' as u32), false));
    }
    let vk = punct_source_vk(key)?;
    Some((vk, punct_char(vk, false) != Some(key)))
}

impl Coordinator {
    /// 按下的字符 `ch` 此刻是否作通配进缓冲——两个接线点（字母臂、`try_code_char_gate`）
    /// 的唯一入口。
    ///
    /// ★ overlay（临拼 / 临英 / 快捷输入 / 特殊模式 / 辅助码…）激活时恒 `false`：通配键取自
    /// **活跃方案**的引擎，而 overlay 用的是别的方案或别的输入语义（临拼里 `z` 是拼音字母）。
    /// 引擎管理器没有 overlay 状态，这道门只能由协调器把。现有分派里 overlay 在两个接线点之前
    /// 就 return 了，这里是结构性兜底：日后哪条 overlay 路径复用了这两个接线点，不会静默带上通配。
    pub(crate) fn wildcard_enters(&self, state: &State, ch: char) -> bool {
        state.active.is_none()
            && self.engine_mgr.active_wildcard_key() == Some(ch)
            && self.wildcard_decision(&state.input_buffer, ch) == WildcardDecision::Enter
    }

    /// §3.3 按键裁决。`buffer` 是**按下前**的输入缓冲。
    ///
    /// ⚠️ 调用点必须晚于 `try_activate_mode` 与 `try_z_fallback`（顺序铁律，见
    /// `codetable-input-chars.md` §3.4）：首位让位本就是它们先赢；这里的首位判据只处理
    /// 「它们都没接手」之后的去向。
    pub(crate) fn wildcard_decision(&self, buffer: &str, key: char) -> WildcardDecision {
        let yield_ = if buffer.is_empty() {
            self.wildcard_lead_yields(key)
        } else {
            // 首字符是让位进来的字面通配键 ⇒ 整轮都是字面（否则 `zzbd` 变 `z?bd`，
            // 出厂 `zz*` 短语整批消失）。
            self.wildcard_lead_literal(buffer, key) || !self.wildcard_mid_owners(key).is_empty()
        };
        if yield_ {
            WildcardDecision::Yield
        } else {
            WildcardDecision::Enter
        }
    }

    /// 首位：通配键已绑定任何功能 / 模式即让位（spec §3.3 首位一行）。
    fn wildcard_lead_yields(&self, key: char) -> bool {
        let Some((vk, _)) = wildcard_key_vk(key) else {
            return true;
        };
        // `bound_action_for` 是三层链（方案 `[key_actions]` → 全局 → `z_key_action`）。
        if self.bound_action_for(vk).is_some_and(|a| a.is_enabled()) {
            return true;
        }
        // 取**配置开关**而不是 `z_key_repeat_text()`（有无上屏历史）：本判据要从缓冲反复
        // 重算，随历史变化的判据会让同一串缓冲前后解释不一。
        if key == 'z' && self.engine_mgr.codetable_settings().z_key_repeat {
            return true;
        }
        if self.is_any_mode_trigger(vk) {
            return true;
        }
        self.has_code_prefix(&key.to_string())
    }

    /// 缓冲首字符是通配键，且首位按规则会让位 ⇒ 它是字面码元，整轮组码不作通配。
    fn wildcard_lead_literal(&self, buffer: &str, key: char) -> bool {
        buffer.starts_with(key) && self.wildcard_lead_yields(key)
    }

    /// 非首位：通配键若是组码中途的功能键，返回占用它的功能名（空 = 不冲突）。
    ///
    /// 判据反查现有函数，与 `code_char_conflicts` / `symbol_buffer_key_free` 同源——
    /// ⚠️ 那两处的 owners 链增减时这里要跟着改。
    ///
    /// 会话表里一个键只有一个动作，故按动作**细分**成一条名字：`select_key_offset` /
    /// `select_char_index` 都是 `session_action_for` 的派生，再单独问一遍就会把同一个 `;`
    /// 报成「会话键」和「次选键」两条。
    fn wildcard_mid_owners(&self, key: char) -> Vec<&'static str> {
        use wind_config::{AuxCodeShare, SessionAction};
        let Some((vk, shift)) = wildcard_key_vk(key) else {
            return Vec::new();
        };
        let mut owners: Vec<&'static str> = Vec::new();
        if let Some(a) = self.session_action_for(vk, shift, true) {
            let owner = match a {
                // 选词 / 以词定字的两个消费点自带 `!shift` 守卫：shift 形态本就不归它们。
                SessionAction::SelectCandidate(_) | SessionAction::SelectChar(_) if shift => None,
                SessionAction::SelectCandidate(2) => Some("次选键"),
                SessionAction::SelectCandidate(3) => Some("三选键"),
                SessionAction::SelectCandidate(_) => Some("选词键"),
                SessionAction::SelectChar(_) => Some("以词定字键"),
                SessionAction::AuxCode(AuxCodeShare::Solo) => Some("辅助码键"),
                SessionAction::AuxCode(AuxCodeShare::PageNext) => Some("辅助码/翻页键"),
                SessionAction::PagePrev | SessionAction::PageNext => Some("翻页键"),
                SessionAction::HighlightUp | SessionAction::HighlightDown => Some("高亮键"),
                SessionAction::Cancel => Some("取消键"),
                SessionAction::CommitHighlighted => Some("上屏键"),
                SessionAction::Command(_) => Some("命令键"),
                SessionAction::SingleChar(_) => Some("单字键"),
                SessionAction::None => None,
            };
            owners.extend(owner);
        }
        if self.manual_separator_key(vk) {
            owners.push("音节分隔符");
        }
        owners
    }

    /// 通配键与既有按键功能的冲突清单（只告警）。空 = 无冲突或通配关闭。
    ///
    /// 首位让位**不报**（那是设计内行为）；只报非首位让位（意味着通配在组码中实际不可用）
    /// 与「通配键是显式配置的方案码元」（该码元将被通配吞掉）。
    ///
    /// 码元一项只在 `input_chars` **显式配置**时报：五笔 86 未配 `input_chars`，默认 `a-z`
    /// 含 `z`，但词库里 `z` 开头 / 含 `z` 的码为 0——按默认集报会让出厂主用例每次启动都告警。
    pub fn wildcard_conflicts(&self) -> Vec<&'static str> {
        let Some(key) = self.engine_mgr.active_wildcard_key() else {
            return Vec::new();
        };
        let mut owners = self.wildcard_mid_owners(key);
        if self.wildcard_is_configured_code_char(key) {
            owners.push("方案码元（组码中将被通配吞掉）");
        }
        owners
    }

    fn wildcard_is_configured_code_char(&self, key: char) -> bool {
        let charset = self.engine_mgr.active_input_chars();
        !charset.is_default_alpha() && charset.contains(key)
    }

    /// 启动时把 [`Self::wildcard_conflicts`] 写进日志。只告警，不改行为。
    ///
    /// 两类后果相反，故分两条：功能键冲突是「通配让位、组码中用不了」；码元冲突是「通配
    /// 照样生效、该码元被吞掉」。混在一句里用户读不出该改哪边。
    pub(crate) fn warn_wildcard_conflicts(&self) {
        let Some(key) = self.engine_mgr.active_wildcard_key() else {
            return;
        };
        let owners = self.wildcard_mid_owners(key);
        if !owners.is_empty() {
            warn!(
                "通配键 {:?} 同时配作 {}；组码中它按原功能处理，通配在该方案下不可用——请在方案设置里换一个通配键",
                key,
                owners.join(" / ")
            );
        }
        if self.wildcard_is_configured_code_char(key) {
            warn!(
                "通配键 {:?} 是本方案显式配置的码元（input_chars）；组码中该码元将被通配吞掉，不再按字面参与编码——如非有意，请换一个通配键",
                key
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WildcardDecision::{Enter, Yield};
    use crate::coordinator::Coordinator;
    use std::path::PathBuf;
    use std::sync::Arc;
    use wind_config::Config;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
    }

    /// 五笔 86 + 通配键 `z`；缺数据返回 `None`（同集成测试的静默跳过约定）。
    fn wubi_z(tweak: impl FnOnce(&mut Config)) -> Option<Arc<Coordinator>> {
        if !data_dir()
            .join("schemas/wubi86/wubi86_jidian.dict.yaml")
            .exists()
        {
            eprintln!("跳过：五笔词库不存在");
            return None;
        }
        let mut cfg = Config::default();
        cfg.schema.available = vec!["wubi86".into()];
        cfg.schema.active = "wubi86".into();
        cfg.input.default.chinese_mode = true;
        cfg.schema.codetable.wildcard = true;
        cfg.schema.codetable.wildcard_key = "z".into();
        tweak(&mut cfg);
        Some(Coordinator::new_headless(cfg, Some(&data_dir())))
    }

    /// 首位 z 是 `zz*` 活码 ⇒ 首位让位，且**整轮**字面（后续 z 也让位）；
    /// 非 z 起头的缓冲里 z 照常作通配。
    #[test]
    fn leading_literal_wildcard_makes_whole_buffer_literal() {
        let Some(c) = wubi_z(|_| {}) else { return };
        let seed = |code: &str, text: &str| wind_phrase::PhraseSeed {
            code: code.into(),
            text: text.into(),
            weight: 0,
            position: 0,
            is_system: true,
            category: String::new(),
        };
        c.debug_install_phrases(vec![seed("zzbd", "、")]);
        assert_eq!(c.wildcard_decision("", 'z'), Yield, "首位活码让位");
        assert_eq!(c.wildcard_decision("z", 'z'), Yield, "首位字面 ⇒ 整轮字面");
        assert_eq!(c.wildcard_decision("zzb", 'z'), Yield);
        assert_eq!(c.wildcard_decision("a", 'z'), Enter, "对照：a 起头照常通配");
    }

    /// overlay 激活时通配不生效（`wildcard_enters` 的 `state.active` 门）。
    /// 现有分派里 overlay 在两个接线点之前就 return，集成层测不到这道门，故在此直接钉。
    #[test]
    fn overlay_active_blocks_wildcard() {
        let Some(c) = wubi_z(|_| {}) else { return };
        let mut st = c.state.lock().unwrap_or_else(|e| e.into_inner());
        st.input_buffer = "a".into();
        st.active = None;
        assert!(
            c.wildcard_enters(&st, 'z'),
            "对照：无 overlay 时 a 后的 z 作通配"
        );
        st.active = Some(crate::pipeline::ModeKind::TempPinyin);
        assert!(!c.wildcard_enters(&st, 'z'), "overlay 激活时不作通配");
    }

    /// `refresh_config_in_memory` 改了全局通配开关 ⇒ 透传集按**新**配置重算。
    /// 从管理器自身的全局副本取的话，这里拿到的仍是构造时的旧开关。
    #[test]
    fn refresh_config_in_memory_recomputes_wildcard_passthrough() {
        let Some(c) = wubi_z(|cfg| {
            cfg.schema.codetable.wildcard = false;
            cfg.schema.codetable.wildcard_key = "/".into();
        }) else {
            return;
        };
        assert!(
            c.rt().cn_passthrough_punct_chars.contains(&'/'),
            "前置：关闭时透传"
        );
        c.refresh_config_in_memory(|cfg| cfg.schema.codetable.wildcard = true);
        assert!(
            !c.rt().cn_passthrough_punct_chars.contains(&'/'),
            "开启后 `/` 不再透传"
        );
        c.refresh_config_in_memory(|cfg| cfg.schema.codetable.wildcard = false);
        assert!(
            c.rt().cn_passthrough_punct_chars.contains(&'/'),
            "关回后恢复透传"
        );
    }

    /// `z_key_repeat` 按**配置开关**让位，与有无上屏历史无关（新建协调器无历史）。
    /// 按 `z_key_repeat_text()` 判的话这里会得 Enter，同一串缓冲前后解释不一。
    #[test]
    fn z_key_repeat_yields_by_switch_not_history() {
        let Some(c) = wubi_z(|cfg| cfg.schema.codetable.z_key_repeat = true) else {
            return;
        };
        assert_eq!(c.wildcard_decision("", 'z'), Yield);
        assert_eq!(c.wildcard_decision("z", 'z'), Yield, "首位让位 ⇒ 整轮字面");
        assert_eq!(c.wildcard_decision("a", 'z'), Enter);
    }
}
