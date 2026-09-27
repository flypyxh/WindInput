# 渲染文字的分段着色：主题角色色 + 模板内联颜色

> 2026-09-27 设计，§16 三项已于同日确认，§17 为待确认。
> 前身：`candidate-comment-layering.md`（注释模板语法）、`candidate-tooltip-sections.md`
> （气泡段列表与 `TooltipDoc`；其 §9「段标题的独立样式……属主题能力扩展，另立」的**颜色**部分
> 由本设计承接，加粗仍不做）。

## 1 问题

候选注释与悬停提示的文字，现在整段只有一个颜色：

- View 的文字叶子只有一个 `text_color`（`wind-ui/src/view.rs` `View`），
  `TextRenderer::draw` 也只收一个 `color`（DirectWrite / CoreText / mock 三个后端同契约）。
- 主题只能给整个节点配色：`[comment] color`、`[comment.selected] color`、`[tooltip] color`。

而模板恰恰是把**多种信息拼进一行**的工具：

| 场景 | 渲染结果 | 看不出来的 |
|---|---|---|
| 出厂注释 `${code_hint\|code_rev\|shuangpin}` | `kao` | 这是剩余编码、反查编码还是双拼 |
| `(拼: ${pinyin} ${chaizi})` | `(拼: nǐ 亻尔)` | 读音与字根的分界 |
| 气泡合并段 `${char}：{${chaizi}{ [${chaizi_code}]}\t}${readings}` | `好：女子 [vbg]	hǎo` | 字根、编码、读音三列 |
| 气泡段名 | `[编码(五笔)]` | 段名与内容同色同字（文档站 candidate-tooltip「已知局限」写明了这一条） |

### 1.1 调研核实的现状（2026-09-27）

| 事实 | 位置 | 对本设计的影响 |
|---|---|---|
| 文字叶子是「整串一次排版」，插入符是覆盖层、刻意不拆节点 | `view.rs` `View::caret_at` 字段文档 | 着色同理：片段挂在叶子上，不拆节点（§7.1） |
| DirectWrite 测量缓存键 = 文本 + 字号 + 字重 + 字族（`measure_key`）；**layout 不缓存**，measure 未命中与每次 draw 各建一个 | `dwrite.rs` `measure` / `draw` | 颜色只进 draw，不进 measure，缓存天然不被污染（§7.2） |
| `DrawGlyphRun` 的 drawing effect 参数（`_effect`）至今未用；颜色经 `clientDrawingContext` 整段透传 | `dwrite.rs` `GlyphRenderer` | 按区间上色走 `SetDrawingEffect` + 读 effect（§7.2） |
| 文字 alpha 不交给 DirectWrite，而是 draw 第 3 步按**整段一个** `fa` 后混 | `dwrite.rs` `draw` 步骤 3 | 片段颜色 alpha 不同时要分组多遍（§7.2） |
| CoreText 按属性串排版，`make_line` 已给全区间设 `kCTForegroundColorAttributeName` | `coretext.rs` | 改成按区间设即可，alpha 由 CG 原生处理 |
| 状态 patch 只对 `item`/`text`/`index`/`comment`/`footer_bar.disabled`/菜单项求值；**`tooltip` 没有状态 patch** | `resolve.rs` `resolve_views` | 角色表：注释节点有常态/选中/悬停三份，气泡只有常态（§5.3） |
| `resolve_state` 的 nil 门控只看色/图/边框/字重，状态 patch 只写了别的字段时整体为 `None` | `resolve.rs` `resolve_state` | 只写 `[comment.selected.roles]` 的 patch 会被丢掉，门控要加一条（§5.2） |
| 注释的选中/悬停正文色只看 `comment.selected/hover`，**不继承 `item.selected` 的色** | `candidate_window.rs` `eff_text` | 规则 3 的「该状态正文色」= `eff_text(&v.comment, …)`（§6.3） |
| 直立态（`upright`）注释本就逐字切成多个叶子 | `candidate_window.rs` `upright_text` | 片段按格切分（§7.1），不是本设计引入的拆分 |
| 宿主渲染（TSF 带窗口）候选窗与气泡都由 UI 进程画进 SHM；macOS 候选窗同样是 Rust 画进 SHM | `manager.rs` `try_host_render_candidates`、`manager_macos.rs` | 自动覆盖，无需另改（§7.5） |
| macOS 气泡是 .app 原生绘制，只收纯文本 + 前景/背景两个 hex | `wind-ipc` `encode_tooltip_show`、`TooltipPanel.swift` | 唯一要改线协议的地方（§7.6） |
| `CandidateItem` 经进程内 mpsc（`UiCommand`）交 UI，不过 IPC；TSF UIElement 与 Android 拉取面都不带注释 | `coordinator.rs`、`handle_uielement.rs`、`candidate_pull.rs` | IPC / bridge 不涉及（§8.4） |
| **`parse_hex` 只认 6/8 位**，而文档站 themes.mdx 与主题编辑器（`lib/color.ts`）都声称/支持 `#RGB` | `palette.rs` | 现存不一致：编辑器预览出色、引擎回落。本设计顺手补 3 位（§5.5） |
| 出厂配置、测试、文档站、设置端**无一处**出现 `$[` | grep 全仓 | 新语法与老模板冲突概率极低（§13.1） |

## 2 目标

- 注释与气泡的文字可以**分段着色**，两条通道：
  1. **主题角色色**：每个变量渲染出的文字自动带同名**角色**，主题按角色名配色，模板不用改；
  2. **模板内联色** `$[颜色]{内容}`：用户在模板里直接指定颜色。
- 选中/悬停态有明确、可预期的回落规则（§6.3）。
- 定义一组**所有主题都保证可解析**的标准色名，内联色写名字就跟随主题与明暗。
- **出厂零变化**：出厂主题不配角色、出厂模板不含内联色，外观逐像素不变；且由构造保证而非靠测试兜底（§6.4）。
- 颜色**不影响度量**：上色前后宽度、行高、换行逐像素一致。

