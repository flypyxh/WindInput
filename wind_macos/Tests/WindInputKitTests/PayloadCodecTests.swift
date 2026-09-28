import XCTest
import AppKit   // NSAttributedString.Key.markedClauseSegment / .underlineStyle
@testable import WindInputKit

/// 测试 BinaryCodec 新增的 downstream payload decode 方法 (M2.2-C 实装).
final class PayloadCodecTests: XCTestCase {

    // MARK: - CmdCommitText (0x0101)

    func testDecodeCommitText_PlainAscii() throws {
        // 构 payload: flags(0) + textLen(5) + compLen(0) + "hello"
        var buf = Data(count: 12)
        buf.writeUInt32LE(0, at: 0)
        buf.writeUInt32LE(5, at: 4)
        buf.writeUInt32LE(0, at: 8)
        buf.append(contentsOf: "hello".utf8)

        let p = try BinaryCodec.decodeCommitTextPayload(buf)
        XCTAssertEqual(p.text, "hello")
        XCTAssertEqual(p.newComposition, "")
        XCTAssertFalse(p.modeChanged)
        XCTAssertFalse(p.hasNewComposition)
        XCTAssertFalse(p.chineseMode)
    }

    func testDecodeCommitText_UTF8WithFlagsAndComposition() throws {
        let text = "你好"   // 6 utf-8 bytes
        let comp = "world"  // 5 utf-8 bytes
        let flags: UInt32 = 0x0001 | 0x0002 | 0x0004   // 三个 flag 全开

        var buf = Data(count: 12)
        buf.writeUInt32LE(flags, at: 0)
        buf.writeUInt32LE(UInt32(text.utf8.count), at: 4)
        buf.writeUInt32LE(UInt32(comp.utf8.count), at: 8)
        buf.append(contentsOf: text.utf8)
        buf.append(contentsOf: comp.utf8)

        let p = try BinaryCodec.decodeCommitTextPayload(buf)
        XCTAssertEqual(p.text, text)
        XCTAssertEqual(p.newComposition, comp)
        XCTAssertTrue(p.modeChanged)
        XCTAssertTrue(p.hasNewComposition)
        XCTAssertTrue(p.chineseMode)
    }

    func testDecodeCommitText_TooShort() {
        let buf = Data([0x00, 0x00])
        XCTAssertThrowsError(try BinaryCodec.decodeCommitTextPayload(buf)) { error in
            if case .payloadTooShort = error as? IPCError {} else { XCTFail("wrong: \(error)") }
        }
    }

    // MARK: - CmdUpdateComposition (0x0102)

    func testDecodeUpdateComposition_Roundtrip() throws {
        let text = "ni'hao"
        var buf = Data(count: 4)
        buf.writeUInt32LE(3, at: 0)   // caretPos = 3
        buf.append(contentsOf: text.utf8)

        let p = try BinaryCodec.decodeUpdateCompositionPayload(buf)
        XCTAssertEqual(p.caretPos, 3)
        XCTAssertEqual(p.text, text)
    }

    func testDecodeUpdateComposition_EmptyText() throws {
        var buf = Data(count: 4)
        buf.writeUInt32LE(0, at: 0)
        let p = try BinaryCodec.decodeUpdateCompositionPayload(buf)
        XCTAssertEqual(p.caretPos, 0)
        XCTAssertEqual(p.text, "")
    }

    // MARK: - CmdCommitTextWithCursor (0x0106)

    func testDecodeCommitTextWithCursor_Roundtrip() throws {
        let text = "abc"
        var buf = Data(count: 8)
        buf.writeUInt32LE(UInt32(text.utf8.count), at: 0)
        buf.writeUInt32LE(2, at: 4)    // cursorOffset = 2
        buf.append(contentsOf: text.utf8)

        let p = try BinaryCodec.decodeCommitTextWithCursorPayload(buf)
        XCTAssertEqual(p.text, "abc")
        XCTAssertEqual(p.cursorOffset, 2)
    }

    // MARK: - CmdMoveCursor (0x0107)

    func testDecodeMoveCursor_DirectionRight() throws {
        var buf = Data(count: 4)
        buf.writeUInt32LE(1, at: 0)
        let p = try BinaryCodec.decodeMoveCursorPayload(buf)
        XCTAssertEqual(p.direction, 1)
    }

    // MARK: - CmdStatePush (0x0206 push)

    func testDecodeStatePush_AllFlags() throws {
        let label = "中"   // 3 utf-8 bytes
        var buf = Data(count: 12)
        // 0x0001 ChineseMode | 0x0008 ToolbarVisible | 0x0020 CapsLock
        buf.writeUInt32LE(0x0001 | 0x0008 | 0x0020, at: 0)
        buf.writeUInt32LE(0, at: 4)
        buf.writeUInt32LE(0, at: 8)
        buf.append(contentsOf: label.utf8)

        let p = try BinaryCodec.decodeStatePushPayload(buf)
        XCTAssertEqual(p.iconLabel, "中")
        XCTAssertTrue(p.chineseMode)
        XCTAssertTrue(p.toolbarVisible)
        XCTAssertTrue(p.capsLock)
        XCTAssertFalse(p.fullWidth)
        XCTAssertFalse(p.chinesePunct)
    }

