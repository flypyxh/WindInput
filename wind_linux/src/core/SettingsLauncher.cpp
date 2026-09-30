#include "SettingsLauncher.h"

#include <cstdint>
#include <cstdlib>

#ifndef WIND_SETTING_PATH
#define WIND_SETTING_PATH "/usr/lib/windinput/wind_setting"
#endif

namespace windlinux {

namespace {

// 上限只为挡住畸形数据：服务端实际发的是一两个 `--page=…` / `--schema=…`。
constexpr size_t kMaxArgs = 64;
constexpr size_t kMaxArgBytes = 4096;

// 极简 JSON 读取器：只做 `settings.open` body 需要的那一点（对象、数组、字符串），
// 其余值类型只负责正确跳过。addon 不为这一处引入 JSON 库。
class Json {
public:
    explicit Json(const std::string& s) : s_(s) {}

    bool ok() const { return ok_; }

    void ws()
    {
        while (i_ < s_.size() && (s_[i_] == ' ' || s_[i_] == '\t' || s_[i_] == '\n' || s_[i_] == '\r')) {
            ++i_;
        }
    }

    bool eat(char c)
    {
        ws();
        if (i_ < s_.size() && s_[i_] == c) {
            ++i_;
            return true;
        }
        return false;
    }

    bool atEnd()
    {
        ws();
        return i_ == s_.size();
    }

    char peek()
    {
        ws();
        return i_ < s_.size() ? s_[i_] : '\0';
    }

    std::optional<std::string> string()
    {
        if (!eat('"')) {
            return fail();
        }
        std::string out;
        while (i_ < s_.size()) {
            char c = s_[i_++];
            if (c == '"') {
                return out;
            }
            if (static_cast<unsigned char>(c) < 0x20) {
                return fail(); // JSON 不允许裸控制字符
            }
            if (c != '\\') {
                out.push_back(c);
                continue;
            }
            if (i_ >= s_.size()) {
                return fail();
            }
            switch (s_[i_++]) {
            case '"': out.push_back('"'); break;
            case '\\': out.push_back('\\'); break;
            case '/': out.push_back('/'); break;
            case 'b': out.push_back('\b'); break;
            case 'f': out.push_back('\f'); break;
            case 'n': out.push_back('\n'); break;
            case 'r': out.push_back('\r'); break;
            case 't': out.push_back('\t'); break;
            case 'u': {
                auto cp = hex4();
                if (!cp) {
                    return fail();
                }
                uint32_t u = *cp;
                if (u >= 0xD800 && u <= 0xDBFF) {
                    // 高代理必须紧跟 `\u` 低代理，拼成一个码点。
                    if (i_ + 1 >= s_.size() || s_[i_] != '\\' || s_[i_ + 1] != 'u') {
                        return fail();
                    }
                    i_ += 2;
                    auto lo = hex4();
                    if (!lo || *lo < 0xDC00 || *lo > 0xDFFF) {
                        return fail();
                    }
                    u = 0x10000 + ((u - 0xD800) << 10) + (*lo - 0xDC00);
                } else if (u >= 0xDC00 && u <= 0xDFFF) {
                    return fail(); // 孤立低代理
                }
                utf8(out, u);
                break;
            }
            default: return fail();
            }
        }
        return fail(); // 没有收尾引号
    }

    /// 跳过任意一个值。
    bool skipValue(int depth = 0)
    {
        if (depth > 32) {
            return fail(), false;
        }
        char c = peek();
        if (c == '"') {
            return string().has_value();
        }
        if (c == '{' || c == '[') {
            char close = c == '{' ? '}' : ']';
            eat(c);
            if (eat(close)) {
                return true;
            }
            do {
                if (c == '{' && (!string() || !eat(':'))) {
                    return fail(), false;
                }
                if (!skipValue(depth + 1)) {
                    return false;
                }
            } while (eat(','));
            return eat(close) || (fail(), false);
        }
        // 数字 / true / false / null：吃到分隔符为止。
        size_t start = i_;
        while (i_ < s_.size() && s_[i_] != ',' && s_[i_] != '}' && s_[i_] != ']' && s_[i_] != ' '
               && s_[i_] != '\n' && s_[i_] != '\t' && s_[i_] != '\r') {
            ++i_;
        }
        return i_ > start || (fail(), false);
    }

