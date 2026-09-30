// 交给 Fcitx5 呈现的那部分宿主 UI 的纯逻辑：托盘 / 面板的中英模式图标、应用内预编辑显示什么。
//
// 与候选窗、浮层不同，这两样不是自绘的：模式图标由 Fcitx5 的 UI 模块（classicui 托盘、
// notificationitem 的 StatusNotifierItem、kimpanel）经 `subModeIcon` 取，预编辑由应用自己画。
// 本端只决定给它们什么。
#pragma once

#include "Codec.h"

#include <cstdint>
#include <optional>
#include <string>

namespace windlinux {

/// 用户图标目录下的 hicolor 根：`$XDG_DATA_HOME/icons/hicolor`（未设或非绝对路径时
/// `$HOME/.local/share/icons/hicolor`）。与服务端 `wind_ui::tray_icon::user_hicolor_dir` 同一规则；
/// 拿不到时为空串。
std::string userHicolorDir();

/// 建好 `root/<N>x<N>/apps`（N = 16/22/24/32/48/64，同服务端 `tray_icon::SIZES`）。成功或已存在返回 true。
///
/// Fcitx5 的 `IconTheme`（classicui 托盘）、Qt 的 `QIconTheme`、KIconLoader 在构造主题时把**不存在**的
/// 目录永久排除：服务端第一次写图标若晚于它们，就要等宿主重启才看得见。addon 随 Fcitx5 启动而加载，
/// 在这里先建一次（服务端写之前也会建）。
bool ensureIconDirs(const std::string& root);

/// 状态帧里与托盘图标有关的那几项。
struct ModeStatus {
    bool chinese = true;
    /// 大写锁定：只有带完整状态头的帧（`STATUS_UPDATE` / `STATE_PUSH` / `ACTIVATION_STATUS_PUSH`）
    /// 才说得上来；`MODE_PUSH` 只有中英 / 全角 / 标点三位，不能拿它的「没置位」当「没开」。
    std::optional<bool> capsLock;
    /// 服务端算好的模式主字（`mode_icon_label`：有效中文取方案标签、大写锁定取 `[ui.labels]
    /// caps_lock`、英文取 `english`），同样只在完整状态帧里有。
    std::optional<std::string> label;
};

/// 帧里携带的模式；不带模式的帧返回空。
std::optional<ModeStatus> modeStatusOf(const Frame& frame);

/// 中英 / 大写锁定 / 方案标签的本端镜像。中英与标签只从服务端的帧里学；大写锁定另认
/// 本端按键（见 `CapsLockTracker`）——两者读的是同一个数据（按键的 Lock 位），不是两个真相源。
///
/// 模式的来源有四条，都要接——漏一条，图标就停在旧态直到下一次别的途径碰巧刷新：
/// - 焦点进入：`CMD_FOCUS_GAINED` 的响应 `CMD_MODE_PUSH`（flags u32）；
/// - 按键切换（Shift 单击、热键、CapsLock 松开）：按键响应 `CMD_STATUS_UPDATE`；
/// - 别处切换（菜单点「英文」、设置程序改了默认态）：push 通道的 `CMD_STATE_PUSH`；
/// - 上屏顺带切换（自动切英等）：`CMD_COMMIT_TEXT` 带 `COMMIT_FLAG_MODE_CHANGED`。
/// `CMD_ACTIVATION_STATUS_PUSH` 与 `STATUS_UPDATE` 同载荷，一并认。
///
/// 图标规则对齐 Windows 语言栏（`effective_chinese = chinese && !caps`）：大写锁定时无论中英都是
/// caps 标签（出厂「A」）；否则英文态 english 标签（「英」）；有效中文态是方案标签。图标是服务端按
/// （状态, 主字）运行时渲染进用户图标目录的（`wind-ui/src/tray_icon.rs`），名字由 `dynamicIconName`
/// 算；那组文件还不在（服务没起来、目录不可写、缺字体）就退回随包的种子 `windinput-zh/en/caps`。
class ModeIndicator {
public:
    /// 帧里带模式就记下；返回托盘图标 / 标签是否**变了**（调用方据此刷新状态区）。
    bool update(const Frame& frame);

