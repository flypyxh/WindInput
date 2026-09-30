#include "HostUi.h"

#include "KeyMap.h"
#include "Protocol.h"

#include <cstring>
#include <utility>

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

std::optional<ModeStatus> modeStatusOf(const Frame& frame)
{
    switch (frame.cmd) {
    case CMD_MODE_PUSH: {
        // 只有 flags u32（中英 / 全角 / 标点），没有大写锁定位与标签。
        auto flags = leadingU32(frame.payload);
        if (!flags) {
            return std::nullopt;
        }
        ModeStatus m;
        m.chinese = (*flags & STATUS_CHINESE_MODE) != 0;
        return m;
    }
    case CMD_STATUS_UPDATE:
    case CMD_STATE_PUSH:
    case CMD_ACTIVATION_STATUS_PUSH: {
        // 状态头 flags + keyDown 数 + keyUp 数（各 u32），随后热键哈希，尾部到帧末是标签
        // （`encode_status_update_ex`：标签没有长度前缀）。
        auto flags = leadingU32(frame.payload);
        if (!flags) {
            return std::nullopt;
        }
        ModeStatus m;
        m.chinese = (*flags & STATUS_CHINESE_MODE) != 0;
        const Bytes& p = frame.payload;
        if (p.size() >= 12) {
            uint32_t down = 0;
            uint32_t up = 0;
            std::memcpy(&down, p.data() + 4, 4);
            std::memcpy(&up, p.data() + 8, 4);
            uint64_t labelAt = 12 + (uint64_t(down) + up) * 4;
            if (labelAt <= p.size()) {
                m.capsLock = (*flags & STATUS_CAPS_LOCK) != 0;
                m.label = std::string(p.begin() + labelAt, p.end());
            }
        }
        return m;
    }
    case CMD_COMMIT_TEXT:
        if (auto p = decodeCommitText(frame.payload); p && (p->flags & COMMIT_FLAG_MODE_CHANGED)) {
            ModeStatus m;
            m.chinese = (p->flags & COMMIT_FLAG_CHINESE_MODE) != 0;
            return m;
        }
        return std::nullopt;
    default:
        return std::nullopt;
    }
}

bool ModeIndicator::update(const Frame& frame)
{
    auto st = modeStatusOf(frame);
    if (!st) {
        return false;
    }
    const std::string icon = iconName();
    const std::string text = label();
    chinese_ = st->chinese;
    if (st->capsLock) {
        caps_ = *st->capsLock;
    }
    if (st->label && !st->label->empty()) {
        // 按帧自己报的状态归档（不是按本端镜像）：标签与状态位出自同一帧，天然一致。
        if (st->capsLock.value_or(false)) {
            capsLabel_ = *st->label;
        } else if (st->chinese) {
            schemaLabel_ = *st->label;
        } else {
            englishLabel_ = *st->label;
        }
    }
    return iconName() != icon || label() != text;
}

bool ModeIndicator::noteCapsLock(bool on)
{
    if (caps_ == on) {
        return false;
    }
    caps_ = on;
    return known();
}

std::string ModeIndicator::iconForLabel(const std::string& schemaLabel)
{
    // 与 wind-ui/examples/gen_tray_icons.rs 的 ICONS 表一一对应；host_ui_test 核对文件在不在。
    // 内置方案的 `[schema] icon_label`（data/schemas/*.schema.toml）。
    static const std::pair<const char*, const char*> kTable[] = {
        {"拼", "windinput-zh-pin"},
        {"五", "windinput-zh-wu"},
        {"笔", "windinput-zh-bi"},
        {"双", "windinput-zh-shuang"},
        {"英", "windinput-zh-ying"},
    };
    for (const auto& [text, icon] : kTable) {
        if (schemaLabel == text) {
            return icon;
        }
    }
    // 「中」（五笔拼音、方案未配标签）与一切自定义标签：托盘图标是预生成的，画不出任意字。
    return "windinput-zh";
}

std::string ModeIndicator::iconName() const
{
    if (!known()) {
        return {};
    }
    if (caps_) {
        return "windinput-caps";
    }
    return chinese() ? iconForLabel(schemaLabel_) : "windinput-en";
}

std::string ModeIndicator::label() const
{
    if (!known()) {
        return {};
    }
    if (caps_) {
        return capsLabel_;
    }
    return chinese() ? schemaLabel_ : englishLabel_;
}

std::string ModeIndicator::subModeName() const
{
    if (!known()) {
        return {};
    }
    if (caps_) {
        return "大写锁定";
    }
    return chinese() ? "中文" : "英文";
}

std::optional<bool> CapsLockTracker::onKey(uint32_t keysym, uint32_t states, bool release)
{
    constexpr uint32_t kCapsLock = 0xffe5; // XK_Caps_Lock
    const bool locked = (states & xstate::CapsLock) != 0;
    if (keysym != kCapsLock) {
        return locked;
    }
    if (!release) {
        // 自动重复的按下不覆盖：第一次按下时的状态才是「之前」。
        if (!pressLocked_) {
            pressLocked_ = locked;
        }
        return std::nullopt;
    }
    if (!pressLocked_) {
        return std::nullopt; // 按下落在别处（换焦点前按下）：判不了，等下一个键校准
    }
    bool now = !*pressLocked_;
    pressLocked_.reset();
    return now;
}

std::string clientPreeditText(const std::string& composition)
{
    return composition == kCompositionPlaceholder ? std::string() : composition;
}

} // namespace windlinux
