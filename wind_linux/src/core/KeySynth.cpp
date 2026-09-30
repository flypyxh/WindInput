#include "KeySynth.h"

#include "KeyMap.h"

#include <algorithm>
#include <cctype>
#include <cstdlib>

namespace windlinux {

namespace {

// X11 keysym 取值（X11/keysymdef.h）。
namespace ks {
constexpr uint32_t BackSpace = 0xff08;
constexpr uint32_t Tab = 0xff09;
constexpr uint32_t Return = 0xff0d;
constexpr uint32_t Escape = 0xff1b;
constexpr uint32_t Home = 0xff50;
constexpr uint32_t Left = 0xff51;
constexpr uint32_t Up = 0xff52;
constexpr uint32_t Right = 0xff53;
constexpr uint32_t Down = 0xff54;
constexpr uint32_t Prior = 0xff55;
constexpr uint32_t Next = 0xff56;
constexpr uint32_t End = 0xff57;
constexpr uint32_t Insert = 0xff63;
constexpr uint32_t Menu = 0xff67;
constexpr uint32_t KP_0 = 0xffb0;
constexpr uint32_t F1 = 0xffbe;
constexpr uint32_t Shift_L = 0xffe1;
constexpr uint32_t Control_L = 0xffe3;
constexpr uint32_t Caps_Lock = 0xffe5;
constexpr uint32_t Alt_L = 0xffe9;
constexpr uint32_t Super_L = 0xffeb;
constexpr uint32_t Delete = 0xffff;
} // namespace ks

/// 功能键上限：X keysym 的 F1…F35 连号，Windows VK 只到 F24（0x87）。
constexpr uint32_t kMaxFunctionKey = 24;

/// Windows VK 取值（wind-keys/src/keymap.rs、winuser.h），只用于 `vk:NN` 反查。
namespace vk {
constexpr uint32_t K0 = 0x30;
constexpr uint32_t A = 0x41;
constexpr uint32_t NUMPAD0 = 0x60;
constexpr uint32_t F1 = 0x70;
} // namespace vk

struct NamedKey {
    const char* name;
    uint32_t keysym;
    uint32_t vk; // 0 = 这个别名不参与 vk 反查（同键的另一行已登记）
};

/// 键名表：字母、数字、功能键按区间算，不在这里。
constexpr NamedKey kNamedKeys[] = {
    // 控制 / 编辑键（Windows parse_key 的规范名与别名）
    {"enter", ks::Return, 0x0D},
    {"return", ks::Return, 0},
    {"tab", ks::Tab, 0x09},
    {"escape", ks::Escape, 0x1B},
    {"esc", ks::Escape, 0},
    {"space", ' ', 0x20},
    {"backspace", ks::BackSpace, 0x08},
    {"bksp", ks::BackSpace, 0},
    {"delete", ks::Delete, 0x2E},
    {"del", ks::Delete, 0},
    {"insert", ks::Insert, 0x2D},
    {"ins", ks::Insert, 0},
    {"home", ks::Home, 0x24},
    {"end", ks::End, 0x23},
    {"pageup", ks::Prior, 0x21},
    {"pgup", ks::Prior, 0},
    {"pagedown", ks::Next, 0x22},
    {"pgdn", ks::Next, 0},
    {"left", ks::Left, 0x25},
    {"up", ks::Up, 0x26},
    {"right", ks::Right, 0x27},
    {"down", ks::Down, 0x28},
    // macOS keyCodeMap 另有的
    {"capslock", ks::Caps_Lock, 0x14},
    {"shift", ks::Shift_L, 0x10},
    {"ctrl", ks::Control_L, 0x11},
    {"alt", ks::Alt_L, 0x12},
    {"win", ks::Super_L, 0x5B},
    // 符号键（wind-keys KEY_TABLE 的全部别名；macOS 用的 grave / equal 等也在其中）
    {"backtick", '`', 0xC0},
    {"grave", '`', 0},
    {"`", '`', 0},
    {"semicolon", ';', 0xBA},
    {";", ';', 0},
    {"quote", '\'', 0xDE},
    {"'", '\'', 0},
    {"comma", ',', 0xBC},
    {",", ',', 0},
    {"period", '.', 0xBE},
    {".", '.', 0},
    {"slash", '/', 0xBF},
    {"/", '/', 0},
    {"lbracket", '[', 0xDB},
    {"[", '[', 0},
    {"rbracket", ']', 0xDD},
    {"]", ']', 0},
    {"backslash", '\\', 0xDC},
    {"\\", '\\', 0},
    {"minus", '-', 0xBD},
    {"-", '-', 0},
    {"equal", '=', 0xBB},
    {"equals", '=', 0},
    {"=", '=', 0},
};

/// 只有 vk 写法才到得了的键（没有名字：Windows / macOS 两侧都没给它们起名）。
constexpr NamedKey kVkOnlyKeys[] = {
    {"", ks::Shift_L, 0xA0},   // VK_LSHIFT
    {"", ks::Control_L, 0xA2}, // VK_LCONTROL
    {"", ks::Alt_L, 0xA4},     // VK_LMENU
    {"", ks::Menu, 0x5D},      // VK_APPS
};

std::string lower(std::string s)
{
    for (char& ch : s) {
        ch = char(std::tolower(static_cast<unsigned char>(ch)));
    }
    return s;
}

/// Windows VK → keysym；不认识返回 0。
uint32_t vkToKeysym(uint32_t v)
{
    if (v >= vk::A && v < vk::A + 26) {
        return 'a' + (v - vk::A);
    }
    if (v >= vk::K0 && v < vk::K0 + 10) {
        return '0' + (v - vk::K0);
    }
    if (v >= vk::NUMPAD0 && v < vk::NUMPAD0 + 10) {
        return ks::KP_0 + (v - vk::NUMPAD0);
    }
    if (v >= vk::F1 && v < vk::F1 + kMaxFunctionKey) {
        return ks::F1 + (v - vk::F1);
    }
    for (const auto& k : kNamedKeys) {
        if (k.vk == v) {
            return k.keysym;
        }
    }
    for (const auto& k : kVkOnlyKeys) {
        if (k.vk == v) {
            return k.keysym;
        }
    }
    return 0;
}

/// `vk:` 之后的部分：十六进制，`0x` 可省（同 Windows `parse_key`）。
uint32_t parseVk(const std::string& rest)
{
    std::string h = rest;
    if (h.rfind("0x", 0) == 0) {
        h = h.substr(2);
    }
    if (h.empty() || h.size() > 4) {
        return 0;
    }
    char* end = nullptr;
    unsigned long v = std::strtoul(h.c_str(), &end, 16);
    return *end == '\0' ? vkToKeysym(uint32_t(v)) : 0;
}

} // namespace

uint32_t keyNameToKeysym(const std::string& rawName)
{
    const std::string name = lower(rawName);
    if (name.size() == 1) {
        char ch = name[0];
        if ((ch >= 'a' && ch <= 'z') || (ch >= '0' && ch <= '9')) {
            return uint32_t(ch);
        }
    }
    if (name.size() >= 2 && name.size() <= 3 && name[0] == 'f') {
        char* end = nullptr;
        long n = std::strtol(name.c_str() + 1, &end, 10);
        if (*end == '\0' && std::isdigit(static_cast<unsigned char>(name[1])) && n >= 1
            && n <= long(kMaxFunctionKey)) {
            return ks::F1 + uint32_t(n - 1);
        }
    }
    if (name.rfind("vk:", 0) == 0) {
        return parseVk(name.substr(3));
    }
    for (const auto& k : kNamedKeys) {
        if (name == k.name) {
            return k.keysym;
        }
    }
    return 0;
}

uint32_t modifierNameToKeysym(const std::string& rawName)
{
    const std::string n = lower(rawName);
    if (n == "ctrl" || n == "control") {
        return ks::Control_L;
    }
    if (n == "shift") {
        return ks::Shift_L;
    }
    if (n == "alt" || n == "menu" || n == "option") {
        return ks::Alt_L;
    }
    if (n == "win" || n == "super" || n == "meta" || n == "cmd" || n == "command") {
        return ks::Super_L;
    }
    return 0;
}

uint32_t modifierStateBit(uint32_t keysym)
{
    switch (keysym) {
    case ks::Shift_L: return xstate::Shift;
    case ks::Control_L: return xstate::Ctrl;
    case ks::Alt_L: return xstate::Alt;
    case ks::Super_L: return xstate::Super;
    default: return 0;
    }
}

std::optional<ResolvedCombo> resolveCombo(const KeyComboPayload& combo)
{
    ResolvedCombo out;
    out.key = keyNameToKeysym(combo.key);
    if (out.key == 0) {
        return std::nullopt;
    }
    for (const auto& m : combo.mods) {
        uint32_t sym = modifierNameToKeysym(m);
        if (sym == 0) {
            return std::nullopt;
        }
        // 去重；主键本身就是这个修饰键（`Shift+Shift`）时也不再单按一次。
        if (sym != out.key && std::find(out.mods.begin(), out.mods.end(), sym) == out.mods.end()) {
            out.mods.push_back(sym);
        }
    }
    return out;
}

std::vector<SynthKey> pressEvents(const ResolvedCombo& c, uint32_t base)
{
    std::vector<SynthKey> out;
    uint32_t st = base;
    for (uint32_t m : c.mods) {
        uint32_t bit = modifierStateBit(m);
        if (st & bit) {
            continue; // 已被 key.hold 按住：不再按一次（真键盘上同一个键按不了两下）
        }
        out.push_back({m, st, false});
        st |= bit;
    }
    out.push_back({c.key, st, false});
    return out;
}

std::vector<SynthKey> releaseEvents(const ResolvedCombo& c, uint32_t base)
{
    // 与 pressEvents 对称：先算出按下后的修饰态，主键抬起时 state 含全部（主键本身是修饰键时
    // 也含它自己——X 的抬起事件就是这样），再逆序抬修饰键，每抬一个去掉它的位。
    std::vector<uint32_t> pressed;
    uint32_t st = base;
    for (uint32_t m : c.mods) {
        uint32_t bit = modifierStateBit(m);
        if (!(st & bit)) {
            pressed.push_back(m);
            st |= bit;
        }
    }
    std::vector<SynthKey> out;
    out.push_back({c.key, st | modifierStateBit(c.key), true});
    for (auto it = pressed.rbegin(); it != pressed.rend(); ++it) {
        out.push_back({*it, st, true});
        st &= ~modifierStateBit(*it);
    }
    return out;
}

std::vector<SynthKey> tapEvents(const ResolvedCombo& c, uint32_t base)
{
    std::vector<SynthKey> out = pressEvents(c, base);
    std::vector<SynthKey> up = releaseEvents(c, base);
    out.insert(out.end(), up.begin(), up.end());
    return out;
}

// ── 限流 ──────────────────────────────────────────────────────────────

bool SynthRateLimiter::allow(size_t events, uint64_t nowMs)
{
    auto stale = std::find_if(batches_.begin(), batches_.end(), [&](const Batch& b) {
        return nowMs - b.atMs < kSynthWindowMs;
    });
    for (auto it = batches_.begin(); it != stale; ++it) {
        inWindow_ -= it->events;
    }
    batches_.erase(batches_.begin(), stale);
    if (inWindow_ + events > kMaxSynthEventsPerWindow) {
        return false;
    }
    batches_.push_back({nowMs, events});
    inWindow_ += events;
    return true;
}

// ── key.hold ──────────────────────────────────────────────────────────

uint32_t keyHoldTimeoutMs(const char* env)
{
    if (!env || !*env) {
        return kDefaultKeyHoldTimeoutMs;
    }
    char* end = nullptr;
    long v = std::strtol(env, &end, 10);
    if (*end != '\0' || v <= 0 || v > 3600L * 1000) {
        return kDefaultKeyHoldTimeoutMs;
    }
    return uint32_t(v);
}

KeyHoldTracker::HoldResult KeyHoldTracker::hold(const ResolvedCombo& c, uint64_t nowMs,
                                                uint32_t timeoutMs, std::vector<SynthKey>& out)
{
    out.clear();
    for (const auto& h : held_) {
        if (h.combo == c) {
            return HoldResult::AlreadyHeld;
        }
    }
    if (held_.size() >= kMaxHeldCombos) {
        return HoldResult::Full;
    }
    out = pressEvents(c, heldStates());
    held_.push_back({c, nowMs + timeoutMs});
    return HoldResult::Pressed;
}

std::vector<SynthKey> KeyHoldTracker::releaseAt(size_t i)
{
    ResolvedCombo c = held_[i].combo;
    held_.erase(held_.begin() + long(i));
    return releaseEvents(c, heldStates());
}

std::vector<SynthKey> KeyHoldTracker::release(const ResolvedCombo& c)
{
    for (size_t i = 0; i < held_.size(); ++i) {
        if (held_[i].combo == c) {
            return releaseAt(i);
        }
    }
    return {};
}

std::vector<SynthKey> KeyHoldTracker::releaseAll()
{
    std::vector<SynthKey> out;
    while (!held_.empty()) {
        std::vector<SynthKey> up = releaseAt(held_.size() - 1);
        out.insert(out.end(), up.begin(), up.end());
    }
    return out;
}

std::vector<SynthKey> KeyHoldTracker::releaseExpired(uint64_t nowMs)
{
    std::vector<SynthKey> out;
    for (size_t i = held_.size(); i-- > 0;) {
        if (held_[i].deadlineMs <= nowMs) {
            std::vector<SynthKey> up = releaseAt(i);
            out.insert(out.end(), up.begin(), up.end());
        }
    }
    return out;
}

std::optional<uint64_t> KeyHoldTracker::nextDeadline() const
{
    std::optional<uint64_t> best;
    for (const auto& h : held_) {
        if (!best || h.deadlineMs < *best) {
            best = h.deadlineMs;
        }
    }
    return best;
}

uint32_t KeyHoldTracker::heldStates() const
{
    uint32_t st = 0;
    for (const auto& h : held_) {
        for (uint32_t m : h.combo.mods) {
            st |= modifierStateBit(m);
        }
        st |= modifierStateBit(h.combo.key);
    }
    return st;
}

} // namespace windlinux
