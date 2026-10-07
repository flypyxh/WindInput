#include "WaylandPanel.h"

#include "WaylandIm.h"

#include <fcitx/addoninstance.h>
#include <wayland_public.h>

#include <fcitx-utils/log.h>
#include <fcitx/addonmanager.h>

#include <wayland-client.h>
#include "input-method-unstable-v2-client-protocol.h"
#include "viewporter-client-protocol.h"
#include "xdg-output-unstable-v1-client-protocol.h"

#include <algorithm>
#include "ExtProtocol.h"
#include "Menu.h"

#include <fcitx-utils/event.h>
#include <fcitx-utils/misc.h>

#include <cmath>
#include <cstring>
#include <sys/mman.h>
#include <sys/uio.h>
#include <unistd.h>

#include <cstdlib>

FCITX_DECLARE_LOG_CATEGORY(windinput_log);

#define WIND_DEBUG() FCITX_LOGC(windinput_log, Debug)
#define WIND_WARN() FCITX_LOGC(windinput_log, Warn)
#define WIND_INFO() FCITX_LOGC(windinput_log, Info)

namespace windlinux {

namespace {

/// 读本进程内任意地址：地址无效时返回 false 而不是崩溃（`rawProxyOf` 在猜指针）。
bool safeRead(const void* addr, void* out, size_t n)
{
    struct iovec local{out, n};
    struct iovec remote{const_cast<void*>(addr), n};
    return process_vm_readv(getpid(), &local, 1, &remote, 1, 0) == ssize_t(n);
}

} // namespace

/// libwayland 的 C 回调落点（需要访问私有成员，故做成友元）。
struct WaylandListeners {
    static void registryGlobal(void* data, wl_registry*, uint32_t name, const char* iface,
                               uint32_t version)
    {
        static_cast<WaylandCandidatePanel*>(data)->onGlobal(name, iface, version);
    }
    static void registryRemove(void*, wl_registry*, uint32_t) {}

    static void bufferRelease(void* data, wl_buffer* b)
    {
        static_cast<WaylandCandidatePanel*>(data)->releaseBuffer(b);
    }

    static const wl_pointer_listener kPtr;
    static void seatCaps(void* data, wl_seat* seat, uint32_t caps)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        if ((caps & WL_SEAT_CAPABILITY_POINTER) && !self->pointer_) {
            self->pointer_ = wl_seat_get_pointer(seat);
            wl_pointer_add_listener(self->pointer_, &kPtr, self);
        } else if (!(caps & WL_SEAT_CAPABILITY_POINTER) && self->pointer_) {
            wl_pointer_destroy(self->pointer_);
            self->pointer_ = nullptr;
        }
    }
    static void seatName(void*, wl_seat*, const char*) {}

