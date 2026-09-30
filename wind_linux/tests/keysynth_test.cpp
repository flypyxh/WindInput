// 命令直通车按键合成的纯逻辑：键名 → keysym 的覆盖（对照 Windows / macOS 两侧源码，防漂移）、
// 组合的按下 / 抬起顺序与 state 位、key.hold 的记账与卡键兜底、seq / 频率限流、帧解码。

#include "Codec.h"
#include "KeyMap.h"
#include "KeySynth.h"
#include "TestHarness.h"

#include <fstream>
#include <regex>
#include <set>
#include <sstream>

#ifndef WIND_REPO_DIR
#error "编译时需 -DWIND_REPO_DIR=\"<仓库根>\""
#endif

using namespace windlinux;

namespace {

constexpr uint32_t XK_BackSpace = 0xff08;
constexpr uint32_t XK_Home = 0xff50;
constexpr uint32_t XK_End = 0xff57;
constexpr uint32_t XK_Shift_L = 0xffe1;
constexpr uint32_t XK_Control_L = 0xffe3;
constexpr uint32_t XK_Alt_L = 0xffe9;
constexpr uint32_t XK_Super_L = 0xffeb;
constexpr uint32_t XK_F1 = 0xffbe;

std::string readFile(const std::string& rel)
{
    std::ifstream f(std::string(WIND_REPO_DIR) + "/" + rel);
    std::stringstream ss;
    ss << f.rdbuf();
    return ss.str();
}

/// `text` 里 `begin` 与其后第一个 `end` 之间的一段；找不到返回空串。
std::string between(const std::string& text, const std::string& begin, const std::string& end)
{
    size_t b = text.find(begin);
    if (b == std::string::npos) {
        return {};
    }
    b += begin.size();
    size_t e = text.find(end, b);
    return e == std::string::npos ? std::string() : text.substr(b, e - b);
}

/// 一段源码里全部双引号字符串字面量（处理 `\\` / `\"` 转义）。
std::vector<std::string> quoted(const std::string& text)
{
    std::vector<std::string> out;
    std::regex re(R"re("((?:[^"\\]|\\.)*)")re");
    for (auto it = std::sregex_iterator(text.begin(), text.end(), re); it != std::sregex_iterator();
         ++it) {
        std::string raw = (*it)[1];
        std::string s;
        for (size_t i = 0; i < raw.size(); ++i) {
            if (raw[i] == '\\' && i + 1 < raw.size()) {
                ++i;
            }
            s += raw[i];
        }
        out.push_back(s);
    }
    return out;
}

ResolvedCombo combo(const std::string& key, std::vector<std::string> mods = {})
{
    auto c = resolveCombo(KeyComboPayload{key, std::move(mods)});
    CHECK(c.has_value());
    return c.value_or(ResolvedCombo{});
}

Bytes lenStr(const std::string& s)
{
    Bytes b;
    uint32_t n = uint32_t(s.size());
    for (int i = 0; i < 4; ++i) {
        b.push_back(uint8_t(n >> (8 * i)));
    }
    b.insert(b.end(), s.begin(), s.end());
    return b;
}

Bytes u32le(uint32_t n)
{
    return Bytes{uint8_t(n), uint8_t(n >> 8), uint8_t(n >> 16), uint8_t(n >> 24)};
}

/// 按 Rust `codec.rs::push_key_combo` 的布局手写一个 combo。
Bytes comboBytes(const std::string& key, const std::vector<std::string>& mods)
{
    Bytes b = lenStr(key);
    Bytes n = u32le(uint32_t(mods.size()));
    b.insert(b.end(), n.begin(), n.end());
    for (const auto& m : mods) {
        Bytes s = lenStr(m);
        b.insert(b.end(), s.begin(), s.end());
    }
    return b;
}

} // namespace

int main()
{
    CASE("macOS KeySynthesizer.keyCodeMap 的每个键名在 Linux 都有映射");
    {
        std::string swift = readFile("wind_macos/Sources/WindInputApp/UI/KeySynthesizer.swift");
        std::string map = between(swift, "keyCodeMap: [String: CGKeyCode] = [", "\n    ]");
        std::regex re(R"re("([^"]+)"\s*:)re");
        int n = 0;
        for (auto it = std::sregex_iterator(map.begin(), map.end(), re);
             it != std::sregex_iterator(); ++it, ++n) {
            std::string name = (*it)[1];
            if (keyNameToKeysym(name) == 0) {
                std::printf("  缺映射（macOS 有）：%s\n", name.c_str());
                CHECK(false);
            }
        }
        std::printf("  macOS 键名 %d 个\n", n);
        CHECK(n >= 80); // 实测 87：读不到文件 / 表头改名时别静默通过
    }

    CASE("Windows key_inject::parse_key 的具名键与 KEY_TABLE 符号键在 Linux 都有映射");
    {
        std::string rs = readFile("wind_input/crates/wind-keys/src/key_inject.rs");
        std::string arms = between(rs, "let vk = match low.as_str() {", "_ =>");
        std::vector<std::string> names = quoted(arms);
        std::string km = readFile("wind_input/crates/wind-keys/src/keymap.rs");
        std::string table = between(km, "const KEY_TABLE: &[KeyDef] = &[", "\n];");
        std::istringstream lines(table);
        for (std::string line; std::getline(lines, line);) {
            if (line.find("names:") != std::string::npos) {
                for (auto& s : quoted(line)) {
                    names.push_back(s);
                }
            }
        }
        for (const auto& name : names) {
            if (keyNameToKeysym(name) == 0) {
                std::printf("  缺映射（Windows 有）：%s\n", name.c_str());
                CHECK(false);
            }
        }
        std::printf("  Windows 具名键 + 符号键 %zu 个\n", names.size());
        CHECK(names.size() >= 40); // 实测 46
        // KEY_TABLE 的 `]` / `[` 别名在正则里最容易漏：点名核一次。
        CHECK(std::find(names.begin(), names.end(), "]") != names.end());
        CHECK(std::find(names.begin(), names.end(), "\\") != names.end());
    }

    CASE("字母 / 数字 / F1…F24：基础层 keysym，大小写不敏感");
    for (char ch = 'a'; ch <= 'z'; ++ch) {
        CHECK_EQ(keyNameToKeysym(std::string(1, ch)), uint32_t(ch));
        CHECK_EQ(keyNameToKeysym(std::string(1, char(ch - 32))), uint32_t(ch)); // 'C' 同 'c'
    }
    for (char ch = '0'; ch <= '9'; ++ch) {
        CHECK_EQ(keyNameToKeysym(std::string(1, ch)), uint32_t(ch));
    }
    for (int i = 1; i <= 24; ++i) {
        CHECK_EQ(keyNameToKeysym("f" + std::to_string(i)), XK_F1 + uint32_t(i - 1));
    }
    CHECK_EQ(keyNameToKeysym("F5"), XK_F1 + 4);
    CHECK_EQ(keyNameToKeysym("f0"), 0u);
    CHECK_EQ(keyNameToKeysym("f25"), 0u);
    CHECK_EQ(keyNameToKeysym("f01"), XK_F1); // 同 Windows：`"01".parse::<u32>()` 得 1

    CASE("具名键的 keysym 值");
    CHECK_EQ(keyNameToKeysym("Home"), XK_Home);
    CHECK_EQ(keyNameToKeysym("end"), XK_End);
    CHECK_EQ(keyNameToKeysym("backspace"), XK_BackSpace);
    CHECK_EQ(keyNameToKeysym("bksp"), XK_BackSpace);
    CHECK_EQ(keyNameToKeysym("enter"), 0xff0du);
    CHECK_EQ(keyNameToKeysym("return"), 0xff0du);
    CHECK_EQ(keyNameToKeysym("esc"), 0xff1bu);
    CHECK_EQ(keyNameToKeysym("delete"), 0xffffu);
    CHECK_EQ(keyNameToKeysym("del"), 0xffffu);
    CHECK_EQ(keyNameToKeysym("pageup"), 0xff55u);
    CHECK_EQ(keyNameToKeysym("pgdn"), 0xff56u);
    CHECK_EQ(keyNameToKeysym("space"), 0x20u);
    CHECK_EQ(keyNameToKeysym("capslock"), 0xffe5u);
    CHECK_EQ(keyNameToKeysym("shift"), XK_Shift_L);
    CHECK_EQ(keyNameToKeysym("win"), XK_Super_L);
    CHECK_EQ(keyNameToKeysym("grave"), uint32_t('`'));
    CHECK_EQ(keyNameToKeysym("equals"), uint32_t('='));
    CHECK_EQ(keyNameToKeysym("rbracket"), uint32_t(']'));
    CHECK_EQ(keyNameToKeysym("\\"), uint32_t('\\'));

    CASE("vk:NN 按 Windows VK 十六进制反查（0x 可省，同 Windows parse_key）");
    CHECK_EQ(keyNameToKeysym("vk:0x41"), uint32_t('a'));
    CHECK_EQ(keyNameToKeysym("vk:41"), uint32_t('a'));
    CHECK_EQ(keyNameToKeysym("VK:0x24"), XK_Home);
    CHECK_EQ(keyNameToKeysym("vk:0x14"), 0xffe5u); // 软键盘的 Caps
    CHECK_EQ(keyNameToKeysym("vk:0x5D"), 0xff67u); // Apps / Menu 键，只有 vk 写法
    CHECK_EQ(keyNameToKeysym("vk:0x70"), XK_F1);
    CHECK_EQ(keyNameToKeysym("vk:0x87"), XK_F1 + 23);
    CHECK_EQ(keyNameToKeysym("vk:0x60"), 0xffb0u); // 小键盘 0
    CHECK_EQ(keyNameToKeysym("vk:0xBA"), uint32_t(';'));
    CHECK_EQ(keyNameToKeysym("vk:0xA0"), XK_Shift_L);
    CHECK_EQ(keyNameToKeysym("vk:"), 0u);
    CHECK_EQ(keyNameToKeysym("vk:zz"), 0u);
    CHECK_EQ(keyNameToKeysym("vk:0xFF"), 0u);
    CHECK_EQ(keyNameToKeysym("vk:0x12345"), 0u);

    CASE("未知键名 / 空串 → 0");
    CHECK_EQ(keyNameToKeysym(""), 0u);
    CHECK_EQ(keyNameToKeysym("hyper"), 0u);
    CHECK_EQ(keyNameToKeysym("ab"), 0u);
    CHECK_EQ(keyNameToKeysym("menu"), 0u); // menu 是 Alt 的修饰名，不是主键名（两侧都不收）

    CASE("修饰名：四个规范名 + split_combo 认的别名");
    CHECK_EQ(modifierNameToKeysym("ctrl"), XK_Control_L);
    CHECK_EQ(modifierNameToKeysym("Control"), XK_Control_L);
    CHECK_EQ(modifierNameToKeysym("shift"), XK_Shift_L);
    CHECK_EQ(modifierNameToKeysym("alt"), XK_Alt_L);
    CHECK_EQ(modifierNameToKeysym("menu"), XK_Alt_L);
    CHECK_EQ(modifierNameToKeysym("option"), XK_Alt_L);
    CHECK_EQ(modifierNameToKeysym("win"), XK_Super_L);
    CHECK_EQ(modifierNameToKeysym("super"), XK_Super_L);
    CHECK_EQ(modifierNameToKeysym("cmd"), XK_Super_L);
    CHECK_EQ(modifierNameToKeysym("hyper"), 0u);
    CHECK_EQ(modifierStateBit(XK_Control_L), xstate::Ctrl);
    CHECK_EQ(modifierStateBit(XK_Super_L), xstate::Super);
    CHECK_EQ(modifierStateBit(uint32_t('a')), 0u);

    CASE("resolveCombo：未知主键 / 未知修饰名整条拒绝（不退化成裸键）");
    CHECK(!resolveCombo(KeyComboPayload{"nosuchkey", {}}).has_value());
    CHECK(!resolveCombo(KeyComboPayload{"c", {"hyper"}}).has_value());
    CHECK(!resolveCombo(KeyComboPayload{"", {}}).has_value());
    {
        ResolvedCombo c = combo("c", {"ctrl", "shift", "ctrl"});
        CHECK(c.mods == (std::vector<uint32_t>{XK_Control_L, XK_Shift_L})); // 去重、保序
        ResolvedCombo s = combo("shift", {"shift"});
        CHECK(s.mods.empty()); // 主键就是它，不再单按一次
    }

    CASE("tap Ctrl+C：Ctrl 按下(0) → c 按下(Ctrl) → c 抬起(Ctrl) → Ctrl 抬起(Ctrl)");
    {
        auto ev = tapEvents(combo("c", {"ctrl"}));
        std::vector<SynthKey> want = {
            {XK_Control_L, 0, false},
            {'c', xstate::Ctrl, false},
            {'c', xstate::Ctrl, true},
            {XK_Control_L, xstate::Ctrl, true},
        };
        CHECK(ev == want);
    }

    CASE("tap Ctrl+Shift+End：修饰键按给出顺序按下、逆序抬起，state 逐步累加 / 递减");
    {
        auto ev = tapEvents(combo("end", {"ctrl", "shift"}));
        const uint32_t cs = xstate::Ctrl | xstate::Shift;
        std::vector<SynthKey> want = {
            {XK_Control_L, 0, false},
            {XK_Shift_L, xstate::Ctrl, false},
            {XK_End, cs, false},
            {XK_End, cs, true},
            {XK_Shift_L, cs, true},
            {XK_Control_L, xstate::Ctrl, true},
        };
        CHECK(ev == want);
    }

    CASE("codl 的 seq：Home / Shift+End / Backspace 逐个 tap");
    {
        std::vector<SynthKey> ev;
        for (auto c : {combo("home"), combo("end", {"shift"}), combo("backspace")}) {
            auto t = tapEvents(c);
            ev.insert(ev.end(), t.begin(), t.end());
        }
        std::vector<SynthKey> want = {
            {XK_Home, 0, false},        {XK_Home, 0, true},
            {XK_Shift_L, 0, false},     {XK_End, xstate::Shift, false},
            {XK_End, xstate::Shift, true}, {XK_Shift_L, xstate::Shift, true},
            {XK_BackSpace, 0, false},   {XK_BackSpace, 0, true},
        };
        CHECK(ev == want);
    }

    CASE("修饰键本身作主键：Shift 抬起时 state 含 Shift（X 的真实形态）");
    {
        auto ev = tapEvents(combo("shift"));
        std::vector<SynthKey> want = {{XK_Shift_L, 0, false}, {XK_Shift_L, xstate::Shift, true}};
        CHECK(ev == want);
    }

    CASE("base：按住 Shift 时 tap End = Shift+End；组合里已被按住的修饰键不再按");
    {
        auto ev = tapEvents(combo("end"), xstate::Shift);
        std::vector<SynthKey> want = {{XK_End, xstate::Shift, false}, {XK_End, xstate::Shift, true}};
        CHECK(ev == want);
        auto ev2 = tapEvents(combo("end", {"shift", "ctrl"}), xstate::Shift);
        const uint32_t cs = xstate::Ctrl | xstate::Shift;
        std::vector<SynthKey> want2 = {
            {XK_Control_L, xstate::Shift, false},
            {XK_End, cs, false},
            {XK_End, cs, true},
            {XK_Control_L, cs, true},
        };
        CHECK(ev2 == want2);
    }

    CASE("hold / release：按下不放、成对抬起；没按住的 release 不凭空造抬起");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        CHECK(t.hold(combo("shift"), 1000, 10000, out) == KeyHoldTracker::HoldResult::Pressed);
        CHECK(out == (std::vector<SynthKey>{{XK_Shift_L, 0, false}}));
        CHECK_EQ(t.heldStates(), xstate::Shift);
        CHECK(t.hold(combo("shift"), 1001, 10000, out) == KeyHoldTracker::HoldResult::AlreadyHeld);
        CHECK(out.empty());
        CHECK(t.release(combo("a")).empty());
        auto up = t.release(combo("shift"));
        CHECK(up == (std::vector<SynthKey>{{XK_Shift_L, xstate::Shift, true}}));
        CHECK(t.empty());
        CHECK(t.release(combo("shift")).empty()); // 第二次 release：已无可抬
    }

    CASE("hold Ctrl+A：抬起顺序 a → Ctrl");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        t.hold(combo("a", {"ctrl"}), 0, 10000, out);
        CHECK(out == (std::vector<SynthKey>{{XK_Control_L, 0, false}, {'a', xstate::Ctrl, false}}));
        auto up = t.release(combo("a", {"ctrl"}));
        CHECK(up == (std::vector<SynthKey>{{'a', xstate::Ctrl, true},
                                           {XK_Control_L, xstate::Ctrl, true}}));
    }

    CASE("releaseAll（失焦 / 换 IC / reset / 服务重启 / 断线 / 析构共用）：逆序全抬、清空");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        t.hold(combo("shift"), 0, 10000, out);
        t.hold(combo("ctrl"), 0, 10000, out);
        auto up = t.releaseAll();
        std::vector<SynthKey> want = {
            {XK_Control_L, xstate::Shift | xstate::Ctrl, true},
            {XK_Shift_L, xstate::Shift, true},
        };
        CHECK(up == want);
        CHECK(t.empty());
        CHECK(t.releaseAll().empty());
        CHECK(!t.nextDeadline().has_value());
    }

    CASE("两个 hold 共用修饰键：第二个不再按 Shift，全抬时 Shift 只抬一次");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        t.hold(combo("shift"), 0, 10000, out);
        t.hold(combo("a", {"shift"}), 0, 10000, out);
        CHECK(out == (std::vector<SynthKey>{{'a', xstate::Shift, false}}));
        auto up = t.releaseAll();
        std::vector<SynthKey> want = {{'a', xstate::Shift, true}, {XK_Shift_L, xstate::Shift, true}};
        CHECK(up == want);
    }

    CASE("最长保持时间：到点自动抬起，未到的留着；nextDeadline 取最早");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        t.hold(combo("shift"), 1000, 500, out);
        t.hold(combo("ctrl"), 1200, 500, out);
        CHECK(t.nextDeadline() == std::optional<uint64_t>(1500));
        CHECK(t.releaseExpired(1499).empty());
        auto up = t.releaseExpired(1500);
        // 剩下的 Ctrl 仍按着：Shift 抬起时 state 里还有 Ctrl。
        CHECK(up == (std::vector<SynthKey>{{XK_Shift_L, xstate::Shift | xstate::Ctrl, true}}));
        CHECK(t.nextDeadline() == std::optional<uint64_t>(1700));
        CHECK(t.releaseExpired(1700)
              == (std::vector<SynthKey>{{XK_Control_L, xstate::Ctrl, true}}));
        CHECK(t.empty());
    }

