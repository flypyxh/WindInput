#include "X11Panel.h"

#include "ExtProtocol.h"
#include "Menu.h"

#include <fcitx-utils/log.h>

#include <cstdlib>
#include <cstring>
#include <string>
#include <time.h>
#include <xcb/shape.h>

FCITX_DECLARE_LOG_CATEGORY(windinput_log);
#define WIND_DEBUG() FCITX_LOGC(windinput_log, Debug)
#define WIND_WARN() FCITX_LOGC(windinput_log, Warn)
#define WIND_INFO() FCITX_LOGC(windinput_log, Info)

namespace windlinux {

namespace {

xcb_atom_t internAtom(xcb_connection_t* c, const std::string& name)
{
    auto cookie = xcb_intern_atom(c, 0, uint16_t(name.size()), name.c_str());
    xcb_intern_atom_reply_t* r = xcb_intern_atom_reply(c, cookie, nullptr);
    xcb_atom_t a = r ? r->atom : xcb_atom_t(XCB_ATOM_NONE);
    std::free(r);
    return a;
}

/// 找一个 32 位 TrueColor visual（ARGB）；没有返回 nullptr。
xcb_visualtype_t* findArgbVisual(xcb_screen_t* s)
{
    for (auto d = xcb_screen_allowed_depths_iterator(s); d.rem; xcb_depth_next(&d)) {
        if (d.data->depth != 32) {
            continue;
        }
        for (auto v = xcb_depth_visuals_iterator(d.data); v.rem; xcb_visualtype_next(&v)) {
            if (v.data->_class == XCB_VISUAL_CLASS_TRUE_COLOR) {
                return v.data;
            }
        }
    }
    return nullptr;
}

/// 该深度的 ZPixmap 每像素位数（X 服务器声明的格式表）。
int bitsPerPixel(xcb_connection_t* c, uint8_t depth)
{
    const xcb_setup_t* setup = xcb_get_setup(c);
    for (auto f = xcb_setup_pixmap_formats_iterator(setup); f.rem; xcb_format_next(&f)) {
        if (f.data->depth == depth) {
            return f.data->bits_per_pixel;
        }
    }
    return 0;
}

} // namespace

X11CandidatePanel::X11CandidatePanel(fcitx::EventLoop& loop, Callbacks cb)
    : loop_(loop), cb_(std::move(cb)),
      menuIdleMs_(menuIdleTimeoutMs(std::getenv("WIND_MENU_IDLE_TIMEOUT_MS")))
{
    cand_.instance = "wind-candidate";
    cand_.interactive = true;
    overlays_[OVERLAY_KIND_TOOLTIP - 1].instance = "wind-tooltip";
    overlays_[OVERLAY_KIND_STATUS - 1].instance = "wind-status";
    overlays_[OVERLAY_KIND_TOAST - 1].instance = "wind-toast";
    static const char* const kMenuNames[kMenuLevels] = {"wind-menu-0", "wind-menu-1", "wind-menu-2",
                                                        "wind-menu-3", "wind-menu-4", "wind-menu-5"};
    for (size_t k = 0; k < kMenuLevels; ++k) {
        menus_[k].instance = kMenuNames[k];
        menus_[k].interactive = true; // 收鼠标（抓指针期间事件落在菜单窗口上）
    }
}

X11CandidatePanel::~X11CandidatePanel()
{
    dropConnection();
}

bool X11CandidatePanel::ensureConnection()
{
    if (conn_ && !xcb_connection_has_error(conn_)) {
        return true;
    }
    dropConnection();
    const char* display = std::getenv("DISPLAY");
    if (!display || !*display) {
        if (!warnedNoDisplay_) {
            WIND_WARN() << "没有 DISPLAY：候选窗不显示（Wayland 原生 popup 尚未实现）";
            warnedNoDisplay_ = true;
        }
        return false;
    }
    conn_ = xcb_connect(nullptr, &screenNum_);
    if (xcb_connection_has_error(conn_)) {
        WIND_WARN() << "连不上 X 服务器 " << display << "，候选窗不显示";
        xcb_disconnect(conn_);
        conn_ = nullptr;
        return false;
    }
    auto it = xcb_setup_roots_iterator(xcb_get_setup(conn_));
    for (int i = 0; i < screenNum_ && it.rem; ++i) {
        xcb_screen_next(&it);
    }
    screen_ = it.data;
    const xcb_query_extension_reply_t* shape = xcb_get_extension_data(conn_, &xcb_shape_id);
    shapeAvailable_ = shape && shape->present;
    ioEvent_ = loop_.addIOEvent(xcb_get_file_descriptor(conn_), fcitx::IOEventFlag::In,
                                [this](fcitx::EventSourceIO*, int, fcitx::IOEventFlags) {
                                    onReadable();
                                    return true;
                                });
    return true;
}

void X11CandidatePanel::dropConnection()
{
    ioEvent_.reset();
    grabRetry_.reset();
    menuIdle_.reset();
    pointerGrabbed_ = false; // 连接一断，服务器端的抓取随之失效
    pendingMotion_.reset();
    if (conn_) {
        destroyWindow(cand_);
        for (Surface& o : overlays_) {
            destroyWindow(o);
        }
        for (Surface& m : menus_) {
            destroyWindow(m);
        }
        xcb_disconnect(conn_);
        conn_ = nullptr;
    }
    screen_ = nullptr;
}

bool X11CandidatePanel::hasCompositor()
{
    xcb_atom_t sel = internAtom(conn_, "_NET_WM_CM_S" + std::to_string(screenNum_));
    if (sel == XCB_ATOM_NONE) {
        return false;
    }
    auto r = xcb_get_selection_owner_reply(conn_, xcb_get_selection_owner(conn_, sel), nullptr);
    bool owned = r && r->owner != XCB_NONE;
    std::free(r);
    return owned;
}

bool X11CandidatePanel::ensureWindow(Surface& s, bool argb)
{
    if (s.window && s.argb == argb) {
        return true;
    }
    destroyWindow(s);
    xcb_visualid_t visual = screen_->root_visual;
    s.depth = screen_->root_depth;
    s.colormap = 0;
    if (argb) {
        xcb_visualtype_t* v = findArgbVisual(screen_);
        if (!v) {
            argb = false; // 有合成器却没有 32 位 visual：极少见，按无合成器处理
        } else {
            visual = v->visual_id;
            s.depth = 32;
            s.colormap = xcb_generate_id(conn_);
            xcb_create_colormap(conn_, XCB_COLORMAP_ALLOC_NONE, s.colormap, screen_->root, visual);
        }
    }
    s.argb = argb;
    s.window = xcb_generate_id(conn_);
    // 值的顺序必须与掩码位从低到高一致：BACK_PIXEL, BORDER_PIXEL, OVERRIDE_REDIRECT,
    // EVENT_MASK, COLORMAP。ARGB visual 与根窗口深度不同，border/colormap 必须显式给，
    // 否则 CreateWindow 报 BadMatch。
    uint32_t mask = XCB_CW_BACK_PIXEL | XCB_CW_BORDER_PIXEL | XCB_CW_OVERRIDE_REDIRECT
        | XCB_CW_EVENT_MASK | XCB_CW_COLORMAP;
    uint32_t values[] = {
        0,
        0,
        1, // override-redirect：不归窗口管理器管，不抢焦点、不加边框
        s.interactive ? uint32_t(XCB_EVENT_MASK_BUTTON_PRESS | XCB_EVENT_MASK_POINTER_MOTION
                                 | XCB_EVENT_MASK_LEAVE_WINDOW)
                      : 0u,
        s.colormap ? s.colormap : screen_->default_colormap,
    };
    xcb_create_window(conn_, s.depth, s.window, screen_->root, 0, 0, 1, 1, 0,
                      XCB_WINDOW_CLASS_INPUT_OUTPUT, visual, mask, values);
    // 给截图 / 调试工具认窗用（`xdotool search --classname wind-candidate`）。
    std::string cls = std::string(s.instance) + '\0' + "WindInput" + '\0';
    xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, s.window, XCB_ATOM_WM_CLASS,
                        XCB_ATOM_STRING, 8, uint32_t(cls.size()), cls.data());
    std::string name = std::string("WindInput ") + s.instance;
    xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, s.window, XCB_ATOM_WM_NAME,
                        XCB_ATOM_STRING, 8, uint32_t(name.size()), name.data());
    // 窗口类型提示：合成器据此不给它加阴影/动画（阴影已画在位图里）。
    xcb_atom_t type = internAtom(conn_, "_NET_WM_WINDOW_TYPE");
    xcb_atom_t kind = internAtom(conn_, s.interactive ? "_NET_WM_WINDOW_TYPE_POPUP_MENU"
                                                      : "_NET_WM_WINDOW_TYPE_TOOLTIP");
    if (type && kind) {
        xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, s.window, type, XCB_ATOM_ATOM, 32, 1,
                            &kind);
    }
    // 浮层对鼠标透明：输入区设为空，点击穿透到下面的应用（气泡常弹在光标旁 / 屏幕角）。
    if (!s.interactive && shapeAvailable_) {
        xcb_shape_rectangles(conn_, XCB_SHAPE_SO_SET, XCB_SHAPE_SK_INPUT,
                             XCB_CLIP_ORDERING_UNSORTED, s.window, 0, 0, 0, nullptr);
    }
    s.gc = xcb_generate_id(conn_);
    xcb_create_gc(conn_, s.gc, s.window, 0, nullptr);
    WIND_DEBUG() << s.instance << " 已建："
                 << (s.argb ? "ARGB（有合成器）" : "根 visual + XShape（无合成器）");
    return true;
}