### 非目标

- 候选正文 `text` 不纳入（§8.6）。
- 不做字重、字号、下划线等**非颜色**的分段样式——它们会改度量，破坏「颜色不影响布局」这条地基。
- 不做用户级（`config.toml`）角色色覆盖（§9）。
- 编码栏、状态提示、Toast、菜单不纳入：它们没有模板，没有分段的来源。

## 3 模型：片段串

### 3.1 数据结构（wind-ui-types）

```rust
/// 带分段样式的文字。纯文本照旧是 `text`，样式是挂在旁边的区间表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct StyledText {
    pub text: String,
    /// 按 `start` 升序、互不重叠；区间以字节计、落在字符边界上。空 = 整段节点正文色。
    /// 未被任何区间覆盖的文字 = 节点正文色。
    spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub role: Role,
    /// 在段名（气泡 label）里产出的片段：角色未配色时回落 `title` 角色，而非正文色（§3.2）。
    pub in_title: bool,
    /// 内联色（`$[…]{}`）。有值即优先于角色（§6.2）。
    pub color: Option<InlineColor>,
}

/// `TEXT_ROLES` 的下标；`Role::NONE` = 无角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Role(u16);

pub const TEXT_ROLES: &[&str] = &[
    "title", "literal", "char",
    "code_hint", "code_rev", "code_rev_all", "shuangpin", "pinyin",
    "chaizi", "chaizi_code", "chaizi_all", "chaizi_code_all", "dict", "emoji",
    "word_code", "code_source", "full_text", "readings", "unicode", "unicode_all", "debug",
];
```

- **为什么是「纯文本 + 区间表」而不是 `Vec<(String, Style)>`**：下游绝大多数代码只关心文字
  （测量、截断判定、复制、上屏、指纹、右键菜单标签），它们继续读 `text`，一行不改；只有画字的那一处
  读区间。两种表示信息等价，但前者让「颜色只进 draw」在类型上就成立。
- **为什么捆成一个类型而不是 `CandidateItem` 上并列一个 `comment_spans`**：并列字段迟早出现
  「文字改了、区间没跟着改」——截断、trim、吞空白都在改文字。捆在一起、构造时校验（升序、不重叠、
  在界内、在字符边界），不变量只守一处。
- `InlineColor` / `ColorRef` 的定义在 wind-theme（§4.2），它依赖调色板的解析；wind-ui-types 已依赖 wind-theme。
- `TEXT_ROLES` 放 wind-ui-types：协调器（产出方）、wind-ui（消费方）、文档站与主题编辑器（契约）
  都认这一张表。协调器加一条测试：`eval_var` / `eval_text_var` / `char_var` 认识的每个变量名，
  归一后都在表里——新增变量忘了登记角色就红。

### 3.2 角色从哪来（协调器产出规则）

| 文字来源 | 角色 | 说明 |
|---|---|---|
| `${name}` 的值 | `name`（别名归一） | `${code}` → `code_rev`，`${code_all}` → `code_rev_all`；主题只需认规范名 |
| `${a\|b\|c}` 的值 | **实际取到值的那个变量** | `${code_hint\|code_rev}` 取到反查码时是 `code_rev` |
| `${name:arg}` 的值 | `name` | 参数不影响角色 |
| 模板字面文字（正文） | `literal` | `(拼: `、`：`、`\t`、` [`、`]` |
| 模板字面文字（段名里） | `title` | |
| 气泡段名的装饰 `[` `]` 与 inline 段的 `: ` | `title` | 由 `TooltipDoc` 拼出，不是模板字面 |
| 段名里的变量值（`编码{(${code_source})}` 的 `五笔`） | `code_source`，`in_title = true` | 主题没配 `code_source` 时回落 `title` 色，整个段名同色 |
| 未知变量名的原样回显 `${pinyn}` | `Role::NONE` | 它是错误提示，用正文色，不借任何角色的色 |
| 截断标记 `…`（`comment_max_chars` / 气泡 `max_chars`） | 与被截处前一个字同样式 | 不单设角色：它不承载信息 |
| 非模板来源的注释（快捷加词预览行的提示等） | 无片段 | 整段正文色 |

**结构角色只设 `title`、`literal` 两个**，理由：它们是模板里仅有的两类「不是变量值」的文字，
主题要「淡化装饰、突出信息」或「段名用强调色」时离不开；再细分（括号、分隔符、制表符各一个角色）
没有真实需求，徒增契约面。`char` 是逐字段的变量，按「变量即角色」自然得到，不算结构角色。

`emoji` 角色对彩色 emoji 基本无效：彩色字形各层用字体自带调色板，只有「前景色哨兵层」
（`paletteIndex == 0xFFFF`）才用文字色。文档站注明，不特殊处理。

## 4 内联语法 `$[颜色]{内容}`

### 4.1 语法

```
$[SPEC]{BODY}

SPEC  := COLOR ( ',' KEY '=' COLOR )*        KEY 目前只认 selected
COLOR := ATOM ( '/' ATOM )?                  单值 = 亮暗共用；a/b = 亮/暗
ATOM  := '#' HEX{3|6|8} | NAME               NAME = [a-z0-9_]+，调色板 token 名
```

- `SPEC` 内各记号两侧空白忽略；`SPEC` 不得含 `]` `{` `}` 换行，长度 ≤ 64 字节。
- `BODY` 是普通模板（变量、可选段、嵌套的 `$[…]{}` 都可以写），按层数配对到对应的 `}`。
- 例：`$[accent]{${code_rev}}`、`$[#C00000/#FF8080]{(${pinyin})}`、
  `$[text_dim,selected=on_accent]{${chaizi}}`、`{ $[success]{[${dict}]}}`。

### 4.2 颜色值

