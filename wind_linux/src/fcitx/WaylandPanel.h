// Wayland 候选窗：把服务光栅化好的 BGRA 帧贴到 input-method-v2 的 popup surface 上。
//
// X11 路径（X11Panel）靠 `cursorRect()` 是屏幕坐标这一条自己摆窗口；Wayland 客户端报的却是
// 窗口相对坐标，应用的屏幕位置只有合成器知道。popup surface 正是为此设计的：把 wl_surface 交给
// `zwp_input_method_v2.get_input_popup_surface`，合成器按文本光标矩形自己摆（含翻转、避让）。
// 所以本类**不接收也不使用任何坐标**。
//
// 连接：借 Fcitx5 wayland 模块已建好的 `wl_display`（popup 必须与输入法对象同一连接），对象建在
// 默认队列上，事件由 Fcitx5 的事件读取线程 / 主线程分发，本类不另起 IO。
// 输入法对象：由 waylandim 前端持有（一个座位只能有一个输入法绑定），经 addon 函数
// `getInputMethodV2(ic)` 取得——它返回 Fcitx5 私有包装类，C 层指针见 `rawProxyOf`。
#pragma once

#include "Codec.h"
#include "ExtProtocol.h"
#include "ShmFrame.h"

#include <fcitx-utils/event.h>
#include <fcitx-utils/handlertable.h>
#include <fcitx/addoninstance.h>
#include <fcitx/inputcontext.h>
#include <fcitx/instance.h>

#include <functional>
#include <memory>
#include <optional>
#include <utility>
#include <string>
#include <vector>

struct wl_display;
struct wl_registry;
struct wl_compositor;
struct wl_shm;
struct wl_surface;
struct wl_buffer;
struct wl_seat;
struct wl_pointer;
struct wl_output;
struct zxdg_output_manager_v1;
struct zxdg_output_v1;
struct wp_viewporter;
struct wp_viewport;
struct zwp_input_method_v2;
struct zwp_input_popup_surface_v2;

namespace windlinux {

class WaylandCandidatePanel {
public:
    explicit WaylandCandidatePanel(fcitx::Instance* instance);
    ~WaylandCandidatePanel();

    /// 该 IC 是否由本类负责（Wayland 原生前端 `wayland_v2`）。其余（XIM / DBus 下的 X11 应用…）
    /// 走 X11 窗口。
    static bool handles(const fcitx::InputContext* ic);
    /// 已拿到连接且 wl_compositor / wl_shm 就绪。
    bool ready() const { return compositor_ && shm_; }

    /// 贴一帧并显示。失败（没就绪 / 取不到输入法对象 / 校验不过）返回 false，调用方不退路——
    /// 只是这一帧不显示，原因已记日志。
    bool show(fcitx::InputContext* ic, const SharedFrame& frame);
    /// 只收起候选这一层（其余浮层还在就继续显示）。
    void hide();
    /// 浮层（kind 同 ExtProtocol.h 的 OVERLAY_KIND_*：1 tooltip / 2 状态气泡 / 3 Toast）。
    /// Wayland 下只有 popup 这一个 surface 能由合成器按光标摆位，所以浮层不另开窗口，
    /// 与候选一起竖排合成进同一张位图：候选在上，其后依次是 tooltip、状态气泡、Toast。
    /// `durationMs` > 0 到点自动收起。
    bool showOverlay(fcitx::InputContext* ic, uint32_t kind, const SharedFrame& frame,
                     const OverlayFramePayload& geo);
    void hideOverlay(uint32_t kind);
    /// 全部层一并收起（焦点走了 / 服务断了）。
    void hideAll();
    /// 候选窗所在输出的界面缩放因子（1.0 = 100%）。取自 wl_output 的模式尺寸 / xdg-output 的逻辑
    /// 尺寸（分数缩放下也准），没有 xdg-output 时退回 wl_output 的整数缩放；候选窗出现过后跟着它
    /// 进入的输出走，之前用第一个输出。还没连上 / 没有输出信息时返回 1.0。
    double scale() const;
    /// 告知本类「服务端此刻按多大的缩放在光栅化」。贴图时按它折回逻辑尺寸——帧里的缩放字段是
    /// 取整的，分数缩放下不能用。引擎每次向服务端上报缩放时同步给这里。
    void setReportedScale(double s) { reportedScale_ = s > 0 ? s : 1.0; }
    /// 输出的缩放变了（热插拔、系统设置里改了缩放、候选窗换了屏）：引擎据此重新上报。
    void setOnScaleChanged(std::function<void()> cb) { onScaleChanged_ = std::move(cb); }
    /// 鼠标交互回调（语义同 X11CandidatePanel::Callbacks 的同名项）。
    struct Callbacks {
        std::function<void(int32_t index)> select; // 候选下标；-1 上页 / -2 下页
        std::function<void(int32_t index)> hover;  // -1 = 离开
        std::function<void(int32_t delta)> scroll; // WHEEL_DELTA(120) 倍数，正 = 上滚
        /// 右键：target 同 `menu.open`；x y 是 popup 位图里的像素坐标。
        std::function<void(int32_t target, int32_t x, int32_t y)> contextMenu;
        /// 菜单开着期间的指针事件（event 取 MENU_POINTER_*，button 左 1 中 2 右 3）。
        std::function<void(uint32_t event, uint32_t button, int32_t x, int32_t y)> menuPointer;
    };
    /// 自绘菜单（服务端画、服务端命中）：同样并进 popup 位图。菜单的坐标系原点 = popup 位图
    /// 左上角，工作区见 `virtualWorkArea`。
    bool showMenuLevel(fcitx::InputContext* ic, uint32_t level, const SharedFrame& frame,
                       const OverlayFramePayload& p);
    void hideMenuLevel(uint32_t level);
    bool menuOpen() const;
    /// 本端收起菜单（不报服务端）。
    void closeMenu();
    Rect virtualWorkArea() const;
    void setCallbacks(Callbacks cb) { cb_ = std::move(cb); }
    /// 服务端推来的命中表（位图像素坐标）。
    void setRects(std::vector<CandidateHitRect> rects) { rects_ = std::move(rects); }

private:
    struct Output {
        WaylandCandidatePanel* owner = nullptr;
        uint32_t name = 0; // registry 名
        wl_output* wl = nullptr;
        zxdg_output_v1* xdg = nullptr;
        int32_t scaleInt = 1;
        int32_t modeW = 0, modeH = 0; // 当前模式（物理像素）
        int32_t logW = 0, logH = 0;   // xdg-output 逻辑尺寸
    };
    static double outputScale(const Output& o);
    void attachXdgOutput(Output& o);
    void scaleMaybeChanged();
    void onConnection(const std::string& name, wl_display* display);
    void onClosed(wl_display* display);
    void onGlobal(uint32_t name, const char* interface, uint32_t version);
    /// 丢掉全部 Wayland 对象（连接已死时不能再调 destroy 请求，故分两种）。
    void teardown(bool connectionAlive);
    bool ensurePopup(fcitx::InputContext* ic);
    /// 把各层合成一张位图贴上去；没有任何层在显示就收起 popup。
    bool present(fcitx::InputContext* ic);
    /// `inputW/H`：位图里接收鼠标的那一块（候选窗）的像素尺寸，0 = 整张都不接收。
    bool attachPixels(const uint8_t* data, uint32_t w, uint32_t h, uint32_t stride,
                      uint32_t inputW, uint32_t inputH);
    void hidePopup();
    void dropPopup();
    void releaseBuffer(wl_buffer* b);
    static zwp_input_method_v2* rawProxyOf(void* wrapper);

