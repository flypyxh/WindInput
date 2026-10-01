#include "Menu.h"

#include "ExtProtocol.h"

#include <cstdlib>

namespace windlinux {

int menuLevelOfKind(uint32_t kind)
{
    if (kind >= OVERLAY_KIND_MENU && kind < OVERLAY_KIND_MENU + OVERLAY_MENU_LEVELS) {
        return int(kind - OVERLAY_KIND_MENU);
    }
    return -1;
}

int32_t contextMenuTarget(int32_t hit)
{
    return hit >= 0 ? hit : -1;
}

namespace {

/// 毫秒数的环境变量：正整数（至多一天）才认，否则取默认。
uint32_t msFromEnv(const char* env, uint32_t fallback)
{
    if (!env || !*env) {
        return fallback;
    }
    char* end = nullptr;
    long v = std::strtol(env, &end, 10);
    if (*end != '\0' || v <= 0 || v > 24L * 3600 * 1000) {
        return fallback;
    }
    return uint32_t(v);
}

} // namespace

uint32_t menuIdleTimeoutMs(const char* env)
{
    return msFromEnv(env, kDefaultMenuIdleTimeoutMs);
}

uint32_t menuMaxGrabMs(const char* env)
{
    return msFromEnv(env, kDefaultMenuMaxGrabMs);
}

} // namespace windlinux
