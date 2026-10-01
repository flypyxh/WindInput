#include "WindEngine.h"

#include "ExtProtocol.h"
#include "Menu.h"
#include "ServiceLauncher.h"
#include "SettingsLauncher.h"
#include "Protocol.h"
#include "Utf.h"
#include "X11Panel.h"

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/misc.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/inputpanel.h>
#include <fcitx/statusarea.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include <fcitx/userinterfacemanager.h>

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <unistd.h>

FCITX_DEFINE_LOG_CATEGORY(windinput_log, "windinput");
#define WIND_DEBUG() FCITX_LOGC(windinput_log, Debug)
#define WIND_INFO() FCITX_LOGC(windinput_log, Info)
#define WIND_WARN() FCITX_LOGC(windinput_log, Warn)

namespace windlinux {

namespace {

/// 十六进制格式化。不直接往日志流里塞 std::hex：那个格式标志是粘性的，会把同一流上后续
/// 日志的行号也印成十六进制（实测 `WindEngine.cpp:a8`）。
std::string hex(uint32_t v)
{
    char buf[16];
    std::snprintf(buf, sizeof(buf), "0x%04X", v);
    return buf;
}

uint64_t nowMs()
{
    using namespace std::chrono;
    return uint64_t(duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count());
}

/// InputContext → TextSink 适配器（对位 Swift `IMKClientAdapter`）。每次响应现造，短命。
class ICSink final : public TextSink {
public:
    explicit ICSink(fcitx::InputContext* ic) : ic_(ic) {}

    void commitText(const std::string& utf8) override
    {
        if (!utf8.empty()) {
            ic_->commitString(utf8);
        }
    }

    void setPreedit(const std::string& composition, size_t caretBytes) override
    {
        fcitx::Text text;
        // 占位组合（单个空格）不进应用：Fcitx5 直接给 cursorRect，不靠它取坐标（见 HostUi.h）。
        const std::string utf8 = clientPreeditText(composition);
        if (!utf8.empty()) {
            // DontCommit：客户端没声明 ClientUnfocusCommit 时，Fcitx5 会在失焦那一刻把 client
            // preedit 当正文上屏（实测 DBus 客户端失焦后收到 "ni'hao"）。组合串是编码不是正文，
            // 失焦该丢（与服务端 FocusLost 清缓冲、macOS 失焦抹 marked text 一致）；其中待定
            // 标点那一截由 deactivate 里的 applyClearComposition 自己上屏，不靠这条路。
            text.append(utf8, {fcitx::TextFormatFlag::Underline, fcitx::TextFormatFlag::DontCommit});
            text.setCursor(int(caretBytes));
        }
        auto& panel = ic_->inputPanel();
        // 宿主支持内嵌预编辑就摆在光标处（与 Windows/macOS 的内嵌组合同观感）；不支持的
        // （部分 XIM 客户端）退到 Fcitx5 自己的输入面板——那需要 classicui 之类的 UI 插件在场。
        if (ic_->capabilityFlags().test(fcitx::CapabilityFlag::Preedit)) {
            panel.setClientPreedit(text);
            ic_->updatePreedit();
        } else {
            panel.setPreedit(text);
            ic_->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
        }
    }

    bool deleteBeforeCursor(size_t chars) override
    {
        if (!ic_->capabilityFlags().test(fcitx::CapabilityFlag::SurroundingText)) {
            return false;
        }
        ic_->deleteSurroundingText(-int(chars), unsigned(chars));
        return true;
    }

    void moveCursor(int chars) override
    {
        // 与 macOS 合成方向键同一手段：Fcitx5 没有「移动宿主光标」的通用 API。
        fcitx::Key key(chars < 0 ? FcitxKey_Left : FcitxKey_Right);
        int n = chars < 0 ? -chars : chars;
        for (int i = 0; i < n && i < 64; ++i) {
            ic_->forwardKey(key, false);
            ic_->forwardKey(key, true);
        }
    }

private:
    fcitx::InputContext* ic_;
};

bool isSwitchEvent(const fcitx::InputContextEvent& event)
{
    return event.type() == fcitx::EventType::InputContextSwitchInputMethod;
}

} // namespace

WindEngine::WindEngine(fcitx::Instance* instance) : instance_(instance)
{
    dispatcher_.attach(&instance_->eventLoop());
    keyHoldTimeoutMs_ = keyHoldTimeoutMs(std::getenv("WIND_KEY_HOLD_TIMEOUT_MS"));
    // 托盘运行时图标的目录要赶在各 UI 模块构建图标主题之前就在（见 HostUi.h ensureIconDirs）。
    if (!ensureIconDirs(userHicolorDir())) {
        WIND_WARN() << "用户图标目录建不起来，托盘退回随包的种子图标";
    }

    router_.setTimerScheduler([this](uint32_t ms, std::function<void()> cb) {
        holdTimer_ = instance_->eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + uint64_t(ms) * 1000, 0,
            [cb = std::move(cb)](fcitx::EventSourceTime*, uint64_t) {
                cb();
                return true;
            });
    });
    // 计时器到点时，只认「此刻仍有焦点的那个 IC」。焦点已走 ⇒ nullptr ⇒ router 只清本端状态。
    router_.setSinkProvider([this](const std::function<void(TextSink*)>& use) {
        if (fcitx::InputContext* ic = focusedIC()) {
            ICSink sink(ic);
            use(&sink);
        } else {
            use(nullptr);
        }
    });

