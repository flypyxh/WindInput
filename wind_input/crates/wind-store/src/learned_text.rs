//! 学到的数据的文本绑定规则（设计 §2.1 / §2.4，P5）：temp / draft / freq / assoc 落在
//! `data/<schema>/` 下的表格文本，由引擎按规则自动绑定。

use std::path::Path;
use wind_userdata::engine::{TextBinding, TextFormat};
use wind_userdata::record::Target;
use wind_userdata::text::codec;

pub(crate) const LEARNED_TARGETS: [Target; 4] =
    [Target::Temp, Target::Draft, Target::Freq, Target::Assoc];

/// 某 (目标, 方案) 的文本绑定；不是学到的数据 ⇒ None。
pub(crate) fn learned_binding(
    data_dir: &Path,
    target: Target,
    schema: &str,
) -> Option<TextBinding> {
    let (file, spec, syllable) = match target {
        Target::Temp => ("temp.dict.yaml", &codec::TEMP, schema == "pinyin"),
        Target::Draft => ("draft.dict.yaml", &codec::DRAFT, schema == "pinyin"),
        Target::Freq => ("freq.table.yaml", &codec::FREQ, schema == "pinyin"),
        Target::Assoc => ("assoc.table.yaml", &codec::ASSOC, false),
        _ => return None,
    };
    Some(TextBinding {
        path: data_dir.join(schema).join(file),
        name: format!("{schema}.{}", spec.kind),
        lowercase: false,
        syllable_codes: syllable,
        format: TextFormat::Table(spec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_files_live_under_data_schema_dir() {
        let b = learned_binding(Path::new("/u/data"), Target::Assoc, "pinyin").unwrap();
        assert_eq!(b.path, Path::new("/u/data/pinyin/assoc.table.yaml"));
        assert!(!b.syllable_codes);
        let b = learned_binding(Path::new("/u/data"), Target::Temp, "pinyin").unwrap();
        assert_eq!(b.path, Path::new("/u/data/pinyin/temp.dict.yaml"));
        assert!(b.syllable_codes);
        assert!(learned_binding(Path::new("/u/data"), Target::User, "pinyin").is_none());
        assert!(learned_binding(Path::new("/u/data"), Target::Shadow, "pinyin").is_none());
    }
}