void X11CandidatePanel::destroyWindow(Surface& s)
{
    s.hideTimer.reset();
    if (!conn_) {
        return;
    }
    if (s.pixmap) {
        xcb_free_pixmap(conn_, s.pixmap);
        s.pixmap = 0;
    }
    if (s.gc) {
        xcb_free_gc(conn_, s.gc);
        s.gc = 0;
    }
    if (s.window) {
        xcb_destroy_window(conn_, s.window);
        s.window = 0;
    }
    if (s.colormap) {
        xcb_free_colormap(conn_, s.colormap);
        s.colormap = 0;
    }
    s.pixW = s.pixH = 0;
    s.mapped = false;
}

void X11CandidatePanel::upload(Surface& s, const SharedFrame& f)
{
    if (s.pixW != f.width || s.pixH != f.height) {
        if (s.pixmap) {
            xcb_free_pixmap(conn_, s.pixmap);
        }
        s.pixmap = xcb_generate_id(conn_);
        xcb_create_pixmap(conn_, s.depth, s.pixmap, s.window, uint16_t(f.width),
                          uint16_t(f.height));
        s.pixW = f.width;
        s.pixH = f.height;
    }
    // 服务端给的是 BGRA 预乘 alpha，小端下恰好就是 32 位 ARGB visual 的像素格式（合成器
    // 也要预乘）。深度 24 的 ZPixmap 同样是每像素 4 字节 BGRx，alpha 字节被忽略。
    const uint32_t rowBytes = f.width * 4;
    const uint8_t* src = f.bgra.data();
    std::vector<uint8_t> packed;
    if (f.stride != rowBytes) { // 去掉行尾填充，PutImage 要紧凑行
        packed.resize(size_t(rowBytes) * f.height);
        for (uint32_t y = 0; y < f.height; ++y) {
            std::memcpy(&packed[size_t(y) * rowBytes], src + size_t(y) * f.stride, rowBytes);
        }
        src = packed.data();
    }
    // 单个请求有长度上限（无 BIG-REQUESTS 时 256KB），按行切块。
    uint32_t maxBytes = xcb_get_maximum_request_length(conn_) * 4 - 64;
    uint32_t rowsPerChunk = std::max<uint32_t>(1, maxBytes / rowBytes);
    for (uint32_t y = 0; y < f.height; y += rowsPerChunk) {
        uint32_t rows = std::min(rowsPerChunk, f.height - y);
        xcb_put_image(conn_, XCB_IMAGE_FORMAT_Z_PIXMAP, s.pixmap, s.gc, uint16_t(f.width),
                      uint16_t(rows), 0, int16_t(y), 0, s.depth, rows * rowBytes,
                      src + size_t(y) * rowBytes);
    }
    // 位图挂成窗口背景：重绘（被遮挡后露出）由 X 服务器自己做，不用处理 Expose。
    xcb_change_window_attributes(conn_, s.window, XCB_CW_BACK_PIXMAP, &s.pixmap);
    xcb_clear_area(conn_, 0, s.window, 0, 0, uint16_t(f.width), uint16_t(f.height));
}