    // 候选窗交互：鼠标事件在主线程（X 连接的 fd 挂在 Fcitx5 事件循环上），与按键不会交错，
    // 故直接复用按键那条请求连接。选词 / 翻页的结果经 push 通道异步回来。
    X11CandidatePanel::Callbacks cb;
    cb.select = [this](int32_t i) { sendAndDrain(encodeCandidateSelectFrame(i)); };
    cb.hover = [this](int32_t i) { sendAndDrain(encodeCandidateHoverFrame(i)); };
    cb.scroll = [this](int32_t d) { sendAndDrain(encodeCandidateScrollFrame(d)); };
    // 自绘菜单：右键候选窗请求开菜单；菜单打开期间的指针事件原样报给服务端（命中测试、高亮、
    // 子菜单都在服务端的 popup_menu 里）；本端自己收了菜单（空闲超时）要报，服务端才会复位
    // menu_open——否则它继续吞方向键 / 回车 / Esc。
    cb.contextMenu = [this](int32_t target, int32_t x, int32_t y) { requestMenu(target, x, y); };
    cb.menuPointer = [this](uint32_t e, uint32_t b, int32_t x, int32_t y) {
        // 报不上去（服务不在 / 不响应）：菜单是它画的、命中也在它那儿，本端留着菜单只剩
        // 「指针被抓着、整个桌面点不动」。就地收掉放开（不报：没人收）。
        if (!sendAndDrain(encodeMenuPointerFrame(e, b, x, y))) {
            WIND_WARN() << "菜单指针事件报不上去，本端收起菜单、放开指针";
            panel_->closeMenu(nullptr);
        }
    };
    cb.menuDismissed = [this](const std::string& reason) {
        sendAndDrain(encodeMenuDismissFrame(reason));
    };
    // 右键状态气泡 / 悬停提示：与候选右键同一条 `menu.open`，工作区随请求报上去；提示另带
    // 右键点在位图内的坐标（按段 / 按行的命中在服务端做）。
    cb.overlayMenu = [this](int32_t target, int32_t x, int32_t y, int32_t lx, int32_t ly) {
        Rect wa = panel_->screenWorkArea().value_or(Rect{});
        WIND_DEBUG() << "请求打开浮层菜单 target=" << target << " @(" << x << "," << y << ")";
        sendAndDrain(target == MENU_TARGET_TOOLTIP
                         ? encodeMenuOpenFrame(target, x, y, wa.x, wa.y, wa.x + wa.w,
                                               wa.y + wa.h, lx, ly)
                         : encodeMenuOpenFrame(target, x, y, wa.x, wa.y, wa.x + wa.w,
                                               wa.y + wa.h));
    };
    cb.statusMoved = [this](int32_t x, int32_t y) {
        sendAndDrain(encodePosFrame(EXT_KIND_POS_STATUS_TIP, x, y));
    };
    panel_ = std::make_unique<X11CandidatePanel>(instance_->eventLoop(), std::move(cb));

    settingsAction_.setShortText("清风输入法设置");
    settingsAction_.setIcon("preferences-system");
    settingsAction_.connect<fcitx::SimpleAction::Activated>(
        [this](fcitx::InputContext*) { launchSettings({}); });
    instance_->userInterfaceManager().registerAction("windinput-settings", &settingsAction_);

    push_ = std::make_unique<PushClient>(
        pushSocketPath(),
        [this](Frame f) {
            // 推送线程 → 主线程。
            dispatcher_.schedule([this, f = std::move(f)]() mutable { onPushFrame(std::move(f)); });
        },
        [this](bool connected) {
            // 服务没了（崩溃 / 被杀）：菜单是它画的，也没人再回应指针事件——就地收掉、放开
            // 指针。不报 dismiss（没人收）；新服务的 menu_open 本来就是 false。
            // key.hold 按住的键同理：要它 release 的那个服务已经不在了。
            if (!connected) {
                dispatcher_.schedule([this]() {
                    if (panel_) {
                        panel_->closeMenu(nullptr);
                    }
                    releaseHeldKeys("push_lost");
                });
            }
        });
    push_->start();
    // 引擎一加载就把服务带起来，让它趁用户还没开始打字时完成词库加载。
    ensureConnected();
    WIND_INFO() << "WindInput 引擎已加载，服务端点 " << requestSocketPath();
}

WindEngine::~WindEngine()
{
    // addon 卸载（fcitx5 退出 / 重载）时还按着的键：抬起，别把应用留在「Shift 按住」里。
    releaseHeldKeys("addon_unload");
    keyHoldExpiry_.reset();
    // 顺序要紧：先停推送线程（之后不再有 schedule），再拆 dispatcher。
    push_.reset();
    panel_.reset();
    holdTimer_.reset();
    dispatcher_.detach();
}

// ── 连接 ──────────────────────────────────────────────────────────────

bool WindEngine::ensureConnected()
{
    if (bridge_.isConnected()) {
        return true;
    }
    return reconnect();
}

bool WindEngine::reconnect()
{
    if (bridge_.connect(requestSocketPath())) {
        WIND_INFO() << "已连上服务";
        noteStall();
        if (stallPending_) {
            stallPending_ = false;
            resyncAfterStall();
        }
        return true;
    }
    switch (bridge_.lastFailure()) {
    case BridgeClient::Failure::Suspended:
        return false; // 熔断期内：这一键直接透传，不再付一次超时
    case BridgeClient::Failure::ConnectTimeout:
        // 服务在（socket 有人 listen）但不 accept：卡死，不是没起。拉起也无济于事（单例锁）。
        WIND_WARN() << "服务无响应（连接排队超时），按键暂时透传";
        noteStall();
        return false;
    default:
        break;
    }
    WIND_DEBUG() << "连不上服务: " << bridge_.lastError();
    // 服务没在跑（或刚被杀）：拉起它。本次仍返回 false（这一键透传），服务就绪后由下一次
    // 按键的重连接上；首次启动要建词库缓存，期间按键都会透传，这是已知代价。
    if (launcher_.maybeLaunch()) {
        WIND_INFO() << "服务未运行，已尝试拉起 " << servicePath();
    }
    return false;
}

