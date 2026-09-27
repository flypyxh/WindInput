//! 分段着色的求色（设计 `docs/design/text-span-colors.md` §4.2、§6）。
//!
//! 片段带的是「角色 / 颜色引用」而不是 RGBA：悬停态只有 UI 知道、明暗切换时 UI 拿手里的候选
//! 直接重绘，所以颜色必须在**每次画**的时候按当前主题现求。求色是纯函数、只依赖本 crate 的
//! 类型，候选窗、自绘气泡、macOS 气泡的 runs、设置页预览四处共用同一个口径。

use std::sync::Arc;

use crate::palette::{Rgba, parse_hex};
use crate::resolve::Resolved;
use crate::rvnode::RvNode;

/// 内联色 `$[SPEC]{…}` 的颜色部分：常态一份，可选的选中态一份。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InlineColor {
    pub normal: ColorRef,
    pub selected: Option<ColorRef>,
}

/// 一个颜色引用：亮 / 暗各一个原子（单值时两侧相同）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColorRef {
    pub light: Atom,
    pub dark: Atom,
}

/// 颜色原子。十六进制在**解析时**就校验（坏值即 [`Atom::Invalid`]）；名字只能在求色时
/// 按当前主题查。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Atom {
    Rgba(Rgba),
    Name(Arc<str>),
    Invalid,
}

impl Atom {
    /// `ATOM := '#' HEX{3|6|8} | NAME`，`NAME = [a-z0-9_]+`。其余一律 [`Atom::Invalid`]。
    fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            // 恰好一个 `#`、其后全是十六进制位，才交给 parse_hex 换算。不能直接喂它：它会先
            // 削掉**全部**前导 `#`（`##C00000` 照认），且按 `from_str_radix` 逐两位解析，
            // 那个函数认前导 `+`（`#+F+F+F` 照认）——内联色要的是写法严格。
            let strict =
                matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
            return if strict {
                parse_hex(s).map_or(Atom::Invalid, Atom::Rgba)
            } else {
                Atom::Invalid
            };
        }
        let is_name = !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if is_name {
            Atom::Name(Arc::from(s))
        } else {
            Atom::Invalid
        }
    }
}

impl ColorRef {
    /// `COLOR := ATOM ( '/' ATOM )?`：单值 = 亮暗共用，`a/b` = 亮 / 暗。多于一个 `/` 即非法。
    fn parse(s: &str) -> Self {
        let mut parts = s.split('/');
        let light = Atom::parse(parts.next().unwrap_or(""));
        match (parts.next(), parts.next()) {
            (None, _) => Self {
                dark: light.clone(),
                light,
            },
            (Some(d), None) => Self {
                light,
                dark: Atom::parse(d),
            },
            (Some(_), Some(_)) => Self {
                light: Atom::Invalid,
                dark: Atom::Invalid,
            },
        }
    }
}

impl InlineColor {
    /// `SPEC := COLOR ( ',' KEY '=' COLOR )*`，记号两侧空白忽略。**不失败**：坏值落
    /// [`Atom::Invalid`]（该段按正文色显示，§4.5）；`KEY` 目前只认 `selected`，其余忽略——
    /// 前向兼容，将来加 `hover=` 时老版本不至于把整段判非法。
    pub fn parse(spec: &str) -> Self {
        let mut items = spec.split(',');
        let normal = ColorRef::parse(items.next().unwrap_or(""));
        let mut selected = None;
        for item in items {
            if let Some((k, v)) = item.split_once('=')
                && k.trim() == "selected"
            {
                selected = Some(ColorRef::parse(v));
            }
        }
        Self { normal, selected }
    }
}

/// 文字所处的状态。选中优先于悬停（与候选窗 `eff_text` 同）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextState {
    Normal,
    Selected,
    Hover,
}

fn state_patch(node: &RvNode, state: TextState) -> Option<&RvNode> {
    match state {
        TextState::Normal => None,
        TextState::Selected => node.selected.as_deref(),
        TextState::Hover => node.hover.as_deref(),
    }
}

