# 模糊音正确拼音提示

> ⚠️ 配置与命名已被 [pinyin-correction-and-hints.md](pinyin-correction-and-hints.md) 取代（键挪到 `[input.correction.hint]`，角色 `pinyin_hint`、变量 `${correct_pinyin}`）；下文旧名仅作历史记录。
>
> 2026-10-21 设计；分支 `feat/fuzzy-pinyin-hint`。
> 前身：`text-span-colors.md`（分段样式 / 角色）、`candidate-comment-above-line.md`
> （§2 把「逐字 ruby」另立项——本设计是候选**正文**分段的第一个消费者）。

## 1 问题

开了模糊音后，`pinguo` 能出「苹果」，但用户看不出这是模糊音救回来的，也学不到正确读音。
论坛与 GH 反馈里多次要求「打错音时提示正确拼音」。

想要的效果（维护者定）：

```
1.苹(ping)果  2.平果  3.品
    ^^^^^^ 提示色；上屏仍是「苹果」
```

拼音**内嵌在候选正文里、紧跟出错的字**，颜色与正文不同。

## 2 调研结论（2026-10-21）

- 主流里只有百度把它做成明确设置项：PC「模糊音设置 → 显示正确的拼音」，Android「模糊音设置
  → 纠错提示」，均默认关。
- Rime：`spelling_hints` 在注释区显示整串全拼；雾凇 `corrector.lua` 查人工误读表。双拼下
  显示的也是全拼。社区最大的抱怨是**太吵**——读音本来就对的词、故意打的简拼（`nih`→你好）
  也被标注（oh-my-rime#91）。
- 讯飞/搜狗的「智能纠错」是相邻键误触纠错，与模糊音是两套机制，在编码区标红。
- 正文内嵌的形式没查到先例。维护者认为它比注释区更直观，愿意为此增强候选正文的渲染能力。

### 2.1 实测（2026-10-21，真实词库，Coordinator 首页 7 条，59 条输入）

| | A：zh/ch/sh/n-l/an/en/in 全开 | B：只开 in↔ing |
|---|---|---|
| 首页候选被标占比（全标、仅 D2） | 46% | 14% |
| 打对了的输入、首页仍有标注 | 17/20 | 7/20 |
| 拼错时首选被标 | 8/15 | 1/15 |

最吵样例：`xinqing`（打对）首页 6/7 被标（性情、性侵、辛勤…）；`renmin` 5/7。拼错时首选常是
另一个精确命中的词（`pinguo` 首选「拼过」，「苹果」第 2）。

### 2.2 QQ 拼音（官方帮助 skin.qq.pinyin.cn/help/pinyin05.html）

「开启拼写提示后，会在**第一个模糊音候选词**后提示正确的拼写」；配图 `su'lu'ha` → 只有
「输(shu)入(ru)法(fa)」内嵌拼音，其余候选不标；错音提示「摩羯(jié)座」只标错的那个字。

## 3 决策

| # | 决策 | 理由 / 出处 |
|---|---|---|
| D1 | **正文内嵌**：出错的字后面紧跟 `(正确音节)`，用独立颜色 | 维护者定 |
| D2 | **只标真正因模糊规则命中的音节**：用户敲出的音节 ≠ 词库读音，且两者是一对模糊对偶 | 避开 Rime 的「太吵」。读音本来就对的字、简拼、前缀补全、未完成的末音节都**不标** |
| D3 | 全拼、双拼、混输一律显示**全拼**，不带声调，小写 | 双拼码对学读音没帮助；与雾凇一致。声调：本设计的判断（见 §7） |
| D4 | 开关挂在模糊音下：`[schema.pinyin.fuzzy] hint`（bool，默认 `false`）；设置端 label「显示正确拼音」 | 维护者要求默认关；位置与百度一致；与模糊音同一作用域（全局） |
| D5 | 模糊音总开关 `enabled=false` 时 `hint` 不生效 | 没有模糊命中就无可提示 |
| D6 | 范围选项 `[schema.pinyin.fuzzy] hint_scope`：`"first"`（默认）= 整个候选列表里**只标第一个带提示的候选**；`"all"` = 每个带提示的候选都标。单字、词、整句候选一视同仁 | 实测（§2.1）全标太吵；QQ 拼音即「第一个模糊音候选」才标。`first` 不是「只标首选」——`pinguo` 首选是精确命中的「拼过」，第一个模糊候选「苹果」排第 2 |
| D10 | **末音节例外**：用户敲的**最后一个**音节若是正确音节的真前缀（`pin`→ping、`lan`→lang、`min`→ming），该音节不标 | 用户可能只是还没打完；实测这类占配置 B 全部标注的 36/56。非末音节（`pinguo` 的 pin）照标。双拼不适用（两键即完整音节） |
| D7 | **只影响显示**：上屏文字、学习/调频写入的编码、候选宽度以外的排版逻辑不变 | 上屏仍是「苹果」；学习继续用 `learn_code` |
| D8 | 移动端（Android）同样支持，同一配置项 | 维护者要求 |
| D9 | 新角色 `fuzzy_hint` 进 `TEXT_ROLES`，主题可配色；主题未配时回退注释色 | 注释色本就区别于正文色，满足「色调差异」；不强迫现有主题升级 |

