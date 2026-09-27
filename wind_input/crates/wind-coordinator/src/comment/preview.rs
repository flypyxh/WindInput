//! 模板样例求值——设置页「预览行」的数据来源（设计 text-span-colors.md §11）。
//!
//! 设置页编辑注释模板与悬停提示段时，在输入框下方显示当前主题下的渲染效果。这里负责两件
//! 与主题无关的事：
//!
//! 1. 用**固定样例**（候选「你好」，候选专属变量给样例值）按真实的模板引擎渲染出带片段样式的
//!    文字。取样例值而不是现查词库：预览要稳定、可预期，不该随「反查索引建没建好」「这个用户装
//!    没装拆字库」而时有时无——用户在这里看的是**模板写得对不对**，不是词库里有什么。
//! 2. 诊断：未知变量名、内联色的写法与未知 `key=`。都给**模板里的字节区间**（`problems` 的坐标系
//!    是入参模板，不是输出文字；设置端要把它映射到转义后的显示文本上再标注）。
//!
//! 颜色（按当前主题求色、颜色名在当前主题里查不查得到）由 wind-webdata 做——它能加载主题，
//! 这里只交出每个内联色的写法与位置。

use super::{Template, VarRef, color_bounds, find_byte, find_group_end, role_of, utf8_len};
use std::sync::Arc;
use wind_theme::InlineColor;
use wind_ui_types::StyledText;

/// 样例候选。两个字都是 ≥ U+3400 的汉字，逐字段（`han` / `char`）各出一行。
pub const SAMPLE_TEXT: &str = "你好";

/// 模板用在哪里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateScene {
    /// 候选注释：整个模板是一个隐式可选段，trim。
    Comment,
    /// 悬停提示段名：字面文字的角色是 `title`，trim。
    TooltipLabel,
    /// 悬停提示段内容：`each` = `""` / `"han"` / `"char"`，逐字段每字一行。
    TooltipContent { each: String },
}

/// 模板里的一处问题：`[start, end)` 是**入参模板**的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateProblem {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

/// 模板里的一个内联色：`[start, end)` 是 `SPEC`（`$[` 与 `]` 之间）在入参模板里的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpec {
    pub start: usize,
    pub end: usize,
    pub color: Arc<InlineColor>,
    /// `SPEC` 里认不出的 `key=value` 的 key（前向兼容：引擎忽略它们，预览提示）。
    pub unknown_keys: Vec<String>,
}

/// 样例求值结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSample {
    /// 渲染出的文字（逐字段多行以 `\n` 连接）与片段样式。
    pub text: StyledText,
    /// 与主题无关的问题（未知变量名）。
    pub problems: Vec<TemplateProblem>,
    /// 全部内联色的写法与位置，颜色相关的诊断由调用方按主题做。
    pub colors: Vec<ColorSpec>,
}

/// 求值入口认得的变量：契约清单里的变量 + 兼容别名。
fn is_known(name: &str) -> bool {
    !matches!(name, "title" | "literal") && wind_ui_types::TEXT_ROLES.contains(&role_of(name))
}

