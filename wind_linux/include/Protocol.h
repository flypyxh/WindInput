// 协议常量与帧布局：**直接复用** wind_tsf/include/BinaryProtocol.h，不另起第四份拷贝。
//
// 协议的 SSOT 是 Rust `wind_input/crates/wind-ipc/src/protocol.rs` + `codec.rs`，镜像有
// C++（wind_tsf）与 Swift（wind_macos）两份。Linux addon 若再抄一份，改一个 cmd id 就要
// 同步四处——漏一处的症状永远是「静默丢消息」。BinaryProtocol.h 本身是纯标准 C++
// （<cstdint>/<vector>/<string>），唯一不可移植的是 inline 函数 `GetCurrentModifiers()`，
// 它调 Win32 的 `GetAsyncKeyState`。inline 函数体即使不被调用也要能编过，故在 include
// 前后用宏把那几个 Win32 符号垫成常量、再 #undef——**本文件之外任何地方都不许调
// `GetCurrentModifiers()`**（Linux 的修饰键状态来自 Fcitx 的 Key::states()）。
//
// 若将来 BinaryProtocol.h 又引入别的 Win32 依赖，这里会编译失败——那正是想要的信号：
// 要么把新依赖挪进 wind_tsf 的 .cpp，要么在这里补垫片，而不是在 Linux 侧复制协议。
#pragma once

#define GetAsyncKeyState(vk) (static_cast<short>(0))
#define VK_SHIFT 0
#define VK_CONTROL 0
#define VK_MENU 0
#define VK_LWIN 0
#define VK_RWIN 0
#define VK_LSHIFT 0
#define VK_RSHIFT 0
#define VK_LCONTROL 0
#define VK_RCONTROL 0

#include "../../wind_tsf/include/BinaryProtocol.h"

#undef GetAsyncKeyState
#undef VK_SHIFT
#undef VK_CONTROL
#undef VK_MENU
#undef VK_LWIN
#undef VK_RWIN
#undef VK_LSHIFT
#undef VK_RSHIFT
#undef VK_LCONTROL
#undef VK_RCONTROL
