//! 滑窗草稿：落屏文本流 + 滑窗切分。
//!
//! 设计见 `docs/design/auto-phrase-draft-layer.md`。本模块**只做决策、不做 IO**：
//! 吐出「待写入草稿层的词」，取码/查重/落库由协调器完成。打断语义因此可以全部单测覆盖
//! （同 [`crate::auto_phrase`] 的形态）。
//!
//! # 与 `auto_phrase` 的根本差别：缓冲的是什么
//!
//! `AutoPhraseBuf` 缓冲的是**连续单字序列**——多字词上屏即终止，且该词不入缓冲。
//! 那是「猜一个词」模型下的正确选择，但它正是「正常打字不容易进入造词」的主因：
//! 正常打字里大多数词是选词组上屏的，一句话只要选中一次词组，前面攒的字当场被结算、
//! 词组不参与、之后从零开始。
//!
//! 本模块缓冲的是**最近落屏的文本流**：不论逐字打的还是选词组来的，汉字都进流。
//! 「我今天去上班」不管是怎么打出来的，都切成 `我今天 / 今天去 / 天去上 / 去上班 / …`。
//! **主因那道闸是在换缓冲对象这一步顺带消失的，不是被调松的。**
//!
//! # 杂词
//!
//! 滑窗必然产出大量杂词，这是**预期而非缺陷**：模型的重心从「猜一个词」转移到
//! 「先记一堆、用过的才留」，过滤发生在使用端（草稿层带有效期、用过才跃迁），
//! 不在产生端。这个模型敢用的前提是出厂关闭、受众是主动开启的用户。

use crate::handle_addword::is_han;
use std::time::{Duration, Instant};

/// 滑窗长度下界的兜底值（配置为 0 时用）。
pub const DEFAULT_MIN_WINDOW: usize = 2;
/// 滑窗长度上界的兜底值（配置为 0 时用）。
///
/// 复用 `[schema.codetable.auto_phrase].max_phrase_len` 的语义与取值。★ 它在滑窗模型下
/// 顺带取消了「超长整条序列整体丢弃」那道闸——滑窗本就只切 2–5 字，不存在「整条序列超长」。
pub const DEFAULT_MAX_WINDOW: usize = 5;
/// 两次落屏之间的最大间隔；超过则视作断流（防跨句拼出杂词）。
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// 滑窗切出的一个草稿词，连同其中各字**上屏时用户实际选中的码**（GH#181）。
///
/// 取码时有提示的字按提示取位，没有的回退单字全码表（见
/// `EngineManager::encode_word_with_hints`）。提示只来自单字上屏：词组上屏时候选带的是
/// 整词的码，拆不出每个字各是哪条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftWord {
    pub text: String,
    /// `(字, 码)`，按字在词中的顺序；同一个字只记第一次。
    pub code_hints: Vec<(char, String)>,
}

/// 落屏文本流的滑窗缓冲。
///
/// 只保留最近 `max_window` 个字：再往前的字不可能参与任何新窗口，留着只是占地方。
#[derive(Debug, Default)]
pub struct DraftWindowBuf {
    /// 流里的字，各带上屏时登记的码（没有则 `None`）。
    chars: Vec<(char, Option<String>)>,
    /// 上一次落屏的时刻；`None` = 流为空。
    last_at: Option<Instant>,
    /// 选词出口登记、**下一次落屏**消费的「这个字是用哪条码打出来的」。
    ///
    /// 走登记而不是让落屏动作自己带码：落屏收口在 `note_commit_action`，那里只看得到
    /// `KeyAction`（只有文本）；选中的候选只在选词出口手里。两者在同一次按键内先后发生，
    /// 下一次落屏无论是什么都会把它取走，故不会跨键串到别的字上。
    pending_hint: Option<(char, String)>,
}

