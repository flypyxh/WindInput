// 多行文本以剪贴板粘贴方式上屏（compat 规则 `multiline_commit = "paste"`）的纯逻辑部分。
//
// 有的宿主（新版微信）收到含换行的 TSF 上屏时，会把组合区的编码显示残留在界面上。对这类
// 宿主改为：结束组合 → 文本写入剪贴板 → 注入 Ctrl+V。是否开启由服务端按应用决定，经
// CONFIG_KEY_MULTILINE_COMMIT 下发；接线见 TextService.cpp 的 CCommitTextEditSession。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/multiline_commit_test.cpp）。
#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace wind
{
namespace multiline
{

/// CONFIG_KEY_MULTILINE_COMMIT 的值是否表示 paste。只认 1，其余（含将来的新档）按 commit。
inline bool IsPasteMode(std::uint8_t mode)
{
    return mode == 1;
}

/// 文本里有没有换行（CR 或 LF）。
inline bool ContainsNewline(std::wstring_view text)
{
    return text.find_first_of(L"\r\n") != std::wstring_view::npos;
}

/// 换行统一成 CRLF（CF_UNICODETEXT 的约定）：`\r\n` 算一个换行，孤立的 `\r` 或 `\n` 都转成 `\r\n`。
inline std::wstring NormalizeToCrlf(std::wstring_view text)
{
    std::wstring out;
    out.reserve(text.size() + text.size() / 8 + 2);
    for (std::size_t i = 0; i < text.size(); i++)
    {
        const wchar_t ch = text[i];
        if (ch == L'\r')
        {
            if (i + 1 < text.size() && text[i + 1] == L'\n')
                i++;
            out += L"\r\n";
        }
        else if (ch == L'\n')
        {
            out += L"\r\n";
        }
        else
        {
            out += ch;
        }
    }
    return out;
}

} // namespace multiline
} // namespace wind
