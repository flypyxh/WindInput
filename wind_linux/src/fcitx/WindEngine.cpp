#include "WindEngine.h"

#include "ExtProtocol.h"
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
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

#include <chrono>
#include <cstdio>
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

    void setPreedit(const std::string& utf8, size_t caretBytes) override
    {
        fcitx::Text text;
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
    panel_ = std::make_unique<X11CandidatePanel>(instance_->eventLoop(), std::move(cb));

    push_ = std::make_unique<PushClient>(pushSocketPath(), [this](Frame f) {
        // 推送线程 → 主线程。
        dispatcher_.schedule([this, f = std::move(f)]() mutable { onPushFrame(std::move(f)); });
    });
    push_->start();
    // 引擎一加载就把服务带起来，让它趁用户还没开始打字时完成词库加载。
    ensureConnected();
    WIND_INFO() << "WindInput 引擎已加载，服务端点 " << requestSocketPath();
}

WindEngine::~WindEngine()
{
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
        return true;
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
    if (ensureConnected() && bridge_.request(frame, resp)) {
        return true;
    }
    WIND_WARN() << "服务 I/O 失败（" << bridge_.lastError() << "），重连后重试本帧";
    return reconnect() && bridge_.request(frame, resp);
}

void WindEngine::sendAndDrain(const Bytes& frame)
{
    Frame ignored;
    if (!ensureConnected() || !bridge_.request(frame, ignored)) {
        WIND_DEBUG() << "发送失败: " << bridge_.lastError();
    }
}

void WindEngine::sendAsync(const Bytes& frame)
{
    if (!ensureConnected() || !bridge_.send(frame)) {
        WIND_DEBUG() << "异步发送失败: " << bridge_.lastError();
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
    // 服务端对 FocusGained 回 MODE_PUSH（权威中英状态）；本端没有要用它的地方，读掉即可。
    sendAndDrain(encodeFocusGainedFrame(clientToken(ic), mask, ic->program(), std::string()));
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
    ICSink sink(ic);
    return router_.apply(resp, &sink, hostShortcut);
}

void WindEngine::sendModifierTap(fcitx::InputContext* ic, uint32_t vk)
{
    // 模式切换通常无组合，先刷新 caret 让状态气泡锚到当前插入点。
    sendCaretUpdate(ic);
    KeyEvent e;
    e.keyCode = vk;
    e.eventType = KEY_EVENT_UP; // 协调器只在 keyup 分支处理切换键（TSF 惯例）
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
        ICSink sink(old);
        router_.applyClearComposition(&sink);
        if (bridge_.isConnected()) {
            sendAndDrain(encodeFocusLostFrame(clientToken(old), FOCUS_LOST_REASON_THREAD));
        }
    }
    currentIC_ = ic->watch();
    // 光标前字符的记账只对「这一个文本框里我们自己打出去的东西」有效，换焦点即作废。
    router_.reset();
    tap_.reset();
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

    if (event.isRelease()) {
        if (uint32_t vk = tap_.onRelease(sym, nowMs()); vk != 0) {
            // 干净的修饰键单击：切中英等。松开事件本身照常交给宿主（宿主要看得见修饰键）。
            sendModifierTap(ic, vk);
            return;
        }
        if (eatenKeys_.erase(code) > 0) {
            event.filterAndAccept();
        }
        return;
    }

    tap_.onPress(sym, nowMs());
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
        panel_->hide();
        panel_->hideAllOverlays();
        if (serviceSeenOnce_) {
            WIND_INFO() << "服务已重启，重建连接";
            bridge_.close();
            if (fcitx::InputContext* ic = focusedIC(); ic && reconnect()) {
                sendFocusGained(ic);
            }
        }
        serviceSeenOnce_ = true;
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
        // macOS 的文本提示帧（CMD_TOOLTIP_SHOW 等，Linux 服务不发）/ 按键合成：见 AGENTS.md 差距表。
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
    std::vector<std::string> argv = settingsArgv(*args);
    if (access(argv[0].c_str(), X_OK) != 0) {
        WIND_WARN() << "设置程序不存在或不可执行：" << argv[0];
        return;
    }
    // startProcess 双 fork + setsid：设置程序脱离 fcitx5 的进程组、不留僵尸，继承 fcitx5 的
    // 环境（DISPLAY / WAYLAND_DISPLAY 靠它带过去）。已在运行时由设置程序自己的单实例
    // 转发把 argv 交给首实例（windui single_instance/unix.rs），这里不必判重。
    WIND_INFO() << "启动设置程序 " << argv[0] << "（" << args->size() << " 个参数）";
    const std::string dir = argv[0].substr(0, argv[0].find_last_of('/') + 1);
    fcitx::startProcess(argv, dir.empty() ? "/" : dir);
}

void WindEngine::onOverlayFrame(const OverlayFramePayload& p)
{
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

} // namespace windlinux

FCITX_ADDON_FACTORY(windlinux::WindEngineFactory);
