//! 码表词组编码器：按方案 `[[encoder.rules]]` 的公式，从各字**全码**组装词组编码。
//!
//! 五笔 86 的规则形如 `AaAbBaBb`（二字取各字前两码）、`AaBaCaCb`（三字取前两字首码+末字前
//! 两码）、`AaBaCaZa`（四字及以上取前三字首码+末字首码）。公式每两个字符一组：**大写=字序**
//! （`A`=第 1 字，`Z`=末字），**小写=码序**（`a`=第 1 码）。
//!
//! # 为什么不能拼接各字全码
//!
//! 造词的唯一目的是「造出来的词以后能打出来」。五笔「你好」的词组码是 `wqvb`（各取前两码），
//! 而拼接全码得到 `wqiyvbg` 之类——词库里查不到，等于没造。旧 `learn_phrase_on_commit` 正是
//! 拼接各段码（`code.push_str`），这是自动造词「完全不工作」的两个根因之一。
//!
//! # 与旧 `wind_reverse::wubi_word_code` 的关系
//!
//! 后者把五笔 86 规则**硬编码**成 `match chars.len()` 三个分支，且码源是**拆字表**而非码表
//! 词库。两者对五笔 86 结果等价（公式与硬编码规则一一对应），但硬编码版换任何非五笔码表方案
//! 就静默出错。本模块取代它，手动造词与自动造词统一走这里。

use wind_config::schema::{EncoderRule, EncoderSpec};

/// 公式的一步：取第 `char_index` 个字的第 `code_index` 位码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormulaStep {
    /// 字序，0-based；`-1` 表示末字（公式中的 `Z`）。
    char_index: i32,
    /// 码序，0-based（`a`=0）。
    code_index: usize,
}

/// 取码失败的原因。**携带具体是哪个字卡住**——这是排查「自动造词不生效」最关键的线索
/// （对齐 Go `CalcWordCode returned empty` 那条注释的意图，但 Go 只返回空串丢失了原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// 词太短（< 2 字），没有词组编码的概念。
    TooShort,
    /// 方案没有配 `[[encoder.rules]]`。
    NoRules,
    /// 该词长没有匹配的规则。
    NoMatchingRule { word_len: usize },
    /// 公式本身非法（长度为奇数、含非字母等）。
    BadFormula { formula: String },
    /// 公式引用的字序超出词长（规则与词长不匹配，属方案配置错误）。
    CharIndexOutOfRange { char_index: i32, word_len: usize },
    /// 该字在码表词库中查不到任何码。**整词作废**，不做「跳过该字」的降级。
    MissingCode { ch: char },
    /// 该字的全码位数不够公式要求（如公式要第 2 码但该字只有 1 位码）。
    CodeTooShort { ch: char, code: String, need: usize },
    /// 词里没有可取码的字（纯数字/字母/符号且码表里都没码）。取不到码的非汉字只做跳过
    /// （GH#171），全部跳过之后无码可取。
    NoHanChars,
}

impl EncodeError {
    /// 不含任何字符的失败类别，供 `info` 级日志使用（隐私规则：INFO 不得带词条内容，
    /// 而 `Display` 会带上卡住的那个字）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TooShort => "too_short",
            Self::NoRules => "no_rules",
            Self::NoMatchingRule { .. } => "no_matching_rule",
            Self::BadFormula { .. } => "bad_formula",
            Self::CharIndexOutOfRange { .. } => "char_index_out_of_range",
            Self::MissingCode { .. } => "missing_code",
            Self::CodeTooShort { .. } => "code_too_short",
            Self::NoHanChars => "no_han_chars",
        }
    }
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "词少于 2 字，无词组编码"),
            Self::NoRules => write!(f, "方案未配置 [[encoder.rules]]"),
            Self::NoMatchingRule { word_len } => {
                write!(f, "词长 {word_len} 无匹配的编码规则")
            }
            Self::BadFormula { formula } => write!(f, "编码公式非法: {formula:?}"),
            Self::CharIndexOutOfRange {
                char_index,
                word_len,
            } => write!(f, "公式引用第 {char_index} 字，超出词长 {word_len}"),
            Self::MissingCode { ch } => write!(f, "码表中查不到「{ch}」的编码"),
            Self::CodeTooShort { ch, code, need } => write!(
                f,
                "「{ch}」的全码 {code:?} 只有 {} 位，公式需要第 {} 位",
                code.chars().count(),
                need + 1
            ),
            Self::NoHanChars => write!(f, "不含可取码的字，无从取码"),
        }
    }
}

