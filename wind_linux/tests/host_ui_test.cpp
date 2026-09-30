// 交给 Fcitx5 呈现的宿主 UI 纯逻辑：中英模式镜像（托盘图标）与应用内预编辑的占位过滤。

#include "Codec.h"
#include "HostUi.h"
#include "Protocol.h"
#include "TestHarness.h"

#include <cstring>
#include <fstream>
#include <sstream>

using namespace windlinux;

namespace {

void putU32(Bytes& b, uint32_t v)
{
    uint8_t x[4];
    std::memcpy(x, &v, 4);
    b.insert(b.end(), x, x + 4);
}

Frame flagsFrame(uint16_t cmd, uint32_t flags)
{
    Frame f;
    f.cmd = cmd;
    putU32(f.payload, flags);
    // STATUS_UPDATE 的真实载荷在 flags 之后还有热键计数与标签，模式只看开头。
    putU32(f.payload, 0);
    putU32(f.payload, 0);
    return f;
}

Frame commitFrame(uint32_t flags, const std::string& text)
{
    Frame f;
    f.cmd = CMD_COMMIT_TEXT;
    putU32(f.payload, flags);
    putU32(f.payload, uint32_t(text.size()));
    putU32(f.payload, 0);
    f.payload.insert(f.payload.end(), text.begin(), text.end());
    return f;
}

std::string readFile(const std::string& path)
{
    std::ifstream in(path);
    std::stringstream ss;
    ss << in.rdbuf();
    return ss.str();
}

} // namespace

int main()
{
    CASE("四种状态帧都以 flags 开头，按 STATUS_CHINESE_MODE 取模式");
    for (uint16_t cmd : {CMD_MODE_PUSH, CMD_STATUS_UPDATE, CMD_STATE_PUSH, CMD_ACTIVATION_STATUS_PUSH}) {
        CHECK(chineseModeOf(flagsFrame(cmd, STATUS_CHINESE_MODE | STATUS_FULL_WIDTH)) == true);
        CHECK(chineseModeOf(flagsFrame(cmd, STATUS_FULL_WIDTH)) == false);
    }

    CASE("载荷不足 4 字节的状态帧不算数（不当英文）");
    {
        Frame f;
        f.cmd = CMD_STATE_PUSH; // push_unix 的测试就发过零载荷的 STATE_PUSH
        CHECK(!chineseModeOf(f).has_value());
    }

    CASE("上屏帧只有带 MODE_CHANGED 时才携带模式");
    CHECK(!chineseModeOf(commitFrame(COMMIT_FLAG_CHINESE_MODE, "你好")).has_value());
    CHECK(chineseModeOf(commitFrame(COMMIT_FLAG_MODE_CHANGED, "Hello")) == false);
    CHECK(chineseModeOf(commitFrame(COMMIT_FLAG_MODE_CHANGED | COMMIT_FLAG_CHINESE_MODE, "")) == true);

    CASE("与模式无关的帧不动镜像");
    {
        Frame f;
        f.cmd = CMD_UPDATE_COMPOSITION;
        putU32(f.payload, STATUS_CHINESE_MODE); // 碰巧同值的 caretPos 不能被当成 flags
        CHECK(!chineseModeOf(f).has_value());
    }

    CASE("镜像：未知时不给图标（退回条目图标），学到后只在变化时报变");
    {
        ModeIndicator m;
        CHECK(!m.known());
        CHECK_EQ(m.iconName(), std::string());
        CHECK_EQ(m.label(), std::string());
        CHECK(m.update(flagsFrame(CMD_MODE_PUSH, STATUS_CHINESE_MODE)));
        CHECK_EQ(m.iconName(), std::string("windinput-zh"));
        CHECK_EQ(m.label(), std::string("中"));
        CHECK_EQ(m.subModeName(), std::string("中文"));
        CHECK(!m.update(flagsFrame(CMD_STATUS_UPDATE, STATUS_CHINESE_MODE))); // 没变
        CHECK(!m.update(commitFrame(0, "你好")));                            // 不带模式
        CHECK(m.update(flagsFrame(CMD_STATE_PUSH, 0)));
        CHECK_EQ(m.iconName(), std::string("windinput-en"));
        CHECK_EQ(m.label(), std::string("英"));
        CHECK_EQ(m.subModeName(), std::string("英文"));
    }

    CASE("预编辑：单个空格的占位组合不显示，其余原样");
    CHECK_EQ(clientPreeditText(" "), std::string());
    CHECK_EQ(clientPreeditText("ni'hao"), std::string("ni'hao"));
    CHECK_EQ(clientPreeditText(""), std::string());
    CHECK_EQ(clientPreeditText("  "), std::string("  ")); // 不是占位：只认服务端那一个常量
    CHECK_EQ(clientPreeditText("hello world"), std::string("hello world"));

    CASE("占位常量与服务端 COMPOSITION_PLACEHOLDER 同值（改了那边这里当场红）");
    {
        std::string rs = WIND_PROTOCOL_RS;
        const std::string ipc = "wind-ipc/src/protocol.rs";
        rs.replace(rs.rfind(ipc), ipc.size(), "wind-bridge/src/handler.rs");
        std::string src = readFile(rs);
        CHECK(!src.empty());
        CHECK(src.find("pub const COMPOSITION_PLACEHOLDER: &str = \" \";") != std::string::npos);
    }

    TEST_MAIN_END();
}