bool WindEngine::requestWithRetry(const Bytes& frame, Frame& resp)
{
    if (windlinux::requestWithRetry(bridge_, frame, resp, [this] { return reconnect(); })) {
        return true;
    }
    if (bridge_.lastFailure() != BridgeClient::Failure::Suspended) {
        WIND_WARN() << "服务 I/O 失败（" << bridge_.lastError() << "），本键透传";
    }
    noteStall();
    return false;
}

bool WindEngine::sendAndDrain(const Bytes& frame)
{
    Frame ignored;
    if (!ensureConnected() || !bridge_.request(frame, ignored)) {
        WIND_DEBUG() << "发送失败: " << bridge_.lastError();
        noteStall();
        return false;
    }
    return true;
}

void WindEngine::sendAsync(const Bytes& frame)
{
    if (!ensureConnected() || !bridge_.send(frame)) {
        WIND_DEBUG() << "异步发送失败: " << bridge_.lastError();
        noteStall();
    }
}

void WindEngine::noteStall()
{
    if (!bridge_.takeStallFlag() || stallPending_) {
        return;
    }
    // 服务超时没回：之后一段时间按键直接透传（熔断）。本端的组字 / 候选窗 / 菜单都是那个
    // 服务状态的影子，原地收掉——否则应用里挂着一段预编辑，后面的字母却直通进了正文；
    // 菜单还抓着指针，整个桌面点不动。恢复后由 resyncAfterStall 把服务端也对齐。
    WIND_WARN() << "服务无响应：本端收起组字与候选窗，恢复前按键透传";
    stallPending_ = true;
    if (fcitx::InputContext* ic = focusedIC(); ic && router_.hasComposition()) {
        ICSink sink(ic);
        router_.applyClearComposition(&sink);
    } else {
        router_.reset();
    }
    // 菜单不报 dismiss（没人收）；服务端的 menu_open 由恢复时的 COMPOSITION_TERMINATED 复位。
    panel_->closeMenu(nullptr);
    panel_->hide();
    panel_->hideAllOverlays();
    releaseHeldKeys("service_stalled");
}

void WindEngine::resyncAfterStall()
{
    // 服务恢复后可能已把积在旧连接里的帧处理掉（超时不等于它没收到）：组字缓冲里多出
    // 本端不知道的码。按「组合被宿主终止」复位它（同时复位 menu_open），再重报焦点。
    WIND_INFO() << "服务恢复响应，对齐组字与焦点状态";
    sendAsync(encodeEmptyFrame(CMD_COMPOSITION_TERMINATED, true));
    if (fcitx::InputContext* ic = focusedIC()) {
        sendFocusGained(ic);
    }
}

// ── 焦点 ──────────────────────────────────────────────────────────────

uint64_t WindEngine::clientToken(fcitx::InputContext* ic) const
{
    // 高 32 位 = 宿主标识。Wayland / DBus 客户端拿不到可靠的 pid，与 macOS 同法取程序名的
    // 稳定散列（服务端只拿它做 pid_names 的键，散列满足「同宿主恒等、异宿主相异」）。
    // 低 32 位 = 这个输入上下文的身份：同一文本框重复聚焦得同一 token，换框则不同。
    uint32_t host = stableHash(ic->program());
    const auto& uuid = ic->uuid();
    std::string uuidBytes(uuid.begin(), uuid.end());
    uint32_t low = stableHash(uuidBytes);
    return (uint64_t(host) << 32) | low;
}

uint64_t WindEngine::inputScopeMask(fcitx::InputContext* ic) const
{
    // 密码框：客户端声明 Password 或 Sensitive 能力位（GTK/Qt 的密码输入框都会置）。
    // 服务端据 IS_PASSWORD 位对密码框强制英文半角直通，与 Windows/macOS 共用同一判定。
    return ic->capabilityFlags().testAny(fcitx::CapabilityFlag::PasswordOrSensitive)
        ? INPUT_SCOPE_PASSWORD_BIT
        : 0;
}

void WindEngine::sendFocusGained(fcitx::InputContext* ic)
{
    uint64_t mask = inputScopeMask(ic);
    lastReportedSecure_ = mask != 0;
    // 服务端对 FocusGained 回 MODE_PUSH（权威中英状态）：托盘图标据此对齐。
    Frame resp;
    if (ensureConnected()
        && bridge_.request(encodeFocusGainedFrame(clientToken(ic), mask, ic->program(), std::string()),
                           resp)) {
        noteMode(resp);
    } else {
        WIND_DEBUG() << "发送失败: " << bridge_.lastError();
        noteStall();
    }
}

void WindEngine::sendCaretUpdate(fcitx::InputContext* ic)
{
    const fcitx::Rect& r = ic->cursorRect();
    if (r.height() <= 0 && r.width() <= 0 && r.left() == 0 && r.top() == 0) {
        return; // 宿主没报过光标位置：宁可不报，也别把候选窗钉到屏幕左上角
    }
    // X11 客户端报的是根窗口坐标（物理像素、左上原点、y 向下），正是 wire 坐标系。
    // y 取行顶、height 取行高（对位 Swift `CaretCoords.caretRectToWire`）。
    int h = r.height() > 0 ? r.height() : 16;
    sendAndDrain(encodeCaretUpdateFrame(r.left(), r.top(), h));
}