    // MARK: - CompositionState: caret 恒为 UTF-16 单元, 两端同单位不换算

    func testCompositionState_CaretMapping_ASCII() {
        let s = CompositionState(text: "abc", caretUTF16: 2)
        XCTAssertEqual(s.caretInUTF16(), 2)
        XCTAssertEqual(s.utf16Length, 3)
    }

    func testCompositionState_CaretMapping_CJK() {
        // "你好" 每个字在 BMP 内 = 1 UTF-16 unit
        let s = CompositionState(text: "你好", caretUTF16: 1)
        XCTAssertEqual(s.caretInUTF16(), 1)
        XCTAssertEqual(s.utf16Length, 2)
    }

    /// ★ 这条是旧实现真正错的地方 —— 也是「用中文怎么测都测不出来」的那一格。
    ///
    /// 组合区前缀含扩展 B 区汉字 (生僻字候选上屏后作为已转换前缀) 且光标**不在串尾**时:
    /// 服务端按 UTF-16 给 2 (「𠮷」占 2 个单元), 旧实现把这个 2 当成「前 2 个**字符**」
    /// 再折算, 得出 3 —— 光标凭空右移一格。光标在串尾时被 min 钳住恰好蒙对, 所以它一直没
    /// 暴露。
    func testCompositionState_CaretIsUTF16NotCharacterIndex() {
        let s = CompositionState(text: "𠮷zh", caretUTF16: 2)   // 𠮷 = U+20BB7, 占 2 个单元
        XCTAssertEqual(s.utf16Length, 4)
        XCTAssertEqual(s.caretInUTF16(), 2,
                       "caret 已是 UTF-16 偏移, 不得再按字符数折算 (那样会得到 3)")
    }

    /// 越界一律退到**串尾**而非 0: 组合期的编辑点绝大多数时候就在末尾, 退到 0 会让光标
    /// 停在刚打出的字母之前, 比没有光标更让人困惑。
    func testCompositionState_CaretClampsToTailNotZero() {
        XCTAssertEqual(CompositionState(text: "ab", caretUTF16: 99).caretInUTF16(), 2)
        XCTAssertEqual(CompositionState(text: "ab", caretUTF16: -1).caretInUTF16(), 0)
    }

    func testCompositionState_Clear() {
        var s = CompositionState(text: "ni", caretUTF16: 2)
        s.clear()
        XCTAssertTrue(s.isEmpty)
        XCTAssertEqual(s.caretUTF16, 0)
    }

    // MARK: - MarkedTextAttributes (组合串属性: selectionRange 能否活着到宿主)

    /// 少了 `markedClauseSegment`, IMKit 会替我们合成默认分句并把 selectionRange 覆写成
    /// `{0, 全长}`, 宿主于是把组合内光标画在最前面。这是真机才看得出的缺陷, 故在此守门。
    func testMarkedTextAttributes_AlwaysDeclaresClauseSegment() {
        XCTAssertTrue(MarkedTextAttributes.declaresClauseSegment(
            MarkedTextAttributes.ensureClauseSegment()),
            "controller 拿不到时也必须自带分句声明")
        XCTAssertTrue(MarkedTextAttributes.declaresClauseSegment(
            MarkedTextAttributes.ensureClauseSegment([.underlineStyle: 9])),
            "markForStyle 返回的字典若不含分句声明, 必须补上")
    }

    /// 兜底不得覆盖 `markForStyle:` 已给出的取值 —— 那是系统按当前主题算的。
    func testMarkedTextAttributes_PreservesBaseValues() {
        let attrs = MarkedTextAttributes.ensureClauseSegment([
            .markedClauseSegment: 3,
            .underlineStyle: 9,
        ])
        XCTAssertEqual(attrs[.markedClauseSegment] as? Int, 3)
        XCTAssertEqual(attrs[.underlineStyle] as? Int, 9)
    }

    // MARK: - CmdTooltipShow (0x0508): fontPath / runs 两段后加尾段的新旧兼容

    /// 按 Rust `encode_tooltip_show` 的布局拼 payload: 若干长度前缀字符串 + 可选 runs 尾段。
    private func tooltipPayload(_ strings: [String],
                                runs: [(UInt32, UInt32, [UInt8])]? = nil) -> Data {
        var buf = Data()
        func u32(_ v: UInt32) {
            var d = Data(count: 4); d.writeUInt32LE(v, at: 0); buf.append(d)
        }
        for s in strings { u32(UInt32(s.utf8.count)); buf.append(contentsOf: s.utf8) }
        if let runs = runs {
            u32(UInt32(runs.count))
            for (start, len, rgba) in runs { u32(start); u32(len); buf.append(contentsOf: rgba) }
        }
        return buf
    }