    CASE("同时按住的上限：满了拒绝、不按");
    {
        KeyHoldTracker t;
        std::vector<SynthKey> out;
        for (size_t i = 0; i < kMaxHeldCombos; ++i) {
            CHECK(t.hold(combo(std::string(1, char('a' + i))), 0, 10000, out)
                  == KeyHoldTracker::HoldResult::Pressed);
        }
        CHECK(t.hold(combo("z"), 0, 10000, out) == KeyHoldTracker::HoldResult::Full);
        CHECK(out.empty());
        CHECK_EQ(t.size(), kMaxHeldCombos);
    }

    CASE("最长保持时间的环境变量：正整数覆盖，非法值退回默认");
    CHECK_EQ(keyHoldTimeoutMs(nullptr), kDefaultKeyHoldTimeoutMs);
    CHECK_EQ(keyHoldTimeoutMs(""), kDefaultKeyHoldTimeoutMs);
    CHECK_EQ(keyHoldTimeoutMs("1500"), 1500u);
    CHECK_EQ(keyHoldTimeoutMs("0"), kDefaultKeyHoldTimeoutMs);
    CHECK_EQ(keyHoldTimeoutMs("-5"), kDefaultKeyHoldTimeoutMs);
    CHECK_EQ(keyHoldTimeoutMs("12x"), kDefaultKeyHoldTimeoutMs);

