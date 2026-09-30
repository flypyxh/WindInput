// 极简测试骨架：与 wind_tsf/tests 同风格（无第三方框架，g++ 直接编），每个测试文件一个
// 可执行，失败数作为退出码。
#pragma once

#include <cstdio>
#include <string>

namespace testharness {
inline int g_failures = 0;
inline const char* g_case = "";
} // namespace testharness

#define CHECK(expr)                                                                      \
    do {                                                                                 \
        if (!(expr)) {                                                                   \
            std::printf("  FAIL  %s:%d  [%s]  %s\n", __FILE__, __LINE__,                 \
                        testharness::g_case, #expr);                                     \
            testharness::g_failures++;                                                   \
        }                                                                                \
    } while (0)

#define CHECK_EQ(a, b)                                                                   \
    do {                                                                                 \
        auto _va = (a);                                                                  \
        auto _vb = (b);                                                                  \
        if (!(_va == _vb)) {                                                             \
            std::printf("  FAIL  %s:%d  [%s]  %s == %s\n", __FILE__, __LINE__,           \
                        testharness::g_case, #a, #b);                                    \
            testharness::g_failures++;                                                   \
        }                                                                                \
    } while (0)

#define CASE(name)                                                                       \
    testharness::g_case = name;                                                          \
    std::printf("- %s\n", name);

#define TEST_MAIN_END()                                                                  \
    if (testharness::g_failures == 0) {                                                  \
        std::printf("ALL PASS\n");                                                       \
    } else {                                                                             \
        std::printf("%d FAILURE(S)\n", testharness::g_failures);                         \
    }                                                                                    \
    return testharness::g_failures == 0 ? 0 : 1;