| 写法 | 含义 |
|---|---|
| `#RGB` / `#RRGGBB` / `#RRGGBBAA` | 字面色（3 位扩写为 6 位） |
| `accent`、`text_dim` … | 当前主题调色板里的 token；**标准色**（§5.4）保证任何主题都有 |
| `#C00000/#FF8080`、`text/accent` | 亮/暗各一；两侧各自可以是字面色或名字 |
| `,selected=X` | 候选处于选中态时改用 X；只在注释里有意义，气泡里忽略 |
| 其它 `key=value` | 忽略（前向兼容：将来加 `hover=` 时，老版本不至于把整段判非法）；设置页预览提示 |

wind-theme 新增（`palette.rs`）：

```rust
pub struct InlineColor { pub normal: ColorRef, pub selected: Option<ColorRef> }
pub struct ColorRef { pub light: Atom, pub dark: Atom }      // 单值时两侧相同
pub enum Atom { Rgba(Rgba), Name(Arc<str>), Invalid }
impl InlineColor { pub fn parse(spec: &str) -> Self }           // 不失败，坏值落 Invalid
```

十六进制在**解析时**就校验（坏值即 `Atom::Invalid`）；名字只能在**求色时**按当前主题查（§6.2）。

### 4.3 与可选段的关系

`$[…]{…}` **只管上色，不是可选段**：

- 内部变量照常计入外层 `{…}` 与整个模板的「有值」判定；
- 内部变量全空时，它自己**不会**消失（`$[accent]{(${pinyin})}` 在无读音时仍留 `()`）——
  要「空则消失」就套一层可选段：`{$[accent]{(${pinyin})}}` 或 `$[accent]{{(${pinyin})}}`，两种等价；
- 「空变量吞掉紧邻的一个空白」照旧按**输出文字**判断，跨不跨颜色边界都一样吞。

这样定义是为了让「加颜色」对已有模板是**纯加法**：给任意一段包上 `$[…]{}`，文字输出逐字节不变。

### 4.4 何时仍是字面文字

只有「`$[` + 合法字符的 `SPEC` + `]` + 紧跟 `{` + 能配对闭合的 `BODY`」才构成语法；任一处不满足，
`$` 按字面输出、从 `[` 继续扫描。与现有 `${` 未闭合即退化为字面的宽容取向一致。

### 4.5 非法或查不到的颜色

语法完整、颜色值不对（`#GG0000`、当前主题没有的名字、`transparent`——它占宽却不可见，只会是误用）：
**该段按节点正文色显示，不回显字面**。设置页预览在该段下标出原因（§11）。

与「未知变量名原样回显」的取向不同，理由：拼错变量丢的是**信息**，必须让人看见；
颜色写错丢的只是**装饰**，文字本身完整，回显一串 `$[#GG0000]` 反而把信息淹没。

### 4.6 纯文本出口剥色

模板引擎另有两个出口，产物是**要上屏的文字**而非显示：`alt_commit_text`（上屏注释）与
`reverse_render`（cmdbar `dict.rev`）。它们继续调 `render()` 得 `String`，`$[…]{}` 只留 `BODY` 的文字。
复制 / 上屏 / 「复制全部」同理，一律取 `StyledText::text` 或气泡的原始行（本来就是纯文本）。

## 5 主题：角色表

### 5.1 TOML 写法

在视图节点下加 `roles` 子表，状态照现有 patch 形态写：

```toml
[comment.roles]
pinyin   = "${text_dim}"
code_rev = "${accent}"
literal  = { light = "#B0B0B0", dark = "#606060" }

[comment.selected]
color = "${on_accent}"         # 选中态改了注释正文色 ⇒ 未单列的角色在选中态回落到它（§6.3）

[comment.selected.roles]
code_rev = "#FFE08A"           # 单列：选中态仍保留编码高亮

[tooltip.roles]
title    = "${accent}"
readings = "#9AD0FF"
```

- 值的形态与节点 `color` 完全相同：`${token}` / `#hex` / `{ light, dark }`；`""` = 未设置（跟随正文色），
  派生主题借此撤销 base 配的某个角色。
- 只有 `comment`（含 `selected` / `hover`）与 `tooltip` 消费；写在其它节点下的 `roles` 被忽略。
- 引擎**不校验**角色名：更新版本的主题（用到了新变量的角色）在旧引擎上应静默忽略而非报错。
  拼写检查交给主题编辑器的严格模式（§10）。

### 5.2 内存模型改动（wind-theme）

| 位置 | 改动 |
|---|---|
| `schema.rs` `ViewNode` | `#[serde(default)] pub roles: HashMap<String, Ld>`；状态 patch 递归同一类型，`selected.roles` 自然可用 |
| `normalize.rs` | **不用改**：`roles` 是普通子表，`normalize_node` 不认识的键原样保留，`selected`/`hover` 已递归；加一条测试钉住扁平写法 `[comment.selected.roles]` 落到 `views.comment.selected.roles` |
| `theme.rs` `merge` | **不用改**：表对表逐键深合并，派生主题可只改一个角色；`{light,dark}` 对标量按既有「整体重置」语义 |
| `rvnode.rs` `RvNode` | `pub roles: HashMap<String, Rgba>`（已解析；空 = 未配） |
| `resolve.rs` `resolve_view_node` | 逐项 `resolve_color`，`None`（空串、未解析的 token）不入表——未解析 token 沿用现有 warn |
| `resolve.rs` `resolve_state` | nil 门控加 `|| !resolved_roles.is_empty()`；否则只写了 `[comment.selected.roles]` 的 patch 被整体丢弃 |

气泡节点 `rv.tooltip` 与注释节点走同一个 `resolve_view_node`，无需分别处理。

### 5.3 各节点覆盖的状态

| 节点 | 常态 | selected | hover | 说明 |
|---|---|---|---|---|
| `comment` | ✓ | ✓ | ✓ | `resolve_views` 已求值 `comment.selected` / `comment.hover` |
| `tooltip` | ✓ | — | — | 气泡没有状态；`[tooltip.selected.roles]` 不生效，编辑器不提供 |

