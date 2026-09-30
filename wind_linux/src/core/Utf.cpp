#include "Utf.h"

namespace windlinux {

namespace {

/// 解 `s[i]` 起的一个码点，返回其字节长度（非法字节按 1 字节、码点 U+FFFD）。
size_t decodeOne(const std::string& s, size_t i, uint32_t& cp)
{
    auto b = [&](size_t k) { return static_cast<unsigned char>(s[k]); };
    unsigned char c = b(i);
    size_t len = c < 0x80 ? 1 : (c >> 5) == 0x6 ? 2 : (c >> 4) == 0xE ? 3 : (c >> 3) == 0x1E ? 4 : 0;
    if (len == 0 || i + len > s.size()) {
        cp = 0xFFFD;
        return 1;
    }
    for (size_t k = 1; k < len; ++k) {
        if ((b(i + k) & 0xC0) != 0x80) {
            cp = 0xFFFD;
            return 1;
        }
    }
    switch (len) {
    case 1: cp = c; break;
    case 2: cp = (uint32_t(c & 0x1F) << 6) | (b(i + 1) & 0x3F); break;
    case 3: cp = (uint32_t(c & 0x0F) << 12) | (uint32_t(b(i + 1) & 0x3F) << 6) | (b(i + 2) & 0x3F); break;
    default:
        cp = (uint32_t(c & 0x07) << 18) | (uint32_t(b(i + 1) & 0x3F) << 12)
            | (uint32_t(b(i + 2) & 0x3F) << 6) | (b(i + 3) & 0x3F);
        break;
    }
    return len;
}

} // namespace

size_t utf16Length(const std::string& utf8)
{
    size_t n = 0;
    for (size_t i = 0; i < utf8.size();) {
        uint32_t cp = 0;
        i += decodeOne(utf8, i, cp);
        n += cp >= 0x10000 ? 2 : 1;
    }
    return n;
}

size_t utf16OffsetToUtf8(const std::string& utf8, size_t n)
{
    size_t units = 0;
    size_t i = 0;
    while (i < utf8.size() && units < n) {
        uint32_t cp = 0;
        i += decodeOne(utf8, i, cp);
        units += cp >= 0x10000 ? 2 : 1;
    }
    return i;
}

size_t utf8CharCount(const std::string& utf8)
{
    size_t n = 0;
    for (size_t i = 0; i < utf8.size(); ++n) {
        uint32_t cp = 0;
        i += decodeOne(utf8, i, cp);
    }
    return n;
}

uint32_t lastCodepoint(const std::string& utf8)
{
    uint32_t last = 0;
    for (size_t i = 0; i < utf8.size();) {
        i += decodeOne(utf8, i, last);
    }
    return last;
}

} // namespace windlinux
