#!/usr/bin/env python3
"""一次性工具：生成方案包 / 主题包的 MIME 类型图标（产物已入库，打包时不跑它）。

    python3 scripts/linux/gen-mime-icons.py [--ico 应用图标.ico] [--font 中文字体] [--preview 目录]

产物：wind_linux/data/icons/hicolor/<尺寸>/mimetypes/ 下的
  application-x-windinput-package.png / application-x-windinput-theme.png（16…256 各一份）
  scalable/mimetypes/ 下同名 .svg。
图标名按 shared-mime-info 的默认规则由类型名推出（`/` 换成 `-`），scripts/linux/pkg/windinput-mime.xml
里也显式写了 `<icon name=…/>`。wind_linux/CMakeLists.txt 整棵 hicolor 树原样安装，无需改构建。

依赖（均为生成期依赖，不进包）：
  - Pillow          位图绘制
  - fontTools       角标文字取字形轮廓写进 SVG（SVG 里不留 <text>，不依赖用户装了什么字体）
  - scikit-image    把应用图标里的「风」字描成矢量轮廓（仓里只有它的位图源）
  - 一款含简体中文的字体（默认找 Noto Sans CJK SC，可用 --font 指定 .otf/.ttf/.ttc）
没 root 时装进临时 venv 即可：python3 -m venv /tmp/v && /tmp/v/bin/pip install pillow fonttools scikit-image

造型：文档页（折角）+ 居中的应用图标（蓝底白「风」，与 windinput 应用图标同源）+ 底部彩色角标：
方案包蓝色「方案」、主题包橙色「主题」。字只在 128 及以上出现：48/64 下中文只剩 8~10px，
放大看是一团糊，角标退成不带字的色块；16/22/24/32 再退成页脚的一条色带。两种包在任何尺寸
都靠颜色分得开。应用图标那块在位图里直接取 wind-setting 仓 res/wind_setting.ico 里
逐尺寸手调过的那一档（小尺寸比缩放清楚得多），矢量里用描出来的「风」轮廓。
"""
import argparse
import glob
import os

import numpy as np
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTCollection, TTFont
from PIL import Image, ImageDraw, ImageFont
from skimage import measure

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "wind_linux", "data", "icons", "hicolor")
DEFAULT_ICO = os.path.join(REPO, "..", "wind-setting", "res", "wind_setting.ico")
SIZES = [16, 22, 24, 32, 48, 64, 128, 256]
SS = 8  # 超采样倍数：几何按整像素对齐画在 SS 倍画布上再缩，边缘清楚又有抗锯齿

KINDS = {
    "application-x-windinput-package": ("方案", (0x1F, 0x6F, 0xD0)),
    "application-x-windinput-theme": ("主题", (0xE0, 0x6A, 0x1C)),
}
PAGE_FILL = (0xFF, 0xFF, 0xFF)
PAGE_EDGE = (0xB8, 0xC1, 0xCC)
FOLD_FILL = (0xDD, 0xE3, 0xEA)
MARK_BLUE = (0x2F, 0x93, 0xEE)  # 与应用图标底色一致（取自 wind_setting.ico）


def find_font(explicit):
    if explicit:
        return explicit
    pats = [
        "~/.fonts/NotoSansCJK*-Bold.otf", "~/.fonts/NotoSansCJK*.otf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-*.ttc",
        "/usr/share/fonts/**/NotoSansCJK*.otf", "/usr/share/fonts/**/NotoSansCJK*.ttc",
    ]
    for p in pats:
        hits = sorted(glob.glob(os.path.expanduser(p), recursive=True))
        if hits:
            return hits[0]
    raise SystemExit("找不到中文字体，请用 --font 指定")


def layout(s):
    """尺寸 s 下的几何（像素坐标，整数对齐）。小尺寸单独调，保证页边与色带落在整像素上。"""
    if s <= 24:
        m = 2 if s == 16 else 3
        page = (m, 1, s - m, s - 1)
        fold = s // 4 + (1 if s > 16 else 0)
        mk = s - 2 * m - 4  # 应用图标块边长
        mx = (s - mk) // 2
        my = page[1] + fold // 2 + 1
        band = (page[0] + 1, page[3] - 3 - (s > 16), page[2] - 1, page[3] - 1)
        return dict(page=page, fold=fold, edge=1, radius=1, mark=(mx, my, mk), band=band, text=None)
    if s == 32:
        page = (4, 1, 28, 31)
        return dict(page=page, fold=8, edge=1, radius=2, mark=(8, 7, 16), band=(5, 25, 27, 30), text=None)
    # ≥48：按 256 设计稿等比缩；角标的字只在 ≥128 画（更小就糊了，见文件头）
    k = s / 256
    r = lambda v: round(v * k)
    page = (r(36), r(8), s - r(36), s - r(8))
    return dict(
        page=page, fold=r(56), edge=max(1, r(5)), radius=r(14),
        mark=(r(72), r(44), r(112)),
        badge=(r(52), r(172), s - r(52), r(232)), badge_r=r(14),
        text=r(40) if s >= 128 else None,
    )