`[comment.disabled]` 当前不求值（`resolve_views` 未建），本设计不新增。

### 5.4 标准色契约

内联色写名字才能「跟随主题 + 明暗」。下列名字由 `_base` 给默认，所有出厂主题都继承 `_base`，
故**保证可解析**；第三方主题只要 `base` 链上有 `_base` 同样保证。契约写进文档站（§12），
主题编辑器按此给中文名与分组。

| 名字 | 用途 | `_base` 亮 | `_base` 暗 | 状态 |
|---|---|---|---|---|
| `text` | 主文字 | `#1E1E1E` | `#E0E0E0` | 已有 |
| `text_dim` | 次要文字 | `#646464` | `#B0B0B0` | 已有 |
| `text_hint` | 提示文字（注释默认色） | `#969696` | `#808080` | 已有 |
| `accent` | 强调色 | `${primary}` | 同左 | 已有 |
| `on_accent` | 强调底上的前景 | `#FFFFFF` | 同左 | 已有 |
| `selection_text` | 选中候选文字 | `${text}` | 同左 | 已有 |
| `tooltip_text` | 气泡文字 | `#FFFFFF` | `${text}` | 已有 |
| `accent_text` | 可读的强调**文字**色 | `${accent}` | 同左 | **新增到 `_base`**（`_qingfeng` 已有自己的值，照旧覆盖） |
| `success` | 成功 / 肯定 | `#1E8E3E` | `#81C995` | **新增** |
| `warning` | 提醒 | `#B06000` | `#FDD663` | **新增** |
| `error` | 错误 / 否定 | `#D93025` | `#F28B82` | **新增** |
| `tooltip_success` / `tooltip_warning` / `tooltip_error` | 气泡内的同名语义色 | `#81C995` / `#FDD663` / `#F28B82` | 同左 | **新增**，见下 |

**气泡为什么要单独一组**：`_base` 的气泡底色亮暗两档**都是深色**（`#3C3C3CF0` / `#1E1E1EF0`），
而上面的亮档语义色是为白底候选窗调的。实测对比度：`#D93025` 在白底 4.77、在亮档气泡底 2.31；
`#1E8E3E` 分别 4.21 / 2.62。故内联色在**气泡里**按名字求色时先查 `tooltip_<名字>`，查不到再查 `<名字>`
（§6.2）。这条作用域查找只作用于内联色的名字；主题 `[tooltip.roles]` 里写的 `${token}` 由作者自己选，
不做改写。是否采用见 §17-1。

新增 token 不被任何出厂节点引用，外观零变化；`Resolved.palette` 多出几项，Android 拉取调色板
（`theme_palette`）会多拿到几个键，无害。

### 5.5 `#RGB`

`parse_hex` 补 3 位（`#F80` → `#FF8800`）。这同时修掉 §1.1 那条现存不一致：文档站与编辑器都说
支持 `#RGB`，引擎却回落。作用面是所有主题颜色字面值；此前写了 3 位色的主题在引擎里是「没配」，
修后开始生效——这是文档承诺的行为，不算回归，发版说明写一句。

## 6 求色（UI 侧）

### 6.1 为什么在 UI 侧、而不是协调器里求好颜色再下发

1. **悬停态只有 UI 知道**：选中下标协调器有，悬停由 UI 自己跟踪、不回协调器重算；
2. **明暗切换**：系统切明暗时协调器推一份新的 `Resolved`（`UiCommand::SetTheme`），UI 收到后**用手里的
   候选重绘**（`manager.rs` `SetTheme` 分支）。片段带的是「角色 / 颜色引用」而非 RGBA，重绘即取到新明暗
   下的颜色——内联亮暗对 `#C00000/#FF8080` 的取值时机就是**每次画**，不需要协调器重算候选；
3. 协调器不持有 `Resolved`（它只在切主题时加载一次推给 UI），在候选循环里查主题是新耦合。

求色函数放 wind-ui（新模块，如 `span_color.rs`），候选窗、气泡、macOS 气泡下发三处共用。

### 6.2 顺序

给定节点 `n`（`comment` 或 `tooltip`）、当前状态 `st`（常态 / 选中 / 悬停，选中优先，与 `eff_text` 同）、
一个片段 `s`：

```
body      = eff_text(n, st)                     // 该状态的正文色，现有函数
state_changed_body = st ≠ 常态 && body ≠ eff_text(n, 常态)

if let Some(ic) = s.color:                      // ① 内联色优先
    if st == 选中 && ic.selected 有值 → return resolve(ic.selected) ?? body
    if state_changed_body              → return body        // 规则 3
    return resolve(ic.normal) ?? body                        // 非法 / 查不到 → 正文色
if st ≠ 常态 && n.<st>.roles[s.role] 有值  → return 它         // ② 状态态单列的角色色
if state_changed_body                      → return body      // 规则 3
if n.roles[s.role] 有值                     → return 它         // ③ 常态角色色
if s.in_title && n.roles["title"] 有值      → return 它         // ④ 段名里的变量回落 title
return body                                                   // ⑤ 正文色
```

`resolve(ColorRef)`：按 `Resolved.is_dark` 取亮/暗侧的 `Atom`；`Rgba` 直给；`Name` 查
`Resolved.palette`（气泡里先查 `tooltip_<名字>`，§5.4）；`Invalid` / 查不到 → `None`。

### 6.3 状态回落（规则 3）

> 某状态若改了该节点正文色，而未给某角色在该状态下的颜色 → 该角色（以及内联色）回落到**该状态的
> 正文色**；状态没改正文色 → 沿用常态角色色 / 内联色。

理由：主题把选中底换成深色实心块时，一定会同时把选中态注释色改成浅色（否则灰字压在蓝底上看不见）。
此时常态为浅底调的角色色直接搬到深底上多半不可读；回落到主题作者为这个状态**亲自选过**的正文色，
是唯一有可读性担保的颜色。想在选中态也保留分色，就在 `[comment.selected.roles]` 单列，
或在内联色里写 `selected=`。