    // 鼠标：本类自己绑一份 wl_pointer（与 Fcitx5 classicui 那份互不影响，各收各的事件）。
    static void ptrEnter(void* data, wl_pointer*, uint32_t, wl_surface* surface, wl_fixed_t x,
                         wl_fixed_t y)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        if (surface != self->surface_) {
            return;
        }
        self->pointerInside_ = true;
        self->pointerX_ = wl_fixed_to_double(x);
        self->pointerY_ = wl_fixed_to_double(y);
        self->updateHover();
    }
    static void ptrLeave(void* data, wl_pointer*, uint32_t, wl_surface* surface)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        if (surface != self->surface_) {
            return;
        }
        self->pointerInside_ = false;
        self->setHover(-1);
    }
    static void ptrMotion(void* data, wl_pointer*, uint32_t, wl_fixed_t x, wl_fixed_t y)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        if (!self->pointerInside_) {
            return;
        }
        self->pointerX_ = wl_fixed_to_double(x);
        self->pointerY_ = wl_fixed_to_double(y);
        if (self->menuOpen()) {
            self->reportMenuPointer(MENU_POINTER_MOTION, 0);
            return;
        }
        self->updateHover();
    }
    static void ptrButton(void* data, wl_pointer*, uint32_t, uint32_t, uint32_t button,
                          uint32_t state)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        // linux/input-event-codes.h：BTN_LEFT / BTN_RIGHT / BTN_MIDDLE
        constexpr uint32_t kBtnLeft = 0x110, kBtnRight = 0x111, kBtnMiddle = 0x112;
        if (!self->pointerInside_ || state != WL_POINTER_BUTTON_STATE_PRESSED) {
            return;
        }
        if (self->menuOpen()) {
            // 菜单的命中、高亮、子菜单都在服务端；按 X11 的编号（左 1 中 2 右 3）报上去
            if (button == kBtnLeft || button == kBtnMiddle || button == kBtnRight) {
                self->reportMenuPointer(MENU_POINTER_PRESS,
                                        button == kBtnLeft ? 1 : button == kBtnMiddle ? 2 : 3);
            }
            return;
        }
        if (button == kBtnLeft) {
            const int32_t idx = self->hitAtPointer();
            if (idx != kNoHit && self->cb_.select) {
                self->cb_.select(idx);
            }
        } else if (button == kBtnRight && self->cb_.contextMenu) {
            // 命中候选 → 候选菜单，空白 / 翻页按钮 → 主菜单（同 X11 与 Windows）。
            // 悬停先清掉：菜单开着时候选窗收不到悬停，残留的高亮会一直挂着。
            const int32_t hit = self->hitAtPointer();
            self->setHover(-1);
            self->cb_.contextMenu(contextMenuTarget(hit),
                                  int32_t(std::lround(self->pointerX_ * self->reportedScale_)),
                                  int32_t(std::lround(self->pointerY_ * self->reportedScale_)));
        }
    }
    static void ptrAxis(void* data, wl_pointer*, uint32_t, uint32_t axis, wl_fixed_t value)
    {
        auto* self = static_cast<WaylandCandidatePanel*>(data);
        // 只认竖直滚轮（axis 0）；正值 = 向下，协议上「正 = 上滚」与它相反。
        if (!self->pointerInside_ || axis != WL_POINTER_AXIS_VERTICAL_SCROLL || value == 0
            || !self->cb_.scroll) {
            return;
        }
        self->cb_.scroll(value > 0 ? -120 : 120);
    }

    // ── 输出与缩放 ──
    static void outGeometry(void*, wl_output*, int32_t, int32_t, int32_t, int32_t, int32_t,
                            const char*, const char*, int32_t)
    {
    }
    static void outMode(void* data, wl_output*, uint32_t flags, int32_t w, int32_t h, int32_t)
    {
        auto* o = static_cast<WaylandCandidatePanel::Output*>(data);
        if (flags & WL_OUTPUT_MODE_CURRENT) {
            o->modeW = w;
            o->modeH = h;
            o->owner->scaleMaybeChanged();
        }
    }
    static void outDone(void*, wl_output*) {}
    static void outScale(void* data, wl_output*, int32_t factor)
    {
        auto* o = static_cast<WaylandCandidatePanel::Output*>(data);
        o->scaleInt = factor > 0 ? factor : 1;
        o->owner->scaleMaybeChanged();
    }
    static void xdgPos(void*, zxdg_output_v1*, int32_t, int32_t) {}
    static void xdgSize(void* data, zxdg_output_v1*, int32_t w, int32_t h)
    {
        auto* o = static_cast<WaylandCandidatePanel::Output*>(data);
        o->logW = w;
        o->logH = h;
        o->owner->scaleMaybeChanged();
    }
    static void xdgDone(void*, zxdg_output_v1*) {}
    static void xdgName(void*, zxdg_output_v1*, const char*) {}
    static void xdgDesc(void*, zxdg_output_v1*, const char*) {}
    // 本回调的 data 是占位包装（见 ensurePopup），真正的 this 存在占位内存的 kFakeSelfOffset 处。
    static constexpr size_t kFakeSelfOffset = 512;
    static void surfaceEnter(void* data, wl_surface*, wl_output* out)
    {
        auto* self = *reinterpret_cast<WaylandCandidatePanel**>(static_cast<char*>(data) +
                                                                kFakeSelfOffset);
        for (auto& o : self->outputs_) {
            if (o->wl == out) {
                self->currentOutput_ = o.get();
                self->scaleMaybeChanged();
                break;
            }
        }
    }
    static void surfaceLeave(void*, wl_surface*, wl_output*) {}

    static void popupRect(void*, zwp_input_popup_surface_v2*, int32_t x, int32_t y, int32_t w,
                          int32_t h)
    {
        // 合成器告知的文本光标矩形（popup 自己的坐标系里）。仅诊断用：摆位是合成器的事。
        static int32_t last[4] = {-1, -1, -1, -1};
        if (last[0] != x || last[1] != y || last[2] != w || last[3] != h) {
            last[0] = x, last[1] = y, last[2] = w, last[3] = h;
            WIND_DEBUG() << "popup text_input_rectangle (" << x << "," << y << " " << w << "x" << h
                        << ")";
        }
    }
};