void X11CandidatePanel::applyShape(Surface& s, const SharedFrame& f)
{
    if (!shapeAvailable_) {
        return;
    }
    // 按行扫 alpha，把 alpha >= 128 的连续段合成矩形：圆角外、阴影淡处被抠掉。
    std::vector<xcb_rectangle_t> rects;
    for (uint32_t y = 0; y < f.height; ++y) {
        const uint8_t* row = f.bgra.data() + size_t(y) * f.stride;
        uint32_t x = 0;
        while (x < f.width) {
            while (x < f.width && row[x * 4 + 3] < 128) {
                ++x;
            }
            uint32_t start = x;
            while (x < f.width && row[x * 4 + 3] >= 128) {
                ++x;
            }
            if (x > start) {
                rects.push_back({int16_t(start), int16_t(y), uint16_t(x - start), 1});
            }
        }
    }
    xcb_shape_rectangles(conn_, XCB_SHAPE_SO_SET, XCB_SHAPE_SK_BOUNDING,
                         XCB_CLIP_ORDERING_YX_SORTED, s.window, 0, 0, uint32_t(rects.size()),
                         rects.data());
}

Rect X11CandidatePanel::workArea() const
{
    return Rect{0, 0, screen_->width_in_pixels, screen_->height_in_pixels};
}

