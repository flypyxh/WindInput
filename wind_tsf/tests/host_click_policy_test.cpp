// HostClickPolicy 判据测试
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -I../include -o host_click_test host_click_policy_test.cpp && ./host_click_test
//
// 变异检验：SelectsOnRelease 改成只看 hit ⇒ TestReleasedElsewhereAbandons 红；
// 去掉 kNone 判断 ⇒ TestNoPressNoSelect 红。

#include "HostClickPolicy.h"

#include <cstdio>

namespace
{
using namespace wind::hostclick;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestSameItemSelects()
{
    Expect(SelectsOnRelease(0, 0), "按下与松开都在候选 0 ⇒ 选");
    Expect(SelectsOnRelease(3, 3), "按下与松开都在候选 3 ⇒ 选");
    Expect(SelectsOnRelease(-1, -1), "上一页按钮按下松开 ⇒ 翻页");
    Expect(SelectsOnRelease(-2, -2), "下一页按钮按下松开 ⇒ 翻页");
}

void TestReleasedElsewhereAbandons()
{
    Expect(!SelectsOnRelease(0, 1), "按下候选 0、松开在候选 1 ⇒ 放弃");
    Expect(!SelectsOnRelease(0, kNone), "按下候选 0、松开在空白 / 窗外 ⇒ 放弃");
    Expect(!SelectsOnRelease(-1, 0), "按下翻页、松开在候选上 ⇒ 放弃");
}

void TestNoPressNoSelect()
{
    Expect(!SelectsOnRelease(kNone, kNone), "没按下过 ⇒ 不选");
    Expect(!SelectsOnRelease(kNone, 0), "在别处按下、移进来松开 ⇒ 不选");
}

} // namespace

int main()
{
    TestSameItemSelects();
    TestReleasedElsewhereAbandons();
    TestNoPressNoSelect();
    if (g_failures == 0)
    {
        std::printf("host_click_policy_test: all passed\n");
        return 0;
    }
    std::printf("%d FAILURE(S)\n", g_failures);
    return 1;
}
