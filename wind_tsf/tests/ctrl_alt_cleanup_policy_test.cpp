// CtrlAltCleanupPolicy 判据测试（会话中未登记的 Ctrl/Alt 组合处理完后是否交还宿主）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o ctrl_alt_cleanup_test ctrl_alt_cleanup_policy_test.cpp && ./ctrl_alt_cleanup_test
//
// 变异检验：去掉 committedByResponse 项 ⇒ TestCommitKeepsKey 红；去掉 isComposing 项
// ⇒ TestCompositionKeepsKey 红；ResponseCommitsText 漏列任一上屏类 ⇒ TestCommitResponses 红。

#include "CtrlAltCleanupPolicy.h"

#include <cstdio>
#include <initializer_list>

namespace
{
using wind::ctrlalt::ResponseCommitsText;
using wind::ctrlalt::ShouldReturnToHost;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

// Ctrl+S / Ctrl+C：服务端只清了组合 ⇒ 键归宿主。
void TestCleanupReturnsKey()
{
    Expect(ShouldReturnToHost(false, false, false), "只清组合时应交还宿主");
}

// 候选操作 / 快捷输入里无候选时按选词键：组合仍在 ⇒ 不交还。
void TestCompositionKeepsKey()
{
    Expect(!ShouldReturnToHost(false, true, false), "组合仍在时不交还");
    Expect(!ShouldReturnToHost(true, false, false), "候选仍在时不交还");
}

// 快捷输入选词键整段上屏：组合已收掉，但键是被服务端用掉的 ⇒ 不交还。
void TestCommitKeepsKey()
{
    Expect(!ShouldReturnToHost(false, false, true), "上屏了候选时不交还");
}

// 写了文档的响应都算上屏：选词键上屏走 CommitText，其余三类同样会收掉组合。
void TestCommitResponses()
{
    Expect(ResponseCommitsText(ResponseType::CommitText), "CommitText 算上屏");
    Expect(ResponseCommitsText(ResponseType::InsertTextWithCursor), "InsertTextWithCursor 算上屏");
    Expect(ResponseCommitsText(ResponseType::CommitThenDefer), "CommitThenDefer 算上屏");
    Expect(ResponseCommitsText(ResponseType::ReplaceBackward), "ReplaceBackward 算上屏");
}

// 只清组合 / 只动候选的响应不算：Ctrl+S 的 ClearComposition 若被当成上屏，键就交不回宿主。
void TestNonCommitResponses()
{
    for (ResponseType t : {ResponseType::Ack, ResponseType::PassThrough, ResponseType::UpdateComposition,
                           ResponseType::ClearComposition, ResponseType::ClearCompositionThenPassThrough,
                           ResponseType::StatusUpdate, ResponseType::Consumed, ResponseType::CommitAndHold,
                           ResponseType::HoldComposition, ResponseType::Error})
    {
        Expect(!ResponseCommitsText(t), "非上屏类响应不算上屏");
    }
}

} // namespace

int main()
{
    TestCleanupReturnsKey();
    TestCompositionKeepsKey();
    TestCommitKeepsKey();
    TestCommitResponses();
    TestNonCommitResponses();

    if (g_failures == 0)
    {
        std::printf("ctrl_alt_cleanup_policy_test: all passed\n");
        return 0;
    }
    std::printf("ctrl_alt_cleanup_policy_test: %d failure(s)\n", g_failures);
    return 1;
}
