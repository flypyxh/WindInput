// X11 候选窗 + 光栅浮层：把服务光栅化好的 BGRA 帧贴到 override-redirect 顶层窗口上。
//
// 对位 macOS `CandidatePanel.swift`（NSPanel）。像素不在本进程画，这里只负责：建窗、贴图、
// 落位（翻转/钳制规则在纯逻辑 `placePanel`）、隐藏，以及把鼠标点击/悬停/滚轮翻成
// CMD_CANDIDATE_SELECT/HOVER/SCROLL 交回服务（结果经 push 通道异步回来）。
//
// 自建 xcb 连接（不借 Fcitx5 的 xcb 模块）：候选窗与 Fcitx5 自己的 UI 无关，自己的连接可以
// 在 xcb 模块未加载时照常工作；连接的 fd 挂到 Fcitx5 事件循环上，鼠标事件在主线程处理。
//
// 透明：有合成器（`_NET_WM_CM_S<n>` 有人持有）时用 32 位 ARGB visual，圆角与软件阴影按
// alpha 真透明；没有合成器时 ARGB 窗口的透明像素会画成黑色，于是退到根窗口 visual +
// XShape 按 alpha 抠形（alpha < 128 的像素抠掉）——圆角保住，半透明阴影退化成实色或消失。
#pragma once

#include "Codec.h"
#include "ShmFrame.h"

#include <fcitx-utils/event.h>

#include <functional>
#include <memory>
#include <vector>
#include <xcb/xcb.h>

namespace windlinux {

class X11CandidatePanel {
public:
    struct Callbacks {
        std::function<void(int32_t index)> select; // 候选下标；-1 上页 / -2 下页
        std::function<void(int32_t index)> hover;  // -1 = 离开
        std::function<void(int32_t delta)> scroll; // WHEEL_DELTA(120) 倍数，正 = 上滚
    };

    X11CandidatePanel(fcitx::EventLoop& loop, Callbacks cb);
    ~X11CandidatePanel();

    /// 贴一帧并显示。`x/y` 为服务端建议的左上角（wire 坐标 = 根窗口坐标）。
    /// 没有 X 连接（无 DISPLAY / 连不上）时返回 false、什么都不做。
    bool show(const SharedFrame& frame, int32_t x, int32_t y, bool absolute);
    void hide();
    /// 命中矩形（CMD_CANDIDATE_RECTS，晚于帧到达）。坐标是位图内的像素。
    void setRects(std::vector<CandidateHitRect> rects) { rects_ = std::move(rects); }

    /// 光栅浮层（CMD_OVERLAY_FRAME：tooltip / 状态气泡 / Toast）：各占一个窗口，落位见
    /// `placeOverlay`。`durationMs > 0` 时到点自己藏（计时归宿主，同 macOS `.app`）。
    /// 浮层窗口对鼠标透明（XShape 输入区为空）：点击穿透到下面的应用，不抢候选窗的悬停。
    bool showOverlay(uint32_t kind, const SharedFrame& frame, const OverlayFramePayload& p);
    void hideOverlay(uint32_t kind);
    void hideAllOverlays();

private:
    /// 一个 override-redirect 窗口及其位图。候选窗与三层浮层各一个，共用一条 X 连接。
    struct Surface {
        const char* instance = ""; // WM_CLASS 实例名（xdotool search --classname）
        bool interactive = false;  // 候选窗收鼠标；浮层对鼠标透明
        xcb_window_t window = 0;
        xcb_pixmap_t pixmap = 0;
        xcb_gcontext_t gc = 0;
        xcb_colormap_t colormap = 0;
        bool argb = false;
        uint8_t depth = 0;
        uint32_t pixW = 0;
        uint32_t pixH = 0;
        bool mapped = false;
        std::unique_ptr<fcitx::EventSourceTime> hideTimer;
    };
    static constexpr size_t kOverlayCount = 3;

    bool ensureConnection();
    void dropConnection();
    bool hasCompositor();
    bool ensureWindow(Surface& s, bool argb);
    void destroyWindow(Surface& s);
    bool present(Surface& s, const SharedFrame& frame, const Rect& r);
    void unmap(Surface& s);
    void upload(Surface& s, const SharedFrame& frame);
    void applyShape(Surface& s, const SharedFrame& frame);
    Surface* overlay(uint32_t kind);
    Rect workArea() const;
    void onReadable();
    void handleEvent(xcb_generic_event_t* ev);
    void setHover(int32_t index);

    fcitx::EventLoop& loop_;
    Callbacks cb_;
    xcb_connection_t* conn_ = nullptr;
    xcb_screen_t* screen_ = nullptr;
    int screenNum_ = 0;
    std::unique_ptr<fcitx::EventSourceIO> ioEvent_;
    bool warnedNoDisplay_ = false;
    bool shapeAvailable_ = false;

    Surface cand_;
    Surface overlays_[kOverlayCount]; // 下标 = kind - 1（OVERLAY_KIND_TOOLTIP..TOAST）
    /// 候选窗实际落点 − 服务端建议落点（被翻转/钳制过时非零）。FOLLOW_CANDIDATE 的 tooltip 据此平移。
    int32_t candShiftX_ = 0;
    int32_t candShiftY_ = 0;

    std::vector<CandidateHitRect> rects_;
    int32_t hover_ = -1;
};

} // namespace windlinux