**「改了」按值判，而不是按「写没写」**：`state_changed_body` 比较的是选中态与常态的**有效正文色**。
派生主题常把 base 配的选中注释色「写回」常态色（撤销 base 的深色选中），按「写没写」判会让这类主题
在选中态无端丢掉全部分色。是否采用见 §17-2。

出厂 `_base` / `_qingfeng` 都没写 `[comment.selected] color`，选中态注释正文色 = 常态色，
故选中时角色色 / 内联色照常显示。

### 6.4 出厂零变化：构造保证

UI 把片段解析成颜色后，**丢掉颜色等于正文色的区间**；一个区间都不剩时，叶子不带颜色区间，走与今天
**同一条**绘制路径（§7.2 的早返回）。出厂主题不配角色、出厂模板不含内联色 ⇒ 每个片段都解析到正文色
⇒ 全被丢弃 ⇒ View 树与今天逐字段相同。零回归因此是结构性质，对拍只是确认（§13.2）。

## 7 渲染层

### 7.1 View 叶子

```rust
pub struct View {
    // …
    pub text: Option<String>,
    pub text_color: [u8; 4],
    /// 分段颜色：按字节区间覆盖 `text_color`。空 = 整段 `text_color`（今天的行为）。
    /// **只在 paint 消费，不参与 measure/布局**，理由同 `caret_at`。
    pub color_runs: Vec<ColorRun>,   // ColorRun { start: u32, end: u32, rgba: [u8; 4] }
}
```

**不拆节点**：把 `亻尔 [wq]` 拆成三片各自测量再拼接，`measure(a+b) != measure(a)+measure(b)`
（拆分边界丢字距、每段各自亚像素舍入），宽度随着色边界变化而抖动——与 `View::caret_at` 字段文档
记录的插入符问题是同一个问题，同一个答案：整串一次整形，颜色只作用于绘制。

直立态（`upright_text`）本就把注释逐格切成多个叶子（这个拆分今天就存在，与着色无关）：
每格取落在它字节范围内的区间、平移到格内偏移。

### 7.2 DirectWrite

- `TextRenderer` 新增 `draw_runs(buf, …, text, ts, base_color, runs)`；`runs` 为空直接调现有 `draw`
  ——出厂路径一个字节都不变。
- 建 layout 后对每个区间 `SetDrawingEffect(effect, range)`（区间换算成 UTF-16 码元，同 `pua_runs` 口径）；
  effect 是一个只装 RGBA 的轻量 COM 对象（`#[implement]`）。`DrawGlyphRun` 读 `_effect`：有则用它的色，
  无则用 `clientDrawingContext` 里的基色。彩色 emoji 的前景哨兵层同样取这个色。
- **alpha 分组**：draw 第 3 步把文字 alpha `fa` 在回写时**统一**混入，而「哪个像素属于哪个片段」
  在那一步已经不可知。故按 alpha 把区间分组，每组一遍「拷底 → Draw（只画本组，其余 glyph run 在
  `DrawGlyphRun` 里跳过）→ 按本组 `fa` 回写」。不透明色（绝大多数）与基色同组，常见情形仍是一遍；
  多一个 alpha 多一遍，每遍只扫包围盒。后一遍拷底时前一遍的字已在缓冲里，合成顺序正确。
- **缓存不受污染**：
  - `measure_cache` 键是 `measure_key(text, size, weight, family)`，measure 路径根本拿不到区间；
  - layout 不缓存：measure 未命中与每次 draw 都新建（`create_layout`），effect 只设在 draw 那个一次性
    layout 上，用完即弃；
  - `formats`（按字号）与 `line_heights`（按字号/字重/字族）与颜色无关。
  - ⚠️ 将来若给 draw 加 layout 缓存，effect 必须每次取出后重设、用后清掉，或带 effect 的 layout 不入缓存
    ——在 `create_layout_with` 的文档里写明这条约束。
- drawing effect 是格式属性，不参与整形与度量，故带 effect 的 layout 与 measure 用的 layout 字形位置相同。
  这一点由 P1 的像素测试钉住（§15），不只靠 API 文档。

### 7.3 CoreText

`make_line` 由「全区间一个 `kCTForegroundColorAttributeName`」改为「先设基色、再按区间逐个覆盖」
（`CFRange` 以 UTF-16 计）。CG 原生按区间处理 alpha，无需分组。`measure` 构造的 line 不带颜色，不受影响。

### 7.4 mock（Linux，`dwrite.rs` 的 `imp`）

`draw_runs` 同 `draw` 为空操作。布局测试照常跑；「出厂零变化」的结构断言（叶子 `color_runs` 为空）
在 Linux 上即可验证。

### 7.5 宿主渲染

- **Windows 宿主渲染**（TSF 带窗口）：候选窗与气泡都由 UI 进程 `render_frame` / `render_tooltip_frame`
  画成 BGRA 写进 SHM，C++ 侧 `HostWindow` 只贴图——走的就是 View + `TextRenderer`，自动覆盖。
- **macOS 候选窗**：同样由 Rust 用 CoreText 画进 SHM（`manager_macos.rs` `encode_host_render_frame`），
  自动覆盖。

### 7.6 macOS 原生气泡

`.app` 的 `TooltipPanel` 用 `NSAttributedString` 自绘，收的是 `CmdTooltipShow(text, bg, fg, font_path)`。

- 协议：`encode_tooltip_show` 末尾**追加**第五段 `runs`：`count: u32` + `count × (start_u16: u32, len_u16: u32,
  rgba: u32)`，区间以 **UTF-16** 计（直接对应 `NSRange`），颜色已在 Rust 侧按 §6.2 解析好。
- Swift `decodeTooltipPayload`：沿用 `fontPath` 那条「`off < buf.count` 才读」的先例容忍缺省。
  旧 `.app` + 新服务：多出的尾段被忽略，退化为单色；新 `.app` + 旧服务：没有尾段，同样单色。
