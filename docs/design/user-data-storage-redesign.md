# 用户数据存储重设计：文本真源、快照日志引擎与 redb 退役

状态：设计定稿（经两轮独立审查修订）。日期：2026-10-14。2026-10-15 P5 修订：用户目录装下全部数据（§2.1、§2.4、§2.5、§7.4）。

本文件是实施规划的依据。实现时若与本文冲突，先改本文再改代码。

## 0 背景、目标与决策

### 0.1 现状

- 系统词库：yaml → wdat（双数组 trie）整文件构建，mmap 零拷贝查询，`.fp` 内容指纹判新鲜。只适合静态数据，源小改要整库重建（见 `docs/architecture/rime-dict-loading.md`）。
- 用户数据：全部在服务进程独占的 `userdata.redb`，16 张表。按键时直接打 redb range；所有读写串在 `Store` 的一把 Mutex 上；每次上屏一次同步提交并 fsync；简拼靠两张手工维护的倒排表；读缓存只涨不落（19 万词约 41 MB），靠关库重开回收；按词查码、联想两份内存索引靠代次轮询全表重建。
- 用户数据不可文本化、不可分文件；`.wdict.yaml` 只是导出快照。

### 0.2 目标

1. 用户策划的数据（主动造词、导入词、短语、Shadow、小型覆盖表）以文本文件为唯一真源，任意编辑器可改。
2. 机器学到的数据（临时词、草稿、计数、联想历史、英文学词）走机器格式的运行时库，按键路径写入廉价、读无锁。
3. 文本 ↔ 查询结构的转换增量化：改一个文件只重建一个文件的索引。
4. 日志按设备分文件、带序号，为多设备同步预留数据面。
5. **最终去掉 redb 依赖**，`wind-store` 不再有嵌入式数据库。

非目标：文件监视（不加 `notify`，接口留口）；同步协议本身；系统词库改造（可沿同一思路另立提案）。

### 0.3 决策记录

| 决策点 | 结论 |
|---|---|
| 文本真源边界 | 主动造词 / 导入词 + 短语 + Shadow + 小型覆盖表（快捷格式、常用字、字符集用户层） |
| 运行时库 | 临时词、草稿、计数、联想历史、英文学词；自建「快照 + 日志 + 增量表」引擎，与文本层共用 |
| 文本格式 | 三类词库文件以 Rime `.dict.yaml` 为基底，扩展放头部 `wind:` 键；覆盖表用各自已有的交换格式 |
| 运行时写回 | 按设备二进制日志 + 增量表 + 空闲折回；日志不给人看 |
| 变更感知 | 不加文件监视；`dict.reload` IPC；启动 / 切方案 / 配置重载时快速扫描 |
| 低频小表 | 原子整文件改写，不进引擎 |
| redb | 分阶段退役，最终移除依赖 |
| 数据位置（P5 修订） | 用户目录装下全部数据：学到的数据也以 `data/<schema>/` 下的表格文本为真源，引擎快照降为可重建的加速结构；补全、统计进 `data/`（§2.1、§7.4） |
| 文件格式（P5） | TOML / 表格文本 / 出厂层同格式的 YAML，不用 JSON，不发明二进制源格式；格式异常统一策略（§2.4、§2.5） |

### 0.4 审查修订摘要

初稿经性能审查与稳定性审查各一轮（独立 agent，对照代码）。初稿的三个错误前提已改：

- 初稿把临时词、草稿也放进文本真源，导致折回频率跟打字走、折回目标是最大的文件、delta 守不住上限、计数没有去处、草稿层优先级坍缩。本稿把机器学到的数据收回运行时库。
- 初稿假设「复用 `WdatWriter` 自带简拼区」。实际只有拼音合并构建路径生成简拼区（`manager.rs` 的 `import_tables` 一带），通用 `export_to_wdat` 不生成。本稿新写一条段构建路径。
- 初稿的折回不 fsync、state 无恢复规则、Add 非幂等、匹配键未规范化。本稿 §3 逐条定义。

## 1 三种存储原语

| 原语 | 适用数据 | 持久化方式 | 读路径 |
|---|---|---|---|
| **文本真源** `.dict.yaml` / 覆盖表 | 用户策划、低频变动 | 用户手改；程序按行补丁写回（tmp + fsync + rename） | 解析后建段或载入内存 |
| **快照 + 日志 + 增量表**（引擎） | 按键路径上的词条与计数 | 日志一条一次 `write`；快照 tmp + fsync + rename | 段 mmap + delta 叠加，无锁 |
| **原子小文件** | 低频记录（统计、补全历史、元数据） | 去抖后整文件 tmp + fsync + rename | 启动载入内存 |

引擎只有一套实现，两种折回目标：

- 运行时层：旧快照 + delta → 新快照。
- 文本层：delta → 文本按行补丁 → 文本重新解析 → 新段。

## 2 目录布局与格式

### 2.1 目录策略：用户目录装下全部数据

**前提（P5 定，2026-10-15）：只复制用户目录就带走全部数据。** 本机目录里只能有两类东西：与设备绑定的定位数据 / 临时状态，以及能由用户目录重建的加速结构。初稿把学习数据、补全历史、统计放在 `state/`，复制用户目录会丢掉它们，P5 收回。

| 类别 | 判据 | 位置 | XDG 对应 |
|---|---|---|---|
| 数据 | 用户的一切数据（策划的、学到的、统计的），可手改、备份对象、可漫游 | 用户目录 `user_config_dir()`（漫游 `%APPDATA%` / 便携 `userdata/` / `datadir.conf` 自定义） | `XDG_CONFIG_HOME` |
| 状态 | 只属于本机：设备标识、锁、窗口位置；未折回的日志；可由用户目录重建的快照段 | 本机目录 `local_dir()/state/` | `XDG_STATE_HOME` |
| 缓存 | 纯派生、可随时整目录删除 | 本机目录 `local_dir()/cache/` | `XDG_CACHE_HOME` |

