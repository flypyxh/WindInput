#!/usr/bin/env python3
"""从 octagram `.gram` 语法模型蒸馏联想的「下一词」表（智能联想 Bigram 源的出厂数据）。

    python3 scripts/lm/distill_assoc_bigram.py \
        --gram bgw.gram --dict base.dict.yaml [--dict 8105.dict.yaml ...] \
        --out data/assoc/assoc_bigram.txt

为什么要蒸馏、不直接在线查 `.gram`：`.gram` 的键是「前文 + 后词」直接拼起来的字串，
**没有词边界**，要取某词的全部后继得把它下面整棵子树走一遍再按词库切——在线做太慢，
模型本身（bgw 41MB、万象 400MB）也不适合随手机包带。这里离线走一遍，只留常用上文的
前 K 个后继，产物一两 MB 的纯文本。

★ 只能用 **ln(频次) 单调** 的模型（bgw）。万象 `.gram` 的分值不随频次增减
（实测「今天+三缺一」22.3 > 「今天+是」16.0），按分值排出来是乱的。

产物格式（UTF-8，`#` 开头是注释）：

    上文<TAB>后继1 后继2 …

- 按上文的 UTF-8 字节序升序（读端二分查找）；后继按概率降序。
- 后继有两类，放在同一把尺子上排（见 `_mix`）：模型里的「下一词」，与词库里以上文开头的
  更长的词补出的那截（「我」→「们」）。两类都只显示、上屏补出的那截，读端不必区分。
- 单字上文有两行：普通行（`我`）只有下一词——整词打的人（拼音）单独上屏「我」，就是要这个
  字；`~我` 行混入词内接续——逐字打的人（五笔）上屏「我」，常是「我们」的头一个字。读端按
  方案挑。两种权重是拿 `assoc_eval_prep.py` 的评测集比出来的：单字行混接续，整词上屏的
  Top1 从 10.3% 掉到 8.3%；不混，逐字上屏的 Top1 从 19.5% 掉到 10.4%。
- `$` 是模型里的句末记号，原样保留在它的名次上：读端据此决定标点要不要提前。
"""
import argparse
import hashlib
import math
import mmap
import os
import struct
import sys
from multiprocessing import Pool

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gram import encode  # noqa: E402

# 网页界面上的套话词：bgw / 万象都是网页语料，「添加到播放列表」「开发和发布」「在弹出窗口」
# 这类按钮文字重复成千上万次，把「到 → 播放」「和 → 发布」顶到了常用字后继的前几名。
# 只拦「下一词」，不拦词内接续（「小游戏」是词库里的词，照出）。
WEB_BOILERPLATE = frozenset(
    "播放 播放列表 发布 弹出 弹出窗口 查看 评论 下载 登录 注册 点击 浏览 收藏 分享 转发 "
    "回复 提示 酒店 游戏 网 首页 页面 链接 帖子 楼主 版权 隐私 上传 订阅".split()
)

# 逐字上屏用的单字行的键前缀（见文件头「产物格式」）
CHAR_WISE_MARK = "~"

# .gram 分值 = ln(频次) × 10000
SCORE_SCALE = 10000.0

# .gram 键最多 8 个字（gram_encoding.h::kMaxEncodedUnicode），上文 + 后继合起来不能超过
MAX_KEY_CHARS = 8

_arr = None
_n = 0
_vocab = None
_max_succ_chars = 4


def _open(path):
    f = open(path, "rb")
    mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    (off,) = struct.unpack_from("<i", mm, 40)
    start = 40 + off
    n = (len(mm) - start) // 4
    return memoryview(mm)[start:start + n * 4].cast("I"), n


def _offset(u):
    return (u >> 10) << ((u & 0x200) >> 6)


def _label(u):
    return u & 0x800000FF


def _walk(key):
    idx = 0
    u = _arr[idx]
    for b in key:
        idx ^= _offset(u) ^ b
        if idx >= _n:
            return None
        u = _arr[idx]
        if _label(u) != b:
            return None
    return idx


def _leaf(idx):
    u = _arr[idx]
    if not (u >> 8) & 1:
        return None
    return _arr[idx ^ _offset(u)] & 0x7FFFFFFF


def _children(idx):
    base = idx ^ _offset(_arr[idx])
    for b in range(1, 256):
        c = base ^ b
        if c < _n and _label(_arr[c]) == b:
            yield b, c


