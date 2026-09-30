// ToggleTapPolicy 判据测试
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -I../include -o toggle_tap_test toggle_tap_policy_test.cpp && ./toggle_tap_test
//
// 变异检验：IsTapCancelledByMouse 里去掉任一信号 ⇒ 对应的 Test 红。
// ⚠️ 只测判据；信号怎么取见 ToggleTapPolicy.h 文件头。

#include "ToggleTapPolicy.h"

#include <cstdio>

namespace
{
using namespace wind::toggletap;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestCleanTapStillToggles()
{
    Expect(!IsTapCancelledByMouse(TapSignals{}), "无任何鼠标信号 ⇒ 仍是单击，照常切换");
}

void TestEachSignalCancels()
{
    TapSignals s;
    s = {}; s.mouseDownAtPress = true;
    Expect(IsTapCancelledByMouse(s), "按下 Shift 时鼠标键已按着");
    s = {}; s.selectionChanged = true;
    Expect(IsTapCancelledByMouse(s), "Shift 期间选区变化（Shift+点击）");
    s = {}; s.mouseDownAtRelease = true;
    Expect(IsTapCancelledByMouse(s), "松开 Shift 时鼠标键仍按着");
    s = {}; s.mousePressedSincePress = true;
    Expect(IsTapCancelledByMouse(s), "Shift 期间鼠标键被按下过");
}
} // namespace

int main()
{
    TestCleanTapStillToggles();
    TestEachSignalCancels();
    if (g_failures == 0)
        std::printf("toggle_tap_policy_test: all passed\n");
    return g_failures == 0 ? 0 : 1;
}
