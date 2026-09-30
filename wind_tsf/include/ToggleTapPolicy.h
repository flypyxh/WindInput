// 「修饰键单击切换」要不要被鼠标操作作废的**纯判据**（论坛 t257）。
//
// 症状：Shift 配成切换键（切中英 / 方案）后，在编辑器里 Shift+鼠标左键拖选 / 扩选文本，
// 松开 Shift 时被当成「单击 Shift」，中英 / 方案来回切。键盘侧本来就有「Shift 期间按过别的
// 键则不算单击」（OnTestKeyDown 里非切换键取消 pending），鼠标不走按键流，只能另想办法。
//
// 信号（任一成立 ⇒ 这次 Shift 不算单击）：
//   1. 按下 Shift 那一刻鼠标键已经按着（先按住左键、再按 Shift 扩选）；
//   2. Shift 按住期间宿主报了选区变化（`OnEndEdit` 的 selectionChanged）——Shift+点击必然
//      改选区，这是 TSF 原生信号，不依赖轮询；
//   3. 松开 Shift 时鼠标键仍按着（拖选到一半先松了 Shift）；
//   4. `GetAsyncKeyState` 的「自上次调用后被按下过」位。**微软明说不可依赖**，只作补充：
//      按下 Shift 时先读一次把它清掉，松开时再读，命中即有点击发生过。
//
// ⚠️ 覆盖边界：这里只有判据。四个信号怎么取、何时清，全在 KeyEventSink.cpp / TextService.cpp，
// 判据全绿 ≠ 真机上不误切。真机要验：Shift+拖选不切、单击 Shift 仍切、长按不切（原有）。
#pragma once

namespace wind
{
namespace toggletap
{

struct TapSignals
{
    bool mouseDownAtPress = false;     // 信号 1
    bool selectionChanged = false;     // 信号 2
    bool mouseDownAtRelease = false;   // 信号 3
    bool mousePressedSincePress = false; // 信号 4
};

/// 这次修饰键按放是否被鼠标操作污染（污染 ⇒ 不该当成单击切换）。
inline bool IsTapCancelledByMouse(const TapSignals& s)
{
    return s.mouseDownAtPress || s.selectionChanged || s.mouseDownAtRelease
        || s.mousePressedSincePress;
}

} // namespace toggletap
} // namespace wind
