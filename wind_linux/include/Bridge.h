// UDS 客户端：请求/响应通道（BridgeClient）与服务端推送通道（PushClient），外加端点路径。
// 纯 POSIX，不依赖 Fcitx5。对位 Swift BridgeClient.swift / PushClient.swift 与 Rust
// wind-bridge/src/endpoint.rs。
#pragma once

#include "Codec.h"

#include <atomic>
#include <condition_variable>
#include <functional>
#include <mutex>
#include <string>
#include <thread>

namespace windlinux {

// ── 端点 ────────────────────────────────────────────────────────────

/// 变体后缀：dev 为 "Dev"，release 为 ""。环境变量 `WIND_VARIANT=dev`（忽略大小写）强制
/// dev、其它非空值强制 release（与 Rust `wind_config::variant::is_dev` 同一覆盖口）；
/// 未设置时取编译期默认（CMake 选项 WIND_VARIANT_DEV）。
std::string variantSuffix();

/// 运行时目录，与 Rust `endpoint.rs::runtime_dir` 逐条对齐：
/// `$WIND_INPUT_RUNTIME_DIR` → `$XDG_RUNTIME_DIR/WindInput{Dev}` → `/tmp/wind_input{_dev}`。
std::string runtimeDir();
std::string requestSocketPath();
std::string pushSocketPath();
/// POSIX SHM 名：`/WindInput_SHM{Dev}`（与 Rust `endpoint.rs::shm_name` 对齐）。
std::string shmName();
/// 光栅浮层某一层的 SHM 名：`shmName()` + `_TIP` / `_STS` / `_TST`，菜单第 k 级 `_MN<k>`
/// （与 Rust `endpoint.rs::overlay_shm_name` 对齐）。未知层返回空串。
std::string overlayShmName(uint32_t kind);

// ── 请求/响应 ────────────────────────────────────────────────────────

/// 阻塞式 UDS 客户端。**不是线程安全的**：同一连接上「发一帧、读一帧」必须配对，从别的
/// 线程往里插帧会把两边的读写配对错开（macOS 侧 frontCtx 专用连接就是为此而设）。
///
/// 两条保命设置：
///   - I/O 超时（默认 2000ms）：服务卡死/重启时同步读超时报错，而不是把 Fcitx5 主线程
///     无限挂住（那会让整个桌面的输入都卡死）。
///   - 写用 MSG_NOSIGNAL：对端重启后向死连接写不会 SIGPIPE 杀掉 fcitx5 进程
///     （macOS 侧同一问题靠 SO_NOSIGPIPE，Linux 没有这个 socket 选项）。
class BridgeClient {
public:
    explicit BridgeClient(int ioTimeoutMs = 2000) : timeoutMs_(ioTimeoutMs) {}
    ~BridgeClient() { close(); }
    BridgeClient(const BridgeClient&) = delete;
    BridgeClient& operator=(const BridgeClient&) = delete;

    bool connect(const std::string& path);
    bool isConnected() const { return fd_ >= 0; }
    void close();

    bool send(const Bytes& frame);
    bool readFrame(Frame& out);
    /// 发一帧并读一帧响应。任一步失败即关闭连接并返回 false（交上层重连）。
    bool request(const Bytes& frame, Frame& out);

    const std::string& lastError() const { return lastError_; }

private:
    bool fail(const std::string& what);

    int fd_ = -1;
    int timeoutMs_;
    std::string lastError_;
};

/// 从已连接的 fd 读满 n 字节；EOF/错误返回 false。PushClient 与 BridgeClient 共用。
bool readFully(int fd, uint8_t* buf, size_t n);

// ── 推送 ────────────────────────────────────────────────────────────

/// 订阅 `bridge_push.sock` 的后台线程：连上即收 SERVICE_READY，之后逐帧回调。
/// 断开（服务重启）后每秒重连一次，直到 stop()。
///
/// 回调在**推送线程**上执行；调用方负责把它转回自己的主线程（Fcitx5 侧用
/// EventDispatcher）。push 连接不设读超时：它长期空闲等推送，设了会被误判断连。
///
/// 协议上客户端只读不写（服务端 accept 即登记，见 wind-bridge/src/push_unix.rs）。
class PushClient {
public:
    using FrameHandler = std::function<void(Frame)>;
    using StateHandler = std::function<void(bool connected)>;

    PushClient(std::string path, FrameHandler onFrame, StateHandler onState = {});
    ~PushClient() { stop(); }
    PushClient(const PushClient&) = delete;
    PushClient& operator=(const PushClient&) = delete;

    void start();
    void stop();

private:
    void run();

    std::string path_;
    FrameHandler onFrame_;
    StateHandler onState_;
    std::thread thread_;
    std::atomic<bool> stopping_{false};
    std::mutex mu_;
    std::condition_variable cv_;
    int fd_ = -1; // 受 mu_ 保护（stop 要 shutdown 它来打断阻塞读）
};

} // namespace windlinux