/// 节点在某状态下的正文色：状态 patch 的文字色，未给则回退基态色（不跨态借色），
/// 基态也没给则用渲染层兜底 `fallback`。候选窗的 `eff_text` 与本模块的求色共用它。
pub fn body_color(node: &RvNode, state: TextState, fallback: Rgba) -> Rgba {
    let base = node.text_color.unwrap_or(fallback);
    state_patch(node, state)
        .and_then(|n| n.text_color)
        .unwrap_or(base)
}

/// 按当前明暗解析一个颜色引用。`None` = 非法或当前主题查不到（调用方回落正文色）。
///
/// 气泡里的名字先查 `tooltip_<名>`、查不到再查同名：为白底调的亮档色放进深色气泡会看不清
/// （§5.4）。`transparent` 占着宽度却看不见，只会是误用，按查不到处理（§4.5）。
///
/// 解析出来 alpha 为 0 的一律按查不到处理（字面 `#RRGGBB00`、alpha 为 0 的 token 同理）：
/// 文字占着宽度却看不见，只会是误用，与 `transparent` 同一口径（§4.5）。
fn resolve_ref(theme: &Resolved, is_tooltip: bool, r: &ColorRef) -> Option<Rgba> {
    let c = match if theme.is_dark { &r.dark } else { &r.light } {
        Atom::Rgba(c) => Some(*c),
        Atom::Invalid => None,
        Atom::Name(n) if &**n == "transparent" => None,
        Atom::Name(n) => is_tooltip
            .then(|| theme.palette.get(&format!("tooltip_{n}")))
            .flatten()
            .or_else(|| theme.palette.get(&**n))
            .copied(),
    };
    c.filter(|c| c[3] != 0)
}