def _decode(bs):
    """encode 的逆（只认 ASCII 与 CJK 主区两种形态，其余返回 None）。"""
    out = []
    i = 0
    while i < len(bs):
        b = bs[i]
        if b < 0x80:
            out.append(chr(b))
            i += 1
        elif 0x80 <= b < 0xE0 and i + 1 < len(bs):
            out.append(chr(((b - 0x40) << 8) | bs[i + 1]))
            i += 2
        elif b == 0xE1 and i + 1 < len(bs):
            out.append(chr((bs[i + 1] - 0x40) << 8))
            i += 2
        else:
            return None
    return "".join(out)


def _is_han(s):
    return all("一" <= c <= "鿿" for c in s)


def _init(gram, vocab, max_succ_chars):
    global _arr, _n, _vocab, _max_succ_chars
    _arr, _n = _open(gram)
    _vocab = vocab
    _max_succ_chars = max_succ_chars


def _straddles(ctx, s):
    """多字后继的开头几个字能与上文连成词（「中」+「国人」：中国|人）⇒ 键是跨词边界切错的。

    单字后继不算：「中」+「国」正是「中国」的后半截，智能联想里只显示「国」，正是要的。
    整个「上文+后继」是词也不算：词库里组合式的词条很多（「我的」「我是」），按它过滤会把
    真正的下一词一并拿掉（评测：整词上屏 Top1 10.4% → 7.0%）。
    """
    return any(ctx + s[:k] in _vocab for k in range(1, len(s)))


def _successors(ctx):
    """上文 ctx 的后继：(词, 分值) 按分值降序，后继须是词库里的词（或单字），全汉字；
    以及 ctx 下**全部**后继（不论在不在词库）的总量 ln Σ e^分值——归一化成条件概率用。"""
    node = _walk(encode(ctx))
    if node is None:
        return ctx, [], None
    limit = min(_max_succ_chars, MAX_KEY_CHARS - len(ctx))
    found = []
    total = None
    # 显式栈 DFS；深度按**字**算（CJK 一字两字节，$ 一字节）
    stack = [(node, b"")]
    while stack:
        idx, acc = stack.pop()
        if acc:
            v = _leaf(idx)
            if v is not None:
                total = _logaddexp(total, v / SCORE_SCALE)
                s = _decode(acc)
                if s == "$" or (s and _is_han(s) and s in _vocab and not _straddles(ctx, s)):
                    found.append((s, v))
        if len(acc) >= 2 * limit or acc == b"$":
            continue
        for b, c in _children(idx):
            stack.append((c, acc + bytes([b])))
    found.sort(key=lambda x: -x[1])
    return ctx, found, total


def _logaddexp(a, b):
    if a is None:
        return b
    hi, lo = max(a, b), min(a, b)
    return hi + math.log1p(math.exp(lo - hi))


def _mix(
    ctx, found, total, han, words_sorted, max_succ_chars, cont_weight, blocklist=WEB_BOILERPLATE
):
    """把「下一词」与「词内接续」放到同一把尺子上，返回按概率降序的 [(显示, 概率)]。

    打完 ctx 后用户接着要的东西有两类：
    - **词内接续**：ctx 只是一个更长的词的开头（五笔逐字打「我」「们」）。按词库：
      权重(ctx+尾) / D，D = 权重(ctx) + Σ 权重(以 ctx 开头的更长的词)；
    - **下一词**：ctx 是个完整的词，后面接别的词。按模型：权重(ctx) / D × P(后继 | ctx)，
      P 在 ctx 的全部后继（含句末）上归一。

    两类显示都是「补出的那截」，同一截两处都有就相加。接续尾巴最多 `max_succ_chars`
    之内的 2 字（与读端 `SMART_PREFIX_TAIL_MAX` 一致：更长的读端也不出）。
    """
    import bisect
    w_ctx = han.get(ctx, 0)
    i = bisect.bisect_left(words_sorted, ctx)
    longer = []
    while i < len(words_sorted) and words_sorted[i].startswith(ctx):
        w = words_sorted[i]
        if len(w) > len(ctx):
            longer.append(w)
        i += 1
    d = w_ctx + sum(han[w] for w in longer)
    if d <= 0:
        return [(s, 0.0) for s, _ in found]
    probs = {}
    for w in longer:
        rest = w[len(ctx):]
        if len(rest) <= min(2, max_succ_chars) and han[w] > 0:
            probs[rest] = probs.get(rest, 0.0) + cont_weight * han[w] / d
    if total is not None and w_ctx > 0:
        for s, v in found:
            if s in blocklist:
                continue
            p = w_ctx / d * math.exp(v / SCORE_SCALE - total)
            probs[s] = probs.get(s, 0.0) + p
    ranked = sorted(probs.items(), key=lambda x: -x[1])
    # 「们」排在前面时「们的」只是它的延长，同栏里两个都出是重复
    out = []
    for s, p in ranked:
        if s != "$" and any(s.startswith(t) for t, _ in out if t != "$"):
            continue
        out.append((s, p))
    return out


