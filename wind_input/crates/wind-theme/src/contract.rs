//! 标准色契约的引擎兜底（设计 `docs/design/text-span-colors.md` §5.4「引擎兜底」）。
//!
//! 契约名（候选注释 / 悬停提示模板里 `$[名]{…}` 写的那些名字）由 `_base` 定义，继承 `_base`
//! 的主题天然都有。没写 `base` 的第三方主题一个也没有，`$[error]{…}` 就静默回落正文色。本模块
//! 在调色板解析后把**主题没写的**契约名补上：主题写了的一个不动，契约外的名字一个不加
//! （不是隐式继承 `_base`——那会连带改掉第三方主题的布局与其它颜色）。
//!
//! 补值的唯一来源是编译期嵌入的 `_base/theme.toml` 本身，而不是一张手抄的常量表：两处写同一组
//! 值，迟早只改一处，而这里漂移了没有任何症状（只有无 base 主题看得到）。嵌入而不是运行时从
//! 主题目录读：兜底不该依赖搜索链里恰好有 `_base`（设置页预览、移动端拉调色板各走各的目录）。
//! 代价是本 crate 依赖仓内 `data/` 布局：单独 `cargo package` / vendor 本 crate 会编译失败
//! （失败是显式的；各端构建都在完整仓内，改 `_base` 会经 dep-info 触发重编）。
//!
//! 引用的解析口径：
//! - 契约名之间的引用按**本主题**解析——`selection_text = "${text}"` 取本主题的 text，
//!   `accent_text = "${accent}"` 取本主题的强调色；本主题也没有，再取兜底值。
//! - 契约外的名字（`accent = "${primary}"` 里的 `primary`）只取 `_base` 自己的值：它不在契约
//!   里，第三方主题的同名色未必是同一个意思。

use std::collections::HashMap;
use std::sync::OnceLock;

use toml::Value;

use crate::palette::{Rgba, resolve_palette};

/// 候选窗用的契约名；每个在气泡里另有 `tooltip_<名>`（§5.4）。
pub const NAMES: [&str; 11] = [
    "text",
    "text_dim",
    "text_hint",
    "accent",
    "on_accent",
    "selection_text",
    "accent_text",
    "success",
    "warning",
    "error",
    "info",
];

/// 全部契约名：[`NAMES`] 及其 `tooltip_*`。
pub fn all_names() -> impl Iterator<Item = String> {
    NAMES
        .iter()
        .flat_map(|n| [n.to_string(), format!("tooltip_{n}")])
}

const BASE_THEME: &str = include_str!("../../../../data/themes/_base/theme.toml");

/// `_base` 的 `[colors]` 原表（未求值）。
fn base_colors() -> &'static toml::Table {
    static COLORS: OnceLock<toml::Table> = OnceLock::new();
    COLORS.get_or_init(|| {
        let v: Value = toml::from_str(BASE_THEME).expect("嵌入的 _base/theme.toml 解析失败");
        v.get("colors")
            .and_then(Value::as_table)
            .cloned()
            .expect("嵌入的 _base/theme.toml 没有 [colors]")
    })
}

