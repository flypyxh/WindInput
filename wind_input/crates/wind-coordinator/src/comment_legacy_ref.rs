//! 模板引擎**改动前**的逐字副本（分段着色 P2 之前，`comment.rs` 的 `VarRef` … `render`），
//! 只作对拍参照。
//!
//! 设计 text-span-colors.md §13.2 P2：模板引擎改成片段构建器后，纯文本输出必须与改动前逐字节
//! 相同。参照必须来自改动前的代码——拿新代码自己和自己比证明不了任何事——故整段（解析、
//! 配对、渲染）原样复制进来、自成一体，不引用 `comment.rs` 里任何会随改动变化的东西。
//! ⛔ 不要「顺手」把这里跟着新代码改：它的价值全在于不变。

#![allow(dead_code)]

/// 一次变量引用：名字 + 可选参数（`${chaizi_all:／}` 的 `／`）。
///
/// 参数**不 trim**，名字才 trim：`${chaizi_all: · }` 里那两个空格正是用户要的分隔符，
/// 削掉它就没法配出「亻尔 · 女子」。而 `${ pinyin }` 这种手滑仍要认。
#[derive(Debug, Clone, PartialEq, Eq)]
struct VarRef {
    name: String,
    arg: Option<String>,
}

impl VarRef {
    /// 解析 `name` 或 `name:arg`。只切**第一个**冒号——分隔符本身可以含冒号。
    fn parse(s: &str) -> Self {
        match s.split_once(':') {
            Some((n, a)) => Self {
                name: n.trim().to_string(),
                arg: Some(a.to_string()),
            },
            None => Self {
                name: s.trim().to_string(),
                arg: None,
            },
        }
    }
}

/// 模板节点。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// 字面文本。
    Text(String),
    /// `${a|b}`：按序取首个非空变量。
    Var(Vec<VarRef>),
    /// `{ … }`：段内变量全空则整段消失。可嵌套：`{${a}{ [${b}]}\t}` 里内段只管 `b`，
    /// 外段在 `a`、`b` 任一非空时保留（悬停提示「拆字 / 拼音」合并段就靠这个表达）。
    Group(Vec<Node>),
}

/// 解析模板。**不会失败**——未闭合的 `${` / `{` 一律退化为字面文本。
///
/// 宽容而非报错，是因为这个字符串由用户在设置页手打，且它的产物直接显示在候选栏里：
/// 语法写错时让他看到自己打的原文（`${pinyn}` 原样出现），比弹一个错误对话框或者静默
/// 变空更容易自己改对。
fn parse(tpl: &str) -> Vec<Node> {
    let b = tpl.as_bytes();
    let mut nodes = Vec::new();
    let mut text = String::new();
    let mut i = 0usize;
    while i < b.len() {
        // `${a|b}` —— 先于裸 `{` 判定，否则变量的 `{` 会被当成段起点。
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            if let Some(end) = find_byte(b, i + 2, b'}') {
                if !text.is_empty() {
                    nodes.push(Node::Text(std::mem::take(&mut text)));
                }
                nodes.push(Node::Var(
                    tpl[i + 2..end]
                        .split('|')
                        .map(VarRef::parse)
                        .filter(|v| !v.name.is_empty())
                        .collect(),
                ));
                i = end + 1;
                continue;
            }
            // 未闭合 → 后面全是字面文本。
        } else if b[i] == b'{' {
            // 段内容的扫描必须跳过 `${…}`，否则变量的 `}` 会被误当作段结束。
            if let Some(end) = find_group_end(b, i + 1) {
                if !text.is_empty() {
                    nodes.push(Node::Text(std::mem::take(&mut text)));
                }
                // 内段由递归解析处理：`find_group_end` 已按层数配对，切出来的段内文本是平衡的。
                nodes.push(Node::Group(parse(&tpl[i + 1..end])));
                i = end + 1;
                continue;
            }
        }
        // 按字符推进，保证切片落在 UTF-8 边界上（模板含中文标签文字）。
        let ch_len = utf8_len(b[i]);
        text.push_str(&tpl[i..(i + ch_len).min(tpl.len())]);
        i += ch_len;
    }
    if !text.is_empty() {
        nodes.push(Node::Text(text));
    }
    nodes
}

/// UTF-8 首字节 → 该字符的字节数（非法首字节按 1 处理，与解析的宽容取向一致）。
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

fn find_byte(b: &[u8], from: usize, target: u8) -> Option<usize> {
    (from..b.len()).find(|&i| b[i] == target)
}

