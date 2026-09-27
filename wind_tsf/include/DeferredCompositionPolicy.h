// direct_commit「真提交 + 延迟重开组合」里，**哪一个 keyup 可以开延迟组合**的纯判据。
//
// 从 `CKeyEventSink::OnTestKeyUp / OnKeyUp` 抽出来只为可测（不含 Win32 头，g++ 可跑，
// 见 tests/deferred_composition_policy_test.cpp）。
//
// # 为什么不能「下一个 keyup 就开」
//
// 延迟的全部意义是在「提交」与「重开」之间隔一拍消息泵，让宿主先把提交处理完。旧实现
// 假设「提交之后的下一个 keyup 就是触发键（空格/顶码键）的 keyup」——快打时不成立：
// 用户按下空格时**前一个编码键往往还没松开**（按键重叠），它的 keyup 紧跟着到达，于是
// 延迟组合在提交后 0ms 就开了，那一拍被吃掉。
//
// Tabby（xterm.js）实测后果（2026-09-27 wind_tsf.Tabby.12880.log）：联想态占位空格在
// 「在 / 法 / 后 / 处理 / 看」提交后 0ms 开出（Stash 与 StartDeferred 同一毫秒），xterm 的
// compositionend 延时读 textarea 时新组合已起，占位空格被并进上屏文本发给终端——用户
// 看到的就是「多了一个空格」，随后按退格删掉。其余 70 次延迟 5~150ms 的都没有。
#pragma once

#include <cstdint>

namespace wind
{
namespace deferredcomp
{

/// 这个 keyup 能否开延迟组合。
///
/// - `triggerVk`：Stash 时记下的触发键；0 = 不知道触发键（非按键路径 Stash），维持旧行为。
/// - `keyUpVk`：本次 keyup 的键。
///
/// 别的键的 keyup 一律不开，交给触发键 keyup / 下一次 keydown / 兜底定时器。
inline bool ShouldOpenOnKeyUp(uint32_t triggerVk, uint32_t keyUpVk)
{
    return triggerVk == 0 || triggerVk == keyUpVk;
}

} // namespace deferredcomp
} // namespace wind
