#!/usr/bin/env python3
"""wind_linux 端到端测试的「应用」一侧：经 DBus 扮演一个文本框，对真实 fcitx5 打字。

由 scripts/linux/e2e.sh 在 dbus-run-session 里调用（那边已起好 Rust 服务与 fcitx5）。
每条用例输出一行 `PASS <名字>` 或 `FAIL <名字>: <原因>`，有任何 FAIL 则退出码非 0。

协议：org.fcitx.Fcitx5 的 InputMethod1.CreateInputContext 建上下文，InputContext1.
ProcessKeyEvent 送键（返回值 = 输入法是否吃掉），信号 CommitString / UpdateFormattedPreedit
回传上屏与预编辑。
"""
import asyncio
import os
import subprocess
import sys
import time

from dbus_next.aio import MessageBus
from dbus_next import Variant  # noqa: F401  (保留：调试时常用)

SERVICE = "org.fcitx.Fcitx5"
IM_PATH = "/org/freedesktop/portal/inputmethod"
IM_IFACE = "org.fcitx.Fcitx.InputMethod1"
IC_IFACE = "org.fcitx.Fcitx.InputContext1"

# fcitx CapabilityFlag：Preedit | FormattedPreedit —— 让预编辑走 UpdateFormattedPreedit 信号。
CAP_PREEDIT = (1 << 1) | (1 << 4)
CAP_PASSWORD = 1 << 3
STATE_SHIFT = 1 << 0
STATE_LOCK = 1 << 1
STATE_CTRL = 1 << 2

# X11 keysym 与 US 布局的 X keycode（evdev + 8）。
KEYSYM = {c: ord(c) for c in "abcdefghijklmnopqrstuvwxyz0123456789 "}
KEYSYM.update({"BackSpace": 0xFF08, "Return": 0xFF0D, "Escape": 0xFF1B, "Shift_L": 0xFFE1,
               "Caps_Lock": 0xFFE5,
               "Left": 0xFF51, "Up": 0xFF52, "Right": 0xFF53, "Down": 0xFF54})
KEYCODE = {
    **dict(zip("qwertyuiop", range(24, 34))),
    **dict(zip("asdfghjkl", range(38, 47))),
    **dict(zip("zxcvbnm", range(52, 59))),
    **dict(zip("1234567890", range(10, 20))),
    " ": 65, "Return": 36, "Escape": 9, "BackSpace": 22, "Shift_L": 50, "Caps_Lock": 66,
    "Left": 113, "Up": 111, "Right": 114, "Down": 116,
}


class Ctx:
    """一个输入上下文（≈ 应用里的一个文本框）及其收到的信号。"""

    def __init__(self, bus, path):
        self.path = path
        self.commits = []
        self.preedits = []  # 每次 UpdateFormattedPreedit 的纯文本
        self.forwarded = []
        self.bus = bus

    async def init(self, caps):
        intro = await self.bus.introspect(SERVICE, self.path)
        obj = self.bus.get_proxy_object(SERVICE, self.path, intro)
        self.ic = obj.get_interface(IC_IFACE)
        self.ic.on_commit_string(lambda s: self.commits.append(s))
        self.ic.on_update_formatted_preedit(
            lambda segs, cursor: self.preedits.append("".join(s for s, _ in segs)))
        self.ic.on_forward_key(lambda sym, state, rel: self.forwarded.append((sym, state, rel)))
        await self.ic.call_set_capability(caps)

    @property
    def preedit(self):
        return self.preedits[-1] if self.preedits else ""

    async def key(self, name, state=0):
        sym = KEYSYM[name]
        code = KEYCODE.get(name, 0)
        eaten = await self.ic.call_process_key_event(sym, code, state, False, 0)
        await self.ic.call_process_key_event(sym, code, state | 0, True, 0)
        await asyncio.sleep(0.03)
        return eaten

    async def type(self, text):
        return [await self.key(c) for c in text]

    async def tap_shift(self, state=0):
        # 单击 Shift：按下时 state 不含 Shift，松开时含（X11 语义：state 是事件前的修饰态）。
        code = KEYCODE["Shift_L"]
        sym = KEYSYM["Shift_L"]
        await self.ic.call_process_key_event(sym, code, state, False, 0)
        await self.ic.call_process_key_event(sym, code, state | STATE_SHIFT, True, 0)
        await asyncio.sleep(0.15)

    async def tap_caps_lock(self, locked_before):
        # 按一下 CapsLock，state 按真实 X11 的样子给（Xvfb + xev 实测）：开 → 按下 0、松开 Lock；
        # 关 → 按下 Lock、松开**仍是** Lock（XKB 在松开时才解锁，state 是事件之前的）。
        code = KEYCODE["Caps_Lock"]
        sym = KEYSYM["Caps_Lock"]
        before = STATE_LOCK if locked_before else 0
        await self.ic.call_process_key_event(sym, code, before, False, 0)
        await self.ic.call_process_key_event(sym, code, STATE_LOCK, True, 0)
        await asyncio.sleep(0.15)

    def take(self):
        """取出并清空累计的上屏文本。"""
        s = "".join(self.commits)
        self.commits.clear()
        return s


results = []


def check(name, ok, why=""):
    results.append(ok)
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f": {why}"), flush=True)


async def new_ctx(bus, im, program, caps=CAP_PREEDIT):
    path, _uuid = await im.call_create_input_context([["program", program], ["display", ""]])
    c = Ctx(bus, path)
    await c.init(caps)
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)  # activate → 服务端 FocusGained
    return c


# ── 候选窗（X11）辅助 ───────────────────────────────────────────────

def sh(*args):
    return subprocess.run(args, capture_output=True, text=True).stdout


def candidate_window(classname="wind-candidate"):
    """按 WM_CLASS 实例名找窗（候选窗 wind-candidate；浮层 wind-status / wind-toast /
    wind-tooltip），返回 (id, x, y, w, h, 是否可见)；没建过返回 None。"""
    ids = sh("xdotool", "search", "--classname", classname).split()
    if not ids:
        return None
    wid = ids[0]
    info = sh("xwininfo", "-id", wid)
    def field(name):
        for line in info.splitlines():
            if line.strip().startswith(name):
                return line.split(":", 1)[1].strip()
        return ""
    return (wid, int(field("Absolute upper-left X")), int(field("Absolute upper-left Y")),
            int(field("Width")), int(field("Height")), field("Map State") == "IsViewable")


def xwd_to_png(xwd_bytes, png_path):
    """把 `xwd -root` 的输出转成 PNG（只支持每像素 4 字节的 ZPixmap，Xvfb 24 位深就是它）。
    返回 (宽, 高, 取像素函数)。不引第三方库：e2e 环境里没有 PIL。"""
    import struct
    import zlib
    hdr = struct.unpack(">25I", xwd_bytes[:100])
    header_size, width, height = hdr[0], hdr[4], hdr[5]
    byte_order, bpp, bytes_per_line, ncolors = hdr[7], hdr[11], hdr[12], hdr[19]
    # 有合成器时 xwd 会在头里写 bits_per_pixel=24，实际每行仍按 4 字节/像素排（bytes_per_line
    # = 宽×4）——以行宽为准。
    assert bytes_per_line >= width * 4, f"只支持每像素 4 字节，头 {hdr[:20]}（bpp={bpp}）"
    off = header_size + ncolors * 12
    data = xwd_bytes[off:off + bytes_per_line * height]

    def px(x, y):
        i = y * bytes_per_line + x * 4
        b = data[i:i + 4]
        # LSBFirst(0)：内存里依次 B G R x
        return (b[2], b[1], b[0]) if byte_order == 0 else (b[1], b[2], b[3])

    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            raw.extend(px(x, y))
    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b"")
    open(png_path, "wb").write(png)
    return width, height, px


def screenshot(name, wid=None):
    """截全屏（wid=None）或只截候选窗本身。后者取的是窗口自己的像素（不经合成器），
    用来确认贴上去的位图没错；前者看它在屏幕上摆在哪。"""
    shots = os.environ.get("WIND_E2E_SHOTS", "")
    target = ["-id", wid] if wid else ["-root"]
    xwd = subprocess.run(["xwd", *target, "-silent"], capture_output=True).stdout
    path = os.path.join(shots, name + ".png")
    _, _, px = xwd_to_png(xwd, path)
    return path, px


