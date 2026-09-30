#include "X11Panel.h"

#include <fcitx-utils/log.h>

#include <cstdlib>
#include <cstring>
#include <string>
#include <xcb/shape.h>

FCITX_DECLARE_LOG_CATEGORY(windinput_log);
#define WIND_DEBUG() FCITX_LOGC(windinput_log, Debug)
#define WIND_WARN() FCITX_LOGC(windinput_log, Warn)

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
    : loop_(loop), cb_(std::move(cb))
{
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
    if (conn_) {
        destroyWindow();
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

bool X11CandidatePanel::ensureWindow(bool argb)
{
    if (window_ && argb_ == argb) {
        return true;
    }
    destroyWindow();
    xcb_visualid_t visual = screen_->root_visual;
    depth_ = screen_->root_depth;
    colormap_ = 0;
    if (argb) {
        xcb_visualtype_t* v = findArgbVisual(screen_);
        if (!v) {
            argb = false; // 有合成器却没有 32 位 visual：极少见，按无合成器处理
        } else {
            visual = v->visual_id;
            depth_ = 32;
            colormap_ = xcb_generate_id(conn_);
            xcb_create_colormap(conn_, XCB_COLORMAP_ALLOC_NONE, colormap_, screen_->root, visual);
        }
    }
    argb_ = argb;
    window_ = xcb_generate_id(conn_);
    // 值的顺序必须与掩码位从低到高一致：BACK_PIXEL, BORDER_PIXEL, OVERRIDE_REDIRECT,
    // EVENT_MASK, COLORMAP。ARGB visual 与根窗口深度不同，border/colormap 必须显式给，
    // 否则 CreateWindow 报 BadMatch。
    uint32_t mask = XCB_CW_BACK_PIXEL | XCB_CW_BORDER_PIXEL | XCB_CW_OVERRIDE_REDIRECT
        | XCB_CW_EVENT_MASK | XCB_CW_COLORMAP;
    uint32_t values[] = {
        0,
        0,
        1, // override-redirect：不归窗口管理器管，不抢焦点、不加边框
        XCB_EVENT_MASK_BUTTON_PRESS | XCB_EVENT_MASK_POINTER_MOTION
            | XCB_EVENT_MASK_LEAVE_WINDOW,
        colormap_ ? colormap_ : screen_->default_colormap,
    };
    xcb_create_window(conn_, depth_, window_, screen_->root, 0, 0, 1, 1, 0,
                      XCB_WINDOW_CLASS_INPUT_OUTPUT, visual, mask, values);
    // 给截图 / 调试工具认窗用（`xdotool search --class wind-candidate`）。
    static const char kClass[] = "wind-candidate\0WindInput";
    xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, window_, XCB_ATOM_WM_CLASS,
                        XCB_ATOM_STRING, 8, sizeof(kClass), kClass);
    static const char kName[] = "WindInput Candidates";
    xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, window_, XCB_ATOM_WM_NAME,
                        XCB_ATOM_STRING, 8, sizeof(kName) - 1, kName);
    // 窗口类型提示：合成器据此不给它加阴影/动画（阴影已画在位图里）。
    xcb_atom_t type = internAtom(conn_, "_NET_WM_WINDOW_TYPE");
    xcb_atom_t popup = internAtom(conn_, "_NET_WM_WINDOW_TYPE_POPUP_MENU");
    if (type && popup) {
        xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, window_, type, XCB_ATOM_ATOM, 32, 1,
                            &popup);
    }
    gc_ = xcb_generate_id(conn_);
    xcb_create_gc(conn_, gc_, window_, 0, nullptr);
    WIND_DEBUG() << "候选窗已建：" << (argb_ ? "ARGB（有合成器）" : "根 visual + XShape（无合成器）");
    return true;
}

void X11CandidatePanel::destroyWindow()
{
    if (!conn_) {
        return;
    }
    if (pixmap_) {
        xcb_free_pixmap(conn_, pixmap_);
        pixmap_ = 0;
    }
    if (gc_) {
        xcb_free_gc(conn_, gc_);
        gc_ = 0;
    }
    if (window_) {
        xcb_destroy_window(conn_, window_);
        window_ = 0;
    }
    if (colormap_) {
        xcb_free_colormap(conn_, colormap_);
        colormap_ = 0;
    }
    pixW_ = pixH_ = 0;
    mapped_ = false;
}