namespace {
const wl_registry_listener kRegistryListener = {WaylandListeners::registryGlobal,
                                                WaylandListeners::registryRemove};
const wl_buffer_listener kBufferListener = {WaylandListeners::bufferRelease};
const zwp_input_popup_surface_v2_listener kPopupListener = {WaylandListeners::popupRect};
const wl_output_listener kOutputListener = [] {
    wl_output_listener l{}; // 绑 v≤3：name / description（v4）不会来；逐成员赋值免得各版本头文件成员数不同
    l.geometry = WaylandListeners::outGeometry;
    l.mode = WaylandListeners::outMode;
    l.done = WaylandListeners::outDone;
    l.scale = WaylandListeners::outScale;
    return l;
}();
const zxdg_output_v1_listener kXdgOutputListener = [] {
    zxdg_output_v1_listener l{};
    l.logical_position = WaylandListeners::xdgPos;
    l.logical_size = WaylandListeners::xdgSize;
    l.done = WaylandListeners::xdgDone;
    l.name = WaylandListeners::xdgName;
    l.description = WaylandListeners::xdgDesc;
    return l;
}();
const wl_surface_listener kSurfaceListener = [] {
    wl_surface_listener l{};
    l.enter = WaylandListeners::surfaceEnter;
    l.leave = WaylandListeners::surfaceLeave;
    return l;
}();
const wl_seat_listener kSeatListener = {WaylandListeners::seatCaps, WaylandListeners::seatName};
} // namespace

// 绑的是 wl_pointer v1，较新的 frame / axis_* 事件不会来；逐成员赋值免得各版本头文件的成员数不同而告警。
const wl_pointer_listener WaylandListeners::kPtr = [] {
    wl_pointer_listener l{};
    l.enter = ptrEnter;
    l.leave = ptrLeave;
    l.motion = ptrMotion;
    l.button = ptrButton;
    l.axis = ptrAxis;
    return l;
}();

WaylandCandidatePanel::WaylandCandidatePanel(fcitx::Instance* instance) : instance_(instance)
{
    // wayland 模块：Fcitx5 自己开的 Wayland 连接。回调对已存在的连接也会补调一次。
    auto* wayland = instance_->addonManager().addon("wayland", true);
    if (!wayland) {
        WIND_DEBUG() << "没有 wayland 模块，Wayland 候选窗不可用";
        return;
    }
    connCreated_ = wayland->call<fcitx::IWaylandModule::addConnectionCreatedCallback>(
        [this](const std::string& name, wl_display* display, fcitx::FocusGroup*) {
            onConnection(name, display);
        });
    connClosed_ = wayland->call<fcitx::IWaylandModule::addConnectionClosedCallback>(
        [this](const std::string&, wl_display* display) { onClosed(display); });
}

WaylandCandidatePanel::~WaylandCandidatePanel()
{
    connCreated_.reset();
    connClosed_.reset();
    teardown(display_ != nullptr);
}

bool WaylandCandidatePanel::handles(const fcitx::InputContext* ic)
{
    return ic && std::strcmp(ic->frontend(), "wayland_v2") == 0;
}

void WaylandCandidatePanel::onConnection(const std::string& name, wl_display* display)
{
    if (display_) {
        return; // 只服务第一条连接（桌面会话只有一个）
    }
    display_ = display;
    registry_ = wl_display_get_registry(display_);
    wl_registry_add_listener(registry_, &kRegistryListener, this);
    wl_display_flush(display_);
    WIND_INFO() << "Wayland 候选窗：借用 Fcitx5 的连接 " << name;
}

void WaylandCandidatePanel::onClosed(wl_display* display)
{
    if (display == display_) {
        teardown(false);
    }
}

void WaylandCandidatePanel::onGlobal(uint32_t name, const char* iface, uint32_t version)
{
    if (std::strcmp(iface, "wl_compositor") == 0) {
        compositorVersion_ = version < 4 ? version : 4;
        compositor_ = static_cast<wl_compositor*>(
            wl_registry_bind(registry_, name, &wl_compositor_interface, compositorVersion_));
    } else if (std::strcmp(iface, "wl_shm") == 0) {
        shm_ = static_cast<wl_shm*>(wl_registry_bind(registry_, name, &wl_shm_interface, 1));
    } else if (std::strcmp(iface, "wl_output") == 0) {
        auto o = std::make_unique<Output>();
        o->owner = this;
        o->name = name;
        const uint32_t v = version < 3 ? version : 3;
        o->wl = static_cast<wl_output*>(wl_registry_bind(registry_, name, &wl_output_interface, v));
        wl_output_add_listener(o->wl, &kOutputListener, o.get());
        attachXdgOutput(*o);
        outputs_.push_back(std::move(o));
    } else if (std::strcmp(iface, "zxdg_output_manager_v1") == 0 && !xdgManager_) {
        const uint32_t v = version < 3 ? version : 3;
        xdgManager_ = static_cast<zxdg_output_manager_v1*>(
            wl_registry_bind(registry_, name, &zxdg_output_manager_v1_interface, v));
        for (auto& o : outputs_) {
            attachXdgOutput(*o);
        }
    } else if (std::strcmp(iface, "wp_viewporter") == 0 && !viewporter_) {
        viewporter_ = static_cast<wp_viewporter*>(
            wl_registry_bind(registry_, name, &wp_viewporter_interface, 1));
    } else if (std::strcmp(iface, "wl_seat") == 0 && !seat_) {
        seat_ = static_cast<wl_seat*>(wl_registry_bind(registry_, name, &wl_seat_interface, 1));
        wl_seat_add_listener(seat_, &kSeatListener, this);
    }
}

