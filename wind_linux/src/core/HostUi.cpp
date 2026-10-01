#include "HostUi.h"

#include "Bridge.h"

#include "KeyMap.h"
#include "Protocol.h"

#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <utility>

#include <sys/stat.h>

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

ModeIndicator::Variant ModeIndicator::variant() const
{
    if (caps_) {
        return Variant::Caps;
    }
    return chinese() ? Variant::Chinese : Variant::English;
}

std::string ModeIndicator::dynamicIconName(Variant v, const std::string& label)
{
    return dynamicIconName(v, label, variantSuffix() == "Dev");
}

std::string ModeIndicator::dynamicIconName(Variant v, const std::string& label, bool dev)
{
    static const char* kHex = "0123456789abcdef";
    std::string s = dev ? "windinput-lbl-dev-" : "windinput-lbl-";
    s += v == Variant::Chinese ? "zh" : v == Variant::English ? "en" : "caps";
    s += '-';
    if (label.size() > 32) { // tray_icon::MAX_HEX_LABEL_BYTES
        uint64_t h = 0xcbf29ce484222325ull; // FNV-1a 64
        for (unsigned char c : label) {
            h ^= c;
            h *= 0x100000001b3ull;
        }
        s += 'h';
        for (int shift = 60; shift >= 0; shift -= 4) {
            s += kHex[(h >> shift) & 0xF];
        }
        return s;
    }
    for (unsigned char c : label) {
        s += kHex[c >> 4];
        s += kHex[c & 0xF];
    }
    return s;
}

std::string ModeIndicator::seedIconName(Variant v)
{
    switch (v) {
    case Variant::Chinese: return "windinput-zh";
    case Variant::English: return "windinput-en";
    case Variant::Caps: return "windinput-caps";
    }
    return "windinput-zh";
}

std::string ModeIndicator::iconName() const
{
    if (!known()) {
        return {};
    }
    const Variant v = variant();
    const std::string text = label();
    if (!iconRoot_.empty() && !text.empty()) {
        std::string name = dynamicIconName(v, text);
        // 服务端按尺寸从小到大写、64 最后（`tray_icon::LAST_SIZE`），它在 = 这一组写完了。
        struct stat st {};
        if (::stat((iconRoot_ + "/64x64/apps/" + name + ".png").c_str(), &st) == 0) {
            return name;
        }
    }
    return seedIconName(v);
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

std::string userHicolorDir()
{
    const char* data = std::getenv("XDG_DATA_HOME");
    if (data && data[0] == '/') {
        return std::string(data) + "/icons/hicolor";
    }
    const char* home = std::getenv("HOME");
    if (home && home[0]) {
        return std::string(home) + "/.local/share/icons/hicolor";
    }
    return {};
}

bool ensureIconDirs(const std::string& root)
{
    if (root.empty()) {
        return false;
    }
    std::error_code ec;
    for (const char* n : {"16x16", "22x22", "24x24", "32x32", "48x48", "64x64"}) {
        std::filesystem::create_directories(std::filesystem::path(root) / n / "apps", ec);
        if (ec) {
            return false;
        }
    }
    return true;
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
