// UDS 客户端单测：用进程内的假服务端验证请求/响应、读超时、断线自愈、push 重连。
// socket 放 $TMPDIR（未设则 /tmp）下的短路径——sun_path 上限 108 字节。

#include "Bridge.h"
#include "ExtProtocol.h"
#include "Protocol.h"
#include "TestHarness.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <sys/socket.h>
#include <sys/un.h>
#include <thread>
#include <unistd.h>

using namespace windlinux;

namespace {

std::string tmpPath(const char* name)
{
    const char* t = std::getenv("TMPDIR");
    return std::string(t && *t ? t : "/tmp") + "/wl_" + std::to_string(getpid()) + "_" + name;
}

int listenOn(const std::string& path)
{
    ::unlink(path.c_str());
    int fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
    sockaddr_un a{};
    a.sun_family = AF_UNIX;
    std::strncpy(a.sun_path, path.c_str(), sizeof(a.sun_path) - 1);
    if (::bind(fd, reinterpret_cast<sockaddr*>(&a), sizeof(a)) != 0 || ::listen(fd, 4) != 0) {
        std::printf("  listen %s 失败: %s\n", path.c_str(), std::strerror(errno));
        ::close(fd);
        return -1;
    }
    return fd;
}

void writeAll(int fd, const Bytes& b)
{
    size_t off = 0;
    while (off < b.size()) {
        ssize_t w = ::send(fd, b.data() + off, b.size() - off, MSG_NOSIGNAL);
        if (w <= 0) {
            return;
        }
        off += size_t(w);
    }
}

void TestRequestResponse()
{
    CASE("request：发 KeyEvent、收 Consumed；服务端看到的帧完整");
    std::string path = tmpPath("rr.sock");
    int lfd = listenOn(path);
    CHECK(lfd >= 0);
    std::atomic<uint16_t> seenCmd{0};
    std::atomic<uint32_t> seenLen{0};
    std::thread srv([&] {
        int c = ::accept(lfd, nullptr, nullptr);
        uint8_t hdr[8];
        if (readFully(c, hdr, 8)) {
            HeaderInfo info;
            decodeHeader(hdr, info);
            Bytes p(info.length);
            readFully(c, p.data(), p.size());
            seenCmd = info.cmd;
            seenLen = info.length;
            writeAll(c, encodeEmptyFrame(CMD_CONSUMED));
        }
        ::close(c);
    });
    BridgeClient bc(1000);
    CHECK(bc.connect(path));
    KeyEvent e;
    e.keyCode = 0x4E;
    Frame resp;
    CHECK(bc.request(encodeKeyEventFrame(e), resp));
    CHECK_EQ(resp.cmd, uint16_t(CMD_CONSUMED));
    srv.join();
    CHECK_EQ(seenCmd.load(), uint16_t(CMD_KEY_EVENT));
    CHECK_EQ(seenLen.load(), 18u);

    CASE("对端已关：下一次 request 失败并自动断开（上层据此重连），进程不被 SIGPIPE 杀掉");
    Frame r2;
    CHECK(!bc.request(encodeKeyEventFrame(e), r2));
    CHECK(!bc.isConnected());
    ::close(lfd);
    ::unlink(path.c_str());
}

void TestReadTimeout()
{
    CASE("服务卡死不回：读在超时后失败，而不是无限挂住 fcitx5 主线程");
    std::string path = tmpPath("to.sock");
    int lfd = listenOn(path);
    std::mutex m;
    std::condition_variable cv;
    bool done = false;
    std::thread srv([&] {
        int c = ::accept(lfd, nullptr, nullptr);
        std::unique_lock<std::mutex> lk(m);
        cv.wait(lk, [&] { return done; }); // 收下但永不回复
        ::close(c);
    });
    BridgeClient bc(300);
    CHECK(bc.connect(path));
    auto t0 = std::chrono::steady_clock::now();
    Frame f;
    CHECK(!bc.request(encodeEmptyFrame(CMD_FOCUS_LOST), f));
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                  std::chrono::steady_clock::now() - t0)
                  .count();
    CHECK(ms >= 250 && ms < 2000);
    {
        std::lock_guard<std::mutex> lk(m);
        done = true;
    }
    cv.notify_all();
    srv.join();
    ::close(lfd);
    ::unlink(path.c_str());
}

void TestConnectFailures()
{
    CASE("连接失败：socket 不存在 / 路径超长都如实报错");
    BridgeClient bc;
    CHECK(!bc.connect(tmpPath("nope.sock")));
    CHECK(!bc.lastError().empty());
    CHECK(!bc.connect("/tmp/" + std::string(200, 'x')));
    CHECK(bc.lastError().find("字节") != std::string::npos);
}

