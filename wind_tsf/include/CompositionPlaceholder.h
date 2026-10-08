// 组合区**占位字符**的识别与兜底取值（GH#175）。
//
// 背景：清风在宿主里挂一个「看不见内容的组合」——非嵌入模式（编码由候选窗自绘）、联想态、
// 直达热键进入的模式等都靠它让宿主继续转发按键、并给 GetTextExt 一个非零长度的 range。
// 占位历来是一个空格（Rust 侧 `wind_bridge::handler::COMPOSITION_PLACEHOLDER`）。
//
// 问题：浏览器里的**受控输入**会把组合中的 value 拿去 trim。番茄小说搜索框是 React 17 受控
// `<input>`，`onChange: e => setV(e.target.value.trim())`：组合中 value 带着那个空格 →
// trim 后 state ≠ DOM → React 回写 value → 浏览器终止组合 → 一个字都上不了屏。
//
// 修法：占位字符改由**服务端按应用决定**（compat.toml 的 `composition_placeholder`，
// "space" | "zwsp" | "blank"，出厂只给浏览器开 zwsp）。U+200B（ZWSP）不被 JS `trim()` / `\s` 当空白。
// 第三档 U+2800（BRAILLE PATTERN BLANK）：同样不算空白，且**有宽度**——钉钉在线表格选中单元格
// 直接打字时靠「出现了内容」才进入编辑态，零宽的 ZWSP 被当成没有内容，组合停在左上角的代理
// 输入框里、候选窗定位错；它要的是看得见宽度又不会被 trim 掉的字符。
// 本 DLL **只识别、不决策**：
//   - 服务端自己发出的占位（联想态、加词、非嵌入模式的编码替身）直接就是那个字符；
//   - 组合文本为空时本 DLL 补的兜底占位，用服务端经 CONFIG_KEY_COMPOSITION_PLACEHOLDER
//     按本客户端 pid 下发的那一个（默认空格）；
//   - 判「这段组合是不是占位」时空格 / ZWSP / 盲文空白都认（光标落在占位前等）。
//
// ⚠️ 覆盖边界：这里只有识别与取值。下发怎么收、兜底在哪一层补，见 KeyEventSink.cpp
// `OnSyncConfig` 与 TextService.cpp `CUpdateCompositionEditSession`。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/composition_placeholder_test.cpp）。
#pragma once

#include <cstdint>
#include <string_view>

namespace wind
{
namespace placeholder
{

/// 空格占位（历史行为、默认值）。与 Rust 侧 `COMPOSITION_PLACEHOLDER` 一致。
inline constexpr wchar_t kSpacePlaceholder[] = L" ";

/// 零宽空格占位：U+200B ZERO WIDTH SPACE。与 Rust 侧 `PlaceholderChar::Zwsp` 一致。
inline constexpr wchar_t kZwspPlaceholder[] = L"\u200B";

/// 盲文空白占位：U+2800 BRAILLE PATTERN BLANK。与 Rust 侧 `PlaceholderChar::Blank` 一致。
inline constexpr wchar_t kBlankPlaceholder[] = L"\u2800";

/// CONFIG_KEY_COMPOSITION_PLACEHOLDER 的值 → 兜底占位字符串（长度恒为 1）。
///
/// 1 = ZWSP、2 = 盲文空白；0 与**任何认不出的值**都回落空格——将来服务端加了新档而本 DLL
/// 是旧版时，保持历史行为最安全。（只认 0/1 的旧版 DLL 收到 2 就是走这条回落：兜底占位仍是
/// 空格，与没配规则一样。新加档位只追加新值、不复用旧值，这条回落才一直成立。）
inline const wchar_t* PlaceholderForKind(std::uint8_t kind)
{
    switch (kind)
    {
    case 1:
        return kZwspPlaceholder;
    case 2:
        return kBlankPlaceholder;
    default:
        return kSpacePlaceholder;
    }
}

/// 这段组合文本是不是占位（空格 / ZWSP / 盲文空白），而非用户可见的编码。
///
/// 空串**不算**：它表示「需要兜底占位」，由写组合区的那一层自己补。
inline bool IsPlaceholderText(std::wstring_view text)
{
    return text == std::wstring_view(kSpacePlaceholder) || text == std::wstring_view(kZwspPlaceholder) ||
           text == std::wstring_view(kBlankPlaceholder);
}

} // namespace placeholder
} // namespace wind
