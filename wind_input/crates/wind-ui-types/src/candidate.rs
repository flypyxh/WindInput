//! 候选词条数据。

/// 候选词数据
#[derive(Debug, Clone)]
pub struct CandidateItem {
    pub text: String,
    pub code: String,
    /// 序号标签（如 "1" / "a"）；空则按位置自动用数字编号
    pub label: String,
    /// 悬停提示（结构化，段 → 显示行）。空文档 = 不显示气泡。
    pub tooltip: TooltipDoc,
    /// 候选注释（编码后缀/短语提示等），非空时在候选词右侧以注释样式内联显示；空则不显示
    pub comment: String,
    /// 为 true 时完全不渲染序号节点（用于非候选的提示行，如快捷加词预览），
    /// 避免默认主题下出现空的序号圆圈。
    pub no_index: bool,
}

/// 结构化的悬停提示：有序段列表。纯文本形态见 [`TooltipDoc::to_plain_text`]。
///
/// 段结构一路保留到 UI，右键才有东西可命中（按段 / 按行复制、上屏）：渲染端按
/// [`Self::to_plain_text`] 画出整块文本，右键时用 [`Self::hit_at_line`] 把点中的那一行
/// 换算回 `(段, 原始行)`。只下发**显示行**；原始行（复制 / 上屏的取值）留在协调器。
/// 设计见 `docs/design/candidate-tooltip-sections.md` §6、§7。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TooltipDoc {
    pub sections: Vec<TooltipSection>,
}

/// 气泡的一段。只有非空段才会进 [`TooltipDoc`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipSection {
    /// 已求值的段名；`None` = 无标题行。
    pub title: Option<String>,
    /// 渲染成 `标题: 第一条显示行`（其余显示行照常另起），而非标题独占一行。
    ///
    /// 由协调器按**原始内容恰为一行**判定后置位：一条长内容折成多条显示行仍算一行，
    /// 故这里不再看 `lines.len()`。
    pub inline: bool,
    /// 非空。
    pub lines: Vec<TooltipLine>,
}

/// 气泡的一条显示行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooltipLine {
    pub text: String,
    /// 所属原始行在本段中的下标。现在一条原始行恰对应一条显示行；将来单行截断 / 折行
    /// 会把一条原始行拆成多条显示行，它们指向同一个下标，复制 / 上屏据此取回原文。
    pub raw: u16,
}

/// 右键点中气泡的位置：第几段（`sections` 下标），以及该段的第几条**原始行**
/// （点在标题行上为 `None`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooltipHit {
    pub section: u16,
    pub raw_line: Option<u16>,
}

impl TooltipDoc {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// 纯文本的逐行形态，每行带上它命中的位置。[`Self::to_plain_text`] 与
    /// [`Self::hit_at_line`] 都从这里取——画出来的行与命中换算的行必须是同一份排列。
    ///
    /// inline 段的首行既是标题也是内容，按内容算（命中该段第一条原始行）。
    fn plain_lines(&self) -> Vec<(String, TooltipHit)> {
        let mut out = Vec::new();
        for (si, sec) in self.sections.iter().enumerate() {
            let section = u16::try_from(si).unwrap_or(u16::MAX);
            let hit = |raw| TooltipHit {
                section,
                raw_line: raw,
            };
            match (&sec.title, sec.lines.as_slice()) {
                (Some(t), [first, rest @ ..]) if sec.inline => {
                    out.push((format!("{t}: {}", first.text), hit(Some(first.raw))));
                    out.extend(rest.iter().map(|l| (l.text.clone(), hit(Some(l.raw)))));
                }
                (title, lines) => {
                    if let Some(t) = title {
                        out.push((format!("[{t}]"), hit(None)));
                    }
                    out.extend(lines.iter().map(|l| (l.text.clone(), hit(Some(l.raw)))));
                }
            }
        }
        out
    }

    /// 纯文本第 `line` 行（从 0 起，按 `\n` 切）命中哪里；越界为 `None`。
    pub fn hit_at_line(&self, line: usize) -> Option<TooltipHit> {
        self.plain_lines().get(line).map(|(_, h)| *h)
    }

    /// 纯文本形态：段间换行；有标题的段写成 `[标题]` 独占一行再逐行列内容，
    /// `inline` 段写成 `标题: 第一行`、其余行照列；无标题的段只列内容。
    ///
    /// 这是气泡、「复制全部」与 macOS 下发共用的格式，与段列表引入前的输出逐字节相同。
    pub fn to_plain_text(&self) -> String {
        let lines: Vec<String> = self.plain_lines().into_iter().map(|(t, _)| t).collect();
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(title: Option<&str>, inline: bool, lines: &[&str]) -> TooltipSection {
        TooltipSection {
            title: title.map(str::to_string),
            inline,
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, t)| TooltipLine {
                    text: t.to_string(),
                    raw: i as u16,
                })
                .collect(),
        }
    }

    #[test]
    fn plain_text_formats_titles_inline_and_untitled_sections() {
        let doc = TooltipDoc {
            sections: vec![
                sec(Some("编码"), false, &["vbg"]),
                sec(Some("Unicode"), true, &["U+597D"]),
                sec(Some("拼音"), false, &["好：hǎo", "人：rén"]),
                sec(Some("原文"), true, &["折成", "两行"]),
                sec(None, false, &["无标题"]),
            ],
        };
        assert_eq!(
            doc.to_plain_text(),
            "[编码]\nvbg\nUnicode: U+597D\n[拼音]\n好：hǎo\n人：rén\n原文: 折成\n两行\n无标题",
            "inline 段标题接第一条显示行，其余显示行照常另起"
        );
        assert_eq!(TooltipDoc::default().to_plain_text(), "");
    }

    /// 命中换算与纯文本同一份排列：标题行 → raw None；inline 首行 → 第一条原始行；
    /// 折行产生的多条显示行 → 同一条原始行。
    #[test]
    fn hit_at_line_follows_plain_text_layout() {
        let mut wrapped = sec(Some("原文"), false, &["折成", "两行"]);
        wrapped.lines[1].raw = 0;
        let doc = TooltipDoc {
            sections: vec![
                sec(Some("编码"), false, &["vbg"]),
                sec(Some("Unicode"), true, &["U+597D"]),
                wrapped,
            ],
        };
        let h = |section, raw_line| Some(TooltipHit { section, raw_line });
        assert_eq!(doc.hit_at_line(0), h(0, None));
        assert_eq!(doc.hit_at_line(1), h(0, Some(0)));
        assert_eq!(doc.hit_at_line(2), h(1, Some(0)));
        assert_eq!(doc.hit_at_line(3), h(2, None));
        assert_eq!(doc.hit_at_line(4), h(2, Some(0)));
        assert_eq!(
            doc.hit_at_line(5),
            h(2, Some(0)),
            "折行的第二条显示行指回同一原始行"
        );
        assert_eq!(doc.hit_at_line(6), None);
        assert_eq!(doc.to_plain_text().lines().count(), 6);
    }
}