- `TooltipPanel.show` 在 `.foregroundColor: fg` 之后逐区间 `addAttribute(.foregroundColor, …)`。
- `manager_macos.rs` 需要在 `SetTheme` 时留一份气泡节点的角色表与调色板（现在只留 `tooltip_bg/fg`
  两个 hex），供每帧算 `runs`。

## 8 链路：从模板到像素

### 8.1 模板引擎（`wind-coordinator/src/comment.rs`）

- `Node` 新增 `Color(InlineColor, Vec<Node>)`；`parse` 在 `$` 分支里先判 `${`、再判 `$[`（§4.4）；
  `find_group_end` 不用改——`SPEC` 里不许出现花括号，`$[…]{…}` 贡献的花括号天然配平。
- `render_nodes` 的输出从 `String` 改为一个片段构建器：`push(&str, Role, in_title, color)` 合并相邻同样式；
  `pop_whitespace()` 取代现在的 `out.pop()`（吞空白时同步收缩末尾区间、删掉变空的区间）。
- `render()`（纯文本，供 §4.6 两个出口）保留签名，内部取构建器的 `text`；新增 `render_styled()` 给
  `comment_for` 用。`trim()` 与 `comment_max_chars` 截断改在构建器上做：截断按 `char` 计（现口径不变），
  `…` 继承被截处前一个字的样式。
- ⚠️ `Template::references` 与 `has_literal` 的遍历**必须下钻 `Color` 节点**。漏了的后果不显眼：
  `$[x]{${debug}}` 不被认为引用了 `debug` → 调试上下文不准备 → 段静默为空；`$[x]{…\t…}` 不被认为是
  分列段 → 被折行。两个 walker 各加一条用例。

### 8.2 气泡（`wind-coordinator/src/tooltip.rs`）

`CompiledTooltip::render` 里文字经过的每一步都要带着样式走：

| 步骤 | 现状 | 改后 |
|---|---|---|
| 行求值 `sec.template.render` | `(String, bool)` | `(StyledText, bool)`；逐字段每行一个 |
| `promote` 稳定分组 | 按行排序 | 不变（行是整体） |
| `rows → raw`（按 `\n` 拆、去首尾空白行） | `Vec<String>` | 原始行仍存**纯文本**（复制 / 上屏取值），另存同形的带样式行供显示 |
| `display_lines`：换行符归一 | `replace` | 按字素簇走：`\r\n` 本就是一个字素簇，逐簇映射成 `\n`，样式跟随 |
| 单行截断 `max_chars` + `…` | 字素簇 | 同；`…` 继承前一簇样式 |
| 折行 `wrap` | 已是 `Vec<(&str, 宽度)>` | 元素加样式下标；`trim_end`、续行跳过行首空白都按簇做 |
| 段名 `sec.label.render` + `trim` | `String` | `StyledText`（`in_title = true`，字面文字角色 `title`） |

`wind-ui-types`：

```rust
pub struct TooltipSection { pub title: Option<StyledText>, pub inline: bool, pub lines: Vec<TooltipLine> }
pub struct TooltipLine    { pub text: StyledText, pub raw: u16 }
```

- `plain_lines()` 改为产出带样式的行（`[`、`]`、`: ` 以 `title` 角色拼进去），`to_plain_text()` 与新增的
  `to_styled()`（整块文字 + 区间，给自绘气泡与 macOS 下发）都从它取——画出来的行、命中换算的行、
  下发的行仍是同一份排列。
- `hit_at_line`、行等高命中、`doc_fingerprint` 比对：不受影响。指纹会把片段算进去，UI 与协调器对同一份
  文档算出同一个值，比对语义不变。
- 右键菜单标签（`handle_tooltip.rs` `section_label`）、复制 / 上屏段与行、「复制全部」（`raw_plain_text`）
  全部取纯文本。

### 8.3 类型改动清单

| 类型 | 改动 |
|---|---|
| `CandidateItem.comment` | `String` → `StyledText`（`From<String>` 给无样式来源用；`is_empty()` / `as_str()` 供现有调用点） |
| `TooltipSection.title` | `Option<String>` → `Option<StyledText>` |
| `TooltipLine.text` | `String` → `StyledText` |
| `View` | 加 `color_runs` |
| `TextRenderer`（三后端） | 加 `draw_runs` |
| `wind-ipc` `encode_tooltip_show` | 加尾段 `runs`（Swift 解码同步） |

### 8.4 IPC / bridge

不涉及。`CandidateItem` 经进程内 `UiCommand` 通道交给 UI；TSF UIElement（游戏兼容）与 Android
拉取面（`candidate_pull`）都不带注释；宿主渲染传的是像素。唯一的线协议改动是 §7.6 的 macOS 气泡。

### 8.5 候选正文 `text` 为什么本期不纳入

1. **没有来源**：正文不是模板产物，没有变量可成为角色，也没有地方写内联色；
2. **UI 会改它**：候选窗按可用宽度截断（`truncate_candidate_text`）、协调器按 `ui.candidate.max_chars` 截断、
   命令 / 放宽检索的前缀是拼上去的——每一处都要做区间映射，收益却还没有需求支撑；
3. **可读性最敏感**：正文选中色（`text.selected`）是主题最着力调的一处，分色在这里最容易出事故。

将来有真实需求（例如给「补来的候选」前缀单独着色）时，本设计的 `StyledText` + View 区间能力可直接复用，
只需补正文这条链路的映射。

## 9 用户级角色色：本期不做（config-design-rules R3）

R3「外观覆盖主题」要求用户值逐格回落、只在一处合并。角色色放进 `config.toml` 会是
「角色 × 节点 × 状态 × 亮暗」的开放映射，既不是 REGISTRY 能登记的标量，也没有 R8 那种「勾选行」
能呈现的形态。而用户想要的两件事已有通道：