/// 把主题 `colors` 段里**没写**的契约名补进已解析的 `palette`。
///
/// 「写没写」看 `colors` 的键而不是解析结果：主题写了却解析不出（坏值、`transparent`、断链）
/// 是作者自己的选择，兜底不替它改成 `_base` 的色。
pub fn fill_missing(colors: Option<&Value>, palette: &mut HashMap<String, Rgba>, is_dark: bool) {
    let own = colors.and_then(Value::as_table);
    let written = |n: &str| own.is_some_and(|t| t.contains_key(n));
    // `_base` 原表垫底，主题写了的契约名换成本主题的终值（解析不出的删掉，引用它的也跟着
    // 解析不出，与主题里写 `${它}` 的结果一致）；契约外的名字保持 `_base` 的原样。
    let mut table = base_colors().clone();
    for n in all_names().filter(|n| written(n)) {
        match palette.get(&n) {
            Some(c) => {
                let hex = format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]);
                table.insert(n, Value::String(hex));
            }
            None => {
                table.remove(&n);
            }
        }
    }
    let fallback = resolve_palette(Some(&Value::Table(table)), is_dark);
    for n in all_names().filter(|n| !written(n)) {
        if let Some(c) = fallback.get(&n) {
            palette.insert(n, *c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::parse_hex;

    fn fill(src: &str, dark: bool) -> HashMap<String, Rgba> {
        let v: Value = toml::from_str(src).unwrap();
        let mut p = resolve_palette(Some(&v), dark);
        fill_missing(Some(&v), &mut p, dark);
        p
    }

    fn hex(s: &str) -> Rgba {
        parse_hex(s).unwrap()
    }

    /// `_base` 本身的契约终值（亮 / 暗）——兜底的期望值就从它来，不另抄一份。
    fn base(dark: bool) -> HashMap<String, Rgba> {
        resolve_palette(Some(&Value::Table(base_colors().clone())), dark)
    }

    /// 空主题：契约名全数补齐，值等于 `_base` 的。
    #[test]
    fn empty_theme_gets_every_contract_name_from_base() {
        for dark in [false, true] {
            let p = fill("", dark);
            let b = base(dark);
            for n in all_names() {
                assert_eq!(p.get(&n), b.get(&n), "dark={dark}: {n}");
            }
            assert_eq!(p.len(), 22, "只补契约名，不带进 _base 的其它名字");
        }
    }

    /// 主题写了的名字一个不动；契约外的名字一个不加。
    #[test]
    fn own_values_are_never_overridden() {
        let src = r##"
error = "#010203"
tooltip_error = "#040506"
text = { light = "#111111", dark = "#EEEEEE" }
primary = "#ABCDEF"
"##;
        for dark in [false, true] {
            let p = fill(src, dark);
            assert_eq!(p["error"], hex("#010203"));
            assert_eq!(p["tooltip_error"], hex("#040506"));
            assert_eq!(p["text"], hex(if dark { "#EEEEEE" } else { "#111111" }));
            for extra in ["bg", "surface", "menu_bg", "tooltip_bg", "status_bg"] {
                assert!(!p.contains_key(extra), "不该补契约外的 {extra}");
            }
        }
    }

    /// 写了却解析不出的契约名不补，引用它的兜底也跟着解析不出。
    #[test]
    fn written_but_broken_names_stay_missing() {
        let p = fill(r#"text = "${nowhere}""#, false);
        assert!(!p.contains_key("text"));
        assert!(
            !p.contains_key("selection_text"),
            "selection_text = ${{text}}"
        );
        assert_eq!(p["error"], base(false)["error"]);
    }

    /// 引用型的兜底按本主题解析：accent_text 借本主题强调色，selection_text 借本主题 text，
    /// tooltip_accent_text 借本主题 tooltip_accent。
    #[test]
    fn references_resolve_against_this_theme() {
        let src = r##"
accent = "#FF4554"
text = { light = "#111827", dark = "#F6FAFD" }
tooltip_accent = "#00C3E3"
tooltip_text = "#FAFAFA"
"##;
        for dark in [false, true] {
            let p = fill(src, dark);
            assert_eq!(p["accent_text"], hex("#FF4554"), "dark={dark}");
            let text = hex(if dark { "#F6FAFD" } else { "#111827" });
            assert_eq!(p["selection_text"], text);
            assert_eq!(p["tooltip_accent_text"], hex("#00C3E3"));
            assert_eq!(p["tooltip_on_accent"], hex("#FAFAFA"));
            assert_eq!(p["tooltip_selection_text"], hex("#FAFAFA"));
        }
        // tooltip_text 没写：暗档 = ${text}，取本主题的 text。
        let p = fill(r##"text = "#123456""##, true);
        assert_eq!(p["tooltip_text"], hex("#123456"));
    }

    /// 主题连 accent 也没有：accent 与 accent_text 取 `_base` 的；契约外的 primary 不借主题的。
    #[test]
    fn missing_accent_uses_base_not_theme_primary() {
        let p = fill(r##"primary = "#00FF00""##, false);
        assert_eq!(p["accent"], hex("#4285F4"));
        assert_eq!(p["accent_text"], hex("#4285F4"));
    }
}
