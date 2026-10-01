// 服务进程的自动拉起。
//
// addon 只是薄壳，服务（wind_input）不在就什么都输入不了；而 Linux 上没有 launchd 那样替我们
// 保活的东西（systemd 用户单元需要用户自己启用），所以由 addon 在连不上时自己拉。
// 重复拉起是安全的：服务自己持 flock 单例，后来者会直接退出。
#pragma once

#include <chrono>
#include <string>
#include <vector>

namespace windlinux {

/// 服务可执行文件路径：环境变量 WIND_INPUT_SERVICE 优先，其次编译期的 WIND_SERVICE_PATH。
std::string servicePath();

/// 脱离 fcitx5 地启动 `argv`（argv[0] 为可执行文件路径，不经 PATH、不经 shell）：
///   - 双 fork：中间进程立即退出、当场回收，程序被 init（或 subreaper）收养——fcitx5 存活期间
///     程序退出（菜单「重启服务」、设置程序关掉）不会留僵尸（fcitx5 不替我们回收，实测留 defunct）；
///   - 新会话（setsid）：fcitx5 退出 / 重启时不连带杀掉它，也不受其终端影响；
///   - 信号掩码清空、处置复位为默认（fcitx5 忽略的 SIGPIPE 等会跨 exec 继承）；
///   - stdio 接 /dev/null，3 起的 fd 全关（fcitx5 的 socket / X 连接 / DBus 不一定带 CLOEXEC）；
///   - 继承环境（DISPLAY / WAYLAND_DISPLAY 等会话变量要靠它带过去）；`workDir` 非空则 chdir。
/// 中间进程成功 fork 出程序即返回 true；不等待程序就绪。
bool spawnDetached(const std::vector<std::string>& argv, const std::string& workDir);
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
