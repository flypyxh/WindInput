# 辅助码来源：方案引用 + 码表文件并存

状态：设计（2026-09-27），待确认。
前置：`docs/design/aux-code-settings-ui.md`（P1 / P3 已实施，本文取代其 P2 的「码表选择」一条）。

## 1. 问题

辅助码的来源现在只有一种：`[engine.aux_code].files`，指向 `schemas/aux_code/` 下的 `字=码` 文本。
这带来三件事：

1. **不能拿一个输入方案当辅助码。** 「拼音 + 五笔前两码」是常见用法，五笔的编码已经在
   `wubi86` 方案的 rime 词库里，却要另转一份 txt 才能用。用户反馈正是这一条。
2. **笔画表多转了一道。** `stroke.txt` 由 `gen_aux_code` 从 rime-stroke 的 `stroke.dict.yaml`
   剥出来；上游本来就是一个 rime 输入方案，转成 txt 后反而既不能当方案用，也不能跟着方案机制走。
3. **设置端不能选。** 用哪张表只能手改方案文件（设计文档 P2 未做）。

## 2. 决定

**两种来源并存**：

| 来源 | 写法 | 给谁用 |
|---|---|---|
| 方案引用 | `schema = "wubi86"` | 完整的码表输入方案（五笔、笔画……），编码就是该方案词库里单字的编码 |
| 码表文件 | `files = ["aux_code/flypy_full.txt"]` | 专用辅助码表（小鹤、自然码的 2 码首末形码）。这类表单独当输入方案没有意义，且本身就是 rime-lua-aux-code 的 txt 格式，零转换引入 |

不把 txt 全部改成方案：小鹤 / 自然码形码只有 2 码、重码极高，做成方案也只能隐藏；
保留 txt 反而与 rime 生态兼容。

笔画表改为**码表方案**（第 2 期），全拼默认引用它。

## 3. 配置形态

```toml
[engine.aux_code]
schema = "stroke"                        # 新增：引用一个码表方案（Option<String>）
files  = ["aux_code/flypy_full.txt"]     # 保留
enabled = true                           # 不变
max_phrase_len = 4                       # 不变
```

- **合并顺序**：`schema` 在前、`files` 按序在后，走 `AuxCodeTable::merge` 现有的「先出现 = 高优」语义。
  两者都不写 = 本方案没有辅助码表（与今天 `files = []` 等价）。
- **覆盖**：`schema_overrides/{id}.toml` 同段同名覆盖。`schema` 是标量，覆盖即替换；`files` 是数组，
  `merge_toml` 整体替换。设置端从「文件」切到「方案」时要同时写 `files = []`，否则方案文件里的
  基线 `files` 仍会合并进来。
- **`has_aux_code_table` 判据**：改为「`schema` 非空，或 `files` 非空」。core 与设置端各一处，同改。
- **`enabled` 仍是总闸。** `schema` 非空同样不代表开启（沿用 `AuxCodeSpec` 文档里的判据）。

### 3.1 可被引用的方案

- 引擎类型为 `codetable`；拼音、混输、英文不行（它们的「编码」不是字形码）；
- 不能引用自己；
- 不要求在 `schema.available`（切换列表）里，也不看 `hidden`。被引用 ≠ 被启用。
- 编码必须**只含字母**：辅助码模式只收字母键（`handle_aux_code_key`）。编码含 `;`、数字等的方案，
  设置端不列出；手写进配置的，建表时跳过这类编码并 warn 一次。

### 3.2 从方案建表

`EngineManager` 新增 `aux_code_table_from_schema(id) -> Option<AuxCodeTable>`：

1. `read_schema(id)` 取方案（含 override），`load_dicts_individually` 取**启用的**系统词库。
   与反查索引同一入口（`build_reverse_index_for`），词库启用状态与该方案自身一致。
2. `for_each_entry` 遍历，只收**单字**条目（`text.chars().count() == 1`），保留该字全部编码。
   五笔简码是全码前缀，一并收进来不影响前缀匹配；收全部编码对「简码不是全码前缀」的方案也成立
   （`any_code_starts_with` 按任一码判）。
3. 不含用户词库：辅助码要的是字形真值，用户造的单字码不该改变筛选结果。

构建成本：遍历词库是毫秒到百毫秒级；但**被引用方案的词库第一次用时要从 `.dict.yaml` 编译成
wdat，那是秒级**。故建表只能在后台做，见 §4。

## 4. 加载与失效

今天的加载点是 `enter_aux_code` → `ensure_aux_code_table`，**在按键线程上**同步读 txt。
txt 很小所以没出事；方案来源不能照搬。

**改为**：

- **后台预热**：辅助码生效开启（折叠后 `enabled = true`）且有来源时，在下列时机派后台线程建表，
  结果放进 `aux_code_table`：启动预热（`prewarm_indexes`）、切方案、`schema.saveConfig` /
  `resetConfig`、全局配置重载。复用 `reverse-index-build` 那套 single-flight 去重。
- **按键线程只读不建**：`enter_aux_code` 发现表未就绪时不进入、不吞键，弹一次提示
  「辅助码表加载中」（每次失效后最多一次）。txt 来源仍可同步加载，保持今天的行为不退化。
- **缓存键**：缓存记住「这张表是从哪组来源建的」（`schema` id + 解析后的 `files` 路径）。
  来源变了才重建，不再只靠「切方案清空」。这同时修掉现存 bug：
  **`schema.saveConfig` 后不清缓存**（`refresh_schema_derived_config` 不调
  `invalidate_aux_code_table`），改了码表要切一次方案才生效。
