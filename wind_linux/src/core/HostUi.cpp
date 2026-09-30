#include "HostUi.h"

#include "Protocol.h"

#include <cstring>

namespace windlinux {

namespace {

// 服务端 `wind_bridge::handler::COMPOSITION_PLACEHOLDER`。
constexpr const char* kCompositionPlaceholder = " ";

std::optional<uint32_t> leadingU32(const Bytes& p)
{
    if (p.size() < 4) {
        return std::nullopt;
    }
    uint32_t v = 0;
    std::memcpy(&v, p.data(), 4);
    return v;
}

} // namespace

std::optional<bool> chineseModeOf(const Frame& frame)
{
    switch (frame.cmd) {
    case CMD_MODE_PUSH:
    case CMD_STATUS_UPDATE:
    case CMD_STATE_PUSH:
    case CMD_ACTIVATION_STATUS_PUSH:
        // 四者载荷都以状态 flags u32 开头（STATUS_* 位）。
        if (auto flags = leadingU32(frame.payload)) {
            return (*flags & STATUS_CHINESE_MODE) != 0;
        }
        return std::nullopt;
    case CMD_COMMIT_TEXT:
        if (auto p = decodeCommitText(frame.payload); p && (p->flags & COMMIT_FLAG_MODE_CHANGED)) {
            return (p->flags & COMMIT_FLAG_CHINESE_MODE) != 0;
        }
        return std::nullopt;
    default:
        return std::nullopt;
    }
}

bool ModeIndicator::update(const Frame& frame)
{
    auto mode = chineseModeOf(frame);
    if (!mode || chinese_ == mode) {
        return false;
    }
    chinese_ = mode;
    return true;
}

std::string ModeIndicator::iconName() const
{
    if (!known()) {
        return {};
    }
    return chinese() ? "windinput-zh" : "windinput-en";
}

std::string ModeIndicator::label() const
{
    if (!known()) {
        return {};
    }
    return chinese() ? "中" : "英";
}

std::string ModeIndicator::subModeName() const
{
    if (!known()) {
        return {};
    }
    return chinese() ? "中文" : "英文";
}

std::string clientPreeditText(const std::string& composition)
{
    return composition == kCompositionPlaceholder ? std::string() : composition;
}

} // namespace windlinux