void WaylandCandidatePanel::attachXdgOutput(Output& o)
{
    if (!xdgManager_ || o.xdg) {
        return;
    }
    o.xdg = zxdg_output_manager_v1_get_xdg_output(xdgManager_, o.wl);
    zxdg_output_v1_add_listener(o.xdg, &kXdgOutputListener, &o);
}

double WaylandCandidatePanel::outputScale(const Output& o)
{
    double s = o.scaleInt > 0 ? o.scaleInt : 1;
    // 分数缩放下 wl_output.scale 是向上取整的整数，真实比例要看「模式像素 / 逻辑尺寸」；
    // 用面积比的平方根，转屏（模式宽高与逻辑宽高互换）时同样成立。
    if (o.modeW > 0 && o.modeH > 0 && o.logW > 0 && o.logH > 0) {
        s = std::sqrt(double(o.modeW) * o.modeH / (double(o.logW) * o.logH));
    }
    s = std::round(s * 100.0) / 100.0;
    return s < 1.0 ? 1.0 : (s > 4.0 ? 4.0 : s);
}

double WaylandCandidatePanel::scale() const
{
    const Output* o = currentOutput_ ? currentOutput_ : (outputs_.empty() ? nullptr : outputs_[0].get());
    return o ? outputScale(*o) : 1.0;
}

void WaylandCandidatePanel::scaleMaybeChanged()
{
    const double s = scale();
    if (s == lastNotifiedScale_) {
        return;
    }
    lastNotifiedScale_ = s;
    WIND_INFO() << "输出缩放变为 " << s;
    if (onScaleChanged_) {
        onScaleChanged_();
    }
}

void WaylandCandidatePanel::teardown(bool alive)
{
    if (alive) {
        dropPopup();
        if (buffer_) {
            wl_buffer_destroy(buffer_);
        }
        if (surface_) {
            wl_surface_destroy(surface_);
        }
        if (viewport_) {
            wp_viewport_destroy(viewport_);
        }
        if (viewporter_) {
            wp_viewporter_destroy(viewporter_);
        }
        for (auto& o : outputs_) {
            if (o->xdg) {
                zxdg_output_v1_destroy(o->xdg);
            }
            wl_output_destroy(o->wl);
        }
        if (xdgManager_) {
            zxdg_output_manager_v1_destroy(xdgManager_);
        }
        if (pointer_) {
            wl_pointer_destroy(pointer_);
        }
        if (seat_) {
            wl_seat_destroy(seat_);
        }
        if (shm_) {
            wl_shm_destroy(shm_);
        }
        if (compositor_) {
            wl_compositor_destroy(compositor_);
        }
        if (registry_) {
            wl_registry_destroy(registry_);
        }
        if (display_) {
            wl_display_flush(display_);
        }
    }
    buffer_ = nullptr;
    surface_ = nullptr;
    popup_ = nullptr;
    im_ = nullptr;
    viewport_ = nullptr;
    viewporter_ = nullptr;
    outputs_.clear();
    currentOutput_ = nullptr;
    xdgManager_ = nullptr;
    pointer_ = nullptr;
    seat_ = nullptr;
    shm_ = nullptr;
    compositor_ = nullptr;
    registry_ = nullptr;
    display_ = nullptr;
}

void WaylandCandidatePanel::dropPopup()
{
    if (popup_) {
        zwp_input_popup_surface_v2_destroy(popup_);
        popup_ = nullptr;
    }
    im_ = nullptr;
}

void WaylandCandidatePanel::releaseBuffer(wl_buffer* b)
{
    if (b == buffer_) {
        buffer_ = nullptr;
    }
    wl_buffer_destroy(b);
}

zwp_input_method_v2* WaylandCandidatePanel::rawProxyOf(void* wrapper)
{
    // Fcitx5 的 `fcitx::wayland::ZwpInputMethodV2` 把 C 代理存在某个成员里（版本间偏移不同），
    // 且开发包不给头文件。它构造时把 `this` 登记为该代理的 listener 用户数据，于是可以反推：
    // 逐个 8 字节槽取候选指针，校验「是 wl_proxy、类名对、用户数据正是这个包装对象」，三条都
    // 对才认。校验不过就老实返回空——宁可这一路不可用，也不去碰一个猜错的指针。
    constexpr size_t kScanBytes = 192;
    for (size_t off = 0; off + sizeof(void*) <= kScanBytes; off += sizeof(void*)) {
        void* cand = nullptr;
        std::memcpy(&cand, static_cast<char*>(wrapper) + off, sizeof(cand));
        if (!cand || (reinterpret_cast<uintptr_t>(cand) & 7) != 0) {
            continue;
        }
        // wl_proxy 开头是 wl_object { const wl_interface* interface; const void* implementation; uint32 id; }
        const void* iface = nullptr;
        if (!safeRead(cand, &iface, sizeof(iface)) || !iface) {
            continue;
        }
        const char* namePtr = nullptr;
        if (!safeRead(iface, &namePtr, sizeof(namePtr)) || !namePtr) {
            continue;
        }
        char name[32] = {};
        if (!safeRead(namePtr, name, sizeof(name) - 1)) {
            continue;
        }
        if (std::strcmp(name, "zwp_input_method_v2") != 0) {
            continue;
        }
        auto* proxy = static_cast<wl_proxy*>(cand);
        if (wl_proxy_get_user_data(proxy) != wrapper) {
            continue;
        }
        return reinterpret_cast<zwp_input_method_v2*>(proxy);
    }
    return nullptr;
}

