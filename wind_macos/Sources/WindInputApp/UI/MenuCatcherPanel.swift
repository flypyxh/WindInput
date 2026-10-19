import Cocoa
import WindInputKit

// MenuCatcherPanel — 自绘菜单打开期间接管鼠标的全屏透明窗。
//
// 自绘菜单的命中测试、悬停高亮、子菜单展开全在服务端（`popup_menu`），`.app` 只贴图——
// 所以菜单开着时指针要原样报给服务端。Linux addon 用 X11 指针抓取（pointer grab）做到这一点；
// macOS 没有这种东西，用一块盖住所有屏幕的几乎全透明窗代替：
//
//   - 层级比 `.popUpMenu` 高一级（压过候选窗 / 气泡 / 提示），菜单各级窗口再高一级、且不收鼠标
//     （`ignoresMouseEvents`），于是菜单里外的指针事件一律落到本窗；
//   - 点菜单外 = 点到本窗，被它吃掉并报给服务端（服务端据此关菜单），与 NSMenu「点外面只关菜单、
//     不把这一下传给底下的应用」一致；
//   - nonactivating + 不成为 key：不抢宿主的键盘焦点，按键仍经 IMK 送到服务端。
//
// alpha 取 0.01 而不是 0：完全透明的窗口会被 WindowServer 当作不存在，收不到任何鼠标事件。
//
// **每块屏一个**：系统设置「显示器具有单独的空间」（默认开）下窗口不能跨显示器，一个并集大窗
// 只会出现在其中一块屏上，另一块屏上点菜单外就关不掉菜单。

final class MenuCatcherPanel: NSPanel {
    /// `(event, button, wireX, wireY)`：event 1 移动 / 2 按下；button 1 左 / 2 中 / 3 右，移动时 0。
    var onPointer: ((UInt32, UInt32, CGFloat, CGFloat) -> Void)?

    private let catcher = CatcherView()

    init() {
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isOpaque = false
        backgroundColor = NSColor(calibratedWhite: 0, alpha: 0.01)
        hasShadow = false
        // 比候选窗 / 气泡 / 提示（`.popUpMenu`）高一级：它们每次重绘都会 orderFront，不抬高的话
        // 菜单开着时被重绘的窗口会盖到捕获窗上面，点那一块就去选词 / 拖动而不是关菜单。
        // 菜单各级窗口（`OverlayPanel`）再高一级。
        level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        acceptsMouseMovedEvents = true
        ignoresMouseEvents = false
        catcher.owner = self
        contentView = catcher
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// 盖住指定屏幕。屏幕布局在菜单开着期间可能变，所以每次显示都按当前屏幕重建。
    func cover(_ screen: NSScreen) {
        setFrame(screen.frame, display: false)
        catcher.frame = NSRect(origin: .zero, size: screen.frame.size)
        orderFrontRegardless()
    }

    func uncover() {
        orderOut(nil)
    }

    fileprivate func report(_ event: UInt32, _ button: UInt32) {
        let m = NSEvent.mouseLocation
        onPointer?(event, button, m.x, WireGeometry.flipY(m.y, screenHeight: PanelGeometry.referenceHeight))
    }
}

private final class CatcherView: NSView {
    weak var owner: MenuCatcherPanel?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: .zero,
                                       options: [.activeAlways, .mouseMoved, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) { owner?.report(1, 0) }
    // 按住键拖动也是「移动」：服务端的悬停高亮要跟着走。
    override func mouseDragged(with event: NSEvent) { owner?.report(1, 0) }
    override func rightMouseDragged(with event: NSEvent) { owner?.report(1, 0) }
    override func otherMouseDragged(with event: NSEvent) { owner?.report(1, 0) }

    override func mouseDown(with event: NSEvent) { owner?.report(2, 1) }
    override func rightMouseDown(with event: NSEvent) { owner?.report(2, 3) }
    override func otherMouseDown(with event: NSEvent) { owner?.report(2, 2) }
}