操作规则：**备份 / 迁移 = 复制用户目录**（服务运行时先 `dict.flush`；否则差的是尚未折回的学习记录：停手 30 秒即折回，连续输入时最老的一笔待处理写入超过 5 分钟也折回，重启服务前折回一次）；**排障清缓存只删 `cache/`**；**删掉整个 `state/` 不丢数据**（快照段由用户目录重建，丢的只有未折回的日志尾巴）。

```
<userdata>/                              数据
  config.toml  schemas/  themes/  …      （现有设置）
  meta.toml                              dir_id、format_version（迁移的唯一判据，§7.4；最终布局 = 5）
  userdata.redb.legacy                   旧库，迁完改名留作兜底（§7.3）
  data/
    phrases.dict.yaml                    全局短语
    phrases.sys_hash                     系统短语指纹（短语叠加模型用）
    quick_format.toml                    快捷格式调整（对齐出厂 system.quick.toml）
    common_chars.yaml                    常用字增删（出厂层是 YAML）
    charsets/<key>.yaml                  字符集用户层（出厂层是 YAML）
    completion.table.yaml                邮箱后缀 / 网址 / 快捷输入历史
    stats/<YYYY-MM>.toml  stats/meta.toml  每日统计 / 累计元数据
    <schema>/
      user.dict.yaml                     主动造词 / 导入词
      shadow.dict.yaml                   pin / delete 规则
      temp.dict.yaml                     临时词（学到的）
      draft.dict.yaml                    草稿
      freq.table.yaml                    选词计数
      assoc.table.yaml                   联想历史

<localdata>/
  cache/<schema>/…                       系统词库缓存
  state/
    state.toml                           本机状态（工具栏位置等）
    device.toml  lock                    设备标识、单写者锁
    runtime/<dir_id>/
      journal/<device_id>.wjl            本设备日志（未折回的操作）
      state                              各目标折回水位与文本指纹
      <schema>/<target>-<gen>.wdat+.meta 各目标的快照段（加速结构，由 data/ 重建）
  logs/
```

`<schema>` 指**数据方案 id**（拼音族折叠为 `pinyin`，临时英文与英文方案共用），与现有 `data_schema_id` 一致，全拼与双拼继续共享。用户目录里的 `data/` 与安装根下的出厂 `data/` 只是同名，互不相干。

日志按设备、二进制、不给人看，放 `state/`：被同步盘或漫游配置复制到别的机器只会制造冲突。快照段是加速结构：`state/` 整个删掉后，下次启动由 `data/` 的文本重建。

### 2.2 词库文本格式

三类都是 Rime `.dict.yaml`：YAML 头 + `...` 后的 TSV 正文。Rime 忽略头里不认识的键，扩展放 `wind:` 下。

**词条文件**（`user.dict.yaml`、分库）：

```yaml
# 用户词库
---
name: pinyin.user
columns: [text, code, weight]
wind:
  kind: words
  code_spacing: syllable      # 拼音码以空格分音节；无空格的码表示「边界未知」，不视为单音节
  escape: true                # text 列按 escape_text_field 规则转义 \t \n \\
...
你好	ni hao	100
```

- `code_spacing: syllable` 是必须的扩展：现有库里大量词的边界为「未知」（`boundary == 0`，交给 DAG 现切），标准解析会把无空格码当成单音节（`syllable_boundary_mask` 返回 `0b1`）。本声明让解析器对无空格行给 `boundary = 0`。非拼音方案（五笔等）不声明，码恒无边界。
- `escape: true` 对词条文件也要开：用户词文本可含真实换行与制表符（`store_layer.rs` 顶部注释），标准 Rime 行会被切坏。转义规则照搬 `wdict.rs` 的 `escape_text_field`，含命令栏文本不转义反斜杠的分流。
- 写回与迁移导出一律用 `join_code_by_boundary` 输出带空格码；边界未知的词原样输出无空格码。

**短语文件**（`phrases.dict.yaml`，全局）：

```yaml
columns: [code, text, weight, position, enabled, category, overrides_system]
wind: {kind: phrases, escape: true}
```

`position` 为用户排序位；`enabled` 为 `1`/`0`，软删除即 `0`；`overrides_system: 1` 表示这一行是对自定义层 `system.phrases.toml` 同码同文短语的覆盖（禁用系统短语就是写一行 `enabled=0, overrides_system=1`）。后四列可省略，省略取默认。

**Shadow 文件**（`shadow.dict.yaml`，按数据方案）：

```yaml
columns: [code, text, action, position, cand_id]
wind: {kind: shadow}
```

`action ∈ {pin, delete}`；`position` 是 pin 的**目标槽位**（置顶 = 0、上移 = 原下标 − 1），同一编码下多条 pin 的新旧先后由**行序**表达（先出现的更新，程序写入时插在该组最前）；`cand_id` 可空，供动态短语精准去重。空编码是合法的组（特殊模式的规则桶）。文件开 `escape: true`（候选文本可含换行）。

### 2.3 解析与规范化

- 列规格判定复用 `rime-dict-loading.md` 的规则（`columns:` 声明优先）；`wind:` 块解析为一个结构体；`# no comment` 等指令沿用。
- **匹配键规范化函数唯一**：`normalize_key(code, text)` 去空格、按方案 `lowercase_code` 小写、`normalize_input` 文本。解析器建段、delta 匹配、折回补丁三处都用它。初稿因为没有这一条，运行时拍平后的 `nihao` 永远匹配不上文件里的 `ni hao`，删词会在折回后复活。
- 读入剥 BOM；写回保持原 BOM 与换行风格（CRLF / LF 按文件首行判定）。非 UTF-8 或头部解析失败的文件标记为「隔离」：仍尝试按已有段提供查询，不往里折回，记 WARN 并在设置页提示。
- 重复行：同规范化键多行时，Delete 全删，SetWeight 只改第一行，其余照旧。用户改了某行的 code 列，旧码上的 Delete 成为空操作，这是预期行为。

