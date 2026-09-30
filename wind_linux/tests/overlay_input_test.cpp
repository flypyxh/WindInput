// 光栅浮层的鼠标交互纯逻辑：按键 → 动作、菜单 target、悬停门控、候选悬停的延后、离开提示后
// 的重定、拖动落位（与 placeOverlay 同一条夹回规则），以及两条新增的上行信封编码。

#include "Bridge.h"
#include "Codec.h"
#include "ExtProtocol.h"
#include "OverlayInput.h"
#include "ShmFrame.h"
#include "TestHarness.h"

#include <cstring>

using namespace windlinux;

namespace {

uint32_t u32At(const Bytes& b, size_t off)
{
    uint32_t v = 0;
    std::memcpy(&v, b.data() + off, 4);
    return v;
}

std::string extBody(const Bytes& frame, std::string& kind)
{
    uint32_t kl = u32At(frame, HEADER_SIZE);
    kind.assign(reinterpret_cast<const char*>(frame.data() + HEADER_SIZE + 4), kl);
    uint32_t bl = u32At(frame, HEADER_SIZE + 4 + kl);
    return std::string(reinterpret_cast<const char*>(frame.data() + HEADER_SIZE + 8 + kl), bl);
}

/// 一帧状态气泡：位图 120x40，内容盒 100x24、左上扩边 (8, 6)。
OverlayFramePayload statusGeo()
{
    OverlayFramePayload p;
    p.kind = OVERLAY_KIND_STATUS;
    p.width = 120;
    p.height = 40;
    p.contentX = 8;
    p.contentY = 6;
    p.contentW = 100;
    p.contentH = 24;
    return p;
}

} // namespace