/// 样例值。`ch` = 逐字段当前字（整段求值为 `None`）。`None` 返回值 = 未知变量名。
fn sample_value(name: &str, arg: Option<&str>, ch: Option<char>) -> Option<String> {
    // `_all` 类变量的分隔符参数：样例里用空格连接，参数换掉它。
    let joined = |parts: &[&str]| parts.join(arg.unwrap_or(" "));
    Some(match role_of(name) {
        "char" => ch.map(String::from).unwrap_or_default(),
        "readings" => {
            let all: &[&str] = match ch {
                Some('你') => &["nǐ"],
                Some('好') => &["hǎo", "hào"],
                _ => &[],
            };
            let n = arg
                .and_then(|a| a.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let take = if n == 0 { all.len() } else { n.min(all.len()) };
            all[..take].join("/")
        }
        "unicode" => ch
            .map(|c| format!("U+{:04X}", c as u32))
            .unwrap_or_default(),
        "unicode_all" => SAMPLE_TEXT
            .chars()
            .map(|c| format!("U+{:04X}", c as u32))
            .collect::<Vec<_>>()
            .join(arg.unwrap_or(" ")),
        "code_hint" => "hao".into(),
        "emoji" => "👋".into(),
        "code_rev" => "wqvb".into(),
        // 全部码位默认以 `/` 连接（同 `word_codes_in`），参数换掉它。
        "code_rev_all" => ["wq", "wqvb"].join(arg.unwrap_or("/")),
        "shuangpin" => "nihk".into(),
        "pinyin" => "nǐ hǎo".into(),
        "chaizi" => match ch {
            Some('好') => "女子".into(),
            _ => "亻尔".into(),
        },
        "chaizi_code" => match ch {
            Some('好') => "vbg".into(),
            _ => "wqiy".into(),
        },
        "chaizi_all" => joined(&["亻尔", "女子"]),
        "chaizi_code_all" => joined(&["wqiy", "vbg"]),
        "dict" => "hello".into(),
        "word_code" => "wqvb".into(),
        "code_source" => "五笔".into(),
        "debug" => "来源: 拼音".into(),
        "full_text" => "你好，世界".into(),
        _ => return None,
    })
}

/// 扫出模板里的变量引用与内联色（位置按入参模板计）。结构判定与模板引擎的 `parse` 同一套
/// 规则、同一组私有函数（`color_bounds` / `find_group_end` …），不另写一份配对逻辑。
fn scan(
    tpl: &str,
    base: usize,
    vars: &mut Vec<(usize, usize, Vec<String>)>,
    colors: &mut Vec<ColorSpec>,
) {
    let b = tpl.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'$'
            && i + 1 < b.len()
            && b[i + 1] == b'['
            && let Some((spec_end, body_end)) = color_bounds(b, i + 2)
        {
            let spec = &tpl[i + 2..spec_end];
            let unknown_keys = spec
                .split(',')
                .skip(1)
                .filter_map(|item| {
                    let key = item.split_once('=').map_or(item, |(k, _)| k).trim();
                    (key != "selected").then(|| key.to_string())
                })
                .collect();
            colors.push(ColorSpec {
                start: base + i + 2,
                end: base + spec_end,
                color: Arc::new(InlineColor::parse(spec)),
                unknown_keys,
            });
            scan(
                &tpl[spec_end + 2..body_end],
                base + spec_end + 2,
                vars,
                colors,
            );
            i = body_end + 1;
            continue;
        }
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            if let Some(end) = find_byte(b, i + 2, b'}') {
                let names = tpl[i + 2..end]
                    .split('|')
                    .map(|s| VarRef::parse(s).name)
                    .filter(|n| !n.is_empty())
                    .collect();
                vars.push((base + i, base + end + 1, names));
                i = end + 1;
                continue;
            }
        } else if b[i] == b'{'
            && let Some(end) = find_group_end(b, i + 1)
        {
            scan(&tpl[i + 1..end], base + i + 1, vars, colors);
            i = end + 1;
            continue;
        }
        i += utf8_len(b[i]);
    }
}

