//! 候选**悬停提示**（气泡）的段渲染——`ui.tooltip.sections` 的唯一消费点。
//!
//! 气泡 = 有序段列表，每段 = 段名模板 + 段内容模板，语法与变量沿用注释段（[`crate::comment`]），
//! 不另起一套。本模块只加两样注释段没有的东西：
//!
//! - **逐字求值**（`each = "han" | "char"`）：段内容对每个字求值一次、每字一行。拼音段、拆字段
//!   天然是逐字的；有了它，「拆字 / 拼音」合并只是一行模板，不必再像旧 `merge_chaizi_pinyin`
//!   那样在代码里按字对齐两段。
//! - **`promote`**：按某变量是否非空把逐字行稳定分成两组。合并段靠它复现旧行序（有拆字的字
//!   在前，拆字库未收录的补在末尾）。
//!
//! 变量分三层取值，先到先得：本模块的气泡专属变量（[`char_var`] / [`candidate_var`]）→
//! 调用方注入的候选上下文变量（`word_code` / `code_source` / `debug`）→ 注释段那套词汇。
//! 逐字上下文取不到的再回落候选上下文（设计 §4.1：允许但通常没有意义，不报错）。
//!
//! 解析在配置快照里做一次（[`CompiledTooltip::compile`]，随 `ConfigBundle` 重建），
//! 候选循环里只渲染。设计见 `docs/design/candidate-tooltip-sections.md`。

use crate::comment::Template;
use tracing::warn;
use wind_config::config::TooltipSection as SectionConfig;
use wind_ui_types::{TooltipDoc, TooltipLine, TooltipSection};