uint16_t WindEngine::prevCharFor(fcitx::InputContext* ic)
{
    // 主通路：宿主支持 surrounding text 就如实报光标前那个字符（对位 Windows TSF 现读文档）。
    // 读不到才退回本端记账（对位 macOS 唯一的备用通路）。
    if (ic->capabilityFlags().test(fcitx::CapabilityFlag::SurroundingText)) {
        const auto& st = ic->surroundingText();
        if (st.isValid() && st.cursor() > 0 && st.anchor() == st.cursor()) {
            const std::string& text = st.text();
            size_t len = fcitx::utf8::length(text);
            if (st.cursor() <= len) {
                auto it = fcitx::utf8::nextNChar(text.begin(), st.cursor() - 1);
                uint32_t cp = fcitx::utf8::getChar(it, text.end());
                // 服务端的 prev_char 是 UTF-16 码元：BMP 外的字符报不出，按「不可用」处理。
                return cp < 0x10000 && cp != fcitx::utf8::INVALID_CHAR ? uint16_t(cp) : 0;
            }
        }
    }
    return router_.digitTracker().prevChar();
}

bool WindEngine::applyResponse(fcitx::InputContext* ic, const Frame& resp, bool hostShortcut)
{
    noteMode(resp);
    ICSink sink(ic);
    return router_.apply(resp, &sink, hostShortcut);
}

void WindEngine::noteMode(const Frame& frame)
{
    if (mode_.update(frame)) {
        refreshModeIcon();
    }
}

void WindEngine::refreshModeIcon()
{
    WIND_DEBUG() << "模式图标 → " << mode_.iconName() << "（" << mode_.label() << "）";
    // StatusArea 这一格的 UI 更新就是 Fcitx5 各 UI 模块重取输入法图标的信号（notificationitem
    // 发 NewIcon、kimpanel 重报属性、classicui 重画托盘）。
    if (fcitx::InputContext* ic = focusedIC()) {
        ic->updateUserInterface(fcitx::UserInterfaceComponent::StatusArea);
    }
}

std::string WindEngine::subMode(const fcitx::InputMethodEntry&, fcitx::InputContext&)
{
    return mode_.subModeName();
}

std::string WindEngine::subModeIconImpl(const fcitx::InputMethodEntry&, fcitx::InputContext&)
{
    return mode_.iconName();
}

std::string WindEngine::subModeLabelImpl(const fcitx::InputMethodEntry&, fcitx::InputContext&)
{
    return mode_.label();
}

void WindEngine::sendKeyUp(fcitx::InputContext* ic, uint32_t vk, uint32_t mods, uint8_t toggles)
{
    // 模式切换通常无组合，先刷新 caret 让状态气泡锚到当前插入点。
    sendCaretUpdate(ic);
    KeyEvent e;
    e.keyCode = vk;
    e.modifiers = mods;
    e.eventType = KEY_EVENT_UP; // 协调器只在 keyup 分支处理切换键与 CapsLock（TSF 惯例）
    e.toggles = toggles;
    e.eventSeq = ++keySeq_;
    e.prevChar = prevCharFor(ic);
    Frame resp;
    if (requestWithRetry(encodeKeyEventFrame(e), resp)) {
        applyResponse(ic, resp, false);
    }
}

void WindEngine::activate(const fcitx::InputMethodEntry&, fcitx::InputContextEvent& event)
{
    fcitx::InputContext* ic = event.inputContext();
    // 上一个 IC 还没失焦就有新 IC 获得焦点（没有焦点组的前端允许多个 IC 同时聚焦）：先替它
    // 收尾。否则它迟到的 FocusLost 会被服务端当作「旧宿主的陈旧失焦」丢弃（active token 已是
    // 新 IC），留在服务端缓冲里的旧编码就会接着拼进新文本框（实测「你好你好」）。
    if (fcitx::InputContext* old = currentIC_.get(); old && old != ic) {
        releaseHeldKeys("focus_changed");
        ICSink sink(old);
        router_.applyClearComposition(&sink);
        if (bridge_.isConnected()) {
            sendAndDrain(encodeFocusLostFrame(clientToken(old), FOCUS_LOST_REASON_THREAD));
        }
    }
    currentIC_ = ic->watch();
    // 状态区入口：InputMethod 组在切换输入法时由 Fcitx5 自己清空，每次激活挂一次。
    ic->statusArea().addAction(fcitx::StatusGroup::InputMethod, &settingsAction_);
    // 光标前字符的记账只对「这一个文本框里我们自己打出去的东西」有效，换焦点即作废。
    router_.reset();
    tap_.reset();
    caps_.reset();
    eatenKeys_.clear();
    if (!ensureConnected()) {
        return;
    }
    // 「切到本输入法」与「焦点进入文本框」在 Fcitx5 里都走 activate，事件类型把两者分开：
    // 前者对位 Windows 的 IME_ACTIVATED（服务端据此套用激活初始状态），后者只是焦点切换。
    // 每次焦点进入都报 IME_ACTIVATED 会让「不记忆中英状态」的用户每切一次窗口就被重置。
    if (isSwitchEvent(event)) {
        sendAsync(encodeClientTokenFrame(CMD_IME_ACTIVATED, clientToken(ic)));
    }
    sendFocusGained(ic);
    sendCaretUpdate(ic);
    sendAndDrain(encodeFrontContextFrame(ic->program(), std::string(), std::string()));
}