async def x11_cases(bus, im):
    """P2：候选窗的出现、位置、生命周期、鼠标选词。文字后端就绪前帧里可能没有字，
    这里只断言窗口本身：有像素、摆对地方、该藏时藏、点得动。"""
    c = await new_ctx(bus, im, "e2e-x11")
    # 光标在 (300, 400)，行高 20：候选窗应出现在光标下方附近。
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)
    await c.type("nihao")
    await asyncio.sleep(0.4)
    win = candidate_window()
    check("X11：组字时候选窗出现（已映射、非零尺寸）",
          win is not None and win[5] and win[3] > 0 and win[4] > 0, f"window={win}")
    if not win:
        return
    _, x, y, w, h, _ = win
    print(f"  候选窗几何：{w}x{h} @ ({x},{y})，光标 (300,400) 行高 20", flush=True)
    check("X11：候选窗左上角落在光标下方附近",
          240 <= x <= 330 and 400 <= y <= 470, f"@({x},{y})")
    path, px = screenshot("candidate_nihao")
    colors = {px(xx, yy) for xx in range(x, x + w, max(1, w // 40))
              for yy in range(y, y + h, max(1, h // 20))}
    check("X11：候选窗区域确有绘制（截图里不是单一颜色）", len(colors) > 1,
          f"采样到 {len(colors)} 种颜色，截图 {path}")
    print(f"  截图：{path}", flush=True)
    wpath, _ = screenshot("candidate_window_only", win[0])
    print(f"  候选窗自身像素：{wpath}", flush=True)

    await c.key(" ")
    await asyncio.sleep(0.3)
    win = candidate_window()
    c.take()
    check("X11：上屏后候选窗隐藏", win is not None and not win[5], f"window={win}")

    await c.type("nihao")
    await c.key("Escape")
    await asyncio.sleep(0.3)
    win = candidate_window()
    check("X11：Esc 后候选窗隐藏", win is not None and not win[5], f"window={win}")

    # 滚轮：服务端把它解释成「上下键调整高亮项」——窗口像素要变、不上屏。
    await c.type("nihao")
    await asyncio.sleep(0.3)
    win = candidate_window()
    changed = False
    if win and win[5]:
        before = subprocess.run(["xwd", "-id", win[0], "-silent"], capture_output=True).stdout
        sh("xdotool", "mousemove", str(win[1] + win[3] // 2), str(win[2] + win[4] // 2),
           "click", "5")
        await asyncio.sleep(0.3)
        after = subprocess.run(["xwd", "-id", win[0], "-silent"], capture_output=True).stdout
        changed = before != after
        screenshot("candidate_after_scroll", win[0])
    got = c.take()
    check("X11：滚轮下滚 → 高亮移动（窗口像素变化）且不上屏", changed and got == "",
          f"changed={changed} 上屏={got!r}")
    await c.key("Escape")
    sh("xdotool", "mousemove", "5", "5")
    await asyncio.sleep(0.2)

    # 鼠标选词：候选的命中矩形只有服务端知道，沿窗口中线从左往右逐点点击，直到有上屏。
    await c.type("nihao")
    await asyncio.sleep(0.3)
    win = candidate_window()
    got = ""
    if win and win[5]:
        _, x, y, w, h, _ = win
        for dx in range(4, w, 8):
            sh("xdotool", "mousemove", str(x + dx), str(y + h // 2), "click", "1")
            await asyncio.sleep(0.12)
            got = c.take()
            if got:
                break
    check("X11：鼠标点候选 → 上屏（CMD_CANDIDATE_SELECT → push 回来的 CommitText）",
          got != "", "点遍中线都没有上屏")
    await asyncio.sleep(0.3)
    win = candidate_window()
    check("X11：鼠标选词后候选窗隐藏", win is not None and not win[5], f"window={win}")
    sh("xdotool", "mousemove", "5", "5")

    # 屏幕底边：光标在 (300, 780)，下方放不下 → 翻到光标上方，且不出屏。
    await c.ic.call_set_cursor_rect(300, 780, 2, 16)
    await c.type("nihao")
    await asyncio.sleep(0.4)
    win = candidate_window()
    ok = win is not None and win[5] and win[2] + win[4] <= 800 and win[2] < 780
    check("X11：屏幕底边时翻到光标上方、不出屏", ok, f"window={win}")
    if win:
        screenshot("candidate_flipped")
    await c.key("Escape")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


async def wait_window(classname, visible, timeout=3.0):
    """等某个窗口进入指定可见态，返回最后一次查到的窗口信息（超时也返回，由调用方断言）。"""
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        win = candidate_window(classname)
        if (win is not None and win[5]) == visible:
            return win
        if asyncio.get_running_loop().time() >= deadline:
            return win
        await asyncio.sleep(0.05)


def window_has_content(wid, name):
    """截窗口自身像素存 PNG，返回 (路径, 颜色种数)。"""
    path, px = screenshot(name, wid)
    info = sh("xwininfo", "-id", wid)
    w = h = 0
    for line in info.splitlines():
        if line.strip().startswith("Width"):
            w = int(line.split(":", 1)[1])
        if line.strip().startswith("Height"):
            h = int(line.split(":", 1)[1])
    colors = {px(x, y) for x in range(0, w, max(1, w // 60)) for y in range(0, h, max(1, h // 20))}
    return path, len(colors)


async def overlay_cases(bus, im):
    """光栅浮层：状态气泡 / Toast / 悬停提示。服务光栅化 → 各层 SHM → CMD_OVERLAY_FRAME →
    addon 各自一个 override-redirect 窗口。断言窗口出现、位置合理、有像素、到点消失。
    字形对不对要看截图（路径打印在输出里）。"""
    c = await new_ctx(bus, im, "e2e-overlay")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)

    # a) Shift 单击切中英：光标下方弹状态气泡，几秒后自己消失（计时在 addon）。
    await c.tap_shift()
    win = await wait_window("wind-status", True)
    check("浮层：Shift 切中英后状态气泡出现", win is not None and win[5], f"window={win}")
    if win and win[5]:
        _, x, y, w, h, _ = win
        print(f"  状态气泡：{w}x{h} @ ({x},{y})，光标 (300,400) 行高 20", flush=True)
        check("浮层：状态气泡落在光标下方附近", 280 <= x <= 330 and 410 <= y <= 450,
              f"@({x},{y})")
        path, n = window_has_content(win[0], "status_toggle_en")
        check("浮层：状态气泡有绘制（不是单色）", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  状态气泡截图：{path}", flush=True)
        win = await wait_window("wind-status", False, timeout=8.0)
        check("浮层：状态气泡到点自动消失", win is not None and not win[5], f"window={win}")
    await c.tap_shift()  # 切回中文
    await wait_window("wind-status", True)

    # b) Ctrl+Shift+E 切方案：气泡文字换成方案名（字形看截图）。
    await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, False, 0)
    await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, True, 0)
    await asyncio.sleep(0.3)
    win = await wait_window("wind-status", True)
    ok = win is not None and win[5]
    check("浮层：Ctrl+Shift+E 切方案后状态气泡出现", ok, f"window={win}")
    if ok:
        path, n = window_has_content(win[0], "status_switch_schema")
        check("浮层：切方案气泡有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  切方案气泡截图：{path}", flush=True)
        full, _ = screenshot("status_switch_schema_screen")
        print(f"  全屏截图：{full}", flush=True)
    await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, False, 0)
    await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, True, 0)
    await asyncio.sleep(0.3)

    # c) Toast：`wind_input ui toast` 经 RPC 让在线服务弹一条（右上角，1.5 秒）。
    svc = os.path.join(os.environ.get("W", ""), "svc", "wind_input")
    r = await asyncio.to_thread(subprocess.run,
                                [svc, "ui", "toast", "输入法已就绪", "--pos", "top_right",
                                 "--ms", "1500"], capture_output=True, text=True)
    win = await wait_window("wind-toast", True)
    ok = win is not None and win[5]
    check("浮层：ui toast 后 Toast 出现", ok,
          f"window={win} cli rc={r.returncode} {r.stderr.strip()[:200]}")
    if ok:
        _, x, y, w, h, _ = win
        print(f"  Toast：{w}x{h} @ ({x},{y})，屏幕 1280x800", flush=True)
        check("浮层：Toast 在右上角（离边留白）", x + w >= 1280 - 40 and y <= 40, f"@({x},{y})")
        path, n = window_has_content(win[0], "toast_top_right")
        check("浮层：Toast 有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  Toast 截图：{path}", flush=True)
        win = await wait_window("wind-toast", False, timeout=6.0)
        check("浮层：Toast 到点自动消失", win is not None and not win[5], f"window={win}")

    # d) 悬停提示：鼠标停在候选上 → 候选窗旁出现 tooltip；Esc 收掉候选窗时一起藏。
    await c.type("nihao")
    await asyncio.sleep(0.4)
    cand = candidate_window()
    tip = None
    if cand and cand[5]:
        _, x, y, w, h, _ = cand
        for dx in range(6, w, 10):
            sh("xdotool", "mousemove", str(x + dx), str(y + h // 2))
            tip = await wait_window("wind-tooltip", True, timeout=0.6)
            if tip and tip[5]:
                break
    ok = tip is not None and tip[5]
    check("浮层：鼠标悬停候选后 tooltip 出现", ok, f"tooltip={tip} candidate={cand}")
    if ok:
        _, tx, ty, tw, th, _ = tip
        print(f"  tooltip：{tw}x{th} @ ({tx},{ty})，候选窗 {cand[3]}x{cand[4]} @ ({cand[1]},{cand[2]})",
              flush=True)
        check("浮层：tooltip 在候选窗下方", ty >= cand[2] + cand[4] // 2, f"tooltip@({tx},{ty})")
        path, n = window_has_content(tip[0], "tooltip_hover")
        check("浮层：tooltip 有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  tooltip 截图：{path}", flush=True)
        full, _ = screenshot("tooltip_hover_screen")
        print(f"  全屏截图：{full}", flush=True)
    await c.key("Escape")
    tip = await wait_window("wind-tooltip", False, timeout=2.0)
    check("浮层：候选窗收起时 tooltip 一起隐藏", tip is None or not tip[5], f"tooltip={tip}")
    sh("xdotool", "mousemove", "5", "5")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


# ── 自绘菜单（X11）─────────────────────────────────────────────────────
# 菜单由服务光栅化（复用 Windows 的 popup_menu），每级一个窗口 wind-menu-<级>；addon 在菜单
# 打开期间抓住指针把原始事件报回服务。行高与内边距取默认主题：条目 27px、上下留白约 4px，
# 分隔线 7px——下面按它估算行位置，点错了效果断言会红，不会静默通过。
MENU_ROW = 27


def menu_win(level=0):
    return candidate_window(f"wind-menu-{level}")


def xwd_raw(wid):
    return subprocess.run(["xwd", "-id", wid, "-silent"], capture_output=True).stdout


def menus_gone():
    return all(w is None or not w[5] for w in (menu_win(k) for k in range(6)))


async def wait_menus_gone(timeout=3.0):
    return await wait_until(menus_gone, timeout)


async def typing_works(c, label):
    """关菜单后立刻打字：nihao+空格 → 你好（菜单态没卡住方向键 / 回车 / 字母）。"""
    await c.type("nihao")
    await c.key(" ")
    got = c.take()
    check(f"菜单：{label}后立即能正常打字", got == "你好", f"上屏={got!r}")


async def probe_row(win, from_bottom=False):
    """找菜单窗口里第一个（或最后一个）可选行的纵坐标：从边缘往里逐步移动鼠标，窗口像素一变（高亮
    亮起）就是碰到了那一行。软投影扩边的宽度随主题而定，不能按固定偏移猜。返回行内偏里一点的 y。"""
    wid, x, y, w, h, _ = win
    base = xwd_raw(wid)
    # 走满整个窗口：第一行可能是禁用项（候选菜单首候选的「置顶」），禁用项不高亮。
    steps = range(h - 2, 1, -3) if from_bottom else range(2, h - 1, 3)
    for dy in steps:
        sh("xdotool", "mousemove", str(x + w // 3), str(y + dy))
        await asyncio.sleep(0.08)
        if xwd_raw(wid) != base:
            return y + dy + (-8 if from_bottom else 8)
    return None


async def hover_candidate_point(c):
    """候选的命中矩形只有服务端知道：沿候选窗中线移动鼠标，悬停提示出现处就在某个候选上。"""
    cand = candidate_window()
    if not (cand and cand[5]):
        return None
    _, x, y, w, h, _ = cand
    for dx in range(6, w, 10):
        sh("xdotool", "mousemove", str(x + dx), str(y + h // 2))
        tip = await wait_window("wind-tooltip", True, timeout=0.6)
        if tip and tip[5]:
            return (x + dx, y + h // 2)
    return None


async def open_candidate_menu(c):
    """组字 → 右键某个候选 → 候选菜单。返回 (菜单窗口, 右键点)。"""
    await c.type("nihao")
    await asyncio.sleep(0.4)
    pt = await hover_candidate_point(c)
    if not pt:
        return None, None
    sh("xdotool", "mousemove", str(pt[0]), str(pt[1]), "click", "3")
    return await wait_window("wind-menu-0", True), pt


_IMPANEL = None


async def impanel(bus):
    """同一条总线上只能有一个 kimpanel 替身（名字与对象路径都只能占一次）。"""
    global _IMPANEL
    if _IMPANEL is None:
        _IMPANEL = Impanel(bus)
        await _IMPANEL.start()
    return _IMPANEL


async def open_main_menu(c, tries=1):
    """功能主菜单的鼠标入口只剩组字时的候选窗：右键候选 → 候选菜单末行「更多…」→ 主菜单
    （空闲时的状态区入口是「清风输入法设置」，不再开菜单）。返回主菜单窗口或 None。
    组字（nihao）在菜单关掉后仍挂着，调用方用 end_composition 收掉再验打字。

    tries > 1：服务刚重启时 socket 先于整个服务就绪，menu.open 可能被当「未就绪」丢掉，重试。"""
    for _ in range(tries):
        menu, _pt = await open_candidate_menu(c)
        if menu and menu[5]:
            last = await probe_row(menu, from_bottom=True)
            if last is not None:
                sh("xdotool", "mousemove", str(menu[1] + menu[3] // 3), str(last), "click", "1")
                await asyncio.sleep(0.5)
                main = await wait_window("wind-menu-0", True)
                if main and main[5] and main[4] > menu[4]:
                    return main
        await c.key("Escape")  # 候选菜单若还开着，这一下收菜单；下面那下收组字
        await wait_menus_gone()
        await end_composition(c)
    return None


async def end_composition(c):
    """收掉仍挂着的组字（没有组字时这一下 Esc 交还宿主，无副作用）。"""
    await c.key("Escape")
    c.take()
    c.preedits.clear()


def tray_icon(tag, label):
    """服务端运行时渲染的托盘图标名（与 wind_ui::tray_icon::icon_name / addon 的
    dynamicIconName 同一规则）：windinput-lbl-<zh|en|caps>-<主字 UTF-8 十六进制>。"""
    return f"windinput-lbl-{tag}-{label.encode().hex()}"


ICON_PIN = tray_icon("zh", "拼")
ICON_WU = tray_icon("zh", "五")
ICON_EN = tray_icon("en", "英")
ICON_CAPS = tray_icon("caps", "A")


def tray_icon_files_ok(name):
    """这个图标名在用户图标目录（$XDG_DATA_HOME/icons/hicolor）里各尺寸都在，且是尺寸对得上的 PNG。"""
    root = os.path.join(os.environ["XDG_DATA_HOME"], "icons", "hicolor")
    for n in (16, 22, 24, 32, 48, 64):
        path = os.path.join(root, f"{n}x{n}", "apps", name + ".png")
        try:
            with open(path, "rb") as f:
                head = f.read(24)
        except OSError:
            return False, f"缺 {path}"
        if head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
            return False, f"不是 PNG：{path}"
        w, h = int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")
        if (w, h) != (n, n):
            return False, f"{path} 尺寸 {w}x{h}"
    return True, ""


class KimpanelWatch:
    """听 Fcitx5 kimpanel 模块发给面板的属性：`/Fcitx/im:<名>:<图标>:<子模式>:menu,label=<标签>`
    是当前输入法的托盘图标（取自引擎的 subModeIcon，notificationitem 的 SNI 图标取的是同一个
    值）；`RegisterProperties` 里是状态区动作。"""

    def __init__(self):
        self.props = []
        self.registered = []

    async def start(self, bus):
        name, path = "org.kde.kimpanel.inputmethod", "/kimpanel"
        intro = await bus.introspect(name, path)
        iface = bus.get_proxy_object(name, path, intro).get_interface(name)
        iface.on_update_property(lambda p: self.props.append(p))
        iface.on_register_properties(lambda ps: self.registered.append(list(ps)))

    def im(self):
        for p in reversed(self.props):
            if p.startswith("/Fcitx/im:"):
                return p
        return ""

    def icon(self):
        parts = self.im().split(":")
        return parts[2] if len(parts) > 2 else ""


_KIMPANEL_WATCH = None


async def kimpanel_watch(bus):
    global _KIMPANEL_WATCH
    if _KIMPANEL_WATCH is None:
        await impanel(bus)  # kimpanel 模块看见面板在场才导出 org.kde.kimpanel.inputmethod
        w = KimpanelWatch()
        await w.start(bus)
        _KIMPANEL_WATCH = w
    return _KIMPANEL_WATCH


class Impanel:
    """扮演 KDE / GNOME 的 kimpanel 面板：Fcitx5 kimpanel 模块据此把状态区动作挂出来，
    面板点动作时发 `TriggerProperty("/Fcitx/<动作名>")`——就是用户点「清风输入法设置」。"""

    def __init__(self, bus):
        from dbus_next.service import ServiceInterface, signal

        class Iface(ServiceInterface):
            def __init__(self):
                super().__init__("org.kde.impanel")

            @signal()
            def TriggerProperty(self, key) -> "s":  # noqa: N802
                return key

        self.iface = Iface()
        self.bus = bus

    async def start(self):
        self.bus.export("/org/kde/impanel", self.iface)
        await self.bus.request_name("org.kde.impanel")
        await asyncio.sleep(0.5)  # kimpanel 模块看见名字出现后才接信号

    def trigger(self, key):
        self.iface.TriggerProperty(key)


async def menu_cases(bus, im):
    """自绘菜单：入口、渲染、悬停 / 键盘 / 点选、子菜单、动作生效，以及每一条关闭路径——
    每条关闭后立刻打字，验证 menu_open 没有卡住输入。"""
    watch = await kimpanel_watch(bus)
    c = await new_ctx(bus, im, "e2e-menu")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)

    # a) 右键候选 → 候选菜单，出现在右键处。
    menu, pt = await open_candidate_menu(c)
    ok = menu is not None and menu[5]
    check("菜单：右键候选弹出候选菜单", ok, f"menu={menu} 右键点={pt}")
    if not ok:
        await c.key("Escape")
        return
    _, mx, my, mw, mh, _ = menu
    print(f"  候选菜单：{mw}x{mh} @ ({mx},{my})，右键点 {pt}", flush=True)
    # 窗口 = 内容 − 软投影扩边：内容左上才是右键点，窗口左上在它左上方几个到十几个像素。
    check("菜单：候选菜单出现在右键处（内容左上 = 右键点，窗口含投影扩边）",
          0 <= pt[0] - mx <= 30 and 0 <= pt[1] - my <= 30, f"@({mx},{my}) 右键点 {pt}")
    path, n = window_has_content(menu[0], "menu_candidate")
    check("菜单：候选菜单有绘制", n > 1, f"{n} 种颜色，截图 {path}")
    print(f"  候选菜单截图：{path}", flush=True)
    full, _ = screenshot("menu_candidate_screen")
    print(f"  全屏截图：{full}", flush=True)

    # b) 悬停高亮：鼠标移到第一项上，窗口像素要变。
    row0 = await probe_row(menu)
    check("菜单：悬停可选项 → 高亮（像素变化）", row0 is not None, "从上往下移遍窗口，像素一直没变")
    path, _ = window_has_content(menu[0], "menu_candidate_hover")
    print(f"  悬停高亮截图：{path}", flush=True)

    # c) 键盘：菜单开着时按键被吃；↓ 移动高亮；普通字母关菜单且不外泄（不组字、不上屏）。
    before = xwd_raw(menu[0])
    eaten = await c.key("Down")
    await asyncio.sleep(0.3)
    after = xwd_raw(menu[0])
    check("菜单：↓ 被吃掉、高亮移动", eaten is True and before != after,
          f"eaten={eaten} 像素变化={before != after}")
    c.preedits.clear()
    eaten = await c.key("a")
    gone = await wait_menus_gone()
    check("菜单：普通字母关菜单、被吃掉、不外泄", eaten is True and gone and c.take() == "",
          f"eaten={eaten} 菜单消失={gone}")
    await c.key("Escape")  # 收掉组字
    c.take()
    await typing_works(c, "字母关闭")

    # d) 「更多…」→ 功能主菜单；主菜单比候选菜单高，截图看字形 / 勾选 / 分隔线 / 子菜单箭头。
    menu, pt = await open_candidate_menu(c)
    main = None
    if menu and menu[5]:
        last = await probe_row(menu, from_bottom=True)
        if last is not None:
            sh("xdotool", "mousemove", str(menu[1] + menu[3] // 3), str(last), "click", "1")
            await asyncio.sleep(0.5)
            main = await wait_window("wind-menu-0", True)
    ok = main is not None and main[5] and main[4] > (menu[4] if menu else 0)
    check("菜单：候选菜单「更多…」→ 功能主菜单", ok, f"候选菜单={menu} 主菜单={main}")
    if ok:
        _, mx, my, mw, mh, _ = main
        path, n = window_has_content(main[0], "menu_main")
        check("菜单：主菜单有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  主菜单：{mw}x{mh} @ ({mx},{my})，截图 {path}", flush=True)
        # e) 子菜单：悬停第 0 行「输入方案 ▸」→ 第 1 级出现在右侧。
        await probe_row(main)
        sub = await wait_window("wind-menu-1", True)
        ok = sub is not None and sub[5] and sub[1] >= mx + mw - 10
        check("菜单：悬停「输入方案」→ 子菜单在右侧展开", ok, f"sub={sub} main=({mx},{my},{mw})")
        if ok:
            path, _ = window_has_content(sub[0], "menu_submenu")
            full, _ = screenshot("menu_submenu_screen")
            print(f"  子菜单截图：{path}；全屏：{full}", flush=True)
            # f) 点子菜单第 0 行「英文」→ 切到英文：字母交还宿主。
            r0 = await probe_row(sub)
            if r0 is not None:
                sh("xdotool", "mousemove", str(sub[1] + sub[3] // 3), str(r0), "click", "1")
            gone = await wait_menus_gone()
            eaten = await c.type("abc")
            check("菜单：点子菜单「英文」→ 菜单收起、切到英文（字母直通）",
                  gone and eaten == [False] * 3 and c.take() == "",
                  f"菜单消失={gone} eaten={eaten}")
            # 不经按键的切换：服务端经 push 通道推 CMD_STATE_PUSH，托盘图标要跟上。
            en = await wait_until(lambda: watch.icon() == ICON_EN, 2)
            check("托盘图标：菜单点「英文」→ 「英」（push 通道的状态推送）", en,
                  f"im={watch.im()!r}")
    await c.key("Escape")
    c.take()
    # 空闲时没有主菜单入口了（状态区入口改成了「清风输入法设置」），主菜单都从组字时的
    # 「更多…」进——英文态组不了字，先用 Shift 切回中文（顺带验按键这条来源的托盘图标）。
    await c.tap_shift()
    zh = await wait_until(lambda: watch.icon() == ICON_PIN, 2)
    check("托盘图标：Shift 切回中文 → 「拼」（全拼）", zh, f"im={watch.im()!r}")

    # g) 主菜单的子菜单点选：输入方案 ▸ 全拼（英文、分隔线之后那一行）→ 仍是中文全拼。
    #    关掉菜单后组字还挂着，先收掉再验打字。
    main = await open_main_menu(c)
    ok = main is not None
    check("菜单：组字中经「更多…」再开主菜单", ok, f"menu={main}")
    if ok:
        await probe_row(main)
        sub = await wait_window("wind-menu-1", True)
        r0 = await probe_row(sub) if sub and sub[5] else None
        if r0 is not None:
            # 「英文」之后隔一条分隔线（7px）才是「全拼」。
            sh("xdotool", "mousemove", str(sub[1] + sub[3] // 3), str(r0 + MENU_ROW + 7),
               "click", "1")
        gone = await wait_menus_gone()
        check("菜单：点子菜单「全拼」后菜单收起", gone, f"sub={sub}")
    await end_composition(c)
    await typing_works(c, "子菜单点选「全拼」")

    # h) 键盘驱动主菜单：↓↓ 到「全角」、回车 → 全角生效（空格上屏全角空格）。
    await open_main_menu(c)
    await c.key("Down")
    await c.key("Down")
    await c.key("Return")
    gone = await wait_menus_gone()
    await end_composition(c)
    await c.key(" ")
    got = c.take()
    check("菜单：键盘 ↓↓回车 选「全角」→ 菜单收起、空格上屏全角空格",
          gone and got == "　", f"菜单消失={gone} 上屏={got!r}")
    # 复原：Shift+空格（全角开关热键）。
    await c.ic.call_process_key_event(0x20, 65, STATE_SHIFT, False, 0)
    await c.ic.call_process_key_event(0x20, 65, STATE_SHIFT, True, 0)
    await asyncio.sleep(0.1)
    await typing_works(c, "键盘点选")

    # i) 键盘 → 展开子菜单、← 收回。
    await open_main_menu(c)
    await c.key("Down")
    await c.key("Right")
    sub = await wait_window("wind-menu-1", True)
    await c.key("Left")
    back = await wait_window("wind-menu-1", False)
    check("菜单：→ 展开子菜单、← 收回", sub is not None and sub[5] and back is not None
          and not back[5], f"→ {sub} ← {back}")

    # ── 关闭路径（每条后立刻打字）──
    # 1) Esc
    await c.key("Escape")
    check("菜单关闭：Esc", await wait_menus_gone(), "菜单还在")
    await end_composition(c)
    await typing_works(c, "Esc 关闭")

    # 2) 点菜单外
    await open_main_menu(c)
    sh("xdotool", "mousemove", "1200", "60", "click", "1")
    check("菜单关闭：点菜单外", await wait_menus_gone(), "菜单还在")
    await end_composition(c)
    await typing_works(c, "点菜单外关闭")

    # 3) 右键（任何位置都只关菜单，同 Windows）
    main = await open_main_menu(c)
    if main:
        sh("xdotool", "mousemove", str(main[1] + 20), str(main[2] + 20), "click", "3")
    check("菜单关闭：右键", await wait_menus_gone(), "菜单还在")
    await end_composition(c)
    await typing_works(c, "右键关闭")

    # 4) 失焦
    await open_main_menu(c)
    await asyncio.sleep(0.3)  # 过服务端 250ms 的「刚打开」焦点守卫之外，addon 那条是无条件的
    await c.ic.call_focus_out()
    check("菜单关闭：失焦", await wait_menus_gone(), "菜单还在")
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await end_composition(c)
    await typing_works(c, "失焦关闭")

    # 5) 切换输入上下文（焦点换到另一个文本框）
    await open_main_menu(c)
    d = await new_ctx(bus, im, "e2e-menu-other")
    check("菜单关闭：焦点切到另一个文本框", await wait_menus_gone(), "菜单还在")
    await typing_works(d, "换输入上下文")
    await d.ic.call_focus_out()
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await end_composition(c)

    # 6) 候选被清空 / 组合结束（宿主 Reset：点了别处、挪了光标）→ 候选菜单随之关
    menu, _ = await open_candidate_menu(c)
    ok = menu is not None and menu[5]
    await c.ic.call_reset()
    gone = await wait_menus_gone()
    check("菜单关闭：组合被宿主终止（Reset）时候选菜单一并关", ok and gone,
          f"开={ok} 关={gone}")
    c.preedits.clear()
    await typing_works(c, "组合终止")

    # 7) 空闲超时（e2e 把 addon 的超时调到 WIND_MENU_IDLE_TIMEOUT_MS）
    idle_ms = int(os.environ.get("WIND_MENU_IDLE_TIMEOUT_MS", "0") or 0)
    if idle_ms:
        await open_main_menu(c)
        gone = await wait_until(menus_gone, idle_ms / 1000 + 3)
        check(f"菜单关闭：空闲 {idle_ms}ms 自动收起", gone, "菜单还在")
        await end_composition(c)
        await typing_works(c, "空闲超时")

    sh("xdotool", "mousemove", "5", "5")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)
    return c


async def menu_restart_cases(bus, im, restart):
    """服务一侧没了（被杀 / 重启）：菜单是它画的，addon 必须自己收掉、放开指针；新服务起来后
    照常打字。仅在 e2e 自己管服务时跑（自动拉起模式下服务不归脚本管）。"""
    c = await new_ctx(bus, im, "e2e-menu-restart")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)
    w = os.environ["W"]
    # a) 服务被杀：push 断线 → addon 收菜单。
    menu = await open_main_menu(c, tries=5)
    opened = menu is not None
    pid = open(os.path.join(w, "svc.pid")).read().strip()
    kr = subprocess.run(["kill", "-9", pid], capture_output=True, text=True)
    print(f"  菜单={menu} kill -9 {pid} rc={kr.returncode} {kr.stderr.strip()}", flush=True)
    gone = await wait_menus_gone(5)
    check("菜单关闭：服务进程被杀（push 断线）", opened and gone, f"开={opened} 关={gone}")
    rc = await asyncio.to_thread(subprocess.run, [restart])
    await c.ic.call_focus_in()
    ready = await wait_ready(c)
    check("菜单：服务被杀后重启，按键恢复", rc.returncode == 0 and ready, f"rc={rc.returncode}")
    await typing_works(c, "服务被杀并重启")

    # b) 服务正常重启（SERVICE_READY）：菜单开着时重启。
    menu = await open_main_menu(c, tries=5)
    opened = menu is not None
    rc = await asyncio.to_thread(subprocess.run, [restart])
    gone = await wait_menus_gone(5)
    check("菜单关闭：服务重启（SERVICE_READY）", opened and gone, f"开={opened} 关={gone}")
    await c.ic.call_focus_in()
    await wait_ready(c)
    await typing_works(c, "服务重启")
    return c

# ── 浮层交互（X11）：状态气泡 / 悬停提示 / Toast 的鼠标交互与右键菜单 ─────────────────
# 规则对位 Windows（wind_linux/include/OverlayInput.h）：悬停 / 拖动 / 菜单开着时不自动消失；
# 提示在指针移进它时保持、右键弹提示菜单；气泡可拖（松手报 pos.status_tip，落不落盘看定位
# 方式）、右键弹气泡菜单；Toast 点一下就关。e2e.sh 把气泡时长调成 2 秒、放了假 xclip。

def read_text(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        return ""


def user_config():
    return read_text(os.path.join(os.environ["XDG_CONFIG_HOME"], "WindInput", "config.toml"))


def config_int(text, key):
    import re
    m = re.search(rf"^{key}\s*=\s*(-?\d+)", text, re.M)
    return int(m.group(1)) if m else None


def config_str(text, key):
    import re
    m = re.search(rf'^{key}\s*=\s*"([^"]*)"', text, re.M)
    return m.group(1) if m else None


def fcitx_log_lines():
    return read_lines(os.path.join(os.environ["W"], "fcitx5.log"))


def drag_reports():
    """addon 上报 pos.status_tip 的次数（WIND_DEBUG「状态气泡拖动松手」）。"""
    return sum("状态气泡拖动松手" in l for l in fcitx_log_lines())


def clipboard_log():
    return read_text(os.path.join(os.environ["W"], "clipboard.log"))


def center(win):
    return win[1] + win[3] // 2, win[2] + win[4] // 2


async def pop_status(c):
    """中英各切一次（输入状态不变），等状态气泡出现。"""
    await c.tap_shift()
    await c.tap_shift()
    return await wait_window("wind-status", True)


async def drag(x0, y0, dx, dy, steps=6):
    """左键按在 (x0, y0) 拖 (dx, dy) 后松开（XTEST，分步移动让 addon 收到连续的移动事件）。"""
    sh("xdotool", "mousemove", str(x0), str(y0))
    await asyncio.sleep(0.05)
    sh("xdotool", "mousedown", "1")
    for i in range(1, steps + 1):
        sh("xdotool", "mousemove", str(x0 + dx * i // steps), str(y0 + dy * i // steps))
        await asyncio.sleep(0.03)
    sh("xdotool", "mouseup", "1")
    await asyncio.sleep(0.3)


async def open_tooltip_menu(c):
    """组字 → 悬停候选出提示 → 指针挪进提示 → 右键。返回 (菜单窗口, 提示窗口)。"""
    await c.type("nihao")
    await asyncio.sleep(0.4)
    if not await hover_candidate_point(c):
        return None, None
    tip = candidate_window("wind-tooltip")
    if not (tip and tip[5]):
        return None, tip
    tx, ty = center(tip)
    sh("xdotool", "mousemove", str(tx), str(ty))
    await asyncio.sleep(0.4)
    sh("xdotool", "click", "3")
    return await wait_window("wind-menu-0", True), tip


async def overlay_interaction_cases(bus, im):
    c = await new_ctx(bus, im, "e2e-overlay-ui")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)
    sh("xdotool", "mousemove", "5", "5")

    # ── 状态气泡 ──
    # a) 悬停保持：指针停在气泡上，过了时长（2 秒）仍在；离开后按时长消失。
    st = await pop_status(c)
    ok = st is not None and st[5]
    check("气泡交互：Shift 切换后状态气泡出现", ok, f"window={st}")
    if ok:
        cx, cy = center(st)
        sh("xdotool", "mousemove", str(cx), str(cy))
        await asyncio.sleep(3.0)
        held = candidate_window("wind-status")
        check("气泡交互：指针悬停其上时超过时长也不消失", held is not None and held[5],
              f"window={held}")
        # b) 拖动（出厂跟随光标）：窗口跟手平移；松手上报 pos.status_tip；这一模式不落盘。
        before = drag_reports()
        await drag(cx, cy, 120, 60)
        moved = candidate_window("wind-status")
        ok = moved is not None and moved[5] and (moved[1] - st[1], moved[2] - st[2]) == (120, 60)
        check("气泡交互：左键拖动跟手（窗口平移 = 指针位移）", ok, f"拖前={st} 拖后={moved}")
        check("气泡交互：松手上报 pos.status_tip", drag_reports() == before + 1,
              f"上报 {drag_reports() - before} 次")
        await asyncio.sleep(0.3)
        cfg = user_config()
        check("气泡交互：跟随光标模式下拖动不落盘", config_int(cfg, "custom_x") is None,
              f"config.toml 里出现了 custom_x：{cfg!r}")
        full, _ = screenshot("status_dragged_screen")
        print(f"  拖动后全屏截图：{full}", flush=True)
        # c) 离开后照常消失（悬停结束重新给满一份时长）。
        sh("xdotool", "mousemove", "5", "5")
        gone = await wait_window("wind-status", False, timeout=5.0)
        check("气泡交互：指针离开后按时长自动消失", gone is not None and not gone[5],
              f"window={gone}")

    # d) 右键气泡 → 气泡菜单（常驻显示 / 焦点切换时显示 / 固定位置 / 恢复默认位置 / 截图）；
    #    菜单开着时气泡不消失；点「固定位置」→ 以气泡当前位置落盘（经 pos.status_tip.query）。
    st = await pop_status(c)
    menu = None
    if st and st[5]:
        cx, cy = center(st)
        sh("xdotool", "mousemove", str(cx), str(cy), "click", "3")
        menu = await wait_window("wind-menu-0", True)
    ok = menu is not None and menu[5]
    check("气泡菜单：右键状态气泡弹出菜单", ok, f"menu={menu} 气泡={st}")
    if ok:
        path, n = window_has_content(menu[0], "menu_status")
        check("气泡菜单：菜单有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  气泡菜单截图：{path}", flush=True)
        full, _ = screenshot("menu_status_screen")
        print(f"  全屏截图：{full}", flush=True)
        await asyncio.sleep(2.6)
        still = candidate_window("wind-status")
        check("气泡菜单：菜单开着时气泡超过时长也不消失", still is not None and still[5],
              f"window={still}")
        row0 = await probe_row(menu)
        if row0 is not None:
            sh("xdotool", "mousemove", str(menu[1] + menu[3] // 3), str(row0 + 2 * MENU_ROW),
               "click", "1")
        gone = await wait_menus_gone()
        pinned = await wait_until(
            lambda: config_str(user_config(), "position_mode") == "fixed"
            and config_int(user_config(), "custom_x") is not None, 3)
        cfg = user_config()
        fx, fy = config_int(cfg, "custom_x"), config_int(cfg, "custom_y")
        # 落盘的是内容左上，窗口左上再往左上偏一个软阴影扩边（0～30 像素）。
        near = (fx is not None and fy is not None and 0 <= fx - still[1] <= 30
                and 0 <= fy - still[2] <= 30)
        check("气泡菜单：点「固定位置」→ 以气泡当前位置落盘", gone and pinned and near,
              f"菜单消失={gone} custom=({fx},{fy}) 气泡窗口=({still[1]},{still[2]})")

        # e) 固定位置模式下拖动：落点写回 custom_x/y（位移与拖动一致）。
        st = await pop_status(c)
        ok = st is not None and st[5]
        if ok:
            check("气泡交互：固定位置模式下气泡出现在落盘位置",
                  0 <= fx - st[1] <= 30 and 0 <= fy - st[2] <= 30, f"气泡={st} custom=({fx},{fy})")
            cx, cy = center(st)
            await drag(cx, cy, -100, -50)
            ok = await wait_until(lambda: (config_int(user_config(), "custom_x"),
                                           config_int(user_config(), "custom_y"))
                                  == (fx - 100, fy - 50), 3)
            cfg = user_config()
            check("气泡交互：固定位置模式下拖动落盘（custom_x/y 随落点更新）", ok,
                  f"拖前 ({fx},{fy}) 拖后 ({config_int(cfg, 'custom_x')},{config_int(cfg, 'custom_y')})")

        # f) 恢复默认位置：回到跟随光标。
        st = await pop_status(c)
        if st and st[5]:
            cx, cy = center(st)
            sh("xdotool", "mousemove", str(cx), str(cy), "click", "3")
            menu = await wait_window("wind-menu-0", True)
            row0 = await probe_row(menu) if menu and menu[5] else None
            if row0 is not None:
                sh("xdotool", "mousemove", str(menu[1] + menu[3] // 3),
                   str(row0 + 3 * MENU_ROW), "click", "1")
        await wait_menus_gone()
        # 用户配置只写非默认值：回到出厂的跟随光标后，这几个键从 config.toml 里消失（或写成默认值）。
        back = await wait_until(
            lambda: config_str(user_config(), "position_mode") in (None, "follow_caret")
            and config_int(user_config(), "custom_x") in (None, 0), 3)
        cfg = user_config()
        check("气泡菜单：点「恢复默认位置」→ 回到跟随光标", back,
              f"position_mode={config_str(cfg, 'position_mode')} custom_x={config_int(cfg, 'custom_x')}")
    sh("xdotool", "mousemove", "5", "5")
    await wait_window("wind-status", False, timeout=5.0)
    await typing_works(c, "气泡菜单用完")

    # ── 悬停提示 ──
    # g) 指针从候选挪进提示：提示保持（Windows：光标进入气泡即撤掉候选悬停的变化）。
    await c.type("nihao")
    await asyncio.sleep(0.4)
    pt = await hover_candidate_point(c)
    tip = candidate_window("wind-tooltip")
    kept = None
    if pt and tip and tip[5]:
        tx, ty = center(tip)
        sh("xdotool", "mousemove", str(tx), str(ty))
        await asyncio.sleep(1.2)
        kept = candidate_window("wind-tooltip")
    check("提示交互：指针从候选挪进提示后提示保持显示", kept is not None and kept[5],
          f"提示={tip} 1.2 秒后={kept}")
    # h) 离开提示到别处：宽限后收起。
    sh("xdotool", "mousemove", "5", "5")
    gone = await wait_window("wind-tooltip", False, timeout=2.0)
    check("提示交互：指针离开提示后提示收起", gone is not None and not gone[5], f"提示={gone}")
    await c.key("Escape")
    c.take()

    # i) 右键提示 → 提示菜单（复制「段」/ 上屏「段」/ … / 复制全部 / 截图此窗口）；菜单开着时
    #    提示不消失；点第一项（复制类）→ 剪贴板里是提示内容，并弹「提示内容已复制」。
    menu, tip = await open_tooltip_menu(c)
    ok = menu is not None and menu[5]
    check("提示菜单：右键悬停提示弹出菜单", ok, f"menu={menu} 提示={tip}")
    if ok:
        path, n = window_has_content(menu[0], "menu_tooltip")
        check("提示菜单：菜单有绘制", n > 1, f"{n} 种颜色，截图 {path}")
        print(f"  提示菜单截图：{path}", flush=True)
        full, _ = screenshot("menu_tooltip_screen")
        print(f"  全屏截图：{full}", flush=True)
        await asyncio.sleep(0.6)
        t2 = candidate_window("wind-tooltip")
        check("提示菜单：菜单开着时提示不消失", t2 is not None and t2[5], f"提示={t2}")
        clip0 = len(clipboard_log())
        row0 = await probe_row(menu)
        if row0 is not None:
            sh("xdotool", "mousemove", str(menu[1] + menu[3] // 3), str(row0), "click", "1")
        gone = await wait_menus_gone()
        copied = await wait_until(lambda: len(clipboard_log()) > clip0, 3)
        got = clipboard_log()[clip0:]
        text = "\n".join(l for l in got.splitlines() if not l.startswith("== ")).strip()
        check("提示菜单：点「复制…」→ 剪贴板里是提示内容", gone and copied and text != "",
              f"菜单消失={gone} 剪贴板新增={got!r}")
        print(f"  复制到剪贴板：{text!r}", flush=True)
        toast = await wait_window("wind-toast", True, timeout=2.0)
        check("提示菜单：复制后弹 Toast 反馈", toast is not None and toast[5], f"toast={toast}")
        if toast and toast[5]:
            path, _ = window_has_content(toast[0], "toast_tooltip_copied")
            print(f"  复制反馈 Toast 截图：{path}", flush=True)
    await c.key("Escape")
    c.take()
    sh("xdotool", "mousemove", "5", "5")
    await typing_works(c, "提示菜单用完")

    # ── Toast ──
    svc = os.path.join(os.environ.get("W", ""), "svc", "wind_input")

    async def toast(text, ms):
        await asyncio.to_thread(subprocess.run, [svc, "ui", "toast", text, "--pos", "top_right",
                                                 "--ms", str(ms)], capture_output=True)
        return await wait_window("wind-toast", True)

    await wait_window("wind-toast", False, timeout=5.0)
    # j) 悬停暂停：指针停在 Toast 上时过了时长仍在；离开后照常消失。
    t = await toast("悬停不消失", 1500)
    if t and t[5]:
        tx, ty = center(t)
        sh("xdotool", "mousemove", str(tx), str(ty))
        await asyncio.sleep(2.5)
        held = candidate_window("wind-toast")
        check("Toast：指针悬停其上时超过时长也不消失", held is not None and held[5], f"toast={held}")
        sh("xdotool", "mousemove", "5", "5")
        gone = await wait_window("wind-toast", False, timeout=4.0)
        check("Toast：指针离开后照常消失", gone is not None and not gone[5], f"toast={gone}")
    else:
        check("Toast：ui toast 后出现", False, f"toast={t}")
    # k) 点击提前关闭。
    t = await toast("点我关闭", 10000)
    if t and t[5]:
        tx, ty = center(t)
        sh("xdotool", "mousemove", str(tx), str(ty), "click", "1")
        gone = await wait_window("wind-toast", False, timeout=1.0)
        check("Toast：点击后立即关闭（不等 10 秒）", gone is not None and not gone[5],
              f"toast={gone}")
    else:
        check("Toast：ui toast 后出现", False, f"toast={t}")
    sh("xdotool", "mousemove", "5", "5")

    # ── 复位路径：浮层交互进行中被打断，之后立刻能正常打字 ──
    # 1) 提示菜单开着 → Esc
    menu, _ = await open_tooltip_menu(c)
    ok = menu is not None and menu[5]
    await c.key("Escape")
    check("提示菜单关闭：Esc", ok and await wait_menus_gone(), f"开={ok}")
    await c.key("Escape")
    c.take()
    await typing_works(c, "提示菜单 Esc 关闭")
    # 2) 提示菜单开着 → 组合被宿主终止（Reset）：提示依附于候选，菜单随之收
    menu, _ = await open_tooltip_menu(c)
    ok = menu is not None and menu[5]
    await c.ic.call_reset()
    check("提示菜单关闭：组合被宿主终止时一并收起", ok and await wait_menus_gone(), f"开={ok}")
    c.preedits.clear()
    await typing_works(c, "提示菜单随组合终止")
    # 3) 提示菜单开着 → 失焦
    menu, _ = await open_tooltip_menu(c)
    ok = menu is not None and menu[5]
    await asyncio.sleep(0.3)
    await c.ic.call_focus_out()
    check("提示菜单关闭：失焦", ok and await wait_menus_gone(), f"开={ok}")
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await typing_works(c, "提示菜单失焦关闭")
    # 4) 气泡菜单开着 → 失焦：菜单收起，气泡的「菜单开着」保持随之解除（随后按时长消失）
    sh("xdotool", "mousemove", "5", "5")
    st = await pop_status(c)
    menu = None
    if st and st[5]:
        cx, cy = center(st)
        sh("xdotool", "mousemove", str(cx), str(cy), "click", "3")
        menu = await wait_window("wind-menu-0", True)
    ok = menu is not None and menu[5]
    await asyncio.sleep(0.3)
    await c.ic.call_focus_out()
    closed = await wait_menus_gone()
    sh("xdotool", "mousemove", "5", "5")
    gone = await wait_window("wind-status", False, timeout=5.0)
    check("气泡菜单关闭：失焦（菜单收起、气泡随后按时长消失）",
          ok and closed and gone is not None and not gone[5], f"开={ok} 菜单关={closed} 气泡={gone}")
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await typing_works(c, "气泡菜单失焦关闭")
    # 5) 拖动中失焦：服务端失焦即收状态提示（同 Windows），气泡被摘 → 拖动随之作废、不上报
    #    （用户没松手，谈不上摆到了哪）；松手不卡，之后照常打字、还能再拖。
    st = await pop_status(c)
    if st and st[5]:
        cx, cy = center(st)
        before = drag_reports()
        sh("xdotool", "mousemove", str(cx), str(cy))
        await asyncio.sleep(0.05)
        sh("xdotool", "mousedown", "1")
        sh("xdotool", "mousemove", str(cx + 40), str(cy + 20))
        await c.ic.call_focus_out()
        gone = await wait_window("wind-status", False, timeout=2.0)
        sh("xdotool", "mousemove", str(cx + 80), str(cy + 40))
        sh("xdotool", "mouseup", "1")
        await asyncio.sleep(0.3)
        check("气泡拖动中失焦：气泡随失焦收起、拖动作废不上报",
              gone is not None and not gone[5] and drag_reports() == before,
              f"气泡={gone} 上报 {drag_reports() - before} 次")
        await c.ic.call_focus_in()
        await asyncio.sleep(0.2)
        st = await pop_status(c)
        if st and st[5]:
            before = drag_reports()
            await drag(*center(st), 30, 10)
            check("气泡拖动中失焦：之后还能再拖（上报一次）", drag_reports() == before + 1,
                  f"上报 {drag_reports() - before} 次")
    else:
        check("气泡拖动中失焦：气泡出现", False, f"window={st}")
    sh("xdotool", "mousemove", "5", "5")
    await typing_works(c, "拖动中失焦")
    await wait_window("wind-status", False, timeout=5.0)
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


async def wait_push(c, timeout=10.0):
    """服务（重）启后请求通道先通（按键被吃），push 通道每秒重连一次、要晚一点——在那之前推下来
    的帧（候选窗、浮层）全丢。打一个字等候选窗出现、Esc 等它消失：两步都靠 push，才算通了
    （服务被杀时候选窗可能还挂着旧帧，只看「出现」会被它骗过）。"""
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        await c.key("a")
        shown = await wait_window("wind-candidate", True, timeout=1.0)
        await c.key("Escape")
        hidden = await wait_window("wind-candidate", False, timeout=1.0)
        c.take()
        c.preedits.clear()
        if shown and shown[5] and hidden is not None and not hidden[5]:
            return True
        if asyncio.get_running_loop().time() >= deadline:
            return False


async def overlay_restart_cases(bus, im, restart):
    """浮层交互进行中服务没了（被杀 / 重启）：拖动态、菜单、气泡的保持都得归位，之后照常打字、
    还能再拖。仅在 e2e 自己管服务时跑。"""
    c = await new_ctx(bus, im, "e2e-overlay-restart")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)
    sh("xdotool", "mousemove", "5", "5")
    w = os.environ["W"]
    # 上一组用例刚重启过服务：先等请求与 push 两条通道都通。
    await wait_ready(c)
    await wait_push(c)

    # a) 拖动中服务重启（SERVICE_READY → addon 收掉全部浮层）：拖动态随窗口归位、这次不上报。
    st = await pop_status(c)
    ok = st is not None and st[5]
    before = drag_reports()
    if ok:
        cx, cy = center(st)
        sh("xdotool", "mousemove", str(cx), str(cy))
        sh("xdotool", "mousedown", "1")
        sh("xdotool", "mousemove", str(cx + 30), str(cy + 15))
        rc = await asyncio.to_thread(subprocess.run, [restart])
        gone = await wait_window("wind-status", False, timeout=5.0)
        sh("xdotool", "mousemove", str(cx + 60), str(cy + 30))
        sh("xdotool", "mouseup", "1")
        await asyncio.sleep(0.3)
        check("气泡拖动中服务重启：气泡收起、拖动作废不上报",
              rc.returncode == 0 and gone is not None and not gone[5] and drag_reports() == before,
              f"rc={rc.returncode} 气泡={gone} 上报 {drag_reports() - before} 次")
    else:
        check("气泡拖动中服务重启：气泡出现", False, f"window={st}")
    await c.ic.call_focus_in()
    await wait_ready(c)
    await wait_push(c)
    await typing_works(c, "拖动中服务重启")
    st = await pop_status(c)
    if st and st[5]:
        cx, cy = center(st)
        before = drag_reports()
        await drag(cx, cy, 50, 20)
        moved = candidate_window("wind-status")
        check("气泡拖动中服务重启：之后还能再拖（上报一次、跟手）",
              drag_reports() == before + 1 and moved and (moved[1] - st[1], moved[2] - st[2]) == (50, 20),
              f"上报 {drag_reports() - before} 次 拖前={st} 拖后={moved}")
    sh("xdotool", "mousemove", "5", "5")

    # b) 提示菜单开着时服务重启：菜单收起，之后照常打字。
    menu, _ = await open_tooltip_menu(c)
    opened = menu is not None and menu[5]
    rc = await asyncio.to_thread(subprocess.run, [restart])
    gone = await wait_menus_gone(5)
    check("提示菜单关闭：服务重启", opened and gone, f"开={opened} 关={gone}")
    await c.ic.call_focus_in()
    await wait_ready(c)
    await wait_push(c)
    await typing_works(c, "提示菜单开着时服务重启")

    # c) 拖动中服务被杀：push 断线，没人再推隐藏帧——松手不卡，气泡随后按时长自己消失。
    st = await pop_status(c)
    if st and st[5]:
        cx, cy = center(st)
        sh("xdotool", "mousemove", str(cx), str(cy))
        sh("xdotool", "mousedown", "1")
        sh("xdotool", "mousemove", str(cx + 30), str(cy + 15))
        pid = open(os.path.join(w, "svc.pid")).read().strip()
        subprocess.run(["kill", "-9", pid], capture_output=True)
        await asyncio.sleep(0.5)
        sh("xdotool", "mousemove", str(cx + 60), str(cy + 30))
        sh("xdotool", "mouseup", "1")
        sh("xdotool", "mousemove", "5", "5")
        gone = await wait_window("wind-status", False, timeout=5.0)
        check("气泡拖动中服务被杀：松手不卡、气泡按时长消失", gone is not None and not gone[5],
              f"气泡={gone}")
    else:
        check("气泡拖动中服务被杀：气泡出现", False, f"window={st}")
    rc = await asyncio.to_thread(subprocess.run, [restart])
    await c.ic.call_focus_in()
    ready = await wait_ready(c)
    check("气泡拖动中服务被杀：重启后按键恢复", rc.returncode == 0 and ready, f"rc={rc.returncode}")
    await typing_works(c, "拖动中服务被杀并重启")
    return c


def settings_windows():
    """设置程序的顶层窗口（WM_CLASS 实例名 = 可执行文件名 wind_setting.bin，见 windui
    `x11.rs::wm_class`；e2e 里经包装脚本启动，真程序是那个软链）。"""
    ids = sh("xdotool", "search", "--classname", "wind_setting").split()
    return [i for i in ids if "IsViewable" in sh("xwininfo", "-id", i)]


def read_lines(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read().splitlines()
    except FileNotFoundError:
        return []


async def wait_until(pred, timeout):
    deadline = asyncio.get_running_loop().time() + timeout
    while not pred():
        if asyncio.get_running_loop().time() >= deadline:
            return False
        await asyncio.sleep(0.1)
    return True


async def settings_cases(bus, im):
    """「从输入法打开设置」整条链：Ctrl+Shift+]（keys.open_settings 出厂值）→ addon 按键转发
    → 服务热键分派 open_settings → 下行扩展信封 settings.open → addon 拉起 WIND_INPUT_SETTING
    （e2e.sh 的包装脚本记下 argv 再换成真设置程序）→ 设置程序经控制 socket 连上服务。"""
    w = os.environ["W"]
    argv_log = os.path.join(w, "setting.argv")
    setting_log = os.path.join(os.environ["XDG_DATA_HOME"], "WindInput", "logs",
                               "wind_setting.1.log")
    c = await new_ctx(bus, im, "e2e-settings")

    async def open_settings_hotkey():
        # Shift 按住时 X 客户端送的是 braceright（0x7d），keycode 35 = ]。
        down = await c.ic.call_process_key_event(0x7D, 35, STATE_CTRL | STATE_SHIFT, False, 0)
        await c.ic.call_process_key_event(0x7D, 35, STATE_CTRL | STATE_SHIFT, True, 0)
        return down

    eaten = await open_settings_hotkey()
    check("设置：Ctrl+Shift+] 被输入法吃掉（热键命中 open_settings）", eaten is True,
          f"eaten={eaten}")
    launched = await wait_until(lambda: len(read_lines(argv_log)) >= 1, 10)
    check("设置：addon 收到 settings.open 后拉起设置程序", launched,
          f"{argv_log} 无记录（fcitx5 日志里找「启动设置程序」/「settings.open」）")
    args = read_lines(argv_log)
    check("设置：打开设置不带深链参数（默认页）", args[:1] == [""], f"argv={args}")
    mapped = await wait_until(lambda: len(settings_windows()) == 1, 20)
    wins = settings_windows()
    check("设置：设置窗口已映射", mapped, f"windows={wins}")
    if wins:
        # 映射早于首帧（首帧要等方案列表等数据回来）：轮询到有内容为止。
        shot = {}

        def painted():
            shot["path"], shot["n"] = window_has_content(wins[0], "settings_main")
            return shot["n"] > 8
        await wait_until(painted, 10)
        check("设置：窗口有渲染内容", shot["n"] > 8, f"颜色数={shot['n']} 截图={shot['path']}")
        print(f"      截图 {shot['path']}", flush=True)
    connected = await wait_until(
        lambda: any("apply_loaded 完成, connected=true" in l for l in read_lines(setting_log)), 15)
    check("设置：设置程序经控制 socket 连上服务", connected,
          f"{setting_log} 里没有 connected=true")

    # 已开着时再按一次：新进程只把 argv 转给首实例（windui 单实例），窗口不重复。
    await open_settings_hotkey()
    again = await wait_until(lambda: len(read_lines(argv_log)) >= 2, 10)
    forwarded = await wait_until(
        lambda: any("二次实例 argv" in l for l in read_lines(setting_log)), 10)
    await asyncio.sleep(0.5)
    check("设置：再按一次转交给已开的设置程序、不开第二个窗口",
          again and forwarded and len(settings_windows()) == 1,
          f"再次启动={again} 转交={forwarded} windows={settings_windows()}")

    subprocess.run(["pkill", "-f", "--", "^" + os.path.join(w, "svc", "wind_setting")])
    closed = await wait_until(lambda: not settings_windows(), 5)
    check("设置：设置程序退出后窗口消失", closed, f"windows={settings_windows()}")

    async def launched_again(label, before):
        """从 before 行起又拉起一次设置程序：argv 记录多一行（无深链参数）、窗口出现；随后关掉。"""
        ok = await wait_until(lambda: len(read_lines(argv_log)) > before, 10)
        args = read_lines(argv_log)[before:]
        mapped = await wait_until(lambda: len(settings_windows()) == 1, 20)
        check(f"设置：{label} → 拉起设置程序（默认页）", ok and args[:1] == [""] and mapped,
              f"拉起={ok} argv={args} 窗口={settings_windows()}")
        subprocess.run(["pkill", "-f", "--", "^" + os.path.join(w, "svc", "wind_setting")])
        await wait_until(lambda: not settings_windows(), 5)

    # 空闲入口：Fcitx5 状态区动作「清风输入法设置」（托盘菜单 / kimpanel 面板里那一项）。
    panel = await impanel(bus)
    watch = await kimpanel_watch(bus)
    await c.ic.call_focus_out()
    await c.ic.call_focus_in()  # 聚焦时 kimpanel 重报一遍状态区动作
    await asyncio.sleep(0.3)
    regs = [p for ps in watch.registered for p in ps]
    listed = any(p.startswith("/Fcitx/windinput-settings:清风输入法设置:") for p in regs)
    check("设置：状态区列出「清风输入法设置」、不再有「清风输入法菜单」",
          listed and not any("windinput-menu" in p for p in regs), f"属性={regs[-6:]}")
    before = len(read_lines(argv_log))
    panel.trigger("/Fcitx/windinput-settings")
    await launched_again("状态区点「清风输入法设置」", before)

    # 系统输入法配置里点清风的「配置」：配置工具经 Controller1.GetConfig 取描述。整页只有一个
    # External 项时 fcitx5-configtool（5.1.6+）直接启动它，而不是弹一张空白配置页。
    intro = await bus.introspect(SERVICE, "/controller")
    ctl = bus.get_proxy_object(SERVICE, "/controller", intro).get_interface(
        "org.fcitx.Fcitx.Controller1")
    external = ""
    for uri in ("fcitx://config/inputmethod/windinput", "fcitx://config/addon/windinput"):
        try:
            _, desc = await ctl.call_get_config(uri)
        except Exception as e:  # noqa: BLE001 — 失败原因原样进断言信息
            desc, err = [], repr(e)
        else:
            err = ""
        opts = desc[0][1] if len(desc) == 1 else []
        only = opts[0] if len(opts) == 1 else None
        external = only[4]["External"].value if only and "External" in only[4] else ""
        check(f"配置：{uri} 是唯一一项 External、指向设置程序",
              only is not None and only[1] == "External"
              and external == os.environ.get("WIND_INPUT_SETTING"),
              f"desc={desc} {err}")
    # 照配置工具的做法把 External 当命令启动（QProcess::startDetached），确认它真能拉起来。
    if external:
        before = len(read_lines(argv_log))
        subprocess.Popen([external], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         start_new_session=True)
        await launched_again("配置工具启动 External 命令", before)

    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


async def mode_icon_cases(bus, im):
    """托盘 / 面板图标随模式切换：引擎的 subModeIcon 经 kimpanel 模块报给面板。按键切换
    （Shift 单击 → STATUS_UPDATE 响应）与焦点进入（FocusGained → MODE_PUSH）两条来源；
    菜单切换（push 通道 STATE_PUSH）在 menu_cases 里验。对齐 Windows 语言栏：中文态按方案
    标签（e2e 用全拼，「拼」）、英文态「英」、大写锁定无论中英都是「A」。"""
    watch = await kimpanel_watch(bus)
    c = await new_ctx(bus, im, "e2e-mode-icon")
    zh = await wait_until(lambda: watch.icon() == ICON_PIN, 2)
    check("托盘图标：中文态按方案标签运行时渲染，全拼为「拼」、标签「拼」",
          zh and watch.im().endswith("label=拼"), f"im={watch.im()!r}")
    # 服务端每次构建状态都把中文 / 英文 / 大写三组一起备好（addon 本端判定大写时不必等帧）。
    for name in (ICON_PIN, ICON_EN, ICON_CAPS):
        files, why = tray_icon_files_ok(name)
        check(f"托盘图标：{name} 各尺寸 PNG 已写进用户图标目录", files, why)
    await c.tap_shift()
    en = await wait_until(lambda: watch.icon() == ICON_EN, 2)
    check("托盘图标：Shift 切英文 → 「英」、标签「英」",
          en and watch.im().endswith("label=英"), f"im={watch.im()!r}")
    # 换个文本框再回来：焦点进入时服务端回的 MODE_PUSH 仍是英文，图标不被焦点事件打回中文。
    d = await new_ctx(bus, im, "e2e-mode-icon-2")
    await d.ic.call_focus_out()
    await c.ic.call_focus_in()
    await asyncio.sleep(0.3)
    check("托盘图标：换焦点后仍是「英」", watch.icon() == ICON_EN,
          f"im={watch.im()!r}")

    # 大写锁定（英文态）：按下 + 松开 → 「A」；服务端同步镜像并回 STATUS_CAPS_LOCK。
    await c.tap_caps_lock(False)
    caps = await wait_until(lambda: watch.icon() == ICON_CAPS, 2)
    check("托盘图标：英文态按 CapsLock → 「A」图标、标签「A」",
          caps and watch.im().endswith("label=A"), f"im={watch.im()!r}")
    # 大写锁定下打字母：服务端认得大写（英文态本就直通，这里只验不被吞、图标不动）。
    eaten = await c.key("a", STATE_LOCK)
    check("大写锁定：英文态字母直通、图标仍是「A」",
          eaten is False and watch.icon() == ICON_CAPS, f"eaten={eaten} im={watch.im()!r}")
    # 换焦点：MODE_PUSH 不带大写锁定位，不能把「A」打回「英」。
    await c.ic.call_focus_out()
    await d.ic.call_focus_in()
    await asyncio.sleep(0.3)
    check("托盘图标：大写锁定时换焦点不回退", watch.icon() == ICON_CAPS,
          f"im={watch.im()!r}")
    await d.ic.call_focus_out()
    await c.ic.call_focus_in()
    await asyncio.sleep(0.3)
    # Shift 单击（带着 Lock 位）：切到中文，但大写锁定仍开着 → 仍是「A」。修的是一个真坑：
    # 修饰键单击那一帧曾发 toggles=0，服务端据此把大写锁定镜像校准成「关」。
    await c.tap_shift(STATE_LOCK)
    await asyncio.sleep(0.3)
    check("托盘图标：大写锁定时 Shift 切中文，仍是「A」（中文态也显示「A」）",
          watch.icon() == ICON_CAPS, f"im={watch.im()!r}")
    # 中文态 + 大写锁定：字母按大写直出，不进组字。
    eaten = await c.key("a", STATE_LOCK)
    await asyncio.sleep(0.2)
    check("大写锁定：中文态字母不进组字", c.preedit == "", f"eaten={eaten} preedit={c.preedit!r}")
    c.take()
    # 再按一次 CapsLock：回到当前模式的图标（中文 → 「拼」）。
    await c.tap_caps_lock(True)
    back = await wait_until(lambda: watch.icon() == ICON_PIN, 2)
    check("托盘图标：再按 CapsLock → 回到「拼」",
          back and watch.im().endswith("label=拼"), f"im={watch.im()!r}")
    typed = await c.type("ni")
    check("大写锁定关掉后中文组字恢复", typed == [True, True] and c.preedit != "",
          f"eaten={typed} preedit={c.preedit!r}")
    await c.key("Escape")
    c.take()
    # 在别处开了大写锁定（本引擎没收到 Caps_Lock）：第一个带 Lock 位的键就校准回来。
    await c.key("Left", STATE_LOCK)
    cal = await wait_until(lambda: watch.icon() == ICON_CAPS, 2)
    check("托盘图标：别处开的大写锁定，第一个带 Lock 位的键就校准成「A」", cal,
          f"im={watch.im()!r}")
    await c.key("Left")
    uncal = await wait_until(lambda: watch.icon() == ICON_PIN, 2)
    check("托盘图标：Lock 位消失即回到「拼」", uncal, f"im={watch.im()!r}")

    await c.tap_shift()
    en2 = await wait_until(lambda: watch.icon() == ICON_EN, 2)
    await c.tap_shift()
    back = await wait_until(lambda: watch.icon() == ICON_PIN, 2)
    check("托盘图标：再按 Shift 切英 / 切回 → 「英」/「拼」", en2 and back,
          f"im={watch.im()!r}")
    # 切方案（Ctrl+Shift+E，全拼 ⇄ 五笔）：中英不变、方案标签变，图标跟着换。
    for want, label in ((ICON_WU, "五"), (ICON_PIN, "拼")):
        await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, False, 0)
        await c.ic.call_process_key_event(0x45, 26, STATE_CTRL | STATE_SHIFT, True, 0)
        ok = await wait_until(lambda: watch.icon() == want, 2)
        files, why = tray_icon_files_ok(want)
        check(f"托盘图标：Ctrl+Shift+E 切方案 → 「{label}」的运行时图标（{want}，各尺寸 PNG 在盘上）",
              ok and files and watch.im().endswith(f"label={label}"),
              f"im={watch.im()!r} {why}")
    check("托盘图标：切方案前后图标名不同", ICON_WU != ICON_PIN)
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)

async def set_config(key, value):
    """经 `wind_input config set` 改在线服务的配置（RPC 热重载，不重启服务）。"""
    svc = os.path.join(os.environ["W"], "svc", "wind_input")
    r = await asyncio.to_thread(subprocess.run, [svc, "config", "set", key, value],
                                capture_output=True, text=True)
    await asyncio.sleep(0.3)
    return r


async def preedit_display_cases(bus, im):
    """编码显示在候选窗里（非嵌入，`preedit_display = candidate_top`）：服务端照旧给宿主一段单
    空格的占位组合（Windows 靠它取光标坐标），addon 不把它写进应用——应用里既看不到编码、也
    不会多出一格空白把光标推走；上屏后文本里没有多余空格。嵌入模式（出厂）预编辑照常。"""
    r = await set_config("ui.candidate.preedit_display", "candidate_top")
    check("非嵌入：config set preedit_display=candidate_top", r.returncode == 0,
          f"rc={r.returncode} {r.stderr.strip()[:200]}")
    c = await new_ctx(bus, im, "e2e-preedit")
    await c.ic.call_set_cursor_rect(300, 400, 2, 20)
    c.preedits.clear()
    eaten = await c.type("nihao")
    await asyncio.sleep(0.3)
    seen = list(c.preedits)
    check("非嵌入：组字时应用收到的预编辑恒为空（无占位空格）",
          all(eaten) and all(p == "" for p in seen), f"eaten={eaten} 预编辑={seen}")
    if os.environ.get("DISPLAY"):
        win = candidate_window()
        ok = win is not None and win[5]
        check("非嵌入：候选窗出现、左上角落在光标下方附近",
              ok and 240 <= win[1] <= 330 and 400 <= win[2] <= 470, f"window={win}")
        if ok:
            path, _ = screenshot("candidate_preedit_top", win[0])
            print(f"  非嵌入候选窗截图：{path}", flush=True)
    await c.key(" ")
    got = c.take()
    check("非嵌入：上屏「你好」、没有多余空格", got == "你好", f"上屏={got!r}")
    # 首键即上屏的路径（数字选词）同样不带占位。
    await c.type("shi")
    await c.key("1")
    got = c.take()
    check("非嵌入：数字选词上屏不带空格", got != "" and " " not in got, f"上屏={got!r}")

    r = await set_config("ui.candidate.preedit_display", "app_inline")
    c.preedits.clear()
    await c.type("nihao")
    shown = c.preedit
    await c.key(" ")
    got = c.take()
    check("嵌入（复原 app_inline）：预编辑是编码本身、上屏正常",
          r.returncode == 0 and shown.replace("'", "") == "nihao" and got == "你好",
          f"rc={r.returncode} 预编辑={shown!r} 上屏={got!r}")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)

# ── 命令直通车按键合成（CMD_KEY_TAP / SEQ / HOLD / RELEASE → forwardKey）────────────

XK_HOME, XK_END, XK_BACKSPACE = 0xFF50, 0xFF57, 0xFF08
XK_SHIFT_L, XK_CONTROL_L = 0xFFE1, 0xFFE3
DOWN, UP = False, True


async def run_phrase(c, code, want_events, timeout=3.0):
    """打短语码 + 空格选首选（e2e.sh 追加的 $CC 短语，weight 2000 排第一），等 addon 经
    forwardKey 交出 `want_events` 个事件（服务端推帧是异步的）。返回 (事件, 上屏)。
    选词之前清空 `c.preedits`：之后收到的预编辑都是选词及其后的。"""
    c.forwarded.clear()
    c.take()
    await c.type(code)
    c.preedits.clear()
    await c.key(" ")
    await wait_until(lambda: len(c.forwarded) >= want_events, timeout)
    await asyncio.sleep(0.2)  # 多出来的也收进来，好断言「恰好这些」
    return list(c.forwarded), c.take()


async def keysynth_cases(bus, im):
    """按键合成走 Fcitx5 `InputContext::forwardKey`：DBus 前端把它发成 ForwardKey(sym, state,
    isRelease) 信号，这里逐个断言按下 / 抬起的顺序与 state 位（X 语义：事件之前的修饰态）。"""
    c = await new_ctx(bus, im, "e2e-keys")

    # a) codl（Linux 出厂删行短语）：Home、Shift+End、Backspace 逐个 tap。
    ev, got = await run_phrase(c, "codl", 8)
    want = [(XK_HOME, 0, DOWN), (XK_HOME, 0, UP),
            (XK_SHIFT_L, 0, DOWN), (XK_END, STATE_SHIFT, DOWN),
            (XK_END, STATE_SHIFT, UP), (XK_SHIFT_L, STATE_SHIFT, UP),
            (XK_BACKSPACE, 0, DOWN), (XK_BACKSPACE, 0, UP)]
    check("按键合成：codl → Home / Shift+End / Backspace 的按下抬起序列与 state", ev == want,
          f"收到 {ev}")
    check("按键合成：codl 选中后不上屏候选文字", got == "", f"上屏={got!r}")

    # b) key.tap("Ctrl+C")：Ctrl 按下 → c 按下 → c 抬起 → Ctrl 抬起。
    ev, _ = await run_phrase(c, "zkcc", 4)
    want = [(XK_CONTROL_L, 0, DOWN), (ord("c"), STATE_CTRL, DOWN),
            (ord("c"), STATE_CTRL, UP), (XK_CONTROL_L, STATE_CTRL, UP)]
    check("按键合成：key.tap(Ctrl+C) 修饰键先按后抬、主键居中", ev == want, f"收到 {ev}")

    # d) 合成的键不回流进输入法：key.tap("a") 之后应用侧没有预编辑、服务端没有组合
    #    （接着按空格交还宿主 = 服务端没有在组字），随后照常打字。
    ev, got = await run_phrase(c, "zkta", 2)
    seen = [p for p in c.preedits if p]
    space_eaten = await c.key(" ")
    check("按键合成：key.tap(a) 只经 forwardKey 交给应用",
          ev == [(ord("a"), 0, DOWN), (ord("a"), 0, UP)] and got == "", f"收到 {ev} 上屏={got!r}")
    check("按键合成：合成的 a 不回流成组字（无预编辑、空格交还宿主）",
          not seen and space_eaten is False, f"预编辑={seen} 空格被吃={space_eaten}")
    await c.type("nihao")
    await c.key(" ")
    got = c.take()
    check("按键合成：之后照常打字", got == "你好", f"上屏={got!r}")

    # 不认识的组合（Hyper+C）整条丢弃；超长 seq（65 个组合）整条丢弃。
    ev, _ = await run_phrase(c, "zkbad", 1, timeout=1.0)
    check("按键合成：不认识的修饰名整条丢弃（不退化成裸 c）", ev == [], f"收到 {ev}")
    ev, _ = await run_phrase(c, "zklong", 1, timeout=1.0)
    check("按键合成：超过 64 个组合的 key.seq 整条丢弃", ev == [], f"收到 {ev}")

    # c) key.hold / key.release 成对；按住 Shift 时 tap End 带 Shift 位。
    ev, _ = await run_phrase(c, "zkhs", 1)
    check("按键合成：key.hold(Shift) 只按下", ev == [(XK_SHIFT_L, 0, DOWN)], f"收到 {ev}")
    ev, _ = await run_phrase(c, "zkte", 2)
    check("按键合成：按住 Shift 时 tap End 带 Shift 位",
          ev == [(XK_END, STATE_SHIFT, DOWN), (XK_END, STATE_SHIFT, UP)], f"收到 {ev}")
    ev, _ = await run_phrase(c, "zkrs", 1)
    check("按键合成：key.release(Shift) 抬起", ev == [(XK_SHIFT_L, STATE_SHIFT, UP)], f"收到 {ev}")
    ev, _ = await run_phrase(c, "zkrs", 1, timeout=1.0)
    check("按键合成：没按住时 key.release 不凭空造抬起", ev == [], f"收到 {ev}")

    release = [(XK_SHIFT_L, STATE_SHIFT, UP)]

    # 卡键保护：失焦。
    await run_phrase(c, "zkhs", 1)
    c.forwarded.clear()
    await c.ic.call_focus_out()
    await wait_until(lambda: c.forwarded, 2.0)
    check("卡键保护：按住 Shift 后失焦 → 补发抬起", c.forwarded == release, f"收到 {c.forwarded}")
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)

    # 卡键保护：换 IC（焦点重叠——B 先 FocusIn、A 才 FocusOut：抬起发回 A，不发给 B）。
    await run_phrase(c, "zkhs", 1)
    c.forwarded.clear()
    b = await new_ctx(bus, im, "e2e-keys-b")
    await wait_until(lambda: c.forwarded, 2.0)
    check("卡键保护：按住 Shift 后焦点换到另一个 IC → 抬起发回原 IC、新 IC 收不到",
          c.forwarded == release and b.forwarded == [], f"A 收到 {c.forwarded} B 收到 {b.forwarded}")
    await c.ic.call_focus_out()
    await b.ic.call_focus_out()
    await c.ic.call_focus_in()
    await asyncio.sleep(0.2)

    # 卡键保护：宿主 reset（鼠标点击挪了光标等）。
    await run_phrase(c, "zkhs", 1)
    c.forwarded.clear()
    await c.ic.call_reset()
    await wait_until(lambda: c.forwarded, 2.0)
    check("卡键保护：按住 Shift 后宿主 reset → 补发抬起", c.forwarded == release,
          f"收到 {c.forwarded}")

    # 卡键保护：最长保持时间（e2e 调成 2 秒，出厂 10 秒）到点自动抬起。
    await run_phrase(c, "zkhs", 1)
    c.forwarded.clear()
    t0 = asyncio.get_running_loop().time()
    await asyncio.sleep(1.0)
    early = list(c.forwarded)
    await wait_until(lambda: c.forwarded, 3.0)
    dt = asyncio.get_running_loop().time() - t0
    check("卡键保护：按住超过最长保持时间自动抬起（未到点前不抬）",
          early == [] and c.forwarded == release and 1.5 <= dt <= 3.5,
          f"1 秒时={early} 最终={c.forwarded} 用时 {dt:.1f}s")

    # 收尾后打字照常。
    await c.type("nihao")
    await c.key(" ")
    got = c.take()
    check("按键合成用例之后照常打字", got == "你好", f"上屏={got!r}")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)
    return c


async def keysynth_restart_cases(bus, im, restart):
    """卡键保护：服务重启（push 断线 + 新服务的 SERVICE_READY）补发抬起。"""
    c = await new_ctx(bus, im, "e2e-keys-restart")
    # 上一次重启后 push 通道每秒重连一次，通之前推下来的帧全丢：按到收到为止。
    ev = []
    for _ in range(10):
        ev, _ = await run_phrase(c, "zkhs", 1, timeout=1.0)
        if ev:
            break
    c.forwarded.clear()
    rc = await asyncio.to_thread(subprocess.run, [restart])
    await wait_until(lambda: c.forwarded, 5.0)
    check("卡键保护：按住 Shift 后服务重启 → 补发抬起",
          ev == [(XK_SHIFT_L, 0, DOWN)] and rc.returncode == 0
          and c.forwarded == [(XK_SHIFT_L, STATE_SHIFT, UP)],
          f"按下={ev} rc={rc.returncode} 之后收到 {c.forwarded}")
    ready = await wait_ready(c)
    check("服务重启（按键合成用例）后 addon 自愈重连", ready, "一直透传")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


def service_pid():
    return open(os.path.join(os.environ["W"], "svc.pid")).read().strip()


async def timed_key(c, name):
    t0 = time.monotonic()
    eaten = await c.key(name)
    return eaten, time.monotonic() - t0


async def stall_cases(bus, im):
    """服务卡死（进程在、不响应，用 SIGSTOP 模拟）：addon 跑在 fcitx5 主进程里，每个键都同步
    等服务——不设防的话每键卡 4~6 秒、最坏永久挂住。要求：至多付一次超时（2 秒量级），之后
    熔断期内按键直接透传、焦点切换也不卡；恢复（SIGCONT）后自愈，且服务在恢复时处理掉的
    积压帧不会把旧码拼进新输入。"""
    c = await new_ctx(bus, im, "e2e-stall")
    await wait_ready(c)
    await c.type("ni")
    composing = c.preedit != ""
    pid = service_pid()
    subprocess.run(["kill", "-STOP", pid])
    try:
        first_eaten, first = await timed_key(c, "h")
        rest = [await timed_key(c, k) for k in "aoxy"]
        t0 = time.monotonic()
        await c.ic.call_focus_out()
        await c.ic.call_focus_in()
        refocus = time.monotonic() - t0
        print(f"  卡死期间：首键 {first:.2f}s 其余 {[round(d, 2) for _, d in rest]} "
              f"焦点切换 {refocus:.2f}s", flush=True)
        check("服务卡死：首键至多付一次超时（< 3 秒）", composing and first < 3.0,
              f"组字={composing} 首键 {first:.2f}s")
        check("服务卡死：熔断期内后续按键不再等（每键 < 0.5 秒）、直接透传",
              all(d < 0.5 for _, d in rest) and not any(e for e, _ in rest) and not first_eaten,
              f"{rest} 首键吃={first_eaten}")
        check("服务卡死：焦点切换不卡（< 0.5 秒）", refocus < 0.5, f"{refocus:.2f}s")
        check("服务卡死：本端收起组字（应用里不挂着旧预编辑）", c.preedit == "",
              f"preedit={c.preedit!r}")
    finally:
        subprocess.run(["kill", "-CONT", pid])
    c.take()
    c.preedits.clear()
    await asyncio.sleep(3.5)  # 熔断期（3 秒）过去
    # 不用 wait_ready 探活：它按 a 再按 Esc，Esc 会顺手清掉服务端积压出来的码，掩盖对齐缺失。
    # 服务一直是就绪的（只是停过），熔断到期后的第一个键就该被吃。
    eaten = await c.type("nihao")
    await c.key(" ")
    got = c.take()
    check("服务恢复（SIGCONT）后 addon 自愈：熔断到期后的按键重新被吃", all(eaten), f"{eaten}")
    check("服务恢复后照常上屏「你好」（卡死时积压在旧连接里的码没拼进来）", got == "你好",
          f"上屏={got!r}")
    await c.ic.call_focus_out()
    await asyncio.sleep(0.2)


async def wait_ready(c):
    """服务的 socket 起得比引擎早：词库（首次还要建 .wdat 缓存）加载完成前，按键一律透传。
    用真实通路探活——按 a 直到被吃（= 引擎开始组字），再 Esc 撤掉。"""
    for _ in range(600):
        if await c.key("a"):
            await c.key("Escape")
            c.take()
            c.preedits.clear()
            return True
        await asyncio.sleep(0.2)
    return False


async def main():
    bus = await MessageBus().connect()
    intro = await bus.introspect(SERVICE, IM_PATH)
    im = bus.get_proxy_object(SERVICE, IM_PATH, intro).get_interface(IM_IFACE)
    a = await new_ctx(bus, im, "e2e-editor")

    if not await wait_ready(a):
        check("服务引擎就绪（120s 内开始组字）", False, "一直透传")
        return 1

    # 1. 全拼 nihao + 空格 → 你好；组字期间有预编辑、上屏后预编辑清空
    eaten = await a.type("nihao")
    had_preedit = a.preedit != ""
    eaten.append(await a.key(" "))
    got = a.take()
    check("全拼 nihao+空格 上屏「你好」", got == "你好", f"上屏={got!r}")
    check("组字期间按键全部被吃、有预编辑", all(eaten) and had_preedit,
          f"eaten={eaten} preedit_seen={had_preedit}")
    check("上屏后预编辑清空", a.preedit == "", f"preedit={a.preedit!r}")

    # 2. 退格：nihaoo ← → nihao
    await a.type("nihaoo")
    await a.key("BackSpace")
    await a.key(" ")
    got = a.take()
    check("退格删一码后上屏「你好」", got == "你好", f"上屏={got!r}")

    # 3. Esc 取消：不上屏、预编辑清空；随后空格交还宿主
    await a.type("nihao")
    await a.key("Escape")
    got = a.take()
    check("Esc 取消组字：不上屏", got == "", f"上屏={got!r}")
    check("Esc 后预编辑清空", a.preedit == "", f"preedit={a.preedit!r}")
    space_eaten = await a.key(" ")
    check("Esc 后空格交还宿主（不被吃）", space_eaten is False, f"eaten={space_eaten}")
    a.take()

    # 4. 数字选词：1 选首选；2 选第二候选（与首选不同）
    await a.type("nihao")
    await a.key("1")
    got = a.take()
    check("数字 1 选首选「你好」", got == "你好", f"上屏={got!r}")
    await a.type("shi")
    await a.key(" ")
    first = a.take()
    await a.type("shi")
    await a.key("2")
    second = a.take()
    check("数字 2 选第二候选（非空且不同于首选）",
          first != "" and second != "" and first != second, f"首选={first!r} 第二={second!r}")

    # 5. Shift 单击切英文：字母交还宿主、不上屏；再切回中文恢复
    await a.tap_shift()
    eaten = await a.type("abc")
    got = a.take()
    check("Shift 切英文后字母直通（不被吃、不上屏）", eaten == [False] * 3 and got == "",
          f"eaten={eaten} 上屏={got!r}")
    await a.tap_shift()
    await a.type("nihao")
    await a.key(" ")
    got = a.take()
    check("再按 Shift 切回中文", got == "你好", f"上屏={got!r}")

    # 5b. 带修饰键的功能热键（服务的默认键位）：到得了服务、且真的生效
    async def chord(sym, code, state):
        eaten = await a.ic.call_process_key_event(sym, code, state, False, 0)
        await a.ic.call_process_key_event(sym, code, state, True, 0)
        await asyncio.sleep(0.05)
        return eaten

    # Shift+空格：全角开关。开着时无组字的空格上屏全角空格 U+3000。
    e1 = await chord(0x20, 65, STATE_SHIFT)
    await a.key(" ")
    got = a.take()
    await chord(0x20, 65, STATE_SHIFT)
    await a.key(" ")
    back = a.take()
    check("Shift+空格 被服务吃掉（全角开关）", e1 is True, f"eaten={e1}")
    check("Shift+空格 后空格上屏全角空格", got == "\u3000", f"上屏={got!r}")
    check("再按 Shift+空格 恢复半角", back == "", f"上屏={back!r}（半角空格应交还宿主）")

    # Ctrl+Shift+E：切换引擎（方案）。切走后 nihao+空格 不再是「你好」，再切回来恢复。
    e2 = await chord(0x45, 26, STATE_CTRL | STATE_SHIFT)  # Shift 下 e 的 keysym 是 'E'
    await a.type("nihao")
    await a.key(" ")
    other = a.take()
    await a.key("Escape")
    await chord(0x45, 26, STATE_CTRL | STATE_SHIFT)
    await a.type("nihao")
    await a.key(" ")
    again = a.take()
    check("Ctrl+Shift+E 被服务吃掉（切引擎）", e2 is True, f"eaten={e2}")
    check("Ctrl+Shift+E 切走后不再出「你好」", other != "你好", f"上屏={other!r}")
    check("再按 Ctrl+Shift+E 切回全拼", again == "你好", f"上屏={again!r}")

    # 6. 焦点切换：组字中失焦 → 宿主预编辑被清；回来后是全新一轮
    await a.type("nihao")
    check("失焦前确有预编辑（下一条的前提）", a.preedit != "", "组字没起来")
    await a.ic.call_focus_out()
    await asyncio.sleep(0.2)
    check("失焦时宿主预编辑被清", a.preedit == "", f"preedit={a.preedit!r}")
    await a.ic.call_focus_in()
    await asyncio.sleep(0.2)
    space_eaten = await a.key(" ")
    got = a.take()
    check("回到焦点后旧编码已丢弃（空格直通、不上屏）",
          space_eaten is False and got == "", f"eaten={space_eaten} 上屏={got!r}")

    # 7. 两个文本框之间切换：A 组字中，焦点到 B → A 的预编辑清掉，B 正常输入
    await a.type("nihao")
    await a.ic.call_focus_out()
    b = await new_ctx(bus, im, "e2e-other")
    check("切到另一个文本框时 A 的预编辑被清", a.preedit == "", f"A preedit={a.preedit!r}")
    await b.type("nihao")
    await b.key(" ")
    got_b = b.take()
    got_a = a.take()
    check("B 框输入上屏到 B、不串到 A", got_b == "你好" and got_a == "",
          f"B={got_b!r} A={got_a!r}")

    # 7b. 焦点重叠：A 组字中，B 先 FocusIn、A 才 FocusOut（无焦点组的前端允许这种顺序）。
    #     A 迟到的失焦会被服务端当陈旧事件丢掉——addon 必须在 B 激活时先替 A 收尾。
    await b.ic.call_focus_out()
    await a.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await a.type("nihao")
    await b.ic.call_focus_in()
    await asyncio.sleep(0.2)
    await a.ic.call_focus_out()
    await asyncio.sleep(0.2)
    check("焦点重叠时 A 的预编辑同样被清", a.preedit == "", f"A preedit={a.preedit!r}")
    await b.type("nihao")
    await b.key(" ")
    got_b = b.take()
    got_a = a.take()
    check("焦点重叠时旧编码不拼进 B", got_b == "你好" and got_a == "",
          f"B={got_b!r} A={got_a!r}")

    # 8. 密码框：声明 Password 能力位 → 服务端强制英文直通
    p = await new_ctx(bus, im, "e2e-login", caps=CAP_PREEDIT | CAP_PASSWORD)
    eaten = await p.type("nihao")
    got = p.take()
    check("密码框里字母直通（不组字、不上屏）", eaten == [False] * 5 and got == "",
          f"eaten={eaten} 上屏={got!r}")

    # 候选窗（X11）：只在 e2e.sh 起了 Xvfb 时跑
    await p.ic.call_focus_out()
    await preedit_display_cases(bus, im)
    await keysynth_cases(bus, im)
    if os.environ.get("DISPLAY"):
        await x11_cases(bus, im)
        await overlay_cases(bus, im)
        await menu_cases(bus, im)
        await overlay_interaction_cases(bus, im)
        await mode_icon_cases(bus, im)
        await settings_cases(bus, im)
        await a.ic.call_focus_in()
        await asyncio.sleep(0.2)

    # 9. 服务重启：addon 的请求连接变成死连接，必须自愈（重连 + 重试当前键），无需重启 fcitx5
    restart = os.environ.get("WIND_E2E_RESTART")
    if restart:
        rc = await asyncio.to_thread(subprocess.run, [restart])
        check("重启服务", rc.returncode == 0, f"restart 退出码 {rc.returncode}")
        await a.ic.call_focus_in()
        ready = await wait_ready(a)
        check("服务重启后 addon 自愈重连（按键重新被吃）", ready, "一直透传")
        await a.type("nihao")
        await a.key(" ")
        got = a.take()
        check("服务重启后照常上屏「你好」", got == "你好", f"上屏={got!r}")
        await a.ic.call_focus_out()
        await stall_cases(bus, im)
        await keysynth_restart_cases(bus, im, restart)
        await a.ic.call_focus_in()
        await asyncio.sleep(0.2)
        if os.environ.get("DISPLAY"):
            await a.ic.call_focus_out()
            await menu_restart_cases(bus, im, restart)
            await overlay_restart_cases(bus, im, restart)

    ok = all(results)
    print(f"{'PASS' if ok else 'FAIL'} 总计 {sum(results)}/{len(results)}", flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
