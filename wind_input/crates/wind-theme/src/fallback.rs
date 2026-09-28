//! 渲染层在主题没给颜色时的兜底色。
//!
//! 放在 wind-theme 而不是各自的渲染模块里：候选窗 / 气泡（wind-ui）与设置页的模板预览
//! （wind-webdata `appearance.previewTemplate`）必须用同一组值，否则主题缺色时预览与真实
//! 渲染对不上，而两边各写一份字面量迟早只改一处。

use crate::Rgba;

/// 候选窗底色（`[window]` 没配底色时）。
pub const WINDOW_BG: Rgba = [255, 255, 255, 255];
/// 选中候选底色（`[item.selected]` 没配底色时）。
pub const SELECTED_BG: Rgba = [230, 240, 255, 255];
/// 悬停候选底色（`[item.hover]` 没配底色时）。
pub const HOVER_BG: Rgba = [238, 242, 247, 255];
/// 注释文字色（`[comment]` 没配文字色时）；分段着色求色的 `body_fallback` 与之同源。
pub const COMMENT_TEXT: Rgba = [150, 150, 150, 255];
/// 气泡底色（调色板没有 `tooltip_bg` 时）。
pub const TOOLTIP_BG: Rgba = [60, 60, 64, 240];
/// 气泡文字色（调色板没有 `tooltip_text` 时）。
pub const TOOLTIP_TEXT: Rgba = [240, 240, 245, 255];