bool X11CandidatePanel::present(Surface& s, const SharedFrame& f, const Rect& r, bool raise)
{
    uint32_t geo[] = {uint32_t(r.x), uint32_t(r.y), f.width, f.height, XCB_STACK_MODE_ABOVE};
    uint16_t mask = XCB_CONFIG_WINDOW_X | XCB_CONFIG_WINDOW_Y | XCB_CONFIG_WINDOW_WIDTH
        | XCB_CONFIG_WINDOW_HEIGHT;
    if (raise) {
        mask |= XCB_CONFIG_WINDOW_STACK_MODE;
    }
    xcb_configure_window(conn_, s.window, mask, geo);
    upload(s, f);
    if (!s.argb) {
        applyShape(s, f);
    }
    if (!s.mapped) {
        xcb_map_window(conn_, s.window);
        s.mapped = true;
    }
    xcb_flush(conn_);
    WIND_DEBUG() << s.instance << " " << f.width << "x" << f.height << " @ (" << r.x << ","
                 << r.y << ")";
    return true;
}

void X11CandidatePanel::unmap(Surface& s)
{
    s.hideTimer.reset();
    if (conn_ && s.window && s.mapped) {
        xcb_unmap_window(conn_, s.window);
        xcb_flush(conn_);
    }
    s.mapped = false;
}

bool X11CandidatePanel::show(const SharedFrame& f, int32_t x, int32_t y, bool absolute)
{
    if (f.width == 0 || f.height == 0 || f.bgra.empty() || !ensureConnection()) {
        return false;
    }
    if (!ensureWindow(cand_, hasCompositor())) {
        return false;
    }
    if (bitsPerPixel(conn_, cand_.depth) != 32) {
        WIND_WARN() << "深度 " << int(cand_.depth) << " 的像素格式不是 32 bpp，候选窗不显示";
        return false;
    }
    Rect r = placePanel(x, y, int32_t(f.width), int32_t(f.height), workArea(), absolute);
    candShiftX_ = r.x - x;
    candShiftY_ = r.y - y;
    return present(cand_, f, r);
}

void X11CandidatePanel::hide()
{
    unmap(cand_);
    rects_.clear();
    hover_ = -1;
    // tooltip 挂在候选窗上：候选窗藏了它必须一起藏（服务端也会推隐藏帧，这里兜底）。
    hideOverlay(OVERLAY_KIND_TOOLTIP);
}

X11CandidatePanel::Surface* X11CandidatePanel::overlay(uint32_t kind)
{
    if (kind < 1 || kind > kOverlayCount) {
        return nullptr;
    }
    return &overlays_[kind - 1];
}

