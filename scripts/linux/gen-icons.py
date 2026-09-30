#!/usr/bin/env python3
"""一次性工具：生成 wind_linux/data/icons/hicolor 下的图标（产物已入库，打包时不跑它）。

    python3 scripts/linux/gen-icons.py [--font 字体文件] [--ico 应用图标.ico]

依赖 Pillow + fontTools 与一款 CJK 字体（默认 Noto Sans CJK SC）。产出两类：

- `windinput`：输入法 / 设置程序的图标。只有位图源（兄弟仓库 wind-setting 的
  `res/wind_setting.ico`，逐尺寸手调过的 16…256），照原样按尺寸拆出来；22px 那档 ico 里没有，
  由 24px 缩。没有矢量源，故不出 scalable——拿位图包一层 SVG 不是真的可缩放。
- `windinput-zh` / `windinput-en`：托盘 / 面板随中英模式切换的图标。带底色的圆角方块 +
  白字「中」「英」：底色让它在深浅两种面板上都看得清。字形取自字体轮廓，SVG 里是路径，
  不依赖目标机装了什么字体。
"""
import argparse
import os
import sys

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "wind_linux", "data", "icons", "hicolor")
APP_SIZES = [16, 22, 24, 32, 48, 64, 128, 256]
MODE_SIZES = [16, 22, 24, 32, 48, 64]

# 中文态用应用图标的品牌蓝，英文态用石板灰：两态一眼可分，白字在两种底色上都够对比。
#
# 英文态用拉丁字母「A」而不是「英」：托盘图标常见尺寸是 16/22/24px，「英」八画挤进十几像素，
# 笔画之间的空隙被描边填死、糊成一团（实测放大看是一块白疙瘩）；「A」三笔，在任何尺寸都锐利，
# 也是 macOS 输入法指示器的惯例（中 / A）。「中」四笔、结构开阔，加一像素描边后很清晰。
# 字段：(字符, 底色, 字体键, 字号/边长, SVG 描边/边长)
MODES = {
    "windinput-zh": ("中", "#2F86E6", "cjk", 0.80, 0.035),
    "windinput-en": ("A", "#5E6B78", "latin", 0.74, 0.0),
}
RADIUS = 0.22  # 圆角半径 / 边长（与应用图标的圆角接近）
GLYPH = 0.74  # SVG 里字身 / 边长（位图按 MODES 里各自的字号比）


def app_icons(ico_path):
    ico = Image.open(ico_path)
    have = set(ico.info.get("sizes", []))
    for s in APP_SIZES:
        if (s, s) in have:
            ico.size = (s, s)
            img = ico.copy().convert("RGBA")
        else:
            src = min((w for w, _ in have if w > s), default=256)
            ico.size = (src, src)
            img = ico.copy().convert("RGBA").resize((s, s), Image.LANCZOS)
        save_png(img, s, "windinput")


def save_png(img, size, name):
    d = os.path.join(OUT, f"{size}x{size}", "apps")
    os.makedirs(d, exist_ok=True)
    img.save(os.path.join(d, name + ".png"), optimize=True)


def mode_png(char, color, size, font_path, glyph_ratio, bold_ratio):
    # 底板（圆角方块）8 倍超采样再缩：直接按小尺寸画，圆角是锯齿。
    k = 8
    n = size * k
    box = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(box).rounded_rectangle(
        [0, 0, n - 1, n - 1], radius=int(n * RADIUS), fill=color)
    img = box.resize((size, size), Image.LANCZOS)

    # 字形则**直接在目标尺寸上画、位置取整**：先画大图再缩会把笔画边缘抹成灰边，16~24px 下
    # 整个字发软。描边取整数像素（≥1），让横竖笔画落在整像素格上。
    font = ImageFont.truetype(font_path, max(1, round(size * glyph_ratio)))
    stroke = round(size * bold_ratio * 1.3) if bold_ratio > 0 else 0
    if bold_ratio > 0:
        stroke = max(1, stroke)
    glyph = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glyph)
    bb = gd.textbbox((0, 0), char, font=font, stroke_width=stroke)
    x = round((size - (bb[2] - bb[0])) / 2 - bb[0])
    y = round((size - (bb[3] - bb[1])) / 2 - bb[1])
    gd.text((x, y), char, font=font, fill="white", stroke_width=stroke, stroke_fill="white")
    img.alpha_composite(glyph)
    return img


def mode_svg(char, color, font_path, glyph_ratio, bold_ratio):
    font = TTFont(font_path, fontNumber=0)
    gs = font.getGlyphSet()
    name = font.getBestCmap()[ord(char)]
    upm = font["head"].unitsPerEm
    # 画布 100×100。字形放进「字身」框：按 em 缩放、水平按步进居中、垂直按 ascender/descender 居中
    # （与 Pillow 的 anchor="mm" 同一口径，两种产物的字位一致）。
    scale = 100 * glyph_ratio / upm
    # 按字形自己的包围盒居中：「A」这类拉丁大写字母没有下伸部，按 ascender/descender 居中会偏上。
    bp = BoundsPen(gs)
    gs[name].draw(bp)
    xmin, ymin, xmax, ymax = bp.bounds
    dx = 50 - (xmin + xmax) / 2 * scale
    dy = 50 + (ymin + ymax) / 2 * scale
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
    gs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, dx, dy)))
    r = 100 * RADIUS
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">\n'
        f'  <rect width="100" height="100" rx="{r:g}" ry="{r:g}" fill="{color}"/>\n'
        f'  <path d="{pen.getCommands()}" fill="#fff" '
        + (f'stroke="#fff" stroke-width="{100 * bold_ratio * 2:g}" stroke-linejoin="round" ' if bold_ratio > 0 else '')
        + '/>\n'
        "</svg>\n"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", default=os.path.expanduser("~/.fonts/NotoSansCJKsc-Regular.otf"),
                    help="CJK 字体（「中」）")
    ap.add_argument("--latin-font", default="/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                    help="拉丁字体（「A」），要粗体：细体在 16px 下发虚")
    ap.add_argument("--ico", default=os.path.join(REPO, "..", "wind-setting", "res", "wind_setting.ico"))
    a = ap.parse_args()
    fonts = {"cjk": a.font, "latin": a.latin_font}
    for p in (*fonts.values(), a.ico):
        if not os.path.isfile(p):
            sys.exit(f"找不到 {p}")
    app_icons(a.ico)
    for name, (char, color, fkey, gratio, bratio) in MODES.items():
        for s in MODE_SIZES:
            save_png(mode_png(char, color, s, fonts[fkey], gratio, bratio), s, name)
        d = os.path.join(OUT, "scalable", "apps")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, name + ".svg"), "w", encoding="utf-8") as f:
            f.write(mode_svg(char, color, fonts[fkey], gratio, bratio))
    print(f"已写入 {OUT}")


if __name__ == "__main__":
    main()
