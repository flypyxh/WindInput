// 光栅浮层（状态气泡 / 悬停提示 / Toast）与候选窗悬停的鼠标交互纯逻辑（不依赖 Fcitx5 / X11）。
//
// 这三层浮层与候选窗同构：像素是服务端画的，本端只贴图。交互分两类：
//   - **本端就能定的**：悬停保持、拖动、Toast 点击关闭、自动隐藏的暂停——计时与落位本来就在
//     本端（同 macOS `.app`），服务端没有要跟着动的状态；
//   - **要服务端定的**：右键菜单（菜单项、勾选态、按段 / 按行命中都在服务端），经 `menu.open`
//     报上去，与候选右键菜单同一条路（见 Menu.h）。
// 每条规则对位 Windows 的哪个窗口 / 函数，写在各自的注释里。
#pragma once

#include "ShmFrame.h"

#include <cstdint>

namespace windlinux {

/// 指针在某层浮层上按下时做什么。对位 Windows：
///   - 状态气泡 `StatusTipMouse`：左键拖动，右键请求菜单；
///   - 悬停提示 `TooltipMouse`：只认右键（请求菜单），左键无动作；
///   - Toast：Windows 没有鼠标交互，这里加的是「点一下提前关掉」（任意键）。
enum class OverlayPress { None, Drag, Menu, Close };
OverlayPress overlayPressAction(uint32_t kind, uint8_t button);

/// 右键某层浮层时 `menu.open` 的 target（MENU_TARGET_STATUS / MENU_TARGET_TOOLTIP）；
/// 该层没有菜单返回 MENU_TARGET_MAIN（调用方不应走到这里）。
int32_t overlayMenuTarget(uint32_t kind);

/// 悬停门控：**「收到移动事件」不等于「用户动了鼠标」**。气泡弹在静止的指针下面、或状态刷新时
/// 挪到指针下面，X 一样会发进入 / 移动事件；把它当悬停，气泡就永远不会自动消失。故以窗口出现
/// 那一刻的指针位置为基线，位置真变了才算。对位 Windows `StatusTipMouse::accept_move`。
class HoverGate {
public:
    /// 窗口从隐藏变为显示时调用，基线取此刻的指针位置。已显示时不要调（会抹掉真实悬停）。
    void rebase(int32_t x, int32_t y)
    {
        x_ = x;
        y_ = y;
    }
    /// 一次指针事件：与基线不同才算真实移动（并推进基线）。
    bool accept(int32_t x, int32_t y)
    {
        if (x == x_ && y == y_) {
            return false;
        }
        x_ = x;
        y_ = y;
        return true;
    }

private:
    int32_t x_ = INT32_MIN;
    int32_t y_ = INT32_MIN;
};

/// 候选悬停变化的延后（毫秒；0 = 立即报）。对位 Windows `candidate_window::hover_move`：只有
/// `last` 的悬停提示正显示时才延后——去往「无」（空隙 / 离开候选窗）按 kTipGraceMs，去往另一个
/// 候选按 kTipSwitchMs。给「从候选行穿过空隙挪进提示」留时间：一离开就报，提示当场被收掉，
/// 用户永远够不着它。`raw` 为 -1 表示无候选。
constexpr uint32_t kTipGraceMs = 280;
constexpr uint32_t kTipSwitchMs = 150;
uint32_t hoverDeferMs(int32_t last, int32_t raw, bool tipShown);

/// 「不改悬停」的哨兵（`tipRecheckHover` 的返回值）。
constexpr int32_t kKeepHover = INT32_MIN;

/// 离开悬停提示后宽限到期（或菜单关闭后）重新定悬停。对位 Windows `menu_step` 的
/// `Recheck` / `Closed`：指针回到提示上 → 不动；回到提示所属的候选行 → 不动（此后去留跟着
/// 候选悬停走）；在候选窗的别处 → 报那里（候选下标或 -1）；在外面 → 报 -1（提示随之收起）。
/// `held` 是本端最近报的悬停，`candidateHit` 为指针处的候选命中（kNoHit / 翻页按钮都算「无」）。
int32_t tipRecheckHover(bool onTip, bool onCandidate, int32_t candidateHit, int32_t held);

/// 拖动状态气泡：指针在 `(px, py)`、按下时指针相对窗口左上的偏移为 `(grabDx, grabDy)`，返回
/// 新的**窗口**矩形。内容盒夹进工作区，规则与 `placeOverlay` 的最后一步是同一条（阴影扩边
/// 允许出屏）——落点再经服务端存成固定位置、下一次按 ABSOLUTE 摆放时不会被挪动。
/// 对位 Windows `StatusTipMouse` 的 WM_MOUSEMOVE（那边按窗口矩形钳制，这里按内容盒，与本端
/// 其余浮层的落位一致）。`geo` 取该层最近一帧的尺寸与内容盒偏移。
Rect dragOverlay(int32_t px, int32_t py, int32_t grabDx, int32_t grabDy,
                 const OverlayFramePayload& geo, const Rect& workArea);

/// 窗口矩形里是否包含屏幕点（半开区间）。
bool rectContains(const Rect& r, int32_t x, int32_t y);

} // namespace windlinux
