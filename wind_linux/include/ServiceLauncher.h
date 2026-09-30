// 服务进程的自动拉起。
//
// addon 只是薄壳，服务（wind_input）不在就什么都输入不了；而 Linux 上没有 launchd 那样替我们
// 保活的东西（systemd 用户单元需要用户自己启用），所以由 addon 在连不上时自己拉。
// 重复拉起是安全的：服务自己持 flock 单例，后来者会直接退出。
#pragma once

#include <chrono>
#include <string>

namespace windlinux {

/// 服务可执行文件路径：环境变量 WIND_INPUT_SERVICE 优先，其次编译期的 WIND_SERVICE_PATH。
std::string servicePath();

/// 脱离当前进程组地启动 `path`（新会话、stdio 接 /dev/null、继承环境——DISPLAY 等会话变量
/// 要靠它带给服务）。成功 fork 出子进程即返回 true；不等待服务就绪。
bool spawnDetached(const std::string& path);

/// 带节流的拉起：距上次尝试不足 `minInterval` 就不动作。返回是否真的发起了一次拉起。
class ServiceLauncher {
public:
    explicit ServiceLauncher(std::chrono::seconds minInterval = std::chrono::seconds(20))
        : minInterval_(minInterval) {}

    bool maybeLaunch(const std::string& path = servicePath());

private:
    std::chrono::seconds minInterval_;
    std::chrono::steady_clock::time_point last_{};
    bool everTried_ = false;
};

} // namespace windlinux