    /// 本端判定的大写锁定态（`CapsLockTracker` 的结论）；返回图标 / 标签是否变了。
    bool noteCapsLock(bool on);

    /// 还没从服务端学到过模式。此时图标用输入法条目自己的（`Icon=windinput`）。
    bool known() const { return chinese_.has_value(); }
    bool chinese() const { return chinese_.value_or(true); }
    bool capsLock() const { return caps_; }

    /// 托盘 / 面板图标名：运行时图标（`dynamicIconName`）在就用它，否则种子（`seedIconName`）；
    /// 未知时为空，Fcitx5 据此退回条目图标。
    std::string iconName() const;
    /// 面板的文字标签（kimpanel、classicui 偏好文字图标时）：当前态的主字；未知时为空。
    /// 用的是服务端下发的真实标签，`[ui.labels]` / 自定义方案标签在这里如实显示。
    std::string label() const;
    /// 子模式名（Fcitx5 输入法信息提示里跟在输入法名后面）。
    std::string subModeName() const;

    /// 运行时图标所在的 hicolor 根（默认 `userHicolorDir()`；单测指到临时目录）。
    void setIconRoot(std::string root) { iconRoot_ = std::move(root); }

    /// 状态档：图标名里的状态段，也决定底色。
    enum class Variant { Chinese, English, Caps };
    Variant variant() const;
    /// 运行时图标名：`windinput-lbl-<zh|en|caps>-<主字 UTF-8 小写十六进制>`。
    /// **与 `wind_ui::tray_icon::icon_name` 同一规则**（两侧单测钉同一组样例）。
    static std::string dynamicIconName(Variant v, const std::string& label);
    /// 随包种子：`windinput-zh` / `windinput-en` / `windinput-caps`。
    static std::string seedIconName(Variant v);

private:
    std::optional<bool> chinese_;
    bool caps_ = false;
    /// 按状态分存的标签：本端大写锁定翻转时不必等服务端再发一帧就能给出对的字；
    /// 且大写锁定 / 英文态下服务端发的是 caps / english 标签，不能拿它覆盖方案标签。
    std::string schemaLabel_ = "中";
    std::string englishLabel_ = "英";
    std::string capsLabel_ = "A";
    std::string iconRoot_ = userHicolorDir();
};


/// 大写锁定的本端判定（纯逻辑）。
///
/// X11 事件里的修饰状态是事件**之前**的（实测 Xvfb + xev：开 → 按下 state=0、松开 state=Lock；
/// 关 → 按下 state=Lock、松开**仍是** Lock——XKB 的 LockMods 在按下时上锁、在松开时解锁）。
/// 所以 Caps_Lock 本身的两个事件里都读不出「之后」的状态，只能按「按下时没锁 ⇒ 松开后锁上」
/// 推；别的键的 Lock 位则就是当前锁定态（它们不改锁定），拿来校准（在别的输入法 / 应用里
/// 切过大写回来，第一个键就对上）。
class CapsLockTracker {
public:
    /// 喂一个按键事件（按下 / 松开都喂）；返回本事件之后的锁定态（能判定时）。
    std::optional<bool> onKey(uint32_t keysym, uint32_t states, bool release);

    /// 焦点切换：作废半截的 Caps_Lock 按下（它的松开会落到别的 IC / 根本收不到）。
    void reset() { pressLocked_.reset(); }

private:
    /// 最近一次 Caps_Lock 按下时的 Lock 位；松开时消费。
    std::optional<bool> pressLocked_;
};

/// 服务端下发的组合串 → 应用内预编辑该显示的文本。
///
/// 非嵌入模式（`preedit_display` ≠ app_inline）与联想态、加词等模式下，服务端让宿主挂一段
/// 单个空格的**占位组合**（`COMPOSITION_PLACEHOLDER`）：Windows 的 TSF 要有 composition 才
/// 取得到光标坐标。Fcitx5 直接给 `cursorRect`，不需要它；而它在应用里是真的一格空白，把光标
/// 往右推一格。本端照常把它记成「有组合」（失焦 / 宿主重置要据此收尾、报服务端），只是不显示。
std::string clientPreeditText(const std::string& composition);

} // namespace windlinux
