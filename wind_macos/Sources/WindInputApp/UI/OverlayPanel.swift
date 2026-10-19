import Cocoa
import WindInputKit

// OverlayPanel — 一层光栅浮层（状态气泡 / Toast / 悬停提示 / 自绘菜单的一级）的窗口。
//
// 只贴图：像素是服务进程按主题画好、经各层独立的 SHM 段推来的，这里不排字、不上色，
// 所以主题里的背景图 / 渐变 / 图层 / 软阴影 / 边框 / 字号偏移在 macOS 上与 Windows、Linux
// 完全一致。落位、计时、鼠标转发由 `OverlayHost` 做。
//
// 窗口属性与候选窗同一套（见 `CandidatePanel.init` 的说明）：borderless + nonactivating、
// 层级 `.popUpMenu`（压得住 Spotlight 等高层宿主）、跟随用户切 Space、点击不抢键盘焦点。

/// 鼠标事件的统一出口。坐标：`wire*` 是 wire 点（主屏左上原点、y 向下）；`local*` 是窗口内的
/// **位图像素**（含软阴影扩边，= 视图点 × scale），悬停提示的右键命中按它算。
protocol OverlayPanelDelegate: AnyObject {
    func overlayMouseEntered(_ panel: OverlayPanel)
    func overlayMouseExited(_ panel: OverlayPanel)
    func overlayMouseMoved(_ panel: OverlayPanel, wireX: CGFloat, wireY: CGFloat)
    func overlayMouseDown(_ panel: OverlayPanel, button: Int, wireX: CGFloat, wireY: CGFloat,
                          localX: CGFloat, localY: CGFloat)
    func overlayMouseDragged(_ panel: OverlayPanel, wireX: CGFloat, wireY: CGFloat)
    func overlayMouseUp(_ panel: OverlayPanel, wireX: CGFloat, wireY: CGFloat)
}

final class OverlayContentView: NSView {
    private var image: NSImage?
    weak var owner: OverlayPanel?

