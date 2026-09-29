# 候选注释的「上方注释条」

> 2026-09-29 设计（未实施）。
> 前身：`candidate-comment-layering.md`（注释三层模型，模板是唯一消费点）、
> `theme-capability-extension.md`（新主题字段必须「引擎消费了才算数」）。

## 1 问题

候选注释很长时（拼音 + 拆字 + 其它注释），即便竖排也放不下。现状是模板里写 `\n` 能把注释
撑成多行，但对齐差：

- 一个候选项是 `Row[序号, 文字, 注释]`，`cross(Align::Center)`。多行注释把整行撑高，
  且相对单行的候选文字**垂直居中**——上下各露半行，视觉上像「注释悬空」。
- 注释在 UI 层不截断，只有协调器按 `comment_max_chars` 字数截（`\n` 也占一个字符）。

想要的效果（其他用户基于开源代码已做过）：

```
    pin yin hello world        ← 上方注释条
1.  拼音 拆字和其它注释        ← 序号 + 候选文字 + 右侧注释
```

单个候选项不再局限于单行。

## 2 决策

| # | 决策 | 理由 |
|---|---|---|
| 1 | **只加一个全局布尔开关**，不加任何模板 key | 注释模板有 方案 / 临时状态 / 横排 / 竖排 多个配置位，再加一套 key 要全部对齐。`\n` 本来就在，用它做分隔 |
| 2 | 开关关 = 现状，逐字节不变 | 老用户用 `\n` 的现有行为不动 |
| 3 | 开关开：模板**字面文字**里的**第一个** `\n` 把结果拆成上、下两段 | 上段进上方条，下段留右侧。变量值自带的换行（如注释库词条）不算分隔符，不会误拆 |
| 4 | 主题新增 `views.comment_above`，缺省回退 `views.comment` | 上方条的高度、字号可能与右侧注释不同（通常比候选项低一点） |
| 5 | 上方条左边对齐到**候选文字起点**（序号右侧） | 拼音要悬在文字上方；序号属于主行 |
| 6 | `comment_max_chars` 开关开后含义变「每段」 | 两段互不挤占；关闭时含义不变 |

不做：逐字拼音悬在每个字上方（ruby）。它改的是候选**文字**的排版（每字宽 = max(字宽, 音节宽)，
要动 `cand_metrics`/`truncate_candidate_text`/`water_fill` 并需要字→音节的结构化数据），
与本设计是两件事，另立项；本设计的外层结构不挡它。

## 3 行为

### 3.1 配置

`ui.candidate.comment_above`（bool，默认 `false`），全局一份，不进模式级 / 方案级三态，
不分横竖。三态规则（R3）的适用前提是「各作用域有不同的合理取值」，而这是一个纯排版偏好。

### 3.2 拆分（`wind-coordinator/src/comment.rs`）

- 渲染节点时，`Builder` 记录**第一个来自 `Node::Text` 的 `\n`** 在输出中的位置。
  变量值（`Node::Var`）里的 `\n` 不记。
- 渲染完成后在该位置拆开，**先拆、后各自 trim、再各自按 `max_chars` 截断**。
  先拆后 trim 是为了 `${pinyin}\n${chaizi}` 在 chaizi 为空时仍是「上段 = 拼音、下段空」，
  而不是被尾部 trim 吃掉 `\n` 后让拼音掉到右侧。
- 无字面 `\n` ⇒ 只有下段（右侧），与现状一致。
- 上段空、下段有 ⇒ 视同无上方条。
- 拆分点之后的其它 `\n` 留在下段里，行为同现状（撑高右侧注释）。
- `Coordinator::comment_for` 的返回值从单个 `StyledText` 变为「上段 + 下段」，
  `CandidateItem` 新增 `comment_above: StyledText`（空 = 无）。开关关时该字段恒空，
  且**不做拆分**，字面 `\n` 原样留在 `comment` 里。

⚠️ **写法约束（写进设置页说明与文档站）**：`\n` 要写在可选段 `{…}` **之外**。
写在段内（`${pinyin}{\n${chaizi}}`）时，段为空则 `\n` 一并消失，拼音会落到右侧而非上方。

