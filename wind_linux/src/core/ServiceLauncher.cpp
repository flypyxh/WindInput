#include "ServiceLauncher.h"

#include <algorithm>
#include <cerrno>
#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
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

bool spawnDetached(const std::vector<std::string>& argv, const std::string& workDir)
{
    if (argv.empty() || access(argv[0].c_str(), X_OK) != 0) {
        return false;
    }
    // fork 之后、exec 之前只能调 async-signal-safe 的函数（fcitx5 是多线程进程，别的线程可能正
    // 拿着 malloc 的锁）：argv、/dev/null、fd 上限都在这里备好。
    std::vector<char*> cargv;
    for (const auto& a : argv) {
        cargv.push_back(const_cast<char*>(a.c_str()));
    }
    cargv.push_back(nullptr);
    const char* dir = workDir.empty() ? nullptr : workDir.c_str();
    const int devnull = ::open("/dev/null", O_RDWR | O_CLOEXEC);
    rlimit rl{};
    const int maxFd = ::getrlimit(RLIMIT_NOFILE, &rl) == 0 && rl.rlim_cur != RLIM_INFINITY
        ? int(std::min<rlim_t>(rl.rlim_cur, 65536))
        : 65536;

    const pid_t child = ::fork();
    if (child < 0) {
        if (devnull >= 0) {
            ::close(devnull);
        }
        return false;
    }
    if (child == 0) {
        // 中间进程：新会话（fcitx5 退出 / 重启时不连带杀掉子进程，也不受其终端影响），再 fork
        // 一次就退出——孙进程被 init（或会话的 subreaper）收养，fcitx5 这边不留僵尸。
        ::setsid();
        const pid_t grandchild = ::fork();
        if (grandchild != 0) {
            ::_exit(grandchild < 0 ? 1 : 0);
        }
        // 孙进程：信号掩码清空、处置全部复位（fcitx5 忽略的 SIGPIPE 等会跨 exec 继承下去）；
        // stdio 接 /dev/null；3 起的 fd 全关（fcitx5 的 socket、X 连接、DBus 等不带
        // CLOEXEC 的都会漏进去）。
        sigset_t none;
        ::sigemptyset(&none);
        ::sigprocmask(SIG_SETMASK, &none, nullptr);
        struct sigaction dfl {};
        dfl.sa_handler = SIG_DFL;
        for (int sig = 1; sig < NSIG; ++sig) {
            ::sigaction(sig, &dfl, nullptr); // SIGKILL / SIGSTOP 失败无妨
        }
        if (devnull >= 0) {
            ::dup2(devnull, 0);
            ::dup2(devnull, 1);
            ::dup2(devnull, 2);
        }
#ifdef SYS_close_range
        if (::syscall(SYS_close_range, 3u, ~0u, 0u) != 0)
#endif
        {
            for (int fd = 3; fd < maxFd; ++fd) {
                ::close(fd);
            }
        }
        if (dir && ::chdir(dir) != 0) {
            ::_exit(127);
        }
        ::execve(cargv[0], cargv.data(), environ);
        ::_exit(127);
    }
    if (devnull >= 0) {
        ::close(devnull);
    }
    int status = 0;
    pid_t r;
    do {
        r = ::waitpid(child, &status, 0);
    } while (r < 0 && errno == EINTR);
    if (r < 0) {
        return errno == ECHILD; // 有人替我们收了（SIGCHLD 被忽略时内核自动回收）：只能当成功
    }
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

bool spawnDetached(const std::string& path)
{
    return spawnDetached(std::vector<std::string>{path}, std::string());
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