### 2.4 格式规则

| 数据形状 | 格式 | 文件 |
|---|---|---|
| 设置与小型结构化数据 | TOML | `meta.toml`、`quick_format.toml`、`stats/*.toml` |
| 词条清单（码 → 词） | 表格文本 `.dict.yaml` | `user` / `shadow` / `temp` / `draft` / 短语 |
| 其他列表（计数、历史） | 表格文本 `.table.yaml` | `freq` / `assoc` / `completion` |
| 出厂层本是 YAML 的覆盖层 | YAML | `common_chars.yaml`、`charsets/*.yaml` |

- 表格文本 = Rime 风格 YAML 头 + `...` 后的 TSV 正文，头里 `columns:` 声明列序，`wind: {kind: …, escape: true}` 区分种类。`.dict.yaml` 与 `.table.yaml` 同一套解析与按行补丁，只是扩展名告诉人「这是词条」还是「这是别的清单」。
- 不用 JSON（与现有配置风格不统一）；不发明二进制源格式（实测 19 万行表格文本解析 ~100 ms、指纹 0.5 ms、整写 + fsync 5 ms，二进制省下的这点时间不抵手改、排障、合并的损失；加速交给 `state/` 里的快照段）。
- 时间戳写 Unix 秒的整数。

学到的数据各列（`code` 按方案规范化，有音节边界就写空格（任何方案，读时还原边界），拼音族单个汉字的无空格码按单音节读；`assoc` 的 `context` 是上文原样不规范化）：

| 文件 | `kind` | 列 |
|---|---|---|
| `temp.dict.yaml` | `temp` | `text, code, weight, count, created_at` |
| `draft.dict.yaml` | `draft` | `text, code, last_used` |
| `freq.table.yaml` | `freq` | `code, text, count, last_used` |
| `assoc.table.yaml` | `assoc` | `context, text, count, last_used` |
| `completion.table.yaml` | `completion` | `kind, text, count, last_used` |

- 覆盖表（`quick_format.toml`、`common_chars.yaml`、`charsets/*.yaml`）、短语、Shadow：用户策划的小表，内存持有，改一次整文件提交（带指纹复核），不走日志。
- 补全历史、统计：机器写的小文件，内存持有、去抖整文件原子写，退出时写一次。
- 学到的数据（temp / draft / freq / assoc）：文本目标，走引擎（日志 + delta + 空闲 30 秒折回按行补丁写回），与 `user.dict.yaml` 同一条路径。
- `<userdata>/meta.toml`：`dir_id`、`format_version`。随目录漫游 / 复制。
- `<localdata>/state/device.toml`：`device_id`、`machine_fp`、`dir_id`。只属于本机。

### 2.5 格式异常统一策略

所有文件按「谁写的」分两类，规则统一：

1. **行级**：某一行解析不了（列缺、数字列不是数字）⇒ 跳过这一行、原样留在文件里（按行补丁不碰它）、计数，记 WARN（只记文件与行数，**不记内容**），在设置页提示。
2. **手改类文件整份坏**（非 UTF-8、头部坏；`user` / `shadow` / `temp` / `draft` / `freq` / `assoc` / 短语 / 覆盖表）⇒ 隔离：**绝不覆盖**，按最后一次成功的数据继续服务；引擎目标的新写入留在日志与 delta 里，修好后下一次同步 / 折回自动合入；覆盖表与短语这类小表在隔离期间拒绝写入并报「文件格式有误，请先修复」。用户显式的整体替换（清空、导入、还原）先把坏文件改名 `<文件>.corrupt-<时间>` 留存，再写新文件。
3. **机写文件坏**（`completion.table.yaml`、`stats/*.toml`）⇒ 先从 `.bak` 自愈；`.bak` 也坏就把坏文件改名 `<文件>.corrupt-<时间>` 后从空重建（统计元数据由各月文件重算，不归零），提示一次。
4. 任何解析失败的文件都不被程序覆盖。设置页从 `system.storageStatus` 的 `issues` 拿到「哪个文件、坏几行 / 已隔离 / 已重建」。

## 3 快照 + 日志 + 增量表引擎

放在新 crate `wind-userdata`（名字可议），依赖 `wind-dict`（wdat、cache_fp、reader_pool、build_guard）与 `wind-store`（编解码）。`wind-dict` 的 `store_layer.rs` 退役后不再依赖 `wind-store`，依赖方向变为：

```
wind-store (文本编解码、小文件) ← wind-dict (格式) ← wind-userdata (引擎、层) ← wind-engine
```

### 3.1 快照

- 词条快照 = wdat 段。新写 `build_user_segment()`：输入 `(code_spaced, text, weight, order, boundary, kind)` 迭代器，输出带 AbbrevSection 的 wdat，简拼区**包含无边界组**（`\u{1}` + 码首字符，与 `abbrev_index.rs` 的兜底组语义一致）。构建期串行解析，不起 `available_parallelism` 线程。
- sidecar `.meta`：按 wdat 条目序号对齐的定长记录，`count u32 | created_at i64 | last_used i64 | flags u8`，mmap。
- 草稿、计数、联想历史量小且只做精确查询：定长记录排序数组 + 二分，`.bin` 文件 mmap 或常驻（≤ 3 MB 常驻，同 `.wridx` 规则）。
- 段按代号命名（`<name>-<gen>.wdat`），sidecar 头记 gen 并作为**提交点**原子写：每次换段一个新路径，从不覆盖正被映射的文件（`reader_pool` 不会按大小与 mtime 复用陈旧 reader，也就无需 `replacing`）；崩溃在 wdat 与 sidecar 之间只留下无人引用的新文件，旧段原样可读。sidecar 在而段打不开 ⇒ 该目标水位不可信，整份回放日志。
- 空输入定义为「零段」，不是失败；`CachedDict` 现有的「空则报错、写失败退化内存模式」不适用于本引擎。

