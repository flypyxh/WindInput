// 协议同步守卫：ExtProtocol.h 里每个 cmd id 都必须与 Rust 真源 protocol.rs 的同名常量
// 逐字相等。protocol.rs 路径由编译参数 WIND_PROTOCOL_RS 传入。
//
// 为什么要这条：外部宿主专用的 cmd（候选帧、tooltip、按键合成…）Windows 不用，故不在
// wind_tsf/include/BinaryProtocol.h 里，是本目录第二份手抄。手抄的值改一边忘一边，症状只会是
// 「某类消息静默丢失」，没有任何报错——正是 wind_macos/AGENTS.md「协议同步铁律」防的事。

#include "ExtProtocol.h"
#include "Protocol.h"
#include "TestHarness.h"

#include <algorithm>
#include <fstream>
#include <regex>
#include <sstream>

#ifndef WIND_PROTOCOL_RS
#error "编译时需 -DWIND_PROTOCOL_RS=\"<仓库>/wind_input/crates/wind-ipc/src/protocol.rs\""
#endif

namespace {

std::string g_src;

/// 在 protocol.rs 里找 `pub const NAME: u16 = 0x....;`，找不到返回 -1。
long rustConst(const std::string& name)
{
    std::regex re("pub const " + name + R"(:\s*u\d+\s*=\s*(0x[0-9A-Fa-f_]+|\d+)\s*;)");
    std::smatch m;
    if (!std::regex_search(g_src, m, re)) {
        return -1;
    }
    std::string v = m[1];
    v.erase(std::remove(v.begin(), v.end(), '_'), v.end());
    return std::stol(v, nullptr, 0);
}

#define SYNC(name, value)                                                                \
    do {                                                                                 \
        long rs = rustConst(name);                                                       \
        if (rs != long(value)) {                                                         \
            std::printf("  FAIL  %s: C++=0x%04lX  protocol.rs=%s0x%04lX\n", name,        \
                        long(value), rs < 0 ? "(未找到) " : "", rs < 0 ? 0 : rs);        \
            testharness::g_failures++;                                                   \
        }                                                                                \
    } while (0)

} // namespace

