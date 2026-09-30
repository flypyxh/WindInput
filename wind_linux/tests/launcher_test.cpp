// ServiceLauncher：拉起是否真的发生、节流是否生效、缺失路径是否如实失败。
#include "ServiceLauncher.h"
#include "TestHarness.h"

#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <sys/stat.h>
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