int main()
{
    CASE("按下动作：气泡左键拖动 / 右键菜单；提示只认右键；Toast 任意键（不含滚轮）关闭");
    CHECK(overlayPressAction(OVERLAY_KIND_STATUS, 1) == OverlayPress::Drag);
    CHECK(overlayPressAction(OVERLAY_KIND_STATUS, 3) == OverlayPress::Menu);
    CHECK(overlayPressAction(OVERLAY_KIND_STATUS, 2) == OverlayPress::None);
    CHECK(overlayPressAction(OVERLAY_KIND_TOOLTIP, 3) == OverlayPress::Menu);
    CHECK(overlayPressAction(OVERLAY_KIND_TOOLTIP, 1) == OverlayPress::None);
    CHECK(overlayPressAction(OVERLAY_KIND_TOAST, 1) == OverlayPress::Close);
    CHECK(overlayPressAction(OVERLAY_KIND_TOAST, 3) == OverlayPress::Close);
    CHECK(overlayPressAction(OVERLAY_KIND_TOAST, 4) == OverlayPress::None);
    CHECK(overlayPressAction(OVERLAY_KIND_TOAST, 5) == OverlayPress::None);
    CHECK(overlayPressAction(OVERLAY_KIND_MENU, 1) == OverlayPress::None);

    CASE("菜单 target：气泡 / 提示各一档，与候选下标（≥0）和主菜单（-1）不撞");
    CHECK_EQ(overlayMenuTarget(OVERLAY_KIND_STATUS), MENU_TARGET_STATUS);
    CHECK_EQ(overlayMenuTarget(OVERLAY_KIND_TOOLTIP), MENU_TARGET_TOOLTIP);
    CHECK_EQ(overlayMenuTarget(OVERLAY_KIND_TOAST), MENU_TARGET_MAIN);
    CHECK(MENU_TARGET_STATUS < 0 && MENU_TARGET_TOOLTIP < 0);
    CHECK(MENU_TARGET_STATUS != MENU_TARGET_MAIN && MENU_TARGET_TOOLTIP != MENU_TARGET_MAIN
          && MENU_TARGET_STATUS != MENU_TARGET_TOOLTIP);

    CASE("悬停门控：窗口出现在静止指针下不算悬停，指针真动了才算");
    HoverGate g;
    g.rebase(300, 400);
    CHECK(!g.accept(300, 400)); // 映射引发的进入 / 移动：位置没变
    CHECK(g.accept(301, 400));
    CHECK(!g.accept(301, 400)); // 同一点重复事件
    CHECK(g.accept(305, 402));
    HoverGate fresh;
    CHECK(fresh.accept(0, 0)); // 没定过基线：任何位置都是真实移动

    CASE("候选悬停延后：只在所离开候选的提示正显示时延后（同 Windows hover_move）");
    CHECK_EQ(hoverDeferMs(-1, 2, true), 0u);             // 从「无」进候选：立即
    CHECK_EQ(hoverDeferMs(1, 2, false), 0u);             // 提示没显示：立即
    CHECK_EQ(hoverDeferMs(1, -1, true), kTipGraceMs);    // 去往「无」：宽限
    CHECK_EQ(hoverDeferMs(1, 2, true), kTipSwitchMs);    // 去往另一个候选：短一些
    CHECK_EQ(hoverDeferMs(1, 1, true), 0u);              // 没变
    CHECK(kTipSwitchMs < kTipGraceMs);
    CHECK_EQ(kTipGraceMs, 280u); // 与 tooltip.rs LEAVE_GRACE_MS / candidate_window TIP_GRACE 同值

    CASE("离开提示后重定悬停：回到提示 / 回到所属候选不动；候选窗别处报那里；外面报 -1");
    CHECK_EQ(tipRecheckHover(true, false, kNoHit, 2), kKeepHover);
    CHECK_EQ(tipRecheckHover(false, true, 2, 2), kKeepHover);
    CHECK_EQ(tipRecheckHover(false, true, 3, 2), 3);
    CHECK_EQ(tipRecheckHover(false, true, -1, 2), -1); // 翻页按钮算「无」
    CHECK_EQ(tipRecheckHover(false, true, kNoHit, 2), -1);
    CHECK_EQ(tipRecheckHover(false, false, kNoHit, 2), -1);
    CHECK_EQ(tipRecheckHover(false, false, kNoHit, -1), kKeepHover); // 本就是「无」

    CASE("拖动：窗口跟着指针走（抓取偏移不变）");
    const Rect wa{0, 0, 1280, 800};
    OverlayFramePayload geo = statusGeo();
    // 在窗口内 (30, 10) 处按下，指针拖到 (500, 300)：窗口左上 = (470, 290)。
    Rect r = dragOverlay(500, 300, 30, 10, geo, wa);
    CHECK_EQ(r.x, 470);
    CHECK_EQ(r.y, 290);
    CHECK_EQ(r.w, 120);
    CHECK_EQ(r.h, 40);

    CASE("拖动：拖出屏幕时内容盒夹回工作区（阴影扩边可出屏），与 placeOverlay 同一规则");
    r = dragOverlay(5, 3, 30, 10, geo, wa); // 窗口想到 (-25, -7)
    CHECK_EQ(r.x + geo.contentX, 0);        // 内容盒左上贴工作区
    CHECK_EQ(r.y + geo.contentY, 0);
    r = dragOverlay(1279, 799, 30, 10, geo, wa);
    CHECK_EQ(r.x + geo.contentX + int32_t(geo.contentW), 1280);
    CHECK_EQ(r.y + geo.contentY + int32_t(geo.contentH), 800);
    // 左侧副屏（工作区从负坐标起）：负坐标是合法落点。
    const Rect left{-1920, 0, 1920, 1080};
    r = dragOverlay(-1000, 500, 30, 10, geo, left);
    CHECK_EQ(r.x, -1030);
    CHECK_EQ(r.y, 490);
    // 夹回后的落点按 ABSOLUTE 重新摆放，位置不变（落盘 → 下次显示不跳）。
    OverlayFramePayload again = geo;
    again.place = OVERLAY_PLACE_ABSOLUTE;
    Rect d = dragOverlay(1279, 799, 30, 10, geo, wa);
    again.x = d.x + geo.contentX;
    again.y = d.y + geo.contentY;
    Rect back = placeOverlay(again, wa, 0, 0);
    CHECK_EQ(back.x, d.x);
    CHECK_EQ(back.y, d.y);

    CASE("rectContains：半开区间");
    CHECK(rectContains(Rect{10, 20, 5, 5}, 10, 20));
    CHECK(rectContains(Rect{10, 20, 5, 5}, 14, 24));
    CHECK(!rectContains(Rect{10, 20, 5, 5}, 15, 24));
    CHECK(!rectContains(Rect{10, 20, 5, 5}, 9, 20));

    CASE("menu.open 带提示位图内坐标：lx/ly 追加在 work 之后（对位 Rust decode_menu_open）");
    std::string kind;
    std::string body = extBody(
        encodeMenuOpenFrame(MENU_TARGET_TOOLTIP, 300, 420, 0, 0, 1280, 800, 17, -3), kind);
    CHECK_EQ(kind, std::string("menu.open"));
    CHECK_EQ(body,
             std::string(R"({"target":-3,"x":300,"y":420,"work":[0,0,1280,800],"lx":17,"ly":-3})"));

    CASE("pos.status_tip：{x,y}（对位 Rust decode_ext_point），负坐标原样");
    body = extBody(encodePosFrame(EXT_KIND_POS_STATUS_TIP, -800, 12), kind);
    CHECK_EQ(kind, std::string("pos.status_tip"));
    CHECK_EQ(body, std::string(R"({"x":-800,"y":12})"));

    TEST_MAIN_END();
}
