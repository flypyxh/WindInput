import XCTest
@testable import WindInputKit

final class WireGeometryTests: XCTestCase {

    /// 1080p 主屏。
    private let H: CGFloat = 1080
    private let vf = CGRect(x: 0, y: 0, width: 1920, height: 1055) // 顶部 25pt 菜单栏

    func testFlipYIsSelfInverse() {
        for y in [CGFloat(0), 1, 540, 1079, 1080] {
            XCTAssertEqual(WireGeometry.flipY(WireGeometry.flipY(y, screenHeight: H),
                                              screenHeight: H), y)
        }
    }

    /// wire 顶边 0 = 屏幕最上方；wire y 增大 = 往下走。
    func testWireTopLeftMapsScreenTopToZero() {
        let size = CGSize(width: 200, height: 60)
        // 窗口顶边贴屏幕顶 → Cocoa 左下角 y = 1080 - 60。
        let atTop = CGRect(origin: CGPoint(x: 30, y: H - size.height), size: size)
        let p = WireGeometry.wireTopLeft(of: atTop, screenHeight: H)
        XCTAssertEqual(p.x, 30)
        XCTAssertEqual(p.y, 0)
    }

    /// **拖动不漂移的根据**：`wireTopLeft` 与 `cocoaOrigin` 必须严格互逆。
    ///
    /// 回归意义：拖动落点经 wire 上报 → 落盘 → 下一帧再发回来摆窗，若两个方向差一个窗口
    /// 高度或用了不同参照，每一轮都会多偏一次，候选窗逐次往上/下爬。
    func testWireAndCocoaRoundTrip() {
        let size = CGSize(width: 240, height: 72)
        for (wx, wy) in [(CGFloat(0), CGFloat(0)), (100, 200), (1680, 1008), (-40, 13)] {
            let origin = WireGeometry.cocoaOrigin(wireX: wx, wireY: wy, size: size, screenHeight: H)
            let back = WireGeometry.wireTopLeft(of: CGRect(origin: origin, size: size),
                                                screenHeight: H)
            XCTAssertEqual(CGFloat(back.x), wx, "x 不互逆 @(\(wx),\(wy))")
            XCTAssertEqual(CGFloat(back.y), wy, "y 不互逆 @(\(wx),\(wy))")
        }
    }

    func testClampPullsBackFromRightAndBottom() {
        let size = CGSize(width: 200, height: 60)
        let out = WireGeometry.clamp(origin: CGPoint(x: 1900, y: -30), size: size, visibleFrame: vf)
        XCTAssertEqual(out.x, vf.maxX - size.width)
        XCTAssertEqual(out.y, vf.minY)
    }

    /// 越过顶边（菜单栏下沿）时回拉，避免窗口钻到菜单栏后面。
    func testClampRespectsTopOfVisibleFrame() {
        let size = CGSize(width: 200, height: 60)
        let out = WireGeometry.clamp(origin: CGPoint(x: 10, y: 1040), size: size, visibleFrame: vf)
        XCTAssertEqual(out.y, vf.maxY - size.height)
    }

    /// 窗口比屏幕还大时保证**左上角**可见：先按右/下回拉会把左上角推出屏幕，
    /// 必须再按左/上兜一次。
    func testClampKeepsTopLeftVisibleWhenOversized() {
        let huge = CGSize(width: 3000, height: 2000)
        let out = WireGeometry.clamp(origin: CGPoint(x: 500, y: 500), size: huge, visibleFrame: vf)
        XCTAssertEqual(out.x, vf.minX)
        XCTAssertEqual(out.y + huge.height, vf.minY + huge.height, "左上角须落在可见区内")
    }

    // MARK: - 状态气泡锚点 (C2-33 / GH#148)

    /// 可见区 (Cocoa, y 向上): 左下 (0,0), 顶部让出 25pt 菜单栏。
    private let tip = CGSize(width: 120, height: 34)

    func testAnchorOriginScreenCorners() {
        let m: CGFloat = 16
        XCTAssertEqual(WireGeometry.anchorOrigin(anchor: .screenTopLeft, target: vf, size: tip, margin: m),
                       CGPoint(x: 16, y: 1055 - 16 - 34), "「顶」是 maxY")
        XCTAssertEqual(WireGeometry.anchorOrigin(anchor: .screenTopRight, target: vf, size: tip, margin: m),
                       CGPoint(x: 1920 - 16 - 120, y: 1055 - 16 - 34))
        XCTAssertEqual(WireGeometry.anchorOrigin(anchor: .screenBottomLeft, target: vf, size: tip, margin: m),
                       CGPoint(x: 16, y: 16))
        XCTAssertEqual(WireGeometry.anchorOrigin(anchor: .screenBottomRight, target: vf, size: tip, margin: m),
                       CGPoint(x: 1920 - 16 - 120, y: 16))
    }

    func testAnchorOriginCenterIgnoresMargin() {
        let c0 = WireGeometry.anchorOrigin(anchor: .screenCenter, target: vf, size: tip, margin: 0)
        let c16 = WireGeometry.anchorOrigin(anchor: .screenCenter, target: vf, size: tip, margin: 16)
        XCTAssertEqual(c0, c16)
        XCTAssertEqual(c0, CGPoint(x: 900, y: (527.5 - 17).rounded()))
    }

    /// 窗口锚点拿不到宿主窗口边框 ⇒ 降级为同位置的屏幕锚点。
    func testWindowAnchorsDegradeToScreen() {
        XCTAssertEqual(StatusAnchor.windowCenter.screenEquivalent, .screenCenter)
        XCTAssertEqual(StatusAnchor.windowBottomLeft.screenEquivalent, .screenBottomLeft)
        XCTAssertEqual(StatusAnchor.screenTopRight.screenEquivalent, .screenTopRight)
    }
}