- **按自己的意思给某段上色** → 模板内联色。它本身就是用户值，优先于主题角色（符合 R3.1 的「用户 > 主题」）；
  写标准色名还能跟随主题与明暗；颜色非法只让该段回落正文色，不牵连别的段（符合 R3.4）。
- **整体换一套角色配色** → 派生主题：`base = "default"` + `[comment.roles]` 几行。文档站给出配方。

真出现「不想碰模板、也不想建主题」的需求时，再按 R3 加 `ui.candidate.comment_role_colors` 一类映射键。

## 10 主题编辑器（WindInputThemeEditor）同步点

编辑器对外的契约在面板文案里，以下几处要与引擎同步：

| 文件 | 改动 |
|---|---|
| `lib/theme3/types.ts` `ViewNodeV3` | 加 `roles?: Record<string, Color>` |
| `lib/theme3/toml.ts` | `NODE_KNOWN` 加 `roles`；读写 `[comment.roles]`、`[comment.selected.roles]`、`[comment.hover.roles]`、`[tooltip.roles]` |
| `lib/theme3/strict.ts` | `roles` 的键**不是**开放命名空间：按公开角色表（从 `TEXT_ROLES` 抄一份，附来源注释）报未知角色，抓 `pinyn` 这类错字 |
| `lib/theme3/resolve.ts` + `engineParity.test.ts` | 求值角色表（含状态），与引擎对拍 |
| `lib/theme3/tokenMeta.ts` | 新 token 中文名：`accent_text` 强调文字、`success` 成功、`warning` 提醒、`error` 错误、`tooltip_*` 同名「（提示框）」；`TOKEN_GROUPS` 新增「文字标注色」组 |
| `lib/theme3/presets/baseChain.ts` | `_base` 改动后跑 `pnpm bake:theme` 重新烘焙（勿手改） |
| `lib/color.ts` | 已支持 `#RGB`，无需改；§5.5 修的是引擎一侧 |
| `components/form/ViewsEditorV3.vue` | 注释、提示框节点加「文字角色色」面板：常态 / 选中 / 悬停三栏（提示框只有常态）；文案写明回落规则 §6.3 |
| `lib/preview/candidateBox.ts`、`otherWindows.ts` | 预览样例带片段（如注释 `kao` 标 `code_hint`、气泡段名标 `title`），按 §6.2 求色，否则配了角色在预览里看不出来 |

## 11 设置端（wind-setting）

两处编辑模板的界面：候选注释对话框（`manifest.rs` `build_dialog_button_comment` →
`field_dialogs.rs` `build_comment_dialog`）与提示内容对话框的段表单（`dialogs/tooltip_sections_dialog.rs`）。

- **预览行**（两处都加）：模板输入框下方一行，显示当前主题下的渲染效果（常态；注释另显一行选中态）。
  - 值来自 core 新 RPC（如 `appearance.previewTemplate`）：入参模板 + 场景（注释 / 段名 / 段内容 + `each`），
    core 用固定样例（`你好`，候选专属变量给样例值）求值，返回 `{ text, runs: [{start, end, rgba}], problems:
    [{start, end, message}] }`，颜色已按当前主题与明暗解析。设置端不依赖 wind-theme，求色必须在 core。
  - 渲染用 windui 的 `RichText`（`src/ui/rich.rs`，支持逐 span 固定色），底色取主题窗口底色 / 气泡底色。
  - `problems` 覆盖：颜色非法、名字当前主题查不到、未知 `key=`、未知变量名（后者今天只在候选栏里能看到）。
  - 新 RPC 在 `wind-rpc` 的 dispatch / security 白名单登记，mock 回包补一条。
- **调色板点选插入**：输入框旁「插入颜色」按钮，弹出标准色色块（§5.4，按当前主题着色、带中文名）
  + 「自定义 #…」。有选区（windui core `selection_of`）则包住选区成 `$[名]{选区}`，无选区插入 `$[名]{}`
  并把光标放进花括号。
- 参考区（可复制的变量表 / 示例）补一行 `$[颜色]{内容}` 与一个示例。
- 新文案过设置项 label 的拼音检索表（「插入颜色」「预览」若进检索索引）。

## 12 文档站（WindInputDocs）

| 页面 | 改动 |
|---|---|
| `settings/appearance/candidate-comment.mdx` | 「语法」加 `$[颜色]{内容}` 小节（§4 的颜色值表、与可选段的关系、写错的表现、选中态回落）；加 `<Since>` |
| `settings/appearance/candidate-tooltip.mdx` | 同上一句带过并链到注释页；「平台支持与已知局限」里「段名没有单独的样式（加粗、颜色）」改为「段名可由主题配色（`title` 角色），不能加粗」；macOS 旧版 `.app` 单色的说明 |
| `settings/appearance/themes.mdx` | 新增「文字角色色」节：`[comment.roles]` / `[tooltip.roles]`、状态写法、回落规则、角色名全表（§3.2 + `TEXT_ROLES`）；新增「标准色」节：§5.4 表（写明第三方主题要继承 `_base` 才有保证）；「颜色格式」处 `#RGB` 与实现对齐 |
| 配方 | 「选中时保留编码高亮」「淡化装饰、突出信息」「派生主题只改角色色」 |

## 13 兼容与零回归

### 13.1 老模板里的 `$[`

出厂 `data/config.toml`、wind-coordinator / wind-ui 测试、文档站、设置端均为 0 处。用户模板要撞上，
得恰好写出「`$[` + 64 字节内的 `]` + 紧跟 `{…}` 配平」——注释模板里几乎不可能；撞上时表现为
`$[…]` 消失、内容照常显示（颜色非法按正文色），不丢信息。

### 13.2 对拍