### 3.2 日志 `.wjl`

```
Header : magic "WJNL" | version u16 | device_id [u8;16] | dir_id [u8;16]
Record : len u32 | crc32 u32 | seq u64 | ts_ms i64 | kind u8 | target u8 | schema | code | text | payload
```

- kind：`Upsert{weight, kind}`、`Delete`、`SetWeight`、`Incr{count}`、`Touch{last_used}`、`Promote`、`Expire`、`PhraseUpsert`、`PhraseDelete`、`ShadowPin`、`ShadowDelete`、`ShadowRemove`、`BatchMarker{op}`。
- target：目标（`user` / `temp` / `draft` / `freq` / `assoc` / `phrases` / `shadow`）。Delete 与 SetWeight 必须带目标，不做「所有文件都删」。
- **一条记录一次 `write(2)`，无用户态缓冲**。进程崩溃（含 `panic = "abort"`、OOM abort、Android SIGKILL）不丢已返回的记录。断电：后台节拍每 5 秒检查，有新写入就 `fdatasync` 一次（P5），断电最多丢最近 5 秒。
- 计数记增量不记终值：多设备合并时求和，无需冲突规则。
- 日志永不 mmap；裁剪走重写 + rename，不原地截断（Linux 上原地截断被映射的文件会 SIGBUS）。
- 保留策略：折回后保留至大小上限（默认 8 MB）或时间上限（默认 7 天）取其长；裁剪只裁已折回的头部。某目标长期折不回时不强裁，而是触发折回。

### 3.3 增量表 delta

- 两级结构：冻结层 `Arc<[Entry]>`（按规范化键有序）+ 尾巴 `Vec<Entry>`（≤ 128 条）。写入只克隆尾巴；尾巴满时并入冻结层。每条带 `seq`、`target`。
- 附带声母分组索引 `BTreeMap<group, Vec<idx>>`，分组规则复用 `abbrev_index::group_of`，含无边界组。初稿的线性扫在 2000 条时每键约 1.16 ms，已吃光基线，不可用。
- 同键多操作并存（晋升 = temp Delete + user Upsert），按 seq 顺序叠加，不合并成一条。
- 折回只摘除 `seq ≤ 该目标已折回水位` 的条目，折回期间新到的操作保留。
- 查询叠加规则：Delete 剔除、SetWeight 覆盖、Upsert 并入或覆盖。前缀 Top-K 时每段多取 `K + 该前缀下 delta 覆盖条数`，否则被删条目占名额、被调高的条目漏掉。
- delta 按数据方案 id 放进进程级注册表，不随引擎重建（`engines.clear()`）丢失。

### 3.4 快照发布与写后可读

- `Snapshot { segments, sidecars, delta, phrases, shadow }` 用 `ArcSwap` 发布（新增依赖 `arc-swap`，理由：读路径零锁、无 `RwLock` 读者计数的缓存行争用；若不接受新依赖，用 `RwLock<Arc<Snapshot>>` 只在克隆 Arc 时持读锁亦可）。
- **追加日志与发布快照在调用线程上同步完成**，由一把写锁串行化（写者之间互斥，读者不等）。这保证加词后立即刷新候选、英文学词先查后写、设置页增加后立即列出，以及现有测试的「写后可读」语义。折回在独立线程。
- 消费方只借用 `Arc<Snapshot>`，不把它或其中的 reader 存进长寿结构；测试锁定「折回后旧段 strong_count 归零」。

### 3.5 折回（fold）

触发（按成本，不按短空闲）：

- 运行时目标：delta 中该目标条数 ≥ 阈值（默认 4000），或日志超过保留上限，或长空闲（默认 10 分钟）且有未折回操作，或显式 `dict.flush`。进程退出时尝试折回，但**不依赖它**（Android 会被 SIGKILL）。
- 文本目标：delta 中该目标有操作且空闲 30 秒，或显式重载 / 导出 / 备份 / 还原 / `schema.delete` 之前。文本目标的操作来自用户主动动作，一天几次，成本不是问题。

折回线程：Windows 用 `THREAD_MODE_BACKGROUND_BEGIN`（同时降 IO 优先级），其他平台降 nice；构建期串行，不抢满核。

**持久化次序（真源级，必须严格）**：

1. 文本目标：重新读文件，校验长度、mtime、指纹并记录。按行补丁：Upsert 已存在则更新权重取 max 并保留原行位置，不存在才追加末尾；Delete 删匹配行；SetWeight 改权重列。写 `tmp`，**fsync tmp**，再次校验原文件指纹未变（变了说明编辑器刚保存，丢弃本轮重做），`rename` 覆盖，**fsync 目录**（Windows 用 `MoveFileExW(MOVEFILE_WRITE_THROUGH)`），保留上一版为 `.bak`。
2. 重建该目标的段（新代号文件 + sidecar 原子写为提交点，见 §3.1）。
3. 构造新 Snapshot 换入；只有新段换入成功才从 delta 摘除该目标 `seq ≤ 水位` 的条目。段重建失败则保留 delta、不推进 state、记 WARN。
4. 写 `state`（tmp + fsync + rename）。

运行时目标省去第 1 步，快照直接由旧快照 + delta 生成，同样 tmp + fsync + rename。

**冲突规则（文本目标）**：

- 文件里已无该规范化键：Delete、SetWeight 为空操作；Upsert 追加。用户删掉的行不复活。
- 记录 `ts_ms` 早于文件 mtime 的 SetWeight 视为被手改覆盖，跳过；Upsert 仍按「存在则取 max」处理。
- 文件缺失：区分「暂不可用」（目录存在、文件被占用或为 `.icloud` 占位）与「已删除」。前者跳过本轮；后者视为用户明确删除，丢弃该目标的旧记录并把 state 该目标清零。
- 真源被占用（杀软、同步盘）导致 rename 失败：指数退避重试 5 次，仍失败则保留 delta 不推进 state。