/// 一个片段该用的颜色（§6.2）。
///
/// - `node`：`views.comment` 或 `views.tooltip`；`is_tooltip` 决定内联色名字的作用域查找。
/// - `body_fallback`：节点未配正文色时的渲染层兜底（候选窗注释是 `[150,150,150,255]`，
///   气泡是 palette `tooltip_text`）。
/// - `role`：片段角色（归一后的变量名或 `title` / `literal`）；`None` = 无角色（未知变量回显）。
/// - `in_title`：段名里产出的片段，角色未配色时回落 `title` 角色。
///
/// 顺序：① 内联色（选中态有 `selected=` 用它；否则状态改了正文色就回落正文色；否则常态值）
/// → ② 该状态单列的角色色 → 规则 3（状态改了正文色 ⇒ 正文色）→ ③ 常态角色色 →
/// ④ 段名里的变量回落 `title` → ⑤ 正文色。
///
/// 「状态改了正文色」按**有效色值**比（该状态正文色 ≠ 常态正文色），不按主题写没写
/// `[comment.selected] color`：派生主题常把 base 的选中色写回常态色，按「写没写」判会让它
/// 在选中态无端丢掉全部分色（§6.3，2026-09-27 确认）。
#[allow(clippy::too_many_arguments)]
pub fn span_color(
    theme: &Resolved,
    node: &RvNode,
    is_tooltip: bool,
    state: TextState,
    body_fallback: Rgba,
    role: Option<&str>,
    in_title: bool,
    inline: Option<&InlineColor>,
) -> Rgba {
    let body = body_color(node, state, body_fallback);
    let state_changed_body =
        state != TextState::Normal && body != body_color(node, TextState::Normal, body_fallback);
    if let Some(ic) = inline {
        if state == TextState::Selected
            && let Some(sel) = &ic.selected
        {
            return resolve_ref(theme, is_tooltip, sel).unwrap_or(body);
        }
        if state_changed_body {
            return body;
        }
        return resolve_ref(theme, is_tooltip, &ic.normal).unwrap_or(body);
    }
    if let Some(r) = role
        && let Some(c) = state_patch(node, state).and_then(|n| n.roles.get(r))
    {
        return *c;
    }
    if state_changed_body {
        return body;
    }
    if let Some(r) = role {
        if let Some(c) = node.roles.get(r) {
            return *c;
        }
        if in_title && let Some(c) = node.roles.get("title") {
            return *c;
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BODY: Rgba = [150, 150, 150, 255];
    const RED: Rgba = [255, 0, 0, 255];
    const GREEN: Rgba = [0, 255, 0, 255];
    const BLUE: Rgba = [0, 0, 255, 255];
    const SEL_BODY: Rgba = [255, 255, 255, 255];

    fn name(s: &str) -> Atom {
        Atom::Name(Arc::from(s))
    }

    fn both(a: Atom) -> ColorRef {
        ColorRef {
            light: a.clone(),
            dark: a,
        }
    }

    // ---------------- 解析 ----------------

    #[test]
    fn parse_literal_name_and_light_dark_pair() {
        assert_eq!(
            InlineColor::parse("#C00000/#FF8080").normal,
            ColorRef {
                light: Atom::Rgba([0xC0, 0, 0, 255]),
                dark: Atom::Rgba([0xFF, 0x80, 0x80, 255]),
            }
        );
        assert_eq!(InlineColor::parse(" accent ").normal, both(name("accent")));
        assert_eq!(
            InlineColor::parse("text/accent").normal,
            ColorRef {
                light: name("text"),
                dark: name("accent"),
            }
        );
        assert_eq!(
            InlineColor::parse("#F80").normal,
            both(Atom::Rgba([0xFF, 0x88, 0, 255]))
        );
    }

    #[test]
    fn parse_selected_variant_and_ignores_unknown_keys() {
        let ic = InlineColor::parse("text_dim , selected = on_accent, hover=accent");
        assert_eq!(ic.normal, both(name("text_dim")));
        assert_eq!(ic.selected, Some(both(name("on_accent"))));
        assert_eq!(InlineColor::parse("accent,hover=text").selected, None);
    }

    /// 坏值不让解析失败，落 Invalid：语法成立、颜色非法按正文色显示（§4.4、§4.5）。
    #[test]
    fn bad_values_are_invalid_atoms() {
        for spec in ["", "红", "#GG0000", "#12", "Accent", "a/b/c", "bad-name"] {
            assert_eq!(
                InlineColor::parse(spec).normal,
                both(Atom::Invalid),
                "{spec:?}"
            );
        }
        // 裸 6 位不带 # 是名字（查不到即回落），不是颜色。
        assert_eq!(InlineColor::parse("ff0000").normal, both(name("ff0000")));
        // 写法严格：多个 `#`、`from_str_radix` 认的前导 `+`、夹空格都不是颜色。
        for spec in ["##C00000", "#+F+F+F", "#+FF0000", "# FF0000", "#FF 000"] {
            assert_eq!(
                InlineColor::parse(spec).normal,
                both(Atom::Invalid),
                "{spec:?}"
            );
        }
    }

    // ---------------- 求色 ----------------

    fn theme(dark: bool) -> Resolved {
        let mut palette = HashMap::new();
        palette.insert("accent".to_string(), RED);
        palette.insert("error".to_string(), RED);
        palette.insert("tooltip_error".to_string(), GREEN);
        palette.insert("text".to_string(), BLUE);
        palette.insert("tooltip_text".to_string(), SEL_BODY);
        Resolved {
            is_dark: dark,
            palette,
            views: Default::default(),
            behavior: Default::default(),
            resources: Default::default(),
            asset_dirs: Vec::new(),
            langbar_text: Default::default(),
        }
    }

    fn node() -> RvNode {
        RvNode::default()
    }

    fn with_roles(mut n: RvNode, roles: &[(&str, Rgba)]) -> RvNode {
        n.roles = roles.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        n
    }

    fn selected(mut n: RvNode, patch: RvNode) -> RvNode {
        n.selected = Some(Box::new(patch));
        n
    }

    fn body_patch(c: Rgba) -> RvNode {
        RvNode {
            text_color: Some(c),
            ..Default::default()
        }
    }

    fn sc(
        t: &Resolved,
        n: &RvNode,
        tip: bool,
        st: TextState,
        role: Option<&str>,
        in_title: bool,
        inline: Option<&str>,
    ) -> Rgba {
        let ic = inline.map(InlineColor::parse);
        span_color(t, n, tip, st, BODY, role, in_title, ic.as_ref())
    }

    use TextState::{Hover, Normal, Selected};

    #[test]
    fn no_role_no_inline_is_body() {
        let t = theme(false);
        assert_eq!(sc(&t, &node(), false, Normal, None, false, None), BODY);
        assert_eq!(
            sc(&t, &node(), false, Normal, Some("pinyin"), false, None),
            BODY
        );
    }

    #[test]
    fn inline_literal_and_names_follow_light_dark() {
        let (l, d) = (theme(false), theme(true));
        let n = node();
        let spec = Some("#C00000/#FF8080");
        assert_eq!(
            sc(&l, &n, false, Normal, None, false, spec),
            [0xC0, 0, 0, 255]
        );
        assert_eq!(
            sc(&d, &n, false, Normal, None, false, spec),
            [0xFF, 0x80, 0x80, 255]
        );
        assert_eq!(
            sc(&l, &n, false, Normal, None, false, Some("text/accent")),
            BLUE
        );
        assert_eq!(
            sc(&d, &n, false, Normal, None, false, Some("text/accent")),
            RED
        );
    }

    /// 内联色优先于角色色；非法 / 查不到 / transparent 回落正文色（不借角色色）。
    #[test]
    fn inline_wins_and_bad_inline_falls_back_to_body() {
        let t = theme(false);
        let n = with_roles(node(), &[("pinyin", GREEN)]);
        assert_eq!(
            sc(&t, &n, false, Normal, Some("pinyin"), false, Some("accent")),
            RED
        );
        for bad in ["nope", "#GG0000", "transparent", "红"] {
            assert_eq!(
                sc(&t, &n, false, Normal, Some("pinyin"), false, Some(bad)),
                BODY,
                "{bad}"
            );
        }
    }

    /// 气泡里名字先查 `tooltip_<名>`，查不到再查同名；`#hex` 不受作用域影响。
    #[test]
    fn tooltip_scope_lookup() {
        let t = theme(false);
        let n = node();
        assert_eq!(sc(&t, &n, true, Normal, None, false, Some("error")), GREEN);
        assert_eq!(sc(&t, &n, false, Normal, None, false, Some("error")), RED);
        assert_eq!(
            sc(&t, &n, true, Normal, None, false, Some("text")),
            SEL_BODY,
            "$[text] 在气泡里取 tooltip_text"
        );
        assert_eq!(
            sc(&t, &n, true, Normal, None, false, Some("accent")),
            RED,
            "没有 tooltip_accent 时回落 accent"
        );
    }

    /// 规则 3：选中态改了正文色 ⇒ 未单列的角色与内联色回落该态正文色；
    /// `selected=` 与 `[comment.selected.roles]` 单列的仍生效。
    #[test]
    fn rule3_falls_back_when_state_changes_body() {
        let t = theme(false);
        let mut patch = body_patch(SEL_BODY);
        patch.roles = [("code_rev".to_string(), BLUE)].into();
        let n = selected(
            with_roles(node(), &[("pinyin", GREEN), ("code_rev", GREEN)]),
            patch,
        );
        assert_eq!(
            sc(&t, &n, false, Selected, Some("pinyin"), false, None),
            SEL_BODY
        );
        assert_eq!(
            sc(&t, &n, false, Selected, Some("code_rev"), false, None),
            BLUE
        );
        assert_eq!(
            sc(&t, &n, false, Selected, None, false, Some("accent")),
            SEL_BODY
        );
        assert_eq!(
            sc(
                &t,
                &n,
                false,
                Selected,
                None,
                false,
                Some("accent,selected=text")
            ),
            BLUE
        );
        // 常态不受影响。
        assert_eq!(
            sc(&t, &n, false, Normal, Some("pinyin"), false, None),
            GREEN
        );
        assert_eq!(sc(&t, &n, false, Normal, None, false, Some("accent")), RED);
    }

    /// 「改了」按有效值判：选中态正文色写成与常态同值 ⇒ 不回落。
    #[test]
    fn rule3_compares_effective_values_not_presence() {
        let t = theme(false);
        let n = selected(with_roles(node(), &[("pinyin", GREEN)]), body_patch(BODY));
        assert_eq!(
            sc(&t, &n, false, Selected, Some("pinyin"), false, None),
            GREEN
        );
        assert_eq!(
            sc(&t, &n, false, Selected, None, false, Some("accent")),
            RED
        );
    }

    /// 亮暗各判：只在暗色下改了选中正文色，就只有暗色回落。
    #[test]
    fn rule3_is_judged_per_light_dark() {
        // 同一个节点在亮 / 暗两份 Resolved 里各求一次（节点本身已按明暗解析过）。
        let light = selected(with_roles(node(), &[("pinyin", GREEN)]), body_patch(BODY));
        let dark = selected(
            with_roles(node(), &[("pinyin", GREEN)]),
            body_patch(SEL_BODY),
        );
        assert_eq!(
            sc(
                &theme(false),
                &light,
                false,
                Selected,
                Some("pinyin"),
                false,
                None
            ),
            GREEN
        );
        assert_eq!(
            sc(
                &theme(true),
                &dark,
                false,
                Selected,
                Some("pinyin"),
                false,
                None
            ),
            SEL_BODY
        );
    }

    /// 悬停与选中同理；选中态的 `selected=` 不作用于悬停。
    #[test]
    fn hover_state() {
        let t = theme(false);
        let mut n = with_roles(node(), &[("pinyin", GREEN)]);
        n.hover = Some(Box::new(body_patch(SEL_BODY)));
        assert_eq!(
            sc(&t, &n, false, Hover, Some("pinyin"), false, None),
            SEL_BODY
        );
        assert_eq!(
            sc(
                &t,
                &n,
                false,
                Hover,
                None,
                false,
                Some("accent,selected=text")
            ),
            SEL_BODY
        );
        let plain_hover = with_roles(node(), &[("pinyin", GREEN)]);
        assert_eq!(
            sc(
                &t,
                &plain_hover,
                false,
                Hover,
                None,
                false,
                Some("accent,selected=text")
            ),
            RED
        );
    }

    /// 段名里的变量回落 `title`；无角色（未知变量回显）不回落；段外不回落。
    #[test]
    fn in_title_falls_back_to_title_role() {
        let t = theme(false);
        let n = with_roles(node(), &[("title", BLUE), ("code_source", GREEN)]);
        let bare = with_roles(node(), &[("title", BLUE)]);
        assert_eq!(
            sc(&t, &n, true, Normal, Some("code_source"), true, None),
            GREEN
        );
        assert_eq!(
            sc(&t, &bare, true, Normal, Some("code_source"), true, None),
            BLUE
        );
        assert_eq!(sc(&t, &bare, true, Normal, None, true, None), BODY);
        assert_eq!(
            sc(&t, &bare, true, Normal, Some("code_source"), false, None),
            BODY
        );
    }

    // ---------------- 标准色契约（§5.4）----------------

    fn factory(name: &str, dark: bool) -> Resolved {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/themes");
        crate::load_resolved(&dir, name, dark).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    const FACTORY: &[&str] = &[
        "_base",
        "_qingfeng",
        "default",
        "amber",
        "jade",
        "violet",
        "msime",
    ];

    /// 契约表：名字 → `_base` 的（亮, 暗）值。表即断言。
    const CONTRACT: &[(&str, &str, &str)] = &[
        ("success", "#1E8E3E", "#81C995"),
        ("warning", "#B06000", "#FDD663"),
        ("error", "#D93025", "#F28B82"),
        ("info", "#1A73E8", "#8AB4F8"),
        ("tooltip_text_dim", "#BDBDBD", "#BDBDBD"),
        ("tooltip_text_hint", "#A0A0A0", "#A0A0A0"),
        ("tooltip_accent", "#8AB4F8", "#8AB4F8"),
        ("tooltip_accent_text", "#8AB4F8", "#8AB4F8"),
        ("tooltip_success", "#81C995", "#81C995"),
        ("tooltip_warning", "#FDD663", "#FDD663"),
        ("tooltip_error", "#F28B82", "#F28B82"),
        ("tooltip_info", "#8AB4F8", "#8AB4F8"),
    ];

    /// 候选窗用的契约名（已有 + 新增）；每个在气泡里都有 `tooltip_<名>`。
    const NAMES: &[&str] = &[
        "text",
        "text_dim",
        "text_hint",
        "accent",
        "on_accent",
        "selection_text",
        "accent_text",
        "success",
        "warning",
        "error",
        "info",
    ];

    /// 全部出厂主题 × 亮暗：契约名与其 `tooltip_*` 都能解析。
    #[test]
    fn every_factory_theme_resolves_the_contract() {
        for name in FACTORY {
            for dark in [false, true] {
                let t = factory(name, dark);
                for n in NAMES {
                    assert!(t.palette.contains_key(*n), "{name} dark={dark}: 缺 {n}");
                    let tip = format!("tooltip_{n}");
                    assert!(t.palette.contains_key(&tip), "{name} dark={dark}: 缺 {tip}");
                }
                // 出厂节点不配角色：零变化的前提之一。
                assert!(t.views.comment.roles.is_empty());
            }
        }
    }

    #[test]
    fn base_contract_values() {
        for dark in [false, true] {
            let t = factory("_base", dark);
            for (n, l, d) in CONTRACT {
                let want = parse_hex(if dark { d } else { l }).unwrap();
                assert_eq!(t.palette[*n], want, "_base dark={dark}: {n}");
            }
            assert_eq!(t.palette["accent_text"], t.palette["accent"]);
            assert_eq!(t.palette["tooltip_on_accent"], t.palette["tooltip_text"]);
            assert_eq!(
                t.palette["tooltip_selection_text"],
                t.palette["tooltip_text"]
            );
        }
    }

    /// 清风系各自覆盖 tooltip_accent（本主题 accent_text 的暗档值）；msime 沿用 _base 的蓝。
    #[test]
    fn tooltip_accent_follows_each_theme() {
        for (name, want) in [
            ("_qingfeng", "#60a5fa"),
            ("default", "#60a5fa"),
            ("amber", "#fbbf24"),
            ("jade", "#34d399"),
            ("violet", "#a78bfa"),
            ("msime", "#8AB4F8"),
        ] {
            for dark in [false, true] {
                let t = factory(name, dark);
                assert_eq!(
                    t.palette["tooltip_accent"],
                    parse_hex(want).unwrap(),
                    "{name}"
                );
                assert_eq!(
                    t.palette["tooltip_accent_text"],
                    parse_hex(want).unwrap(),
                    "{name}"
                );
            }
            if name != "msime" {
                assert_eq!(
                    factory(name, true).palette["accent_text"],
                    parse_hex(want).unwrap(),
                    "{name}: tooltip_accent 应等于 accent_text 暗档"
                );
            }
        }
    }

    /// 全透明色（字面 `#RRGGBB00`、alpha 为 0 的 token、`transparent`）一律按正文色。
    #[test]
    fn fully_transparent_colors_fall_back_to_body() {
        let mut t = theme(false);
        t.palette.insert("clear".to_string(), [9, 9, 9, 0]);
        t.palette.insert("tooltip_ghost".to_string(), [9, 9, 9, 0]);
        let n = node();
        for spec in ["#FF000000", "clear", "transparent", "#FF000000/#00FF0000"] {
            assert_eq!(
                sc(&t, &n, false, Normal, None, false, Some(spec)),
                BODY,
                "{spec}"
            );
        }
        assert_eq!(sc(&t, &n, true, Normal, None, false, Some("ghost")), BODY);
        // 半透明不受影响。
        assert_eq!(
            sc(&t, &n, false, Normal, None, false, Some("#FF000080")),
            [255, 0, 0, 0x80]
        );
    }

    /// 节点自带正文色时，兜底不参与。
    #[test]
    fn node_text_color_overrides_fallback() {
        let t = theme(false);
        let n = RvNode {
            text_color: Some(BLUE),
            ..Default::default()
        };
        assert_eq!(sc(&t, &n, false, Normal, Some("x"), false, None), BLUE);
        assert_eq!(
            body_color(&n, Selected, BODY),
            BLUE,
            "选中 patch 未给色 → 基态"
        );
    }

    // ───────────── 与主题编辑器的共用期望表（span-roles 测试主题）─────────────
    //
    // 主题编辑器是引擎的「前端孪生」，它的 roles 求值与求色必须与这里逐项相同。两边共用
    // `testdata/themes/span-roles/theme.toml` 这份测试数据；引擎把求值结果导出成
    // `expected.json` 检入，编辑器在主仓在场时读它逐项断言。期望值因此出自引擎、不是手写。
    // 引擎行为有意改变时 `WIND_THEME_BLESS=1 cargo test -p wind-theme span_roles_expected` 重录。

    fn hex8(c: Rgba) -> String {
        format!("\"#{:02X}{:02X}{:02X}{:02X}\"", c[0], c[1], c[2], c[3])
    }

    fn roles_json(r: &HashMap<String, Rgba>) -> String {
        let mut keys: Vec<&String> = r.keys().collect();
        keys.sort();
        let items: Vec<String> = keys
            .iter()
            .map(|k| format!("\"{k}\": {}", hex8(r[*k])))
            .collect();
        format!("{{{}}}", items.join(", "))
    }

    fn state_json(n: Option<&RvNode>) -> String {
        match n {
            None => "null".into(),
            Some(n) => format!(
                "{{\"text\": {}, \"roles\": {}}}",
                n.text_color.map_or("null".into(), hex8),
                roles_json(&n.roles)
            ),
        }
    }

    const COMMENT_FALLBACK: Rgba = [150, 150, 150, 255];
    const PROBE_ROLES: &[Option<&str>] = &[
        Some("code_rev"),
        Some("pinyin"),
        Some("literal"),
        Some("chaizi"),
        Some("dict"),
        Some("title"),
        Some("code_source"),
        Some("readings"),
        None,
    ];

    fn span_roles_expected() -> String {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dirs = [
            root.join("testdata/themes"),
            root.join("../../../data/themes"),
        ];
        let mut modes = Vec::new();
        for (mode, dark) in [("light", false), ("dark", true)] {
            let t = crate::load_resolved_dirs(&dirs, "span-roles", dark).unwrap();
            let c = &t.views.comment;
            let tip = t.views.tooltip.clone().unwrap_or_default();
            let tip_fb = t.color("tooltip_text", [255, 255, 255, 255]);
            let mut spans = Vec::new();
            for (st_name, st) in [
                ("normal", TextState::Normal),
                ("selected", TextState::Selected),
                ("hover", TextState::Hover),
            ] {
                for role in PROBE_ROLES {
                    let col = span_color(&t, c, false, st, COMMENT_FALLBACK, *role, false, None);
                    spans.push(format!(
                        "{{\"node\": \"comment\", \"state\": \"{st_name}\", \"role\": {}, \"in_title\": false, \"color\": {}}}",
                        role.map_or("null".into(), |r| format!("\"{r}\"")),
                        hex8(col)
                    ));
                }
            }
            for in_title in [false, true] {
                for role in PROBE_ROLES {
                    let col = span_color(
                        &t,
                        &tip,
                        true,
                        TextState::Normal,
                        tip_fb,
                        *role,
                        in_title,
                        None,
                    );
                    spans.push(format!(
                        "{{\"node\": \"tooltip\", \"state\": \"normal\", \"role\": {}, \"in_title\": {in_title}, \"color\": {}}}",
                        role.map_or("null".into(), |r| format!("\"{r}\"")),
                        hex8(col)
                    ));
                }
            }
            modes.push(format!(
                "  \"{mode}\": {{\n    \"comment\": {{\"text\": {}, \"roles\": {}, \"selected\": {}, \"hover\": {}}},\n    \"tooltip\": {{\"text\": {}, \"roles\": {}}},\n    \"span\": [\n      {}\n    ]\n  }}",
                c.text_color.map_or("null".into(), hex8),
                roles_json(&c.roles),
                state_json(c.selected.as_deref()),
                state_json(c.hover.as_deref()),
                hex8(tip_fb),
                roles_json(&tip.roles),
                spans.join(",\n      ")
            ));
        }
        format!("{{\n{}\n}}\n", modes.join(",\n"))
    }

    #[test]
    fn span_roles_expected_table_is_current() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/themes/span-roles/expected.json");
        let got = span_roles_expected();
        if std::env::var_os("WIND_THEME_BLESS").is_some() {
            std::fs::write(&path, &got).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path).expect("读 expected.json");
        assert_eq!(
            got, want,
            "引擎求值与检入的期望表不一致；有意改变时 WIND_THEME_BLESS=1 重录"
        );
    }
}
