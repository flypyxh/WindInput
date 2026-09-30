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

# X11 keysym 与 US 布局的 X keycode（evdev + 8）。
KEYSYM = {c: ord(c) for c in "abcdefghijklmnopqrstuvwxyz0123456789 "}
KEYSYM.update({"BackSpace": 0xFF08, "Return": 0xFF0D, "Escape": 0xFF1B, "Shift_L": 0xFFE1})
KEYCODE = {
    **dict(zip("qwertyuiop", range(24, 34))),
    **dict(zip("asdfghjkl", range(38, 47))),
    **dict(zip("zxcvbnm", range(52, 59))),
    **dict(zip("1234567890", range(10, 20))),
    " ": 65, "Return": 36, "Escape": 9, "BackSpace": 22, "Shift_L": 50,
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
        self.ic.on_forward_key(lambda sym, state, rel: self.forwarded.append((sym, rel)))
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

    async def tap_shift(self):
        # 单击 Shift：按下时 state 不含 Shift，松开时含（X11 语义：state 是事件前的修饰态）。
        code = KEYCODE["Shift_L"]
        sym = KEYSYM["Shift_L"]
        await self.ic.call_process_key_event(sym, code, 0, False, 0)
        await self.ic.call_process_key_event(sym, code, STATE_SHIFT, True, 0)
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

    ok = all(results)
    print(f"{'PASS' if ok else 'FAIL'} 总计 {sum(results)}/{len(results)}", flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