bool WaylandCandidatePanel::ensurePopup(fcitx::InputContext* ic)
{
    auto* waylandim = instance_->addonManager().addon("waylandim");
    if (!waylandim) {
        return false;
    }
    auto* wrapper = waylandim->call<fcitx::IWaylandIMModule::getInputMethodV2>(ic);
    if (!wrapper) {
        if (!warnedNoIm_) {
            warnedNoIm_ = true;
            WIND_WARN() << "waylandim 没给出 zwp_input_method_v2，Wayland 候选窗不可用";
        }
        return false;
    }
    zwp_input_method_v2* im = rawProxyOf(wrapper);
    if (!im) {
        if (!warnedProbe_) {
            warnedProbe_ = true;
            WIND_WARN() << "认不出 Fcitx5 输入法对象里的 wl 代理（Fcitx5 内部布局与预期不同），"
                           "Wayland 候选窗不可用";
        }
        return false;
    }
    if (!surface_) {
        surface_ = wl_compositor_create_surface(compositor_);
        // Fcitx5 classicui 对本连接上每个 wl_surface 的指针进入事件都会取 user_data 当它自己的
        // 包装对象解引用（`surface->userData()`）：我们的 surface 它不认识，user_data 为空就崩
        // （实测鼠标一移上候选窗就带崩 fcitx5）。给一块全零的内存，它读到的 userData 是空指针，
        // 按「不是我的窗口」直接返回。包装类内部布局没有头文件可对，故留足余量而不依赖偏移。
        // 注意 wl_proxy_add_listener 会把 user_data 覆盖成它的 data 参数：listener 的 data 也必须
        // 传占位（实测传 this 时，classicui 把 this 当它的窗口对象解引用，照样崩）；
        // this 另存进占位内存的 kFakeSelfOffset 处供回调取回。
        static char fakeWrapper[1024] = {};
        *reinterpret_cast<WaylandCandidatePanel**>(fakeWrapper + WaylandListeners::kFakeSelfOffset) =
            this;
        wl_surface_add_listener(surface_, &kSurfaceListener, fakeWrapper);
        wl_surface_set_user_data(surface_, fakeWrapper);
        WIND_INFO() << "Wayland 候选窗：surface 已建，输入法对象 " << static_cast<void*>(im);
    }
    if (popup_ && im_ == im) {
        return true;
    }
    dropPopup();
    popup_ = zwp_input_method_v2_get_input_popup_surface(im, surface_);
    zwp_input_popup_surface_v2_add_listener(popup_, &kPopupListener, this);
    im_ = im;
    WIND_INFO() << "Wayland 候选窗：已把 surface 登记为 input popup";
    return true;
}

bool WaylandCandidatePanel::show(fcitx::InputContext* ic, const SharedFrame& f)
{
    layers_[0].frame = f;
    layers_[0].on = true;
    return present(ic);
}

bool WaylandCandidatePanel::showOverlay(fcitx::InputContext* ic, uint32_t kind,
                                        const SharedFrame& f, const OverlayFramePayload& geo)
{
    const int32_t durationMs = geo.durationMs;
    if (kind < 1 || kind >= kLayers) {
        return false;
    }
    Layer& l = layers_[kind];
    l.frame = f;
    l.geo = geo;
    l.geo.width = f.width;
    l.geo.height = f.height;
    l.on = true;
    l.hideTimer.reset();
    if (durationMs > 0) {
        l.hideTimer = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + uint64_t(durationMs) * 1000, 0,
            [this, kind](fcitx::EventSourceTime*, uint64_t) {
                // 回调里不能销毁自己：只改状态，计时器对象留到下次 show / hide 再换。
                WIND_DEBUG() << "浮层 kind=" << kind << " 到时自动收起";
                layers_[kind].on = false;
                if (auto* ic = instance_->mostRecentInputContext()) {
                    present(ic);
                } else {
                    hidePopup();
                }
                return true;
            });
    }
    return present(ic);
}

