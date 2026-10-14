// SmartPunctPrevChar 单元测试
//
// 跑法（纯 C++17、不含 Win32 头，本机不需要 MSVC）：
//   g++ -std=c++17 -I../include -o smart_punct_prev_char_test smart_punct_prev_char_test.cpp && ./smart_punct_prev_char_test
//
// 守的是「数字后智能标点只认输入序列」：文档里的数字不得再让标点变半角，序列里的数字
// 必须让标点变半角；非数字的文档值原样上报（智能符号 press2 守卫要用）。
//
// ⚠️ 测不到的部分：_lastPassthroughDigit 在哪些记录点被设置/清零、调用点是否真用了本函数。
// 那是接线，只能靠 KeyEventSink.cpp 的注释与真机验证。

#include "SmartPunctPrevChar.h"

#include <cstdio>

namespace
{

using WindSmartPunct::ResolvePrevChar;

int g_failures = 0;

void Expect(const char* name, uint16_t got, uint16_t want)
{
    if (got != want)
    {
        std::printf("FAIL %s: got 0x%04X, want 0x%04X\n", name, got, want);
        ++g_failures;
    }
}

constexpr uint16_t kDui = 0x5BF9;      // 对
constexpr uint16_t kCnPeriod = 0x3002; // 。

} // namespace

int main()
{
    // 光标前是旧数字（点鼠标挪过去 / 上次会话留下的），但最近一笔输入不是数字 → 不算数字后。
    Expect("doc_digit_without_sequence_is_masked", ResolvePrevChar('3', 0, true), 0);
    // 非标点键同样抹：数字永远不该从文档这条路进服务端。
    Expect("doc_digit_masked_for_non_punct_key", ResolvePrevChar('3', 0, false), 0);

    // 刚打的数字：无论文档读回什么都以序列为准——包括 OnEndEdit 晚到的旧字（微信快打「对3.」）。
    Expect("sequence_digit_when_doc_unreadable", ResolvePrevChar(0, '5', true), '5');
    Expect("sequence_digit_with_matching_doc", ResolvePrevChar('5', '5', true), '5');
    Expect("sequence_digit_beats_late_doc_char", ResolvePrevChar(kDui, '5', true), '5');
    Expect("sequence_digit_beats_other_doc_digit", ResolvePrevChar('3', '5', true), '5');

    // 序列值只给标点键：非标点键不带出数字（调用方随后清零序列）。
    Expect("sequence_digit_ignored_for_non_punct_key", ResolvePrevChar(0, '5', false), 0);

    // 非数字的文档值原样上报——智能符号 press2 守卫靠它比对武装串末位。
    Expect("non_digit_doc_char_passes_through", ResolvePrevChar(kCnPeriod, 0, true), kCnPeriod);
    Expect("ascii_period_doc_char_passes_through", ResolvePrevChar('.', 0, true), '.');
    Expect("unreadable_doc_stays_zero", ResolvePrevChar(0, 0, true), 0);

    // 全角数字不是 ASCII 数字，按普通字符原样上报（服务端本就不认它）。
    Expect("fullwidth_digit_not_masked", ResolvePrevChar(0xFF13, 0, true), 0xFF13);

    if (g_failures == 0)
        std::printf("smart_punct_prev_char_test: all passed\n");
    return g_failures == 0 ? 0 : 1;
}
