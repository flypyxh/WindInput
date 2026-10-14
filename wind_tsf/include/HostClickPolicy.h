#pragma once

// Host render 候选窗的左键点选判据。纯 C++17、不含 Win32 头，本机 g++ 可测
// （tests/host_click_policy_test.cpp）。
//
// 点选在**松开**时生效：按下只记命中，松开仍落在同一项上才发 CMD_CANDIDATE_SELECT；按住移出
// 后松开视为放弃（按钮的通行语义），翻页按钮同理。与服务端本地候选窗（wind-ui
// candidate_window.rs `click_on_release`）、Linux（ClickGate）、macOS（CandidateContentView.mouseUp）
// 一致——按下就选的话，上屏、收起候选窗在松开前就做完了，松开落到候选窗后面的宿主上。

#include <climits>

namespace wind::hostclick
{

// 没有按下 / 没命中的哨兵，与 CHostWindow::_HitTest 的「没命中」同值。
constexpr int kNone = INT_MIN;

// pressed：按下时的命中（kNone = 这次松开之前没有在可点项上按下）；hit：松开处的命中。
inline bool SelectsOnRelease(int pressed, int hit)
{
    return pressed != kNone && pressed == hit;
}

} // namespace wind::hostclick
