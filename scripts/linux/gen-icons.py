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

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "wind_linux", "data", "icons", "hicolor")
APP_SIZES = [16, 22, 24, 32, 48, 64, 128, 256]
MODE_SIZES = [16, 22, 24, 32, 48, 64]

# 中文态用应用图标的品牌蓝，英文态用石板灰：两态一眼可分，白字在两种底色上都够对比。
MODES = {
    "windinput-zh": ("中", "#2F86E6"),
    "windinput-en": ("英", "#5E6B78"),
}
RADIUS = 0.22  # 圆角半径 / 边长（与应用图标的圆角接近）
GLYPH = 0.74  # 字身 / 边长
BOLD = 0.035  # 描边宽 / 边长：Regular 字重在 16px 下太细，加粗一档


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


def mode_png(char, color, size, font_path):
    # 8 倍超采样再缩：直接按小尺寸画，圆角与字边都是锯齿。
    k = 8
    n = size * k
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, n - 1, n - 1], radius=int(n * RADIUS), fill=color)
    font = ImageFont.truetype(font_path, int(n * GLYPH))
    stroke = max(1, int(n * BOLD))
    d.text((n / 2, n / 2), char, font=font, fill="white", anchor="mm",
           stroke_width=stroke, stroke_fill="white")
    return img.resize((size, size), Image.LANCZOS)


def mode_svg(char, color, font_path):
    font = TTFont(font_path, fontNumber=0)
    gs = font.getGlyphSet()
    name = font.getBestCmap()[ord(char)]
    upm = font["head"].unitsPerEm
    # 画布 100×100。字形放进「字身」框：按 em 缩放、水平按步进居中、垂直按 ascender/descender 居中
    # （与 Pillow 的 anchor="mm" 同一口径，两种产物的字位一致）。
    scale = 100 * GLYPH / upm
    adv = gs[name].width
    hhea = font["hhea"]
    mid = (hhea.ascent + hhea.descent) / 2
    dx = 50 - adv * scale / 2
    dy = 50 + mid * scale
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
    gs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, dx, dy)))
    r = 100 * RADIUS
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">\n'
        f'  <rect width="100" height="100" rx="{r:g}" ry="{r:g}" fill="{color}"/>\n'
        f'  <path d="{pen.getCommands()}" fill="#fff" stroke="#fff" '
        f'stroke-width="{100 * BOLD * 2:g}" stroke-linejoin="round"/>\n'
        "</svg>\n"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", default=os.path.expanduser("~/.fonts/NotoSansCJKsc-Regular.otf"))
    ap.add_argument("--ico", default=os.path.join(REPO, "..", "wind-setting", "res", "wind_setting.ico"))
    a = ap.parse_args()
    for p in (a.font, a.ico):
        if not os.path.isfile(p):
            sys.exit(f"找不到 {p}")
    app_icons(a.ico)
    for name, (char, color) in MODES.items():
        for s in MODE_SIZES:
            save_png(mode_png(char, color, s, a.font), s, name)
        d = os.path.join(OUT, "scalable", "apps")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, name + ".svg"), "w", encoding="utf-8") as f:
            f.write(mode_svg(char, color, a.font))
    print(f"已写入 {OUT}")


if __name__ == "__main__":
    main()