void WindEngine::deactivate(const fcitx::InputMethodEntry&, fcitx::InputContextEvent& event)
{
    fcitx::InputContext* ic = event.inputContext();
    // 按住的键先抬起，发给当初按下的那个 IC（通常就是正在失焦的这个）。
    releaseHeldKeys("focus_out");
    // 失焦 / 切走输入法：菜单随之收起（服务端也会在 FocusLost 上关，但它有 250ms 的「刚打开」
    // 守卫；本端这条无条件，且先于 FocusLost 报，服务端据此复位）。
    panel_->closeMenu("focus_out");
    // 失焦即清干净：残留的预编辑要从宿主里抹掉（待定标点转为提交），否则切回来时旧编码还挂着。
    if (router_.hasComposition()) {
        ICSink sink(ic);
        router_.applyClearComposition(&sink);
    } else {
        router_.reset();
    }
    ic->inputPanel().reset();
    ic->updatePreedit();
    ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
    eatenKeys_.clear();
    if (!bridge_.isConnected()) {
        return;
    }
    uint64_t token = clientToken(ic);
    sendAndDrain(encodeFocusLostFrame(token, FOCUS_LOST_REASON_THREAD));
    if (isSwitchEvent(event)) {
        sendAsync(encodeClientTokenFrame(CMD_IME_DEACTIVATED, token));
    }
    if (currentIC_.get() == ic) {
        currentIC_.unwatch();
    }
}

void WindEngine::reset(const fcitx::InputMethodEntry&, fcitx::InputContextEvent& event)
{
    // 宿主要求重置（鼠标点击挪了光标、宿主主动 reset）：对位 Windows 的「组合被意外终止」。
    fcitx::InputContext* ic = event.inputContext();
    releaseHeldKeys("reset");
    if (!router_.hasComposition()) {
        return;
    }
    ICSink sink(ic);
    router_.applyClearComposition(&sink);
    if (bridge_.isConnected()) {
        sendAsync(encodeEmptyFrame(CMD_COMPOSITION_TERMINATED, true));
    }
}

// ── 按键 ──────────────────────────────────────────────────────────────

void WindEngine::keyEvent(const fcitx::InputMethodEntry&, fcitx::KeyEvent& event)
{
    fcitx::InputContext* ic = event.inputContext();
    currentIC_ = ic->watch();
    const fcitx::Key& key = event.rawKey();
    uint32_t sym = uint32_t(key.sym());
    uint32_t states = uint32_t(key.states());
    int code = key.code() != 0 ? key.code() : -int(sym); // 无硬件键码（DBus 合成）时退到 keysym

    // 大写锁定：本端先判（托盘图标立即换，不等服务端），见 CapsLockTracker。
    const std::optional<bool> caps = caps_.onKey(sym, states, event.isRelease());
    if (caps && mode_.noteCapsLock(*caps)) {
        refreshModeIcon();
    }
    if (sym == uint32_t(FcitxKey_Caps_Lock)) {
        // 同 Windows：CapsLock 的按下不转发（锁定态由系统维护），松开时报一帧 VK_CAPITAL keyup
        // 带新的锁定态——服务端据此同步镜像、处理正在打的编码、弹状态气泡，并回 STATUS_UPDATE
        // （带 STATUS_CAPS_LOCK 与「A」标签）。键本身两个方向都交还宿主。
        if (!event.isRelease()) {
            tap_.onPress(sym, nowMs()); // Shift 按住时按 CapsLock：那次 Shift 不再是单击
        } else if (caps && ensureConnected()) {
            uint8_t toggles = statesToToggles(states) & ~TOGGLE_CAPSLOCK;
            sendKeyUp(ic, keysymToVK(sym), statesToModifiers(states),
                      *caps ? uint8_t(toggles | TOGGLE_CAPSLOCK) : toggles);
        }
        return;
    }

    if (event.isRelease()) {
        if (uint32_t vk = tap_.onRelease(sym, nowMs()); vk != 0) {
            // 干净的修饰键单击：切中英等。松开事件本身照常交给宿主（宿主要看得见修饰键）。
            if (ensureConnected()) {
                sendKeyUp(ic, vk, 0, statesToToggles(states));
            }
            return;
        }
        if (eatenKeys_.erase(code) > 0) {
            event.filterAndAccept();
        }
        return;
    }

    tap_.onPress(sym, nowMs());
    // 菜单开着时按键由服务端转给菜单（方向键 / 回车 / Esc / 其它键关菜单并吞掉）：
    // 键盘操作同样算菜单上的活动，空闲计时重新起算。
    panel_->noteMenuActivity();
    // 修饰键本身的按下不上报（Windows 吃掉切换键的 keydown、macOS 走 flagsChanged 不发）：
    // 单击判定在上面，组合键的修饰状态随下一个键的 states 一起到。
    if (key.isModifier()) {
        return;
    }
    uint32_t vk = keysymToVK(sym);
    uint32_t mods = statesToModifiers(states);
    if (vk == 0) {
        router_.digitTracker().noteKeyPassthrough(0, mods);
        return; // 未覆盖的键：交还宿主
    }
    if (!ensureConnected()) {
        router_.digitTracker().noteKeyPassthrough(vk, mods);
        return; // 服务不在：直通，下一键再试
    }

    // 密码框实时跟随：同一 IC 内能力位翻转（网页里普通框 ↔ 密码框）不会触发 activate，
    // 在发本键前补报一次焦点，让服务端处理本键时密码态已最新（同连接串行，顺序有保证）。
    bool secure = inputScopeMask(ic) != 0;
    if (secure != lastReportedSecure_) {
        sendFocusGained(ic);
    }
    // 无组合时本端 caret 可能是上一次组字的旧位置，先刷新，让状态气泡/首帧候选锚对地方。
    if (!router_.hasComposition()) {
        sendCaretUpdate(ic);
    }

    KeyEvent e;
    e.keyCode = vk;
    e.modifiers = mods;
    e.eventType = KEY_EVENT_DOWN;
    e.toggles = statesToToggles(states);
    e.eventSeq = ++keySeq_;
    e.prevChar = prevCharFor(ic); // 必须在处理本键之前取
    Frame resp;
    if (!requestWithRetry(encodeKeyEventFrame(e), resp)) {
        router_.digitTracker().noteKeyPassthrough(vk, mods);
        return;
    }
    bool consumed = applyResponse(ic, resp, isHostShortcut(states));
    WIND_DEBUG() << "key vk=" << hex(vk) << " mods=" << hex(mods) << " resp=" << hex(resp.cmd)
                 << (consumed ? " 吃" : " 透传");
    if (router_.hasComposition()) {
        sendCaretUpdate(ic);
    }
    if (consumed) {
        eatenKeys_.insert(code);
        event.filterAndAccept();
    } else {
        router_.digitTracker().noteKeyPassthrough(vk, mods);
    }
}