    fcitx::Instance* instance_;
    std::unique_ptr<fcitx::HandlerTableEntryBase> connCreated_;
    std::unique_ptr<fcitx::HandlerTableEntryBase> connClosed_;

    wl_display* display_ = nullptr;
    wl_registry* registry_ = nullptr;
    wl_compositor* compositor_ = nullptr;
    uint32_t compositorVersion_ = 0;
    wl_shm* shm_ = nullptr;
    std::vector<std::unique_ptr<Output>> outputs_;
    Output* currentOutput_ = nullptr; // 候选窗最近进入的输出
    zxdg_output_manager_v1* xdgManager_ = nullptr;
    wp_viewporter* viewporter_ = nullptr;
    wp_viewport* viewport_ = nullptr; // 随 surface 建 / 毁
    double reportedScale_ = 1.0;
    double lastNotifiedScale_ = 1.0;
    std::function<void()> onScaleChanged_;
    wl_seat* seat_ = nullptr;
    wl_pointer* pointer_ = nullptr;
    wl_surface* surface_ = nullptr;
    zwp_input_method_v2* im_ = nullptr;      // 当前 popup 挂在哪个输入法对象上
    zwp_input_popup_surface_v2* popup_ = nullptr;
    wl_buffer* buffer_ = nullptr;            // 最近一次 attach 的（release 时销毁）
    // 层 0 = 候选，1..3 = OVERLAY_KIND_TOOLTIP/STATUS/TOAST。
    static constexpr int kLayers = 4;
    struct Layer {
        SharedFrame frame;
        OverlayFramePayload geo; // 浮层的落位说明（tooltip 按它相对候选窗摆）
        bool on = false;
        std::unique_ptr<fcitx::EventSourceTime> hideTimer;
    };
    Layer layers_[kLayers];
    static constexpr size_t kMenuLevels = OVERLAY_MENU_LEVELS;
    struct MenuLayer {
        SharedFrame frame;
        Rect rect; // 位图在 popup 坐标系里的矩形（含阴影扩边）
        bool on = false;
    };
    MenuLayer menus_[kMenuLevels];
    std::optional<std::pair<int32_t, int32_t>> lastMenuMotion_;
    void reportMenuPointer(uint32_t event, uint32_t button);
    Callbacks cb_;
    std::vector<CandidateHitRect> rects_;
    bool pointerInside_ = false;
    double pointerX_ = 0, pointerY_ = 0; // surface 内逻辑坐标
    int32_t hover_ = -1;
    /// 指针在位图里的命中（kNoHit / 候选下标 / 翻页 -1 -2）。
    int32_t hitAtPointer() const;
    void setHover(int32_t index);
    void updateHover();
    bool warnedProbe_ = false;
    bool warnedNoIm_ = false;

    friend struct WaylandListeners;
};

} // namespace windlinux