/// 按场景用样例求值并诊断。
pub fn sample(template: &str, scene: &TemplateScene) -> TemplateSample {
    let t = Template::parse(template);
    let whole = |name: &str, arg: Option<&str>| sample_value(name, arg, None);
    let text = match scene {
        TemplateScene::Comment => t.render_whole(0, &whole),
        TemplateScene::TooltipLabel => t.render_styled(&whole, &|_| true, true).0.trim(),
        TemplateScene::TooltipContent { each } => match each.trim() {
            "han" | "char" => {
                let han = each.trim() == "han";
                let rows: Vec<StyledText> = SAMPLE_TEXT
                    .chars()
                    .filter(|c| {
                        if han {
                            *c as u32 >= 0x3400
                        } else {
                            !c.is_whitespace()
                        }
                    })
                    .filter_map(|c| {
                        let eval = |name: &str, arg: Option<&str>| sample_value(name, arg, Some(c));
                        let (row, filled) = t.render_styled(&eval, &|n| n != "char", false);
                        filled.then_some(row)
                    })
                    .collect();
                StyledText::join(&rows, "\n")
            }
            _ => {
                let (row, filled) = t.render_styled(&whole, &|_| true, false);
                if filled { row } else { StyledText::new() }
            }
        },
    };
    let (mut vars, mut colors) = (Vec::new(), Vec::new());
    scan(template, 0, &mut vars, &mut colors);
    let problems = vars
        .into_iter()
        .filter_map(|(start, end, names)| {
            let unknown: Vec<String> = names.into_iter().filter(|n| !is_known(n)).collect();
            (!unknown.is_empty()).then(|| TemplateProblem {
                start,
                end,
                message: format!("未知变量：{}", unknown.join("、")),
            })
        })
        .collect();
    TemplateSample {
        text,
        problems,
        colors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(t: &StyledText) -> Vec<(&str, Option<&str>, bool)> {
        t.spans()
            .iter()
            .map(|s| {
                (
                    &t.as_str()[s.start as usize..s.end as usize],
                    s.role,
                    s.color.is_some(),
                )
            })
            .collect()
    }

    #[test]
    fn comment_scene_renders_sample_with_roles() {
        let s = sample(
            "${code_hint|code_rev} $[accent]{(${pinyin})}",
            &TemplateScene::Comment,
        );
        assert_eq!(s.text.as_str(), "hao (nǐ hǎo)");
        assert_eq!(
            roles(&s.text),
            vec![
                ("hao", Some("code_hint"), false),
                (" ", Some("literal"), false),
                ("(", Some("literal"), true),
                ("nǐ hǎo", Some("pinyin"), true),
                (")", Some("literal"), true),
            ]
        );
        assert!(s.problems.is_empty());
    }

    #[test]
    fn per_char_content_one_row_per_han() {
        let s = sample(
            "${char}：${readings}",
            &TemplateScene::TooltipContent { each: "han".into() },
        );
        assert_eq!(s.text.as_str(), "你：nǐ\n好：hǎo/hào");
        let one = sample(
            "${char}：${readings:1}",
            &TemplateScene::TooltipContent {
                each: "char".into(),
            },
        );
        assert_eq!(one.text.as_str(), "你：nǐ\n好：hǎo");
    }

    #[test]
    fn label_scene_marks_title() {
        let s = sample("编码{(${code_source})}", &TemplateScene::TooltipLabel);
        assert_eq!(s.text.as_str(), "编码(五笔)");
        assert!(s.text.spans().iter().all(|sp| sp.in_title));
    }

    /// 问题区间是**模板**坐标：`${pinyn}` 在模板里的位置，而不是输出里回显的位置。
    #[test]
    fn unknown_variables_are_reported_in_template_offsets() {
        let tpl = "码:{${code_hint}} ${pinyn|dict}";
        let s = sample(tpl, &TemplateScene::Comment);
        assert_eq!(s.problems.len(), 1);
        let p = &s.problems[0];
        assert_eq!(&tpl[p.start..p.end], "${pinyn|dict}");
        assert!(p.message.contains("pinyn") && !p.message.contains("dict"));
        // 兼容别名不算未知。
        assert!(
            sample("${code}${code_all}", &TemplateScene::Comment)
                .problems
                .is_empty()
        );
    }

    /// 内联色的位置（SPEC 区间，含嵌套在可选段 / 内联色里的）与未知 key。
    #[test]
    fn color_specs_with_positions_and_unknown_keys() {
        let tpl = "{a $[accent,hover=x]{b $[#GG]{c}}}";
        let s = sample(tpl, &TemplateScene::Comment);
        let got: Vec<(&str, Vec<String>)> = s
            .colors
            .iter()
            .map(|c| (&tpl[c.start..c.end], c.unknown_keys.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("accent,hover=x", vec!["hover".to_string()]),
                ("#GG", vec![])
            ]
        );
        // 结构不成立的 `$[` 不算内联色。
        assert!(
            sample("$[accent${code_hint}", &TemplateScene::Comment)
                .colors
                .is_empty()
        );
    }

    /// 样例表覆盖契约里的全部变量（否则预览会把合法变量报成未知）。
    #[test]
    fn every_listed_role_has_a_sample_value() {
        for r in wind_ui_types::TEXT_ROLES {
            if matches!(*r, "title" | "literal") {
                continue;
            }
            assert!(sample_value(r, None, Some('你')).is_some(), "{r}");
        }
    }
}
