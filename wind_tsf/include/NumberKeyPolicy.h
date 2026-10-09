// 数字类键（主键盘 0-9、小键盘 0-9 与运算符；HotkeyManager 归 Number）的吃键判据。
//
// OnTestKeyDown 与 OnKeyDown **调同一个函数**：Test 放行的数字键，OnKeyDown 也必须放行、不转发
// 服务端。此前 OnKeyDown 在中文模式把 Number 一律算输入键转发，靠服务端回 PassThrough 兜着；
// 服务端某条路径一旦出字（论坛 t285 小键盘、主键盘 0、follow_main 运算符先后漏过三次），
// Chrome 类宿主（Test 不吃仍照调 OnKeyDown）就宿主出一次、我们插一次，PIN 框里一键两个字。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/number_key_policy_test.cpp）。
#pragma once

namespace wind
{
namespace numberkey
{

/// 数字类键该不该吃（转发服务端）。
///
/// - 有输入会话：选词 / 翻页 / 顶屏，吃。
/// - 中文 + 全角、无会话：服务端出全角数字，吃（否则纯 TSF 宿主直接得到半角）。
/// - 其余（中文半角空闲、英文半角）：不吃，宿主自己出字。英文全角另有 english_fullwidth 分支，
///   不经本函数。
inline bool ShouldEatNumberKey(bool hasInputSession, bool chineseMode, bool fullWidth)
{
    return hasInputSession || (chineseMode && fullWidth);
}

} // namespace numberkey
} // namespace wind