int main()
{
    std::ifstream in(WIND_PROTOCOL_RS);
    if (!in) {
        std::printf("  FAIL  打不开 %s\n", WIND_PROTOCOL_RS);
        return 1;
    }
    std::stringstream ss;
    ss << in.rdbuf();
    g_src = ss.str();

    CASE("ExtProtocol.h 的外部宿主专用 cmd 与 protocol.rs 一致");
    SYNC("CMD_CANDIDATE_CONTEXT_MENU", windlinux::CMD_CANDIDATE_CONTEXT_MENU);
    SYNC("CMD_MENU_ACTION", windlinux::CMD_MENU_ACTION);
    SYNC("CMD_FRONT_CONTEXT", windlinux::CMD_FRONT_CONTEXT);
    SYNC("CMD_HOST_RENDER_FRAME", windlinux::CMD_HOST_RENDER_FRAME);
    SYNC("CMD_CANDIDATE_RECTS", windlinux::CMD_CANDIDATE_RECTS);
    SYNC("CMD_MODE_STATUS", windlinux::CMD_MODE_STATUS);
    SYNC("CMD_CANDIDATE_MENU_FLAGS", windlinux::CMD_CANDIDATE_MENU_FLAGS);
    SYNC("CMD_MENU_SHOW", windlinux::CMD_MENU_SHOW);
    SYNC("CMD_TOOLTIP_SHOW", windlinux::CMD_TOOLTIP_SHOW);
    SYNC("CMD_TOOLTIP_HIDE", windlinux::CMD_TOOLTIP_HIDE);
    SYNC("CMD_STATUS_SHOW", windlinux::CMD_STATUS_SHOW);
    SYNC("CMD_STATUS_HIDE", windlinux::CMD_STATUS_HIDE);
    SYNC("CMD_TOAST_SHOW", windlinux::CMD_TOAST_SHOW);
    SYNC("CMD_TOAST_HIDE", windlinux::CMD_TOAST_HIDE);
    SYNC("CMD_KEY_TAP", windlinux::CMD_KEY_TAP);
    SYNC("CMD_KEY_SEQ", windlinux::CMD_KEY_SEQ);
    SYNC("CMD_KEY_HOLD", windlinux::CMD_KEY_HOLD);
    SYNC("CMD_KEY_RELEASE", windlinux::CMD_KEY_RELEASE);
    SYNC("CMD_KEY_TYPE", windlinux::CMD_KEY_TYPE);
    SYNC("FLAG_SOFTWARE_SHADOW", windlinux::FRAME_FLAG_SOFTWARE_SHADOW);
    SYNC("FLAG_ABSOLUTE_POS", windlinux::FRAME_FLAG_ABSOLUTE_POS);

    CASE("addon 用到的 BinaryProtocol.h 常量也与 protocol.rs 一致（那份头同样是手抄镜像）");
    SYNC("CMD_KEY_EVENT", CMD_KEY_EVENT);
    SYNC("CMD_FOCUS_GAINED", CMD_FOCUS_GAINED);
    SYNC("CMD_FOCUS_LOST", CMD_FOCUS_LOST);
    SYNC("CMD_IME_ACTIVATED", CMD_IME_ACTIVATED);
    SYNC("CMD_IME_DEACTIVATED", CMD_IME_DEACTIVATED);
    SYNC("CMD_COMPOSITION_TERMINATED", CMD_COMPOSITION_TERMINATED);
    SYNC("CMD_CARET_UPDATE", CMD_CARET_UPDATE);
    SYNC("CMD_CANDIDATE_SELECT", CMD_CANDIDATE_SELECT);
    SYNC("CMD_CANDIDATE_HOVER", CMD_CANDIDATE_HOVER);
    SYNC("CMD_CANDIDATE_SCROLL", CMD_CANDIDATE_SCROLL);
    SYNC("CMD_EXT", CMD_EXT);
    SYNC("CMD_ACK", CMD_ACK);
    SYNC("CMD_PASS_THROUGH", CMD_PASS_THROUGH);
    SYNC("CMD_COMMIT_TEXT", CMD_COMMIT_TEXT);
    SYNC("CMD_UPDATE_COMPOSITION", CMD_UPDATE_COMPOSITION);
    SYNC("CMD_CLEAR_COMPOSITION", CMD_CLEAR_COMPOSITION);
    SYNC("CMD_CLEAR_THEN_PASS_THROUGH", CMD_CLEAR_THEN_PASS_THROUGH);
    SYNC("CMD_COMMIT_TEXT_WITH_CURSOR", CMD_COMMIT_TEXT_WITH_CURSOR);
    SYNC("CMD_MOVE_CURSOR", CMD_MOVE_CURSOR);
    SYNC("CMD_DELETE_PAIR", CMD_DELETE_PAIR);
    SYNC("CMD_REPLACE_BACKWARD", CMD_REPLACE_BACKWARD);
    SYNC("CMD_HOLD_COMPOSITION", CMD_HOLD_COMPOSITION);
    SYNC("CMD_COMMIT_AND_HOLD", CMD_COMMIT_AND_HOLD);
    SYNC("CMD_COMMIT_THEN_DEFER", CMD_COMMIT_THEN_DEFER);
    SYNC("CMD_CONSUMED", CMD_CONSUMED);
    SYNC("CMD_SERVICE_READY", CMD_SERVICE_READY);
    SYNC("CMD_STATUS_UPDATE", CMD_STATUS_UPDATE);
    SYNC("COMMIT_FLAG_REPLACING_HELD", COMMIT_FLAG_REPLACING_HELD);

    TEST_MAIN_END();
}
