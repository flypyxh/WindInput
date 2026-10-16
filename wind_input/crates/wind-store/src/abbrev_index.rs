//! 简拼检索：用户词 / 临时词按声母串召回。
//!
//! P1（临时词）、P2（用户词）起数据都在运行时引擎：快照段带简拼区、delta 按声母分组，
//! 本模块只是把引擎层（`UserDataUserLayer` / `UserDataTempLayer`）的 `search_abbrev` 包成
//! `UserWordRecord` 的老接口。redb 的 `USER_ABBREV` / `TEMP_ABBREV` 两张倒排表已退役（只读保留到 P4）。

use crate::user_words::UserWordRecord;
use wind_dict::DictLayer;

/// 声母串：按 `boundary` 的置位取码里的字符。
///
/// ⚠️ 必须与 `wind_engine::pinyin` 的 `abbrev_of_code` 的 `boundary != 0` 分支**逐位一致**：
/// 那边是查询侧的判据，这边是索引侧的键。故此处**不加任何额外守卫**（例如「bit0 必须置位」）。
pub fn abbrev_of(code: &str, boundary: u64) -> String {
    code.char_indices()
        .filter(|(i, _)| *i < 64 && (boundary >> i) & 1 == 1)
        .map(|(_, ch)| ch)
        .collect()
}

impl crate::Store {
    fn user_layer(&self, schema: &str) -> wind_userdata::layer::UserDataUserLayer {
        self.ensure_user_text(schema);
        wind_userdata::layer::UserDataUserLayer::new(self.runtime().data().clone(), schema)
    }

    fn temp_layer(&self, schema: &str) -> wind_userdata::layer::UserDataTempLayer {
        wind_userdata::layer::UserDataTempLayer::new(self.runtime().data().clone(), schema)
    }

    /// 按声母串检索**用户词**（含无边界词的首字符兜底组；返回超集，判据仍在引擎侧）。
    pub fn search_user_words_by_abbrev(
        &self,
        schema: &str,
        abbrev: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<UserWordRecord>> {
        Ok(records(
            self.user_layer(schema).search_abbrev(abbrev, limit),
        ))
    }

    /// 用户词里声母串**恰为** `abbrev` 的词条：只查本组、不查无边界组。`limit = 0` 不设上限。
    ///
    /// 给整句词图的用户层简拼节点用（`wind_engine::pinyin::lattice::add_store_abbrev_nodes`）：
    /// 词图节点要求真值切分，无边界组必然全被拒收——手动加过几个同首字母的词就能把名额占满。
    pub fn search_user_words_by_abbrev_exact(
        &self,
        schema: &str,
        abbrev: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<UserWordRecord>> {
        Ok(records(
            self.user_layer(schema).search_abbrev_exact(abbrev, limit),
        ))
    }

    /// 按声母串检索**临时词**。见 [`Self::search_user_words_by_abbrev`]。
    pub fn search_temp_words_by_abbrev(
        &self,
        schema: &str,
        abbrev: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<UserWordRecord>> {
        Ok(records(
            self.temp_layer(schema).search_abbrev(abbrev, limit),
        ))
    }

    /// 临时词版的 [`Self::search_user_words_by_abbrev_exact`]。
    pub fn search_temp_words_by_abbrev_exact(
        &self,
        schema: &str,
        abbrev: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<UserWordRecord>> {
        Ok(records(
            self.temp_layer(schema).search_abbrev_exact(abbrev, limit),
        ))
    }
}

/// 引擎层候选 → 记录（count / created_at 不在候选里，补 0；order 取候选的入库顺序，未折回的新词
/// 已由用户词层映射为最大值）。
fn records(cands: Vec<wind_candidate::Candidate>) -> Vec<UserWordRecord> {
    cands
        .into_iter()
        .map(|c| UserWordRecord {
            order: c.natural_order.max(0) as u32,
            code: c.code,
            text: c.text,
            weight: c.weight,
            count: 0,
            created_at: 0,
            boundary: c.boundary,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 声母提取必须与引擎侧 `abbrev_of_code` 的 boundary 分支逐位一致，不加引擎没有的守卫。
    #[test]
    fn abbrev_of_matches_engine_semantics() {
        assert_eq!(abbrev_of("nihao", 0b101), "nh");
        assert_eq!(
            abbrev_of("xianning", 0b10101),
            "xan",
            "xi|an|ning 的真值声母是 xan（不是 maximum_match 猜的 xn）"
        );
        assert_eq!(abbrev_of("nihao", 0b100), "h", "按位取，与引擎同式，不特判");
        assert_eq!(abbrev_of("", 0b1), "");
    }
}
