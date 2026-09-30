// 「外部宿主」（macOS `.app` / Linux Fcitx5 addon）专用的 cmd id —— Windows TSF 不用，
// 故不在 wind_tsf/include/BinaryProtocol.h 里。
//
// 真源：wind_input/crates/wind-ipc/src/protocol.rs（同名常量）。Swift 镜像在
// wind_macos/Sources/WindInputKit/IPC/ProtocolTypes.swift。本文件的每个值都由
// tests/protocol_sync_test.cpp 在测试时逐个对照 protocol.rs 的源码——改了那边而忘了
// 这边，单测当场红，而不是等到「消息静默丢失」。
#pragma once

#include <cstdint>

namespace windlinux {

// ── 上行（addon → 服务）──
constexpr uint16_t CMD_CANDIDATE_CONTEXT_MENU = 0x020F;
constexpr uint16_t CMD_MENU_ACTION = 0x0210;
constexpr uint16_t CMD_FRONT_CONTEXT = 0x0215;
/// 自绘菜单打开期间的指针事件（仅 Linux）：event u32 + button u32 + x i32 + y i32（根窗口坐标）。
constexpr uint16_t CMD_MENU_POINTER = 0x021B;
constexpr uint32_t MENU_POINTER_MOTION = 1;
constexpr uint32_t MENU_POINTER_PRESS = 2;

// ── 下行 push（服务 → addon）──
constexpr uint16_t CMD_HOST_RENDER_FRAME = 0x0502;
constexpr uint16_t CMD_CANDIDATE_RECTS = 0x0503;
constexpr uint16_t CMD_MODE_STATUS = 0x0504;
constexpr uint16_t CMD_CANDIDATE_MENU_FLAGS = 0x0505;
constexpr uint16_t CMD_MENU_SHOW = 0x0506;
constexpr uint16_t CMD_TOOLTIP_SHOW = 0x0508;
constexpr uint16_t CMD_TOOLTIP_HIDE = 0x0509;
constexpr uint16_t CMD_STATUS_SHOW = 0x050A;
constexpr uint16_t CMD_STATUS_HIDE = 0x050B;
constexpr uint16_t CMD_TOAST_SHOW = 0x050C;
constexpr uint16_t CMD_TOAST_HIDE = 0x050D;
constexpr uint16_t CMD_KEY_TAP = 0x050E;
constexpr uint16_t CMD_KEY_SEQ = 0x050F;
constexpr uint16_t CMD_KEY_HOLD = 0x0510;
constexpr uint16_t CMD_KEY_RELEASE = 0x0511;
constexpr uint16_t CMD_KEY_TYPE = 0x0512;
/// 光栅浮层帧（状态气泡 / Toast / tooltip），仅 Linux。布局见 Codec.h `OverlayFramePayload`。
constexpr uint16_t CMD_OVERLAY_FRAME = 0x0513;

// ── CMD_OVERLAY_FRAME 的枚举（protocol.rs `pub mod overlay`）──
constexpr uint32_t OVERLAY_KIND_TOOLTIP = 1;
constexpr uint32_t OVERLAY_KIND_STATUS = 2;
constexpr uint32_t OVERLAY_KIND_TOAST = 3;
/// 自绘菜单第 0 级；第 k 级子菜单 = OVERLAY_KIND_MENU + k（k < OVERLAY_MENU_LEVELS）。
constexpr uint32_t OVERLAY_KIND_MENU = 4;
constexpr uint32_t OVERLAY_MENU_LEVELS = 6;
constexpr uint32_t OVERLAY_PLACE_ABSOLUTE = 0;
constexpr uint32_t OVERLAY_PLACE_FLIP = 1;
constexpr uint32_t OVERLAY_PLACE_FOLLOW_CANDIDATE = 2;
constexpr uint32_t OVERLAY_PLACE_ANCHOR = 3;
/// 坐标已由服务端按本端报去的工作区算定：原样摆放、不夹回（命中测试在服务端按这个位置做）。
constexpr uint32_t OVERLAY_PLACE_EXACT = 4;
constexpr uint32_t OVERLAY_ANCHOR_CENTER = 1;
constexpr uint32_t OVERLAY_ANCHOR_TOP_LEFT = 2;
constexpr uint32_t OVERLAY_ANCHOR_TOP_RIGHT = 3;
constexpr uint32_t OVERLAY_ANCHOR_BOTTOM_LEFT = 4;
constexpr uint32_t OVERLAY_ANCHOR_BOTTOM_RIGHT = 5;
constexpr uint32_t OVERLAY_ANCHOR_TOP_CENTER = 6;
constexpr uint32_t OVERLAY_ANCHOR_BOTTOM_CENTER = 7;

// ── 扩展信封 kind（protocol.rs `ext_kind`）──
/// 上行：请求打开自绘菜单。body 见 `encodeMenuOpenFrame`。
inline constexpr const char* EXT_KIND_MENU_OPEN = "menu.open";
/// 上行：本端把菜单收掉了（空闲超时 / 失焦 / 服务重启…），服务端据此复位 menu_open。
inline constexpr const char* EXT_KIND_MENU_DISMISS = "menu.dismiss";

/// InputScope 的 IS_PASSWORD 位（TSF 枚举 31）。服务端据此对密码框强制英文半角直通；
/// 与 Swift `InputController.inputScopePasswordBit`、Rust 协调器的判定同值。
constexpr uint64_t INPUT_SCOPE_PASSWORD_BIT = uint64_t(1) << 31;

/// 帧 flags（SharedRenderHeader / HostRenderFrame 共用）：位图内已画软件阴影。
constexpr uint32_t FRAME_FLAG_SOFTWARE_SHADOW = 0x0004;
/// 帧 flags：`(x, y)` 是固定位置的绝对屏幕坐标，不是按光标推算的落点。
constexpr uint32_t FRAME_FLAG_ABSOLUTE_POS = 0x0008;

} // namespace windlinux
