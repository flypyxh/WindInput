//! 用户词文本文件（设计 §2.2 / §2.3）：Rime `.dict.yaml` + 头部 `wind:` 扩展。列规格与注释规则
//! 复用 `wind_dict::rime_layout`（规则只此一份）；匹配键只经 [`normalize_key`]。

use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindHeader {
    pub kind: Option<String>,
    /// `code_spacing: syllable`：码以空格分音节；无空格码 = 边界未知（0），文本为单个汉字时为单音节。
    pub syllable_codes: bool,
    /// `escape: true`：text 列按 `escape_text_field` 规则转义。
    pub escape: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEntry {
    /// 该行在「按 `\n` 切开的全部行」里的下标（含头部）。补丁按它定位。
    pub line: usize,
    /// 规范化后的扁平码。
    pub code: String,
    pub text: String,
    pub boundary: u64,
    pub weight: i32,
}

#[derive(Clone, Debug)]
pub struct TextDict {
    pub header: WindHeader,
    pub entries: Vec<TextEntry>,
    pub bom: bool,
    pub crlf: bool,
    /// 解析不了而跳过的数据行（列不够、码 / 词空、权重不是数字；§2.5）。空行与注释不算。
    pub bad_lines: usize,
}

#[derive(Debug)]
pub enum ParseError {
    NotUtf8,
    BadHeader(String),
    NoBody,
}

#[derive(Deserialize, Default)]
struct RawHeader {
    #[serde(default)]
    wind: Option<RawWind>,
}

#[derive(Deserialize, Default)]
struct RawWind {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    code_spacing: Option<String>,
    #[serde(default)]
    escape: Option<bool>,
}

/// 匹配键规范化（§2.3，唯一实现）：码去空格得扁平码与音节边界（无空格 ⇒ 0），按方案小写；
/// 文本只做输入归一（行尾）。
pub fn normalize_key(code: &str, text: &str, lowercase: bool) -> (String, u64, String) {
    let (mut flat, boundary) = wind_wdict::split_spaced_code(code.trim());
    if lowercase {
        flat = flat.to_lowercase();
    }
    let text = wind_utils::text::normalize_input(text).into_owned();
    (flat, boundary, text)
}

/// 文本恰为一个 CJK 统一表意文字（含扩展区与兼容区）。
pub(crate) fn is_single_han(text: &str) -> bool {
    let mut it = text.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => matches!(
            c as u32,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3134F
        ),
        _ => false,
    }
}