## 4 现状（调研时 main = `5c9179ec`）

路径相对 `wind_input/crates/`。

- **配置**：`wind-config/src/config.rs` `PinyinFuzzy`（`enabled` + 13 组对偶），挂在
  `PinyinGlobal.fuzzy`，字段注册 `config_schema.rs:322-335`，出厂 `data/config.toml:343`。
  引擎侧 `wind-engine/src/manager.rs` 转 `FuzzyConfig`。
- **规则与展开**：`wind-engine/src/pinyin/fuzzy.rs`（`INITIAL_GROUPS`/`FINAL_GROUPS`、
  `fuzzy_variants_scored`、`expand_syllables(_masked)`）；模糊拼写进第二棵 trie
  （`syllable.rs` `load_fuzzy_spellings`）。
- **词语查询**：`pinyin/mod.rs` `lookup_with_fuzzy_in`；用户词/临时词 `search_store_fuzzy`。
  候选 `code` = 用户敲的串，模糊命中时 `boundary` 被置 0（**用户的音节切分丢失**）；词库真实
  编码与边界在 `meta.learn_code: Option<(String, u64)>`（`wind-candidate/src/candidate.rs:90`）。
  ⚠️ `learn_code` 不只模糊才有——简拼、双拼全拼降级支路也设它。
- **整句**：`lattice.rs` 生成模糊节点（带 `canon`），`viterbi.rs` `ViterbiResult.pieces`
  每段有 `start/end/syl_mask/canon`；整句候选 `code`+`boundary` = 用户逐字音节，
  `learn_code` = 真实逐字音节，可一一对应；但整句候选不设 `is_fuzzy`，`learn_code` 里也混着简拼段。
- **双拼**：`ShuangpinConverter::convert` 先转全拼再进同一引擎。转换器在
  `use_fuzzy` 下自己会加模糊对偶声母、以及「错音串垫底」（`shuangpin.rs` 约 :413-433），
  且只取第一个结果——这类候选 `is_fuzzy=false`、无 `learn_code`，**是盲区**。
- **混输**：`MixedEngine.secondary` 即递归建出的 `PinyinEngine`，共用全局模糊配置。
- **UI 数据**：`wind-ui-types/src/candidate.rs` `CandidateItem.text: String`（无样式）；
  `comment`/`comment_above` 是 `StyledText`（字节区间分段，带角色或内联色）。
- **渲染**：Windows / macOS / Linux 的候选窗都由 `wind-ui` 绘制（Linux 走共享内存帧，
  Fcitx5 面板只贴图）。注释着色见 `wind-ui/src/candidate_window.rs` 约 :4096 的 `color_runs`。
- **移动端**：`wind-mobile` → `candidate_pull.rs`（`CandidateWindow`、`comments_for` 把
  `StyledText` 压成纯文本）→ UniFFI → `../WindInputAndroid`（`CandidateBarView.kt` 单色
  `commentPaint`；`ExpandedCandidatesWindow.kt` 用 Spannable）。

## 5 设计

### 5.1 引擎：给候选带上「逐音节读音对照」

在候选上新增一份**显示用**数据（建议放 `CandidateMeta`）：

```
fuzzy_hints: Vec<FuzzyHint>   // 空 = 无提示（绝大多数候选）
FuzzyHint { char_index: u16,  // 候选文字里第几个字（按 char 计）之后插提示
            correct: CompactStr }  // 词库读音，全拼小写，如 "ping"
```

只在 `hint` 开启时计算（关时零开销、零分配）。判据（D2）逐音节比较：

- 用户音节 `u` 与词库音节 `c` 不同；
- `u` 是**完整音节**（不是简拼首字母、不是前缀补全、不是末尾未完成的半截）；
- `c ∈ fuzzy_variants(u)`（按当前已开的规则）。三条都满足才产一条提示。

各来源要做的事：

| 来源 | 要补的 |
|---|---|
| 系统/用户/临时词的模糊查询 | 在 `LookupHit` / `search_store_fuzzy` 里记下当时用户的音节切分，与词库编码逐音节比较 |
| 整句（lattice + viterbi） | 用 `pieces`（或 `code`+`boundary` 对 `learn_code`）逐段比较；简拼段按 D2 跳过 |
| 双拼转换器的模糊对偶 / 错音串垫底 | 转换出的音节若来自模糊对偶，要把「用户键对按本方案应读的音节」与「转换后的音节」差异带到候选上（形式由实现者定），并补 `is_fuzzy` |
| 混输 | 拼音侧候选自动继承；码表侧不产提示 |

