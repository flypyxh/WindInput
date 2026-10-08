// 焦点顶层窗口**标题**的采集判据（compat.toml 的 `title` 条件，设计稿 docs/design/compat-window-match.md
// 「标题上报」）。
//
// 标题是用户数据（网页标题、文件名、聊天对象）：
//   - 只在服务端经 CONFIG_KEY_COMPAT_TITLE_MATCH 推开时才取——规则表里没有标题规则就一次都不取，
//     DLL 每次重连从「关」起步；
//   - 取法是 InternalGetWindowText（不发 WM_GETTEXT，宿主 UI 线程卡住也不阻塞），截断到
//     kMaxTitleChars 个 UTF-16 码元，随 FocusGained 的变长段 ③ 上报；
//   - 任何级别的日志只记长度，不记原文。
//
// ⚠️ 覆盖边界：这里只有纯判据（开关值解析、截断）。Win32 取标题在 TextService.cpp
// `_QueryFocusRootWindowIdentity`，开关接收在 KeyEventSink.cpp `OnSyncConfig`，拼帧在 IPCClient.cpp
// `SendFocusGained`。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/window_title_policy_test.cpp）。
#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>

namespace wind
{
namespace window_title
{

/// 上报的标题最多这么多个 UTF-16 码元（设计稿定 256）。匹配用的是通配模式，256 足够区分窗口；
/// 再长只是把更多用户数据送出宿主进程。
inline constexpr std::size_t kMaxTitleChars = 256;

/// CONFIG_KEY_COMPAT_TITLE_MATCH 的值 → 开关。格式 enabled(u8)。
///
/// 空值（不该出现）保持现状：一条残缺的推送不该把已开的采集关掉，也不该把关着的打开。
inline bool ParseTitleMatchValue(const std::uint8_t* data, std::size_t len, bool current)
{
    if (data == nullptr || len == 0)
        return current;
    return data[0] != 0;
}

/// 这次焦点要不要取标题：只看服务端推来的开关（默认关）。单列成函数是为了让「默认不采集」
/// 这条硬要求有一个可测的落点——调用方不许绕过它直接取。
inline bool ShouldCollectTitle(bool serverEnabled)
{
    return serverEnabled;
}

/// 标题采集开关由关变开时，要不要用当前焦点补发一次 focus_gained（带类名与标题）。
///
/// 新进程首焦与 core 的握手推送是竞态：首个 focus_gained 常常赶在开关到达之前、没带标题，
/// 标题规则（含 initial_mode）对这次切进就失效了。补发条件：
///   - 本焦点会话发出的那条 focus_gained 确实没带标题（开关在首焦前就到了的话它本就带着）；
///   - 开关现在是开的、实例仍有焦点；
///   - 不在组合中（用户已在打字，初始模式已无意义，而 focus_gained 会作废组合起点锚定）。
inline bool ShouldResyncFocusOnSwitch(bool sentTitleBlind, bool enabled, bool hasFocus, bool composing)
{
    return sentTitleBlind && enabled && hasFocus && !composing;
}

/// 截断到至多 kMaxTitleChars 个 UTF-16 码元；截断点落在代理对中间时连那半个一起丢掉
/// （半个代理对转 UTF-8 会变成 U+FFFD，让以 emoji 结尾的标题与模式对不上）。
inline std::wstring_view TruncateTitle(std::wstring_view title)
{
    if (title.size() <= kMaxTitleChars)
        return title;
    std::size_t n = kMaxTitleChars;
    const wchar_t last = title[n - 1];
    if (last >= 0xD800 && last <= 0xDBFF) // 高代理在末尾：它的低代理被截掉了
        --n;
    return title.substr(0, n);
}

} // namespace window_title
} // namespace wind
