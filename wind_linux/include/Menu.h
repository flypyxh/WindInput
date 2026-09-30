// 自绘菜单（主菜单 / 候选右键菜单）的纯逻辑：级与 kind 的换算、右键目标、空闲超时配置。
//
// 菜单与候选窗、浮层同构：服务进程光栅化（复用 Windows 的 popup_menu），本端只贴图、在菜单
// 打开期间抓住指针把原始事件报回去（CMD_MENU_POINTER）。命中测试、高亮、子菜单都在服务端，
// 本端不知道哪一行是什么——这里只有「什么时候开、开在哪、多久没动静就收」这几件事。
#pragma once

#include <cstdint>

namespace windlinux {

/// CMD_OVERLAY_FRAME 的 kind → 菜单级（0 = 顶层）；不是菜单层返回 -1。
int menuLevelOfKind(uint32_t kind);

/// 候选窗上右键的命中结果 → `menu.open` 的 target：命中候选给页内下标（候选右键菜单），
/// 翻页按钮 / 空白处给 -1（功能主菜单）——同 Windows `CandidateWindow::right_click`。
int32_t contextMenuTarget(int32_t hit);

/// 菜单空闲超时（毫秒）：这么久没有任何指针事件 / 菜单帧就本端收菜单并报 `menu.dismiss`。
/// 默认 60000，与服务端 `MENU_IDLE_TIMEOUT` 同值；环境变量 `WIND_MENU_IDLE_TIMEOUT_MS`
/// （正整数）可覆盖——给 e2e 验这条兜底用，不是用户配置。
constexpr uint32_t kDefaultMenuIdleTimeoutMs = 60000;
uint32_t menuIdleTimeoutMs(const char* env);

} // namespace windlinux
