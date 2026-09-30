#include "OverlayInput.h"

#include "ExtProtocol.h"

namespace windlinux {

OverlayPress overlayPressAction(uint32_t kind, uint8_t button)
{
    switch (kind) {
    case OVERLAY_KIND_STATUS:
        return button == 1 ? OverlayPress::Drag : button == 3 ? OverlayPress::Menu : OverlayPress::None;
    case OVERLAY_KIND_TOOLTIP:
        return button == 3 ? OverlayPress::Menu : OverlayPress::None;
    case OVERLAY_KIND_TOAST:
        // 滚轮（4/5）不算点击：指针停在 Toast 上滚动页面时不该把它关掉。
        return button >= 1 && button <= 3 ? OverlayPress::Close : OverlayPress::None;
    default:
        return OverlayPress::None;
    }
}

int32_t overlayMenuTarget(uint32_t kind)
{
    switch (kind) {
    case OVERLAY_KIND_STATUS:
        return MENU_TARGET_STATUS;
    case OVERLAY_KIND_TOOLTIP:
        return MENU_TARGET_TOOLTIP;
    default:
        return MENU_TARGET_MAIN;
    }
}

uint32_t hoverDeferMs(int32_t last, int32_t raw, bool tipShown)
{
    if (last < 0 || !tipShown || raw == last) {
        return 0;
    }
    return raw >= 0 ? kTipSwitchMs : kTipGraceMs;
}

int32_t tipRecheckHover(bool onTip, bool onCandidate, int32_t candidateHit, int32_t held)
{
    if (onTip) {
        return kKeepHover;
    }
    const int32_t target = onCandidate && candidateHit >= 0 ? candidateHit : -1;
    return target == held ? kKeepHover : target;
}

Rect dragOverlay(int32_t px, int32_t py, int32_t grabDx, int32_t grabDy,
                 const OverlayFramePayload& geo, const Rect& workArea)
{
    OverlayFramePayload p = geo;
    p.place = OVERLAY_PLACE_ABSOLUTE;
    // 窗口左上 = 指针 − 抓取偏移；ABSOLUTE 的坐标指内容盒，加回扩边。
    p.x = px - grabDx + p.contentX;
    p.y = py - grabDy + p.contentY;
    return placeOverlay(p, workArea, 0, 0);
}

bool rectContains(const Rect& r, int32_t x, int32_t y)
{
    return x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
}

} // namespace windlinux
