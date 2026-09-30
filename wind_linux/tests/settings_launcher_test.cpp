// SettingsLauncher：`settings.open` body 解析与 argv 组装（程序路径不受信封内容影响）。
#include "SettingsLauncher.h"
#include "TestHarness.h"

#include <cstdlib>

using namespace windlinux;

using Args = std::vector<std::string>;

int main()
{
    CASE("服务端的典型 body：页 + 附加参数，顺序保留");
    auto a = parseSettingsOpenArgs(R"({"args":["--page=dict","--schema=wubi86"]})");
    CHECK(a.has_value());
    CHECK_EQ(*a, (Args{"--page=dict", "--schema=wubi86"}));

    CASE("空数组 / 缺 args 键 = 打开默认页");
    CHECK_EQ(*parseSettingsOpenArgs(R"({"args":[]})"), Args{});
    CHECK_EQ(*parseSettingsOpenArgs(R"({})"), Args{});
    CHECK_EQ(*parseSettingsOpenArgs(R"( { "future" : {"x":[1,true,null]} , "args" : [ "--page=ui" ] } )"),
             (Args{"--page=ui"}));

    CASE("含空白与中文的值仍是一个参数（加词页 --text=你 好）");
    CHECK_EQ(*parseSettingsOpenArgs(R"({"args":["--page=add-word","--text=你 好"]})"),
             (Args{"--page=add-word", "--text=你 好"}));

    CASE("JSON 转义全解：引号、反斜杠、\\u 与代理对");
    CHECK_EQ(*parseSettingsOpenArgs(R"({"args":["a\"b\\c","你好","😀","x\ty"]})"),
             (Args{"a\"b\\c", "你好", "\xF0\x9F\x98\x80", "x\ty"}));

    CASE("畸形或越界：整条作废，不带着残缺参数启动");
    CHECK(!parseSettingsOpenArgs(""));
    CHECK(!parseSettingsOpenArgs("[]"));
    CHECK(!parseSettingsOpenArgs(R"({"args":"--page=ui"})"));
    CHECK(!parseSettingsOpenArgs(R"({"args":["--page=ui",1]})"));
    CHECK(!parseSettingsOpenArgs(R"({"args":["--page=ui")"));
    CHECK(!parseSettingsOpenArgs(R"({"args":["a\u0000b"]})"));
    CHECK(!parseSettingsOpenArgs(R"({"args":["\ud83d"]})"));
    CHECK(!parseSettingsOpenArgs(R"({"args":[]} trailing)"));
    std::string many = R"({"args":[)";
    for (int i = 0; i < 65; ++i) {
        many += std::string(i ? "," : "") + "\"--x\"";
    }
    CHECK(!parseSettingsOpenArgs(many + "]}"));

    CASE("argv[0] 恒为设置程序路径；信封内容只进参数位");
    unsetenv("WIND_INPUT_SETTING");
    auto argv = settingsArgv({"/bin/sh", "-c", "evil"});
    CHECK_EQ(argv[0], std::string("/usr/lib/windinput/wind_setting"));
    CHECK_EQ(argv.size(), size_t(4));
    CHECK_EQ(argv[1], std::string("/bin/sh"));

    CASE("环境变量覆盖设置程序路径");
    setenv("WIND_INPUT_SETTING", "/opt/x/wind_setting", 1);
    CHECK_EQ(settingsArgv({})[0], std::string("/opt/x/wind_setting"));
    unsetenv("WIND_INPUT_SETTING");

    TEST_MAIN_END();
}
