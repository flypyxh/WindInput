// SHM 帧读端与候选窗落位几何单测。SHM 用本进程建一段假装是服务端写的（布局按 Rust
// `shared_render_frame.rs::encode_frame_into`：64 字节头 + 紧跟像素）。

#include "Protocol.h"
#include "ShmFrame.h"
#include "TestHarness.h"

#include <cstring>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

using namespace windlinux;

namespace {

struct FakeServiceShm {
    std::string name;
    int fd = -1;
    uint8_t* p = nullptr;
    size_t size;

    FakeServiceShm(size_t sz) : size(sz)
    {
        name = "/wl_test_" + std::to_string(getpid());
        shm_unlink(name.c_str());
        fd = shm_open(name.c_str(), O_CREAT | O_RDWR | O_EXCL, 0600);
        if (ftruncate(fd, off_t(size)) != 0) {
            std::printf("ftruncate 失败\n");
        }
        p = static_cast<uint8_t*>(mmap(nullptr, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0));
    }
    ~FakeServiceShm()
    {
        munmap(p, size);
        close(fd);
        shm_unlink(name.c_str());
    }

    void write(uint32_t seq, int32_t x, int32_t y, uint32_t w, uint32_t h, uint8_t fill,
               uint32_t dataSizeOverride = 0)
    {
        SharedRenderHeader hdr{};
        hdr.magic = SHARED_RENDER_MAGIC;
        hdr.version = SHARED_RENDER_VERSION;
        hdr.sequence = seq;
        hdr.flags = SHARED_FLAG_VISIBLE | SHARED_FLAG_CONTENT_READY;
        hdr.x = x;
        hdr.y = y;
        hdr.width = w;
        hdr.height = h;
        hdr.stride = w * 4;
        hdr.dataSize = dataSizeOverride ? dataSizeOverride : w * h * 4;
        size_t pix = size_t(w) * h * 4;
        std::memset(p + 64, fill, pix + 64 <= size ? pix : size - 64); // 越界帧只写头，像素写到映射尾为止
        std::memcpy(p, &hdr, sizeof(hdr));
    }
};

void TestReadFrame()
{
    CASE("SHM：读出头部字段与像素拷贝");
    FakeServiceShm svc(1 << 20);
    ShmFrameReader r;
    CHECK(r.open(svc.name, svc.size));
    svc.write(5, -3, 40, 10, 4, 0x7F);
    SharedFrame f;
    CHECK(r.snapshot(f));
    CHECK_EQ(f.sequence, 5u);
    CHECK_EQ(f.screenX, -3);
    CHECK_EQ(f.screenY, 40);
    CHECK_EQ(f.width, 10u);
    CHECK_EQ(f.bgra.size(), size_t(10 * 4 * 4));
    CHECK_EQ(f.bgra[0], uint8_t(0x7F));
    CHECK((f.flags & SHARED_FLAG_VISIBLE) != 0);

    CASE("拷贝与 SHM 脱钩：服务端写下一帧不改已取出的像素");
    svc.write(6, 0, 0, 10, 4, 0x11);
    CHECK_EQ(f.bgra[0], uint8_t(0x7F));
    CHECK(r.snapshot(f));
    CHECK_EQ(f.bgra[0], uint8_t(0x11));
}

void TestRejectBad()
{
    CASE("magic 不对 / 尺寸越界 / 头部自相矛盾 → 拒收，不越界读");
    FakeServiceShm svc(4096);
    ShmFrameReader r;
    CHECK(r.open(svc.name, svc.size));
    SharedFrame f;
    CHECK(!r.snapshot(f)); // 全 0：magic 不对
    svc.write(1, 0, 0, 100, 100, 0); // 40000 字节 > 4096 映射
    CHECK(!r.snapshot(f));
    svc.write(1, 0, 0, 4, 4, 0, /*dataSize=*/16); // dataSize < stride*height
    CHECK(!r.snapshot(f));

    CASE("隐藏帧（dataSize=0）：成功、像素为空");
    SharedRenderHeader hdr{};
    hdr.magic = SHARED_RENDER_MAGIC;
    hdr.sequence = 9;
    std::memcpy(svc.p, &hdr, sizeof(hdr));
    CHECK(r.snapshot(f));
    CHECK(f.bgra.empty());
    CHECK_EQ(f.sequence, 9u);
}

void TestOpenMissing()
{
    CASE("服务还没建段：open 失败而不是崩");
    ShmFrameReader r;
    CHECK(!r.open("/wl_definitely_missing_" + std::to_string(getpid())));
    CHECK(!r.isOpen());
    SharedFrame f;
    CHECK(!r.snapshot(f));
}

void TestPlacement()
{
    const Rect wa{0, 0, 1920, 1080};
    CASE("落位：放得下就照服务端给的点");
    Rect r = placePanel(100, 200, 300, 150, wa, false);
    CHECK(r.x == 100 && r.y == 200);

    CASE("右溢回拉、左溢推回");
    r = placePanel(1800, 200, 300, 150, wa, false);
    CHECK_EQ(r.x, 1620);
    r = placePanel(-50, 200, 300, 150, wa, false);
    CHECK_EQ(r.x, 0);

    CASE("下方放不下 → 翻到光标上方（让出 18px 光标高）");
    r = placePanel(100, 1000, 300, 150, wa, false);
    CHECK_EQ(r.y, 1000 - 18 - 150);

    CASE("固定位置（absolute）只钳制不翻转");
    r = placePanel(100, 1000, 300, 150, wa, true);
    CHECK_EQ(r.y, 1080 - 150);

    CASE("翻转后仍越界 → 夹进工作区");
    r = placePanel(100, 100, 300, 1000, Rect{0, 0, 1920, 1080}, false);
    CHECK(r.y >= 0 && r.y + 1000 <= 1080);
}

void TestHitTest()
{
    CASE("命中测试：候选下标 / 翻页 -1 -2 / 未命中");
    std::vector<CandidateHitRect> rects = {{0, 10, 5, 40, 20}, {1, 60, 5, 40, 20}, {-2, 200, 5, 16, 20}};
    CHECK_EQ(hitTest(rects, 12, 6), 0);
    CHECK_EQ(hitTest(rects, 99, 24), 1);
    CHECK_EQ(hitTest(rects, 100, 24), kNoHit); // 右边界是开区间
    CHECK_EQ(hitTest(rects, 205, 10), -2);
    CHECK_EQ(hitTest(rects, 0, 0), kNoHit);
}

} // namespace

int main()
{
    TestReadFrame();
    TestRejectBad();
    TestOpenMissing();
    TestPlacement();
    TestHitTest();
    TEST_MAIN_END();
}
