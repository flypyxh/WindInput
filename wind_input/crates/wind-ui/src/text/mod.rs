//! 文本渲染后端
//!
//! 与 Go 版本 `wind_input/internal/ui/text_drawer*.go` 对齐。

pub mod dwrite;

// 按脚本切字体段的纯逻辑（不依赖任何平台 API），故与后端并列而非藏在 dwrite 里：
// 它的区间表与继承规则要能在 CI 的 Linux test job 上跑到——同 `pua_runs` 的先例。
pub mod script;

// 配置字体名 → 渲染端 family 的判定顺序（纯逻辑，平台能力以闭包注入），同上理由。
pub mod font_resolve;

// macOS：CoreText 真字形后端，提供与 dwrite 同契约的 TextRenderer（dwrite.rs 在
// target_os="macos" 下 re-export 它），让候选窗在 mac 上渲染真实汉字（非 mock 桩）。
#[cfg(target_os = "macos")]
pub mod coretext;
