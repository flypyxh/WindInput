// MultilineCommit 纯逻辑测试（CRLF 规范化 / 换行判定 / 下发值判读）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o multiline_commit_test multiline_commit_test.cpp && ./multiline_commit_test
//
// 变异检验：
//   - 把 `\r\n` 当两个换行 ⇒ 「CRLF 原样保留」红；
//   - 孤立 `\r` 不转 ⇒ 「孤立 CR」红；
//   - IsPasteMode 认非 1 的值 ⇒ 「认不出的值按 commit」红。

#include "MultilineCommit.h"

#include <cstdio>
#include <string>

namespace
{
using namespace wind::multiline;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

void TestNormalizeToCrlf()
{
    Expect(NormalizeToCrlf(L"") == L"", "空串");
    Expect(NormalizeToCrlf(L"abc") == L"abc", "无换行原样");
    Expect(NormalizeToCrlf(L"a\nb") == L"a\r\nb", "LF → CRLF");
    Expect(NormalizeToCrlf(L"a\r\nb") == L"a\r\nb", "CRLF 原样保留");
    Expect(NormalizeToCrlf(L"a\rb") == L"a\r\nb", "孤立 CR → CRLF");
    Expect(NormalizeToCrlf(L"a\n\rb") == L"a\r\n\r\nb", "LF CR 是两个换行");
    Expect(NormalizeToCrlf(L"a\r\r\nb") == L"a\r\n\r\nb", "CR 后接 CRLF");
    Expect(NormalizeToCrlf(L"\n\n") == L"\r\n\r\n", "连续 LF");
    Expect(NormalizeToCrlf(L"末尾\r") == L"末尾\r\n", "末尾孤立 CR");
    Expect(NormalizeToCrlf(L"中文\n第二行") == L"中文\r\n第二行", "非 ASCII 正文不动");
}

void TestContainsNewline()
{
    Expect(!ContainsNewline(L""), "空串");
    Expect(!ContainsNewline(L"一行"), "单行");
    Expect(ContainsNewline(L"a\nb"), "LF");
    Expect(ContainsNewline(L"a\rb"), "CR");
    Expect(ContainsNewline(L"\r\n"), "CRLF");
}

void TestIsPasteMode()
{
    Expect(!IsPasteMode(0), "0 = commit");
    Expect(IsPasteMode(1), "1 = paste");
    Expect(!IsPasteMode(2), "认不出的值按 commit");
    Expect(!IsPasteMode(255), "认不出的值按 commit");
}
} // namespace

int main()
{
    TestNormalizeToCrlf();
    TestContainsNewline();
    TestIsPasteMode();

    if (g_failures == 0)
    {
        std::printf("multiline_commit_test: all passed\n");
        return 0;
    }
    std::printf("multiline_commit_test: %d failure(s)\n", g_failures);
    return 1;
}