void X11CandidatePanel::upload(const SharedFrame& f)
{
    if (pixW_ != f.width || pixH_ != f.height) {
        if (pixmap_) {
            xcb_free_pixmap(conn_, pixmap_);
        }
        pixmap_ = xcb_generate_id(conn_);
        xcb_create_pixmap(conn_, depth_, pixmap_, window_, uint16_t(f.width), uint16_t(f.height));
        pixW_ = f.width;
        pixH_ = f.height;
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
        xcb_put_image(conn_, XCB_IMAGE_FORMAT_Z_PIXMAP, pixmap_, gc_, uint16_t(f.width),
                      uint16_t(rows), 0, int16_t(y), 0, depth_, rows * rowBytes,
                      src + size_t(y) * rowBytes);
    }
    // 位图挂成窗口背景：重绘（被遮挡后露出）由 X 服务器自己做，不用处理 Expose。
    xcb_change_window_attributes(conn_, window_, XCB_CW_BACK_PIXMAP, &pixmap_);
    xcb_clear_area(conn_, 0, window_, 0, 0, uint16_t(f.width), uint16_t(f.height));
}

void X11CandidatePanel::applyShape(const SharedFrame& f)
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
                         XCB_CLIP_ORDERING_YX_SORTED, window_, 0, 0, uint32_t(rects.size()),
                         rects.data());
}

bool X11CandidatePanel::show(const SharedFrame& f, int32_t x, int32_t y, bool absolute)
{
    if (f.width == 0 || f.height == 0 || f.bgra.empty() || !ensureConnection()) {
        return false;
    }
    bool argb = hasCompositor();
    if (!ensureWindow(argb)) {
        return false;
    }
    if (bitsPerPixel(conn_, depth_) != 32) {
        WIND_WARN() << "深度 " << int(depth_) << " 的像素格式不是 32 bpp，候选窗不显示";
        return false;
    }
    Rect wa{0, 0, screen_->width_in_pixels, screen_->height_in_pixels};
    Rect r = placePanel(x, y, int32_t(f.width), int32_t(f.height), wa, absolute);
    uint32_t geo[] = {uint32_t(r.x), uint32_t(r.y), f.width, f.height, XCB_STACK_MODE_ABOVE};
    xcb_configure_window(conn_, window_,
                         XCB_CONFIG_WINDOW_X | XCB_CONFIG_WINDOW_Y | XCB_CONFIG_WINDOW_WIDTH
                             | XCB_CONFIG_WINDOW_HEIGHT | XCB_CONFIG_WINDOW_STACK_MODE,
                         geo);
    upload(f);
    if (!argb_) {
        applyShape(f);
    }
    if (!mapped_) {
        xcb_map_window(conn_, window_);
        mapped_ = true;
    }
    xcb_flush(conn_);
    WIND_DEBUG() << "候选窗 " << f.width << "x" << f.height << " @ (" << r.x << "," << r.y << ")";
    return true;
}

void X11CandidatePanel::hide()
{
    if (conn_ && window_ && mapped_) {
        xcb_unmap_window(conn_, window_);
        xcb_flush(conn_);
    }
    mapped_ = false;
    rects_.clear();
    hover_ = -1;
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
    if (xcb_connection_has_error(conn_)) {
        WIND_WARN() << "X 连接断开，下一帧重连";
        // 不能在 IO 回调里销毁自己所属的 EventSourceIO，只先停掉它（断开的 fd 恒可读，
        // 不停会空转）。真正的清理由下一次 show 里 ensureConnection 见 has_error 时做。
        ioEvent_->setEnabled(false);
    }
}

void X11CandidatePanel::handleEvent(xcb_generic_event_t* ev)
{
    switch (ev->response_type & 0x7F) {
    case XCB_BUTTON_PRESS: {
        auto* e = reinterpret_cast<xcb_button_press_event_t*>(ev);
        if (e->event != window_) {
            break;
        }
        if (e->detail == 1) {
            int32_t idx = hitTest(rects_, e->event_x, e->event_y);
            if (idx != kNoHit && cb_.select) {
                cb_.select(idx);
            }
        } else if ((e->detail == 4 || e->detail == 5) && cb_.scroll) {
            cb_.scroll(e->detail == 4 ? 120 : -120);
        }
        break;
    }
    case XCB_MOTION_NOTIFY: {
        auto* e = reinterpret_cast<xcb_motion_notify_event_t*>(ev);
        if (e->event != window_) {
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
