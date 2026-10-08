// 自绘菜单的纯逻辑：kind ↔ 级、候选窗右键的目标、空闲超时配置，
// 以及菜单相关的上行编码（字节偏移按 Rust server.rs 的解码写死）与落位（EXACT 不夹回）。

#include "Bridge.h"
#include "Codec.h"
#include "ExtProtocol.h"
#include "Menu.h"
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
    // 头 8 字节，之后 kindLen u32 + kind + bodyLen u32 + body。
    uint32_t kl = u32At(frame, HEADER_SIZE);
    kind.assign(reinterpret_cast<const char*>(frame.data() + HEADER_SIZE + 4), kl);
    uint32_t bl = u32At(frame, HEADER_SIZE + 4 + kl);
    return std::string(reinterpret_cast<const char*>(frame.data() + HEADER_SIZE + 8 + kl), bl);
}

} // namespace

int main()
{
    CASE("kind → 菜单级：只认 MENU..MENU+LEVELS-1");
    CHECK_EQ(menuLevelOfKind(OVERLAY_KIND_MENU), 0);
    CHECK_EQ(menuLevelOfKind(OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS - 1),
             int(OVERLAY_MENU_LEVELS) - 1);
    CHECK_EQ(menuLevelOfKind(OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS), -1);
    CHECK_EQ(menuLevelOfKind(OVERLAY_KIND_TOAST), -1);

    CASE("候选窗右键：命中候选 → 候选菜单；翻页按钮 / 空白 → 主菜单（同 Windows right_click）");
    CHECK_EQ(contextMenuTarget(0), 0);
    CHECK_EQ(contextMenuTarget(3), 3);
    CHECK_EQ(contextMenuTarget(-1), -1);
    CHECK_EQ(contextMenuTarget(-2), -1);
    CHECK_EQ(contextMenuTarget(kNoHit), -1);

    CASE("空闲超时：缺省 60 s，环境变量只认正整数");
    CHECK_EQ(menuIdleTimeoutMs(nullptr), kDefaultMenuIdleTimeoutMs);
    CHECK_EQ(menuIdleTimeoutMs(""), kDefaultMenuIdleTimeoutMs);
    CHECK_EQ(menuIdleTimeoutMs("2500"), 2500u);
    CHECK_EQ(menuIdleTimeoutMs("0"), kDefaultMenuIdleTimeoutMs);
    CHECK_EQ(menuIdleTimeoutMs("-5"), kDefaultMenuIdleTimeoutMs);
    CHECK_EQ(menuIdleTimeoutMs("12ab"), kDefaultMenuIdleTimeoutMs);

    CASE("抓指针的绝对上限：默认 120 秒（长于空闲超时），环境变量正整数覆盖");
    CHECK_EQ(menuMaxGrabMs(nullptr), 120000u);
    CHECK(kDefaultMenuMaxGrabMs > kDefaultMenuIdleTimeoutMs);
    CHECK_EQ(menuMaxGrabMs("8000"), 8000u);
    CHECK_EQ(menuMaxGrabMs("0"), kDefaultMenuMaxGrabMs);
    CHECK_EQ(menuMaxGrabMs("x"), kDefaultMenuMaxGrabMs);

    CASE("CMD_MENU_POINTER：16 字节，event button x y（x/y 有符号）");
    Bytes f = encodeMenuPointerFrame(MENU_POINTER_PRESS, 3, -20, 700);
    CHECK_EQ(f.size(), HEADER_SIZE + 16);
    uint16_t cmd = uint16_t(f[2] | (f[3] << 8));
    CHECK_EQ(cmd, CMD_MENU_POINTER);
    CHECK_EQ(u32At(f, 4), 16u);
    CHECK_EQ(u32At(f, HEADER_SIZE + 0), MENU_POINTER_PRESS);
    CHECK_EQ(u32At(f, HEADER_SIZE + 4), 3u);
    CHECK_EQ(int32_t(u32At(f, HEADER_SIZE + 8)), -20);
    CHECK_EQ(int32_t(u32At(f, HEADER_SIZE + 12)), 700);

    CASE("menu.open / menu.dismiss 信封：kind 与 body 格式对位 Rust decode_menu_open");
    std::string kind;
    std::string body = extBody(encodeMenuOpenFrame(-1, 300, 420, 0, 0, 1280, 800), kind);
    CHECK_EQ(kind, std::string("menu.open"));
    CHECK_EQ(body, std::string(R"({"target":-1,"x":300,"y":420,"work":[0,0,1280,800]})"));
    body = extBody(encodeMenuDismissFrame("idle"), kind);
    CHECK_EQ(kind, std::string("menu.dismiss"));
    CHECK_EQ(body, std::string(R"({"reason":"idle"})"));
    body = extBody(encodeHostDisplayFrame(true, true, 1.5), kind);
    CHECK_EQ(kind, std::string("host.display"));
    CHECK_EQ(body, std::string(R"({"caret_free":true,"caret_trusted":true,"scale":1.50})"));
    body = extBody(encodeHostDisplayFrame(false, true, 1.0), kind);
    CHECK_EQ(body, std::string(R"({"caret_free":false,"caret_trusted":true,"scale":1.00})"));
    body = extBody(encodeHostDisplayFrame(false, false, 1.25), kind);
    CHECK_EQ(body, std::string(R"({"caret_free":false,"caret_trusted":false,"scale":1.25})"));
    body = extBody(encodeHostDisplayFrame(false, true, 1.05), kind);
    CHECK_EQ(body, std::string(R"({"caret_free":false,"caret_trusted":true,"scale":1.05})"));
    body = extBody(encodeHostDisplayFrame(true, true, 0.0), kind); // 不知道缩放：不带该字段
    CHECK_EQ(body, std::string(R"({"caret_free":true,"caret_trusted":true})"));

    CASE("菜单各级的 SHM 名：_MN<级>，与 Rust overlay_shm_name 对齐");
    CHECK_EQ(overlayShmName(OVERLAY_KIND_MENU), shmName() + "_MN0");
    CHECK_EQ(overlayShmName(OVERLAY_KIND_MENU + 5), shmName() + "_MN5");
    CHECK_EQ(overlayShmName(OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS), std::string());

    CASE("EXACT 落位：原样摆放、越界也不夹回（命中测试在服务端按这个位置做）");
    OverlayFramePayload p;
    p.kind = OVERLAY_KIND_MENU;
    p.place = OVERLAY_PLACE_EXACT;
    p.width = 200;
    p.height = 300;
    p.x = 1200;
    p.y = 700;
    p.contentW = 200;
    p.contentH = 300;
    Rect r = placeOverlay(p, Rect{0, 0, 1280, 800}, 0, 0);
    CHECK_EQ(r.x, 1200);
    CHECK_EQ(r.y, 700);
    CHECK_EQ(r.w, 200);
    CHECK_EQ(r.h, 300);

    TEST_MAIN_END();
}
