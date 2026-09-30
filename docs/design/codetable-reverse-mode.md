# 码表反查模式 / 通配仅单字 / 反查含未启用扩展词库（设计）

> **状态：P1–P3 已实施**（提交见 git log --grep 反查 / 通配）。实施计划见 `codetable-reverse-mode-plan.md`；实施中偏离见 §8。承接 `codetable-wildcard.md`（行内通配，已实施）。
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
  没开关或没用到时零开销。（实施后改为：开关开时由后台线程预热，见 §8.4。）
- 合并规则：影子层结果与已启用层按 `(text, code)` 去重（沿用 Composite 语义），**已启用的排前**，影子层命中排后，
  排序仍走 `cmp_exact_first` + `base_sort`。
- 注释反查：`ReverseIndex` 增加「含未启用库」的变体，缓存 key 带开关位（避免与常规索引串文件）；
  仍只用 `reverse_index_if_ready`，未建好则后台建、本次不显示（沿用防卡死约束）。
  （实施后改为：变体未就绪时回退常规索引，先显示启用集里的码，见 §8.4。）
- 失效：`set_dict_enabled_live`、方案重载、开关切换时清影子层与含未启用的反查索引。
- 依赖（混输）：混输方案取主方案的开关；扇出与既有 `set_dict_enabled_live` 一致。

### 4.3 风险

- 内存：影子层是 mmap，Private 不计；但反查索引含全部库时体积增大，需实测（见 memory「Windows 内存看 Private」）。
- 词库文件缺失：跳过并 warn，不阻塞。
- 首次加载的卡顿：影子层加载要逐库 mmap、缺缓存时现建 wdat，大扩展库上是秒级读盘；若留在首次通配的按键线程上
  （此时持协调器 state 锁）会让整机顿住。故开关开时改由后台预热（§8.4）；预热没完成时首次通配仍可能等它。
- 孤儿变体文件：开关关掉（或未启用库全被启用）后，`<key>/<key>.with_disabled.wridx` 与其 `.fp` 留在缓存目录不再被读，
  也不会被自动删；下次开开关时按指纹复用或重建。「重建缓存」（`purge_cache_files` 按 `wridx` 扩展名）会一并清掉。
  暂不做自动清理：只占磁盘、不占内存，量级与常规索引同档。
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

## 8. 实施后偏离

实施时读码与执行中发现的、改变了上文字面的点。上文 spec 原文不改，以本节为准。

### 8.1 实施前读码落定

1. **A「截断之前过滤」用加倍重取实现**：词库层 `DictManager::search_pattern` 自己按 `limit` 分档截断，引擎拿到的已是截断后的列表。
   不改 wind-dict 的 `DictLayer` 接口，在引擎 `wildcard_query` 内加倍重取：过滤后不够 `limit` 且词库未取尽时 `fetch` ×2 重查，直到够数 / 取尽 / 到硬上限
   `WILDCARD_RESULT_LIMIT`。与生僻字模式「准入下推 + 不足则加大重取」同构；仅单字关时只查一轮，与原实现逐条相同。
   验收串用 `azzz`（`a???` 等长 4066 条、单字 1342、权重前 100 条里只有 38 个单字），不用 `azz`（`a??` 318 条全是单字，测不出截断先后）。
2. **命名**：本仓「shadow」已专指候选调整，spec 的「影子层」在代码里叫 `DisabledDictLayers`（`codetable/disabled_dicts.rs`），候选标记叫 `Candidate::from_disabled_dict`。
3. **「已启用排前」的落点**：协调器 `build_candidates` 会用 `candidate_display_order` 把引擎结果整体重排，只在引擎内排好会被推翻。
   新增布尔键 `from_disabled_dict`（false 在前），同时插进引擎通配排序与 `candidate_display_order`，位置在 `cmp_exact_first` 之后：等长 / 更长两档不变，档内先启用后未启用。
4. **失效点**：禁用走「引擎摘层 + 返回 true、不重建」（返回 false 会触发重建，曾让已删词复活）。影子层记住本方案全部扩展库来源与当前启用集，
   `set_dict_enabled(id, false)` 摘层后调 `mark_disabled(id)`；启用照旧整体失效重建。
5. **注释反查的范围**：变体索引只供候选注释的 `code_rev` / `code` / `code_rev_all` / `code_all`；加词查重、辅助码来源、悬停 `[编码]`、联想、单字全码表一律用启用集
   （查重以启用集为准，否则只在未启用库里有的码+词会被误判为已存在）。方案没有未启用扩展库时退化为常规索引，不另建文件。
6. **反查模式的实现形态**：沿用生僻字先例，`ModeKind::Reverse` 是 special 的参数变体，复用 `special_buffer` 与 `handle_special_key`；候选另写 `build_reverse_candidates`
   （`convert_reverse` + 常用字判定 + `apply_filter`），并给翻页扩充、末页放宽、放宽失效各补 Reverse 分支。
