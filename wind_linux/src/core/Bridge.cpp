#include "Bridge.h"

#include "ExtProtocol.h"

#include <cerrno>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <strings.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <unistd.h>

#ifndef WIND_VARIANT_DEV
#define WIND_VARIANT_DEV 0
#endif

namespace windlinux {

namespace {

std::string envOr(const char* name)
{
    const char* v = std::getenv(name);
    return v ? std::string(v) : std::string();
}

bool isDev()
{
    std::string v = envOr("WIND_VARIANT");
    if (!v.empty()) {
        return strcasecmp(v.c_str(), "dev") == 0;
    }
    return WIND_VARIANT_DEV != 0;
}

int connectUnix(const std::string& path, std::string& err)
{
    sockaddr_un addr{};
    if (path.size() >= sizeof(addr.sun_path)) {
        err = "socket 路径超过 " + std::to_string(sizeof(addr.sun_path) - 1) + " 字节: " + path;
        return -1;
    }
    int fd = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        err = std::string("socket: ") + std::strerror(errno);
        return -1;
    }
    addr.sun_family = AF_UNIX;
    std::memcpy(addr.sun_path, path.c_str(), path.size() + 1);
    if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        err = std::string("connect ") + path + ": " + std::strerror(errno);
        ::close(fd);
        return -1;
    }
    return fd;
}

bool writeFully(int fd, const uint8_t* buf, size_t n)
{
    while (n > 0) {
        ssize_t w = ::send(fd, buf, n, MSG_NOSIGNAL);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            return false;
        }
        buf += w;
        n -= size_t(w);
    }
    return true;
}

/// 读一帧（头 + payload）。
bool readOneFrame(int fd, Frame& out, std::string& err)
{
    uint8_t hdr[HEADER_SIZE];
    if (!readFully(fd, hdr, HEADER_SIZE)) {
        err = errno ? std::string("read header: ") + std::strerror(errno) : "EOF";
        return false;
    }
    HeaderInfo info;
    switch (decodeHeader(hdr, info)) {
    case HeaderError::VersionMismatch: err = "协议版本不匹配"; return false;
    case HeaderError::PayloadTooLarge: err = "payload 过大"; return false;
    case HeaderError::None: break;
    }
    out.cmd = info.cmd;
    out.isAsync = info.isAsync;
    out.payload.assign(info.length, 0);
    if (info.length > 0 && !readFully(fd, out.payload.data(), info.length)) {
        err = "read payload 截断";
        return false;
    }
    return true;
}

} // namespace

bool readFully(int fd, uint8_t* buf, size_t n)
{
    errno = 0;
    while (n > 0) {
        ssize_t r = ::read(fd, buf, n);
        if (r == 0) {
            return false; // EOF
        }
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            return false;
        }
        buf += r;
        n -= size_t(r);
    }
    return true;
}

std::string variantSuffix()
{
    return isDev() ? "Dev" : "";
}

std::string runtimeDir()
{
    std::string env = envOr("WIND_INPUT_RUNTIME_DIR");
    if (!env.empty()) {
        return env;
    }
    std::string xdg = envOr("XDG_RUNTIME_DIR");
    if (!xdg.empty()) {
        return xdg + "/WindInput" + variantSuffix();
    }
    // 无会话运行时目录时的兜底：沿用管道风格的 snake 后缀（`_dev`），同 Rust。
    return std::string("/tmp/wind_input") + (isDev() ? "_dev" : "");
}

std::string requestSocketPath()
{
    return runtimeDir() + "/bridge.sock";
}

std::string pushSocketPath()
{
    return runtimeDir() + "/bridge_push.sock";
}

std::string shmName()
{
    return "/WindInput_SHM" + variantSuffix();
}

std::string overlayShmName(uint32_t kind)
{
    switch (kind) {
    case OVERLAY_KIND_TOOLTIP:
        return shmName() + "_TIP";
    case OVERLAY_KIND_STATUS:
        return shmName() + "_STS";
    case OVERLAY_KIND_TOAST:
        return shmName() + "_TST";
    default:
        // 菜单每级一段：`_MN0`、`_MN1`…
        if (kind >= OVERLAY_KIND_MENU && kind < OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS) {
            return shmName() + "_MN" + std::to_string(kind - OVERLAY_KIND_MENU);
        }
        return {};
    }
}

// ── BridgeClient ───────────────────────────────────────────────────

bool BridgeClient::connect(const std::string& path)
{
    close();
    fd_ = connectUnix(path, lastError_);
    if (fd_ < 0) {
        return false;
    }
    if (timeoutMs_ > 0) {
        timeval tv{};
        tv.tv_sec = timeoutMs_ / 1000;
        tv.tv_usec = (timeoutMs_ % 1000) * 1000;
        ::setsockopt(fd_, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
        ::setsockopt(fd_, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    }
    return true;
}

void BridgeClient::close()
{
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

bool BridgeClient::fail(const std::string& what)
{
    lastError_ = what;
    close();
    return false;
}

bool BridgeClient::send(const Bytes& frame)
{
    if (fd_ < 0) {
        lastError_ = "未连接";
        return false;
    }
    if (!writeFully(fd_, frame.data(), frame.size())) {
        return fail(std::string("write: ") + std::strerror(errno));
    }
    return true;
}

bool BridgeClient::readFrame(Frame& out)
{
    if (fd_ < 0) {
        lastError_ = "未连接";
        return false;
    }
    std::string err;
    if (!readOneFrame(fd_, out, err)) {
        return fail(err);
    }
    return true;
}

bool BridgeClient::request(const Bytes& frame, Frame& out)
{
    return send(frame) && readFrame(out);
}

// ── PushClient ─────────────────────────────────────────────────────

PushClient::PushClient(std::string path, FrameHandler onFrame, StateHandler onState)
    : path_(std::move(path)), onFrame_(std::move(onFrame)), onState_(std::move(onState))
{
}

void PushClient::start()
{
    if (thread_.joinable()) {
        return;
    }
    stopping_ = false;
    thread_ = std::thread([this] { run(); });
}

void PushClient::stop()
{
    stopping_ = true;
    {
        std::lock_guard<std::mutex> lk(mu_);
        if (fd_ >= 0) {
            ::shutdown(fd_, SHUT_RDWR); // 打断阻塞中的 read
        }
    }
    cv_.notify_all();
    if (thread_.joinable()) {
        thread_.join();
    }
}

void PushClient::run()
{
    while (!stopping_) {
        std::string err;
        int fd = connectUnix(path_, err);
        if (fd >= 0) {
            {
                std::lock_guard<std::mutex> lk(mu_);
                // stop() 先置 stopping_ 再拿锁 shutdown：它若抢在这里之前拿了锁，看到的
                // fd_ 还是 -1、什么都没关，只能由这边补查一次，否则下面的 read 永远不醒。
                if (stopping_) {
                    ::close(fd);
                    break;
                }
                fd_ = fd;
            }
            if (onState_) {
                onState_(true);
            }
            Frame f;
            while (!stopping_ && readOneFrame(fd, f, err)) {
                onFrame_(std::move(f));
                f = Frame{};
            }
            {
                std::lock_guard<std::mutex> lk(mu_);
                fd_ = -1;
            }
            ::close(fd);
            if (onState_) {
                onState_(false);
            }
        }
        // 服务未起 / 已断开：1 秒后重试（可被 stop 提前唤醒）。
        std::unique_lock<std::mutex> lk(mu_);
        cv_.wait_for(lk, std::chrono::seconds(1), [this] { return stopping_.load(); });
    }
}

} // namespace windlinux
