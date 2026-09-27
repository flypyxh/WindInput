//! 「这个字有哪些辅助码」的查询接口。筛选只通过它取码，不再假定背后是一张静态表：
//! 来源可以是码表文件，也可以是一个输入方案（系统词库 + 用户词库，见协调器的
//! `aux_code_source`）。设计见 `docs/design/aux-code-schema-source.md` §4。

use crate::table::AuxCodeTable;

pub trait AuxCodeLookup {
    /// 按优先级把 `ch` 的每个辅助码交给 `pred`，任一返回 true 即停并返回 true；无码返回 false。
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool;

    /// 一个字都没有（未挂载）。筛选据此走「不过滤、原样放行」的防御语义。
    fn is_empty(&self) -> bool;

    /// 任一码以 `prefix` 开头（单字筛选）。
    fn any_code_starts_with(&self, ch: char, prefix: &str) -> bool {
        self.any_code(ch, &mut |c| c.starts_with(prefix))
    }

    /// 任一码以单字符 `c` 开头（词组逐字首码，零分配）。
    fn any_code_starts_with_char(&self, ch: char, c: char) -> bool {
        self.any_code(ch, &mut |code| code.starts_with(c))
    }
}

impl AuxCodeLookup for AuxCodeTable {
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
        self.view_codes(ch).is_some_and(|v| v.iter().any(pred))
    }

    fn is_empty(&self) -> bool {
        AuxCodeTable::is_empty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuxCodeFilterOptions, AuxCodeTable, filter_by_aux_code};
    use wind_candidate::Candidate;

    /// 两张表按序拼接的最小实现，证明筛选对「多层来源」成立。
    struct Pair(AuxCodeTable, AuxCodeTable);
    impl AuxCodeLookup for Pair {
        fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
            self.0.any_code(ch, pred) || self.1.any_code(ch, pred)
        }
        fn is_empty(&self) -> bool {
            AuxCodeLookup::is_empty(&self.0) && AuxCodeLookup::is_empty(&self.1)
        }
    }

    fn cand(t: &str) -> Candidate {
        Candidate {
            text: t.into(),
            ..Default::default()
        }
    }

    #[test]
    fn filter_sees_codes_from_every_layer() {
        let lk = Pair(
            AuxCodeTable::from_rows(vec![('李', "mz")]),
            AuxCodeTable::from_rows(vec![('河', "sk")]),
        );
        let out = filter_by_aux_code(
            vec![cand("李"), cand("河"), cand("樱")],
            &lk,
            "s",
            &AuxCodeFilterOptions::default(),
        );
        let kept: Vec<&str> = out.kept.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(kept, vec!["河"], "第二层的码同样参与筛选");
    }

    #[test]
    fn table_impl_matches_inherent_methods() {
        let t = AuxCodeTable::from_rows(vec![('厑', "ib"), ('厑', "ii")]);
        let lk: &dyn AuxCodeLookup = &t;
        assert!(lk.any_code_starts_with('厑', "ib"));
        assert!(lk.any_code_starts_with_char('厑', 'i'));
        assert!(!lk.any_code_starts_with('厑', "x"));
        assert!(!lk.is_empty());
    }
}