/// 解析编码公式为步骤列表。每两个字符一组：大写=字序（`A`=0…`Y`=24，`Z`=-1 表末字），
/// 小写=码序（`a`=0）。长度为奇数或含非法字符返回 `None`。
fn parse_formula(formula: &str) -> Option<Vec<FormulaStep>> {
    let bytes = formula.as_bytes();
    // 公式恒为 ASCII 字母；非 ASCII 直接判非法，避免按字节切进多字节字符中间。
    if !formula.is_ascii() || bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut steps = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let (upper, lower) = (pair[0], pair[1]);
        if !upper.is_ascii_uppercase() || !lower.is_ascii_lowercase() {
            return None;
        }
        steps.push(FormulaStep {
            // 'Z' 是末字的专用记号，不参与 A=0 的顺序编号。
            char_index: if upper == b'Z' {
                -1
            } else {
                (upper - b'A') as i32
            },
            code_index: (lower - b'a') as usize,
        });
    }
    Some(steps)
}

/// 为词长挑规则：先找 `length_equal` 精确匹配，再找 `length_in_range` 区间匹配
/// （对齐 Go `MatchRule` 的两轮顺序——精确规则优先于区间规则，与书写顺序无关）。
fn match_rule(rules: &[EncoderRule], word_len: usize) -> Option<&EncoderRule> {
    rules
        .iter()
        .find(|r| r.length_equal != 0 && r.length_equal == word_len)
        .or_else(|| {
            rules.iter().find(|r| {
                matches!(r.length_in_range.as_slice(), [min, max]
                    if word_len >= *min && word_len <= *max)
            })
        })
}

/// 取码意义上的「汉字」：只认表意文字区（基本区、扩展 A、兼容表意文字、平面 2/3 整体）。
///
/// 全角标点（U+FF00–FFEF）、中文标点（U+3000–303F）、部首、笔画都不算——它们不是
/// 造词素材。`wind_coordinator::handle_addword::is_han` 直接委托到这里，两边口径同源：
/// 加词的准入判据与取码时的跳过判据一旦漂移，就会出现「放进来了却取不出码」。
pub fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF      // 基本区
        | 0x3400..=0x4DBF    // 扩展 A
        | 0xF900..=0xFAFF    // 兼容表意文字
        // 平面 2（SIP）/ 平面 3（TIP）整体：两个平面专用于表意文字，扩展 B–J 与兼容汉字
        // 补充全在其中，将来的扩展 K/L 亦然。
        | 0x20000..=0x3FFFF)
}

