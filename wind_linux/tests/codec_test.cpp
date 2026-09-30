// 帧编解码单测。字节偏移按 Rust `wind-ipc` 的解码器写死——这里断言的是「服务端按它的
// 布局能不能解出来」，不是「编码再解码回来一样」（后者两边同错也会绿）。

#include "Codec.h"
#include "ExtProtocol.h"
#include "Protocol.h"
#include "TestHarness.h"

using namespace windlinux;

namespace {

uint32_t u32At(const Bytes& b, size_t off)
{
    return uint32_t(b[off]) | (uint32_t(b[off + 1]) << 8) | (uint32_t(b[off + 2]) << 16)
        | (uint32_t(b[off + 3]) << 24);
}

uint16_t u16At(const Bytes& b, size_t off)
{
    return uint16_t(b[off] | (b[off + 1] << 8));
}

Bytes le32(uint32_t v)
{
    return {uint8_t(v), uint8_t(v >> 8), uint8_t(v >> 16), uint8_t(v >> 24)};
}

void append(Bytes& b, const Bytes& more)
{
    b.insert(b.end(), more.begin(), more.end());
}

void append(Bytes& b, const std::string& s)
{
    b.insert(b.end(), s.begin(), s.end());
}

void TestHeader()
{
    CASE("header：版本/cmd/长度小端，async 置最高位");
    Bytes h = encodeHeader(CMD_KEY_EVENT, 18);
    CHECK_EQ(h.size(), size_t(8));
    CHECK_EQ(u16At(h, 0), uint16_t(0x1001));
    CHECK_EQ(u16At(h, 2), uint16_t(0x0101));
    CHECK_EQ(u32At(h, 4), 18u);
    Bytes a = encodeHeader(CMD_IME_ACTIVATED, 8, true);
    CHECK_EQ(u16At(a, 0), uint16_t(0x9001));

    HeaderInfo info;
    CHECK(decodeHeader(a.data(), info) == HeaderError::None);
    CHECK(info.isAsync);
    CHECK_EQ(info.cmd, uint16_t(CMD_IME_ACTIVATED));

    Bytes bad = encodeHeader(CMD_ACK, 0);
    bad[1] = 0x20; // major 2
    CHECK(decodeHeader(bad.data(), info) == HeaderError::VersionMismatch);
    Bytes big = encodeHeader(CMD_ACK, MAX_PAYLOAD_SIZE + 1);
    CHECK(decodeHeader(big.data(), info) == HeaderError::PayloadTooLarge);
}

void TestKeyEvent()
{
    CASE("KeyEvent：18 字节 payload，字段偏移与 KeyPayload 一致");
    KeyEvent e;
    e.keyCode = 0x4E;
    e.modifiers = KEYMOD_SHIFT;
    e.eventType = KEY_EVENT_UP;
    e.toggles = TOGGLE_CAPSLOCK;
    e.eventSeq = 0x1234;
    e.prevChar = 0x33;
    Bytes f = encodeKeyEventFrame(e);
    CHECK_EQ(f.size(), size_t(8 + 18));
    CHECK_EQ(u32At(f, 4), 18u);
    CHECK_EQ(u32At(f, 8), 0x4Eu);
    CHECK_EQ(u32At(f, 12), 0u);
    CHECK_EQ(u32At(f, 16), uint32_t(KEYMOD_SHIFT));
    CHECK_EQ(f[20], uint8_t(KEY_EVENT_UP));
    CHECK_EQ(f[21], uint8_t(TOGGLE_CAPSLOCK));
    CHECK_EQ(u16At(f, 22), uint16_t(0x1234));
    CHECK_EQ(u16At(f, 24), uint16_t(0x33));
}

void TestFocusGained()
{
    CASE("FocusGained：定长 39 字节 + bundleId 段 + windowClass 段");
    Bytes f = encodeFocusGainedFrame(0x1122334455667788ull, INPUT_SCOPE_PASSWORD_BIT, "gedit", "wc");
    Bytes p(f.begin() + 8, f.end());
    // ← 发短了 Rust `FocusGainedPayload::from_bytes` 解码恒失败（macOS 12 字节那次事故）
    CHECK(p.size() >= 39);
    CHECK_EQ(p.size(), size_t(39 + 4 + 5 + 4 + 2));
    for (int i = 0; i < 20; ++i) {
        CHECK_EQ(p[i], uint8_t(0)); // caret 段全 0
    }
    CHECK_EQ(u32At(p, 20), 0x55667788u); // clientToken 低 32
    CHECK_EQ(u32At(p, 24), 0x11223344u); // clientToken 高 32 = 宿主标识
    CHECK_EQ(u32At(p, 28), 0x80000000u); // IS_PASSWORD
    CHECK_EQ(u32At(p, 32), 0u);
    CHECK_EQ(p[36], uint8_t(0));
    CHECK_EQ(p[37], uint8_t(0));
    CHECK_EQ(p[38], uint8_t(0));
    CHECK_EQ(u32At(p, 39), 5u); // bundleIdLen，Rust VAR_SECTION_OFFSET = 39
    CHECK(std::string(p.begin() + 43, p.begin() + 48) == "gedit");
    CHECK_EQ(u32At(p, 48), 2u);
    CHECK(std::string(p.begin() + 52, p.end()) == "wc");
}

void TestFocusLostAndToken()
{
    CASE("FocusLost 9 字节同步；IME_ACTIVATED 带 AsyncFlag（服务端从不回响应）");
    Bytes f = encodeFocusLostFrame(0xAABBCCDD00000001ull, FOCUS_LOST_REASON_THREAD);
    CHECK_EQ(u16At(f, 0), uint16_t(0x1001));
    CHECK_EQ(u32At(f, 4), 9u);
    CHECK_EQ(u32At(f, 8), 1u);
    CHECK_EQ(u32At(f, 12), 0xAABBCCDDu);
    CHECK_EQ(f[16], uint8_t(0));

    Bytes a = encodeClientTokenFrame(CMD_IME_ACTIVATED, 7);
    // ← 不带 async 的话调用方会去读一个永远不来的响应，fcitx5 主线程卡 2 秒
    CHECK(u16At(a, 0) & ASYNC_FLAG);
    CHECK_EQ(u32At(a, 4), 8u);
}

void TestSmallUpstream()
{
    CASE("CaretUpdate 12 字节；Select/Hover/Scroll/MenuAction 为 i32");
    Bytes c = encodeCaretUpdateFrame(-5, 300, 18);
    CHECK_EQ(u32At(c, 4), 12u);
    CHECK_EQ(int32_t(u32At(c, 8)), -5);
    CHECK_EQ(u32At(c, 12), 300u);
    CHECK_EQ(u32At(c, 16), 18u);

    CHECK_EQ(u16At(encodeCandidateSelectFrame(2), 2), uint16_t(CMD_CANDIDATE_SELECT));
    CHECK_EQ(int32_t(u32At(encodeCandidateHoverFrame(-1), 8)), -1);
    CHECK_EQ(int32_t(u32At(encodeCandidateScrollFrame(-120), 8)), -120);
    CHECK_EQ(u16At(encodeMenuActionFrame(9), 2), uint16_t(CMD_MENU_ACTION));

    Bytes e = encodeExtFrame("pos.candidate", "{}");
    CHECK_EQ(u32At(e, 8), 13u);
    CHECK_EQ(u32At(e, 8 + 4 + 13), 2u);
}

void TestDecodeCommit()
{
    CASE("CommitText：flags + 两段长度 + 两段文本；截断返回空");
    Bytes p;
    append(p, le32(COMMIT_FLAG_HAS_NEW_COMPOSITION));
    append(p, le32(6));
    append(p, le32(1));
    append(p, std::string("你好"));
    append(p, std::string("n"));
    auto c = decodeCommitText(p);
    CHECK(c.has_value());
    CHECK(c->text == "你好");
    CHECK(c->newComposition == "n");
    p.pop_back();
    CHECK(!decodeCommitText(p).has_value());
}

void TestDecodeComposition()
{
    CASE("UpdateComposition：caret u32 + 剩余字节是文本");
    Bytes p = le32(3);
    append(p, std::string("nih"));
    auto u = decodeUpdateComposition(p);
    CHECK(u.has_value());
    CHECK_EQ(u->caretPos, 3u);
    CHECK(u->text == "nih");
    CHECK(!decodeUpdateComposition(Bytes{1, 2}).has_value());
}

void TestDecodeDeferred()
{
    CASE("HoldComposition / CommitAndHold 布局");
    Bytes h = le32(500);
    append(h, le32(3));
    append(h, std::string("，"));
    auto hd = decodeHoldComposition(h);
    CHECK(hd.has_value());
    CHECK_EQ(hd->timeoutMs, 500u);
    CHECK(hd->holdText == "，");
    CHECK(hd->commitText.empty());

    Bytes c = le32(0);
    append(c, le32(3));
    append(c, le32(1));
    append(c, std::string("工"));
    append(c, std::string("a"));
    auto cd = decodeCommitAndHold(c);
    CHECK(cd.has_value());
    CHECK(cd->commitText == "工");
    CHECK(cd->holdText == "a");
}

void TestDecodeMisc()
{
    CASE("ReplaceBackward / CommitTextWithCursor / MoveCursor / KeyType");
    Bytes r = le32(1);
    append(r, le32(1));
    append(r, std::string("."));
    auto rd = decodeReplaceBackward(r);
    CHECK(rd && rd->count == 1 && rd->text == ".");

    Bytes w = le32(6);
    append(w, le32(1));
    append(w, std::string("（）"));
    auto wd = decodeCommitTextWithCursor(w);
    CHECK(wd && wd->cursorOffset == 1 && wd->text == "（）");

    auto m = decodeMoveCursor(le32(1));
    CHECK(m && *m == 1);
    CHECK(decodeKeyType(Bytes{'h', 'i'}) == "hi");
}

void TestDecodeRender()
{
    CASE("HostRenderFrame 24/28 字节；OverlayFrame 68 字节；CandidateRects；Ext 信封");
    Bytes f;
    for (uint32_t v : {7u, uint32_t(-10), 20u, 100u, 50u, 0x9u}) {
        append(f, le32(v));
    }
    auto fr = decodeHostRenderFrame(f);
    CHECK(fr.has_value());
    CHECK_EQ(fr->x, -10);
    CHECK_EQ(fr->scale, 1u);
    CHECK(fr->visible());
    CHECK((fr->flags & FRAME_FLAG_ABSOLUTE_POS) != 0);
    append(f, le32(2));
    CHECK_EQ(decodeHostRenderFrame(f)->scale, 2u);
    CHECK(!decodeHostRenderFrame(Bytes(20, 0)).has_value());

    // CMD_OVERLAY_FRAME：偏移按 Rust `encode_overlay_frame` 写死（布局改了两边一起改）。
    Bytes ov;
    for (uint32_t v : {2u, 9u, 120u, 40u, 0x5u, 1u, uint32_t(-30), 200u, uint32_t(-31), 150u, 7u,
                       16u, 3u, 4u, 110u, 30u, 1500u}) {
        append(ov, le32(v));
    }
    auto od = decodeOverlayFrame(ov);
    CHECK(od.has_value());
    CHECK_EQ(od->kind, 2u);
    CHECK_EQ(od->seq, 9u);
    CHECK_EQ(od->width, 120u);
    CHECK_EQ(od->height, 40u);
    CHECK(od->visible());
    CHECK_EQ(od->place, 1u);
    CHECK_EQ(od->x, -30);
    CHECK_EQ(od->y, 200);
    CHECK_EQ(od->altX, -31);
    CHECK_EQ(od->altY, 150);
    CHECK_EQ(od->anchor, 7u);
    CHECK_EQ(od->margin, 16);
    CHECK_EQ(od->contentX, 3);
    CHECK_EQ(od->contentY, 4);
    CHECK_EQ(od->contentW, 110u);
    CHECK_EQ(od->contentH, 30u);
    CHECK_EQ(od->durationMs, 1500);
    ov.pop_back();
    CHECK(!decodeOverlayFrame(ov).has_value());

    Bytes rects = le32(1);
    for (uint32_t v : {0u, 4u, 5u, 30u, 20u}) {
        append(rects, le32(v));
    }
    auto rd = decodeCandidateRects(rects);
    CHECK(rd && rd->size() == 1);
    CHECK((*rd)[0].contains(4, 5));
    CHECK(!(*rd)[0].contains(34, 5));
    rects.pop_back();
    CHECK(!decodeCandidateRects(rects).has_value());

    Bytes e = le32(4);
    append(e, std::string("shot"));
    append(e, le32(2));
    append(e, std::string("{}"));
    auto ed = decodeExt(e);
    CHECK(ed && ed->kind == "shot" && ed->body.size() == 2);
    e.pop_back();
    CHECK(!decodeExt(e).has_value());
}

void TestStableHash()
{
    CASE("stableHash：空串 0，非空恒非 0 且稳定");
    CHECK_EQ(stableHash(""), 0u);
    CHECK(stableHash("gedit") != 0);
    CHECK_EQ(stableHash("gedit"), stableHash("gedit"));
    CHECK(stableHash("gedit") != stableHash("kate"));
    // 与 Swift FNV-1a 同算法：'a' 的标准 FNV-1a 32 值
    CHECK_EQ(stableHash("a"), 0xE40C292Cu);
}

} // namespace

int main()
{
    TestHeader();
    TestKeyEvent();
    TestFocusGained();
    TestFocusLostAndToken();
    TestSmallUpstream();
    TestDecodeCommit();
    TestDecodeComposition();
    TestDecodeDeferred();
    TestDecodeMisc();
    TestDecodeRender();
    TestStableHash();
    TEST_MAIN_END();
}
