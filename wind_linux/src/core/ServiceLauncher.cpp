#include "ServiceLauncher.h"

#include <cstdlib>
#include <fcntl.h>
#include <spawn.h>
#include <unistd.h>

extern char** environ;

#ifndef WIND_SERVICE_PATH
#define WIND_SERVICE_PATH "/usr/lib/windinput/wind_input"
#endif

namespace windlinux {

std::string servicePath()
{
    if (const char* env = std::getenv("WIND_INPUT_SERVICE"); env && *env) {
        return env;
    }
    return WIND_SERVICE_PATH;
}

bool spawnDetached(const std::string& path)
{
    if (access(path.c_str(), X_OK) != 0) {
        return false;
    }
    posix_spawn_file_actions_t fa;
    posix_spawnattr_t attr;
    posix_spawn_file_actions_init(&fa);
    posix_spawnattr_init(&attr);
    for (int fd = 0; fd <= 2; ++fd) {
        posix_spawn_file_actions_addopen(&fa, fd, "/dev/null", fd == 0 ? O_RDONLY : O_WRONLY, 0);
    }
    // 新会话：fcitx5 退出/重启时不会连带杀掉服务，也不受其终端影响。
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETSID);
    char* argv[] = {const_cast<char*>(path.c_str()), nullptr};
    pid_t pid = 0;
    int rc = posix_spawn(&pid, path.c_str(), &fa, &attr, argv, environ);
    posix_spawn_file_actions_destroy(&fa);
    posix_spawnattr_destroy(&attr);
    // 子进程不 wait：服务是长驻的，僵尸由 fcitx5 退出时 init 收养；SIGCHLD 在 fcitx5 里
    // 默认已被忽略/回收，不额外处理。
    return rc == 0;
}

bool ServiceLauncher::maybeLaunch(const std::string& path)
{
    auto now = std::chrono::steady_clock::now();
    if (everTried_ && now - last_ < minInterval_) {
        return false;
    }
    everTried_ = true;
    last_ = now;
    return spawnDetached(path);
}

} // namespace windlinux
