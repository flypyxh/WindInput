// 中文模式空缓冲空格的吃键判据（论坛 t337）。
//
// 空缓冲空格默认透传，保留宿主对空格键的原生语义（网页滚动等）；全角态吃下出 U+3000。
// 自定义标点「空格」行配了半角列时也得吃：只在 OnKeyDown 转发不够——Notepad++、WPS 这类
// 宿主按 OnTestKeyDown 的结论走，不吃就直接收到半角空格；照调 OnKeyDown 的 Chrome 类宿主
// 才「碰巧正常」。列掩码由 core 推送（CONFIG_KEY_SPACE_CUSTOM_COLS）。
//
// 本头文件刻意**不含任何 Win32 头**，好用 g++ 在 Linux 上单测（tests/space_key_policy_test.cpp）。
#pragma once

#include <cstdint>

namespace wind
{
namespace spacekey
{

/// 自定义标点表的列号，与 core `wind_punct::punct_col_idx` 逐条对应：0=中半 1=英全 2=中全 3=英半。
inline int PunctColumn(bool chinesePunct, bool fullWidth)
{
    if (chinesePunct)
        return fullWidth ? 2 : 0;
    return fullWidth ? 1 : 3;
}

/// 中文模式、无输入会话时空格该不该吃。
inline bool ShouldEatIdleSpace(bool chinesePunct, bool fullWidth, uint8_t customCols)
{
    return fullWidth || (customCols & (1u << PunctColumn(chinesePunct, fullWidth))) != 0;
}

} // namespace spacekey
} // namespace wind
