#pragma once

// 语言栏「加载中」图标的像素生成（纯 C++17，不含 Win32 头，可在本机单测）。
//
// ## 为什么是加载中而不是本地画主字
//
// 语言栏图标由服务端渲好经共享内存发布（IconShmReader），主字颜色随主题 / 用户配置走。
// DLL 这边拿不到那些颜色，以前在 SHM 不可用时自己按 0/255 画黑白「中/英」，结果服务
// 未就绪 / 重启期间图标颜色时有时无，用户分不清「服务没好」和「正常状态」。
// 现在 DLL 一律不画主字，只画一个与任何模式主字都不像的三点「…」，让这两种情形一眼可分。
//
// ## 画法
//
// 三个横排实心圆点，几何全用整数：坐标单位取 1/(2·SS) 像素，采样点落在每个子像素
// 中心，于是图像对竖直中轴**严格**镜像对称（浮点算距离会在边界上舍入出 1 级 alpha 差）。
// SS×SS 超采样给出简单抗锯齿。尺寸跟 sizePx 走：点径 ≈ 0.2·size、点距 ≈ 0.3125·size（取整像素）。
//
// 颜色是中灰，按任务栏明暗二选一；RGB 在全图恒为该灰，只靠 alpha 成形（非预乘 BGRA，
// 与 CreateIconIndirect 的 hbmColor 约定一致）。

#include <cstdint>
#include <vector>

namespace loading_icon
{

// 浅色任务栏 #6E6E6E / 深色任务栏 #B4B4B4。
constexpr uint8_t kGrayOnLight = 0x6E;
constexpr uint8_t kGrayOnDark  = 0xB4;

constexpr int kSuperSample = 4;

inline uint8_t GrayFor(bool darkTaskbar)
{
    return darkTaskbar ? kGrayOnDark : kGrayOnLight;
}

// 生成 sizePx×sizePx 的 BGRA（top-down、非预乘）。sizePx < 1 时返回空。
inline std::vector<uint8_t> RenderBgra(int sizePx, bool darkTaskbar)
{
    std::vector<uint8_t> out;
    if (sizePx < 1)
        return out;
    out.assign(static_cast<size_t>(sizePx) * sizePx * 4, 0);

    const int unit   = 2 * kSuperSample;             // 每像素多少坐标单位
    const int center = kSuperSample * sizePx;        // = sizePx/2 像素
    // 点距 0.3125·size，四舍五入到**整像素**：三点的子像素相位因此一致，抗锯齿边完全
    // 相同；不取整时（如 20px 点距 6.25）中间点与两侧点会一实一虚，看着大小不一。
    const int step   = ((sizePx * 5 + 8) / 16) * unit;
    int radius       = (sizePx * unit + 5) / 10;     // 半径 0.1·size
    if (radius < unit)                               // 至少 1px 半径，太小就看不见了
        radius = unit;
    const int64_t r2 = static_cast<int64_t>(radius) * radius;
    const uint8_t gray = GrayFor(darkTaskbar);
    const int maxHits = kSuperSample * kSuperSample;

    for (int y = 0; y < sizePx; ++y)
    {
        for (int x = 0; x < sizePx; ++x)
        {
            int hits = 0;
            for (int sy = 0; sy < kSuperSample; ++sy)
            {
                const int64_t py = static_cast<int64_t>(unit) * y + 2 * sy + 1 - center;
                for (int sx = 0; sx < kSuperSample; ++sx)
                {
                    const int64_t px = static_cast<int64_t>(unit) * x + 2 * sx + 1 - center;
                    for (int k = -1; k <= 1; ++k)
                    {
                        const int64_t dx = px - static_cast<int64_t>(k) * step;
                        if (dx * dx + py * py <= r2)
                        {
                            ++hits;
                            break;
                        }
                    }
                }
            }
            if (hits == 0)
                continue;
            uint8_t* p = &out[(static_cast<size_t>(y) * sizePx + x) * 4];
            p[0] = gray;
            p[1] = gray;
            p[2] = gray;
            p[3] = static_cast<uint8_t>((hits * 255 + maxHits / 2) / maxHits);
        }
    }
    return out;
}

} // namespace loading_icon