void WaylandCandidatePanel::hideOverlay(uint32_t kind)
{
    if (kind < 1 || kind >= kLayers || !layers_[kind].on) {
        return;
    }
    WIND_DEBUG() << "浮层 kind=" << kind << " 被服务端收起";
    layers_[kind].on = false;
    layers_[kind].hideTimer.reset();
    if (auto* ic = instance_->mostRecentInputContext()) {
        present(ic);
    } else {
        hidePopup();
    }
}

void WaylandCandidatePanel::hide()
{
    if (!layers_[0].on) {
        return;
    }
    layers_[0].on = false;
    pointerInside_ = false;
    hover_ = -1; // 服务端那边随候选窗收起自行复位悬停，这里只清本端状态
    rects_.clear();
    if (auto* ic = instance_->mostRecentInputContext()) {
        present(ic);
    } else {
        hidePopup();
    }
}

void WaylandCandidatePanel::hideAll()
{
    for (auto& l : layers_) {
        l.on = false;
        l.hideTimer.reset();
    }
    for (auto& m : menus_) {
        m.on = false;
    }
    lastMenuMotion_.reset();
    pointerInside_ = false;
    hover_ = -1;
    rects_.clear();
    hidePopup();
}

bool WaylandCandidatePanel::present(fcitx::InputContext* ic)
{
    // 凑出要显示的层并排版。候选恒在左上角（命中表坐标因此不受影响）；tooltip 在已知悬停行时
    // 贴在候选窗右侧、与该行顶对齐，否则与状态气泡、Toast 一起竖排在候选下方。层与层之间的
    // 间隙由各层自带的阴影扩边提供。
    struct Placed {
        const SharedFrame* f;
        int32_t x, y; // 菜单的阴影扩边可能落到负坐标（超出 surface 左上，被裁掉）
    };
    Placed placed[kLayers + kMenuLevels];
    int n = 0;
    auto usable = [](const Layer& l) {
        const SharedFrame& f = l.frame;
        return l.on && f.width != 0 && f.height != 0 && f.stride >= f.width * 4
               && f.bgra.size() >= size_t(f.stride) * f.height;
    };
    uint32_t candW = 0, candH = 0, canvasW = 0, canvasH = 0;
    auto grow = [&](const Placed& p) {
        canvasW = std::max<int64_t>(canvasW, int64_t(p.x) + p.f->width);
        canvasH = std::max<int64_t>(canvasH, int64_t(p.y) + p.f->height);
    };
    if (usable(layers_[0])) {
        placed[n++] = {&layers_[0].frame, 0, 0};
        candW = layers_[0].frame.width;
        candH = layers_[0].frame.height;
        grow(placed[0]);
    }
    int32_t stackY = int32_t(candH);
    for (int k = 1; k < kLayers; ++k) {
        if (!usable(layers_[k])) {
            continue;
        }
        const SharedFrame* f = &layers_[k].frame;
        Placed p{f, 0, int32_t(stackY)};
        bool stacked = true;
        if (k == 1 && candW > 0) {
            // 按服务端的落位（横排：行下方 / 行上方；竖排：窗口右 / 左侧）摆：它的坐标系里候选窗
            // 左上是 (screenX, screenY)，我们的位图里候选窗在 (0,0)，平移一下即可。
            Rect r = placeOverlay(layers_[1].geo, virtualWorkArea(), -layers_[0].frame.screenX,
                                  -layers_[0].frame.screenY);
            p = {f, r.x, r.y};
            stacked = false;
        }
        if (stacked) {
            stackY += int32_t(f->height);
        }
        placed[n++] = p;
        grow(p);
    }
    bool menuOn = false;
    for (size_t k = 0; k < kMenuLevels; ++k) {
        if (menus_[k].on && menus_[k].frame.width && menus_[k].frame.height
            && menus_[k].frame.bgra.size()
                   >= size_t(menus_[k].frame.stride) * menus_[k].frame.height) {
            placed[n] = {&menus_[k].frame, menus_[k].rect.x, menus_[k].rect.y};
            grow(placed[n]);
            ++n;
            menuOn = true;
        }
    }
    if (n == 0) {
        hidePopup();
        return true;
    }
    if (!ready() || !ensurePopup(ic)) {
        return false;
    }
    // 菜单开着时整张位图都收鼠标（菜单项的命中在服务端按位图坐标做）。
    if (menuOn) {
        candW = canvasW;
        candH = canvasH;
    }
    if (n == 1 && placed[0].x == 0 && placed[0].y == 0) {
        const SharedFrame& f = *placed[0].f;
        return attachPixels(f.bgra.data(), f.width, f.height, f.stride, candW, candH);
    }
    const uint32_t stride = canvasW * 4;
    std::vector<uint8_t> canvas(size_t(stride) * canvasH, 0);
    for (int i = 0; i < n; ++i) {
        const SharedFrame& f = *placed[i].f;
        // 裁掉落在 surface 左 / 上之外的部分
        const int32_t sx = std::max(0, -placed[i].x), sy = std::max(0, -placed[i].y);
        for (int32_t r = sy; r < int32_t(f.height); ++r) {
            uint8_t* dst = canvas.data() + size_t(placed[i].y + r) * stride
                           + size_t(placed[i].x + sx) * 4;
            const uint8_t* src = f.bgra.data() + size_t(r) * f.stride + size_t(sx) * 4;
            // 预乘 alpha 的 over：dst = src + dst * (1 - srcA)。多数层互不重叠（dst 为 0，等于拷贝）；
            // tooltip 会压到候选窗的阴影边上，需要真混合。
            for (uint32_t px = 0; px < f.width - sx; ++px, dst += 4, src += 4) {
                const uint32_t inv = 255 - src[3];
                for (int ch = 0; ch < 4; ++ch) {
                    dst[ch] = uint8_t(std::min<uint32_t>(255, src[ch] + (dst[ch] * inv + 127) / 255));
                }
            }
        }
    }
    return attachPixels(canvas.data(), canvasW, canvasH, stride, candW, candH);
}

