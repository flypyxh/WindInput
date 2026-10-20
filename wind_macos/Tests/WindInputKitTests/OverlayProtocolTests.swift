import XCTest
@testable import WindInputKit

final class OverlayProtocolTests: XCTestCase {

    /// 编一帧载荷（与 Rust `encode_overlay_frame` 同布局）。`scale == nil` 编成 68 字节旧版。
    private func payload(_ f: OverlayFrame, withScale: Bool = true) -> Data {
        var d = Data(count: withScale ? 72 : 68)
        func u(_ v: UInt32, _ o: Int) { d.writeUInt32LE(v, at: o) }
        func i(_ v: Int32, _ o: Int) { d.writeUInt32LE(UInt32(bitPattern: v), at: o) }
        u(f.kind, 0); u(f.seq, 4); u(f.width, 8); u(f.height, 12); u(f.flags, 16); u(f.place, 20)
        i(f.x, 24); i(f.y, 28); i(f.altX, 32); i(f.altY, 36)
        u(f.anchor, 40); i(f.margin, 44); i(f.contentX, 48); i(f.contentY, 52)
        u(f.contentW, 56); u(f.contentH, 60); i(f.durationMs, 64)
        if withScale { u(f.scale.bitPattern, 68) }
        return d
    }

    /// 一帧状态气泡：位图 120x40 像素，内容盒 100x24、左上扩边 (8, 6)，scale 1。
    private func statusFrame(scale: Float = 1) -> OverlayFrame {
        var f = OverlayFrame()
        f.kind = OverlayKind.status
        f.width = 120; f.height = 40
        f.contentX = 8; f.contentY = 6; f.contentW = 100; f.contentH = 24
        f.flags = 0x1
        f.scale = scale
        return f
    }

    private let wa = WireRect(x: 0, y: 0, w: 1000, h: 800)

    // MARK: - 解码

    func testDecodeRoundTripsAllFields() {
        var f = statusFrame(scale: 2)
        f.seq = 9; f.place = OverlayPlace.flip
        f.x = -30; f.y = 200; f.altX = -31; f.altY = 150
        f.anchor = 7; f.margin = 16; f.durationMs = 1500
        let back = OverlayFrame.decode(payload(f))
        XCTAssertEqual(back, f)
    }

    func testDecodeOldSizeDefaultsScaleToOne() {
        let back = OverlayFrame.decode(payload(statusFrame(scale: 3), withScale: false))
        XCTAssertEqual(back?.scale, 1)
        XCTAssertEqual(back?.width, 120)
    }

    func testDecodeRejectsShortOrGarbageScale() {
        XCTAssertNil(OverlayFrame.decode(Data(count: 67)))
        var f = statusFrame(); f.scale = 0
        XCTAssertEqual(OverlayFrame.decode(payload(f))?.scale, 1, "0 / 非法缩放按 1 处理")
    }

    // MARK: - 落位（对位 wind_linux placeOverlay）

