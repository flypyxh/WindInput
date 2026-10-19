// SpaceKeyPolicy 判据测试（论坛 t337：自定义空格映射在 Notepad++ / WPS 不生效）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o space_key_test space_key_policy_test.cpp && ./space_key_test
//
// 变异检验：去掉 fullWidth ⇒ TestFullWidthAlwaysEats 红；列号写错 ⇒ TestHalfWidthFollowsColumn 红。

#include "SpaceKeyPolicy.h"

#include <cstdio>

namespace
{
using wind::spacekey::PunctColumn;
using wind::spacekey::ShouldEatIdleSpace;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestColumnsMatchCore()
{
    Expect(PunctColumn(true, false) == 0, "中半 = 列 0");
    Expect(PunctColumn(false, true) == 1, "英全 = 列 1");
    Expect(PunctColumn(true, true) == 2, "中全 = 列 2");
    Expect(PunctColumn(false, false) == 3, "英半 = 列 3");
}

void TestNoMappingPassesHalfWidth()
{
    Expect(!ShouldEatIdleSpace(true, false, 0), "无映射的中文标点半角放行：保留宿主空格语义");
    Expect(!ShouldEatIdleSpace(false, false, 0), "无映射的英文标点半角放行");
}

void TestFullWidthAlwaysEats()
{
    Expect(ShouldEatIdleSpace(true, true, 0), "全角态恒吃：core 出 U+3000");
    Expect(ShouldEatIdleSpace(false, true, 0), "英文标点 + 全角同样吃");
}

void TestHalfWidthFollowsColumn()
{
    Expect(ShouldEatIdleSpace(true, false, 0b0001), "中半列配了映射 ⇒ 中文标点半角吃");
    Expect(!ShouldEatIdleSpace(false, false, 0b0001), "只配中半列，英文标点半角不吃");
    Expect(ShouldEatIdleSpace(false, false, 0b1000), "英半列配了映射 ⇒ 英文标点半角吃");
    Expect(!ShouldEatIdleSpace(true, false, 0b0100), "只配中全列，中文标点半角不吃");
}

} // namespace

int main()
{
    TestColumnsMatchCore();
    TestNoMappingPassesHalfWidth();
    TestFullWidthAlwaysEats();
    TestHalfWidthFollowsColumn();
    if (g_failures == 0)
        std::printf("space_key_policy_test: all passed\n");
    return g_failures == 0 ? 0 : 1;
}