pub fn parse(bytes: &[u8], path: &Path, lowercase: bool) -> Result<TextDict, ParseError> {
    let s = std::str::from_utf8(bytes).map_err(|_| ParseError::NotUtf8)?;
    let bom = s.starts_with('\u{feff}');
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let crlf = s
        .find('\n')
        .is_some_and(|i| i > 0 && s.as_bytes()[i - 1] == b'\r');
    let layout = wind_dict::rime_layout(s, path).ok_or(ParseError::NoBody)?;
    let head_text = &s[..layout.body_offset];
    let yaml = head_text
        .trim_start_matches("---")
        .trim_end()
        .trim_end_matches("...");
    let raw: RawHeader = if yaml.trim().is_empty() {
        RawHeader::default()
    } else {
        serde_yaml::from_str(yaml).map_err(|e| ParseError::BadHeader(e.to_string()))?
    };
    let wind = raw.wind.unwrap_or_default();
    let header = WindHeader {
        kind: wind.kind,
        syllable_codes: wind.code_spacing.as_deref() == Some("syllable"),
        escape: wind.escape.unwrap_or(false),
    };
    let mut entries = Vec::new();
    let mut bad_lines = 0usize;
    let mut offset = 0usize;
    for (idx, raw_line) in s.split('\n').enumerate() {
        let start = offset;
        offset += raw_line.len() + 1;
        if start < layout.body_offset {
            continue;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let is_comment_zone = layout.comment_cutoff.is_none_or(|c| start <= c);
        if line.starts_with('#') && is_comment_zone {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        let (Some(text), Some(code)) = (parts.get(layout.text_col), parts.get(layout.code_col))
        else {
            bad_lines += 1;
            continue;
        };
        if text.is_empty() || code.trim().is_empty() {
            bad_lines += 1;
            continue;
        }
        let weight = match layout
            .weight_col
            .and_then(|i| parts.get(i))
            .map(|w| w.trim())
        {
            None | Some("") => 0,
            Some(w) => match w.parse::<i32>() {
                Ok(w) => w,
                Err(_) => {
                    bad_lines += 1;
                    continue;
                }
            },
        };
        let text = if header.escape {
            wind_wdict::unescape_text_field(text)
        } else {
            (*text).to_string()
        };
        let (code, boundary, text) = normalize_key(code, &text, lowercase);
        let boundary = if !header.syllable_codes {
            0
        } else if boundary == 0 && is_single_han(&text) {
            // 空格载体表达不了单音节；一个汉字只对应一个音节
            0b1
        } else {
            boundary
        };
        entries.push(TextEntry {
            line: idx,
            code,
            text,
            boundary,
            weight,
        });
    }
    Ok(TextDict {
        header,
        entries,
        bom,
        crlf,
        bad_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn p(s: &str, lower: bool) -> TextDict {
        parse(s.as_bytes(), Path::new("user.dict.yaml"), lower).unwrap()
    }

    const HEAD: &str = "---\nname: pinyin.user\ncolumns: [text, code, weight]\nwind:\n  kind: words\n  code_spacing: syllable\n  escape: true\n...\n";

    #[test]
    fn parses_header_entries_and_boundaries() {
        let d = p(
            &format!("{HEAD}你好\tni hao\t100\n先\txian\t5\n# 注释\n\n西安\txi an\t\n"),
            false,
        );
        assert_eq!(d.header.kind.as_deref(), Some("words"));
        assert!(d.header.syllable_codes && d.header.escape);
        let e: Vec<(&str, &str, u64, i32)> = d
            .entries
            .iter()
            .map(|e| (e.code.as_str(), e.text.as_str(), e.boundary, e.weight))
            .collect();
        assert_eq!(
            e,
            vec![
                ("nihao", "你好", 0b101, 100),
                ("xian", "先", 0b1, 5),
                ("xian", "西安", 0b101, 0)
            ],
            "单个汉字 = 单音节；空权重 = 0"
        );
        assert_eq!(d.entries[0].line, 8, "行号含头部行");
    }

    /// 文件头前的说明行不参与解析，行号照常含它。
    #[test]
    fn banner_line_before_header_is_ignored() {
        let d = p(
            &format!(
                "{}\n{HEAD}你好\tni hao\t100\n",
                crate::text::banner::USER_DICT
            ),
            false,
        );
        assert_eq!(d.header.kind.as_deref(), Some("words"));
        assert!(d.header.syllable_codes && d.header.escape);
        assert_eq!(d.entries.len(), 1);
        assert_eq!((d.entries[0].text.as_str(), d.entries[0].line), ("你好", 9));
        assert_eq!(d.bad_lines, 0);
    }

    #[test]
    fn bad_lines_are_skipped_and_counted() {
        let d = p(
            &format!("{HEAD}你好\tni hao\t100\n坏\n缺码\t\t3\n权重坏\tquan\tabc\n好\thao\t1\n"),
            false,
        );
        let texts: Vec<&str> = d.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["你好", "好"]);
        assert_eq!(d.bad_lines, 3, "列不够、码空、权重不是数字各算一行");
    }

    /// 单字词的单音节边界：空格载体表达不了 0b1，但一个汉字只对应一个音节——拼音文件里码无空格、
    /// 文本恰为一个汉字 ⇒ 0b1。非汉字的单字符（emoji 等）码可能是多音节，仍为 0。
    #[test]
    fn single_han_char_without_spaces_is_one_syllable() {
        let d = p(
            &format!("{HEAD}先\txian\t5\n😊\txiaolian\t1\n西安\txian\t1\n"),
            false,
        );
        let b: Vec<u64> = d.entries.iter().map(|e| e.boundary).collect();
        assert_eq!(b, vec![0b1, 0, 0]);
        let wb = p("---\nname: wb.user\n...\n工\taaaa\t3\n", false);
        assert_eq!(wb.entries[0].boundary, 0, "非拼音文件不推断");
    }

    #[test]
    fn bom_crlf_escape_and_no_comment() {
        let raw = format!(
            "\u{feff}{}C:\\\\note\tcn\t1\r\n# no comment\r\n#井\tjing\t2\r\n",
            HEAD.replace('\n', "\r\n")
        );
        let d = p(&raw, false);
        assert!(d.bom && d.crlf);
        assert_eq!(d.entries[0].text, "C:\\note", "escape: true 时反转义");
        assert_eq!(d.entries[1].text, "#井", "no comment 之后 # 行是数据");
    }

    #[test]
    fn codetable_file_without_syllable_flag_has_no_boundaries() {
        let d = p("---\nname: wubi86.user\n...\n工人\taaww\t3\n", false);
        assert_eq!(d.entries[0].boundary, 0);
        assert!(!d.header.syllable_codes && !d.header.escape);
    }

    #[test]
    fn normalize_key_flattens_spaces_and_lowercases() {
        assert_eq!(
            normalize_key("ni hao", "你好", false),
            ("nihao".into(), 0b101, "你好".into())
        );
        assert_eq!(normalize_key("Hello", "Hello", true).0, "hello");
        assert_eq!(normalize_key("Hello", "Hello", false).0, "Hello");
    }

    #[test]
    fn broken_files_are_classified() {
        assert!(matches!(
            parse(&[0xff, 0xfe, 0x00], Path::new("x"), false),
            Err(ParseError::NotUtf8)
        ));
        assert!(matches!(
            parse(b"---\nname: [\n...\n", Path::new("x"), false),
            Err(ParseError::BadHeader(_))
        ));
        assert!(matches!(
            parse(b"just text", Path::new("x"), false),
            Err(ParseError::NoBody)
        ));
        assert!(p(HEAD, false).entries.is_empty(), "只有头 = 零条目");
    }
}
