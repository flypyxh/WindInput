// CompositionPlaceholder 识别 / 兜底取值测试（GH#175）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o placeholder_test composition_placeholder_test.cpp && ./placeholder_test
//
// 变异检验：
//   - IsPlaceholderText 漏认 ZWSP ⇒ 「浏览器里写入的 ZWSP」红；
//   - PlaceholderForKind 把未知值映射成 ZWSP ⇒ 「认不出的值回落空格」红。
// ⚠️ 只测识别与取值；占位字符由谁决定（服务端 compat 规则）、怎么下发，见 CompositionPlaceholder.h 文件头。

#include "CompositionPlaceholder.h"

#include <cstdio>
#include <string>

namespace
{
using namespace wind::placeholder;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestPlaceholderForKind()
{
    Expect(std::wstring(PlaceholderForKind(0)) == L" ", "0 = 空格（WPS 等靠它撑出非零高度光标矩形）");
    Expect(std::wstring(PlaceholderForKind(1)) == L"\u200B", "1 = ZWSP（trim / \\s 都不删它）");
    Expect(std::wstring(PlaceholderForKind(2)) == L" ", "认不出的值回落空格（旧 DLL 遇新档保持历史行为）");
    Expect(std::wstring(PlaceholderForKind(255)) == L" ", "认不出的值回落空格");
    Expect(std::wstring(PlaceholderForKind(1)).size() == 1, "ZWSP 占位长度恒为 1，光标仍可落在它前面");
}

void TestIsPlaceholderText()
{
    Expect(IsPlaceholderText(L" "), "空格占位");
    Expect(IsPlaceholderText(L"\u200B"), "浏览器里写入的 ZWSP 占位");
    Expect(!IsPlaceholderText(L""), "空串不是占位（是「需要兜底占位」）");
    Expect(!IsPlaceholderText(L"  "), "两个空格不是占位");
    Expect(!IsPlaceholderText(L"a"), "普通编码");
    Expect(!IsPlaceholderText(L" a"), "带空格前缀的编码");
    Expect(!IsPlaceholderText(L"\u200B\u200B"), "两个 ZWSP 不是占位");
    Expect(!IsPlaceholderText(L"　"), "全角空格不是占位（它是可上屏的正文）");
}
} // namespace

int main()
{
    TestPlaceholderForKind();
    TestIsPlaceholderText();

    if (g_failures == 0)
    {
        std::printf("composition_placeholder_test: all passed\n");
        return 0;
    }
    std::printf("composition_placeholder_test: %d failure(s)\n", g_failures);
    return 1;
}
