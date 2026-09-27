// CandidateHotkeyPolicy 真值表测试
//
// 跑法（纯 C++17、不含 Win32 头，本机不需要 MSVC）：
//   g++ -std=c++17 -Iwind_tsf/include -o chp_test wind_tsf/tests/candidate_hotkey_policy_test.cpp && ./chp_test
//
// 变异检验：去掉 chineseMode 那一项（= 修复前）→ TestEnglishModeDoesNotRegister 红。

#include "CandidateHotkeyPolicy.h"

#include <cstdio>

namespace
{

using namespace wind::candhotkey;

int g_failures = 0;

void Check(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("  FAIL: %s\n", what);
        ++g_failures;
    }
}

/// ★ 英文模式有候选（英文补全）时不注册：否则 Alt+数字 / Alt+空格被吞、宿主拿不到。
void TestEnglishModeDoesNotRegister()
{
    Check(!ShouldRegister(true, true, false), "英文模式不注册候选热键");
}

void TestChineseModeRegisters()
{
    Check(ShouldRegister(true, true, true), "中文模式、前台且有焦点时注册");
}

void TestFocusGatesStillHold()
{
    Check(!ShouldRegister(false, true, true), "无 thread focus 不注册");
    Check(!ShouldRegister(true, false, true), "非前台进程不注册");
}

} // namespace

int main()
{
    TestEnglishModeDoesNotRegister();
    TestChineseModeRegisters();
    TestFocusGatesStillHold();
    if (g_failures != 0)
    {
        std::printf("candidate_hotkey_policy_test: %d failure(s)\n", g_failures);
        return 1;
    }
    std::printf("candidate_hotkey_policy_test: all passed\n");
    return 0;
}
