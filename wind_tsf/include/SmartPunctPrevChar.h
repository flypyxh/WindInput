#pragma once

#include <cstdint>

// ============================================================================
// SmartPunctPrevChar —— 发往服务端的 KeyPayload.prevChar 怎么取
// ============================================================================
//
// prevChar 在服务端有两个消费者，判据不同：
//
//   ① 数字后智能标点（wind-punct `is_smart_punct_after_digit`）：只问「是不是 ASCII 数字」。
//   ② 智能符号 press2 守卫：问「光标前是不是 press1 刚上屏的那个符号」。
//
// ★ ① 只认**输入法自己记录的输入序列**（`_lastPassthroughDigit`：透传出去的数字键 + 经引擎
// 上屏文本的末位），不认 TSF 现读的文档。理由（2026-10-13 定）：
//   · 现读文档受宿主 TSF 支持度左右，只在部分宿主、部分时机生效，同一操作时灵时不灵；
//   · 现读文档会把「光标恰好落在旧数字后面」（点鼠标、方向键挪过去、上次会话留下的数字）
//     也当成数字后，用户在那里想打全角 `。` `，` 打不出来。
// 「刚刚打了数字」是用户意图；「光标前碰巧是数字」不是。
//
// ⛔ 别让文档值「否决」序列（序列有数字、文档读到别的字就不算数字后）。看似能治「打完数字
// 用鼠标点到别处」，实则把 TSF 时序依赖带回来：微信这类 OnEndEdit 晚到的宿主，快打「对3.」
// 时缓存里是晚到的「对」，`.` 被误否决成 `。`——正是本次要去掉的时灵时不灵。
//
// 只认序列的已知代价（与 macOS 同，见 SmartPunctDigitTracker.swift）：
//   · 打完数字后用鼠标把光标点到别处，下一个标点仍按数字后出半角（键盘移动会清零，鼠标看不见）；
//   · 「34 → Backspace → .」出 `。`（退格把序列清零，而用户多半想要 `.`）；
//   · 记账漏掉的上屏路径不再有文档兜底。
//
// ② 仍需要文档现读值：宿主能读回时它是 press2 守卫唯一的事实来源。所以现读值照常上报，
// 只把其中的 ASCII 数字抹成 0——数字对 ② 从来不会匹配（武装串末位恒为符号），抹掉只让 ②
// 退回「读不回」这一档，与 macOS（只上报序列）同语义；而不抹就会让 ① 重新吃到文档数字。
//
// 本文件不含任何 Win32 头，可被 tests/smart_punct_prev_char_test.cpp 在非 Windows 工具链下
// 直接编译运行（`g++ -std=c++17 -Iwind_tsf/include`）。接线在 KeyEventSink::_SendKeyToService。
namespace WindSmartPunct
{

inline bool IsAsciiDigit(uint16_t c) { return c >= 0x30 && c <= 0x39; }

/// docChar：TSF 现读的光标前字符（0 = 读不回）；seqDigit：输入序列记下的数字（0 = 最近一笔
/// 不是数字）；isPunctKey：本键是否标点键（序列值只给标点键消费，其余键由调用方清零）。
inline uint16_t ResolvePrevChar(uint16_t docChar, uint16_t seqDigit, bool isPunctKey)
{
    if (isPunctKey && seqDigit != 0)
        return seqDigit;
    return IsAsciiDigit(docChar) ? 0 : docChar;
}

} // namespace WindSmartPunct