/// 找可选段的结束 `}`，**跳过内部的 `${…}`**、按层数配对内段。未闭合返回 `None`。
///
/// 曾经不配对（段内第一个 `}` 即结束），于是 `{${a}{ [${b}]}\t}` 会被切成段 `${a}{ [${b}]`
/// 加字面 `\t}`——内段吞掉了外段的右括号。悬停提示合并段的模板要嵌套才表达得出来。
fn find_group_end(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut depth = 0usize;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            // 变量未闭合 ⇒ 段也无从闭合（`?` 即 return None）。
            i = find_byte(b, i + 2, b'}')? + 1;
            continue;
        }
        match b[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// 渲染结果：文本 + 「本段里出现过非空变量吗」。
///
/// 后者是可选段与顶层的存废依据，**必须与文本分开返回**：一个段可能渲染出非空文本
/// （字面装饰字符）却一个变量都没填上，那正是要整段丢弃的情形（`(拼: )`）。
struct Rendered {
    text: String,
    any_var_filled: bool,
}

/// 渲染节点序列。`eval` 按变量名求值：`None` = **未知变量名**，`Some("")` = 已知但为空。
///
/// 两者刻意区分：未知变量名原样输出 `${name}` 并**计作已填充**，于是拼错的变量名一定会
/// 显示在候选栏里让用户看见。若把未知当空处理，用户得到的是「配了没反应」——本仓记忆里
/// 反复出现的那类静默失效。
///
/// `counts(name)` 决定「这个变量填上了」算不算数：悬停提示的逐字段里 `${char}` 恒非空，
/// 若计入，查不到读音的字会留下孤零零的 `好：`。注释段传恒真。
fn render_nodes(
    nodes: &[Node],
    eval: &impl Fn(&str, Option<&str>) -> Option<String>,
    counts: &impl Fn(&str) -> bool,
) -> Rendered {
    let mut out = String::new();
    let mut any = false;
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Var(refs) => {
                // 未知名恒排在「首个非空」判定之外单独处理：它不是值，是错误提示。
                let mut value: Option<(String, bool)> = None;
                for r in refs {
                    match eval(&r.name, r.arg.as_deref()) {
                        None => {
                            value = Some((format!("${{{}}}", r.name), true));
                            break;
                        }
                        Some(v) if !v.is_empty() => {
                            value = Some((v, counts(&r.name)));
                            break;
                        }
                        Some(_) => {} // 已知但空 → 试下一个回退
                    }
                }
                match value {
                    Some((v, counted)) => {
                        out.push_str(&v);
                        any |= counted;
                    }
                    // 空变量吞掉紧邻的一个空白：`(拼: ${pinyin} ${chaizi})` 在拆字为空时
                    // 不留下 `)` 前那个多余空格。只吞一个——吞到底会把用户有意排的版式抹平。
                    None => {
                        if out.ends_with(' ') || out.ends_with('\t') {
                            out.pop();
                        }
                    }
                }
            }
            Node::Group(inner) => {
                let r = render_nodes(inner, eval, counts);
                if r.any_var_filled {
                    out.push_str(&r.text);
                    any = true;
                } else if out.ends_with(' ') || out.ends_with('\t') {
                    // 整段消失时同样吞掉紧邻空白（`${code}{ (${pinyin})}` → `wq`，非 `wq `）。
                    out.pop();
                }
            }
        }
    }
    Rendered {
        text: out,
        any_var_filled: any,
    }
}

/// 渲染模板。**纯函数**。
///
/// 整个模板按一个隐式可选段处理：所有变量都为空 ⇒ 返回空串。否则返回渲染结果（已 trim
/// 首尾空白——模板里为分隔而写的空格，在相邻内容缺席时不该留在两端）。
///
/// `max_chars` = 0 表示不限；超出则截断并加 `…`。
pub(crate) fn render(
    tpl: &str,
    max_chars: usize,
    eval: impl Fn(&str, Option<&str>) -> Option<String>,
) -> String {
    let r = render_nodes(&parse(tpl), &eval, &|_| true);
    if !r.any_var_filled {
        return String::new();
    }
    let s = r.text.trim();
    if max_chars == 0 {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let head: String = chars[..max_chars].iter().collect();
    format!("{head}…")
}

/// 旧 `Template::render` 的等价入口（解析 + 渲染，不 trim、不做全空消失）。副本之外唯一新增的
/// 函数，只为让对拍测试够得着私有的 `render_nodes`。
pub(super) fn render_template(
    tpl: &str,
    eval: &impl Fn(&str, Option<&str>) -> Option<String>,
    counts: &impl Fn(&str) -> bool,
) -> (String, bool) {
    let r = render_nodes(&parse(tpl), eval, counts);
    (r.text, r.any_var_filled)
}
