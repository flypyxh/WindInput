// 会话中的 Ctrl/Alt 组合（cleanup 通路：OnTestKeyDown 的 `ctrl_alt_cleanup` 吃下、OnKeyDown
// 转发服务端）处理完之后，这一键要不要还给宿主。
//
// 这条通路收的是**没登记成热键**的组合：Ctrl+S / Ctrl+C 这类宿主快捷键，服务端清掉组合后
// 键照样归宿主；服务端拿它做了事（候选操作留着组合、快捷输入选词键 `keys.quick_input_select`
// 上屏候选）就不能再交出去，否则宿主会把同一键再执行一次。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/ctrl_alt_cleanup_policy_test.cpp）。
#pragma once

#include "ResponseType.h"

namespace wind
{
namespace ctrlalt
{

/// 这类响应是否把文字写进了文档（上屏）。KeyEventSink 据此置 `_responseCommitted`。
///
/// 只认「真写了文档」的：CommitAndHold 只把文字并进组合前缀、未真提交，但组合还在，
/// 下面的判据已按「组合仍在」留住这一键，不必列入。
inline bool ResponseCommitsText(ResponseType type)
{
    switch (type)
    {
    case ResponseType::CommitText:
    case ResponseType::InsertTextWithCursor:
    case ResponseType::CommitThenDefer:
    case ResponseType::ReplaceBackward:
        return true;
    default:
        return false;
    }
}

/// 服务端已吃下（pfEaten 为真）之后，是否改判为交还宿主。
///
/// - hasCandidates / isComposing：响应处理**之后**的会话状态。组合还在 ⇒ 服务端拿它做了事。
/// - committedByResponse：本次响应上屏了文字（见 ResponseCommitsText）。上屏会把组合收掉，
///   单看上面两项会误判成「只是清了组合」——快捷输入里选词键选中整段上屏正是这种形态。
inline bool ShouldReturnToHost(bool hasCandidates, bool isComposing, bool committedByResponse)
{
    return !hasCandidates && !isComposing && !committedByResponse;
}

} // namespace ctrlalt
} // namespace wind
