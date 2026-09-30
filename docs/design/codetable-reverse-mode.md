# 码表反查模式 / 通配仅单字 / 反查含未启用扩展词库（设计）

> **状态：P1（A 通配仅单字）已实施；P2、P3 待实施。** 实施计划见 `codetable-reverse-mode-plan.md`。承接 `codetable-wildcard.md`（行内通配，已实施）。
> 该文 §8 把「专门的通配/反查模式（首位也想通配）」留待后续，本文即是。

## 1. 范围

三项互相独立、可分期落地：

| # | 项 | 一句话 |
|---|---|---|
| A | 通配仅单字 | 方案级开关，通配结果只留单字 |
| B | 独立反查模式 | 用户自绑键进入；模式内输入本方案编码（含通配，**首位也可通配**），出候选 |
| C | 反查含未启用扩展词库 | 方案级开关；通配 + 反查模式 + 注释反查都能查到 `is_enabled()==false` 的扩展库 |

用户已拍板的决定：
- B 的语义是「输入**本方案编码**（含通配）→ 出候选」，不是拼音反查（拼音反查沿用现有临时拼音）。
- B 的进入方式：新增 `BoundAction` 动词，用户自绑键，出厂**不绑任何键、默认关**。
- A 的形态：方案级独立开关，与 `wildcard` 主开关正交，不改 `wildcard` 的布尔契约。
- C 的范围：方案级**一个**开关，覆盖通配 + 反查模式 + 注释反查；普通打字候选不受影响。

## 2. A：通配仅单字

- 配置：`schema.codetable.wildcard_single_only`（bool，默认 `false`）。
- 作用面：行内通配、B 的反查模式（两处都走 `convert_wildcard`）。
- 判据：候选 `text` 恰好 1 个字素簇（`wind_candidate::single_markable_char` / UAX #29，**禁用 `chars().count()`**，见 issue #83）。
- 时机：在引擎 `convert_wildcard` 内、**截断上限之前**过滤，否则首批 100 条被词组占满、单字不足。
  `WILDCARD_RESULT_LIMIT`/首批上限对过滤后的数量计。
- 混输方案：只作用于通配那一侧（`merge_wildcard` 的 wildcard 分支），字面 `convert` 与拼音候选不动。
- 分期无依赖，最先落地。

## 3. B：独立反查模式

### 3.1 入口

- 新增 `BoundAction::Reverse`（键名 `reverse`）与 `ModeKind::Reverse`，按 `prefix-hijack-modes.md` §3 清单接线。
- 触发键通过 `keys.key_actions` / 方案 `[key_actions]` 绑定（如 `semicolon = "reverse"`）；**出厂无绑定**，
  模式开关 `input.reverse.enabled`（默认 `false`）。
- 只在码表方案与混输方案（取主方案）生效；其他方案 `bound_action_yield_reason` 让位。
- 把新动词补进 `is_any_mode_trigger` 与 `code_char_conflicts` 的 owners 链（既有规约：新增引导键类动词必须同步）。
  这样 §3.3 的首键冲突体检、行内通配的让位判据自动认得它。

### 3.2 模式内行为

- 缓冲区是**本方案编码**；模式内通配键**在任何位置（含首位）都是通配**，且**不受 `schema.codetable.wildcard` 主开关约束**
  （用户显式进了反查模式，主开关只管「行内」）。通配键取 `schema.codetable.wildcard_key`（默认 `z`）；
  若主开关关且键未配，模式内固定用 `z`，符号键方案用户自行配置 `wildcard_key`。
- 查询走 `convert_wildcard`：等长 + 既有前缀补全、不含短语、不自动上屏（沿用 §3.1/§5）、
  A 的单字过滤、§11 的整组常用字过滤与翻页扩充、候选注释显示完整编码。
- **不做混输拼音调度**：模式内只查主码表（字面 + 通配），拼音不参与。
  理由：反查是「查编码」，拼音侧只会稀释结果；进入前的混输调度（§10）不受影响。
- 退出：Esc / 提交 / 清空，同其他 overlay；候选布局沿用 `mode-candidate-layout.md`（可选 `input.reverse.candidate_layout`，默认跟随）。
- 模式内选中一个候选：上屏该候选（与行内通配一致），**不**上屏编码。