**多字词的字→音节对应**：一字一音节。遇到音节数 ≠ 字数（儿化、含英文/数字的词条、符号）
时整条不标——宁可漏标，不可错位。

### 5.2 协调器 → UI：候选正文分段

`CandidateItem` 新增显示用正文（建议 `text_styled: StyledText`，空 = 用 `text`）。协调器在
组装候选（`coordinator.rs` 注释组装附近）时，若候选有 `fuzzy_hints`，生成
`苹` + `(ping)`[角色 `fuzzy_hint`] + `果`。`text` 仍是「苹果」，供上屏和一切非显示用途。

- 括号用半角 `(` `)`，与括号内拼音同色（整段 `(ping)` 一个角色）。
- 角色 `fuzzy_hint` 登记进 `TEXT_ROLES`；它不是模板变量，守卫测试
  `every_text_role_is_evaluable_somewhere` 需按「结构角色」处理（同现有结构角色的豁免方式）。

### 5.3 桌面渲染（`wind-ui`，Win/mac/Linux 共用）

- 候选文字的测宽、绘制、截断（`cand_metrics` / `truncate_candidate_text` / `water_fill`）
  改为接受分段文字；角色色查主题，`fuzzy_hint` 未配 → 注释色（D9）。
- 字号：与正文同号（本设计的判断，见 §7）。
- 截断：候选过长需截断时，**整段提示要么完整保留要么整段去掉**，不能截出 `(pi…`。
- 竖排、展开网格、蒙古文竖排布局都要过一遍；golden 渲染测试（`render_golden.rs`）补用例。
- 不受影响：编码区（preedit）、游戏兼容 TSF UIElement / IMM32 候选列表（只给纯文字，
  显示「苹果」）、悬停气泡、右键菜单。

### 5.4 主题

- 主题新增 `fuzzy_hint` 角色色（含高亮态，与注释色的结构一致）。
- 求色（`wind_theme::fuzzy_hint_color`）：状态单列 `text.<态>.roles` 优先；该状态改了正文色时，
  常态提示色对该状态候选项实际底色（逐层合成到窗口底）的对比度 ≥ 3:1 才沿用，对比度不足或底色
  取不到（渐变 / 图片 / 覆盖图 / 窗口底半透明）则回落该状态的注释色；其余沿用 `text.roles`，
  再无则注释色。完整顺序见 text-span-colors.md §5.1。
- 内置主题：至少默认主题给一个与注释色可区分的提示色（具体取色由实现者定，深浅两套都要过对比度）。
- 主题编辑器在独立仓 `WindInputThemeEditor`，需同步该字段（另列任务）。

### 5.5 Android

- `wind-mobile` 的 `CandidateWindow` 增加每个候选的分段信息（文字段 + 角色），经 UniFFI 投影。
  只需投影本功能用到的角色，但结构应能容纳以后的注释分段。
- `../WindInputAndroid`：`CandidateBarView` 与 `ExpandedCandidatesWindow` 绘制分段正文；
  提示色取 Android 主题的对应色，缺省回退注释色。
- Android 自带的 `assets/data/settings_manifest.toml` 加同一开关（过 `check-settings-labels.py`）。

### 5.6 设置端与文档

- 两个配置项：`hint`（bool）与 `hint_scope`（`"first"`/`"all"`）；设置端在「显示正确拼音」下给
  范围下拉「首个模糊候选 / 全部模糊候选」，`hint` 关时禁用。
- 核心：`config.rs` 加字段、`config_schema.rs` 注册、`data/config.toml` 写注释。
- `../wind-setting`：`settings_manifest.toml` 模糊音区加「显示正确拼音」，放在「扩展设置」对话框里
  或总开关下单独一行（由实现者按现有版式定）；label 新字要过拼音检索表守卫。
- 文档站（`WindInputDocs`）模糊音章节补说明与截图。

## 6 非目标

- 键盘误触纠错（相邻键、漏打、颠倒）及其提示。
- 人工误读表（雾凇 corrector 那类「馄饨读 hún tun」）——不是模糊音规则产生的。
- ruby（拼音悬在字上方）排版。（带声调已改为可选，见 §9）
- 改写编码区 / 上屏内容。
- 码表方案的任何提示。

## 7 交给实现者的（本设计的判断，非维护者决定）

- 不带声调（D3）；提示与正文同字号；括号半角。
- 数据结构命名与放置（`CandidateMeta.fuzzy_hints`、`CandidateItem.text_styled`）。
- 默认主题的提示取色。
- 设置项在模糊音区里的具体位置。
- D2 的「当前已开的规则」用 `FuzzyConfig::without_shared_key()` 判：双拼同键对端音节（罚分 0）
  不算打错。出厂布局无同键对（守卫 `builtin_layouts_shared_key_pairs_are_all_excluded`），
  只影响自定义布局。
