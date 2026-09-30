// 「宿主经 IMM32 设的候选窗位置（CANDIDATEFORM）能不能当锚点」的**纯判据**。
//
// 背景：走 IMM32→TSF 兼容层（CUAS）的老宿主——Java AWT/Swing 是典型——根本不经 TSF 报坐标，
// `GetTextExt` 对选区、组合起点、组合整体三次查询给的是**同一个**工作区右下角像素
// （医疗 HIS/LIS 客户端实测 (1919,1039,1920,1039)，1920×1040 工作区）。它们报位置的唯一
// 渠道是 `ImmSetCandidateWindow`：AWT 收到 `IMN_OPENCANDIDATE`/`IMN_CHANGECANDIDATE` 后
// 按 `getTextLocation(leading(0))` 算出组合起点**下沿**，以客户区坐标写进 CANDIDATEFORM。
// 这一点先于我们把 IMM32 探测接进降级链的时候只有源码依据、没有实测日志，所以判据刻意偏紧。
//
// 抽成头文件只为可测：判据跑在宿主进程里，真机才观察得到；而它放宽的代价是把某个宿主残留的
// 陈旧 CANDIDATEFORM 以「权威坐标」的身份送下去。
//
// ⚠️ 覆盖边界：这里只有判据。IMC 怎么取、ClientToScreen 用哪个窗口、高度怎么合成，都在
// CaretEditSession.cpp 里，本文件一行也覆盖不到。
#pragma once

namespace wind
{
namespace caret
{

// CANDIDATEFORM::dwStyle 的取值（imm.h）。本头文件不引 Win32，故就地重述；
// CaretEditSession.cpp 用 static_assert 把它们与 imm.h 对齐。
constexpr unsigned long kCfsDefault = 0x0000;
constexpr unsigned long kCfsPoint = 0x0002;
constexpr unsigned long kCfsCandidatePos = 0x0040;
constexpr unsigned long kCfsExclude = 0x0080;

/// 这个 dwStyle 说的是不是「候选窗该放在 ptCurrentPos」。
///
/// 三种都以 ptCurrentPos 为落点：CFS_CANDIDATEPOS 是标准写法，CFS_EXCLUDE 另带一块避让区
/// （rcArea），CFS_POINT 是 AWT 的写法（它对候选窗也用这个）。
/// CFS_DEFAULT = 宿主没表态，ptCurrentPos 无意义。
constexpr bool IsCandidateFormPositioned(unsigned long style)
{
    return style == kCfsPoint || style == kCfsCandidatePos || style == kCfsExclude;
}

/// 宿主设的候选窗落点（已换成屏幕坐标）能否当锚点采信。
///
/// ★ **只在 `hostSaysNoCaret` 时考虑**（TSF 三次查询给出同一个退化矩形，见
/// `wind::caret::IsHostDefaultPosition`）：证据只有这一种形态。其它退化——尤其「只退化几十毫秒、
/// 随后给出真矩形」的宿主——正确做法是等下一帧，采信一个残留的 CANDIDATEFORM 只会凭空多一次
/// 闪跳；GetTextExt 整个失败的游戏类宿主同理，攒到实测数据再放开。
///
/// ★ **失败关闭**：`hasScreenExt` 为假一律不采信。与二级降级同一个理由——这是新增的来源，
/// 拿不到显示区时跳过它只是退回没有本级时的行为（无损）；放行则可能以 TSF 权威域的名义
/// 送下去一个陈旧的 CANDIDATEFORM（宿主上一个输入框留下的）。
///
/// ★ 落点必须在宿主自己声明的显示区内（边界含）：CANDIDATEFORM 是客户区坐标，换算用的窗口
/// 若不是宿主设它时用的那个，结果会整体偏出窗口，这道校验正好把它挡住。
template <class RectT>
constexpr bool AcceptCandidateFormPoint(bool hostSaysNoCaret, bool gotForm, unsigned long style, long x, long y,
                                        bool hasScreenExt, const RectT& screenExt)
{
    return hostSaysNoCaret && gotForm && IsCandidateFormPositioned(style) && hasScreenExt && x >= screenExt.left
           && x <= screenExt.right && y >= screenExt.top && y <= screenExt.bottom;
}

} // namespace caret
} // namespace wind