### 3.6 state 与序号

```
state (postcard, 版本化)
  generation u32                        还原 / 批量替换后递增
  next_seq u64                          序号续号来源
  targets: map<target_id, { folded_seq u64, text_fp Option<u64>, snapshot_fp u64 }>
```

- 启动：读 state；读不出或版本不符 → 视为「水位未知」，**不整份回放**：以当前快照为准，日志只回放 `generation` 相同且 `seq > 所有快照内嵌的 max_seq` 的记录（快照头部记录其覆盖到的 seq）。即快照自带水位，state 只是加速。
- 文本目标多一步：state 里的 `text_fp` 与文件当前指纹一致才只回放增量；不一致说明有人手改过，仍只回放 `seq > folded_seq`，绝不回退。
- `next_seq = max(state.next_seq, 日志尾 seq) + 1`，持久化在 state；用户删掉 `journal/` 后序号也不会倒退。
- `generation`：P1 起批量操作（清空、导入、还原、迁移）一律走 `fold_with`——新快照内嵌 `max_seq = upto` 原子换入即推进水位，被取代的旧记录不再回放，不需要按代过滤。字段保留，留给日后「整目录文件级替换」式还原。

### 3.7 单写者与设备标识

- `state/lock` 锁文件（`std::fs::File::try_lock` 或 `fs2`）。拿不到锁的进程进入只读模式：查询正常、写入丢弃并记 WARN（与现状「第二个进程打不开 redb 就不落盘」一致）。redb 退役后这是唯一的进程互斥。
- Android：IME 进程持锁；设置 App 若在独立进程，写操作经 IPC 转发给 IME 进程，不直接写。
- `device_id` 不是机器 id 也不是目录 id。`state/device.toml` 存 `device_id`、`machine_fp` 与上次配对的 `dir_id`；`<userdata>/meta.toml` 存 `dir_id`。启动时三者对不上就说明发生了下列之一：用户目录被复制到另一台机器（`machine_fp` 不符）、本机换了用户目录（`dir_id` 不符）。两种情况都生成新 `device_id`，旧日志整体折回后封存（改名 `.wjl.sealed`，仍可作同步源）。同机多账户各有本机目录，自然不同 id。

### 3.8 批量操作

`dict.import`、`dict.clear`、`temp.promoteAll`、`schema.delete`、备份还原、迁移：不逐条走日志。流程是先折回全部待处理操作，`generation += 1`，直接生成新文本或新快照整文件替换，日志只记一条 `BatchMarker`。还原前封存旧日志，杜绝旧操作回放复活。

### 3.9 索引产物

- 按词查码索引（`UserTextIndex`）与联想索引（`UserAssocIndex`）改为**按段在构建时顺带生成并落盘** `.wtxi` / `.wasi`（骨架同 `.wridx`），mmap；delta 上另挂小覆盖表。换段成本为零，不再有代次轮询、5 秒节流和全表重建。
- 联想排序键 `count << 32 | created_at` 与门槛 `count ≥ 2` 由 sidecar 提供。

## 4 读路径

每数据方案一个 `UserDataLayer: DictLayer` 对外暴露**三个子层**，保持现有 `LayerType` 与 `base_order`：

| 子层 | LayerType | 数据来源 |
|---|---|---|
| 用户层 | User = 1 | 文本段（user + 分库）+ delta(target=user) |
| 临时层 | Temp = 2 | temp 快照 + sidecar + delta(target=temp) |
| 草稿层 | Draft = 5 | draft 快照 + delta(target=draft)，只精确查询、不建简拼、开关开了才挂 |

`CompositeDict::merge_search` 的去重、继承权重、前缀取最短码语义不变；每层各自 limit，临时词不会被用户词挤占名额。

- 用户自放的只读词库走扩展词库目录（系统层，见 §10 P2c），不进用户层；删除其中的词照旧是 Shadow delete。
- 临时词晋升：`temp Delete + user Upsert` 两条记录，同一 seq 对；晋升阈值判断读 sidecar 的 count 与 delta 的 Incr 之和。
- `on_word_selected` 阈值调权：count 走 Incr，满阈值时追加 SetWeight；语义与现状一致。
- 英文学词：`meta.is_temp_dict` 对应 target = temp；先查后写在同步写路径下成立。

## 5 快速扫描与 IPC

- `dict.reload(paths?: [string])`：路径限定在 `<userdata>/data/` 内，越界拒绝。带路径只校验并重建这几个文件；空参数全量扫描。供设置页与日后同步组件使用。
- 快速扫描：先比 `len + mtime`，**不同则必算指纹；相同只作「大概率未变」提示，仍对声明过 `wind: {verify: content}` 或上次指纹时间超过 24 小时的文件算一次内容指纹**（`rsync -t`、同步工具会保留 mtime，`cache_fp` 当初拒绝 mtime 的原因同样适用）。触发：启动、切方案、配置热重载、`dict.reload`。
- `dict.flush`：显式折回全部目标，供备份 / 导出 / 调试。

## 6 redb 各表去向

| 表 | 去向 | 阶段 |
|---|---|---|
| `temp_words` `temp_abbrev` | 运行时层 temp 快照 + sidecar + 日志（P1b 起旧表只读） | P1 |
| `draft_words` | 运行时层 draft 快照 + 日志（P1b 起旧表只读） | P1 |
| `freq` | 运行时层 freq 快照 + 日志（Incr/Touch）（P1b 起旧表只读） | P1 |
| `assoc_history` | 运行时层 assoc 快照 + 日志（P1b 起旧表只读） | P1 |
| `user_words` `user_abbrev` | 文本 `user.dict.yaml` + 段 + delta | P2 |
| `phrases` | 文本 `phrases.dict.yaml` | P3 |
| `shadow` | 文本 `shadow.dict.yaml` | P3 |
| `quick_format` `common_chars` `charset_user` | 覆盖表文本文件，原子整写 | P4 |
| `completion` `stats_daily` | 原子小文件 | P4 |
| `meta` | `meta.toml` + `state` | P4 |