impl DraftWindowBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    #[cfg(test)]
    fn buffered(&self) -> String {
        self.chars.iter().map(|(c, _)| *c).collect()
    }

    /// 登记：下一次落屏若恰是单字 `ch`，它是用 `code` 打出来的。
    pub fn set_code_hint(&mut self, ch: char, code: String) {
        self.pending_hint = Some((ch, code));
    }

    /// 作废未消费的登记（本次按键没有落屏）。
    pub fn clear_code_hint(&mut self) {
        self.pending_hint = None;
    }

    /// 文本落屏。返回**本次新产生的窗口**（可能为空）。
    ///
    /// `text` 含任何非汉字即视为断流：标点、英文、数字、空格都是句子边界的信号，
    /// 跨过它们切出来的窗口是纯噪声（「上班。今天」→「班今天」）。判据与
    /// `feed_auto_phrase` 的 `all_han` 一致。
    ///
    /// **逐字推进**：多字词（选词组上屏）会被拆成一个个字依次进流，每个字各自产出
    /// 以它结尾的窗口。这正是本模型与 `AutoPhraseBuf` 的分界——那边多字词是终止符。
    pub fn on_commit(
        &mut self,
        text: &str,
        now: Instant,
        idle_timeout: Duration,
        min_window: usize,
        max_window: usize,
    ) -> Vec<String> {
        self.on_commit_hinted(text, now, idle_timeout, min_window, max_window)
            .into_iter()
            .map(|w| w.text)
            .collect()
    }

    /// 同 [`Self::on_commit`]，但窗口带上各字登记过的上屏码（见 [`DraftWord`]）。
    ///
    /// 消费 [`Self::set_code_hint`] 的登记：本次落屏恰是那个单字时随字进流，否则作废。
    pub fn on_commit_hinted(
        &mut self,
        text: &str,
        now: Instant,
        idle_timeout: Duration,
        min_window: usize,
        max_window: usize,
    ) -> Vec<DraftWord> {
        if text.is_empty() {
            return Vec::new();
        }
        let hint = self.pending_hint.take();
        let mut cs = text.chars();
        let hint = match (cs.next(), cs.next(), hint) {
            (Some(c), None, Some((h, code))) if c == h => Some(code),
            _ => None,
        };
        if !text.chars().all(is_han) {
            self.terminate();
            return Vec::new();
        }
        // 距上次落屏太久 ⇒ 断流，本次以新段起头。
        if self
            .last_at
            .is_some_and(|t| now.saturating_duration_since(t) > idle_timeout)
        {
            self.terminate();
        }
        self.last_at = Some(now);

        let (min, max) = normalize_bounds(min_window, max_window);
        let mut out = Vec::new();
        for ch in text.chars() {
            self.chars.push((ch, hint.clone()));
            // 只保留可能参与后续窗口的那一段。
            if self.chars.len() > max {
                let drop = self.chars.len() - max;
                self.chars.drain(..drop);
            }
            // 以本字结尾、长度 min..=max 的全部窗口。
            let n = self.chars.len();
            for len in min..=max.min(n) {
                let span = &self.chars[n - len..];
                let mut code_hints: Vec<(char, String)> = Vec::new();
                for (c, code) in span {
                    if let Some(code) = code
                        && !code_hints.iter().any(|(h, _)| h == c)
                    {
                        code_hints.push((*c, code.clone()));
                    }
                }
                out.push(DraftWord {
                    text: span.iter().map(|(c, _)| *c).collect(),
                    code_hints,
                });
            }
        }
        out
    }

    /// 回改已落屏的内容（`KeyAction::ReplaceBackward`）：先回退 `utf16_count` 个
    /// **UTF-16 单元**，调用方随后再把替换文本喂进 [`Self::on_commit`]。
    ///
    /// ⚠️ 量纲是 UTF-16 单元而非 `char`，与 `ReplaceBackward.count` 对齐（那是发给宿主的
    /// 删除量）。扩展区汉字占 2 个单元，按 char 数回退会多删一个字——而码表里的生僻字
    /// 大量落在扩展区，正是最需要造词的那批。
    ///
    /// 回退量超过缓冲现有内容时**清空**：那说明被改掉的文字有一部分早于本段流，
    /// 剩下的字与新文本之间的连续性已不可知，继续切窗口就是在造幽灵词。
    pub fn rewind(&mut self, utf16_count: usize) {
        let mut remaining = utf16_count;
        while remaining > 0 {
            match self.chars.pop() {
                Some((c, _)) => remaining = remaining.saturating_sub(c.len_utf16()),
                None => break,
            }
        }
        if self.chars.is_empty() {
            self.last_at = None;
        }
    }

    /// 断流（标点/英文/数字上屏、焦点丢失、IME 停用、模式切换、切换方案、光标移动、idle 超时）。
    ///
    /// 语义是「流到此为止，下一段重新起」，**不是** `AutoPhraseBuf::terminate` 那种
    /// 「把攒的字结算成一个词」——本模型在每次落屏时就已经把窗口吐出去了，断流时没有
    /// 什么待结算的东西。
    pub fn terminate(&mut self) {
        self.chars.clear();
        self.last_at = None;
        self.pending_hint = None;
    }
}