| 期 | 对拍 |
|---|---|
| P1 | Windows 目标（wine / CI）：① 同一文本，`runs` 为空 vs 全部 run 取基色 → 缓冲逐字节相同；② 多色 run vs 单色 → 「被改动像素」掩码逐像素相同（颜色不影响字形位置）；③ `measure` 结果与缓存条目数不受 `draw_runs` 调用影响 |
| P2 | 旧 `render()` 作为测试内参照实现保留：出厂模板 + 现有全部注释用例，`render_styled().text` 与参照逐字节相同；气泡 2⁵ 开关组合对拍（`tooltip.rs` 现有）改用 `doc.to_plain_text()` 断言，照旧全绿；每个 `$[…]{}` 用例同时断言「去掉 `$[…]{` `}` 后的模板输出相同文字」（§4.3 纯加法） |
| P3 | 全部出厂主题（`_base`、`_qingfeng`、`default`、`amber`、`jade`、`violet`、`msime`）× 亮暗：`RvViews` 各节点 `roles` 为空；出厂主题 × 出厂模板 × 选中/悬停/常态构建候选窗 View 树：所有叶子 `color_runs` 为空（Linux 可跑，§6.4 的结构保证） |

### 13.3 性能

- 协调器：每次按键为整页（≤10）候选各渲染一次注释、一份气泡。片段只是每段几个区间，无样式时
  `Vec::new()` 不分配；模板解析多一个 `$[` 分支。验收：满页 `notify_ui_update` 耗时增幅 < 5%
  （出厂配置，≥3 次取中位）。
- UI：出厂路径早返回，零新增开销；有颜色时每叶子一次 `SetDrawingEffect` × 区间数，alpha 不同才多遍。

### 13.4 前向 / 后向

- 旧引擎读新主题：`roles` 是未知字段，serde 忽略；新 token 无人引用。
- 新引擎读旧主题：无 `roles`，角色全回落正文色。
- 旧 `.app` / 新服务、新 `.app` / 旧服务：§7.6，均退化为单色，不出错。
- 模板里的 `$[…]{}` 在旧引擎上是字面文字（旧 `parse` 只认 `${`）——跨版本回退时用户会看到字面，
  这是新语法的固有代价，文档注明版本号。

## 14 不做的

- 字重 / 字号 / 下划线分段（改度量，§2 非目标）。
- 候选正文（§8.5）、编码栏、状态提示、Toast、菜单。
- 用户级角色色（§9）。
- `[comment.disabled]`、气泡状态态：引擎现在就没有这两个状态，不为本需求新增。
- 内联的 `hover=` 变体：悬停按规则 3 回落已够用；语法里预留了 `key=` 位置。

## 15 分期

| 期 | 内容 | 验收 |
|---|---|---|
| P1 | View `color_runs`（含直立态按格切分）；DirectWrite `draw_runs`（drawing effect + alpha 分组）；CoreText 按区间属性；mock；`parse_hex` 补 `#RGB`；`encode_tooltip_show` 尾段 + Swift 解码与 `TooltipPanel` 着色 | §13.2 P1 三条；Swift 解码新旧两形单测 |
| P2 | `StyledText` / `Span` / `Role` / `TEXT_ROLES`；`InlineColor` 解析；模板引擎 `Color` 节点与构建器；注释与气泡全链路带片段（截断、折行、段名、`plain_lines`）；UI 求色（§6.2，含内联色的状态回落）；标准色 token 入 `_base`（内联写名字才有东西可查） | §13.2 P2；`TEXT_ROLES` 覆盖测试；walker 下钻用例；性能 < 5% |
| P3 | 主题 `roles`（schema / resolve / 状态门控）与角色色求值；出厂零变化对拍；主题编辑器同步（§10） | §13.2 P3；编辑器 `engineParity` 绿、`pnpm bake:theme` 后 `check:base` 绿；靶机人工验判据随 P3 给出 |
| P4 | 设置端预览 RPC、预览行、插入颜色（§11）；文档站（§12） | 设置仓五道闸门绿；新 label 过拼音检索表 |

P1～P3 与设置仓 P4 同版发布（同 `candidate-tooltip-sections.md` 的做法），P1 不单独出包：
单有渲染能力、没有产出片段的来源，用户看不到任何东西。

## 16 已确认（2026-09-27）

1. **变量即角色，主题按角色配色**：变量渲染出的文字自动带同名角色，另有结构角色 `title`、`literal`；
   主题落点为视图节点下的 `roles` 子表，沿用状态 patch（`[comment.roles]`、`[comment.selected.roles]`、
   `[tooltip.roles]`）；出厂主题不配任何角色，外观逐像素不变。
2. **状态回落**：某状态改了节点正文色、而未给某角色在该状态下的颜色 → 该角色与内联色回落到该状态的
   正文色；状态没改正文色 → 沿用常态角色色 / 内联色（§6.3）。
3. **内联语法 `$[颜色]{内容}`**：只管上色、不改可选段语义，可与 `{…}` 互相嵌套；颜色取 `#RGB` /
   `#RRGGBB` / `#RRGGBBAA`、标准色名、亮暗对、可选 `selected=` 变体；非法 / 未知颜色按正文色显示、
   不回显字面，设置页预览提示；`$[` 不构成完整语法时仍为字面文字。

## 17 待确认

1. **气泡里的标准色名先查 `tooltip_<名字>`**（§5.4）。建议采用：`_base` 气泡底色亮暗都是深色，
   为白底调的亮档语义色放进气泡对比度只有 2.3～2.6。备选是不做作用域查找、把语义色取成两种底都凑合的
   中间调（实测最好也只到 3 左右），代价是候选窗里的也跟着变淡。
2. **规则 3 的「改了正文色」按有效色值比较，而非按主题写没写**（§6.3）。这是对已确认规则的落地口径；
   按「写没写」判会让「派生主题把选中注释色写回常态色」的主题在选中态丢掉全部分色。
3. **新增标准色清单与默认值**（§5.4）：`accent_text`（提升到 `_base`）、`success`、`warning`、`error`
   及气泡同名三色。要不要再加（如 `info`、`link`），或换一套色值。
