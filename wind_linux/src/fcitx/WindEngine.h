// Fcitx5 输入法引擎：清风输入法在 Linux 上的「薄壳」。
//
// 与 wind_tsf（Windows TSF DLL）、wind_macos（IMKit .app）对位：按键转发给 Rust 服务、
// 把服务的上屏/预编辑写回 InputContext、把服务光栅化好的候选窗帧显示出来。引擎/词库/
// 候选逻辑全在服务里，这里不做任何「中英判定」。
//
// 线程模型：Fcitx5 在主线程（事件循环）调 keyEvent/activate/deactivate，本类在其中**同步**
// 请求服务（UDS 往返 <1ms，2s 超时兜底）。push 通道在后台线程收帧，经 EventDispatcher
// 转回主线程再碰 InputContext——InputContext 与 InputPanel 都不是线程安全的。
#pragma once

#include "Bridge.h"
#include "KeyMap.h"
#include "ResponseRouter.h"
#include "ServiceLauncher.h"
#include "SettingsLauncher.h"
#include "ExtProtocol.h"
#include "HostUi.h"
#include "ShmFrame.h"

#include <fcitx-utils/event.h>
#include <fcitx-utils/eventdispatcher.h>
#include <fcitx-utils/trackableobject.h>
#include <fcitx-config/configuration.h>
#include <fcitx-config/option.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/action.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>

#include <memory>
#include <unordered_set>

namespace windlinux {

class X11CandidatePanel;

/// 输入法 / addon 的「配置」：只有一项外部工具（设置程序）。Fcitx5 的配置工具（fcitx5-configtool
/// 5.1.6 起）遇到「整页只有一个 External 项」就直接启动它，不再弹一张空白配置页；更早的版本
/// 显示一页、页上一个按钮。fcitx5-mozc 用的是同一机制（它的 addon 里没有静态 .desc）。
FCITX_CONFIGURATION(WindConfig,
                    fcitx::ExternalOption settings{this, "WindSetting", "清风输入法设置",
                                                   settingsPath()};);

class WindEngine final : public fcitx::InputMethodEngineV2 {
public:
    explicit WindEngine(fcitx::Instance* instance);
    ~WindEngine() override;

    void keyEvent(const fcitx::InputMethodEntry& entry, fcitx::KeyEvent& event) override;
    void activate(const fcitx::InputMethodEntry& entry, fcitx::InputContextEvent& event) override;
    void deactivate(const fcitx::InputMethodEntry& entry, fcitx::InputContextEvent& event) override;
    void reset(const fcitx::InputMethodEntry& entry, fcitx::InputContextEvent& event) override;

    // 托盘 / 面板随中英模式换图标与标签（Fcitx5 的 classicui / notificationitem / kimpanel
    // 都经这两个取）。服务端的模式是全局的，与 IC 无关。
    std::string subMode(const fcitx::InputMethodEntry& entry, fcitx::InputContext& ic) override;
    std::string subModeIconImpl(const fcitx::InputMethodEntry& entry, fcitx::InputContext& ic) override;
    std::string subModeLabelImpl(const fcitx::InputMethodEntry& entry, fcitx::InputContext& ic) override;

    const fcitx::Configuration* getConfig() const override { return &config_; }

private:
    // ── 连接 ──
    /// 连不上服务时按节流拉起它（服务自己持单例锁，重复拉起无害）。
    ServiceLauncher launcher_;
    bool ensureConnected();
    bool reconnect();
    /// 在 request 连接上发一帧、读响应；连接失败时重连并**重试一次**（服务重启后的第一个键
    /// 就自愈、不丢字——对位 macOS `handle` 的同名策略）。
    bool requestWithRetry(const Bytes& frame, Frame& resp);
    /// 发一帧、读掉 ack，失败只记日志。
    void sendAndDrain(const Bytes& frame);
    /// 发一帧不读响应（AsyncFlag 帧）。
    void sendAsync(const Bytes& frame);

    // ── 焦点 ──
    uint64_t clientToken(fcitx::InputContext* ic) const;
    uint64_t inputScopeMask(fcitx::InputContext* ic) const;
    void sendFocusGained(fcitx::InputContext* ic);
    void sendCaretUpdate(fcitx::InputContext* ic);
    uint16_t prevCharFor(fcitx::InputContext* ic);
    /// 修饰键单击：发一帧 eventType=UP 的 KeyEvent 并应用其响应。
    void sendModifierTap(fcitx::InputContext* ic, uint32_t vk);
    bool applyResponse(fcitx::InputContext* ic, const Frame& resp, bool hostShortcut);
    /// 帧里带中英模式就记下；变了则让 Fcitx5 的 UI 模块重取图标。
    void noteMode(const Frame& frame);

    // ── push ──
    void onPushFrame(Frame frame);
    void onRenderFrame(const HostRenderFramePayload& p);
    void onOverlayFrame(const OverlayFramePayload& p);
    void onExt(const ExtEnvelope& ext);
    void onMenuFrame(uint32_t level, const OverlayFramePayload& p);
    /// 启动设置程序（`settings.open` 信封与状态区入口共用）。只启动自己的设置程序，args 只进参数位。
    void launchSettings(const std::vector<std::string>& args);
    fcitx::InputContext* focusedIC();

    // ── 自绘菜单 ──
    /// 请服务端打开菜单（`menu.open`）：target ≥ 0 候选右键菜单，-1 主菜单；(x, y) 锚点。
    void requestMenu(int32_t target, int32_t x, int32_t y);

    fcitx::Instance* instance_;
    BridgeClient bridge_;
    ResponseRouter router_;
    ToggleTapDetector tap_;
    uint16_t keySeq_ = 0;
    bool lastReportedSecure_ = false;
    bool serviceSeenOnce_ = false;
    /// 按下时被本输入法吃掉的键（按硬件键码记）。松开时同样吃掉，免得宿主收到一个没有
    /// 按下的松开——多数宿主无所谓，但有的会据此触发快捷键。
    std::unordered_set<int> eatenKeys_;

    fcitx::TrackableObjectReference<fcitx::InputContext> currentIC_;
    fcitx::EventDispatcher dispatcher_;
    std::unique_ptr<fcitx::EventSourceTime> holdTimer_;
    std::unique_ptr<PushClient> push_;
    ShmFrameReader shm_;
    /// 光栅浮层各层的 SHM 读端（下标 = kind - 1）。与 `shm_` 同样在 SERVICE_READY 时关掉重开。
    ShmFrameReader overlayShm_[3];
    /// 自绘菜单各级的 SHM 读端（下标 = 级）。
    ShmFrameReader menuShm_[OVERLAY_MENU_LEVELS];
    std::unique_ptr<X11CandidatePanel> panel_;
    /// 「清风输入法设置」：挂进 Fcitx5 状态区（托盘菜单 / kimpanel 面板），点了打开设置程序。
    /// 主菜单不从这里进（组字时候选窗右键 / 候选菜单「更多…」）。
    fcitx::SimpleAction settingsAction_;
    ModeIndicator mode_;
    WindConfig config_;
};

class WindEngineFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance* create(fcitx::AddonManager* manager) override
    {
        return new WindEngine(manager->instance());
    }
};

} // namespace windlinux