void TestPushReconnect()
{
    CASE("push：收帧回调；服务重启后自动重连再收");
    std::string path = tmpPath("push.sock");
    std::mutex m;
    std::condition_variable cv;
    std::vector<uint16_t> got;
    int connects = 0;
    PushClient pc(
        path,
        [&](Frame f) {
            std::lock_guard<std::mutex> lk(m);
            got.push_back(f.cmd);
            cv.notify_all();
        },
        [&](bool up) {
            std::lock_guard<std::mutex> lk(m);
            if (up) {
                ++connects;
            }
        });
    pc.start(); // 服务还没起：客户端应每秒重试

    auto serveOnce = [&](uint16_t cmd) {
        int lfd = listenOn(path);
        int c = ::accept(lfd, nullptr, nullptr);
        writeAll(c, encodeEmptyFrame(CMD_SERVICE_READY));
        writeAll(c, encodeEmptyFrame(cmd));
        std::unique_lock<std::mutex> lk(m);
        cv.wait_for(lk, std::chrono::seconds(3), [&] {
            return !got.empty() && got.back() == cmd;
        });
        lk.unlock();
        ::close(c);
        ::close(lfd);
        ::unlink(path.c_str());
    };
    serveOnce(CMD_STATE_PUSH);
    serveOnce(0x0512); // CMD_KEY_TYPE；第二次 = 服务重启后

    pc.stop();
    std::lock_guard<std::mutex> lk(m);
    CHECK(got.size() == 4);
    if (got.size() == 4) {
        CHECK_EQ(got[0], uint16_t(CMD_SERVICE_READY));
        CHECK_EQ(got[1], uint16_t(CMD_STATE_PUSH));
        CHECK_EQ(got[3], uint16_t(0x0512));
    }
    CHECK_EQ(connects, 2);
}

void TestStopWhileIdle()
{
    CASE("stop：连着但空闲时也能立即退出（shutdown 打断阻塞读）");
    std::string path = tmpPath("idle.sock");
    int lfd = listenOn(path);
    std::thread srv([&] {
        int c = ::accept(lfd, nullptr, nullptr);
        char b;
        while (::read(c, &b, 1) > 0) {
        }
        ::close(c);
    });
    PushClient pc(path, [](Frame) {});
    pc.start();
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    auto t0 = std::chrono::steady_clock::now();
    pc.stop();
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                  std::chrono::steady_clock::now() - t0)
                  .count();
    CHECK(ms < 500);
    srv.join();
    ::close(lfd);
    ::unlink(path.c_str());
}

void TestEndpoints()
{
    CASE("端点：WIND_INPUT_RUNTIME_DIR 优先；XDG_RUNTIME_DIR/WindInput[Dev]；/tmp 兜底");
    setenv("WIND_INPUT_RUNTIME_DIR", "/tmp/rt1", 1);
    CHECK(requestSocketPath() == "/tmp/rt1/bridge.sock");
    CHECK(pushSocketPath() == "/tmp/rt1/bridge_push.sock");
    unsetenv("WIND_INPUT_RUNTIME_DIR");
    setenv("XDG_RUNTIME_DIR", "/run/user/1000", 1);
    setenv("WIND_VARIANT", "release", 1);
    CHECK(runtimeDir() == "/run/user/1000/WindInput");
    CHECK(shmName() == "/WindInput_SHM");
    setenv("WIND_VARIANT", "DEV", 1);
    CHECK(runtimeDir() == "/run/user/1000/WindInputDev");
    CHECK(shmName() == "/WindInput_SHMDev");
    CHECK(overlayShmName(OVERLAY_KIND_STATUS) == "/WindInput_SHMDev_STS");
    CHECK(overlayShmName(OVERLAY_KIND_TOOLTIP) == "/WindInput_SHMDev_TIP");
    CHECK(overlayShmName(OVERLAY_KIND_TOAST) == "/WindInput_SHMDev_TST");
    CHECK(overlayShmName(0).empty());
    unsetenv("XDG_RUNTIME_DIR");
    CHECK(runtimeDir() == "/tmp/wind_input_dev");
    unsetenv("WIND_VARIANT");
}

} // namespace

int main()
{
    TestRequestResponse();
    TestReadTimeout();
    TestConnectFailures();
    TestPushReconnect();
    TestStopWhileIdle();
    TestEndpoints();
    TEST_MAIN_END();
}
