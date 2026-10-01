// ImmCandidateFormPolicy 判据测试
//
// 跑法（纯 C++17、不含 Win32 头，本机不需要 MSVC）：
//   g++ -std=c++17 -I../include -o icf_test imm_candidate_form_policy_test.cpp && ./icf_test
// 或经 CMake：
//   cmake -S wind_tsf -B build/tsf-tests -DWIND_TSF_TESTS=ON
//   cmake --build build/tsf-tests && ctest --test-dir build/tsf-tests
//
// 变异检验已做（2026-09-30），逐条命中：去掉 hostSaysNoCaret / gotForm / 样式判据 / hasScreenExt、四条边界比较
// 任一换成 true、三种样式任一从白名单删掉——都有用例变红。改测试时请保住这个性质。
//
// ⚠️ 覆盖边界：这里测的只是**判据**。IMC 怎么取、ClientToScreen 用哪个窗口、高度怎么合成、
// 组合起点怎么改写，全在 CaretEditSession.cpp 里，这里一行也覆盖不到。

#include "ImmCandidateFormPolicy.h"

#include <cstdio>

namespace
{

using namespace wind::caret;

int g_failures = 0;
const char* g_case = "";

#define CHECK(expr)                                                                      \
    do                                                                                   \
    {                                                                                    \
        if (!(expr))                                                                     \
        {                                                                                \
            std::printf("  FAIL  %s:%d  [%s]  %s\n", __FILE__, __LINE__, g_case, #expr); \
            g_failures++;                                                                \
        }                                                                                \
    } while (0)

#define CASE(name)                   \
    do                               \
    {                                \
        g_case = name;               \
        std::printf("  %s\n", name); \
    } while (0)

// Win32 RECT 的最小替身：本头文件按成员名取值（模板），不依赖 windows.h。
struct Rect
{
    long left;
    long top;
    long right;
    long bottom;
};

// 医疗 HIS/LIS 的 Java 客户端最大化时的 GetScreenExt 实测值（2026-09-30）。
constexpr Rect kMaximized{0, 23, 1920, 1040};

void TestStyle()
{
    CASE("IsCandidateFormPositioned：以 ptCurrentPos 为落点的三种写法都认");
    CHECK(IsCandidateFormPositioned(kCfsPoint));        // AWT 的写法
    CHECK(IsCandidateFormPositioned(kCfsCandidatePos)); // 标准写法（SDL 等）
    CHECK(IsCandidateFormPositioned(kCfsExclude));      // 另带避让区

    CASE("IsCandidateFormPositioned：宿主没表态 / 别的样式不认");
    CHECK(!IsCandidateFormPositioned(kCfsDefault));
    CHECK(!IsCandidateFormPositioned(0x0001)); // CFS_RECT：组合窗用的，不是候选落点
    CHECK(!IsCandidateFormPositioned(0x0020)); // CFS_FORCE_POSITION 单独出现
}

void TestAccept()
{
    CASE("AcceptCandidateFormPoint：宿主设了、样式对、落在显示区内 ⇒ 采信");
    CHECK(AcceptCandidateFormPoint(true, true, kCfsPoint, 356, 166, true, kMaximized));
    // 边界含：落点正压在显示区边上仍算内。
    CHECK(AcceptCandidateFormPoint(true, true, kCfsCandidatePos, 0, 23, true, kMaximized));
    CHECK(AcceptCandidateFormPoint(true, true, kCfsCandidatePos, 1920, 1040, true, kMaximized));

    CASE("AcceptCandidateFormPoint：宿主经 TSF 给了真矩形（或只是还没排完版）⇒ 不采信");
    // 证据只覆盖「三者全同」那一种形态；其它退化帧采信残留的 CANDIDATEFORM 只会多一次闪跳。
    CHECK(!AcceptCandidateFormPoint(false, true, kCfsPoint, 356, 166, true, kMaximized));

    CASE("AcceptCandidateFormPoint：ImmGetCandidateWindow 失败 ⇒ 不采信");
    CHECK(!AcceptCandidateFormPoint(true, false, kCfsPoint, 356, 166, true, kMaximized));

    CASE("AcceptCandidateFormPoint：CFS_DEFAULT ⇒ ptCurrentPos 无意义，不采信");
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsDefault, 356, 166, true, kMaximized));

    CASE("AcceptCandidateFormPoint：落在显示区外 ⇒ 换算窗口不对或陈旧值，不采信");
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, -1, 166, true, kMaximized));
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, 356, 22, true, kMaximized));
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, 1921, 166, true, kMaximized));
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, 356, 1041, true, kMaximized));
    // 小窗口（登录框）实测显示区：主窗口留下的陈旧落点在它之外。
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, 356, 166, true, Rect{760, 480, 1160, 560}));

    CASE("AcceptCandidateFormPoint：拿不到显示区 ⇒ 失败关闭");
    // 与二级降级同一个理由：新增来源，跳过它无损，放行可能送出自信的错坐标。
    CHECK(!AcceptCandidateFormPoint(true, true, kCfsPoint, 356, 166, false, kMaximized));
}

} // namespace

int main()
{
    std::printf("ImmCandidateFormPolicy tests\n");
    TestStyle();
    TestAccept();

    if (g_failures == 0)
    {
        std::printf("OK\n");
        return 0;
    }
    std::printf("%d FAILURE(S)\n", g_failures);
    return 1;
}
