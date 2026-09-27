//! 调色板解析：把 colors 段解析为「名称 → RGBA」具体色值。
//!
//! 与 Go 版本 `wind_input/pkg/theme/palette.go` 对齐。
//! 处理 `${var}` 引用（递归 + 环检测）、{light,dark} 变体、#RRGGBB[AA] 十六进制。

use std::collections::{HashMap, HashSet};
use toml::Value;

/// 颜色 [R, G, B, A]，与 UI 缓冲约定一致。
pub type Rgba = [u8; 4];

/// 解析 `#RRGGBB` / `#RRGGBBAA`（`#` 可省），以及 `#RGB`（`#` 必带，扩写为 `#RRGGBB`）。
///
/// 3 位简写必须带 `#`：本函数的调用方不止主题，还有语言栏配置（`[ui.langbar]` 各色，用户手填）
/// 与模板内联色（`$[…]{}`，名字与色值共用一个位置）。3 位也可裸写的话，`bad`、`fed`、`ace`
/// 这类英文单词会被当成颜色。6 / 8 位的裸写保持原样——那是既有行为，手写配置里见过。
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(d) = s.strip_prefix('#')
        && d.len() == 3
        && d.is_ascii()
    {
        // 单个十六进制位 v 扩写为 vv，即 v × 17。
        let n = |i: usize| u8::from_str_radix(&d[i..=i], 16).ok().map(|v| v * 17);
        return Some([n(0)?, n(1)?, n(2)?, 255]);
    }
    let s = s.trim_start_matches('#');
    // 先挡非 ASCII：下面按字节切片，多字节字符会让切点落在字中间而 panic。
    if !s.is_ascii() {
        return None;
    }
    let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    match s.len() {
        6 => Some([h(0)?, h(2)?, h(4)?, 255]),
        8 => Some([h(0)?, h(2)?, h(4)?, h(6)?]),
        _ => None,
    }
}

/// 解析整个调色板：colors 段（Mapping）→ {名称: Rgba}。
/// is_dark 选择 {light,dark} 变体。derive 等非颜色项被忽略。
pub fn resolve_palette(colors: Option<&Value>, is_dark: bool) -> HashMap<String, Rgba> {
    let mut out = HashMap::new();
    let map = match colors.and_then(|c| c.as_table()) {
        Some(m) => m,
        None => return out,
    };
    let mut visiting = HashSet::new();
    let names: Vec<String> = map.keys().cloned().collect();
    for name in names {
        resolve_name(&name, map, is_dark, &mut out, &mut visiting);
    }
    out
}

/// 按名解析（递归 + 记忆 + 环检测）。
fn resolve_name(
    name: &str,
    map: &toml::Table,
    is_dark: bool,
    out: &mut HashMap<String, Rgba>,
    visiting: &mut HashSet<String>,
) -> Option<Rgba> {
    if let Some(c) = out.get(name) {
        return Some(*c);
    }
    if visiting.contains(name) {
        return None; // 环
    }
    visiting.insert(name.to_string());
    let v = map.get(name)?;
    let color = resolve_value(v, map, is_dark, out, visiting);
    visiting.remove(name);
    if let Some(c) = color {
        out.insert(name.to_string(), c);
    }
    color
}

/// 解析一个颜色值（字符串 hex / `${var}` / {light,dark}）。
fn resolve_value(
    v: &Value,
    map: &toml::Table,
    is_dark: bool,
    out: &mut HashMap<String, Rgba>,
    visiting: &mut HashSet<String>,
) -> Option<Rgba> {
    match v {
        Value::String(s) => resolve_str(s, map, is_dark, out, visiting),
        Value::Table(m) => {
            // {light: .., dark: ..} 变体
            let key = if is_dark { "dark" } else { "light" };
            if let Some(inner) = m.get(key) {
                resolve_value(inner, map, is_dark, out, visiting)
            } else {
                None // derive 等非颜色映射
            }
        }
        _ => None,
    }
}

fn resolve_str(
    s: &str,
    map: &toml::Table,
    is_dark: bool,
    out: &mut HashMap<String, Rgba>,
    visiting: &mut HashSet<String>,
) -> Option<Rgba> {
    let s = s.trim();
    if let Some(var) = s.strip_prefix("${").and_then(|x| x.strip_suffix('}')) {
        resolve_name(var, map, is_dark, out, visiting)
    } else {
        parse_hex(s)
    }
}

/// 把 views 中的颜色 token（"${name}" 或 "#hex" 或 {light,dark}）按已解析调色板转为 Rgba。
pub fn color_token(v: &Value, palette: &HashMap<String, Rgba>, is_dark: bool) -> Option<Rgba> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if let Some(var) = s.strip_prefix("${").and_then(|x| x.strip_suffix('}')) {
                palette.get(var).copied()
            } else {
                parse_hex(s)
            }
        }
        Value::Table(m) => {
            let key = if is_dark { "dark" } else { "light" };
            m.get(key)
                .and_then(|inner| color_token(inner, palette, is_dark))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex() {
        assert_eq!(parse_hex("#FF8040"), Some([255, 128, 64, 255]));
        assert_eq!(parse_hex("#00000080"), Some([0, 0, 0, 128]));
        assert_eq!(parse_hex("nope"), None);
        // 非 ASCII：6 字节的「红红」长度凑得上 6 位分支，按字节切片会切在字中间而 panic。
        // 用户手改 config.toml 就能喂进来（角标色、语言栏主字色都走这里）。
        assert_eq!(parse_hex("红红"), None);
        assert_eq!(parse_hex("#红红ab"), None);
    }

    #[test]
    fn short_hex_expands_each_digit() {
        assert_eq!(parse_hex("#F80"), Some([0xFF, 0x88, 0x00, 255]));
        assert_eq!(parse_hex(" #abc "), Some([0xAA, 0xBB, 0xCC, 255]));
        assert_eq!(parse_hex("#GG0"), None);
        // 「红」恰是 3 字节：不先挡非 ASCII 的话，按字节切片会切在字中间而 panic。
        assert_eq!(parse_hex("#红"), None);
    }

    /// 3 位不带 `#` 一律不认：语言栏配置与内联色里，`bad` / `fed` / `ace` 是词不是颜色。
    #[test]
    fn short_hex_requires_hash() {
        for word in ["bad", "fed", "ace", "F80", "##F80"] {
            assert_eq!(parse_hex(word), None, "{word} 不该被当成颜色");
        }
        // 6 / 8 位的裸写是既有行为，保持。
        assert_eq!(parse_hex("FF8040"), Some([255, 128, 64, 255]));
    }

    #[test]
    fn test_palette_var_and_lightdark() {
        let src = r##"
primary = "#4285F4"
accent = "${primary}"
bg = { light = "#FFFFFF", dark = "#2D2D2D" }
selection_text = "${text}"
text = { light = "#1E1E1E", dark = "#E0E0E0" }
"##;
        let v: Value = toml::from_str(src).unwrap();
        let light = resolve_palette(Some(&v), false);
        assert_eq!(light["primary"], [0x42, 0x85, 0xF4, 255]);
        assert_eq!(light["accent"], [0x42, 0x85, 0xF4, 255]); // ${primary}
        assert_eq!(light["bg"], [255, 255, 255, 255]);
        assert_eq!(light["selection_text"], [0x1E, 0x1E, 0x1E, 255]); // ${text}

        let dark = resolve_palette(Some(&v), true);
        assert_eq!(dark["bg"], [0x2D, 0x2D, 0x2D, 255]);
        assert_eq!(dark["text"], [0xE0, 0xE0, 0xE0, 255]);
    }
}
