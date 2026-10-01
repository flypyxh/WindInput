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

/// push 通道 connect 的超时（见 connectUnix）。
constexpr int kPushConnectTimeoutMs = 2000;

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

/// connect 的结果：失败时区分「排队超时」（服务在但不 accept）与其它（服务不在）。
enum class ConnectResult { Ok, Failed, TimedOut };

/// 超时在 connect **之前**设：AF_UNIX 的 connect 在对端 listen 队列满时会阻塞（服务整体停住
/// 时就是这样），它认 SO_SNDTIMEO，到点返回 EAGAIN。读超时一并设好（timeoutMs = 0 不设）。
int connectUnix(const std::string& path, int timeoutMs, std::string& err, ConnectResult& result)
{
    result = ConnectResult::Failed;
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
    if (timeoutMs > 0) {
        timeval tv{};
        tv.tv_sec = timeoutMs / 1000;
        tv.tv_usec = (timeoutMs % 1000) * 1000;
        ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
        ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    }
    addr.sun_family = AF_UNIX;
    std::memcpy(addr.sun_path, path.c_str(), path.size() + 1);
    int rc;
    do {
        rc = ::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr));
    } while (rc != 0 && errno == EINTR);
    if (rc != 0) {
        const int e = errno;
        err = std::string("connect ") + path + ": " + std::strerror(e);
        if (e == EAGAIN || e == EWOULDBLOCK || e == EINPROGRESS) {
            result = ConnectResult::TimedOut;
        }
        ::close(fd);
        return -1;
    }
    result = ConnectResult::Ok;
    return fd;
}

/// 写满一帧；失败时 errno 留着给调用方分类（EAGAIN = 写超时）。
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

enum class ReadResult { Ok, Eof, Timeout, Error };

/// 读满 n 字节，`got` 累计实际读到的字节数（判断「一个响应字节都没读到」用）。
ReadResult readExact(int fd, uint8_t* buf, size_t n, size_t& got)
{
    while (n > 0) {
        ssize_t r = ::read(fd, buf, n);
        if (r == 0) {
            return ReadResult::Eof;
        }
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            return errno == EAGAIN || errno == EWOULDBLOCK ? ReadResult::Timeout
                                                           : ReadResult::Error;
        }
        buf += r;
        n -= size_t(r);
        got += size_t(r);
    }
    return ReadResult::Ok;
}

/// 读一帧（头 + payload）。失败时 `why` 给原因、`got` 给已读字节数。
bool readOneFrame(int fd, Frame& out, std::string& err, ReadResult& why, size_t& got)
{
    got = 0;
    uint8_t hdr[HEADER_SIZE];
    why = readExact(fd, hdr, HEADER_SIZE, got);
    if (why != ReadResult::Ok) {
        err = why == ReadResult::Eof ? std::string("EOF")
                                     : std::string("read header: ") + std::strerror(errno);
        return false;
    }
    HeaderInfo info;
    switch (decodeHeader(hdr, info)) {
    case HeaderError::VersionMismatch: err = "协议版本不匹配"; why = ReadResult::Error; return false;
    case HeaderError::PayloadTooLarge: err = "payload 过大"; why = ReadResult::Error; return false;
    case HeaderError::None: break;
    }
    out.cmd = info.cmd;
    out.isAsync = info.isAsync;
    out.payload.assign(info.length, 0);
    if (info.length > 0) {
        why = readExact(fd, out.payload.data(), info.length, got);
        if (why != ReadResult::Ok) {
            err = "read payload 截断";
            return false;
        }
    }
    return true;
}

} // namespace

bool readFully(int fd, uint8_t* buf, size_t n)
{
    errno = 0;
    size_t got = 0;
    return readExact(fd, buf, n, got) == ReadResult::Ok;
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
    if (suspended()) {
        lastFailure_ = Failure::Suspended;
        lastError_ = "服务无响应，熔断中";
        return false;
    }
    ConnectResult result;
    fd_ = connectUnix(path, timeoutMs_, lastError_, result);
    if (fd_ < 0) {
        return fail(result == ConnectResult::TimedOut ? Failure::ConnectTimeout : Failure::Connect,
                    lastError_);
    }
    lastFailure_ = Failure::None;
    return true;
}

void BridgeClient::close()
{
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

bool BridgeClient::suspended() const
{
    return std::chrono::steady_clock::now() < unhealthyUntil_;
}

bool BridgeClient::takeStallFlag()
{
    const bool s = stalled_;
    stalled_ = false;
    return s;
}

bool BridgeClient::fail(Failure kind, const std::string& what)
{
    lastFailure_ = kind;
    lastError_ = what;
    close();
    if (isTimeout(kind)) {
        unhealthyUntil_ = std::chrono::steady_clock::now() + std::chrono::milliseconds(breakerMs_);
        stalled_ = true;
    }
    return false;
}

bool BridgeClient::send(const Bytes& frame)
{
    if (fd_ < 0) {
        lastFailure_ = Failure::NotConnected;
        lastError_ = "未连接";
        return false;
    }
    if (!writeFully(fd_, frame.data(), frame.size())) {
        const int e = errno;
        return fail(e == EAGAIN || e == EWOULDBLOCK ? Failure::SendTimeout : Failure::Send,
                    std::string("write: ") + std::strerror(e));
    }
    return true;
}

bool BridgeClient::readFrame(Frame& out)
{
    if (fd_ < 0) {
        lastFailure_ = Failure::NotConnected;
        lastError_ = "未连接";
        return false;
    }
    std::string err;
    ReadResult why;
    size_t got = 0;
    if (!readOneFrame(fd_, out, err, why, got)) {
        Failure kind = Failure::Broken;
        if (why == ReadResult::Timeout) {
            kind = Failure::ReadTimeout;
        } else if (got == 0) {
            kind = Failure::PeerClosed; // 帧头一个字节都没到就 EOF / ECONNRESET
        }
        return fail(kind, err);
    }
    return true;
}

bool BridgeClient::request(const Bytes& frame, Frame& out)
{
    return send(frame) && readFrame(out);
}

bool requestWithRetry(BridgeClient& client, const Bytes& frame, Frame& out,
                      const std::function<bool()>& reconnect)
{
    if (!client.isConnected() && !reconnect()) {
        return false;
    }
    if (client.request(frame, out)) {
        return true;
    }
    if (!BridgeClient::retryable(client.lastFailure())) {
        return false;
    }
    return reconnect() && client.request(frame, out);
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
        ConnectResult result;
        // 连接也带超时：服务停住时 connect 会阻塞，stop() 要等它返回才 join 得上。
        int fd = connectUnix(path_, kPushConnectTimeoutMs, err, result);
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
            if (kPushConnectTimeoutMs > 0) {
                // 连接时设的超时只为 connect：push 连接长期空闲等推送，读超时会被误判断连。
                timeval none{};
                ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &none, sizeof(none));
            }
            Frame f;
            ReadResult why;
            size_t got = 0;
            while (!stopping_ && readOneFrame(fd, f, err, why, got)) {
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
