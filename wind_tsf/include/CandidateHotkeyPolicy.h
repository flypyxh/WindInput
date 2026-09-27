// 「候选可见时要不要把 SESSION 热键表注册成系统级热键」的**纯判据**。
//
// 从 `CTextService::_RegisterCandidateHotkeys` 里抽出来只为让它可测。表里的键（置顶 / 删除 /
// `input.alt_commit` 的 Alt+0..9 与 Alt+Space）在服务端**全部只在中文模式认领**；英文模式
// 下也会有候选（英文补全），此时注册了就是把宿主的 Alt 组合吞掉、服务端又不接，键凭空消失。
// 与 `OnTestKeyDown` 里 session 热键的退路分支同一判据（chineseMode && 有会话）。
//
// ⚠️ 覆盖边界：这里只有判据。RegisterHotKey 本身与调用时机都在 TextService.cpp。
#pragma once

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

} // namespace candhotkey
} // namespace wind