- **被引用方案的词库变了**（改启用词库、重建缓存）：`schema.invalidate` / `rebuildCache`
  对该 id 生效时，若它正被当作辅助码来源，同样失效重建。

## 5. 设置端

辅助码卡片（`schema_manager.rs` 方案设置里）在「本方案启用辅助码」之后加一行 **「辅助码来源」**，
按 R8.1 勾选行：

- 不勾：只读显示「跟随方案（笔画）」——即方案文件声明的来源；多个来源用「+」连接；
  声明的来源不存在时加「（未安装）」。写回：`schema` 与 `files` 两键都写 `null`（删键）。
- 勾上：下拉，分两组——「输入方案」（§3.1 的可选方案，显示方案名）、「辅助码表」（`schemas/aux_code/*.txt`，
  显示 `# name:` 头，缺头用文件名）。选方案写 `schema = id, files = []`；选码表写
  `schema = null, files = [path]`（`null` 删键即回落方案基线，基线若是 `schema` 需写空串 `""` 屏蔽，
  见 §7 待定 1）。
- 用勾选行而不是 R8.2 下拉：可选项是「所有码表方案 + 所有 txt」，会超过 6 项；也与卡片上
  另两行形态一致。
- 「全部恢复跟随」一并清这一行；卡片摘要「已自定义 N 项」计入它。
- 卡片出现条件随 §3 的 `has_aux_code_table` 一起改。

新 RPC **`schema.auxCodeSources`**（主仓 wind-webdata），返回：

```json
{
  "schemas": [{ "id": "wubi86", "name": "五笔" }, { "id": "stroke", "name": "笔画" }],
  "files":   [{ "path": "aux_code/flypy_full.txt", "label": "小鹤" }]
}
```

`files` 用 `list_schema_resource_dir(data_dir, "aux_code", ".txt")`，与注释词库来源
（`web_comment_sources`）同一做法；`schemas` 取全部已安装方案（含 hidden、不看 available）
中符合 §3.1 的，排除请求方自己（参数 `id`）。设置端 mock（`rpc.rs`）同步补上。

## 6. 笔画改为码表方案（第 2 期）

- `gen_aux_code`（或新增 `gen_stroke_schema`）从 `.cache/aux-code/` 的 rime-stroke 原档产出
  `schemas/stroke/stroke.dict.yaml` 与 `schemas/stroke.schema.toml`（`type = "codetable"`、
  名称「笔画」）。**字集裁剪规则不变**（GB 18030-2000 ∪ 通用规范汉字表 ∪ 〇），
  既保持辅助码表规模，也避免这份方案收进 11 万字。许可同今天：构建期下载、不入库（NOTICE.md 更新措辞）。
- 不进出厂 `schema.available`：方案管理里能看到、能启用，默认不在切换列表。
- 全拼 `pinyin.schema.toml` 改为 `schema = "stroke"`，删掉 `files`。
- **兼容**：用户 override 里已写的 `files = ["aux_code/stroke.txt"]` 不能失效——继续生成
  `stroke.txt`（它只是同一数据的另一种排版），不做迁移。

## 7. 待定

1. **「选码表」如何屏蔽方案基线里的 `schema`。** 覆盖层写 `null` 等于删键，删键会回落到方案
   文件的 `schema = "stroke"`，于是选了小鹤仍会合并笔画。候选：(a) 空串 `schema = ""` 表示
   「不引用」，建表时视为无；(b) 把两键收成一个 `source` 列表。倾向 (a)：不改已有 `files` 的形态，
   代价是 `""` 成了 R3 意义下的哨兵，需要在 `AuxCodeSpec` 文档里写明。
2. **编码含非字母的方案**：设置端直接不列，还是列出但置灰并注明原因？倾向不列（与「只列能用的」一致），
   手写的由 core warn。

## 8. 分期

| 期 | 内容 | 仓 |
|---|---|---|
| 1 | `AuxCodeSpec.schema`、从方案建表、后台预热、缓存键与失效（含 saveConfig 不失效的 bug）、`schema.auxCodeSources`、`has_aux_code_table` 判据 | 主仓 |
| 2 | 笔画码表方案、全拼默认改引用、NOTICE | 主仓（工具 + 数据） |
| 3 | 「辅助码来源」勾选行、mock、渲染与写回测试 | wind-setting |
| 4 | 文档：方案配置页 `[engine.aux_code].schema`；新增「用五笔 / 笔画方案作拼音辅助码」用法 | 文档站 |

触发键与分隔符冲突警示（原 P2 第 6 条）不在本文范围，另立。

## 9. 验收

- 核心：全拼引用 `wubi86` 作辅助码，输入 `gong` + 触发键 + `a`，「工」「攻」保留、「公」滤掉；
  引用不存在的方案不崩、warn 一次、不进入；表未就绪时不进入且不吞键；
  `schema.saveConfig` 改来源后下一次进入即用新表（先写这条，验证它在修复前失败）。
- 设置端：勾选 / 取消 / 切换方案与码表三种写回；跟随档显示方案基线与「（未安装）」；「全部恢复跟随」清这一行。
- 第 2 期：`stroke` 方案可单独启用打字；全拼默认辅助码行为与今天逐字一致（同一字集、同一编码）。
