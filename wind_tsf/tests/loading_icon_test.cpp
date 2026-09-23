// LoadingIcon 单元测试
//
// 跑法（纯 C++17、不含 Win32 头，本机不需要 MSVC）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o loading_icon_test loading_icon_test.cpp && ./loading_icon_test
// 加 -DDUMP 会把每档打成字符画，肉眼看形状。

#include "LoadingIcon.h"

#include <cstdio>

namespace
{

int g_failures = 0;
const char* g_case = "";
int g_size = 0;

#define CHECK(expr)                                                                             \
    do                                                                                          \
    {                                                                                           \
        if (!(expr))                                                                            \
        {                                                                                       \
            std::printf("  FAIL  %s:%d  [%s size=%d]  %s\n", __FILE__, __LINE__, g_case, g_size, \
                        #expr);                                                                 \
            g_failures++;                                                                       \
        }                                                                                       \
    } while (0)

const int kSizes[] = { 16, 20, 24, 28, 32, 40, 48 };

uint8_t AlphaAt(const std::vector<uint8_t>& px, int size, int x, int y)
{
    return px[(static_cast<size_t>(y) * size + x) * 4 + 3];
}

// 按列求 alpha 和，找出连续非空列段 = 圆点的水平投影。
std::vector<std::pair<int, int>> ColumnRuns(const std::vector<uint8_t>& px, int size)
{
    std::vector<std::pair<int, int>> runs;
    int start = -1;
    for (int x = 0; x <= size; ++x)
    {
        int sum = 0;
        if (x < size)
            for (int y = 0; y < size; ++y)
                sum += AlphaAt(px, size, x, y);
        if (sum > 0 && start < 0)
            start = x;
        if (sum == 0 && start >= 0)
        {
            runs.emplace_back(start, x - 1);
            start = -1;
        }
    }
    return runs;
}

void TestBufferSize()
{
    g_case = "buffer size";
    for (int s : kSizes)
    {
        g_size = s;
        CHECK(loading_icon::RenderBgra(s, false).size() == static_cast<size_t>(s) * s * 4);
    }
    g_size = 0;
    CHECK(loading_icon::RenderBgra(0, false).empty());
    CHECK(loading_icon::RenderBgra(-3, true).empty());
}

void TestThreeSeparateDots()
{
    g_case = "three dots";
    for (int s : kSizes)
    {
        g_size = s;
        const auto px = loading_icon::RenderBgra(s, false);
        const auto runs = ColumnRuns(px, s);
        CHECK(runs.size() == 3);
        if (runs.size() != 3)
            continue;
        // 三段等宽，间距相等（中间点居中）。
        const int w0 = runs[0].second - runs[0].first;
        CHECK(runs[1].second - runs[1].first == w0);
        CHECK(runs[2].second - runs[2].first == w0);
        CHECK(runs[1].first - runs[0].first == runs[2].first - runs[1].first);
        // 不贴边：左右各至少留 1 列透明。
        CHECK(runs[0].first >= 1);
        CHECK(runs[2].second <= s - 2);
        // 点够大，16px 上也看得见（至少 2 列宽）。
        CHECK(w0 + 1 >= 2);
    }
}

void TestMirrorSymmetry()
{
    g_case = "symmetry";
    for (int s : kSizes)
    {
        g_size = s;
        const auto px = loading_icon::RenderBgra(s, true);
        bool lr = true, tb = true;
        for (int y = 0; y < s; ++y)
            for (int x = 0; x < s; ++x)
            {
                if (AlphaAt(px, s, x, y) != AlphaAt(px, s, s - 1 - x, y))
                    lr = false;
                if (AlphaAt(px, s, x, y) != AlphaAt(px, s, x, s - 1 - y))
                    tb = false;
            }
        CHECK(lr);
        CHECK(tb);
    }
}

void TestVerticallyCentered()
{
    g_case = "vertical band";
    for (int s : kSizes)
    {
        g_size = s;
        const auto px = loading_icon::RenderBgra(s, false);
        // 顶行、底行全透明；图标中线那行有实心像素。
        bool edgeClear = true;
        for (int x = 0; x < s; ++x)
            if (AlphaAt(px, s, x, 0) || AlphaAt(px, s, x, s - 1))
                edgeClear = false;
        CHECK(edgeClear);
        bool solidOnMid = false;
        for (int x = 0; x < s; ++x)
            if (AlphaAt(px, s, x, s / 2) == 255)
                solidOnMid = true;
        CHECK(solidOnMid);
    }
}

void TestColors()
{
    g_case = "colors";
    for (int s : kSizes)
    {
        g_size = s;
        for (bool dark : { false, true })
        {
            const uint8_t want = dark ? 0xB4 : 0x6E;
            const auto px = loading_icon::RenderBgra(s, dark);
            bool ok = true, partial = false;
            for (size_t i = 0; i < px.size(); i += 4)
            {
                const uint8_t a = px[i + 3];
                if (a == 0)
                {
                    // 透明处 RGB 也清零，避免缩放时灰边外溢出别的颜色
                    if (px[i] || px[i + 1] || px[i + 2])
                        ok = false;
                    continue;
                }
                if (px[i] != want || px[i + 1] != want || px[i + 2] != want)
                    ok = false;
                if (a < 255)
                    partial = true;
            }
            CHECK(ok);
            CHECK(partial); // 有抗锯齿边
        }
    }
    g_size = 0;
    CHECK(loading_icon::GrayFor(false) == 0x6E);
    CHECK(loading_icon::GrayFor(true) == 0xB4);
}

void TestNotEmptyNotFull()
{
    g_case = "coverage";
    for (int s : kSizes)
    {
        g_size = s;
        const auto px = loading_icon::RenderBgra(s, false);
        long sum = 0;
        for (size_t i = 3; i < px.size(); i += 4)
            sum += px[i];
        const double cover = sum / (255.0 * s * s);
        // 三个 0.2·size 直径的圆 ≈ 3·π·0.01 ≈ 9.4%；给宽容区间，只防退化
        CHECK(cover > 0.05);
        CHECK(cover < 0.15);
    }
}

#ifdef DUMP
void Dump()
{
    for (int s : kSizes)
    {
        std::printf("size %d\n", s);
        const auto px = loading_icon::RenderBgra(s, false);
        for (int y = 0; y < s; ++y)
        {
            for (int x = 0; x < s; ++x)
            {
                const uint8_t a = AlphaAt(px, s, x, y);
                std::putchar(a == 0 ? '.' : a == 255 ? '#' : a >= 128 ? '+' : '-');
            }
            std::putchar('\n');
        }
    }
}
#endif

} // namespace

int main()
{
    TestBufferSize();
    TestThreeSeparateDots();
    TestMirrorSymmetry();
    TestVerticallyCentered();
    TestColors();
    TestNotEmptyNotFull();
#ifdef DUMP
    Dump();
#endif
    if (g_failures == 0)
    {
        std::printf("loading_icon_test: all passed\n");
        return 0;
    }
    std::printf("loading_icon_test: %d failure(s)\n", g_failures);
    return 1;
}
