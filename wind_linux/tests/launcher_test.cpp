// ServiceLauncher：拉起是否真的发生、节流是否生效、缺失路径是否如实失败。
#include "ServiceLauncher.h"
#include "TestHarness.h"

#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <fstream>
#include <pthread.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

using namespace windlinux;

static std::string writeScript(const std::string& dir, const std::string& body)
{
    std::string p = dir + "/fake_service.sh";
    std::ofstream(p) << "#!/bin/sh\n" << body << "\n";
    chmod(p.c_str(), 0755);
    return p;
}

int main()
{
    char tmpl[] = "/tmp/wi-launcher-XXXXXX";
    std::string dir = mkdtemp(tmpl);
    std::string marker = dir + "/started";

    CASE("拉起后子进程确实运行（写出标记文件）");
    std::string svc = writeScript(dir, "echo $$ > " + marker);
    CHECK(spawnDetached(svc));
    for (int i = 0; i < 100 && access(marker.c_str(), F_OK) != 0; ++i) {
        usleep(20 * 1000);
    }
    CHECK(access(marker.c_str(), F_OK) == 0);

    CASE("不留僵尸：中间进程当场回收，程序不是本进程的子进程（程序退出后没有 defunct）");
    {
        std::string out = dir + "/ppid";
        std::string sh = writeScript(dir, "echo $PPID > " + out);
        CHECK(spawnDetached(sh));
        for (int i = 0; i < 100 && access(out.c_str(), F_OK) != 0; ++i) {
            usleep(20 * 1000);
        }
        usleep(100 * 1000); // 等脚本写完并退出
        std::ifstream in(out);
        long ppid = 0;
        in >> ppid;
        CHECK(ppid != 0 && ppid != long(getpid()));
        int st = 0;
        CHECK(waitpid(-1, &st, WNOHANG) < 0 && errno == ECHILD); // 本进程名下没有子进程了
    }

    CASE("fd 不继承、信号掩码与处置复位、新会话、工作目录");
    {
        // 一个不带 CLOEXEC 的 fd（fcitx5 里这样的 fd 不少），放到 77 号好认。
        int raw = open("/dev/null", O_RDONLY);
        CHECK(dup2(raw, 77) == 77);
        close(raw);
        sigset_t blk;
        sigemptyset(&blk);
        sigaddset(&blk, SIGUSR1);
        sigset_t old;
        pthread_sigmask(SIG_BLOCK, &blk, &old);
        signal(SIGUSR2, SIG_IGN);
        std::string out = dir + "/env";
        std::string sh = writeScript(dir, "{ ls /proc/$$/fd | tr '\\n' ' '; echo; "
                                          "grep -E '^(SigBlk|SigIgn)' /proc/$$/status; "
                                          "cut -d' ' -f6 /proc/$$/stat; pwd; } > " + out + ".tmp; mv "
                                          + out + ".tmp " + out);
        CHECK(spawnDetached(std::vector<std::string>{sh}, "/tmp"));
        for (int i = 0; i < 100 && access(out.c_str(), F_OK) != 0; ++i) {
            usleep(20 * 1000);
        }
        pthread_sigmask(SIG_SETMASK, &old, nullptr);
        signal(SIGUSR2, SIG_DFL);
        close(77);
        std::ifstream in(out);
        std::string fds, blkLine, ignLine, sid, cwd;
        std::getline(in, fds);
        std::getline(in, blkLine);
        std::getline(in, ignLine);
        std::getline(in, sid);
        std::getline(in, cwd);
        std::printf("  fd: %s| %s | %s | sid=%s cwd=%s\n", fds.c_str(), blkLine.c_str(),
                    ignLine.c_str(), sid.c_str(), cwd.c_str());
        CHECK(!fds.empty() && (" " + fds).find(" 77 ") == std::string::npos);
        CHECK(blkLine.find("0000000000000000") != std::string::npos);
        // SIGUSR2 = 12 → 位 11（0x800）：父进程忽略它，子进程应已复位为默认。
        unsigned long long ign = 0;
        if (ignLine.size() > 8) {
            ign = std::stoull(ignLine.substr(ignLine.find_first_of("0123456789abcdef", 7)), nullptr, 16);
        }
        CHECK((ign & 0x800ull) == 0);
        CHECK(!sid.empty() && std::stol(sid) != long(getsid(0)));
        CHECK_EQ(cwd, std::string("/tmp"));
    }

    CASE("路径不存在或不可执行：如实返回 false");
    CHECK(!spawnDetached(dir + "/nope"));
    std::ofstream(dir + "/plain") << "x";
    CHECK(!spawnDetached(dir + "/plain"));

    CASE("节流：间隔内第二次不动作，间隔为 0 时每次都动作");
    ServiceLauncher slow(std::chrono::seconds(60));
    CHECK(slow.maybeLaunch(svc));
    CHECK(!slow.maybeLaunch(svc));
    ServiceLauncher fast(std::chrono::seconds(0));
    CHECK(fast.maybeLaunch(svc));
    CHECK(fast.maybeLaunch(svc));

    CASE("失败的尝试同样计入节流（缺服务文件时不能每次按键都重试）");
    ServiceLauncher missing(std::chrono::seconds(60));
    CHECK(!missing.maybeLaunch(dir + "/nope"));
    CHECK(!missing.maybeLaunch(svc));

    CASE("环境变量覆盖服务路径");
    setenv("WIND_INPUT_SERVICE", "/custom/path", 1);
    CHECK_EQ(servicePath(), std::string("/custom/path"));
    unsetenv("WIND_INPUT_SERVICE");
    CHECK(!servicePath().empty());

    if (std::system(("rm -rf " + dir).c_str()) != 0) {
        std::printf("(清理临时目录失败，忽略)\n");
    }
    TEST_MAIN_END();
}