7. **引擎入口**：`convert_wildcard` 首行受主开关约束，故新增 `Engine::convert_reverse` / `reverse_wildcard_key`，与 `convert_wildcard` 共用内核 `wildcard_query`；混输只代理主码表，不调拼音。
8. **不加 `trigger_keys` 与占位组合区配置**：进入键走既有 `key_actions`（动词 `reverse`）与方案页「z 键引导功能」；直达热键的占位组合区固定按开（同生僻字出厂值）。
   `z_key_action = "reverse"` 需在 `try_z_fallback` 补一臂，否则五笔出厂 `zz*` 短语让首键 z 恒让位、永远进不去。
9. **词频与候选调整**：模式内上屏的记账码取候选全码；不做词频重排、不吃候选调整（通配串不是任何码位）。
10. **退出残留**：special 族原本不写 `has_more` / `candidate_limit` / `scope_relaxed`，Reverse 会写；`exit_special_mode` 一并复位（对 Special / RareChar 是空操作）。
11. **§4.1 措辞**：「`default=false` 即扩展库」以 `DictSpec::is_enabled()` 为准（看 `enabled` / `default` / `default_enabled`）。
12. **`prefix-hijack-modes.md` §3 清单**的函数名已更正为现名。

### 8.2 执行中新增的裁决

- **首批固定 100**：反查首批恒为 100 条，混输方案也不套 300，因为反查不含拼音，没有拼音那一路要分配额度。
- **索引串行构建**：常规索引与变体索引串行构建（避免首次开开关时两份并行、内存峰值翻倍）；常规索引被崩溃保护跳过时，变体也放弃。
- **反查里的右键候选调整只留复制**：查询串不是码位，调整写下去会落到主路径并重建、清空候选。生僻字模式同款问题另立项。
- **通配键体检只报导航真正吃掉的键**：翻页 / 高亮 / 取消 / 上屏 / 命令 / 单字输入；选词、以词定字、独立辅助码键不吃通配键，不报。
- **修饰键绑定**：反查绑修饰键（如 `rshift = "reverse"`）在拼音方案里不被吞：让位判定放在「取键字符」之后，无字符的修饰键不进让位分支，照常落回全局链。
- **失焦复位**：`reset_exclusive_modes` 补复位 `scope_relaxed`，放宽后失焦再打字不处于放宽态（主路径与临拼此前也有同样缺口）。
- **回车上屏查询串原文**：回车上屏的是缓冲里的查询串（含通配键），不是某个候选，也不是编码；空格 / 数字才上屏候选。

### 8.3 已知遗留

- 通配键与导航键冲突的体检是静态判定，只看配置，不看运行时动态改键。
- `special_id = 0` 与快符方案 0 撞车（继承自生僻字模式）。
- 反查候选管线没有 `(text, code)` 去重，也没有「单字输入」开关的处理。
- 「通配与反查含扩展词库」开着时，热禁用扩展库后通配会重新读入该库（重新 mmap）。
- 变体索引文件在方案不再有未启用库后成为孤儿文件，不会自动清理。
- 注释反查的方案来源不一致（改前已有，另立项）：后台预热取 `code_source_schema`，而 `code` / `code_rev` 取值读的是
  `primary_codetable`，活跃方案不是主码表时两者可能不同，预热的那份与读取的那份对不上。
- 反查键在拼音等不支持的方案里「让位」的那条分支只影响 debug 日志，行为上与没绑键相同。
- 绑了 `reverse` 但 `input.reverse.enabled = false` 时，组合键仍会注册全局热键槽位（按下被吞、不进入），与临时拼音同款。
- 生僻字模式同款问题（需另立项）：右键调整候选后主路径重建、清空候选。反查模式已把右键菜单收成只有「复制」规避。

### 8.4 最终审查后的修正

- **影子层后台预热**：开关开时，影子层不再等首次通配在按键线程上加载，改由 `EngineManager` 在后台线程预热：
  活跃方案的引擎（重）建好时、活跃方案变更时、热禁用扩展库之后各触发一次（`warm_disabled_dicts_async`）。
  开关关或方案无扩展库时引擎不挂影子层，一查即返回、不起线程；已建好或正在建（`try_lock` 判）不重复起。
  预热与查询共用影子层内部那把锁，按键线程撞上正在进行的预热就等它建完、直接复用，最坏情形等同改前；
  失败的库照旧 warn 跳过。只预热活跃方案，非活跃方案切过去时再热。§4.2「首次用到才加载」据此改为「开关开即后台加载」。
- **注释反查范围免读盘**：`comment_reverse_scope` 先看已加载的码表引擎——挂着影子层才可能是变体范围；没挂说明构建时折叠出的
  开关（全局 + 方案级覆盖同一口径）是关的或方案没有扩展库，直接判常规、不读方案文件、不写缓存。引擎未加载（或是混输）时
  照旧读盘判定并缓存。取舍：引擎与范围缓存同生命周期（`invalidate_schema` / `reload_from_config` 一并清），不会拿旧引擎判新配置。
- **变体未就绪时回退常规索引**：候选注释的 `code` / `code_rev` / `code_all` / `code_rev_all` 在变体还没建好、或被崩溃保护跳过时，
  改用常规索引（若它已就绪），先显示启用集里的码，变体就绪后自然升级；开关关时本就只用常规索引，路径不变。
  因为取到的不再是 `None`，协调器补建变体的信号改为「每次取值后都调 `warm_comment_reverse_index`」（已就绪 / 在建 / 被跳过时立即返回）。