bool WaylandCandidatePanel::attachPixels(const uint8_t* data, uint32_t w, uint32_t h,
                                         uint32_t stride, uint32_t inputW, uint32_t inputH)
{
    const size_t size = size_t(stride) * h;
    int fd = memfd_create("windinput-candidate", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, off_t(size)) != 0) {
        WIND_WARN() << "memfd 建不起来，丢这一帧";
        if (fd >= 0) {
            close(fd);
        }
        return false;
    }
    void* map = mmap(nullptr, size, PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) {
        close(fd);
        return false;
    }
    // 服务给的是预乘 alpha 的 BGRA，小端下与 WL_SHM_FORMAT_ARGB8888 同一字节序、同一预乘约定。
    std::memcpy(map, data, size);
    munmap(map, size);
    wl_shm_pool* pool = wl_shm_create_pool(shm_, fd, int32_t(size));
    close(fd); // 合成器已持有自己的引用
    wl_buffer* buf = wl_shm_pool_create_buffer(pool, 0, int32_t(w), int32_t(h),
                                               int32_t(stride), WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    wl_buffer_add_listener(buf, &kBufferListener, this);
    // 位图按物理像素光栅化（服务端用上报的缩放）；折回逻辑尺寸交给合成器，否则 150% 缩放下候选窗
    // 会按 1.5 倍大小再被放大一次。有 viewporter 时按逻辑尺寸设目标大小（分数缩放也准）；没有时
    // 只能处理整数缩放。
    const double sc = reportedScale_;
    if (viewporter_ && (sc != 1.0 || viewport_)) {
        if (!viewport_) {
            viewport_ = wp_viewporter_get_viewport(viewporter_, surface_);
        }
        if (sc == 1.0) {
            wp_viewport_set_destination(viewport_, -1, -1);
        } else {
            wp_viewport_set_destination(viewport_, std::max(1, int(std::lround(w / sc))),
                                        std::max(1, int(std::lround(h / sc))));
        }
    } else if (!viewporter_ && sc >= 2.0 && std::fabs(sc - std::round(sc)) < 0.01) {
        wl_surface_set_buffer_scale(surface_, int(std::round(sc)));
    } else if (!viewporter_ && sc != 1.0) {
        static bool warned = false;
        if (!warned) {
            warned = true;
            WIND_WARN() << "合成器没有 wp_viewporter，分数缩放 " << sc << " 下候选窗大小会不准";
        }
    }
    // 只有候选那一块接收鼠标：旁边的 tooltip、状态气泡、Toast 以及透明留白不拦指针
    // （否则指针滑进 tooltip 会触发离开 / 悬停抖动，Toast 还会挡住下面的应用）。
    {
        wl_region* region = wl_compositor_create_region(compositor_);
        if (inputW > 0 && inputH > 0) {
            wl_region_add(region, 0, 0, std::max(1, int(std::lround(inputW / sc))),
                          std::max(1, int(std::lround(inputH / sc))));
        }
        wl_surface_set_input_region(surface_, region);
        wl_region_destroy(region);
    }
    wl_surface_attach(surface_, buf, 0, 0);
    if (compositorVersion_ >= 4) {
        wl_surface_damage_buffer(surface_, 0, 0, int32_t(w), int32_t(h));
    } else {
        wl_surface_damage(surface_, 0, 0, int32_t(w), int32_t(h));
    }
    wl_surface_commit(surface_);
    wl_display_flush(display_);
    buffer_ = buf;
    return true;
}

void WaylandCandidatePanel::reportMenuPointer(uint32_t event, uint32_t button)
{
    const int32_t x = int32_t(std::lround(pointerX_ * reportedScale_));
    const int32_t y = int32_t(std::lround(pointerY_ * reportedScale_));
    if (event == MENU_POINTER_MOTION) {
        // 同一像素不重复报：每次报都是一趟往返
        if (lastMenuMotion_ && *lastMenuMotion_ == std::make_pair(x, y)) {
            return;
        }
        lastMenuMotion_ = std::make_pair(x, y);
    }
    if (cb_.menuPointer) {
        cb_.menuPointer(event, button, x, y);
    }
}