P4 结束后 `wind-store` 删除 `store.rs` 与 redb 依赖，保留编解码与小文件工具；`Store::with_db`、`drop_page_cache`、空闲回收线程、`pause/resume` 一并删除。

## 7 迁移、回滚与 redb 退役

原则：**兼容性护栏必须在破坏性变更之前的版本里发出去**。去掉 redb 的那个版本只是「不再打开旧文件」，不承担迁移。

### 7.1 降级保护（最先发布，先于 P1）

- `<userdata>/meta.toml` 的 `format_version` 从下一个发版起写入（当前值 1 = 纯 redb）。读到比自己认识的更高的版本号时，程序进入只读模式并提示「数据由更新版本写入，请升级或还原备份」，不写任何用户数据文件。
- 这一条必须在新格式落地之前就进入现场，否则用户升级后回退，旧版本会在新格式目录旁继续写 redb，两边分叉。

### 7.2 逐阶段迁移（P1–P4，主程序内）

- 每阶段首次启动时对该阶段的表做一次性导出：写到 `<localdata>/state/migrate.tmp/`（状态类）或 `<userdata>/user.migrate.tmp/`（真源类），每个文件 fsync，整目录 rename 到位，最后写完成标记 `state/migrated-<phase>`。没有标记就整体重做，不按「文件已存在」跳过。
- 迁移成功后旧表只读保留，`format_version` 升一级。
- 回滚：用户回退到仍带 redb 的旧版时，旧版因 §7.1 进入只读模式，不会把数据写回 redb，所以再次升级时无需合并。若用户主动删除 `meta.toml` 强行让旧版写入，再次升级时按「标记已存在但 redb 表 `words_gen` 比标记记录的大」做一次增量 upsert 导入。
- 两套真源并存只允许发生在**不同数据类别**之间（如 P1 后临时词在引擎、用户词在 redb），同一类别不并存。`import-export-backup-design.md` 的逐表 upsert 改为按类别路由到对应原语；`user-override.md` 的模型 C（用户数据只进数据库）随 P2 改写；`auto-phrase-draft-layer.md` 的存储节随 P1 改写。
- 备份：方案清单改为扫描 `user/` 目录 + `state/` 目录，不再来自 redb；用户分库纳入备份范围。还原按 §3.8 走。

### 7.3 redb 退役版本与跳跃升级

- 正常逐版升级的用户在到达退役版本前已经完成全部迁移，退役版本只检查 `format_version`（§7.4）。
- 跳跃升级（从未经过迁移版本的旧版直接升到退役版本）由**独立迁移器** `wind_migrate` 覆盖：工作区里单独的 bin crate，是唯一允许依赖 redb 的 crate，feature 隔离，随安装包分发。主程序启动时发现 `userdata.redb` 存在且缺少标记，调用它完成 §7.2 的全部阶段，再继续启动。它的输出就是正常迁移的输出，走同一套暂存目录与标记。
- `wind_migrate` 保留两个大版本周期后从安装包移除，文档改为「先升到 X 再升」。核心 crate 从退役版本起零 redb 依赖，`cargo tree -p wind_input_service` 无 redb 作为退出判据。
- **源文件永不删除**：迁移成功只把 `userdata.redb` 改名为 `userdata.redb.legacy`。用户任何时候都能用旧便携版的 `dict export` 导出 wdict 再导入新版本，这是最后的兜底。`.legacy` 文件计入备份但不计入还原。
- Android / macOS / Linux 同样随包携带 `wind_migrate`，各自按平台目标编译。

### 7.4 P5：用户目录装下全部数据（`format_version` 仍为 5）

P2–P4a 的中间布局（`user/` 目录名、补全 / 统计在 `state/`、`quick_format.yaml`）只在开发分支与维护者的测试机上出现过，从未发布，所以 P5 **不做中间布局的迁移，也不升版本号**：`format_version` 5 就是最终布局。发布版本只有 1（纯 redb），走 P1–P4a 的旧库链直接迁成最终布局。

- **唯一判据**：`<userdata>/meta.toml` 的 `format_version`。≥ 5 表示旧库各表已迁完、不再打开 redb。`state/` 里的 `migrated-*` 标记只在旧库链进行中作断点续做用，`format_version` 写到 5 时全部删除；此后它们丢失、残留都不影响任何判断。
- 写到 5 的同时，`userdata.redb` 改名 `userdata.redb.legacy`（已有 `.legacy` 则加序号，从不覆盖；改名失败只 WARN，下次启动再试；此时 redb 已不再打开）。
- 学到的数据：引擎里有该目标（快照或 delta）而 `data/<schema>/` 下没有对应文本、state 也没记过文本指纹 ⇒ 绑定时由当前视图渲染出文本（不由「文件不存在」推出「数据为空」）；文件已在 ⇒ 文件为准。旧库链的 P1 迁移在绑定规则设好之后进行，直接写成文本。
- `meta.toml` 缺失时：旁边有 `userdata.redb` 记 1，否则（全新目录，或旧库已改名 `.legacy`）记最新。
- redb 依赖与 P1–P4 迁移代码保留到维护者决定的退役版本（§7.3、P4b）。

## 8 环境降级

