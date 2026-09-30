#!/usr/bin/env python3
"""一次性工具：生成 wind_linux/data/icons/hicolor 下的应用图标 `windinput`（产物已入库，打包时不跑它）。

    python3 scripts/linux/gen-icons.py [--ico 应用图标.ico]

依赖 Pillow。`windinput` 是输入法条目 / 设置程序的图标，只有位图源（兄弟仓库 wind-setting 的
`res/wind_setting.ico`，逐尺寸手调过的 16…256），照原样按尺寸拆出来；22px 那档 ico 里没有，
由 24px 缩。没有矢量源，故不出 scalable——拿位图包一层 SVG 不是真的可缩放。

托盘 / 面板随模式切换的 `windinput-zh` / `-en` / `-caps` / `-zh-<方案>` **不归本脚本**，由
`wind_input/crates/wind-ui/examples/gen_tray_icons.rs` 生成（16px 手绘点阵 + 大尺寸字形栅格），
见 docs/design/linux-port.md §5d。
"""
import argparse
import os
import sys

from PIL import Image

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "wind_linux", "data", "icons", "hicolor")
APP_SIZES = [16, 22, 24, 32, 48, 64, 128, 256]


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ico", default=os.path.join(REPO, "..", "wind-setting", "res", "wind_setting.ico"))
    a = ap.parse_args()
    if not os.path.isfile(a.ico):
        sys.exit(f"找不到 {a.ico}")
    app_icons(a.ico)
    print(f"已写入 {OUT}")


if __name__ == "__main__":
    main()