bool WaylandCandidatePanel::menuOpen() const
{
    for (const auto& m : menus_) {
        if (m.on) {
            return true;
        }
    }
    return false;
}

Rect WaylandCandidatePanel::virtualWorkArea() const
{
    // 菜单的落位在服务端按「工作区」做；Wayland 下没有屏幕坐标，约定坐标系的原点就是 popup
    // surface 的左上角，工作区取输出的大小（位图像素）。合成器如何摆 popup 在这里不可知，
    // 所以靠近屏幕下 / 右缘时菜单可能被合成器裁掉一部分——比让菜单整个出不来强。
    const Output* o = currentOutput_ ? currentOutput_ : (outputs_.empty() ? nullptr : outputs_[0].get());
    int32_t w = 3840, h = 2160;
    if (o) {
        const double s = outputScale(*o);
        const int32_t lw = o->logW > 0 ? o->logW : int32_t(o->modeW / std::max(1.0, s));
        const int32_t lh = o->logH > 0 ? o->logH : int32_t(o->modeH / std::max(1.0, s));
        if (lw > 0 && lh > 0) {
            w = int32_t(std::lround(lw * reportedScale_));
            h = int32_t(std::lround(lh * reportedScale_));
        }
    }
    return Rect{0, 0, w, h};
}

bool WaylandCandidatePanel::showMenuLevel(fcitx::InputContext* ic, uint32_t level,
                                          const SharedFrame& f, const OverlayFramePayload& p)
{
    if (level >= kMenuLevels) {
        return false;
    }
    OverlayFramePayload geo = p;
    geo.width = f.width;
    geo.height = f.height;
    menus_[level].frame = f;
    menus_[level].rect = placeOverlay(geo, virtualWorkArea(), 0, 0);
    menus_[level].on = true;
    return present(ic);
}

void WaylandCandidatePanel::hideMenuLevel(uint32_t level)
{
    if (level >= kMenuLevels || !menus_[level].on) {
        return;
    }
    menus_[level].on = false;
    lastMenuMotion_.reset();
    if (auto* ic = instance_->mostRecentInputContext()) {
        present(ic);
    } else {
        hidePopup();
    }
}

void WaylandCandidatePanel::closeMenu()
{
    if (!menuOpen()) {
        return;
    }
    for (auto& m : menus_) {
        m.on = false;
    }
    lastMenuMotion_.reset();
    if (auto* ic = instance_->mostRecentInputContext()) {
        present(ic);
    } else {
        hidePopup();
    }
}

int32_t WaylandCandidatePanel::hitAtPointer() const
{
    // 命中表是位图像素坐标，指针是 surface 逻辑坐标，差的就是服务端的光栅化缩放。
    return hitTest(rects_, int32_t(std::lround(pointerX_ * reportedScale_)),
                   int32_t(std::lround(pointerY_ * reportedScale_)));
}

void WaylandCandidatePanel::updateHover()
{
    const int32_t idx = hitAtPointer();
    // 悬停只报候选（>=0）：命中表里的 -1/-2 是翻页按钮，悬停协议里 -1 表示「无」。
    setHover(idx != kNoHit && idx >= 0 ? idx : -1);
}

void WaylandCandidatePanel::setHover(int32_t index)
{
    if (index == hover_) {
        return;
    }
    hover_ = index;
    if (cb_.hover) {
        cb_.hover(index);
    }
}

void WaylandCandidatePanel::hidePopup()
{
    if (!surface_ || !display_) {
        return;
    }
    // 销毁 popup 角色（合成器据此摘窗，下一次显示重建时按当时的文本光标矩形重新摆位——只摘 buffer
    // 不销毁角色，treeland 会让候选窗一直钉在第一次出现的地方），但 **wl_surface 本身留着**：
    // 它一旦销毁，鼠标恰好正进入它时迟到的 `wl_pointer.enter` 带的是已销毁对象（libwayland 交给
    // 回调的是 NULL），Fcitx5 classicui 的指针回调对它直接解引用 → 崩 fcitx5（实测鼠标一移上
    // 候选窗就崩）。surface 活着，那些事件的 user_data（见 ensurePopup 里的占位）才有效。
    // 先摘 buffer 提交（surface 取消映射），再销毁角色：反过来会让合成器有一瞬把无角色的
    // surface 当普通内容画（实测 Toast 收起时闪出整块纯黑）。
    wl_surface_attach(surface_, nullptr, 0, 0);
    wl_surface_commit(surface_);
    dropPopup();
    wl_display_flush(display_);
    buffer_ = nullptr;
}

} // namespace windlinux