/// 规整长度上下界：0 用兜底值，且保证 `min <= max`。
///
/// `min > max` 时取 `max = min`（只产出一种长度的窗口）而不是静默返回空：配置写反了应当
/// 表现为「窗口变少」，不该表现为「造词功能整个失灵」——后者会被当成缺陷来查很久。
fn normalize_bounds(min_window: usize, max_window: usize) -> (usize, usize) {
    let min = if min_window == 0 {
        DEFAULT_MIN_WINDOW
    } else {
        min_window
    };
    let max = if max_window == 0 {
        DEFAULT_MAX_WINDOW
    } else {
        max_window
    };
    (min, max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    fn feed(buf: &mut DraftWindowBuf, text: &str, t: Instant) -> Vec<String> {
        buf.on_commit(text, t, DEFAULT_IDLE_TIMEOUT, 2, 5)
    }

    /// ★ 本模型的立身之本：**词组上屏不再中断造词**。
    ///
    /// 「我」+「今天」+「去」——中间那个是选词组上屏的。旧模型在这里会把「我」结算掉、
    /// 词组不参与、之后从零开始，于是攒不出任何跨词组的窗口。新模型必须切出「我今天」
    /// 这种横跨单字与词组的组合，否则等于没解决主因。
    #[test]
    fn phrases_no_longer_break_the_stream() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "我", t);
        let w = feed(&mut b, "今天", t);
        assert!(
            w.contains(&"我今天".to_string()),
            "跨「单字→词组」的窗口必须切得出来，否则主因没解决：{w:?}"
        );
        let w = feed(&mut b, "去", t);
        assert!(w.contains(&"今天去".to_string()), "{w:?}");
        assert!(w.contains(&"我今天去".to_string()), "{w:?}");
    }

    /// 每次落屏只产出**以新字结尾**的窗口，长度落在 [min,max]。
    #[test]
    fn each_commit_yields_windows_ending_at_the_new_char() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        assert!(
            feed(&mut b, "一", t).is_empty(),
            "只有 1 个字，够不到 min=2"
        );
        assert_eq!(feed(&mut b, "二", t), vec!["一二"]);
        assert_eq!(feed(&mut b, "三", t), vec!["二三", "一二三"]);
        assert_eq!(feed(&mut b, "四", t), vec!["三四", "二三四", "一二三四"]);
        assert_eq!(
            feed(&mut b, "五", t),
            vec!["四五", "三四五", "二三四五", "一二三四五"]
        );
        // 第 6 个字：最长仍是 5，最早那个字已滑出
        assert_eq!(
            feed(&mut b, "六", t),
            vec!["五六", "四五六", "三四五六", "二三四五六"],
            "max=5 之外的窗口不该出现"
        );
    }

    /// 非汉字断流：标点两侧的字不能拼进同一个窗口。
    #[test]
    fn non_han_breaks_the_stream() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "上", t);
        feed(&mut b, "班", t);
        assert!(feed(&mut b, "。", t).is_empty(), "标点本身不产出窗口");
        let w = feed(&mut b, "今", t);
        assert!(w.is_empty(), "断流后重新起头，第一个字够不到 min");
        let w = feed(&mut b, "天", t);
        assert_eq!(w, vec!["今天"], "「班今天」这种跨句杂词不该出现：{w:?}");
    }

    /// idle 超时断流。
    #[test]
    fn idle_timeout_breaks_the_stream() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "上", t);
        let later = t + DEFAULT_IDLE_TIMEOUT + Duration::from_secs(1);
        let w = b.on_commit("班", later, DEFAULT_IDLE_TIMEOUT, 2, 5);
        assert!(w.is_empty(), "隔了很久再打，不该与上一段拼起来：{w:?}");
    }

    /// 回退按 **UTF-16 单元**，不是 char 数。
    ///
    /// 扩展区汉字（`𠀀` U+20000）占 2 个单元。按 char 数回退会多删一个字，
    /// 而码表里的生僻字大量落在扩展区——正是最需要造词的那批。
    #[test]
    fn rewind_counts_utf16_units_not_chars() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "工", t);
        feed(&mut b, "𠀀", t); // 2 个 UTF-16 单元
        assert_eq!(b.buffered(), "工𠀀");
        b.rewind(2); // 只该删掉那个扩展区字
        assert_eq!(b.buffered(), "工", "按 char 数回退会把「工」也删掉");
    }

    /// 回退量超过缓冲 ⇒ 清空。剩下的字与新文本之间的连续性已不可知。
    #[test]
    fn rewinding_past_the_buffer_clears_it() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "工", t);
        b.rewind(10);
        assert!(b.is_empty());
        let w = feed(&mut b, "人", t);
        assert!(w.is_empty(), "清空后是新段的第一个字，不该产出窗口：{w:?}");
    }

    /// 配置写反（min > max）表现为「窗口变少」，不是造词整个失灵。
    #[test]
    fn inverted_bounds_degrade_instead_of_disabling() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        b.on_commit("一", t, DEFAULT_IDLE_TIMEOUT, 4, 2);
        b.on_commit("二", t, DEFAULT_IDLE_TIMEOUT, 4, 2);
        b.on_commit("三", t, DEFAULT_IDLE_TIMEOUT, 4, 2);
        let w = b.on_commit("四", t, DEFAULT_IDLE_TIMEOUT, 4, 2);
        assert_eq!(
            w,
            vec!["一二三四"],
            "min>max 时该退化成只产出 min 长度：{w:?}"
        );
    }

    /// 0 = 用兜底值。
    #[test]
    fn zero_bounds_fall_back_to_defaults() {
        assert_eq!(
            normalize_bounds(0, 0),
            (DEFAULT_MIN_WINDOW, DEFAULT_MAX_WINDOW)
        );
    }

    /// 空文本不影响流（承自 `AutoPhraseBuf::empty_commit_is_noop`）。
    ///
    /// 空的 `InsertText` 在生产里确实会出现（若干路径把「没有可上屏的文本」也走同一个
    /// 返回变体）。它既不该产出窗口，也**不该被当成非汉字而断流**——那会让流被无端切碎。
    #[test]
    fn empty_commit_neither_yields_nor_breaks() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "你", t);
        assert!(feed(&mut b, "", t).is_empty(), "空文本不产出窗口");
        assert_eq!(b.buffered(), "你", "空文本不该影响流");
        assert_eq!(
            feed(&mut b, "好", t),
            vec!["你好"],
            "流没被切断，窗口照常切出"
        );
    }

    // ───────────── GH#181：窗口带上各字的上屏码 ─────────────

    fn hinted(b: &mut DraftWindowBuf, text: &str, t: Instant) -> Vec<DraftWord> {
        b.on_commit_hinted(text, t, DEFAULT_IDLE_TIMEOUT, 2, 5)
    }

    /// 单字上屏前登记的码，随这个字进流，并出现在**所有**含它的窗口里。
    #[test]
    fn code_hint_rides_along_with_its_char() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        b.set_code_hint('好', "hc".into());
        hinted(&mut b, "好", t);
        b.set_code_hint('嘞', "lw".into());
        let w = hinted(&mut b, "嘞", t);
        assert_eq!(
            w,
            vec![DraftWord {
                text: "好嘞".into(),
                code_hints: vec![('好', "hc".into()), ('嘞', "lw".into())],
            }]
        );
        // 下一个字没登记码：窗口里只有前两个字带码。
        let w = hinted(&mut b, "啊", t);
        assert_eq!(
            w[1].code_hints,
            vec![('好', "hc".into()), ('嘞', "lw".into())],
            "{w:?}"
        );
        assert_eq!(w[0].text, "嘞啊");
        assert_eq!(w[0].code_hints, vec![('嘞', "lw".into())]);
    }

    /// 登记的码**只管下一次落屏**、且必须字对得上：词组上屏、换了个字上屏都不能张冠李戴。
    #[test]
    fn code_hint_applies_only_to_the_matching_single_char_commit() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        b.set_code_hint('今', "wyn".into());
        let w = hinted(&mut b, "今天", t);
        assert!(w[0].code_hints.is_empty(), "词组上屏不吃单字的码：{w:?}");
        b.set_code_hint('去', "fcu".into());
        let w = hinted(&mut b, "上", t);
        assert!(
            w.iter().all(|d| d.code_hints.is_empty()),
            "字对不上不能套用：{w:?}"
        );
        let w = hinted(&mut b, "去", t);
        assert!(
            w.iter().all(|d| d.code_hints.is_empty()),
            "登记只管下一次落屏，过期不再生效：{w:?}"
        );
    }

    /// 断流顺带作废未消费的登记。
    #[test]
    fn terminate_drops_pending_code_hint() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        hinted(&mut b, "你", t);
        b.set_code_hint('好', "vb".into());
        b.terminate();
        hinted(&mut b, "你", t);
        let w = hinted(&mut b, "好", t);
        assert!(w[0].code_hints.is_empty(), "{w:?}");
    }

    /// 断流不产出任何待结算的东西——与 `AutoPhraseBuf::terminate` 的语义分界。
    #[test]
    fn terminate_yields_nothing_because_windows_were_already_emitted() {
        let mut b = DraftWindowBuf::new();
        let t = now();
        feed(&mut b, "你", t);
        feed(&mut b, "好", t); // 「你好」这个窗口在这一刻就已经吐出去了
        b.terminate();
        assert!(b.is_empty());
    }
}
