//! 定制版「设置界面减法」：`data_custom/custom.toml` 的 `[settings] hide` 选择器。
//!
//! # 语义
//!
//! 这一段只决定**设置程序显示什么**，不影响任何功能——被隐藏的设置照常生效，值仍来自
//! 定制层 / 用户层。与同一份清单里的 `[schemas] hide` / `[themes] hide`（「这个东西在本
//! 定制版里**不存在**」）刻意不同：定制者要关掉一个功能，应在 `data_custom/config.toml`
//! 里把值设好，再在这里把它藏起来。把两件事合成一个开关，就会重演 `hidden` 与 `hide`
//! 被混用的那个坑（见 `docs/design/data-custom-layer.md` 契约 5）。
//!
//! # 为什么住在 wind-config
//!
//! 消费者有两个：`wind_input config check --custom`（打包前校验）与设置程序（真正按它
//! 隐藏）。两边各写一份解析，迟早一边认 `key:` 带空格、另一边不认；而设置程序本就依赖
//! 本 crate，放这里是唯一不用复制的位置。
//!
//! # 选择器
//!
//! - `key:<配置键>`：精确匹配设置项的 key；以 `*` 结尾时按前缀匹配（`key:input.temp_english.*`）。
//!   用配置键而不是界面上的分区名：分区名是文案，随界面调整改名；配置键有迁移保障。
//!   一个分区里的项全被隐藏后，设置程序会把那张卡片连同二级导航一并去掉，故不需要单独的
//!   「隐藏分区」语法。
//! - `feature:<id>`：不由设置清单驱动的界面元素，取值见 [`SETTINGS_FEATURES`]。

/// `feature:` 选择器认识的 id。新增一项 = 设置程序那边多一个消费点，两边同时改。
///
/// - `wizard`：设置向导（不自动弹出，侧栏也不显示入口）。定制版通常已替用户选好方案，
///   向导的「选方案 + 套推荐预设」会写进用户层、盖掉定制层调好的值。
pub const SETTINGS_FEATURES: &[&str] = &["wizard"];

/// 一条解析过的 `[settings] hide` 选择器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsSelector {
    /// 设置项 key。`prefix = true` 时 `pattern` 是去掉尾部 `*` 后的前缀。
    Key { pattern: String, prefix: bool },
    /// 界面元素，取值属于 [`SETTINGS_FEATURES`]。
    Feature(String),
}

impl SettingsSelector {
    /// 解析一条选择器。错误文案直接给定制者看（`config check` 原样输出），故写成「哪里错 +
    /// 该怎么写」。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        let Some((kind, body)) = s.split_once(':') else {
            return Err(format!(
                "`{s}` 缺少类型前缀。写成 `key:<配置键>` 或 `feature:<{}>`。",
                SETTINGS_FEATURES.join("|")
            ));
        };
        let body = body.trim();
        match kind.trim() {
            "key" => {
                if body.is_empty() || body == "*" {
                    return Err(format!(
                        "`{s}` 没有给出配置键。隐藏全部设置不受支持——要整体精简，请逐段列出。"
                    ));
                }
                let (pattern, prefix) = match body.strip_suffix('*') {
                    Some(p) => (p, true),
                    None => (body, false),
                };
                if pattern.contains('*') {
                    return Err(format!("`{s}` 里的 `*` 只能出现在末尾（前缀匹配）。"));
                }
                Ok(Self::Key {
                    pattern: pattern.to_string(),
                    prefix,
                })
            }
            "feature" => {
                if SETTINGS_FEATURES.contains(&body) {
                    Ok(Self::Feature(body.to_string()))
                } else {
                    Err(format!(
                        "`{s}` 不是认识的界面元素。可用：{}。",
                        SETTINGS_FEATURES.join(" / ")
                    ))
                }
            }
            other => Err(format!(
                "`{s}` 的类型 `{other}` 不认识。只有 `key:` 与 `feature:` 两种。"
            )),
        }
    }

    /// 是否命中设置项 `key`。`Feature` 永不命中设置项。
    pub fn matches_key(&self, key: &str) -> bool {
        match self {
            Self::Key { pattern, prefix } => {
                if *prefix {
                    key.starts_with(pattern.as_str())
                } else {
                    key == pattern
                }
            }
            Self::Feature(_) => false,
        }
    }
}