    override var isFlipped: Bool { true }   // 左上原点，与位图行序一致
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    func setImage(_ img: NSImage) {
        image = img
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        // isFlipped=true 时 NSImage.draw(in:) 能正确补偿坐标系翻转（与候选窗同一做法）。
        image?.draw(in: bounds)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: .zero,
                                       options: [.activeAlways, .mouseMoved, .mouseEnteredAndExited, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    /// 当前鼠标的 wire 坐标（全局，不依赖事件所在窗口——拖动时指针早已出了本窗口）。
    private var wirePoint: (CGFloat, CGFloat) {
        let m = NSEvent.mouseLocation
        return (m.x, WireGeometry.flipY(m.y, screenHeight: PanelGeometry.referenceHeight))
    }

    private func local(_ event: NSEvent) -> (CGFloat, CGFloat) {
        let p = convert(event.locationInWindow, from: nil)
        let s = CGFloat(owner?.scale ?? 1)
        return (p.x * s, p.y * s)
    }

    override func mouseEntered(with event: NSEvent) {
        guard let o = owner else { return }
        o.panelDelegate?.overlayMouseEntered(o)
    }

    override func mouseExited(with event: NSEvent) {
        guard let o = owner else { return }
        o.panelDelegate?.overlayMouseExited(o)
    }

    override func mouseMoved(with event: NSEvent) {
        guard let o = owner else { return }
        let (x, y) = wirePoint
        o.panelDelegate?.overlayMouseMoved(o, wireX: x, wireY: y)
    }

    private func down(_ event: NSEvent, button: Int) {
        guard let o = owner else { return }
        let (x, y) = wirePoint
        let (lx, ly) = local(event)
        o.panelDelegate?.overlayMouseDown(o, button: button, wireX: x, wireY: y, localX: lx, localY: ly)
    }

    // AppKit 的 buttonNumber 0/1/2 → X11 约定 1 左 / 3 右 / 2 中（与 `OverlayInput.pressAction` 同一套）。
    override func mouseDown(with event: NSEvent) { down(event, button: 1) }
    override func rightMouseDown(with event: NSEvent) { down(event, button: 3) }
    override func otherMouseDown(with event: NSEvent) { down(event, button: 2) }

    override func mouseDragged(with event: NSEvent) {
        guard let o = owner else { return }
        let (x, y) = wirePoint
        o.panelDelegate?.overlayMouseDragged(o, wireX: x, wireY: y)
    }

    override func mouseUp(with event: NSEvent) {
        guard let o = owner else { return }
        let (x, y) = wirePoint
        o.panelDelegate?.overlayMouseUp(o, wireX: x, wireY: y)
    }
}

final class OverlayPanel: NSPanel {
    let kind: UInt32
    weak var panelDelegate: OverlayPanelDelegate?
    /// 最近一帧的元数据（落位 / 命中 / 拖动都要用它的内容盒偏移与尺寸）。
    private(set) var frameInfo: OverlayFrame?
    /// 最近一次摆放的 wire 矩形（窗口，含软阴影扩边）。
    private(set) var wireRect: WireRect?
    private let content = OverlayContentView()

    var scale: Float { frameInfo?.scale ?? 1 }

    init(kind: UInt32) {
        self.kind = kind
        super.init(contentRect: NSRect(x: 0, y: 0, width: 40, height: 20),
                   styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered,
                   defer: false)
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false   // 主题要阴影就已经画进位图
        // 菜单要压过同层的状态气泡 / 提示与菜单捕获窗，故高两级；其余与候选窗一致（见 CandidatePanel.init 的说明，
        // 别再设 isFloatingPanel——它会把 level 冲回 3）。
        level = OverlayKind.isMenu(kind)
            ? NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 2)
            : .popUpMenu
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        // 菜单各级不收鼠标：指针事件统一由全屏的捕获窗接，命中测试在服务端做。
        ignoresMouseEvents = OverlayKind.isMenu(kind)
        content.owner = self
        contentView = content
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// 贴一帧并摆到 wire 矩形处。
    func present(frame f: OverlayFrame, image: NSImage, wireRect r: WireRect) {
        frameInfo = f
        wireRect = r
        content.frame = NSRect(origin: .zero, size: image.size)
        content.setImage(image)
        setContentSize(image.size)
        let origin = WireGeometry.cocoaOrigin(wireX: r.x, wireY: r.y, size: image.size,
                                              screenHeight: PanelGeometry.referenceHeight)
        setFrameOrigin(origin)
        // 不叠系统阴影：主题要阴影就已经画进位图（软阴影），不要就是不要，两平台同一外观。
        orderFrontRegardless()
    }

    /// 拖动中就地挪窗口（不换像素）。
    func move(to r: WireRect) {
        wireRect = r
        let origin = WireGeometry.cocoaOrigin(wireX: r.x, wireY: r.y, size: frame.size,
                                              screenHeight: PanelGeometry.referenceHeight)
        setFrameOrigin(origin)
    }

    func hidePanel() {
        frameInfo = nil
        wireRect = nil
        orderOut(nil)
    }

    /// 内容盒（用户视觉上看到的窗口边缘）左上角的 wire 坐标；不可见时 nil。
    /// 固定位置落盘记的是它，与候选窗 / 配置里 `custom_x/y` 同义。
    func contentTopLeftWire() -> (x: Int32, y: Int32)? {
        guard isVisible, let r = wireRect, let f = frameInfo else { return nil }
        let m = OverlayPlacer.metrics(of: f)
        return (Int32((r.x + m.contentX).rounded()), Int32((r.y + m.contentY).rounded()))
    }

    /// 指针（wire 坐标）是否落在窗口矩形内。
    func containsWire(_ x: CGFloat, _ y: CGFloat) -> Bool {
        isVisible && (wireRect?.contains(x, y) ?? false)
    }
}