def load_marks(ico_path):
    ico = Image.open(ico_path)
    have = sorted(w for w, _ in ico.info["sizes"])
    out = {}
    for w in have:
        ico.size = (w, w)
        out[w] = ico.copy().convert("RGBA")
    return out


def mark_image(marks, size):
    """应用图标块：优先取 ico 里恰好这一档（手调过），没有就从大一档缩。"""
    if size in marks:
        return marks[size]
    src = min((w for w in marks if w > size), default=max(marks))
    return marks[src].resize((size, size), Image.LANCZOS)


def page_polygon(page, fold):
    x0, y0, x1, y1 = page
    return [(x0, y0), (x1 - fold, y0), (x1, y0 + fold), (x1, y1), (x0, y1)]


def draw_png(name, label, color, s, marks, font_path):
    L = layout(s)
    big = Image.new("RGBA", (s * SS, s * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    sc = lambda pts: [(x * SS, y * SS) for x, y in pts]
    x0, y0, x1, y1 = L["page"]
    fold = L["fold"]
    # 页：先画边色整块，再内缩一个边宽画白，得到均匀的描边（折角斜边同理）
    e = L["edge"]
    d.polygon(sc(page_polygon(L["page"], fold)), fill=PAGE_EDGE)
    inner = (x0 + e, y0 + e, x1 - e, y1 - e)
    d.polygon(sc(page_polygon(inner, max(1, fold - e))), fill=PAGE_FILL)
    d.polygon(sc([(x1 - fold, y0), (x1 - fold, y0 + fold), (x1, y0 + fold)]), fill=PAGE_EDGE)
    d.polygon(sc([(x1 - fold + e, y0 + e * 2), (x1 - fold + e, y0 + fold - e), (x1 - e * 2, y0 + fold - e)]),
              fill=FOLD_FILL)
    if L.get("band"):
        bx0, by0, bx1, by1 = L["band"]
        d.rectangle((bx0 * SS, by0 * SS, bx1 * SS - 1, by1 * SS - 1), fill=color)
    if L.get("badge"):
        bx0, by0, bx1, by1 = L["badge"]
        d.rounded_rectangle((bx0 * SS, by0 * SS, bx1 * SS, by1 * SS), radius=L["badge_r"] * SS, fill=color)
    img = big.resize((s, s), Image.LANCZOS)
    mx, my, mk = L["mark"]
    img.alpha_composite(mark_image(marks, mk), (mx, my))
    if L.get("text"):
        # 字直接按目标尺寸画（不走超采样）：Pillow 的字形光栅化自带微调，小字比缩放清楚
        bx0, by0, bx1, by1 = L["badge"]
        f = ImageFont.truetype(font_path, L["text"])
        td = ImageDraw.Draw(img)
        sw = 1 if L["text"] >= 36 else 0  # 常规字重在大尺寸下加 1px 描边当粗体；再小就糊成一团
        td.text(((bx0 + bx1) / 2, (by0 + by1) / 2), label, font=f, fill=(255, 255, 255),
                anchor="mm", stroke_width=sw, stroke_fill=(255, 255, 255))
    return img


def glyph_paths(font_path, text, size, cx, cy):
    """文字 → SVG path（字形轮廓，按 em 缩放后居中于 (cx, cy)）。"""
    if font_path.endswith(".ttc"):
        font = TTCollection(font_path).fonts[0]
    else:
        font = TTFont(font_path)
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    upm = font["head"].unitsPerEm
    k = size / upm
    names = [cmap[ord(c)] for c in text]
    total = sum(gs[n].width for n in names) * k
    x = cx - total / 2
    # CJK 字面框大致在 [-0.12em, 0.88em]：取中线 0.38em 与角标中线对齐
    base = cy + 0.38 * size
    ds = []
    for n in names:
        pen = SVGPathPen(gs)
        gs[n].draw(TransformPen(pen, (k, 0, 0, -k, x, base)))
        ds.append(pen.getCommands())
        x += gs[n].width * k
    return " ".join(ds)


def feng_path(marks, x, y, size):
    """应用图标 256 档里的白色「风」→ 轮廓 path（放到 (x, y) 起、边长 size 的方块里）。"""
    im = np.asarray(marks[max(marks)].convert("RGBA")).astype(float) / 255
    # 白字在蓝底上：R 通道从底色的 ~0.18 升到 1，取其归一化值当覆盖率
    r, a = im[..., 0], im[..., 3]
    cov = np.clip((r - MARK_BLUE[0] / 255) / (1 - MARK_BLUE[0] / 255), 0, 1) * (a > 0.5)
    cov = np.pad(cov, 1)
    n = cov.shape[0] - 2
    k = size / n
    parts = []
    for c in measure.find_contours(cov, 0.5):
        c = measure.approximate_polygon(c, tolerance=0.35)
        pts = " ".join(f"{x + (p[1] - 1) * k:.2f},{y + (p[0] - 1) * k:.2f}" for p in c)
        parts.append(f"M{pts}Z")
    return " ".join(parts)


def svg(name, label, color, marks, font_path):
    L = layout(256)
    x0, y0, x1, y1 = L["page"]
    fold, e = L["fold"], L["edge"]
    mx, my, mk = L["mark"]
    bx0, by0, bx1, by1 = L["badge"]
    hexc = lambda c: "#%02x%02x%02x" % c
    page = " ".join(f"{x},{y}" for x, y in page_polygon((x0 + e / 2, y0 + e / 2, x1 - e / 2, y1 - e / 2), fold))
    text_d = glyph_paths(font_path, label, L["text"], (bx0 + bx1) / 2, (by0 + by1) / 2)
    rr = round(mk * 0.22)  # 应用图标块的圆角（与 ico 的观感一致）
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <!-- 由 scripts/linux/gen-mime-icons.py 生成，别手改 -->
  <polygon points="{page}" fill="{hexc(PAGE_FILL)}" stroke="{hexc(PAGE_EDGE)}" stroke-width="{e}" stroke-linejoin="round"/>
  <polygon points="{x1 - fold},{y0 + e / 2} {x1 - fold},{y0 + fold} {x1 - e / 2},{y0 + fold}" fill="{hexc(FOLD_FILL)}" stroke="{hexc(PAGE_EDGE)}" stroke-width="{e}" stroke-linejoin="round"/>
  <rect x="{mx}" y="{my}" width="{mk}" height="{mk}" rx="{rr}" fill="{hexc(MARK_BLUE)}"/>
  <path d="{feng_path(marks, mx, my, mk)}" fill="#ffffff"/>
  <rect x="{bx0}" y="{by0}" width="{bx1 - bx0}" height="{by1 - by0}" rx="{L['badge_r']}" fill="{hexc(color)}"/>
  <path d="{text_d}" fill="#ffffff" stroke="#ffffff" stroke-width="{L['text'] / 28:.2f}" stroke-linejoin="round"/>
</svg>
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ico", default=DEFAULT_ICO)
    ap.add_argument("--font")
    ap.add_argument("--preview", help="另出一张放大拼图到此目录，便于肉眼检查小尺寸")
    a = ap.parse_args()
    font_path = find_font(a.font)
    marks = load_marks(a.ico)
    rows = []
    for name, (label, color) in KINDS.items():
        row = []
        for s in SIZES:
            img = draw_png(name, label, color, s, marks, font_path)
            d = os.path.join(OUT, f"{s}x{s}", "mimetypes")
            os.makedirs(d, exist_ok=True)
            img.save(os.path.join(d, name + ".png"), optimize=True)
            row.append(img)
        rows.append(row)
        d = os.path.join(OUT, "scalable", "mimetypes")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, name + ".svg"), "w", encoding="utf-8") as f:
            f.write(svg(name, label, color, marks, font_path))
    if a.preview:
        # 每档按最近邻放大到 8 倍（最大 256）并排，看得清每个像素
        cells = [[im.resize((min(256, im.width * 8),) * 2, Image.NEAREST) for im in row] for row in rows]
        w = sum(c.width + 8 for c in cells[0])
        sheet = Image.new("RGBA", (w, 2 * 264), (0xC8, 0xC8, 0xC8, 255))
        for ri, row in enumerate(cells):
            x = 0
            for c in row:
                sheet.alpha_composite(c, (x, ri * 264))
                x += c.width + 8
        os.makedirs(a.preview, exist_ok=True)
        sheet.save(os.path.join(a.preview, "mime-icons-preview.png"))
        actual = Image.new("RGBA", (sum(s + 8 for s in SIZES), 2 * 264), (0xF0, 0xF0, 0xF0, 255))
        for ri, row in enumerate(rows):
            x = 0
            for im in row:
                actual.alpha_composite(im, (x, ri * 264))
                x += im.width + 8
        actual.save(os.path.join(a.preview, "mime-icons-actual.png"))


if __name__ == "__main__":
    main()