### 3.3 布局（`wind-ui/src/candidate_window.rs`，`build_tree`）

- 开关关，或该候选无上段：item 结构不变。
- 有上段：item 变为 `Column[上方条叶子, Row[序号, 文字, 右注释]]`。
  选中 / 悬停背景仍由外层 item 容器绘制，包住两行。
- 上方条左缩进 = 序号宽 + 序号与文字的间距（`idx_w + text.margin.left`），
  即候选文字起点。
- **预留高度按页判定**：这一页里只要有任何候选带上段，本页所有 item 都预留上方条高度。
  避免竖排出现「有的行高、有的行矮」的锯齿。占位行 `placeholder_row` 与真实行同构，
  同样预留（现有硬约束）。
- 横排：一页里各 item 用 `Align::End`，使主行底边对齐。
- item 自然宽度 = `max(主行宽, 上方条左缩进 + 上方条宽)`；只有候选**文字**参与
  `water_fill` 截断。上方条自身在 UI 层按可用宽度加 `…`，避免超长上段顶宽窗口
  （与 `comment_max_chars` 叠加，UI 层是兜底）。
- 蒙古文直立 / 旋转态：不支持，回退为开关关时的表现（含不拆分）。
- 悬停气泡 tooltip 不受影响。

### 3.4 主题：`views.comment_above`（`wind-theme`）

新增一个 `ViewNode`，与 `views.comment` 同型（margin / padding / 字体 / 字号偏移 / 颜色 /
`roles` / `selected` / `hover`）。

**回退规则**（`resolve` 阶段做，渲染只读解析后的值）：

| 字段 | 未写时 |
|---|---|
| `font_family` / `font_weight` / `font_size` / `color` | 继承 `views.comment` |
| `roles`（含选中、悬停各态） | 继承 `[comment.roles]` |
| `selected` / `hover` 状态 patch | 继承 `views.comment` 对应态 |
| `margin` / `padding` | **不继承**，默认 0 |

margin / padding 不继承的原因：基础主题里 `comment.margin.left = 6` 是「文字与**右侧**注释的间距」，
右侧专用。上方条若继承会被推歪 6dp。上方条与主行之间的纵向间距靠自己的 `padding.bottom`
（或 `margin.bottom`）调。

内联色 `$[色]{…}` 与 `[comment.roles]` 对上方条同样生效——不同注释配不同颜色不需要新工作。

## 4 跨仓与同步

- 主题编辑器（`WindInputThemeEditor`，独立仓）：字段在引擎渲染**消费之后**再同步
  （`types` / `resolve` / `defaultViews` / `candidateBox` 预览 / `engineParity.test.ts`）。
- 设置端（`wind-setting`）：`settings_manifest.toml` 新增开关一项，同步
  `capabilities.snapshot.json`、`mockdata/config.json`；模板输入框旁补「`\n` 拆上方条」的说明。
- 文档站（`WindInputDocs`）：`settings/appearance/candidate-comment.mdx` 补开关、写法约束、
  主题字段。
- 出厂 `data/config.toml` 补键与注释；`docs/config-key-migration.md` 无需迁移（新增键）。

## 5 测试

补现有缺口——目前**没有**任何候选窗测试覆盖「注释含 `\n`」。

1. `comment.rs`：字面 `\n` 拆分；变量值含 `\n` 不拆；`\n` 后变量为空时上段仍在；
   `\n` 在可选段内且段为空时无上方条；开关关时不拆；各段独立截断。
2. `candidate_window.rs`：有 / 无上段的 item 高度；按页预留（一页里混合有无）；
   上方条左缩进 = 文字起点；`placeholder_row` 同构；横排主行对齐；直立态回退；
   超长上段被 UI 层省略。
3. `render_golden.rs`：新增带上方条的金图（竖排 + 横排 + 选中态）。
4. `wind-theme`：`comment_above` 回退规则（继承与不继承两组）、`roles` 各态。

## 6 分期

1. **P1（主仓）**：开关、拆分、`CandidateItem` 字段、主题节点、`build_tree`、测试。
2. **P2**：设置端、主题编辑器、文档站同步。
3. **另立项**：ruby 式逐字拼音。