    func testAbsolutePlacesContentBoxAndSubtractsShadowMargin() {
        var f = statusFrame()
        f.place = OverlayPlace.absolute; f.x = 300; f.y = 200
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r, WireRect(x: 292, y: 194, w: 120, h: 40), "内容盒左上在 (300,200)，窗口再往左上退扩边")
    }

    func testExactIsNeverClamped() {
        var f = statusFrame()
        f.place = OverlayPlace.exact; f.x = 5000; f.y = -40
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r.x, 5000 - 8)
        XCTAssertEqual(r.y, -40 - 6)
    }

    func testFlipUsesAltWhenOverflowingRightAndBottom() {
        var f = statusFrame()
        f.place = OverlayPlace.flip
        f.x = 950; f.y = 790; f.altX = 880; f.altY = 700
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r.x + 8, 880, "右溢改用 altX")
        XCTAssertEqual(r.y + 6, 700, "下溢改用 altY")
    }

    func testFlipAboveTopFallsBackToBottomEdge() {
        var f = statusFrame()
        f.place = OverlayPlace.flip
        f.x = 10; f.y = 790; f.altX = 10; f.altY = -50
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r.y + 6, 800 - 24, "备选点也越过上沿时贴下沿")
    }

    func testFollowCandidateShiftsBothPoints() {
        var f = statusFrame()
        f.kind = OverlayKind.tooltip
        f.place = OverlayPlace.followCandidate
        f.x = 100; f.y = 100; f.altX = 90; f.altY = 60
        let r = OverlayPlacer.place(f, workArea: wa, shift: (dx: 20, dy: 30))
        XCTAssertEqual(r.x + 8, 120)
        XCTAssertEqual(r.y + 6, 130)
    }

    func testAnchorCornersAndCenter() {
        var f = statusFrame()
        f.place = OverlayPlace.anchor; f.margin = 16
        f.anchor = OverlayAnchor.topLeft
        XCTAssertEqual(OverlayPlacer.place(f, workArea: wa).x + 8, 16)
        f.anchor = OverlayAnchor.bottomRight
        let br = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(br.x + 8, 1000 - 16 - 100)
        XCTAssertEqual(br.y + 6, 800 - 16 - 24)
        f.anchor = OverlayAnchor.center
        let c = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(c.x + 8, 450)
        XCTAssertEqual(c.y + 6, 388)
        f.anchor = OverlayAnchor.bottomCenter
        XCTAssertEqual(OverlayPlacer.place(f, workArea: wa).x + 8, 450)
    }

    func testClampKeepsContentInsideWorkArea() {
        var f = statusFrame()
        f.place = OverlayPlace.absolute; f.x = 990; f.y = -20
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r.x + 8, 1000 - 100)
        XCTAssertEqual(r.y + 6, 0)
    }

    /// Retina：位图 / 内容盒是像素，落位按点算——内容盒 100px 只占 50pt。
    func testScaleConvertsBitmapPixelsToPoints() {
        var f = statusFrame(scale: 2)
        f.place = OverlayPlace.absolute; f.x = 300; f.y = 200
        let r = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(r, WireRect(x: 296, y: 197, w: 60, h: 20))
        f.place = OverlayPlace.anchor; f.anchor = OverlayAnchor.bottomRight; f.margin = 16
        let br = OverlayPlacer.place(f, workArea: wa)
        XCTAssertEqual(br.x + 4, 1000 - 16 - 50)
    }

    func testDragFollowsPointerKeepingGrabOffsetAndClamps() {
        var f = statusFrame()
        f.place = OverlayPlace.absolute
        // 抓住窗口内 (30, 12) 处，指针挪到 (500, 400)。
        let r = OverlayPlacer.drag(pointer: 500, 400, grab: (dx: 30, dy: 12), frame: f, workArea: wa)
        XCTAssertEqual(r.x, 470)
        XCTAssertEqual(r.y, 388)
        // 往右下拖出工作区：内容盒被夹回，阴影扩边允许出屏。
        let o = OverlayPlacer.drag(pointer: 2000, 2000, grab: (dx: 0, dy: 0), frame: f, workArea: wa)
        XCTAssertEqual(o.x + 8, 1000 - 100)
        XCTAssertEqual(o.y + 6, 800 - 24)
    }

    // MARK: - 交互纯逻辑

    func testPressActions() {
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.status, button: 1), .drag)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.status, button: 3), .menu)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.status, button: 2), .none)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.tooltip, button: 3), .menu)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.tooltip, button: 1), .none)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.toast, button: 1), .close)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.toast, button: 3), .close)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.toast, button: 4), .none)
        XCTAssertEqual(OverlayInput.pressAction(kind: OverlayKind.menu, button: 1), .none)
    }

    func testMenuTargets() {
        XCTAssertEqual(OverlayInput.menuTarget(kind: OverlayKind.status), -2)
        XCTAssertEqual(OverlayInput.menuTarget(kind: OverlayKind.tooltip), -3)
        XCTAssertEqual(OverlayInput.menuTarget(kind: OverlayKind.toast), -1)
    }

    func testHoverDeferOnlyWhenTipShownAndHoverMoved() {
        // 提示没显示 / 本就没悬停 / 没变化：立即报。
        XCTAssertEqual(OverlayInput.hoverDeferMs(last: 2, raw: -1, tipShown: false), 0)
        XCTAssertEqual(OverlayInput.hoverDeferMs(last: -1, raw: 3, tipShown: true), 0)
        XCTAssertEqual(OverlayInput.hoverDeferMs(last: 2, raw: 2, tipShown: true), 0)
        // 提示显示着：去往「无」给 280ms，去往另一个候选给 150ms。
        XCTAssertEqual(OverlayInput.hoverDeferMs(last: 2, raw: -1, tipShown: true), 280)
        XCTAssertEqual(OverlayInput.hoverDeferMs(last: 2, raw: 4, tipShown: true), 150)
    }

    /// 同 Windows heading_to_tip 那张表。
    func testHeadingToTip() {
        let rightTop = WireRect(x: 100, y: 0, w: 100, h: 40)
        XCTAssertTrue(OverlayInput.headingToTip(10, 120, 14, 120, tip: rightTop))   // 沿行水平右移
        XCTAssertTrue(OverlayInput.headingToTip(10, 120, 14, 116, tip: rightTop))   // 斜着往上
        XCTAssertFalse(OverlayInput.headingToTip(10, 120, 14, 126, tip: rightTop))  // 往下偏得比靠近快
        XCTAssertFalse(OverlayInput.headingToTip(10, 120, 6, 120, tip: rightTop))   // 往回走
        XCTAssertFalse(OverlayInput.headingToTip(10, 120, 10, 120, tip: rightTop))  // 没动
        XCTAssertTrue(OverlayInput.headingToTip(10, 20, 12, 26, tip: rightTop))     // 跨度内上下随意
        XCTAssertTrue(OverlayInput.headingToTip(10, 20, 6, 20, tip: WireRect(x: -200, y: 0, w: 100, h: 40)))
        let below = WireRect(x: 0, y: 50, w: 80, h: 70)
        XCTAssertTrue(OverlayInput.headingToTip(40, 10, 40, 14, tip: below))
        XCTAssertFalse(OverlayInput.headingToTip(40, 10, 40, 6, tip: below))
        XCTAssertFalse(OverlayInput.headingToTip(150, 60, 151, 60, tip: WireRect(x: 100, y: 0, w: 100, h: 100)))
        XCTAssertEqual(OverlayInput.tipGap(25, 5, tip: rightTop), 75)   // 同 Windows tip_gap
        XCTAssertEqual(OverlayInput.tipGap(40, 10, tip: below), 40)
        XCTAssertEqual(OverlayInput.tipGap(150, 60, tip: WireRect(x: 100, y: 0, w: 100, h: 100)), 0)
    }

    func testTipRecheckHover() {
        let keep = OverlayInput.keepHover
        XCTAssertEqual(OverlayInput.tipRecheckHover(onTip: true, onCandidate: false, candidateHit: -1, held: 2), keep)
        XCTAssertEqual(OverlayInput.tipRecheckHover(onTip: false, onCandidate: true, candidateHit: 2, held: 2), keep)
        XCTAssertEqual(OverlayInput.tipRecheckHover(onTip: false, onCandidate: true, candidateHit: 5, held: 2), 5)
        XCTAssertEqual(OverlayInput.tipRecheckHover(onTip: false, onCandidate: false, candidateHit: 5, held: 2), -1)
        XCTAssertEqual(OverlayInput.tipRecheckHover(onTip: false, onCandidate: true, candidateHit: -1, held: -1), keep)
    }

    func testHoverGateIgnoresStationaryPointer() {
        var g = HoverGate()
        g.rebase(10, 20)
        XCTAssertFalse(g.accept(10, 20), "窗口弹在静止的指针下面不算悬停")
        XCTAssertTrue(g.accept(11, 20))
        XCTAssertFalse(g.accept(11, 20))
    }

    func testMenuKindRange() {
        XCTAssertTrue(OverlayKind.isMenu(OverlayKind.menu))
        XCTAssertTrue(OverlayKind.isMenu(OverlayKind.menu + OverlayKind.menuLevels - 1))
        XCTAssertFalse(OverlayKind.isMenu(OverlayKind.menu + OverlayKind.menuLevels))
        XCTAssertFalse(OverlayKind.isMenu(OverlayKind.toast))
    }

    func testMenuPointerFrameLayout() {
        let f = BinaryCodec.encodeMenuPointerFrame(event: 2, button: 3, x: -40, y: 700)
        XCTAssertEqual(f.count, WireProtocol.headerSize + 16)
        let h = try? BinaryCodec.decodeHeader(f)
        XCTAssertEqual(h?.cmd, UpstreamCmd.menuPointer)
        XCTAssertEqual(h?.length, 16)
        XCTAssertEqual(h?.isAsync, true, "移动事件不等 ack")
        let p = f.subdata(in: WireProtocol.headerSize..<f.count)
        XCTAssertEqual(p.readUInt32LE(at: 0), 2)
        XCTAssertEqual(p.readUInt32LE(at: 4), 3)
        XCTAssertEqual(Int32(bitPattern: p.readUInt32LE(at: 8)), -40)
        XCTAssertEqual(Int32(bitPattern: p.readUInt32LE(at: 12)), 700)
    }
}