### 3.3 与首位让位规则的关系

行内通配的首位让位规则不变。B 是它的「出口」：想在首位通配就进反查模式。
反查触发键本身受既有首键冲突处理（触发键为字母且是方案首码时让位，符号键不让）。

## 4. C：反查含未启用扩展词库

### 4.1 现状（调研）

- 扩展词库 = `[[dictionaries]]` 里 `default=false` 且 path 非空者；`DictSpec::is_enabled()`（schema.rs:704）为假即「未启用」。
- 未启用库**不读盘、不建缓存、不 mmap**（manager.rs `load_codetable_layers`）；无「按需临时加载」的既有机制。
- 通配走 `DictLayer::search_pattern`，只遍历已挂层；注释反查走 `ReverseIndex`（`.wridx`，由 `enabled_dict_specs` 选库构建）。

### 4.2 设计

- 配置：`schema.codetable.lookup_disabled_dicts`（bool，默认 `false`）。
- 新增引擎内的**影子层**（shadow layers）：与 `enabled` 层分开，仅由「通配 / 反查模式 / 注释反查」使用，
  **普通 `convert` 不看它**（这是「打字候选不受影响」的保证）。
- 影子层**首次用到才加载**（复用 `CachedDict::load_at_with`、`reader_pool` 的 `Weak` 共享 mmap、`resolve_dict_file`/`cache_path`）；
  没开关或没用到时零开销。
- 合并规则：影子层结果与已启用层按 `(text, code)` 去重（沿用 Composite 语义），**已启用的排前**，影子层命中排后，
  排序仍走 `cmp_exact_first` + `base_sort`。
- 注释反查：`ReverseIndex` 增加「含未启用库」的变体，缓存 key 带开关位（避免与常规索引串文件）；
  仍只用 `reverse_index_if_ready`，未建好则后台建、本次不显示（沿用防卡死约束）。
- 失效：`set_dict_enabled_live`、方案重载、开关切换时清影子层与含未启用的反查索引。
- 依赖（混输）：混输方案取主方案的开关；扇出与既有 `set_dict_enabled_live` 一致。

### 4.3 风险

- 内存：影子层是 mmap，Private 不计；但反查索引含全部库时体积增大，需实测（见 memory「Windows 内存看 Private」）。
- 词库文件缺失：跳过并 warn，不阻塞。
- 用户在设置页看到的「启用」语义不变；开关文案要写明「仅用于通配与反查，不影响普通候选」。

## 5. 设置与文档

- wind-config：`wildcard_single_only`、`lookup_disabled_dicts`（均 schema.codetable 下）、`input.reverse.{enabled,candidate_layout}`、`BoundAction::Reverse`。
- wind-setting：manifest 三处新项（`dev.sh sg/st` 生成并跑测试）；注意 label 会撞拼音检索表（memory）。
- WindInputDocs：`codetable.mdx` 通配输入节补 A/C；新增反查模式说明；`pnpm lint`。
- `codetable-wildcard.md` 顶部范围声明与 §8 加指向本文的一句。
- 测试：真实词库（wubi86 / wubi86_pinyin，`wubi86_xzqy` 为未启用样本）验收：
  A（`azz` 仅单字）、B（首位通配、Esc 退出、触发键冲突让位）、C（开关前后 `xzqy` 内字条可见性、普通候选不变）。

## 6. 分期

1. **P1 = A**：最小，先合。
2. **P2 = C**：架构改动（影子层 + 索引变体）。
3. **P3 = B**：新模式；依赖 A、C 已在（模式内直接受二者开关影响）。

## 7. 已做的裁决（可推翻）

| 裁决 | 理由 | 错了的代价 |
|---|---|---|
| 反查模式内不参与拼音 | 反查是查编码 | 多加一路调度，可后补 |
| 反查模式内通配不受主开关约束 | 模式本身即显式意图 | 改成受约束只是加一个判断 |
| 影子层「已启用排前」 | 不让扩展库冲掉常用结果 | 仅调排序 |
| `lookup_disabled_dicts` 不改普通候选 | 用户只要求反查/通配 | 需要时另立开关 |
