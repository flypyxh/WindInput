//! 带分段样式的文字（设计 `docs/design/text-span-colors.md` §3）。
//!
//! 候选注释与悬停提示的文字由模板拼成，每个变量值、每段字面文字各自带一个**角色**（变量名或
//! `title` / `literal`）或内联色；UI 按当前主题把它们解析成颜色（`wind_theme::span_color`）。

use std::sync::Arc;

pub use wind_theme::InlineColor;

/// 对外契约清单：模板里能产出的**全部角色名**（变量名 + 结构角色）。
///
/// 文档站角色表与主题编辑器严格模式照它列出与校验；引擎本身不查它（主题里写了未知角色
/// 静默忽略，更新版本的主题在旧引擎上不该报错）。新增模板变量时同步加进来——守它的测试
/// 在 wind-coordinator（每个名字都要能被某个求值入口求出值）。
pub const TEXT_ROLES: &[&str] = &[
    // 候选上下文（`Coordinator::eval_var`）
    "code_hint",
    "emoji",
    "code_rev",
    "code_rev_all",
    "shuangpin",
    "pinyin",
    "chaizi",
    "chaizi_code",
    "chaizi_all",
    "chaizi_code_all",
    "dict",
    // 气泡候选上下文
    "word_code",
    "code_source",
    "debug",
    "full_text",
    "unicode_all",
    // 逐字上下文
    "readings",
    "unicode",
    "char",
    // 结构角色：模板字面文字（正文 / 段名里）
    "title",
    "literal",
];

/// 带样式文字里的一段：`[start, end)` 字节区间（落在字符边界上）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    /// 角色 = 归一后的变量名，或结构角色 `title` / `literal`；`None` = 无角色（未知变量回显）。
    pub role: Option<Arc<str>>,
    /// 在段名（气泡 label）里产出的片段：角色未配色时回落 `title` 角色，而非正文色。
    pub in_title: bool,
    /// 内联色（`$[…]{}`）。有值即优先于角色。
    pub color: Option<InlineColor>,
}

impl Span {
    fn same_style(
        &self,
        role: &Option<Arc<str>>,
        in_title: bool,
        color: &Option<InlineColor>,
    ) -> bool {
        self.role == *role && self.in_title == in_title && self.color == *color
    }
}

/// 一段文字的样式（[`StyledText::push`] 的参数、[`StyledText::style_at`] 的结果）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub role: Option<Arc<str>>,
    pub in_title: bool,
    pub color: Option<InlineColor>,
}

impl SpanStyle {
    /// 没有任何可着色的信息：无角色、无内联色（`in_title` 单独不足以着色——它只是角色的回落
    /// 方向，§3.2 未知变量回显在段名里也不回落 title）。这样的文字不建区间，按正文色。
    fn is_plain(&self) -> bool {
        self.role.is_none() && self.color.is_none()
    }
}

/// 带分段样式的文字：纯文本 + 挂在旁边的区间表。
///
/// 下游绝大多数代码只关心文字（测量、截断判定、复制、上屏、右键菜单标签），读 [`Self::as_str`]；
/// 只有画字的那一处读区间。文字与区间捆在一个类型里、`text` 私有：并列字段或公开 `text` 都会
/// 出现「文字改了、区间没跟着改」——截断、trim、吞空白都在改文字。改文字只能走这里的方法，
/// 不变量（升序、不重叠、在界内、在字符边界）只守一处。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct StyledText {
    text: String,
    /// 按 `start` 升序、互不重叠、非空。未被覆盖的文字 = 节点正文色。
    spans: Vec<Span>,
}

impl From<String> for StyledText {
    fn from(text: String) -> Self {
        Self {
            text,
            spans: Vec::new(),
        }
    }
}

impl From<&str> for StyledText {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

impl StyledText {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn into_string(self) -> String {
        self.text
    }

    /// 追加一段文字。与末尾区间同样式且相邻时合并，免得逐字追加时区间碎成一地。
    pub fn push(&mut self, s: &str, style: &SpanStyle) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(s);
        let end = self.text.len() as u32;
        if style.is_plain() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.end == start
            && last.same_style(&style.role, style.in_title, &style.color)
        {
            last.end = end;
            return;
        }
        self.spans.push(Span {
            start,
            end,
            role: style.role.clone(),
            in_title: style.in_title,
            color: style.color.clone(),
        });
    }

    /// 追加另一段带样式文字（区间按偏移并入）。
    pub fn append(&mut self, other: &StyledText) {
        let mut at = 0usize;
        for sp in &other.spans {
            let (s, e) = (sp.start as usize, sp.end as usize);
            if at < s {
                self.push(&other.text[at..s], &SpanStyle::default());
            }
            self.push(
                &other.text[s..e],
                &SpanStyle {
                    role: sp.role.clone(),
                    in_title: sp.in_title,
                    color: sp.color.clone(),
                },
            );
            at = e;
        }
        if at < other.text.len() {
            self.push(&other.text[at..], &SpanStyle::default());
        }
    }

    /// 末尾字符满足 `pred` 就删掉它（并收缩 / 删除末尾区间）。模板引擎「空变量吞掉紧邻的
    /// 一个空白」用它：吞的永远是**同一个**输出缓冲的末尾，跨不跨颜色边界都一样。
    pub fn pop_if(&mut self, pred: impl Fn(char) -> bool) -> bool {
        let Some(c) = self.text.chars().next_back() else {
            return false;
        };
        if !pred(c) {
            return false;
        }
        self.text.pop();
        let end = self.text.len() as u32;
        if let Some(last) = self.spans.last_mut()
            && last.end > end
        {
            last.end = end;
            if last.start >= last.end {
                self.spans.pop();
            }
        }
        true
    }