// ── push ──────────────────────────────────────────────────────────────

fcitx::InputContext* WindEngine::focusedIC()
{
    fcitx::InputContext* ic = currentIC_.get();
    return (ic && ic->hasFocus()) ? ic : nullptr;
}

void WindEngine::onPushFrame(Frame frame)
{
    switch (frame.cmd) {
    case CMD_SERVICE_READY:
        // 服务（重）启：它丢了全部焦点状态。有焦点就重报一次，免得「服务重启后第一段输入
        // 不认宿主 / 密码框」；请求连接此时多半已是死连接，顺手换新。
        // 服务重启会 shm_unlink + 重建 SHM 段（新 inode）：旧映射成了孤儿，候选窗会卡在旧帧。
        shm_.close();
        for (auto& r : overlayShm_) {
            r.close();
        }
        for (auto& r : menuShm_) {
            r.close();
        }
        panel_->hide();
        panel_->hideAllOverlays();
        // 旧服务 hold 的键没人再来 release 了。
        releaseHeldKeys("service_ready");
        {
            // 菜单是旧服务画的：收掉、放开指针。新服务的 menu_open 本就是 false，仍补报一次
            // dismiss——两端对齐不靠「恰好一致」。
            const bool hadMenu = panel_->menuOpen();
            panel_->closeMenu(nullptr);
            if (serviceSeenOnce_) {
                WIND_INFO() << "服务已重启，重建连接";
                bridge_.close();
                if (fcitx::InputContext* ic = focusedIC(); ic && reconnect()) {
                    sendFocusGained(ic);
                }
            }
            if (hadMenu) {
                sendAndDrain(encodeMenuDismissFrame("service_ready"));
            }
        }
        serviceSeenOnce_ = true;
        break;
    case CMD_STATE_PUSH:
    case CMD_ACTIVATION_STATUS_PUSH:
    case CMD_MODE_PUSH:
        // 别处改了模式（菜单点「英文」、设置程序改配置）：只刷托盘图标。
        noteMode(frame);
        break;
    case CMD_COMMIT_TEXT:
    case CMD_UPDATE_COMPOSITION:
    case CMD_CLEAR_COMPOSITION:
    case CMD_KEY_TYPE:
        // 鼠标选词的上屏 / 组合更新、命令直通车的 key.type 都经 push 异步到达，落到当前焦点。
        if (fcitx::InputContext* ic = focusedIC()) {
            applyResponse(ic, frame, false);
        } else {
            WIND_DEBUG() << "push cmd=" << hex(frame.cmd) << " 无焦点 IC，丢弃";
        }
        break;
    case CMD_KEY_TAP:
    case CMD_KEY_SEQ:
    case CMD_KEY_HOLD:
    case CMD_KEY_RELEASE:
        onKeySynth(frame);
        break;
    case CMD_HOST_RENDER_FRAME:
        if (auto p = decodeHostRenderFrame(frame.payload)) {
            onRenderFrame(*p);
        }
        break;
    case CMD_CANDIDATE_RECTS:
        if (auto rects = decodeCandidateRects(frame.payload)) {
            panel_->setRects(std::move(*rects));
        }
        break;
    case CMD_OVERLAY_FRAME:
        if (auto p = decodeOverlayFrame(frame.payload)) {
            onOverlayFrame(*p);
        }
        break;
    case CMD_EXT:
        if (auto ext = decodeExt(frame.payload)) {
            onExt(*ext);
        }
        break;
    default:
        // macOS 的文本提示帧（CMD_TOOLTIP_SHOW 等，Linux 服务不发）：见 AGENTS.md 差距表。
        break;
    }
}

void WindEngine::onRenderFrame(const HostRenderFramePayload& p)
{
    if (!p.visible() || p.width == 0 || p.height == 0) {
        panel_->hide();
        return;
    }
    if (!shm_.isOpen() && !shm_.open(shmName())) {
        WIND_WARN() << "打不开候选窗共享内存 " << shmName();
        return;
    }
    SharedFrame f;
    if (!shm_.snapshot(f) || f.bgra.empty()) {
        WIND_DEBUG() << "候选帧 seq=" << p.seq << " 读取失败或为空";
        return;
    }
    // 通知与 SHM 之间没有锁：读到的可能已是更新的一帧（服务端连推两帧、我们只赶上第二帧的
    // 像素）。像素与头部同帧即可显示，坐标取 SHM 头里的——与像素配套的那一份。
    if (p.scale > 1) {
        WIND_DEBUG() << "候选帧 scale=" << p.scale << "：X11 下按物理像素原样贴";
    }
    panel_->show(f, f.screenX, f.screenY, (p.flags & FRAME_FLAG_ABSOLUTE_POS) != 0);
}