- 目录只读、磁盘满：日志写失败 ⇒ 本次写入返回错误、不进 delta（不做「内存保留」模式：内存里攒着却写不进去的数据退出即丢，给用户造成「存了」的错觉），原因记入 `write_error`（不含内容），设置页经 `system.storageStatus` 提示「最近的修改未保存」；之后写成功即清除。（2026-10-16 修订：原稿的「内存保留、delta 上限 2 万」未实现，改为此行为。）
- 单写者锁拿不到（另一个进程占着）或锁 API 本身报错（网络家目录上 flock 不可用）：只读模式，查询正常、写入丢弃，设置页提示。
- 文本真源隔离（非 UTF-8、头损坏）：见 §2.3。
- 杀软 / 同步盘占用：见 §3.5。

## 9 指标（审查修正值，开发机口径；低配机 ×2–3）

| 项 | 预期 | 依据 |
|---|---|---|
| 按键查询延迟 | 单段精确 / 前缀 / 简拼与系统段同量级；delta 带分组索引后 19 万词简拼召回不劣于 1.19 ms 基线 | abbrev-index 文档 §8.1；审查实测 |
| 写入延迟（按键路径） | 一次 `write` + 尾巴克隆，≤ 0.2 ms | 审查实测（2000 条整表克隆 0.14 ms，本稿只克隆 ≤128 条尾巴） |
| 单段重建 | 几千条 < 50 ms；5 万 0.4 s / 40 MB；20 万 1.2–2 s / 130–160 MB；65 万 4–7 s / 420–500 MB | schema-dict-discovery §6.3；manager.rs 注释 |
| 每次折回写盘 | 运行时：快照大小；文本：文本 + 约 3 倍的段 | datformat 布局 |
| delta 常驻 | 每条约 170 B（含 malloc）；运行时目标 ≤ 4000 条 ≈ 0.7 MB | 审查实测 |
| 磁盘 | wdat ≈ 文本 2.8–2.9 倍（约 95 B/条）；日志 ≤ 8 MB 或 7 天 | reader_pool.rs 注释 |
| 常驻私有内存 | 段与 sidecar mmap 不计 Private；redb 页缓存（19 万词 41 MB）消失 | memory-footprint §6 |

## 10 分阶段