    /// 取 `[start, end)` 字节区间（须在字符边界上），区间裁剪并平移。
    pub fn slice(&self, start: usize, end: usize) -> StyledText {
        let (s, e) = (start as u32, end as u32);
        StyledText {
            text: self.text[start..end].to_string(),
            spans: self
                .spans
                .iter()
                .filter_map(|sp| {
                    let (a, b) = (sp.start.max(s), sp.end.min(e));
                    (a < b).then(|| Span {
                        start: a - s,
                        end: b - s,
                        ..sp.clone()
                    })
                })
                .collect(),
        }
    }

    /// 字节位置 `at` 处那个字的样式（不在任何区间内 = 默认样式）。
    pub fn style_at(&self, at: usize) -> SpanStyle {
        let at = at as u32;
        self.spans
            .iter()
            .find(|sp| sp.start <= at && at < sp.end)
            .map(|sp| SpanStyle {
                role: sp.role.clone(),
                in_title: sp.in_title,
                color: sp.color.clone(),
            })
            .unwrap_or_default()
    }

    /// 去掉首尾空白（Unicode 空白，同 `str::trim`）。
    pub fn trim(&self) -> StyledText {
        let t = self.text.trim_start();
        let start = self.text.len() - t.len();
        let end = start + t.trim_end().len();
        self.slice(start, end)
    }

    /// 按字符数截断：超过 `max_chars` 个字就只留前 `max_chars` 个，再接 `mark`。`mark` 继承被截处
    /// 前一个字的样式——它不承载信息，不单设角色。`max_chars == 0` 表示不限。
    pub fn truncate_chars(&self, max_chars: usize, mark: &str) -> StyledText {
        if max_chars == 0 {
            return self.clone();
        }
        let Some((cut, _)) = self.text.char_indices().nth(max_chars) else {
            return self.clone();
        };
        self.cut_with_mark(cut, mark)
    }

    /// 截在字节位置 `cut`（字符边界），接上继承前一个字样式的 `mark`。
    pub fn cut_with_mark(&self, cut: usize, mark: &str) -> StyledText {
        let mut out = self.slice(0, cut);
        let prev = self.text[..cut]
            .char_indices()
            .next_back()
            .map(|(i, _)| self.style_at(i))
            .unwrap_or_default();
        out.push(mark, &prev);
        out
    }

    /// 以 `sep` 连接多段。
    pub fn join(parts: &[StyledText], sep: &str) -> StyledText {
        let mut out = StyledText::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(sep, &SpanStyle::default());
            }
            out.append(p);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(r: &str) -> SpanStyle {
        SpanStyle {
            role: Some(Arc::from(r)),
            ..Default::default()
        }
    }

    fn ranges(t: &StyledText) -> Vec<(u32, u32, Option<&str>)> {
        t.spans()
            .iter()
            .map(|s| (s.start, s.end, s.role.as_deref()))
            .collect()
    }

    #[test]
    fn push_merges_adjacent_same_style_and_skips_plain() {
        let mut t = StyledText::new();
        t.push("ab", &role("pinyin"));
        t.push("c", &role("pinyin"));
        t.push("-", &SpanStyle::default());
        t.push("你", &role("chaizi"));
        assert_eq!(t.as_str(), "abc-你");
        assert_eq!(
            ranges(&t),
            vec![(0, 3, Some("pinyin")), (4, 7, Some("chaizi"))]
        );
    }

    #[test]
    fn pop_shrinks_and_drops_emptied_span() {
        let mut t = StyledText::new();
        t.push("a", &role("x"));
        t.push(" ", &role("literal"));
        assert!(t.pop_if(|c| c == ' '));
        assert_eq!(ranges(&t), vec![(0, 1, Some("x"))]);
        assert!(!t.pop_if(|c| c == ' '));
    }

    #[test]
    fn append_offsets_spans() {
        let mut a = StyledText::from("x");
        let mut b = StyledText::new();
        b.push("好", &role("char"));
        b.push("：", &SpanStyle::default());
        a.append(&b);
        assert_eq!(a.as_str(), "x好：");
        assert_eq!(ranges(&a), vec![(1, 4, Some("char"))]);
    }

    #[test]
    fn trim_and_truncate_keep_styles() {
        let mut t = StyledText::new();
        t.push("  ", &role("literal"));
        t.push("你好吗", &role("pinyin"));
        t.push(" ", &role("literal"));
        let trimmed = t.trim();
        assert_eq!(trimmed.as_str(), "你好吗");
        assert_eq!(ranges(&trimmed), vec![(0, 9, Some("pinyin"))]);
        let cut = trimmed.truncate_chars(2, "…");
        assert_eq!(cut.as_str(), "你好…");
        assert_eq!(
            ranges(&cut),
            vec![(0, 9, Some("pinyin"))],
            "… 继承前一个字的样式"
        );
        assert_eq!(trimmed.truncate_chars(3, "…"), trimmed, "没超出不动");
    }

    #[test]
    fn slice_and_style_at() {
        let mut t = StyledText::new();
        t.push("ab", &role("a"));
        t.push("cd", &role("b"));
        let s = t.slice(1, 3);
        assert_eq!(s.as_str(), "bc");
        assert_eq!(ranges(&s), vec![(0, 1, Some("a")), (1, 2, Some("b"))]);
        assert_eq!(t.style_at(2), role("b"));
        assert_eq!(StyledText::from("x").style_at(0), SpanStyle::default());
    }

    #[test]
    fn join_offsets_every_part() {
        let mut a = StyledText::new();
        a.push("[", &role("title"));
        let j = StyledText::join(&[a.clone(), a], "\n");
        assert_eq!(j.as_str(), "[\n[");
        assert_eq!(
            ranges(&j),
            vec![(0, 1, Some("title")), (2, 3, Some("title"))]
        );
    }
}