- D2 的「完整音节」= 非简拼、非前缀补全、非末尾半截，**不要求是合法音节**：全拼 `din`、
  自然码 `dn` 走错音串垫底时同样提示（否则 §8 双拼对偶路径验不过）。
- `char_index` 是引擎文字的下标；协调器按显示文字（`cand_display_text`）换算：逆切分次选只显示
  后段时扣前段字数；简繁转换后字数变了则整条不标（宁漏不错）。
- 临拼（形码方案里的临时拼音）本就跟随 `[schema.pinyin.fuzzy]`，按 overlay 对齐主路的原则同样出提示。
- Android：提示与正文同字号，颜色缺省回退 `Palette.hint`。

## 8 验收

- 全拼：开 `ing↔in`，`pinguo` → 首选显示「苹(ping)果」，上屏「苹果」；`pingguo` → 「苹果」无提示。
- 双拼（小鹤 / 自然码各一）：同一词的模糊输入出提示，显示全拼；转换器模糊对偶路径也出提示。
- 混输：拼音侧候选同上；码表候选无提示。
- 整句：句中只有模糊命中的字带提示。
- 简拼 `pg` → 「苹果」无提示；末音节未打完（`pinggu`）不标末字。
- `hint_scope="first"`：`pinguo` 只有「苹(ping)果」带提示，其后的模糊候选不标；`xinqing`
  （打对）整个列表只有第一个模糊候选带提示；翻页后不再出现提示。`"all"`：每个带提示的候选都标。
- 末音节例外（D10）：开 in↔ing，单输 `pin` → 「平」「凭」等**不标**；`lan`（开 n↔l）→「难(nan)」照标。
- `hint=false` 或 `enabled=false`：显示与现状逐字节一致（golden 不变）。
- 截断：长候选截断时不出现半截提示。
- Android：候选栏与展开面板都显示带色提示；开关在 Android 设置里可切换。

## 9 追加（2026-10-21，对照 GH#230 补齐）

GH#230 楼主的两条诉求首版未覆盖，维护者定补：

### 9.1 带声调（可选）

- 配置 `[schema.pinyin.fuzzy] hint_tone`（bool，默认 `false`）；设置端 label「提示带声调」，**不依赖 `hint`**（它同时作用于 §9.2 变量，`hint` 关着也有用），不设 `enabled_when`。
- 开启后内嵌提示与 §9.2 变量都用带调拼音：`苹(píng)果`。
- 取调复用 `wind-reverse` 的 `toned_pinyin_of(text, Some(&syllables), …)`：用词库音节筛读音表；同音异调（「为」wéi/wèi）
  取读音表里最常用的那个——与现有 `${pinyin}` 同一口径，是数据下界。某字读音表里查不到匹配音节时，该处退回不带调的
  词库音节（宁可少调，不可错音）。
- 只影响显示；`(…)` 宽度变化同样走现有截断规则。

### 9.2 模板变量 `${fuzzy_pinyin}`

- 只在候选**带模糊提示**（`fuzzy_hints` 非空）时有值，否则为空串——可写成 `${code_hint|code_rev}{ ${fuzzy_pinyin}}`。
- 值为该候选**整词的正确拼音**，音节以空格分隔（`zhong guo`），按 `hint_tone` 决定带不带调；字数与音节数对不上时为空。
  音节由引擎随提示一并填进 `meta.fuzzy_syllables`（与内嵌提示同一切法，码无边界时按音节图现切），协调器不另行切分。
- 不受 `hint_scope` 约束（它只管正文内嵌）；用模板的人自己决定放哪。
- 引擎计算条件改为 `enabled && (hint || 任一生效注释模板引用了 fuzzy_pinyin)`：只想在注释里看、不想正文内嵌的人可以关
  `hint`、只写模板。条件值放在 `EngineManager` 持有的共享开关里，注入各拼音引擎与双拼转换器、`convert` 时读取；
  模板引用变化只翻开关、不重建引擎（提示是收尾后处理，不影响候选集合与排序）。
- 角色 `fuzzy_pinyin`（同其他模板变量各有同名角色），登记 `TEXT_ROLES`；出厂 `_base` 的 `[comment.roles]` 给它与
  `text.roles.fuzzy_hint` 同色，主题未配时随注释色。
- 设置端注释模板的变量清单 / 预览值、文档站模板变量表同步。

验收补充：`hint_tone=true` 下 `pinguo` → 「苹(píng)果」；模板含 `${fuzzy_pinyin}`、`hint=false` 时 `zongguo` 的「中国」注释出
`zhōng guó`/`zhong guo`（视 `hint_tone`），正文不内嵌；非模糊候选该变量为空。
