#!/usr/bin/env python3
"""生成 macOS 菜单栏输入法图标 (单色模板 PDF + 源 SVG).

样式对齐系统自带输入法 (ABC / 拼音 / 五笔): 圆角矩形实心底, 字形镂空 (透明),
系统按浅/深色菜单栏对模板图反色. 此前是一个裸「风」字, 字面比系统图标宽/散,
与其它输入法并排时间距显得过宽 (GH#170).

字形取自 Noto Sans CJK SC, 轮廓直接写进路径 (不依赖运行时字体); 先合并重叠笔画,
再用 nonzero 路径做「方块减字形」, 避免 even-odd 在笔画交叠处出洞.

依赖: pip install fonttools skia-pathops pymupdf
用法: gen_menu_icon.py <Noto Sans CJK SC .otf> <Resources 目录>
产物: wind-15.svg / wind-26.svg, menu_icon_15.pdf / menu_icon_26.pdf / menu_icon.pdf (=15pt)
"""
import shutil
import sys
from pathlib import Path

import pathops
import pymupdf
from fontTools.ttLib import TTFont

CHAR = "风"
# 页面尺寸照 fcitx5-macos 实际发布的 menu_icon*.pdf: 21.75x15.75pt (= 其 SVG 的 29x21px * 0.75,
# rsvg-convert 按 96dpi 转 pt). 真机 (macOS 26) 并排实测它的徽标尺寸与系统 ABC/五笔一致 (23x16px).
# 页高超过 15.75 (如 16 / 21) 会被菜单压成方块并压扁字形, 勿放大.
# 圆角: fcitx 用 2.25/5.25pt, 实测仍比系统徽标尖; 按系统徽标左上角像素轮廓反推约 4.5pt.
# 档位名 15/26 疑为 macOS 大版本 (推断), 两档都用 4.5.
W, H = 21.75, 15.75
RADIUS = {15: 4.5, 26: 4.5}
GLYPH_H = 0.56  # 字形高度占高度比例
EMBOLDEN = 0.022  # 字形向外加粗量 (常规字重在 15pt 下偏细)


def glyph_path(font_path: str) -> pathops.Path:
    font = TTFont(font_path)
    gs = font.getGlyphSet()
    name = font.getBestCmap()[ord(CHAR)]
    path = pathops.Path()
    gs[name].draw(path.getPen(glyphSet=gs))
    return path


def build(font_path: str, radius: float) -> str:
    w, n = W, H
    g = glyph_path(font_path)
    g.simplify(fix_winding=True)
    x0, y0, x1, y1 = g.bounds
    scale = n * GLYPH_H / (y1 - y0)
    # 字形 y 轴向上, SVG 向下: 翻转, 并把包围盒居中到画布
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    g = g.transform(scale, 0, 0, -scale, w / 2 - cx * scale, n / 2 + cy * scale)

    # 加粗: 字形 ∪ 描边
    fat = pathops.Path(g)
    fat.stroke(n * EMBOLDEN * 2, pathops.LineCap.ROUND_CAP, pathops.LineJoin.ROUND_JOIN, 4)
    fat.convertConicsToQuads()  # stroke 产出 conic, op() 不接受
    glyph = pathops.op(g, fat, pathops.PathOp.UNION)

    m, r = 0.0, radius
    box = pathops.Path()
    pen = box.getPen()
    sx, sy = w - m, n - m
    pen.moveTo((m + r, m))
    pen.lineTo((sx - r, m)); pen.qCurveTo((sx, m), (sx, m + r))
    pen.lineTo((sx, sy - r)); pen.qCurveTo((sx, sy), (sx - r, sy))
    pen.lineTo((m + r, sy)); pen.qCurveTo((m, sy), (m, sy - r))
    pen.lineTo((m, m + r)); pen.qCurveTo((m, m), (m + r, m))
    pen.closePath()

    icon = pathops.op(box, glyph, pathops.PathOp.DIFFERENCE)
    cmds = []
    for verb, pts in icon.segments:
        f = lambda p: f"{p[0]:.3f} {p[1]:.3f}"
        if verb == "moveTo":
            cmds.append("M" + f(pts[0]))
        elif verb == "lineTo":
            cmds.append("L" + f(pts[0]))
        elif verb == "qCurveTo":
            # pathops 的 segments 已是 TrueType 风格, 展开为二次贝塞尔
            for i in range(len(pts) - 1):
                c = pts[i]
                e = pts[i + 1] if i == len(pts) - 2 else (
                    (pts[i][0] + pts[i + 1][0]) / 2, (pts[i][1] + pts[i + 1][1]) / 2)
                cmds.append(f"Q{f(c)} {f(e)}")
        elif verb == "curveTo":
            cmds.append("C" + " ".join(f(p) for p in pts))
        elif verb == "closePath":
            cmds.append("Z")
    d = "".join(cmds)
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {n}" width="{w}" height="{n}">\n'
        f'  <path fill="black" fill-rule="nonzero" d="{d}"/>\n</svg>\n'
    )


def to_pdf(svg_path: Path, pdf_path: Path) -> None:
    doc = pymupdf.open(str(svg_path))
    pdf_path.write_bytes(doc.convert_to_pdf())


def main() -> None:
    font, out = sys.argv[1], Path(sys.argv[2])
    for tier, radius in RADIUS.items():
        svg = out / f"wind-{tier}.svg"
        svg.write_text(build(font, radius), encoding="utf-8")
        to_pdf(svg, out / f"menu_icon_{tier}.pdf")
    shutil.copyfile(out / "menu_icon_15.pdf", out / "menu_icon.pdf")


if __name__ == "__main__":
    main()