/// 为一段**加词文本**取码（加词 / 设置端出码 / 导入词表 / 自动造词的统一口径）。
///
/// - **整段就一个字符**：直取它的全码（不论是否汉字——码表里可能真收录了符号条目）。
/// - 否则**跳过取不到码的非汉字**（数字、字母、空白、标点、emoji…），用剩下的字取码
///   （GH#171：五笔下加「张三13800138000」这类姓名+号码，维护者定为只用中文部分出码，
///   与「张三」同码可以接受）。剩 0 个 → [`EncodeError::NoHanChars`]；剩 1 个 → 单字规则
///   （直取全码）；≥2 个 → 按方案公式（[`calc_word_code_by`]）。
///
/// ⚠️ 判据是「**有没有码**」优先、「是不是汉字」其次，两头都不能少：
/// - 码表里**有码**的非汉字照样参与（`〇` 在五笔里是 `llll`，私用区字、部首、`々` 在
///   某些码表里也当字用）。只看 `is_han` 会把「二〇二六」算成「二二六」的码。
/// - 取不到码的**汉字**（生僻字）不跳过、让整词作废——那正是 [`calc_word_code`] 防
///   「你X好」被算成「你好」的那条保护，放宽不能连带打穿它。
///
/// `code_of(ch, need)` 见 [`calc_word_code_by`]。
pub fn encode_text<F>(word: &str, spec: &EncoderSpec, code_of: F) -> Result<String, EncodeError>
where
    F: Fn(char, usize) -> Option<String>,
{
    let mut cs = word.chars();
    if let (Some(c), None) = (cs.next(), cs.next()) {
        return code_of(c, 1).ok_or(EncodeError::MissingCode { ch: c });
    }
    // 探码只问「有没有」（need=1）：真正取位时码源还会按公式要的位数再挑一次。
    let han: String = word
        .chars()
        .filter(|&c| is_han(c) || code_of(c, 1).is_some())
        .collect();
    let mut hs = han.chars();
    match (hs.next(), hs.next()) {
        (None, _) => Err(EncodeError::NoHanChars),
        (Some(c), None) => code_of(c, 1).ok_or(EncodeError::MissingCode { ch: c }),
        _ => calc_word_code_by(&han, spec, code_of),
    }
}

/// 按方案编码规则计算词组编码。
///
/// `code_of` 提供单字**全码**（见 `EngineManager::single_char_full_codes` 的全码判据：
/// 码长上限闸 → 最长码长 → 权重降序 → 首次出现）。任一字取不到码即**整词作废**，
/// 不做「跳过该字」的降级——那会把「你X好」算成「你好」的码，静默产出错词。
pub fn calc_word_code<F>(word: &str, spec: &EncoderSpec, code_of: F) -> Result<String, EncodeError>
where
    F: Fn(char) -> Option<String>,
{
    calc_word_code_by(word, spec, |c, _| code_of(c))
}

/// 同 [`calc_word_code`]，但 `code_of(ch, need)` 还会被告知**公式要从该字取到第几位**
/// （`need` = 该字被引用的最大码序 + 1）。
///
/// 存在理由（GH#181②）：码源手里一个字常有多条码（简码 `m` 与全码 `mwdy`、多读音的
/// `le…`/`lw…`），只有知道公式要几位，才能挑出一条既符合用户所打、又够长的码。
/// 码源给回的码仍短于 `need` 时照旧报 [`EncodeError::CodeTooShort`]。
pub fn calc_word_code_by<F>(
    word: &str,
    spec: &EncoderSpec,
    code_of: F,
) -> Result<String, EncodeError>
where
    F: Fn(char, usize) -> Option<String>,
{
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < 2 {
        return Err(EncodeError::TooShort);
    }
    if spec.rules.is_empty() {
        return Err(EncodeError::NoRules);
    }
    let rule = match_rule(&spec.rules, chars.len()).ok_or(EncodeError::NoMatchingRule {
        word_len: chars.len(),
    })?;
    let steps = parse_formula(&rule.formula).ok_or_else(|| EncodeError::BadFormula {
        formula: rule.formula.clone(),
    })?;

    // 先把每一步落到具体的字上，并统计每个字公式要取到第几位（`need`）——码源要据此挑码。
    let mut resolved: Vec<char> = Vec::with_capacity(steps.len());
    let mut need: Vec<(char, usize)> = Vec::with_capacity(chars.len());
    for step in &steps {
        let ch = if step.char_index < 0 {
            chars[chars.len() - 1]
        } else {
            let i = step.char_index as usize;
            if i >= chars.len() {
                return Err(EncodeError::CharIndexOutOfRange {
                    char_index: step.char_index,
                    word_len: chars.len(),
                });
            }
            chars[i]
        };
        resolved.push(ch);
        match need.iter_mut().find(|(c, _)| *c == ch) {
            Some((_, n)) => *n = (*n).max(step.code_index + 1),
            None => need.push((ch, step.code_index + 1)),
        }
    }

    // 按字缓存全码：同一个字在公式里常被取多次（如 `AaAb` 取两次首字），避免重复查表。
    let mut cache: Vec<(char, String)> = Vec::with_capacity(chars.len());
    let mut out = String::with_capacity(steps.len());
    for (step, &ch) in steps.iter().zip(&resolved) {
        let code = match cache.iter().find(|(c, _)| *c == ch) {
            Some((_, code)) => code.clone(),
            None => {
                let n = need.iter().find(|(c, _)| *c == ch).map_or(1, |(_, n)| *n);
                let code = code_of(ch, n).ok_or(EncodeError::MissingCode { ch })?;
                cache.push((ch, code.clone()));
                code
            }
        };
        // 码是 ASCII 键位串，但仍按 char 取以防方案用了非 ASCII 码位。
        let piece = code
            .chars()
            .nth(step.code_index)
            .ok_or_else(|| EncodeError::CodeTooShort {
                ch,
                code: code.clone(),
                need: step.code_index,
            })?;
        out.push(piece);
    }
    Ok(out)
}

