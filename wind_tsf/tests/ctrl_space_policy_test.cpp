// CtrlSpacePolicy 判据测试（GH#172）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o ctrl_space_test ctrl_space_policy_test.cpp && ./ctrl_space_test
//
// 变异检验：ShouldInterceptCtrlSpace 去掉 enabled ⇒ TestDisabledPassesThrough 红；
// IsCtrlSpaceChord 放宽修饰键判据 ⇒ TestChordShape 红；DecodeToggleValue 空值不保持 ⇒
// TestDecode 红。

#include "CtrlSpacePolicy.h"

#include <cstdint>
#include <cstdio>
#include <vector>

namespace
{
using namespace wind::ctrlspace;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

constexpr std::uint32_t VK_SPACE_ = 0x20;
constexpr std::uint32_t VK_A_ = 0x41;
// 与 BinaryProtocol.h 的 KEYMOD_* 同值（那份头含 Win32，测试里不能 include；
// KeyEventSink.cpp 里有 static_assert 钉住两边一致）。
constexpr std::uint32_t SHIFT = 0x0001, CTRL = 0x0002, ALT = 0x0004, LCTRL = 0x0040, RCTRL = 0x0080;

void TestChordShape()
{
    Expect(IsCtrlSpaceChord(VK_SPACE_, CTRL), "Ctrl+Space");
    Expect(IsCtrlSpaceChord(VK_SPACE_, CTRL | LCTRL), "左 Ctrl+Space");
    Expect(IsCtrlSpaceChord(VK_SPACE_, CTRL | RCTRL), "右 Ctrl+Space");
    Expect(!IsCtrlSpaceChord(VK_SPACE_, 0), "裸 Space 不是");
    Expect(!IsCtrlSpaceChord(VK_SPACE_, CTRL | SHIFT), "Ctrl+Shift+Space 是另一个键");
    Expect(!IsCtrlSpaceChord(VK_SPACE_, CTRL | ALT), "Ctrl+Alt+Space 是另一个键");
    Expect(!IsCtrlSpaceChord(VK_SPACE_, SHIFT), "Shift+Space（全半角）不是");
    Expect(!IsCtrlSpaceChord(VK_A_, CTRL), "Ctrl+A 不是");
}

void TestEnabledIntercepts()
{
    Expect(ShouldInterceptCtrlSpace(true, VK_SPACE_, CTRL), "开着：吃 Ctrl+Space（出厂行为）");
    Expect(!ShouldInterceptCtrlSpace(true, VK_A_, CTRL), "开着也不碰别的键");
}

void TestDisabledPassesThrough()
{
    Expect(!ShouldInterceptCtrlSpace(false, VK_SPACE_, CTRL), "关掉：Ctrl+Space 交给宿主（IDEA 代码提示）");
    Expect(!ShouldInterceptCtrlSpace(false, VK_SPACE_, CTRL | LCTRL), "关掉：左 Ctrl 同样放行");
}

void TestDecode()
{
    const std::vector<std::uint8_t> on{1}, off{0}, other{7}, empty{};
    Expect(DecodeToggleValue(on.data(), on.size(), false), "1 ⇒ 开");
    Expect(!DecodeToggleValue(off.data(), off.size(), true), "0 ⇒ 关");
    Expect(DecodeToggleValue(other.data(), other.size(), false), "非 0 ⇒ 开");
    Expect(DecodeToggleValue(empty.data(), empty.size(), true), "空值保持原状（开）");
    Expect(!DecodeToggleValue(empty.data(), empty.size(), false), "空值保持原状（关）");
    Expect(kDefaultEnabled, "未收到下发前默认开：与历史行为一致");
}

} // namespace

int main()
{
    TestChordShape();
    TestEnabledIntercepts();
    TestDisabledPassesThrough();
    TestDecode();
    if (g_failures == 0)
        std::printf("ctrl_space_policy: all passed\n");
    return g_failures == 0 ? 0 : 1;
}
