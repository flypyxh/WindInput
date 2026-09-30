// X11 候选窗 + 光栅浮层 + 自绘菜单：把服务光栅化好的 BGRA 帧贴到 override-redirect 顶层窗口上。
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
#include "ExtProtocol.h"
#include "ShmFrame.h"

#include <fcitx-utils/event.h>

#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>
#include <xcb/xcb.h>

namespace windlinux {

class X11CandidatePanel {
public:
    struct Callbacks {
        std::function<void(int32_t index)> select; // 候选下标；-1 上页 / -2 下页
        std::function<void(int32_t index)> hover;  // -1 = 离开
        std::function<void(int32_t delta)> scroll; // WHEEL_DELTA(120) 倍数，正 = 上滚
        /// 候选窗上右键：target ≥ 0 为命中的候选（页内下标），-1 为空白 / 翻页按钮（主菜单）；
        /// (x, y) 为根窗口坐标。
        std::function<void(int32_t target, int32_t x, int32_t y)> contextMenu;
        /// 菜单打开期间的指针事件（MENU_POINTER_*、X11 按键号、根窗口坐标）→ CMD_MENU_POINTER。
        std::function<void(uint32_t event, uint32_t button, int32_t x, int32_t y)> menuPointer;
        /// 本端自己把菜单收掉了（空闲超时…），reason 报给服务端复位 menu_open。
        std::function<void(const std::string& reason)> menuDismissed;
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

    /// 自绘菜单第 `level` 级（CMD_OVERLAY_FRAME kind = OVERLAY_KIND_MENU + level）：按 EXACT
    /// 原样摆放。第一级出现时抓住指针（点菜单外能看见、并且不漏给下面的应用）、起空闲计时。
    /// 菜单打开期间，本连接上的全部指针事件（含候选窗上的）只报给 `menuPointer`。
    bool showMenuLevel(uint32_t level, const SharedFrame& frame, const OverlayFramePayload& p);
    /// 服务端推来的隐藏帧：摘掉这一级；全部摘掉后放开指针（这条路不报 dismiss——是服务端关的）。
    void hideMenuLevel(uint32_t level);
    /// 本端收菜单（全部级）。`reason` 非空时经 `menuDismissed` 报给服务端。
    void closeMenu(const char* reason);
    bool menuOpen() const;
    /// 有菜单相关的键盘活动（按键转给服务端的那一刻）：空闲计时重新起算。
    void noteMenuActivity();
    /// 工作区（根窗口，不分显示器）。连不上 X 时返回空。
    std::optional<Rect> screenWorkArea();
    /// 当前指针位置（根窗口坐标）。连不上 X 时返回空。
    std::optional<std::pair<int32_t, int32_t>> pointerPosition();

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
    static constexpr size_t kMenuLevels = OVERLAY_MENU_LEVELS;

    bool ensureConnection();
    void dropConnection();
    bool hasCompositor();
    bool ensureWindow(Surface& s, bool argb);
    void destroyWindow(Surface& s);
    /// `raise` = 顺带提到最上层。菜单的重绘（高亮变化）不能重排 z 序：子菜单翻到左侧压在
    /// 父菜单上时，父菜单一次重绘就会把它盖住（同 Windows `plan_render` 的教训）。
    bool present(Surface& s, const SharedFrame& frame, const Rect& r, bool raise = true);
    void unmap(Surface& s);
    void upload(Surface& s, const SharedFrame& frame);
    void applyShape(Surface& s, const SharedFrame& frame);
    Surface* overlay(uint32_t kind);
    Rect workArea() const;
    void onReadable();
    void handleEvent(xcb_generic_event_t* ev);
    void setHover(int32_t index);
    /// 菜单打开期间的指针事件：移动合并到本批末尾再报，按下立即报（先把积着的移动报掉保序）。
    void handleMenuEvent(xcb_generic_event_t* ev);
    void flushMenuMotion();
    void grabPointer();
    void releasePointer();
    void armMenuIdle();
    /// 摘掉全部菜单窗口、放开指针、停计时（不报 dismiss）。`fromIdleTimer`：由空闲计时器
    /// 回调调用时不能销毁那个计时器本身。
    void dropMenu(bool fromIdleTimer = false);

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

    Surface menus_[kMenuLevels];
    bool pointerGrabbed_ = false;
    int grabAttempts_ = 0;
    std::unique_ptr<fcitx::EventSourceTime> grabRetry_;
    std::unique_ptr<fcitx::EventSourceTime> menuIdle_;
    uint32_t menuIdleMs_;
    std::optional<std::pair<int32_t, int32_t>> pendingMotion_;
};

} // namespace windlinux