void WindEngine::onExt(const ExtEnvelope& ext)
{
    if (ext.kind == EXT_KIND_POS_STATUS_TIP_QUERY) {
        // 切「固定位置」时服务端问气泡此刻在哪，好以当前位置落盘。不在屏上不答（服务端保留旧值）。
        if (auto at = panel_->statusContentOrigin()) {
            sendAndDrain(encodePosFrame(EXT_KIND_POS_STATUS_TIP, at->first, at->second));
        }
        return;
    }
    if (ext.kind != EXT_KIND_SETTINGS_OPEN) {
        // 未知 kind 安静忽略：新服务配旧 addon 时不该出错（信封的版本兼容语义）。
        WIND_DEBUG() << "未处理的扩展信封 kind=" << ext.kind;
        return;
    }
    auto args = parseSettingsOpenArgs(std::string(ext.body.begin(), ext.body.end()));
    if (!args) {
        WIND_WARN() << "settings.open 的参数解析失败，不启动设置程序";
        return;
    }
    launchSettings(*args);
}

void WindEngine::launchSettings(const std::vector<std::string>& args)
{
    std::vector<std::string> argv = settingsArgv(args);
    if (access(argv[0].c_str(), X_OK) != 0) {
        WIND_WARN() << "设置程序不存在或不可执行：" << argv[0];
        return;
    }
    // startProcess 双 fork + setsid：设置程序脱离 fcitx5 的进程组、不留僵尸，继承 fcitx5 的
    // 环境（DISPLAY / WAYLAND_DISPLAY 靠它带过去）。已在运行时由设置程序自己的单实例
    // 转发把 argv 交给首实例（windui single_instance/unix.rs），这里不必判重。
    WIND_INFO() << "启动设置程序 " << argv[0] << "（" << args.size() << " 个参数）";
    const std::string dir = argv[0].substr(0, argv[0].find_last_of('/') + 1);
    fcitx::startProcess(argv, dir.empty() ? "/" : dir);
}

void WindEngine::onOverlayFrame(const OverlayFramePayload& p)
{
    if (int level = menuLevelOfKind(p.kind); level >= 0) {
        onMenuFrame(uint32_t(level), p);
        return;
    }
    if (p.kind < 1 || p.kind > 3) {
        WIND_DEBUG() << "未知浮层 kind=" << p.kind << "，忽略";
        return;
    }
    if (!p.visible() || p.width == 0 || p.height == 0) {
        panel_->hideOverlay(p.kind);
        return;
    }
    ShmFrameReader& shm = overlayShm_[p.kind - 1];
    const std::string name = overlayShmName(p.kind);
    if (!shm.isOpen() && !shm.open(name)) {
        WIND_WARN() << "打不开浮层共享内存 " << name;
        return;
    }
    SharedFrame f;
    if (!shm.snapshot(f) || f.bgra.empty()) {
        WIND_DEBUG() << "浮层 kind=" << p.kind << " seq=" << p.seq << " 读取失败或为空";
        return;
    }
    panel_->showOverlay(p.kind, f, p);
}

void WindEngine::onMenuFrame(uint32_t level, const OverlayFramePayload& p)
{
    if (!p.visible() || p.width == 0 || p.height == 0) {
        panel_->hideMenuLevel(level);
        return;
    }
    ShmFrameReader& shm = menuShm_[level];
    const std::string name = overlayShmName(p.kind);
    if (!shm.isOpen() && !shm.open(name)) {
        WIND_WARN() << "打不开菜单共享内存 " << name;
        return;
    }
    SharedFrame f;
    if (!shm.snapshot(f) || f.bgra.empty()) {
        WIND_DEBUG() << "菜单第 " << level << " 级 seq=" << p.seq << " 读取失败或为空";
        return;
    }
    panel_->showMenuLevel(level, f, p);
}

// ── 命令直通车按键合成 ────────────────────────────────────────────────

namespace {

/// 日志里的组合写法（`ctrl+shift+end`）。
std::string describeCombo(const KeyComboPayload& c)
{
    std::string s;
    for (const auto& m : c.mods) {
        s += m + "+";
    }
    return s + c.key;
}

} // namespace

