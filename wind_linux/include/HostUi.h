// 交给 Fcitx5 呈现的那部分宿主 UI 的纯逻辑：托盘 / 面板的中英模式图标、应用内预编辑显示什么。
//
// 与候选窗、浮层不同，这两样不是自绘的：模式图标由 Fcitx5 的 UI 模块（classicui 托盘、
// notificationitem 的 StatusNotifierItem、kimpanel）经 `subModeIcon` 取，预编辑由应用自己画。
// 本端只决定给它们什么。
#pragma once

#include "Codec.h"

#include <optional>
#include <string>

namespace windlinux {

/// 中英模式的本端镜像，只从服务端的帧里学，不自己判定。
///
/// 模式的来源有四条，都要接——漏一条，图标就停在旧态直到下一次别的途径碰巧刷新：
/// - 焦点进入：`CMD_FOCUS_GAINED` 的响应 `CMD_MODE_PUSH`（flags u32）；
/// - 按键切换（Shift 单击、热键）：按键响应 `CMD_STATUS_UPDATE`；
/// - 别处切换（菜单点「英文」、设置程序改了默认态）：push 通道的 `CMD_STATE_PUSH`；
/// - 上屏顺带切换（自动切英等）：`CMD_COMMIT_TEXT` 带 `COMMIT_FLAG_MODE_CHANGED`。
/// `CMD_ACTIVATION_STATUS_PUSH` 与 `STATUS_UPDATE` 同载荷，一并认。
class ModeIndicator {
public:
    /// 帧里带模式就记下；返回模式是否**变了**（调用方据此刷新托盘图标）。
    bool update(const Frame& frame);

    /// 还没从服务端学到过模式。此时图标用输入法条目自己的（`Icon=windinput`）。
    bool known() const { return chinese_.has_value(); }
    bool chinese() const { return chinese_.value_or(true); }

    /// 托盘 / 面板图标名（hicolor 主题里的 `windinput-zh` / `windinput-en`）；未知时为空，
    /// Fcitx5 据此退回条目图标。
    std::string iconName() const;
    /// 面板的文字标签（kimpanel、classicui 偏好文字图标时）：「中」/「英」；未知时为空。
    std::string label() const;
    /// 子模式名（Fcitx5 输入法信息提示里跟在输入法名后面）。
    std::string subModeName() const;

private:
    std::optional<bool> chinese_;
};

/// 帧里携带的中英模式；不带模式的帧返回空。
std::optional<bool> chineseModeOf(const Frame& frame);

/// 服务端下发的组合串 → 应用内预编辑该显示的文本。
///
/// 非嵌入模式（`preedit_display` ≠ app_inline）与联想态、加词等模式下，服务端让宿主挂一段
/// 单个空格的**占位组合**（`COMPOSITION_PLACEHOLDER`）：Windows 的 TSF 要有 composition 才
/// 取得到光标坐标。Fcitx5 直接给 `cursorRect`，不需要它；而它在应用里是真的一格空白，把光标
/// 往右推一格。本端照常把它记成「有组合」（失焦 / 宿主重置要据此收尾、报服务端），只是不显示。
std::string clientPreeditText(const std::string& composition);

} // namespace windlinux
