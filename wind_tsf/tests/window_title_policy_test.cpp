// WindowTitlePolicy 判据测试（compat.toml 的 title 条件，P3）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o window_title_test window_title_policy_test.cpp && ./window_title_test
//
// 变异检验：
//   - ShouldCollectTitle 恒真 ⇒ 「默认不采集」红；
//   - ParseTitleMatchValue 空值时返回 false / true ⇒ 「空值保持现状」红；
//   - ShouldResyncFocusOnSwitch 漏任一条件 ⇒ 对应「不补」红；
//   - TruncateTitle 不截 / 截到 255 / 不处理代理对 ⇒ 对应三条红。
// ⚠️ 只测纯判据；取标题（InternalGetWindowText）与拼帧见 WindowTitlePolicy.h 文件头。

#include "WindowTitlePolicy.h"

#include <cstdio>
#include <string>

namespace
{
using namespace wind::window_title;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestSwitch()
{
    Expect(!ShouldCollectTitle(false), "默认不采集：服务端没推开（含从未推过）就一次都不取");
    Expect(ShouldCollectTitle(true), "推开了才取");

    const std::uint8_t on[] = {1};
    const std::uint8_t off[] = {0};
    const std::uint8_t other[] = {7};
    Expect(ParseTitleMatchValue(on, 1, false), "1 = 开");
    Expect(!ParseTitleMatchValue(off, 1, true), "0 = 关");
    Expect(ParseTitleMatchValue(other, 1, false), "非 0 = 开");
    Expect(!ParseTitleMatchValue(on, 0, false), "空值保持现状（关）");
    Expect(ParseTitleMatchValue(nullptr, 0, true), "空值保持现状（开）");
}

void TestResyncOnSwitch()
{
    Expect(ShouldResyncFocusOnSwitch(true, true, true, false),
           "首焦早于开关、没带标题 ⇒ 开关打开后补发");
    Expect(!ShouldResyncFocusOnSwitch(false, true, true, false), "首焦已带标题（开关先到）⇒ 不补");
    Expect(!ShouldResyncFocusOnSwitch(true, false, true, false), "开关又关了 ⇒ 不补");
    Expect(!ShouldResyncFocusOnSwitch(true, true, false, false), "已失焦 ⇒ 不补（下次 OnSetFocus 自然带标题）");
    Expect(!ShouldResyncFocusOnSwitch(true, true, true, true), "组合中 ⇒ 不补");
}

void TestTruncate()
{
    Expect(TruncateTitle(L"").empty(), "空标题");
    Expect(TruncateTitle(L"新标签页 - Google Chrome") == L"新标签页 - Google Chrome", "短标题原样");

    std::wstring exact(kMaxTitleChars, L'a');
    Expect(TruncateTitle(exact).size() == kMaxTitleChars, "恰好 256 不截");

    std::wstring longer(kMaxTitleChars + 10, L'b');
    Expect(TruncateTitle(longer).size() == kMaxTitleChars, "超长截到 256");

    // 第 256 个码元是高代理（emoji 的前半）：连它一起丢，不留半个代理对。
    std::wstring split(kMaxTitleChars - 1, L'c');
    // 显式写 UTF-16 码元：Linux 上 wchar_t 是 32 位，L"\U0001F600" 只有一个码元（Windows 上是两个）。
    split += (wchar_t)0xD83D; // 高代理落在下标 255
    split += (wchar_t)0xDE00; // 低代理在 256
    split += L"tail";
    std::wstring_view t = TruncateTitle(split);
    Expect(t.size() == kMaxTitleChars - 1, "截断点落在代理对中间时丢掉半个");
    Expect(t.back() == L'c', "截断后末尾是完整字符");

    // 代理对整个落在界内：保留。
    std::wstring whole(kMaxTitleChars - 2, L'd');
    whole += (wchar_t)0xD83D;
    whole += (wchar_t)0xDE00;
    whole += L"tail";
    Expect(TruncateTitle(whole).size() == kMaxTitleChars, "完整代理对在界内照留");
}
} // namespace

int main()
{
    TestSwitch();
    TestResyncOnSwitch();
    TestTruncate();

    if (g_failures == 0)
    {
        std::printf("window_title_policy_test: all passed\n");
        return 0;
    }
    std::printf("window_title_policy_test: %d failure(s)\n", g_failures);
    return 1;
}
