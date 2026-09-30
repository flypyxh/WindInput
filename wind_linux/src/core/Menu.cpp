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

uint32_t menuIdleTimeoutMs(const char* env)
{
    if (!env || !*env) {
        return kDefaultMenuIdleTimeoutMs;
    }
    char* end = nullptr;
    long v = std::strtol(env, &end, 10);
    if (*end != '\0' || v <= 0 || v > 24L * 3600 * 1000) {
        return kDefaultMenuIdleTimeoutMs;
    }
    return uint32_t(v);
}

} // namespace windlinux
