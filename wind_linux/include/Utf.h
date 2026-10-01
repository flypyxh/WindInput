// UTF-8 / UTF-16 换算（纯逻辑）。
//
// 服务端的组合内光标（UpdateComposition.caretPos）以 **UTF-16 码元**计（为 TSF/IMKit 定的
// 口径），而 Fcitx5 的 `Text::setCursor` 要的是 **UTF-8 字节偏移**。两者在 BMP 以外的
// 字符（生僻字扩展 B 区、emoji）上差得最远，写错的症状是「光标落在字的中间、组合串被截断
// 显示」，只在打生僻字时出现。
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

namespace windlinux {

/// UTF-8 串按 UTF-16 计的长度。非法字节按 1 个码元计（不抛错：显示层宁可错位也不能崩）。
size_t utf16Length(const std::string& utf8);

/// 把「前 n 个 UTF-16 码元」换算成 UTF-8 字节偏移；n 超过串长时钳到串尾。
/// n 落在代理对中间（只取了高半）时向后取整到该字符之后。
size_t utf16OffsetToUtf8(const std::string& utf8, size_t n);

/// 码点个数（非法字节各算一个）。Fcitx5 的 `deleteSurroundingText` 与光标移动都按字符计。
size_t utf8CharCount(const std::string& utf8);

/// 末位码点；空串返回 0。
uint32_t lastCodepoint(const std::string& utf8);

/// 服务端的「删光标前 N 个」以 **UTF-16 码元**计（Windows TSF 的 ACP、macOS 的 NSRange 都是
/// 这个量纲），Fcitx5 的 `deleteSurroundingText` 按**码点**计。从光标前文本 `before` 的末尾往前
/// 数 `units` 个码元，返回覆盖的码点数。落在代理对中间时不含那个字（宁少删不多删，同 macOS
/// 取不到范围时「只插入不删除」的取向）；`before` 不够长（宿主只给了一段）时，余下的码元按
/// 每个 1 码点计。
size_t codepointsForUtf16Back(const std::string& before, size_t units);

} // namespace windlinux
