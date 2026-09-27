// CandidateHotkeyPolicy 真值表测试
//
// 跑法（纯 C++17、不含 Win32 头，本机不需要 MSVC）：
//   g++ -std=c++17 -Iwind_tsf/include -o chp_test wind_tsf/tests/candidate_hotkey_policy_test.cpp && ./chp_test
//
// 变异检验：去掉 chineseMode 那一项（= 修复前）→ TestEnglishModeDoesNotRegister 红；
// OnModeReevaluate 不返回 Register（= H1 修复前）→ TestSwitchBackToChineseWithVisibleCandidatesRegisters 红。

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

/// ★ 候选可见期间英文→中文：候选显隐不再变，只能靠模式重估补注册，否则整段候选热键失效。
void TestSwitchBackToChineseWithVisibleCandidatesRegisters()
{
    Check(OnModeReevaluate(true, true, false) == Action::Register, "中文 + 候选可见 + 未注册 → 注册");
}

void TestModeReevaluateNoOpCases()
{
    Check(OnModeReevaluate(true, true, true) == Action::None, "已注册不重复注册");
    Check(OnModeReevaluate(true, false, false) == Action::None, "候选不可见不注册");
    Check(OnModeReevaluate(false, true, false) == Action::None, "英文模式未注册无事可做");
}

void TestSwitchToEnglishUnregisters()
{
    Check(OnModeReevaluate(false, true, true) == Action::Unregister, "切到英文立即注销");
    Check(OnModeReevaluate(false, false, true) == Action::Unregister, "英文 + 残留注册也注销");
}

} // namespace

int main()
{
    TestEnglishModeDoesNotRegister();
    TestChineseModeRegisters();
    TestFocusGatesStillHold();
    TestSwitchBackToChineseWithVisibleCandidatesRegisters();
    TestModeReevaluateNoOpCases();
    TestSwitchToEnglishUnregisters();
    if (g_failures != 0)
    {
        std::printf("candidate_hotkey_policy_test: %d failure(s)\n", g_failures);
        return 1;
    }
    std::printf("candidate_hotkey_policy_test: all passed\n");
    return 0;
}
