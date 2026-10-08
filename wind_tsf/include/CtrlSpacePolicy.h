// Ctrl+空格 中英切换的吃键判据（GH#172）。
//
// 背景：DLL 在 OnTestKeyDown 无条件吃掉 Ctrl+空格（防 Space 落进输入），OnKeyDown 里还有
// 系统热键失效时的按键侧兜底切换。IDEA / Android Studio 的代码提示正是 Ctrl+空格，用户关不掉。
//
// 修法：服务端把 `keys.ctrl_space_toggle` 的**有效值**（开关 + 自定义按键里绑 none）经
// CONFIG_KEY_CTRL_SPACE_TOGGLE 下发，关闭时本 DLL 不吃、不兜底，键原样交给宿主。
// ⚠ Windows 系统输入法开关热键在 msctf 层（keystroke sink 之下）就消费了按键并翻
// OPENCLOSE compartment，本 DLL 拦不住——那条由服务端拒绝翻转、回包把 compartment 拉回
// （见 wind-coordinator `coordinator/ctrl_space.rs`），而宿主仍收不到键，只能让用户去系统
// 设置里关掉那个热键。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/ctrl_space_policy_test.cpp）。
// 修饰位与 BinaryProtocol.h 的 KEYMOD_* 同值，KeyEventSink.cpp 里有 static_assert 钉住。
#pragma once

#include <cstddef>
#include <cstdint>

namespace wind
{
namespace ctrlspace
{

inline constexpr std::uint32_t kVkSpace = 0x20;
inline constexpr std::uint32_t kModShift = 0x0001; // KEYMOD_SHIFT
inline constexpr std::uint32_t kModCtrl = 0x0002;  // KEYMOD_CTRL
inline constexpr std::uint32_t kModAlt = 0x0004;   // KEYMOD_ALT

/// 未收到服务端下发前的默认值：开（与历史行为一致；DLL 每次重连都从这里起步）。
inline constexpr bool kDefaultEnabled = true;

/// 是不是**光** Ctrl+空格：多一个 Shift / Alt 就是另一个键（Windows 默认热键也只认光 Ctrl）。
inline bool IsCtrlSpaceChord(std::uint32_t vk, std::uint32_t modifiers)
{
    return vk == kVkSpace && (modifiers & kModCtrl) != 0 && (modifiers & (kModAlt | kModShift)) == 0;
}

/// OnTestKeyDown 要不要吃这个键当中英切换。开关关着 ⇒ 不吃，交给宿主。
inline bool ShouldInterceptCtrlSpace(bool enabled, std::uint32_t vk, std::uint32_t modifiers)
{
    return enabled && IsCtrlSpaceChord(vk, modifiers);
}

/// CONFIG_KEY_CTRL_SPACE_TOGGLE 的值 → 开关。格式 enabled(u8)；空值视为畸形，保持原状。
inline bool DecodeToggleValue(const std::uint8_t* data, std::size_t size, bool current)
{
    if (data == nullptr || size == 0)
        return current;
    return data[0] != 0;
}

} // namespace ctrlspace
} // namespace wind