/// 逐字段遍历的「汉字」口径：≥ U+3400（扩展 A 起）。与段列表引入前的气泡一致。
fn is_han(c: char) -> bool {
    c as u32 >= 0x3400
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Each {
    /// 整个候选求值一次。
    Whole,
    /// 显示文本里每个 ≥U+3400 的字。
    Han,
    /// 显示文本里每个非空白字符。
    Char,
}

#[derive(Debug, Clone)]
struct CompiledSection {
    label: Template,
    template: Template,
    each: Each,
    promote: Option<String>,
    inline: bool,
}

/// 预解析的段列表（只含启用的段）。
#[derive(Debug, Clone, Default)]
pub(crate) struct CompiledTooltip {
    sections: Vec<CompiledSection>,
}

impl CompiledTooltip {
    pub(crate) fn compile(cfg: &[SectionConfig]) -> Self {
        let sections = cfg
            .iter()
            .filter(|s| s.enabled)
            .map(|s| CompiledSection {
                label: Template::parse(&s.label),
                template: Template::parse(&s.template),
                each: match s.each.trim() {
                    "" => Each::Whole,
                    "han" => Each::Han,
                    "char" => Each::Char,
                    // 写错不静默：按整段求值照常出内容，但日志里留下线索。
                    other => {
                        warn!("ui.tooltip.sections: 未知的 each = {other:?}，按整段求值处理");
                        Each::Whole
                    }
                },
                promote: Some(s.promote.trim())
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
                inline: s.inline,
            })
            .collect();
        Self { sections }
    }

    /// 启用的段里是否引用了变量 `name`（段名、内容、`promote` 任一处）。调用方据此决定要不要
    /// 准备代价高的数据：调试上下文、编码反查索引——没有段用到就一次也不碰。
    pub(crate) fn references(&self, name: &str) -> bool {
        self.sections.iter().any(|s| {
            s.label.references(name)
                || s.template.references(name)
                || s.promote.as_deref() == Some(name)
        })
    }

    /// 按段列表渲染一个候选的气泡。
    ///
    /// - `disp`：候选的**显示文本**（截断后）。逐字段只遍历它，规模因此受 `ui.candidate.max_chars` 控制。
    /// - `cand`：候选上下文求值，`None` = 未知变量名（渲染层原样回显，拼错看得见）。
    /// - `per_char`：逐字上下文求值；返回 `None` 时回落 `cand`。
    ///
    /// ★ 显示文本里一个 ≥U+3400 的字都没有时，除 `${debug}` 外的变量一律按空处理——即非汉字
    /// 候选只可能出调试段。这是段列表引入前气泡的口径（`tooltip_for` 先滤 CJK、滤空即返回空，
    /// 调试段在外面另行追加），迁移后外观不变靠它。
    pub(crate) fn render(
        &self,
        disp: &str,
        cand: &impl Fn(&str, Option<&str>) -> Option<String>,
        per_char: &impl Fn(char, &str, Option<&str>) -> Option<String>,
    ) -> TooltipDoc {
        let has_han = disp.chars().any(is_han);
        let gate = |name: &str, v: Option<String>| -> Option<String> {
            if has_han || name == "debug" {
                v
            } else {
                v.map(|_| String::new())
            }
        };
        let cand_eval = |name: &str, arg: Option<&str>| gate(name, cand(name, arg));
        let mut doc = TooltipDoc::default();
        for sec in &self.sections {
            let rows = match sec.each {
                Each::Whole => {
                    let (text, filled) = sec.template.render(&cand_eval, &|_| true);
                    if filled { vec![text] } else { Vec::new() }
                }
                Each::Han | Each::Char => {
                    let chars = disp.chars().filter(|&c| match sec.each {
                        Each::Han => is_han(c),
                        _ => !c.is_whitespace(),
                    });
                    let mut rows: Vec<(bool, String)> = Vec::new();
                    for c in chars {
                        let eval = |name: &str, arg: Option<&str>| {
                            gate(name, per_char(c, name, arg).or_else(|| cand(name, arg)))
                        };
                        // `${char}` 恒非空，不能让它撑起一行：查不到读音的字不该剩下 `好：`。
                        let (text, filled) = sec.template.render(&eval, &|n| n != "char");
                        if !filled {
                            continue;
                        }
                        let promoted = sec
                            .promote
                            .as_deref()
                            .is_some_and(|p| eval(p, None).is_some_and(|v| !v.is_empty()));
                        rows.push((promoted, text));
                    }
                    // 稳定分组：非空组在前，组内保持原文顺序。
                    if sec.promote.is_some() {
                        rows.sort_by_key(|(promoted, _)| !promoted);
                    }
                    rows.into_iter().map(|(_, text)| text).collect()
                }
            };
            let lines: Vec<TooltipLine> = rows
                .iter()
                .flat_map(|r| r.split('\n'))
                .filter(|l| !l.trim().is_empty())
                .enumerate()
                .map(|(i, l)| TooltipLine {
                    text: l.to_string(),
                    raw: u16::try_from(i).unwrap_or(u16::MAX),
                })
                .collect();
            if lines.is_empty() {
                continue;
            }
            // 段名的字面文字不随变量全空而消失：`编码{(${code_source})}` 直接输入时就是 `编码`。
            let (title, _) = sec.label.render(&cand_eval, &|_| true);
            doc.sections.push(TooltipSection {
                title: (!title.is_empty()).then_some(title),
                inline: sec.inline,
                lines,
            });
        }
        doc
    }
}

/// 逐字上下文里气泡专属的变量。`None` = 不是这里的变量（交给下一层）。
///
/// - `readings[:N]` —— 该字全部读音（最常用在前）以 `/` 连接，`N` 限前 N 个。取代旧
///   `pinyin_heteronyms`（= `:1`）与 `pinyin_max_readings`（= `:N`）。`N` 不是正整数时按不限。
/// - `unicode` —— 码位，`U+597D`；非 BMP 照写五 / 六位（`U+2A6D6`）。
pub(crate) fn char_var(
    name: &str,
    arg: Option<&str>,
    c: char,
    reverse: &wind_reverse::ReverseLookup,
) -> Option<String> {
    Some(match name {
        "readings" => {
            let max = arg
                .and_then(|a| a.trim().parse::<usize>().ok())
                .unwrap_or(0);
            reverse.readings_of(c, max, "/")
        }
        "unicode" => unicode_of(c),
        _ => return None,
    })
}

/// 候选上下文里气泡专属、且只依赖显示文本的变量。`None` = 不是这里的变量。
///
/// - `unicode_all[:分隔符]` —— 显示文本逐字码位（跳过空白），默认空格连接。
pub(crate) fn candidate_var(name: &str, arg: Option<&str>, disp: &str) -> Option<String> {
    Some(match name {
        "unicode_all" => disp
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(unicode_of)
            .collect::<Vec<_>>()
            .join(arg.unwrap_or(" ")),
        _ => return None,
    })
}

fn unicode_of(c: char) -> String {
    format!("U+{:04X}", c as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wind_config::config::{LegacyTooltipFlags, tooltip_sections_from_legacy};
    use wind_reverse::ReverseLookup;

    // ───────────────────────── 夹具 ─────────────────────────

    /// 夹具反查表。每个字的设定都对应旧实现的一条分支：
    ///
    /// | 字 | 拆字 | 读音 | 用途 |
    /// |---|---|---|---|
    /// | 好 | 女子 [vbg] | hǎo/hào | 多音字、合并行 |
    /// | 重 | 丿一日一土 [tgjf] | zhòng/chóng/tóng | 多音字，读音数截断 |
    /// | 你 | —— | nǐ | 拆字库未收录：合并段里补在末尾 |
    /// | 人 | 人 [] | rén | 有字根无编码：`[编码]` 可选段消失 |
    /// | 㐀 | 丿一 [tgd] | —— | 无拼音有拆字：合并行不带 `\t` |
    /// | 𠀀 | 一丨 [ghk] | hē | 扩展 B（代理对） |
    /// | 龘 | —— | —— | 两表都没有 |
    fn fixture_reverse() -> ReverseLookup {
        // 每次调用独占一个目录：测试并行跑，共用目录会被别的用例的 remove_dir_all 删掉。
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("wind-tooltip-fixture-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let chaizi = dir.join("chaizi.txt");
        std::fs::write(
            &chaizi,
            "好\t女子\tvbg\n重\t丿一日一土\ttgjf\n人\t人\t\n㐀\t丿一\ttgd\n𠀀\t一丨\tghk\n",
        )
        .unwrap();
        let pinyin = dir.join("pinyin_map.txt");
        std::fs::write(
            &pinyin,
            "U+597D: hǎo,hào  # 好\nU+91CD: zhòng,chóng,tóng  # 重\nU+4F60: nǐ  # 你\n\
             U+4EBA: rén  # 人\nU+20000: hē  # 𠀀\n",
        )
        .unwrap();
        let rl = ReverseLookup::load(Some(&pinyin), Some(&chaizi));
        let _ = std::fs::remove_dir_all(&dir);
        rl
    }

    /// 生产同一份的逐字求值（去掉只有协调器才有的引擎类变量，对拍的段列表用不到它们）。
    fn per_char(rl: &ReverseLookup) -> impl Fn(char, &str, Option<&str>) -> Option<String> + '_ {
        move |c, name, arg| {
            char_var(name, arg, c, rl)
                .or_else(|| crate::comment::reverse_text_var(name, arg, &c.to_string(), rl))
        }
    }

    struct Cand<'a> {
        disp: &'a str,
        word_code: Option<&'a str>,
        code_source: Option<&'a str>,
        debug: &'a str,
    }

    fn cand_eval<'a>(c: &'a Cand<'a>) -> impl Fn(&str, Option<&str>) -> Option<String> + 'a {
        move |name, arg| {
            Some(match name {
                "word_code" => c.word_code.unwrap_or_default().to_string(),
                "code_source" => c.code_source.unwrap_or_default().to_string(),
                "debug" => c.debug.to_string(),
                _ => return candidate_var(name, arg, c.disp),
            })
        }
    }

    fn render_new(rl: &ReverseLookup, sections: &[SectionConfig], c: &Cand) -> String {
        CompiledTooltip::compile(sections)
            .render(c.disp, &cand_eval(c), &per_char(rl))
            .to_plain_text()
    }

    // ───────────────── 参照实现：段列表引入前的气泡 ─────────────────

    /// 旧 `ReverseLookup::tooltip_for` + `merge_chaizi_pinyin` + 协调器追加调试段，逐行照搬
    /// （只把私有表访问换成等价的公开方法）。**对拍的基准，不要「顺手修」它。**
    fn legacy(rl: &ReverseLookup, f: LegacyTooltipFlags, c: &Cand) -> String {
        struct Sec {
            label: String,
            lines: Vec<String>,
        }
        let mut tooltip = (|| {
            if rl.is_empty() && c.word_code.is_none() {
                return String::new();
            }
            let chars: Vec<char> = c.disp.chars().filter(|c| (*c as u32) >= 0x3400).collect();
            if chars.is_empty() {
                return String::new();
            }
            let mut sections: Vec<Sec> = Vec::new();
            if f.code
                && let Some(code) = c.word_code.filter(|c| !c.is_empty())
            {
                let label = match c.code_source.filter(|s| !s.is_empty()) {
                    Some(src) => format!("编码({src})"),
                    None => "编码".to_string(),
                };
                sections.push(Sec {
                    label,
                    lines: vec![code.to_string()],
                });
            }
            if f.pinyin {
                let mut lines = Vec::new();
                for &ch in &chars {
                    let all = rl.readings_of(ch, 0, "/");
                    if all.is_empty() {
                        continue;
                    }
                    let len = all.split('/').count();
                    let n = if !f.heteronyms {
                        1
                    } else if f.max_readings > 0 {
                        f.max_readings.min(len)
                    } else {
                        len
                    };
                    let shown = rl.readings_of(ch, n, "/");
                    if !shown.is_empty() {
                        lines.push(format!("{ch}：{shown}"));
                    }
                }
                if !lines.is_empty() {
                    sections.push(Sec {
                        label: "拼音".into(),
                        lines,
                    });
                }
            }
            if f.chaizi {
                let mut lines = Vec::new();
                for &ch in &chars {
                    let s = ch.to_string();
                    let rad = rl.radicals_of(&s, "");
                    if rad.is_empty() {
                        continue;
                    }
                    let code = rl.chaizi_code_of(&s);
                    lines.push(if code.is_empty() {
                        format!("{ch}：{rad}")
                    } else {
                        format!("{ch}：{rad} [{code}]")
                    });
                }
                if !lines.is_empty() {
                    sections.push(Sec {
                        label: "拆字".into(),
                        lines,
                    });
                }
            }
            // merge_chaizi_pinyin
            let ci = sections.iter().position(|s| s.label == "拆字");
            let pi = sections.iter().position(|s| s.label == "拼音");
            if let (Some(ci), Some(pi)) = (ci, pi) {
                let mut pin_map = std::collections::HashMap::new();
                let mut pin_full = std::collections::HashMap::new();
                let mut pin_order = Vec::new();
                for line in &sections[pi].lines {
                    let head = line.chars().next().unwrap();
                    pin_full.insert(head, line.clone());
                    let reading = line
                        .find('：')
                        .map(|i| line[i + '：'.len_utf8()..].to_string())
                        .unwrap_or_else(|| line.clone());
                    pin_map.insert(head, reading);
                    pin_order.push(head);
                }
                let mut used = std::collections::HashSet::new();
                let mut merged = Vec::new();
                for cz in &sections[ci].lines {
                    match cz.chars().next() {
                        Some(h) if pin_map.contains_key(&h) => {
                            used.insert(h);
                            merged.push(format!("{}\t{}", cz, pin_map[&h]));
                        }
                        _ => merged.push(cz.clone()),
                    }
                }
                for r in &pin_order {
                    if !used.contains(r) {
                        merged.push(pin_full[r].clone());
                    }
                }
                let combined = Sec {
                    label: "拆字 / 拼音".into(),
                    lines: merged,
                };
                let mut combined = Some(combined);
                sections = sections
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i != pi)
                    .map(|(i, s)| if i == ci { combined.take().unwrap() } else { s })
                    .collect();
            }
            // format_sections（旧气泡的段全是 always_expand）
            let mut parts = Vec::new();
            for sec in sections {
                parts.push(format!("[{}]", sec.label));
                parts.extend(sec.lines);
            }
            parts.join("\n")
        })();
        if f.debug {
            if !tooltip.is_empty() {
                tooltip.push('\n');
            }
            tooltip.push_str(&format!("[调试]\n{}", c.debug));
        }
        tooltip
    }

    // ───────────────────────── 对拍 ─────────────────────────

    fn all_flags() -> Vec<LegacyTooltipFlags> {
        let mut out = Vec::new();
        for bits in 0..32u32 {
            for max_readings in [0, 1, 2] {
                out.push(LegacyTooltipFlags {
                    code: bits & 1 != 0,
                    pinyin: bits & 2 != 0,
                    heteronyms: bits & 4 != 0,
                    chaizi: bits & 8 != 0,
                    debug: bits & 16 != 0,
                    max_readings,
                });
            }
        }
        out
    }

    const DEBUG: &str = "来源: 码表·五笔\n码 vbg · 权 100 · 序 0 · 用 0次";

    fn fixtures() -> Vec<Cand<'static>> {
        let c = |disp, word_code, code_source| Cand {
            disp,
            word_code,
            code_source,
            debug: DEBUG,
        };
        vec![
            c("好", Some("vbg"), None),
            c("好", Some("v/vb/vbg"), Some("五笔")),
            c("重要", Some("tgsv"), None),
            c("你好", Some("wqvb"), Some("五笔")), // 你无拆字 ⇒ 合并段里排到好之后
            c("你你好人", None, None),             // 重复字 + 有字根无编码
            c("好好", Some("vbvb"), None),
            c("人", None, None),
            c("㐀", None, None), // 无拼音有拆字
            c("𠀀", Some("ghk"), None),
            c("好𠀀你", None, Some("五笔")),
            c("龘", Some("xyz"), None), // 两表都没有，只剩编码
            c("好a人", Some("x"), None),
            c("你好…", None, None),      // 截断后的显示文本
            c("abc", Some("abc"), None), // 纯非 CJK：只可能有调试段
            c("", None, None),
        ]
    }

    /// 旧开关只有一边有内容时，旧实现不合并、保留那一边的标题（`[拼音]` / `[拆字]`），
    /// 而迁移出的合并段标题恒为「拆字 / 拼音」。内容行逐字节一致，差异只在这一行标题。
    fn expected(old: &str, f: LegacyTooltipFlags) -> String {
        if f.chaizi && f.pinyin {
            old.replacen("[拼音]\n", "[拆字 / 拼音]\n", 1).replacen(
                "[拆字]\n",
                "[拆字 / 拼音]\n",
                1,
            )
        } else {
            old.to_string()
        }
    }

    /// ★★★ 验收核心（设计 §8.2）：全部旧开关组合 × 夹具，「旧开关 → 迁移出的段列表 →
    /// 新渲染」与旧实现逐字节相同（唯一例外见 [`expected`]，每一例都单独计数并核对）。
    #[test]
    fn migrated_sections_reproduce_legacy_tooltip_byte_for_byte() {
        let rl = fixture_reverse();
        let mut checked = 0;
        let mut title_only = 0;
        for f in all_flags() {
            let sections = tooltip_sections_from_legacy(f);
            for c in fixtures() {
                let old = legacy(&rl, f, &c);
                let new = render_new(&rl, &sections, &c);
                let want = expected(&old, f);
                assert_eq!(
                    new, want,
                    "\n开关 {f:?}\n候选 {:?} word_code={:?} code_source={:?}\n旧输出:\n{old}\n",
                    c.disp, c.word_code, c.code_source
                );
                checked += 1;
                if want != old {
                    title_only += 1;
                }
            }
        }
        assert_eq!(checked, 96 * fixtures().len());
        // 标题差异只出现在「拆字、拼音同开」且某一边整段为空的组合上；夹具里有这种候选
        // （㐀 无读音、你/龘 无拆字……），数目为零说明夹具退化了，对拍不再覆盖那条分支。
        assert!(title_only > 0);
    }

    /// 行序专项（用户明确要求不变）：「你好」里「你」无拆字，旧实现先出「好」再补「你」。
    #[test]
    fn merged_section_keeps_legacy_row_order() {
        let rl = fixture_reverse();
        let f = LegacyTooltipFlags {
            chaizi: true,
            ..Default::default()
        };
        let c = Cand {
            disp: "你好人㐀",
            word_code: None,
            code_source: None,
            debug: "",
        };
        assert_eq!(
            render_new(&rl, &tooltip_sections_from_legacy(f), &c),
            "[拆字 / 拼音]\n好：女子 [vbg]\thǎo/hào\n人：人\trén\n㐀：丿一 [tgd]\n你：nǐ"
        );
    }

    // ───────────────────────── 新变量与段语义 ─────────────────────────

    fn section(label: &str, each: &str, template: &str) -> SectionConfig {
        SectionConfig {
            enabled: true,
            label: label.into(),
            template: template.into(),
            each: each.into(),
            promote: String::new(),
            inline: false,
        }
    }

    fn cand(disp: &str) -> Cand<'_> {
        Cand {
            disp,
            word_code: Some("vbg"),
            code_source: Some("五笔"),
            debug: "来源: 拼音",
        }
    }

    #[test]
    fn unicode_vars_cover_bmp_and_astral() {
        let rl = ReverseLookup::default();
        let s = [section("Unicode", "char", "${char}：${unicode}")];
        assert_eq!(
            render_new(&rl, &s, &cand("好 𠀀")),
            "[Unicode]\n好：U+597D\n𠀀：U+20000",
            "非 BMP 照写五位；空白字符不出行"
        );
        let all = [
            section("码位", "", "${unicode_all}"),
            section("", "", "${unicode_all:,}"),
        ];
        assert_eq!(
            render_new(&rl, &all, &cand("好𠀀")),
            "[码位]\nU+597D U+20000\nU+597D,U+20000"
        );
    }

    #[test]
    fn readings_arg_limits_count_and_bad_arg_means_all() {
        let rl = fixture_reverse();
        let one = |t: &str| render_new(&rl, &[section("", "han", t)], &cand("重"));
        assert_eq!(one("${readings}"), "zhòng/chóng/tóng");
        assert_eq!(one("${readings:2}"), "zhòng/chóng");
        assert_eq!(one("${readings:x}"), "zhòng/chóng/tóng");
    }

    #[test]
    fn word_code_and_code_source_feed_label_and_content() {
        let rl = ReverseLookup::default();
        let s = [section("编码{(${code_source})}", "", "${word_code}")];
        assert_eq!(render_new(&rl, &s, &cand("好")), "[编码(五笔)]\nvbg");
        let direct = Cand {
            code_source: None,
            ..cand("好")
        };
        assert_eq!(
            render_new(&rl, &s, &direct),
            "[编码]\nvbg",
            "段名的字面文字不随变量全空而消失"
        );
    }

    /// `${char}` 不计入「有值」：一行里除它以外全空即丢行；整段无行即整段不显示。
    #[test]
    fn char_alone_does_not_keep_a_row_or_section() {
        let rl = fixture_reverse();
        let s = [section("拼音", "han", "${char}：${readings}")];
        assert_eq!(render_new(&rl, &s, &cand("龘好")), "[拼音]\n好：hǎo/hào");
        assert_eq!(render_new(&rl, &s, &cand("龘")), "");
    }

    #[test]
    fn debug_var_is_multiline_and_survives_non_cjk_gate() {
        let rl = ReverseLookup::default();
        let s = [
            section("编码", "", "${word_code}"),
            section("调试", "", "${debug}"),
        ];
        let c = Cand {
            debug: "来源: 英文\n码 abc",
            ..cand("abc")
        };
        assert_eq!(render_new(&rl, &s, &c), "[调试]\n来源: 英文\n码 abc");
    }

    #[test]
    fn inline_applies_only_to_single_line_sections() {
        let rl = fixture_reverse();
        let mut s = section("Unicode", "char", "${unicode}");
        s.inline = true;
        assert_eq!(
            render_new(&rl, &[s.clone()], &cand("好")),
            "Unicode: U+597D"
        );
        assert_eq!(
            render_new(&rl, &[s], &cand("好人")),
            "[Unicode]\nU+597D\nU+4EBA"
        );
    }

    #[test]
    fn promote_is_stable_and_generic() {
        let rl = fixture_reverse();
        let mut s = section("", "han", "${char}${readings}");
        s.promote = "chaizi".into();
        // 有拆字的（好、人）按原文序在前，没有的（你、龘→无行）随后，组内顺序不变。
        assert_eq!(
            render_new(&rl, &[s], &cand("你好龘人")),
            "好hǎo/hào\n人rén\n你nǐ"
        );
    }

    #[test]
    fn disabled_sections_and_unknown_each() {
        let rl = ReverseLookup::default();
        let mut off = section("关", "", "${word_code}");
        off.enabled = false;
        let odd = section("怪", "chars", "${word_code}");
        assert_eq!(render_new(&rl, &[off, odd], &cand("好")), "[怪]\nvbg");
    }

    #[test]
    fn references_sees_label_template_and_promote() {
        let mut s = section("编码{(${code_source})}", "han", "${a|debug}");
        s.promote = "chaizi".into();
        let t = CompiledTooltip::compile(&[s]);
        assert!(t.references("code_source"));
        assert!(t.references("debug"));
        assert!(t.references("chaizi"));
        assert!(!t.references("word_code"));
        let mut off = section("", "", "${debug}");
        off.enabled = false;
        assert!(
            !CompiledTooltip::compile(&[off]).references("debug"),
            "关着的段不算"
        );
    }
}