bool X11CandidatePanel::showOverlay(uint32_t kind, const SharedFrame& f,
                                    const OverlayFramePayload& p)
{
    Surface* s = overlay(kind);
    if (!s || f.width == 0 || f.height == 0 || f.bgra.empty() || !ensureConnection()) {
        return false;
    }
    if (!ensureWindow(*s, hasCompositor())) {
        return false;
    }
    if (bitsPerPixel(conn_, s->depth) != 32) {
        return false;
    }
    // 尺寸取像素那一份（通知与 SHM 间无锁，读到的可能已是更新的一帧），内容盒偏移随之不变。
    OverlayFramePayload geo = p;
    geo.width = f.width;
    geo.height = f.height;
    Rect r = placeOverlay(geo, workArea(), candShiftX_, candShiftY_);
    present(*s, f, r);
    s->hideTimer.reset();
    if (p.durationMs > 0) {
        s->hideTimer = loop_.addTimeEvent(
            CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + uint64_t(p.durationMs) * 1000, 0,
            [this, kind](fcitx::EventSourceTime*, uint64_t) {
                // 回调里不能销毁正在执行的自己：只摘窗口，计时器对象留到下次 show/hide 再换。
                if (Surface* o = overlay(kind); o && conn_ && o->window && o->mapped) {
                    xcb_unmap_window(conn_, o->window);
                    xcb_flush(conn_);
                    o->mapped = false;
                }
                return true;
            });
    }
    return true;
}

void X11CandidatePanel::hideOverlay(uint32_t kind)
{
    if (Surface* s = overlay(kind)) {
        unmap(*s);
    }
}

void X11CandidatePanel::hideAllOverlays()
{
    for (uint32_t k = 1; k <= kOverlayCount; ++k) {
        hideOverlay(k);
    }
}

void X11CandidatePanel::onReadable()
{
    if (!conn_) {
        return;
    }
    while (xcb_generic_event_t* ev = xcb_poll_for_event(conn_)) {
        handleEvent(ev);
        std::free(ev);
    }
    flushMenuMotion();
    if (xcb_connection_has_error(conn_)) {
        WIND_WARN() << "X 连接断开，下一帧重连";
        // 不能在 IO 回调里销毁自己所属的 EventSourceIO，只先停掉它（断开的 fd 恒可读，
        // 不停会空转）。真正的清理由下一次 show 里 ensureConnection 见 has_error 时做。
        ioEvent_->setEnabled(false);
    }
}

void X11CandidatePanel::handleEvent(xcb_generic_event_t* ev)
{
    if (menuOpen() && (ev->response_type & 0x7F) != 0) {
        handleMenuEvent(ev);
        return;
    }
    switch (ev->response_type & 0x7F) {
    case XCB_BUTTON_PRESS: {
        auto* e = reinterpret_cast<xcb_button_press_event_t*>(ev);
        if (e->event != cand_.window) {
            break;
        }
        if (e->detail == 1) {
            int32_t idx = hitTest(rects_, e->event_x, e->event_y);
            if (idx != kNoHit && cb_.select) {
                cb_.select(idx);
            }
        } else if (e->detail == 3 && cb_.contextMenu) {
            // 右键：命中候选 → 候选菜单，空白 / 翻页按钮 → 主菜单（同 Windows right_click）。
            // 悬停先清掉：菜单开着时候选窗收不到移动事件，残留的高亮会一直挂着。
            setHover(-1);
            cb_.contextMenu(contextMenuTarget(hitTest(rects_, e->event_x, e->event_y)),
                            e->root_x, e->root_y);
        } else if ((e->detail == 4 || e->detail == 5) && cb_.scroll) {
            cb_.scroll(e->detail == 4 ? 120 : -120);
        }
        break;
    }
    case XCB_MOTION_NOTIFY: {
        auto* e = reinterpret_cast<xcb_motion_notify_event_t*>(ev);
        if (e->event != cand_.window) {
            break;
        }
        int32_t idx = hitTest(rects_, e->event_x, e->event_y);
        // 悬停只报候选（>=0）：命中表里的 -1/-2 是翻页按钮，而悬停协议里 -1 表示「无」。
        setHover(idx >= 0 ? idx : -1);
        break;
    }
    case XCB_LEAVE_NOTIFY:
        setHover(-1);
        break;
    case 0: {
        auto* e = reinterpret_cast<xcb_generic_error_t*>(ev);
        WIND_DEBUG() << "X 错误 code=" << int(e->error_code) << " major=" << int(e->major_code);
        break;
    }
    default:
        break;
    }
}