    std::nullopt_t fail()
    {
        ok_ = false;
        i_ = s_.size();
        return std::nullopt;
    }

private:
    std::optional<uint32_t> hex4()
    {
        if (i_ + 4 > s_.size()) {
            return std::nullopt;
        }
        uint32_t v = 0;
        for (int k = 0; k < 4; ++k) {
            char h = s_[i_++];
            v <<= 4;
            if (h >= '0' && h <= '9') {
                v |= uint32_t(h - '0');
            } else if (h >= 'a' && h <= 'f') {
                v |= uint32_t(h - 'a' + 10);
            } else if (h >= 'A' && h <= 'F') {
                v |= uint32_t(h - 'A' + 10);
            } else {
                return std::nullopt;
            }
        }
        return v;
    }

    static void utf8(std::string& out, uint32_t u)
    {
        if (u < 0x80) {
            out.push_back(char(u));
        } else if (u < 0x800) {
            out.push_back(char(0xC0 | (u >> 6)));
            out.push_back(char(0x80 | (u & 0x3F)));
        } else if (u < 0x10000) {
            out.push_back(char(0xE0 | (u >> 12)));
            out.push_back(char(0x80 | ((u >> 6) & 0x3F)));
            out.push_back(char(0x80 | (u & 0x3F)));
        } else {
            out.push_back(char(0xF0 | (u >> 18)));
            out.push_back(char(0x80 | ((u >> 12) & 0x3F)));
            out.push_back(char(0x80 | ((u >> 6) & 0x3F)));
            out.push_back(char(0x80 | (u & 0x3F)));
        }
    }

    const std::string& s_;
    size_t i_ = 0;
    bool ok_ = true;
};

} // namespace

std::string settingsPath()
{
    if (const char* env = std::getenv("WIND_INPUT_SETTING"); env && *env) {
        return env;
    }
    return WIND_SETTING_PATH;
}

std::optional<std::vector<std::string>> parseSettingsOpenArgs(const std::string& body)
{
    Json j(body);
    if (!j.eat('{')) {
        return std::nullopt;
    }
    std::optional<std::vector<std::string>> args;
    if (!j.eat('}')) {
        do {
            auto key = j.string();
            if (!key || !j.eat(':')) {
                return std::nullopt;
            }
            if (*key != "args") {
                if (!j.skipValue()) {
                    return std::nullopt;
                }
                continue;
            }
            if (!j.eat('[')) {
                return std::nullopt; // args 不是数组
            }
            std::vector<std::string> out;
            if (!j.eat(']')) {
                do {
                    auto a = j.string(); // 数组里混了非字符串 → 整条作废
                    if (!a || a->find('\0') != std::string::npos || a->size() > kMaxArgBytes
                        || out.size() >= kMaxArgs) {
                        return std::nullopt;
                    }
                    out.push_back(std::move(*a));
                } while (j.eat(','));
                if (!j.eat(']')) {
                    return std::nullopt;
                }
            }
            args = std::move(out);
        } while (j.eat(','));
        if (!j.eat('}')) {
            return std::nullopt;
        }
    }
    if (!j.ok() || !j.atEnd()) {
        return std::nullopt;
    }
    // 缺 `args` 键等价于空参数（打开默认页），与服务端 `settings_argv` 的空数组同义。
    return args ? std::move(args) : std::vector<std::string>{};
}

std::vector<std::string> settingsArgv(const std::vector<std::string>& args)
{
    std::vector<std::string> argv;
    argv.reserve(args.size() + 1);
    argv.push_back(settingsPath());
    argv.insert(argv.end(), args.begin(), args.end());
    return argv;
}

} // namespace windlinux
