// 「候选可见时要不要把 SESSION 热键表注册成系统级热键」的**纯判据**。
//
// 从 `CTextService::_RegisterCandidateHotkeys` 里抽出来只为让它可测。表里的键（置顶 / 删除 /
// `input.alt_commit` 的 Alt+0..9 与 Alt+Space）在服务端**全部只在中文模式认领**；英文模式
// 下也会有候选（英文补全），此时注册了就是把宿主的 Alt 组合吞掉、服务端又不接，键凭空消失。
// 与 `OnTestKeyDown` 里 session 热键的退路分支同一判据（chineseMode && 有会话）。
//
// ⚠️ 覆盖边界：这里只有判据。RegisterHotKey 本身与调用时机都在 TextService.cpp。
#pragma once

#include <cstdint>

#include "ResponseType.h"

namespace wind
{
namespace candhotkey
{

/// 是否注册候选热键。
///
/// - `threadFocus` / `processForeground`：多进程 IME 实例竞争同一组热键（1409），只让真正
///   拥有前台窗口的实例注册。
/// - `chineseMode`：英文模式不注册，见文件头。
inline bool ShouldRegister(bool threadFocus, bool processForeground, bool chineseMode)
{
    return threadFocus && processForeground && chineseMode;
}

enum class Action
{
    None,
    Register,
    Unregister,
};

/// 模式变化（`_DoReevaluateAddWordHotkey`）时候选热键该做什么。
///
/// - 切到英文且已注册 → 注销，立即还给宿主；
/// - 中文、候选可见、尚未注册 → 注册。候选可见期间英文→中文时候选显隐不再变化，
///   `NotifyCandidatesVisibilityChanged` 不会再触发，只能在这里补；焦点门卫仍由
///   `_RegisterCandidateHotkeys` 内的 `ShouldRegister` 把关。
inline Action OnModeReevaluate(bool chineseMode, bool candidatesVisible, bool active)
{
    if (!chineseMode) return active ? Action::Unregister : Action::None;
    return (candidatesVisible && !active) ? Action::Register : Action::None;
}

enum class GiveBack
{
    None,
    Replay,
    UnregisterThenReplay,
};

/// 经 `WM_HOTKEY` 派发的热键，服务端响应之后这个键该怎么交还宿主。
///
/// 系统热键一旦命中，原键就被系统吃下、宿主永远看不到；服务端回「交还宿主」类响应
/// （`ClearCompositionThenPassThrough` / `PassThrough`，如联想态的 Ctrl+数字）时，只能
/// 自己注入一次原键。
///
/// - `candidateSegment`：只管候选热键段。加词段（GLOBAL 热键）在候选消失后照旧注册着，
///   注入的键会被自己再次拦下。
/// - `active`：候选热键此刻还注册着就**必须先注销**再注入，否则注入的键同样被拦回来、
///   派发再回 PassThrough，无限循环。`ClearCompositionThenPassThrough` 在响应处理里已收掉
///   候选（随之注销）；`PassThrough` 不收候选，注销后靠下一次候选出现重新注册。
inline GiveBack OnHotkeyDispatched(bool candidateSegment, bool giveBack, bool active)
{
    if (!candidateSegment || !giveBack) return GiveBack::None;
    return active ? GiveBack::UnregisterThenReplay : GiveBack::Replay;
}

/// 服务端这条响应是否要把当前键交还宿主。热键派发据此决定要不要注入原键。
inline bool GivesBackToHost(ResponseType type)
{
    return type == ResponseType::PassThrough || type == ResponseType::ClearCompositionThenPassThrough;
}

// 通用修饰位，与 BinaryProtocol.h 的 KEYMOD_* 同值，TextService.cpp 里有 static_assert 钉住。
inline constexpr std::uint32_t kModShift = 0x0001; // KEYMOD_SHIFT
inline constexpr std::uint32_t kModCtrl = 0x0002;  // KEYMOD_CTRL
inline constexpr std::uint32_t kModAlt = 0x0004;   // KEYMOD_ALT
inline constexpr std::uint32_t kModWin = 0x0008;   // KEYMOD_WIN

/// 注入原键前，热键要求的修饰键是否都还按着。
///
/// 注入只发主键，修饰键靠物理按着叠上去；同步 IPC 期间用户若已松开其中一个，注入出去的
/// 就成了另一个键（Ctrl+1 变成裸 1 打进文档）。此时宁可丢键，也不补注修饰键——补注的
/// Ctrl 会被当成一次 Ctrl 轻敲，误切中英。只看通用位，左右位不参与。
inline bool RequiredModifiersHeld(std::uint32_t required, std::uint32_t held)
{
    const std::uint32_t need = required & (kModShift | kModCtrl | kModAlt | kModWin);
    return (held & need) == need;
}

} // namespace candhotkey
} // namespace wind