    CASE("频率限流：窗口内累计超额整批拒绝；窗口滑过后恢复");
    {
        SynthRateLimiter r;
        CHECK(r.allow(kMaxSynthEventsPerWindow - 8, 0));
        CHECK(r.allow(8, 10));
        CHECK(!r.allow(1, 20));                   // 满了
        CHECK(!r.allow(1, kSynthWindowMs - 1));    // 第一批仍在窗口里
        CHECK(r.allow(kMaxSynthEventsPerWindow - 8, kSynthWindowMs)); // 第一批滑出
        CHECK(!r.allow(9, kSynthWindowMs));
        CHECK(r.allow(8, kSynthWindowMs + 10));   // 第二批（8 个）滑出
        SynthRateLimiter big;
        CHECK(!big.allow(kMaxSynthEventsPerWindow + 1, 0)); // 单批就超额
        // 满长 seq（每个组合 4 修饰 + 主键）一条放得下。
        CHECK(kMaxSeqCombos * 10 <= kMaxSynthEventsPerWindow);
    }

    CASE("帧解码：tap / seq 按 Rust push_key_combo 布局");
    {
        auto c = decodeKeyCombo(comboBytes("c", {"ctrl"}));
        CHECK(c.has_value());
        CHECK(c && c->key == "c" && c->mods == std::vector<std::string>{"ctrl"});
        Bytes seq = u32le(3);
        for (auto& b : {comboBytes("home", {}), comboBytes("end", {"shift"}),
                        comboBytes("backspace", {})}) {
            seq.insert(seq.end(), b.begin(), b.end());
        }
        auto s = decodeKeySeq(seq);
        CHECK(s && s->size() == 3);
        CHECK(s && (*s)[1].key == "end" && (*s)[1].mods == std::vector<std::string>{"shift"});
        CHECK(s && (*s)[2].key == "backspace" && (*s)[2].mods.empty());
        auto empty = decodeKeySeq(u32le(0));
        CHECK(empty && empty->empty());
    }

    CASE("帧解码：截断 / 巨大计数 → 空，不越界不爆内存");
    {
        Bytes b = comboBytes("end", {"shift"});
        b.pop_back();
        CHECK(!decodeKeyCombo(b).has_value());
        CHECK(!decodeKeyCombo(Bytes{}).has_value());
        Bytes hugeMods = lenStr("c");
        Bytes n = u32le(0xFFFFFFFFu);
        hugeMods.insert(hugeMods.end(), n.begin(), n.end());
        CHECK(!decodeKeyCombo(hugeMods).has_value());
        CHECK(!decodeKeySeq(u32le(0x7FFFFFFFu)).has_value());
        Bytes shortSeq = u32le(2);
        Bytes one = comboBytes("a", {});
        shortSeq.insert(shortSeq.end(), one.begin(), one.end());
        CHECK(!decodeKeySeq(shortSeq).has_value());
    }

    TEST_MAIN_END();
}