/// 一份解析好的 `[settings] hide`。解析失败的条目被丢弃（设置程序侧不该因为一条写错的
/// 选择器就整段失效），错误另由 [`SettingsHide::parse`] 的第二个返回值交给调用方记日志。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsHide {
    selectors: Vec<SettingsSelector>,
}

impl SettingsHide {
    /// 解析整段。返回（可用的选择器集合，每条坏选择器的错误文案）。
    pub fn parse<S: AsRef<str>>(raw: &[S]) -> (Self, Vec<String>) {
        let mut selectors = Vec::new();
        let mut errors = Vec::new();
        for r in raw {
            if r.as_ref().trim().is_empty() {
                continue;
            }
            match SettingsSelector::parse(r.as_ref()) {
                Ok(sel) => selectors.push(sel),
                Err(e) => errors.push(e),
            }
        }
        (Self { selectors }, errors)
    }

    /// 没有任何有效选择器。
    pub fn is_empty(&self) -> bool {
        self.selectors.is_empty()
    }

    /// 设置项 `key` 是否被隐藏。
    pub fn hides_key(&self, key: &str) -> bool {
        self.selectors.iter().any(|s| s.matches_key(key))
    }

    /// 界面元素 `id` 是否被隐藏。
    pub fn hides_feature(&self, id: &str) -> bool {
        self.selectors
            .iter()
            .any(|s| matches!(s, SettingsSelector::Feature(f) if f == id))
    }

    /// 全部选择器（设置程序用来报「哪条一项都没命中」）。
    pub fn selectors(&self) -> &[SettingsSelector] {
        &self.selectors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_key_matches_only_itself() {
        let sel = SettingsSelector::parse("key:input.temp_english.enabled").unwrap();
        assert!(sel.matches_key("input.temp_english.enabled"));
        assert!(!sel.matches_key("input.temp_english.enabled_x"));
        assert!(!sel.matches_key("input.temp_english"));
    }

    #[test]
    fn trailing_star_is_prefix_match() {
        let sel = SettingsSelector::parse("key:input.temp_english.*").unwrap();
        assert!(sel.matches_key("input.temp_english.enabled"));
        assert!(sel.matches_key("input.temp_english.trigger_keys"));
        assert!(!sel.matches_key("input.temp_pinyin.enabled"));
    }

    #[test]
    fn whitespace_around_parts_is_tolerated() {
        let sel = SettingsSelector::parse("  key : ui.toolbar.* ").unwrap();
        assert!(sel.matches_key("ui.toolbar.visible"));
    }

    #[test]
    fn known_feature_parses() {
        assert_eq!(
            SettingsSelector::parse("feature:wizard").unwrap(),
            SettingsSelector::Feature("wizard".into())
        );
    }

    #[test]
    fn malformed_selectors_are_rejected() {
        for bad in [
            "input.temp_english.*",
            "key:",
            "key:*",
            "key:input.*.enabled",
            "feature:nope",
            "page:stats",
        ] {
            assert!(SettingsSelector::parse(bad).is_err(), "{bad} 应被拒");
        }
    }

    #[test]
    fn feature_never_matches_a_key() {
        let sel = SettingsSelector::parse("feature:wizard").unwrap();
        assert!(!sel.matches_key("wizard"));
    }

    /// 一条坏选择器不连累其余：设置程序那边宁可少藏一项，也不能整段失效。
    #[test]
    fn bad_entries_are_dropped_good_ones_kept() {
        let (hide, errors) =
            SettingsHide::parse(&["key:ui.toolbar.*", "bogus", "", "feature:wizard"]);
        assert_eq!(errors.len(), 1);
        assert!(hide.hides_key("ui.toolbar.visible"));
        assert!(hide.hides_feature("wizard"));
        assert!(!hide.hides_feature("other"));
    }

    /// 清单里的 `[settings]` 段落进 `CustomManifest::settings`（与 `[schemas]` 同形）。
    #[test]
    fn manifest_settings_section_deserializes() {
        let m: crate::CustomManifest =
            toml::from_str("[settings]\nhide = [\"feature:wizard\"]\n").unwrap();
        assert_eq!(m.settings.hide, vec!["feature:wizard".to_string()]);
        let (hide, errors) = SettingsHide::parse(&m.settings.hide);
        assert!(errors.is_empty());
        assert!(hide.hides_feature("wizard"));
    }
}