void X11CandidatePanel::setHover(int32_t index)
{
    if (index == hover_) {
        return;
    }
    hover_ = index;
    if (cb_.hover) {
        cb_.hover(index);
    }
}

} // namespace windlinux

// ── 自绘菜单 ──────────────────────────────────────────────────────────

namespace windlinux {

bool X11CandidatePanel::menuOpen() const
{
    for (const Surface& m : menus_) {
        if (m.mapped) {
            return true;
        }
    }
    return false;
}

bool X11CandidatePanel::showMenuLevel(uint32_t level, const SharedFrame& f,
                                      const OverlayFramePayload& p)
{
    if (level >= kMenuLevels || f.width == 0 || f.height == 0 || f.bgra.empty()
        || !ensureConnection()) {
        return false;
    }
    Surface& s = menus_[level];
    if (!ensureWindow(s, hasCompositor()) || bitsPerPixel(conn_, s.depth) != 32) {
        return false;
    }
    OverlayFramePayload geo = p;
    geo.width = f.width;
    geo.height = f.height;
    const bool first = !menuOpen();
    // 新映射的一级提到最上（更深的子菜单总是后出现，自然压在父级上面）；已在显示的只换像素 /
    // 位置，不重排 z 序。
    present(s, f, placeOverlay(geo, workArea(), 0, 0), !s.mapped);
    if (first) {
        grabAttempts_ = 0;
        grabPointer();
    }
    armMenuIdle();
    return true;
}

void X11CandidatePanel::hideMenuLevel(uint32_t level)
{
    if (level >= kMenuLevels) {
        return;
    }
    unmap(menus_[level]);
    if (!menuOpen()) {
        dropMenu();
    }
}

void X11CandidatePanel::closeMenu(const char* reason)
{
    if (!menuOpen()) {
        return;
    }
    WIND_DEBUG() << "本端收起菜单：" << (reason ? reason : "（不报服务端）");
    dropMenu();
    if (reason && cb_.menuDismissed) {
        cb_.menuDismissed(reason);
    }
}

void X11CandidatePanel::dropMenu(bool fromIdleTimer)
{
    for (Surface& m : menus_) {
        unmap(m);
    }
    releasePointer();
    grabRetry_.reset();
    pendingMotion_.reset();
    if (!fromIdleTimer) {
        menuIdle_.reset();
    }
}

void X11CandidatePanel::noteMenuActivity()
{
    if (menuOpen()) {
        armMenuIdle();
    }
}

void X11CandidatePanel::armMenuIdle()
{
    const uint64_t due = fcitx::now(CLOCK_MONOTONIC) + uint64_t(menuIdleMs_) * 1000;
    if (menuIdle_) {
        menuIdle_->setTime(due);
        menuIdle_->setOneShot();
        return;
    }
    menuIdle_ = loop_.addTimeEvent(CLOCK_MONOTONIC, due, 0,
                                   [this](fcitx::EventSourceTime*, uint64_t) {
                                       if (menuOpen()) {
                                           WIND_INFO() << "菜单空闲 " << menuIdleMs_
                                                       << "ms 无操作，本端收起";
                                           dropMenu(true);
                                           if (cb_.menuDismissed) {
                                               cb_.menuDismissed("idle");
                                           }
                                       }
                                       return true;
                                   });
}

void X11CandidatePanel::grabPointer()
{
    if (pointerGrabbed_ || !conn_ || !menus_[0].window || !menus_[0].mapped) {
        return;
    }
    // owner_events = 1：指针在我们自己的窗口（各级菜单、候选窗）上时事件照常报给那个窗口，
    // 在别处时报给抓取窗口——于是「点菜单外」看得见，且那一下不会漏给下面的应用（同原生菜单）。
    // 只抓指针不抓键盘：键盘必须照旧走宿主 → Fcitx5 → 服务端 forward_menu_key。
    const uint16_t mask = XCB_EVENT_MASK_BUTTON_PRESS | XCB_EVENT_MASK_BUTTON_RELEASE
        | XCB_EVENT_MASK_POINTER_MOTION;
    auto cookie = xcb_grab_pointer(conn_, 1, menus_[0].window, mask, XCB_GRAB_MODE_ASYNC,
                                   XCB_GRAB_MODE_ASYNC, XCB_NONE, XCB_NONE, XCB_CURRENT_TIME);
    xcb_grab_pointer_reply_t* r = xcb_grab_pointer_reply(conn_, cookie, nullptr);
    const uint8_t status = r ? r->status : uint8_t(0xFF);
    std::free(r);
    if (status == XCB_GRAB_STATUS_SUCCESS) {
        // 不在这里销毁 grabRetry_：本函数可能正是它的回调（一次性计时器，触发后自己就停了）。
        pointerGrabbed_ = true;
        return;
    }
    // 别的客户端还抓着（典型：刚点的是托盘菜单，它的抓取要等菜单收起才放）：每 50ms 再试，
    // 最多 1 秒。抓不住菜单照样能用——Esc / 失焦 / 空闲超时照常收，只是点菜单外看不见。
    if (++grabAttempts_ > 20) {
        WIND_WARN() << "抓不住指针（status=" << int(status) << "），菜单外的点击将无法收起菜单";
        return;
    }
    const uint64_t due = fcitx::now(CLOCK_MONOTONIC) + 50 * 1000;
    if (grabRetry_) {
        grabRetry_->setTime(due);
        grabRetry_->setOneShot();
        return;
    }
    grabRetry_ = loop_.addTimeEvent(CLOCK_MONOTONIC, due, 0,
                                    [this](fcitx::EventSourceTime*, uint64_t) {
                                        grabPointer();
                                        return true;
                                    });
}

void X11CandidatePanel::releasePointer()
{
    if (pointerGrabbed_ && conn_) {
        xcb_ungrab_pointer(conn_, XCB_CURRENT_TIME);
        xcb_flush(conn_);
    }
    pointerGrabbed_ = false;
}

void X11CandidatePanel::handleMenuEvent(xcb_generic_event_t* ev)
{
    switch (ev->response_type & 0x7F) {
    case XCB_MOTION_NOTIFY: {
        auto* e = reinterpret_cast<xcb_motion_notify_event_t*>(ev);
        pendingMotion_ = std::make_pair(int32_t(e->root_x), int32_t(e->root_y));
        break;
    }
    case XCB_BUTTON_PRESS: {
        auto* e = reinterpret_cast<xcb_button_press_event_t*>(ev);
        if (e->detail < 1 || e->detail > 3) {
            break; // 滚轮（4/5）与侧键：Windows 菜单不处理，这里也不报
        }
        flushMenuMotion();
        armMenuIdle();
        if (cb_.menuPointer) {
            cb_.menuPointer(MENU_POINTER_PRESS, e->detail, e->root_x, e->root_y);
        }
        break;
    }
    default:
        break; // 松开 / 离开：菜单只看按下与移动（同 Windows 的 wnd_proc）
    }
}

void X11CandidatePanel::flushMenuMotion()
{
    if (!pendingMotion_) {
        return;
    }
    auto [x, y] = *pendingMotion_;
    pendingMotion_.reset();
    if (!menuOpen()) {
        return;
    }
    armMenuIdle();
    if (cb_.menuPointer) {
        cb_.menuPointer(MENU_POINTER_MOTION, 0, x, y);
    }
}

std::optional<Rect> X11CandidatePanel::screenWorkArea()
{
    if (!ensureConnection()) {
        return std::nullopt;
    }
    return workArea();
}

std::optional<std::pair<int32_t, int32_t>> X11CandidatePanel::pointerPosition()
{
    if (!ensureConnection()) {
        return std::nullopt;
    }
    auto* r = xcb_query_pointer_reply(conn_, xcb_query_pointer(conn_, screen_->root), nullptr);
    if (!r) {
        return std::nullopt;
    }
    std::pair<int32_t, int32_t> p{r->root_x, r->root_y};
    std::free(r);
    return p;
}

} // namespace windlinux
