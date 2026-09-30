// 「打开设置」：服务端经下行扩展信封 `settings.open`（body = `{"args":[…]}`）让宿主启动
// 设置程序（wind_setting）。对位 macOS `.app` 的 openSettings；Windows 由 TSF DLL 的
// ShellExecuteW 承担同一件事。
//
// 安全边界：**只启动自己的设置程序**。程序路径只取编译期常量或环境变量
// WIND_INPUT_SETTING，信封里的内容一律只当参数（argv[1..]），不经 shell、不当路径解析——
// push 通道里来什么都拉不起别的程序。参数内容原样转发（与 Windows 的命令行直通同语义，
// 合法性由设置端自己判断）。
#pragma once

#include <optional>
#include <string>
#include <vector>

namespace windlinux {

/// 扩展信封的 kind（真源 protocol.rs `ext_kind::SETTINGS_OPEN`，由 protocol_sync_test 对账）。
inline constexpr const char* EXT_KIND_SETTINGS_OPEN = "settings.open";

/// 设置程序路径：环境变量 WIND_INPUT_SETTING 优先，其次编译期的 WIND_SETTING_PATH。
std::string settingsPath();

/// 解析 `settings.open` 的 body：取 `args` 字符串数组。
///
/// 只认服务端 serde_json 产出的形状（对象里一个 `args` 字符串数组，其余键忽略），
/// 字符串转义按 JSON 规范全解（含 `\uXXXX` 与代理对 → UTF-8）。形状不对、参数含 NUL
/// （拼不成 C 字符串）、参数过多/过长时返回空——宁可不开，也不带着截断的参数开。
std::optional<std::vector<std::string>> parseSettingsOpenArgs(const std::string& body);

/// 组装完整 argv：`[settingsPath, args…]`。
std::vector<std::string> settingsArgv(const std::vector<std::string>& args);

} // namespace windlinux