    /// 旧服务 (无 fontPath、无 runs): 两个尾段都缺省, 退化为单色。
    func testDecodeTooltip_OldestFormHasNoTail() throws {
        let p = try BinaryCodec.decodeTooltipPayload(tooltipPayload(["[拼音]\n好：hǎo", "#3C3C3CF0", "#FFFFFFFF"]))
        XCTAssertEqual(p.text, "[拼音]\n好：hǎo")
        XCTAssertEqual(p.fontPath, "")
        XCTAssertEqual(p.runs, [])
    }

    /// 分段着色之前的服务 (有 fontPath、无 runs): runs 为空。
    func testDecodeTooltip_FontPathWithoutRuns() throws {
        let p = try BinaryCodec.decodeTooltipPayload(tooltipPayload(["abc", "", "", "/p.ttf"]))
        XCTAssertEqual(p.fontPath, "/p.ttf")
        XCTAssertEqual(p.runs, [])
    }

    /// 新服务: runs 尾段, UTF-16 区间 + rgba 小端 u32 (字节依次 R、G、B、A)。
    func testDecodeTooltip_RunsTail() throws {
        let buf = tooltipPayload(["[拼音]\n你", "", "", ""],
                                 runs: [(0, 4, [0x11, 0x22, 0x33, 0x44]), (5, 1, [0xFF, 0x00, 0x80, 0xFF])])
        let p = try BinaryCodec.decodeTooltipPayload(buf)
        XCTAssertEqual(p.runs, [
            TooltipColorRun(start: 0, length: 4, r: 0x11, g: 0x22, b: 0x33, a: 0x44),
            TooltipColorRun(start: 5, length: 1, r: 0xFF, g: 0x00, b: 0x80, a: 0xFF),
        ])
    }

    /// 空 runs (Rust 端恒写 count=0) 与没有尾段等价。
    func testDecodeTooltip_EmptyRunsCount() throws {
        let p = try BinaryCodec.decodeTooltipPayload(tooltipPayload(["abc", "", "", ""], runs: []))
        XCTAssertEqual(p.runs, [])
    }

    /// 声明了 2 段却只给 1 段: 报 payloadTooShort, 不越界读。
    func testDecodeTooltip_TruncatedRunsThrows() {
        var buf = tooltipPayload(["abc", "", "", ""], runs: [(0, 1, [1, 2, 3, 4])])
        let countAt = buf.count - 16
        buf.writeUInt32LE(2, at: countAt)
        XCTAssertThrowsError(try BinaryCodec.decodeTooltipPayload(buf)) { error in
            if case .payloadTooShort = error as? IPCError {} else { XCTFail("wrong: \(error)") }
        }
    }

    // MARK: - CmdStatusShow (0x050A): anchor 尾段的新旧兼容 (C2-33 / GH#148)

    /// 按 Rust `encode_status_show` 的布局拼 payload: 三段字符串 + x/y/dur (+ 可选 anchor)。
    private func statusPayload(anchor: Int32?) -> Data {
        var buf = Data()
        func u32(_ v: UInt32) {
            var d = Data(count: 4); d.writeUInt32LE(v, at: 0); buf.append(d)
        }
        for s in ["中", "#111", "#eee"] { u32(UInt32(s.utf8.count)); buf.append(contentsOf: s.utf8) }
        u32(UInt32(bitPattern: 50)); u32(UInt32(bitPattern: -80)); u32(1000)
        if let a = anchor { u32(UInt32(bitPattern: a)) }
        return buf
    }

    /// 旧服务 (无 anchor 尾段): 缺省 0 = 按 x/y 摆。
    func testDecodeStatusBubble_WithoutAnchorTail() throws {
        let p = try BinaryCodec.decodeStatusBubblePayload(statusPayload(anchor: nil))
        XCTAssertEqual(p.text, "中")
        XCTAssertEqual(p.x, 50)
        XCTAssertEqual(p.y, -80)
        XCTAssertEqual(p.durationMs, 1000)
        XCTAssertEqual(p.anchor, 0)
    }

    /// 新服务: anchor 尾段原样读出, 与 Rust `status_anchor` 同值。
    func testDecodeStatusBubble_AnchorTail() throws {
        let p = try BinaryCodec.decodeStatusBubblePayload(statusPayload(anchor: 7))
        XCTAssertEqual(p.durationMs, 1000)
        XCTAssertEqual(StatusAnchor(rawValue: p.anchor), .windowBottomLeft)
    }
}