| 阶段 | 内容 | 退出判据 |
|---|---|---|
| 前置 | §7.1 降级保护：`meta.toml` 的 `format_version` 与只读模式；`state/` 目录与 `state.toml` 挪位 | **已实现**（2026-10-14，aa83e4a8 / af84a68a）：随下一个发版进入现场 |
| P0 | `wind-userdata` 引擎：快照、日志、delta、state、锁、device_id、折回；快照段带简拼区与无边界组 | **已实现**（2026-10-14，cbe75b3d…a19027ef；计划 `user-data-storage-plan-p0.md`）：引擎级测试 18 + 5 全绿。与设计的差异：日志记录多一个 `boundary` 字段（回放时 delta 要建简拼分组）；快照发布用 `RwLock<Arc<View>>` 不引 `arc-swap`；折回在调用线程上做，P1 再放独立线程。真实 `userdata.redb` 的等价测试归 P1（迁移时才有对照） |
| P1a | 引擎补强（时间、叠加唯一、delta 二分、fold_with、零条目快照、state 版本、压实与后台折回）；`Runtime` 四组领域 API；临时词层 / 草稿层 | **已实现**（计划 `user-data-storage-plan-p1a.md`）：领域 API 与 `wind-store` 对拍测试全绿 |
| P1b | 接线：协调器写路径、`EngineManager` 换层、设置页 IPC、备份 / 导出、联想索引改走 `Runtime`；redb → 引擎一次性迁移；`format_version` → 2；退役 5 张表 | **已实现**（计划 `user-data-storage-plan-p1b.md`）：与设计的差异——**`Store` 做门面**，四组方法签名不变、方法体委托 `Runtime`，调用方零改动、现有测试即对拍；为此 `wdict` 拆成 `wind-wdict`、Store 三层挪进 `wind-store`。全量回归 4930 条 0 失败；按键路径不经 redb Mutex（`pause()` 测试守）。运行时状态按用户目录分开（`state/runtime/<dir_id>/`）；迁移为合并语义、完成才升 `format_version`。独立审查探针：5000 临时词 + 19 万词频的库迁移 451–476 ms、再开 86–89 ms；真实用户库等价待靶机（`examples/p1_migrate_check.rs`） |
| P2a | 文本层引擎：`user.dict.yaml` 解析与 `normalize_key`、按行补丁、文本目标折回 / 同步 / 整文件替换、sidecar 记行序、用户词层与领域 API | **已实现**（计划 `user-data-storage-plan-p2a.md`）。与设计的差异：文本只存 text / code / weight（用户词 count / created_at 不进文本）；段放在引擎状态目录（沿用 sidecar 提交点，可由文本重建）；`sync_text` 总算内容指纹，不走长度 + mtime 快路径；写出的正文以 `# no comment` 开头；补齐的音节边界改写该行码列；归文本管的目标绑定前不折回；单个文本目标折回失败只退避该目标 |
| P2b | 接线：`wind-store` 用户词门面委托、redb `USER_WORDS` → `user.dict.yaml` 迁移、`format_version` → 3、`dict.reload` / `dict.flush`、启动 / 切方案 / 配置重载时 `sync_text`；退役 2 张表 | **已实现**（计划 `user-data-storage-plan-p2b.md`）。与设计的差异：用户分库移到 P2c；用户词 count / created_at 不再保留，`on_word_selected` 删除（无生产调用方）；`lowercase` 未按方案下发；拼音文本里单个汉字的无空格码按单音节解析（空格载体表达不了 0b1）；空闲写回由 5 秒一拍的后台节拍驱动。合成 19 万词旧库迁移 674 ms、逐码比对 0 不一致 |
| P2c | 用户分库 | **并入现有扩展词库目录，不另建机制**（2026-10-14 裁定）：用户自放的只读 `*.dict.yaml` 已由 `[[dictionary_dirs]]` / 默认 `<主词库目录>/ext`（`inject_dir_dicts`，用户目录优先）识别，按方案 override 逐个开关即「登记」，设置页有入口（`schema-dict-discovery.md`）。再在 `user/<schema>/` 下开第二个入口只会让同一件事两处配置。`user/<schema>/` 只认 `user.dict.yaml`，其余文件不读 |
| P3 | 短语与 Shadow 文本化；设置页列表 / 编辑改走新层；系统短语覆盖语义 | **已实现**（计划 `user-data-storage-plan-p3.md`）。与设计的差异：两张表按 §2.4 小表处理（内存持有、按组补丁、整文件提交，不走日志 + delta），按键路径查 Shadow 零 IO；短语为叠加模型——系统短语只在内存（每次启动由协调器交给 store），文件只记用户行与 `overrides_system: 1` 的修改行；系统短语改键时原键写停用的修改行、新键归用户；`phrase_sys_hash` 留在 redb `META` 等 P4；短语迁移要 TOML 才分得清系统行被改过没有，所以在首次 `set_system_phrases` 时做，做完立即升 `format_version` 到 4 |
| P4a | 覆盖表与小文件：快捷格式 / 常用字 / 字符集用户层进用户目录（文件是真源，写入带指纹复核），补全历史（内存持有、2 秒去抖落盘）/ 每日统计（按月）/ 统计元数据 / `phrase_sys_hash` 进本机状态目录；`format_version` → 5；备份补快捷格式，还原后重载镜像与统计收集器 | **已实现**（计划 `user-data-storage-plan-p4a.md`）。与设计的差异：备份包格式不变（老备份照样能还原），「按类别直接打包 `user/` + `state/`」随 P4b 做；补全历史原在按键路径上读 redb（邮箱 / 网址 / 快捷输入每键一次），现读内存；`schema_version`（META）随 redb 留到 P4b。此后运行期不再写 redb，它只供迁移读 |
| P4b | 删除 redb 依赖；独立迁移器 `wind_migrate`（§7.3）；备份按类别打包目录；删掉暂停 / 恢复与页缓存回收机制 | **须等 P1–P4a 的迁移版本发出后再做**（§7：兼容护栏先于破坏性变更），发版时机由维护者定。退出判据：`cargo tree` 无 redb；备份还原全量回归 |
| P5 | 用户目录装下全部数据：`user/` → `data/`；学到的数据（temp / draft / freq / assoc）改为 `data/<schema>/` 下的文本目标、快照降为加速结构；补全与统计进 `data/`；格式统一（§2.4，去 JSON、快捷格式改 TOML）；格式异常统一策略（§2.5）；日志有写入时每 5 秒 fsync；`format_version` 为唯一迁移判据（仍为 5：中间布局未发布，不迁移不升版本），迁完 `userdata.redb` 改名 `.legacy`（§7.4） | 计划 `user-data-storage-plan-p5.md`。退出判据：删掉整个 `state/` 后重启，学到的数据、补全、统计全在；全量回归。**已实现**（靶机 Windows 实测：从旧库完整迁移、删 `state/` 后逐项一致、手改往返）。与设计的差异：全部七个目标都归文本或小表，§3.5 / §3.3 所说的「运行时目标按 4000 条阈值 / 长空闲 10 分钟折回」不再有对象，改为文本目标空闲 30 秒或最老待处理写入超过 5 分钟折回；§3.9 索引产物（`.wtxi` / `.wasi`）未做，`UserTextIndex` / `UserAssocIndex` 仍按代次重建；§3.2 日志「7 天时间上限」未做（只有 8 MB）；§3.5 折回线程降优先级、`.icloud` 占位区分未做；§8 见修订；草稿不进备份（带有效期）；注销 / 关机（`WM_ENDSESSION`）与 SIGTERM 时先落盘 |
| P6 | 同步协议（日志交换、Incr 求和、LWW） | 另立设计 |

P0 与 P1 先行的理由：运行时层负载最重、没有手改冲突，能先把引擎打磨好；文本层只是在它之上多一步补丁写回。

## 11 验证

引擎级（P0）：

- 日志尾部截断、半条记录、CRC 错：启动恢复到最后一条完整记录。
- 快照半写（tmp 残留）、state 缺失 / 损坏 / 版本不符：按快照内嵌水位恢复，无重复应用、无丢失。
- seq 续号：删除 `journal/` 后新记录 seq 单调递增。
- 折回后旧段 `strong_count` 归零；wdat 与 sidecar 之间崩溃不丢已折回的数据。
- 同键多操作（晋升）按 seq 叠加结果正确。

文本层（P2–P3）：

- 折回往返：带注释、BOM、CRLF、自定义顺序的文件，一轮 Upsert / Delete / SetWeight 后，除被改行外逐字节不变。
- 规范化：`ni hao` 行能被运行时 `nihao` 的 Delete 命中；英文大小写同理。
- 边界未知的词写回后再解析仍为 `boundary = 0`，简拼可召回。
- 编辑器竞争：折回期间文件指纹变化 → 本轮丢弃重做，无数据丢失。
- 文件缺失两种情况的区分。

等价与性能：

- 真实 `userdata.redb`（`WIND_DB` 指向副本）迁移前后，`CompositeDict` 对一批码的返回逐条相同，比产物不比摘要（延续 `union_of_dicts_equals_merged_dict` 的判据）。
- fold 换段期间按键线程持续查询不报错、不见半成品。
- 19 万词简拼召回与基线对比；折回耗时与峰值内存按 §9 核对。

## 12 开放项

- `learned` 导出视图：临时词是否提供只读文本导出供用户查看，由设置页需求决定，不影响本设计。
- Android 设置 App 是否 `android:process` 独立，需核实后决定 IPC 转发的实现位置。
- `arc-swap` 依赖的取舍见 §3.4。
