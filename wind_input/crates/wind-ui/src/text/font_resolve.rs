//! 配置里的字体名 → 渲染端认得的 family 名（+ 名字里带的字重）。
//!
//! 与平台无关的**判定顺序**住在这里，平台能力（「系统里有没有这个 family」「按 face 全名
//! 找字体」）由调用方以闭包注入——同 `script.rs` 的先例：判定要能在 Linux CI 上跑到。
//!
//! 背景见 `wind_config::font_name`（论坛 t95 / 看板 A2-1）：存量配置与主题里躺着
//! GDI face name（「思源宋体 SemiBold」），DirectWrite `FindFamilyName` 查不到，
//! 此前一律静默回落默认字体。

use wind_config::font_name::split_weight_suffix;

/// 名字是怎么解析出来的。决定调用方记什么日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontNameSource {
    /// 原串就是系统里的 family 名。
    Exact,
    /// 剥掉结尾字重词后是系统里的 family 名（旧 GDI face name 的主路径）。
    WeightSuffix,
    /// 按字体 face 的全名 / GDI 名在系统字体集里找到的（覆盖字重词表之外的写法）。
    FaceName,
    /// 系统里确实没有。`family` 原样保留——渲染端照旧会静默回落，调用方要 warn。
    Missing,
    /// 查不了（后端没有系统字体集可问）。原样保留，不 warn。
    Unknown,
}

/// 解析结果。`weight` = 0 表示名字里没带字重。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFont {
    pub family: String,
    pub weight: i32,
    pub source: FontNameSource,
}

/// 按固定顺序解析一个字体名：
/// 1. 原串是 family → 原样用（**写对的名字绝不改写**，`Arial Black` 这类名字里带字重词的
///    合法 family 就靠这一步保住）；
/// 2. 剥结尾字重词，剩下的是 family → 用它 + 那个字重；
/// 3. `find_face` 按 face 全名在字体集里找 → 用它报的 family + 字重；
/// 4. 都不中 → `Missing`，原样保留。
///
/// `exists` 返回 `None`（查不了）时直接 `Unknown` 原样返回：拿不到系统字体集就无从判断
/// 拆分是否正确，宁可不动也不猜。
///
/// ⚠️ 第 2、3 步拆出的 family **必须再过一次** `exists`（第 2 步在这里过；第 3 步由
/// `find_face` 的实现保证它报的是字体集里的真实 family），否则会把「用户写错的名字」
/// 静默纠成另一个字体——那是把静默换个地方继续静默。
pub fn resolve_font_name(
    name: &str,
    exists: impl Fn(&str) -> Option<bool>,
    find_face: impl FnOnce(&str) -> Option<(String, i32)>,
) -> ResolvedFont {
    let name = name.trim();
    let keep = |source| ResolvedFont {
        family: name.to_string(),
        weight: 0,
        source,
    };
    match exists(name) {
        None => return keep(FontNameSource::Unknown),
        Some(true) => return keep(FontNameSource::Exact),
        Some(false) => {}
    }
    if let Some((family, weight)) = split_weight_suffix(name)
        && exists(family) == Some(true)
    {
        return ResolvedFont {
            family: family.to_string(),
            weight,
            source: FontNameSource::WeightSuffix,
        };
    }
    if let Some((family, weight)) = find_face(name) {
        return ResolvedFont {
            family,
            weight,
            source: FontNameSource::FaceName,
        };
    }
    keep(FontNameSource::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(families: &'static [&'static str]) -> impl Fn(&str) -> Option<bool> {
        move |n: &str| Some(families.iter().any(|f| f.eq_ignore_ascii_case(n)))
    }

    fn no_face(_: &str) -> Option<(String, i32)> {
        None
    }

    #[test]
    fn an_existing_family_is_never_rewritten() {
        // 「Arial Black」本身是 family：不能被拆成 Arial + 900。
        let r = resolve_font_name("Arial Black", sys(&["Arial", "Arial Black"]), no_face);
        assert_eq!(r.family, "Arial Black");
        assert_eq!(r.weight, 0);
        assert_eq!(r.source, FontNameSource::Exact);
    }

    #[test]
    fn legacy_gdi_face_name_becomes_family_plus_weight() {
        let r = resolve_font_name("思源宋体 SemiBold", sys(&["思源宋体"]), no_face);
        assert_eq!(
            r,
            ResolvedFont {
                family: "思源宋体".into(),
                weight: 600,
                source: FontNameSource::WeightSuffix
            }
        );
    }

    #[test]
    fn stripped_family_must_exist_or_face_lookup_takes_over() {
        // 剥出来的「文渊宋体」不在系统里 → 不许采用，交给 face 全名查找。
        let r = resolve_font_name("文渊宋体 Bold", sys(&["WenYuan Serif SC"]), |n| {
            (n == "文渊宋体 Bold").then(|| ("WenYuan Serif SC".to_string(), 700))
        });
        assert_eq!(r.family, "WenYuan Serif SC");
        assert_eq!(r.weight, 700);
        assert_eq!(r.source, FontNameSource::FaceName);
    }

    #[test]
    fn a_name_nobody_knows_stays_missing_and_untouched() {
        let r = resolve_font_name(" 不存在 Medium ", sys(&["不存在的别的"]), no_face);
        assert_eq!(r.family, "不存在 Medium");
        assert_eq!(r.source, FontNameSource::Missing);
    }

    #[test]
    fn without_a_font_collection_nothing_is_guessed() {
        let r = resolve_font_name(
            "思源宋体 SemiBold",
            |_| None,
            |_| panic!("查不了时不该去扫字体集"),
        );
        assert_eq!(r.family, "思源宋体 SemiBold");
        assert_eq!(r.source, FontNameSource::Unknown);
    }
}