def _read_dict(path):
    """rime dict.yaml：`...` 之后每行 `词<TAB>编码[<TAB>权重]`。返回 {词: 最大权重}。"""
    words = {}
    body = False
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not body:
                body = line.startswith("...")
                continue
            if not line.strip() or line.startswith("#"):
                continue
            p = line.rstrip("\n").split("\t")
            w = int(p[2]) if len(p) >= 3 and p[2].strip().isdigit() else 0
            if words.get(p[0], -1) < w:
                words[p[0]] = w
    return words


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--gram", required=True, help=".gram 模型（须 ln 频次单调，如 bgw）")
    ap.add_argument("--dict", action="append", required=True, help="词库 dict.yaml，可多次给")
    ap.add_argument("--out", required=True)
    ap.add_argument("--contexts", type=int, default=40000, help="取词库权重最高的多少个上文")
    ap.add_argument("--top", type=int, default=12, help="每个上文留几个后继")
    ap.add_argument("--max-context-chars", type=int, default=4)
    ap.add_argument("--max-succ-chars", type=int, default=4)
    ap.add_argument(
        "--cont-weight-n", type=float, default=0.2, help="多字上文的词内接续权重（评测定的）"
    )
    ap.add_argument(
        "--cont-weight-charwise",
        type=float,
        default=1.0,
        help=f"单字上文「{CHAR_WISE_MARK}」行（逐字上屏用）的词内接续权重；普通单字行不混接续",
    )
    ap.add_argument("--source", default="", help="写进产物头注释的模型来源说明")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    a = ap.parse_args()

    vocab = {}
    for d in a.dict:
        for w, wt in _read_dict(d).items():
            if vocab.get(w, -1) < wt:
                vocab[w] = wt
    han = {w: wt for w, wt in vocab.items() if _is_han(w)}
    contexts = sorted(
        (w for w in han if len(w) <= a.max_context_chars),
        key=lambda w: -han[w],
    )[: a.contexts]
    print(f"词库 {len(han)} 词，上文 {len(contexts)} 个", file=sys.stderr)

    with open(a.gram, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    rows = []
    words_sorted = sorted(han)
    with Pool(a.jobs, initializer=_init, initargs=(a.gram, set(han), a.max_succ_chars)) as pool:
        for i, (ctx, succ, total) in enumerate(
            pool.imap_unordered(_successors, contexts, chunksize=1)
        ):
            cont = 0.0 if len(ctx) == 1 else a.cont_weight_n
            mixed = _mix(ctx, succ, total, han, words_sorted, a.max_succ_chars, cont)
            if mixed:
                rows.append((ctx, [s for s, _ in mixed[: a.top]]))
            if len(ctx) == 1:
                cw = _mix(
                    ctx, succ, total, han, words_sorted, a.max_succ_chars, a.cont_weight_charwise
                )
                if cw:
                    rows.append((CHAR_WISE_MARK + ctx, [s for s, _ in cw[: a.top]]))
            if i % 5000 == 0:
                print(f"  {i}/{len(contexts)}", file=sys.stderr)
    rows.sort(key=lambda r: r[0].encode("utf-8"))

    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("# 联想「下一词」表：上文<TAB>后继（按概率降序，空格分隔；$ = 句末）\n")
        f.write("# 由 scripts/lm/distill_assoc_bigram.py 生成，勿手改\n")
        if a.source:
            f.write(f"# 模型来源：{a.source}\n")
        f.write(f"# 模型 sha256：{digest}\n")
        f.write(
            f"# 参数：上文 {a.contexts} 个（≤{a.max_context_chars} 字），每个留 {a.top} 个后继"
            f"（≤{a.max_succ_chars} 字），后继须在词库内；混入词库里的词内接续，按概率排\n"
        )
        for ctx, succ in rows:
            f.write(f"{ctx}\t{' '.join(succ)}\n")
    print(f"写出 {len(rows)} 行 → {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