/// 按**用户实际打的码**为一个字挑全码（GH#181①）；挑不出返回 `None`，调用方回退全码表。
///
/// - `hint`：用户上屏这个字时选中的那条码（候选的词条码，如打 `lw` 选中「嘞」得 `lwkk`
///   或 `lw`）。它决定了读音/拆法——同一个字的多条同长全码只有它知道用户要哪条。
/// - `full`：单字全码表给的那条（按权重挑过）。它以 `hint` 开头且够长就用它。
/// - `all_codes`：该字在方案词库里的全部码（反查索引）。否则从中取以 `hint` 开头、
///   不超码长上限 `cap`（0 = 不设闸）、长度 ≥ `need` 的**最长**一条，同长取先出现者。
///
/// 返回 `None` 的典型情形：`hint` 根本不是本方案的码（临时拼音上屏的字带的是拼音码），
/// 或同前缀里没有够长的码。这两种都不该硬用 `hint`。
pub fn pick_hinted_code<'a>(
    hint: &str,
    full: Option<&str>,
    all_codes: impl Iterator<Item = &'a str>,
    cap: usize,
    need: usize,
) -> Option<String> {
    if hint.is_empty() {
        return None;
    }
    let len = |c: &str| c.chars().count();
    if let Some(f) = full
        && f.starts_with(hint)
        && len(f) >= need
    {
        return Some(f.to_string());
    }
    let mut best: Option<&str> = None;
    for c in all_codes {
        let n = len(c);
        if !c.starts_with(hint) || n < need || (cap > 0 && n > cap) {
            continue;
        }
        if best.is_none_or(|b| n > len(b)) {
            best = Some(c);
        }
    }
    best.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 五笔 86 标准三条规则（与 `data/schemas/wubi86.schema.toml` 一致）。
    fn wubi_spec() -> EncoderSpec {
        EncoderSpec {
            max_word_length: 10,
            exclude_patterns: Vec::new(),
            rules: vec![
                EncoderRule {
                    length_equal: 2,
                    length_in_range: Vec::new(),
                    formula: "AaAbBaBb".into(),
                },
                EncoderRule {
                    length_equal: 3,
                    length_in_range: Vec::new(),
                    formula: "AaBaCaCb".into(),
                },
                EncoderRule {
                    length_equal: 0,
                    length_in_range: vec![4, 10],
                    formula: "AaBaCaZa".into(),
                },
            ],
        }
    }

    fn codes() -> HashMap<char, String> {
        // 取真实五笔全码，便于人工核对。
        [
            ('你', "wqiy"),
            ('好', "vbg"),
            ('我', "trnt"),
            ('中', "khk"),
            ('国', "lgyi"),
            ('人', "wwww"),
            ('民', "nav"),
            ('共', "awu"),
            ('和', "tkg"),
        ]
        .into_iter()
        .map(|(c, s)| (c, s.to_string()))
        .collect()
    }

    fn lookup(m: &HashMap<char, String>) -> impl Fn(char) -> Option<String> + '_ {
        move |c| m.get(&c).cloned()
    }

    #[test]
    fn parses_formula_pairs() {
        let steps = parse_formula("AaAbBaBb").unwrap();
        assert_eq!(
            steps,
            vec![
                FormulaStep {
                    char_index: 0,
                    code_index: 0
                },
                FormulaStep {
                    char_index: 0,
                    code_index: 1
                },
                FormulaStep {
                    char_index: 1,
                    code_index: 0
                },
                FormulaStep {
                    char_index: 1,
                    code_index: 1
                },
            ]
        );
    }

    /// `Z` 是末字记号，不能被当成 A=0 序列里的第 26 个字。
    #[test]
    fn z_means_last_char() {
        let steps = parse_formula("Za").unwrap();
        assert_eq!(steps[0].char_index, -1);
    }

    #[test]
    fn rejects_malformed_formula() {
        assert!(parse_formula("Aa B").is_none(), "含空格应判非法");
        assert!(parse_formula("AaB").is_none(), "奇数长度应判非法");
        assert!(parse_formula("aA").is_none(), "大小写颠倒应判非法");
        assert!(parse_formula("").is_none(), "空公式应判非法");
        assert!(
            parse_formula("A字").is_none(),
            "非 ASCII 应判非法且不 panic"
        );
    }

    /// 二字词：各取前两码。你(wqiy)+好(vbg) → wq+vb = wqvb。
    #[test]
    fn two_char_word_takes_first_two_codes_each() {
        let m = codes();
        let code = calc_word_code("你好", &wubi_spec(), lookup(&m)).unwrap();
        assert_eq!(code, "wqvb");
    }

    /// 三字词：前两字首码 + 末字前两码。中(k)+国(l)+人(ww) → klww。
    #[test]
    fn three_char_word_uses_third_rule() {
        let m = codes();
        let code = calc_word_code("中国人", &wubi_spec(), lookup(&m)).unwrap();
        assert_eq!(code, "klww");
    }

    /// 四字：前三字首码 + 末字首码。中(k)+国(l)+人(w)+民(n) → klwn。
    #[test]
    fn four_char_word_uses_range_rule() {
        let m = codes();
        let code = calc_word_code("中国人民", &wubi_spec(), lookup(&m)).unwrap();
        assert_eq!(code, "klwn");
    }

    /// 五字及以上：`Za` 取的是**末字**，中间字全部跳过——中(k)+国(l)+人(w)+末字和(t)。
    /// 这条专门盯住 `Z` 的语义：若把 `Z` 当成 A=0 序列的第 26 个字，此处会越界报错而非取到「和」。
    #[test]
    fn five_char_word_skips_middle_and_takes_last() {
        let m = codes();
        let code = calc_word_code("中国人民和", &wubi_spec(), lookup(&m)).unwrap();
        assert_eq!(code, "klwt");
    }

    /// 缺码整词作废，且错误携带具体是哪个字——这是排查造词不生效的关键线索。
    #[test]
    fn missing_code_fails_whole_word_with_char() {
        let m = codes();
        let err = calc_word_code("你囧好", &wubi_spec(), lookup(&m)).unwrap_err();
        assert_eq!(err, EncodeError::MissingCode { ch: '囧' });
    }

    /// 公式要第 2 码但该字只有 1 位码（简码被误当全码时的典型症状）→ 明确报错，不静默截断。
    #[test]
    fn code_shorter_than_formula_needs_reports_which_char() {
        let mut m = codes();
        m.insert('好', "v".into()); // 模拟错取了简码
        let err = calc_word_code("你好", &wubi_spec(), lookup(&m)).unwrap_err();
        assert_eq!(
            err,
            EncodeError::CodeTooShort {
                ch: '好',
                code: "v".into(),
                need: 1,
            }
        );
    }

    #[test]
    fn rejects_single_char_and_missing_rules() {
        let m = codes();
        assert_eq!(
            calc_word_code("你", &wubi_spec(), lookup(&m)).unwrap_err(),
            EncodeError::TooShort
        );
        let empty = EncoderSpec::default();
        assert_eq!(
            calc_word_code("你好", &empty, lookup(&m)).unwrap_err(),
            EncodeError::NoRules
        );
    }

    /// 精确规则优先于区间规则，与书写顺序无关（区间写在前也不应抢走 length_equal 的匹配）。
    #[test]
    fn exact_rule_wins_over_range_regardless_of_order() {
        let spec = EncoderSpec {
            max_word_length: 10,
            exclude_patterns: Vec::new(),
            rules: vec![
                EncoderRule {
                    length_equal: 0,
                    length_in_range: vec![2, 10],
                    formula: "AaBa".into(),
                },
                EncoderRule {
                    length_equal: 2,
                    length_in_range: Vec::new(),
                    formula: "AaAbBaBb".into(),
                },
            ],
        };
        let m = codes();
        assert_eq!(
            calc_word_code("你好", &spec, lookup(&m)).unwrap(),
            "wqvb",
            "词长 2 应命中 length_equal 规则而非区间规则"
        );
    }

    // ───────────── GH#171：只用中文部分取码 ─────────────

    fn encode(word: &str) -> Result<String, EncodeError> {
        let m = codes();
        encode_text(word, &wubi_spec(), |c, _| m.get(&c).cloned())
    }

    /// 姓名+手机号：数字整段跳过，按「张三」那部分取码（维护者已定：两者同码可接受）。
    #[test]
    fn digits_are_skipped_and_han_part_is_encoded() {
        assert_eq!(encode("你好13800138000").unwrap(), "wqvb");
        assert_eq!(encode("你1好").unwrap(), "wqvb", "夹在中间的数字同样跳过");
        assert_eq!(encode("中国人abc").unwrap(), "klww", "ASCII 字母跳过");
        assert_eq!(encode("中国 人-民").unwrap(), "klwn", "空白与符号跳过");
        assert_eq!(encode("你好，").unwrap(), "wqvb", "全角标点同样跳过");
    }

    /// ★ 码表里**有码**的非汉字（`〇` 在五笔里是 `llll`、私用区字、部首…）照样参与取码，
    /// 只有取不到码的非汉字才跳过。按「是不是汉字」一刀切会把「二〇二六」算成「二二六」的码。
    #[test]
    fn non_han_with_a_code_still_takes_part() {
        let mut m = codes();
        for (c, code) in [('〇', "llll"), ('二', "fgg"), ('六', "uygy")] {
            m.insert(c, code.into());
        }
        let enc = |w: &str| encode_text(w, &wubi_spec(), |c, _| m.get(&c).cloned());
        // 四字规则 AaBaCaZa：二(f)〇(l)二(f)六(u)。
        assert_eq!(enc("二〇二六").unwrap(), "flfu");
        assert_eq!(enc("二〇二六1").unwrap(), "flfu", "没码的数字仍跳过");
    }

    /// 剩 1 个汉字按单字规则（直取全码），不进词组公式。
    #[test]
    fn single_han_left_uses_single_char_rule() {
        assert_eq!(encode("你123").unwrap(), "wqiy");
    }

    /// 一个汉字都不剩 → 仍失败，不能退化成空码或拿符号去凑。
    #[test]
    fn no_han_left_still_fails() {
        assert_eq!(encode("13800138000"), Err(EncodeError::NoHanChars));
        assert_eq!(encode("ab"), Err(EncodeError::NoHanChars));
    }

    /// ★「你X好」保护不能被这次放宽连带打穿：X 是**取不到码的汉字**（生僻字）时仍整词作废。
    /// 只有非汉字才可跳过——把生僻字也跳过，就会静默造出「你好」的码。
    #[test]
    fn rare_han_without_code_still_fails_whole_word() {
        assert_eq!(
            encode("你囧好123"),
            Err(EncodeError::MissingCode { ch: '囧' })
        );
    }

    /// 词本身就是单个字符：保持直取口径（不论是否汉字），取不到报 MissingCode。
    #[test]
    fn single_char_word_keeps_direct_lookup() {
        assert_eq!(encode("你").unwrap(), "wqiy");
        assert_eq!(encode("a"), Err(EncodeError::MissingCode { ch: 'a' }));
    }

    /// 公式对每个字要取到第几位，要如实告诉码源——码源据此挑一条够长的码（GH#181②）。
    #[test]
    fn code_source_is_told_how_many_codes_each_char_needs() {
        let m = codes();
        let seen = std::cell::RefCell::new(Vec::new());
        calc_word_code_by("中国人", &wubi_spec(), |c, need| {
            seen.borrow_mut().push((c, need));
            m.get(&c).cloned()
        })
        .unwrap();
        // AaBaCaCb：中、国只取第 1 码，人取到第 2 码。
        assert_eq!(seen.into_inner(), vec![('中', 1), ('国', 1), ('人', 2)]);
    }

    // ───────────── GH#181①：按用户实际打的码挑全码 ─────────────

    /// 多读音同长（嘞 le…/lw…）：全码表按权重挑了 le，用户打的是 lw → 必须取 lw 那条。
    #[test]
    fn hint_picks_the_reading_user_typed() {
        let all = ["lekk", "lwkk"];
        assert_eq!(
            pick_hinted_code("lw", Some("lekk"), all.iter().copied(), 4, 2).as_deref(),
            Some("lwkk")
        );
    }

    /// 全码表的那条本就与用户所打一致 → 用它（它是按权重挑过的）。
    #[test]
    fn hint_prefers_full_table_code_when_consistent() {
        let all = ["lw", "lwka", "lwkk"];
        assert_eq!(
            pick_hinted_code("lw", Some("lwkk"), all.iter().copied(), 4, 2).as_deref(),
            Some("lwkk")
        );
    }

    /// 用户打的是 1 位简码（没 → m）、公式要第 2 位：取同前缀里够长的码，不取 m 本身。
    #[test]
    fn short_hint_is_extended_to_a_long_enough_code() {
        let all = ["m", "mody", "mwdy"];
        assert_eq!(
            pick_hinted_code("mw", Some("m"), all.iter().copied(), 4, 2).as_deref(),
            Some("mwdy")
        );
        assert_eq!(
            pick_hinted_code("m", Some("m"), all.iter().copied(), 4, 2).as_deref(),
            Some("mody"),
            "全码表那条不够长 → 退到同前缀的更长码（同长取先出现者）"
        );
    }

    /// 超过码长上限的怪码（6 码扩展码）不参与；与用户所打对不上的（临拼出的拼音码）→ None，
    /// 由调用方回退全码表。
    #[test]
    fn hint_respects_cap_and_falls_back_when_inconsistent() {
        let all = ["m", "okuvuu"];
        assert_eq!(
            pick_hinted_code("ok", Some("m"), all.iter().copied(), 4, 2),
            None
        );
        let all = ["lekk"];
        assert_eq!(
            pick_hinted_code("lei", Some("lekk"), all.iter().copied(), 4, 2),
            None
        );
    }

    /// 词长超出所有规则覆盖范围 → 明确报 NoMatchingRule，不静默返回空串。
    #[test]
    fn word_longer_than_all_rules_reports_no_matching_rule() {
        let m = codes();
        let spec = EncoderSpec {
            max_word_length: 10,
            exclude_patterns: Vec::new(),
            rules: vec![EncoderRule {
                length_equal: 2,
                length_in_range: Vec::new(),
                formula: "AaAbBaBb".into(),
            }],
        };
        assert_eq!(
            calc_word_code("中国人", &spec, lookup(&m)).unwrap_err(),
            EncodeError::NoMatchingRule { word_len: 3 }
        );
    }
}