void WindEngine::onKeySynth(const Frame& frame)
{
    // 与 CMD_KEY_TYPE 同：落到此刻的焦点 IC，没有就丢弃（服务端只投给活跃客户端，焦点
    // 刚走的空窗里到达的帧没有合适的去处）。
    fcitx::InputContext* ic = focusedIC();
    if (!ic) {
        WIND_DEBUG() << "按键合成 cmd=" << hex(frame.cmd) << " 无焦点 IC，丢弃";
        return;
    }
    // 按住的键属于别的 IC（焦点切换时本该已抬起，这里兜底）：先抬起，再在新 IC 上干活。
    if (!keyHolds_.empty() && keyHoldIC_.get() != ic) {
        releaseHeldKeys("ic_changed");
    }
    const uint64_t now = nowMs();

    if (frame.cmd == CMD_KEY_SEQ) {
        auto seq = decodeKeySeq(frame.payload);
        if (!seq) {
            WIND_WARN() << "key.seq 帧解不开，丢弃";
            return;
        }
        if (seq->size() > kMaxSeqCombos) {
            WIND_WARN() << "key.seq 有 " << seq->size() << " 个组合，超过上限 " << kMaxSeqCombos
                        << "，整条丢弃";
            return;
        }
        // 先全部解析再动手：有一个不认识就整条不做（「删行」只做一半比不做更糟）。
        std::vector<SynthKey> events;
        for (const auto& c : *seq) {
            auto r = resolveCombo(c);
            if (!r) {
                WIND_WARN() << "key.seq 里有不认识的组合 " << describeCombo(c) << "，整条丢弃";
                return;
            }
            auto t = tapEvents(*r, keyHolds_.heldStates());
            events.insert(events.end(), t.begin(), t.end());
        }
        if (!synthRate_.allow(events.size(), now)) {
            WIND_WARN() << "按键合成过于频繁（每 " << kSynthWindowMs << "ms 至多 "
                        << kMaxSynthEventsPerWindow << " 个事件），丢弃 key.seq";
            return;
        }
        forwardKeys(ic, events);
        return;
    }

    auto payload = decodeKeyCombo(frame.payload);
    if (!payload) {
        WIND_WARN() << "按键合成帧 cmd=" << hex(frame.cmd) << " 解不开，丢弃";
        return;
    }
    auto combo = resolveCombo(*payload);
    if (!combo) {
        WIND_WARN() << "按键合成：不认识的组合 " << describeCombo(*payload) << "，丢弃";
        return;
    }
    switch (frame.cmd) {
    case CMD_KEY_TAP: {
        auto events = tapEvents(*combo, keyHolds_.heldStates());
        if (!synthRate_.allow(events.size(), now)) {
            WIND_WARN() << "按键合成过于频繁，丢弃 key.tap " << describeCombo(*payload);
            return;
        }
        forwardKeys(ic, events);
        break;
    }
    case CMD_KEY_HOLD: {
        if (!synthRate_.allow(combo->mods.size() + 1, now)) {
            WIND_WARN() << "按键合成过于频繁，丢弃 key.hold " << describeCombo(*payload);
            return;
        }
        std::vector<SynthKey> down;
        switch (keyHolds_.hold(*combo, now, keyHoldTimeoutMs_, down)) {
        case KeyHoldTracker::HoldResult::AlreadyHeld:
            WIND_DEBUG() << "key.hold " << describeCombo(*payload) << " 已按住，不重复按";
            return;
        case KeyHoldTracker::HoldResult::Full:
            WIND_WARN() << "已按住 " << kMaxHeldCombos << " 个组合，拒绝 key.hold "
                        << describeCombo(*payload);
            return;
        case KeyHoldTracker::HoldResult::Pressed:
            break;
        }
        keyHoldIC_ = ic->watch();
        forwardKeys(ic, down);
        armKeyHoldExpiry();
        break;
    }
    case CMD_KEY_RELEASE: {
        auto up = keyHolds_.release(*combo);
        if (up.empty()) {
            WIND_DEBUG() << "key.release " << describeCombo(*payload) << "：没有按住，忽略";
            return;
        }
        forwardKeys(ic, up);
        if (keyHolds_.empty()) {
            keyHoldIC_.unwatch();
        }
        break;
    }
    default:
        break;
    }
}

void WindEngine::forwardKeys(fcitx::InputContext* ic, const std::vector<SynthKey>& keys)
{
    // 键码填 0：各前端（XIM / Wayland / GTK·Qt 模块）按 keysym 在当前键盘布局里反查键码。
    for (const auto& k : keys) {
        ic->forwardKey(fcitx::Key(static_cast<fcitx::KeySym>(k.sym), fcitx::KeyStates(k.states)),
                       k.release);
    }
    WIND_DEBUG() << "按键合成 " << keys.size() << " 个事件 → " << ic->program();
}

void WindEngine::releaseHeldKeys(const char* reason)
{
    if (keyHolds_.empty()) {
        return;
    }
    std::vector<SynthKey> up = keyHolds_.releaseAll();
    if (fcitx::InputContext* ic = keyHoldIC_.get()) {
        WIND_INFO() << "补发 key.hold 按住键的抬起（" << reason << "）";
        forwardKeys(ic, up);
    } else {
        WIND_INFO() << "key.hold 按住的键所在 IC 已销毁，无处补发抬起（" << reason << "）";
    }
    keyHoldIC_.unwatch();
    if (keyHoldExpiry_) {
        keyHoldExpiry_->setEnabled(false);
    }
}

void WindEngine::armKeyHoldExpiry()
{
    std::optional<uint64_t> deadline = keyHolds_.nextDeadline();
    if (!deadline) {
        if (keyHoldExpiry_) {
            keyHoldExpiry_->setEnabled(false);
        }
        return;
    }
    const uint64_t now = nowMs();
    const uint64_t at = fcitx::now(CLOCK_MONOTONIC) + (*deadline > now ? *deadline - now : 0) * 1000;
    if (keyHoldExpiry_) {
        // 复用同一个事件源：回调里重设自己是安全的，销毁自己则不是。
        keyHoldExpiry_->setTime(at);
        keyHoldExpiry_->setOneShot();
        return;
    }
    keyHoldExpiry_ = instance_->eventLoop().addTimeEvent(
        CLOCK_MONOTONIC, at, 0, [this](fcitx::EventSourceTime*, uint64_t) {
            std::vector<SynthKey> up = keyHolds_.releaseExpired(nowMs());
            if (!up.empty()) {
                if (fcitx::InputContext* ic = keyHoldIC_.get()) {
                    WIND_INFO() << "key.hold 超过最长保持时间 " << keyHoldTimeoutMs_
                                << "ms，自动抬起";
                    forwardKeys(ic, up);
                }
            }
            if (keyHolds_.empty()) {
                keyHoldIC_.unwatch();
            }
            armKeyHoldExpiry();
            return true;
        });
}

void WindEngine::requestMenu(int32_t target, int32_t x, int32_t y)
{
    // 工作区随请求报上去：菜单的翻转 / 子菜单左右展开在服务端做，屏幕几何只有这边拿得到。
    Rect wa = panel_->screenWorkArea().value_or(Rect{});
    WIND_DEBUG() << "请求打开菜单 target=" << target << " @(" << x << "," << y << ")";
    sendAndDrain(encodeMenuOpenFrame(target, x, y, wa.x, wa.y, wa.x + wa.w, wa.y + wa.h));
}

} // namespace windlinux

FCITX_ADDON_FACTORY(windlinux::WindEngineFactory);
