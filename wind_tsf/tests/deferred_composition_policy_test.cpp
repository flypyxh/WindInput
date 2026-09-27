// DeferredCompositionPolicy 单元测试
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -I../include -o deferred_composition_policy_test deferred_composition_policy_test.cpp && ./deferred_composition_policy_test
//
// 守的是「延迟组合只由触发键自己的 keyup 开」。反例来自 Tabby 真机日志：空格提交后，
// 还按着的前一个编码键的 keyup 立刻把联想占位空格开了出来，空格被 xterm 并进上屏文本。

#include "DeferredCompositionPolicy.h"

#include <cstdio>

namespace
{
constexpr uint32_t VK_SPACE_ = 0x20;
constexpr uint32_t VK_D_ = 0x44; // 「在」的编码键，按空格时还没松开
int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}
} // namespace

int main()
{
    using wind::deferredcomp::ShouldOpenOnKeyUp;

    Expect(ShouldOpenOnKeyUp(VK_SPACE_, VK_SPACE_), "触发键自己的 keyup 开组合");
    Expect(!ShouldOpenOnKeyUp(VK_SPACE_, VK_D_), "重叠按键：前一个编码键的 keyup 不能开组合");
    Expect(ShouldOpenOnKeyUp(0, VK_D_), "触发键未知（非按键路径）维持任意 keyup 即开");

    if (g_failures == 0)
        std::printf("deferred_composition_policy_test: all passed\n");
    return g_failures == 0 ? 0 : 1;
}
