# 系统词库缓存与索引改进

状态：设计定稿，按阶段实施。日期：2026-10-14。
姊妹文档：[user-data-storage-redesign.md](user-data-storage-redesign.md)（用户数据那条线）。

本文件只管系统词库（yaml → wdat / wridx 等派生产物）这条链路，不碰用户数据。现状架构见
`docs/architecture/rime-dict-loading.md` §4–§5。

## 0 要解决的四件事

| 痛点 | 现状出处 | 本文对应阶段 |
|---|---|---|
| 判一次缓存新鲜要读满整份源算指纹，大方案启动读几百 MB | `cache_fp::fingerprint` 流式读全部内容 | S1 |
| 用户覆盖的同名词库与自带词库共用同一缓存落点，来回切换互相踩、反复重建；mmap 期间 rename 要靠 `FILE_SHARE_DELETE` 与替换代数兜底；`.fp` 先删后写有窗口 | `user-override.md` §4；`reader_pool.rs` 头注释与 `Entry.stamp` | S2 |
| 构建期把全部条目以 owned `String` 收进内存，65 万条 130 MB、合并路径峰值 420–500 MB | `datformat.rs:430`、`manager.rs` merged 构建注释 | S3 |
| Rime 主表 + import_tables 任一子表变了就整份重解析重建 merged（雾凇约 4 s） | `load_rime_pinyin_dict` | S4 |

不做的事：

- **发布包预构建 wdat**。Android 已定「GPL 词库以原文分发、端上编译」（`WindInputAndroid/docs/architecture.md` §3.5），桌面沿用同一决策。S3/S4 把端上编译的代价压下去就够了。
- 索引产物合并（`.wridx` 与单字全码表 `charcodes`）。两者已各自落盘并按派生指纹复用，合并收益小，不排期。

## S1 指纹判新鲜的快路径

> 状态：**已实现**（2026-10-14）。实测 `wubi86_pinyin`（含雾凇拼音合并库）二次启动从源文件读取
> 19.1 MB → 0.3 MB，零重建；删掉 `.st` 即回到旧行为并在下次判新鲜时自动回写。实现时多发现一条
> 窗口：同一 mtime 刻度内两次同长度写入骗过快路径，已用 racy 窗口（源 mtime 距取 stamp 前的
> 时刻不足 2 s 不记 stamp）堵住。独立审查后又补两处：stamp 加 inode 与 ctime（Flatpak / Nix
> 把 mtime 钉死的发行形态）；racy 基准从「哈希完成后的现在」改为「取 stamp 之前的时刻」，并
> 用 `.st` 自身 mtime 按文件系统时钟复核。

**目标**：源文件没动时，判新鲜只 stat 不读内容；语义不变，仍以内容指纹为准。

**机制**：

- 新增 sidecar `<cache>.st`，内容是「这份指纹是在哪组文件状态下算出来的」：第一行为对应的指纹值，第二行解析语义版本与 tag，其后每源一行 `name \t exists \t len \t mtime_nanos \t ino \t ctime_nanos`（后两项 Unix 以外恒 0）。`.fp` 格式不变（单行指纹），老版本读到新目录零影响。
- `cache_is_fresh` 顺序：
  1. 读 `.fp`；没有则不新鲜。
  2. 读 `.st`；首行指纹与 `.fp` 一致、且每个源当前的 `(exists, len, mtime)` 与记录逐项相等 → **新鲜，不读内容**。
  3. 否则算完整指纹比对。相等则新鲜，并**重写 `.st`** 为当前状态（部署刷新了 mtime 的情况下，只多算这一次）。不等则不新鲜。
- `write_cache_fp` 同时写 `.fp` 与 `.st`；`remove_cache_fp` 同时删两个。
- 取 stamp 在哈希之前；哈希完再 stat 一次，两次不一致（并发改写）就不写 `.st`，只写 `.fp`。
- `mtime` 取不到的平台不写 `.st`，退回全量指纹。

**为什么这不违背「拒绝 mtime」**：那条原则拒绝的是**用 mtime 判定要不要重建**，部署刷新 mtime 会误判成要重建。这里 mtime 只决定**要不要重新算指纹**，误判的代价是多算一次哈希，不是多建一次库。反向风险是「内容变了但 len 与 mtime 都没变」，需要有人刻意回拨时间戳；`schema.rebuildCache` 仍是逃生口，文档注明。

**改动面**：`wind-dict/src/cache_fp.rs`。调用方零改动。

**测试**：

- 第二次 `cache_is_fresh` 不读内容（用一个只在被读时才会失败的源，比如改权限或用计数包装）。
- 只刷新 mtime 不改内容：判新鲜，且 `.st` 被重写。
- 改内容但 len 不变、mtime 变：判不新鲜。
- `.st` 首行指纹与 `.fp` 不符：忽略 `.st`，走全量。
- 源缺失的 stamp 稳定可复用；缺失转存在时失效。

**退出判据**：feihuzj2 量级方案（250 MB 源）二次启动的新鲜判定读盘量从几百 MB 降到 KB 级，用 strace / 计数包装验证。

## S2 缓存内容寻址与回收

> 状态：**已实现**（2026-10-14）。与下文设计的差异：`.fp` 不退役，仍写在实际文件旁供派生缓存
> 取摘要（省得改 `cache_digest` 的全部消费方）；回收不用 manifest，改成按槽位就地保留最近
> 2 份（`prune_slot`，沿用英文词组索引 `phrase-<fp>.wphr` 的做法），旧式非寻址文件顺手清掉。
> 实测自带版与改过一行的覆盖版交替三轮：第 1 轮各建一次，之后零重建，两份并存。
> `reader_pool` 的替换代数与 stamp 复查保留未删。wcmt / wemj 两类缓存仍是原地覆盖，未纳入。

**目标**：缓存文件名由「源内容指纹」决定，同名不同内容的源落到不同文件；文件一旦写成就不再改写。

**机制**：

- 缓存路径从 `<cache>/<ns>/<stem>.wdat` 改为 `<cache>/<ns>/<stem>-<fp12>.wdat`，`fp12` 是 `cache_fp` 指纹的前 12 个十六进制字符。`cache_path` 仍是源路径与源内容的纯函数，`reader_pool` 按路径复用的契约不变。
- 新鲜判定退化为「目标文件存在」：指纹算出来就是文件名的一部分，`.fp` sidecar 退役；S1 的 `.st` 改为记在 `<cache>/<ns>/<stem>.st`（按源命名，不带指纹），作用仍是「跳过算指纹」，内容多一行当前指向的指纹。
- 派生产物（`.wridx`、`charcodes`、英文词组索引）的 `cache_digest` 直接取文件名里的指纹，不再读 sidecar；wdat-only 投放词库仍走 `sz:len:mtime`。
- 写入仍 tmp + rename，但目标永远是新文件名，不覆盖正在 mmap 的文件。`reader_pool` 的 `(大小, mtime, 替换代数)` 复查与 `replacing` 协议因此失去存在理由，S2 内保留不删，S2 退出后另开一条清理任务删掉。
- **回收**：每个方案目录下维护 `manifest`（当前各源指向的文件名）。回收时机：构建成功写完 manifest 之后，以及启动后台预热结束时。规则：目录里不在 manifest 内、且 mtime 早于 24 小时的文件删除（24 小时是给并发进程与回滚留的余量）。Windows 上正被 mmap 的旧文件删除会失败，忽略，下次再收。
- `purge_legacy_combined`、`remove_stale_cache` 并入回收。

**改动面**：`wind-dict/src/cache_ns.rs`（命名）、`cache_fp.rs`（摘要来源）、`cached.rs`（加载流程）、`wind-engine/src/manager.rs`（`cache_path` 17 处调用方、merged / wridx / charcodes 路径、回收挂点）。

**测试**：

- 同名不同内容的两个源各得各的缓存，交替加载零重建。
- 重建期间旧 reader 继续可读，新 reader 读到新内容（替换 `rebuilt_file_is_not_served_from_stale_entry` 的角色）。
- 回收不删 manifest 内文件、不删 24 小时内的文件；删除失败不报错。
- `.wridx` 派生指纹在源变化时失效、源不变时复用，与现有测试等价。

**退出判据**：用户目录覆盖 `pinyin/rime_frost.dict.yaml` 后在两份之间切换，第二次起不重建。

## S3 流式构建：两遍写入、常量级峰值

> 状态：**S3 与 S4 的前两步已实现**（2026-10-14）：`WdatStreamWriter`（唯一构建路径，`WdatWriter`
> 委托）、`WdatReader::codes()` 有序游标、每张子表自己的段 + k 路归并生成 merged。产物与旧算法
> **逐字节相同**（oracle 测试 + 真实雾凇 md5）。实测（release，3 次一致）：
>
> | 场景 | 旧路径 | 现在 |
> |---|---:|---:|
> | 首次全量构建（7 张子表，无缓存）峰值 RSS | 420–430 MB | 320–326 MB |
> | 首次全量构建耗时（解析到 merged 可用） | 5.5–6.7 s | ~3.5 s（串行建段 1.7–2.0 s + 归并与 DAT 1.65 s） |
> | 只改一张子表后的重建峰值（段已缓存） | 420–430 MB | 匿名内存 216 MB，RSS 273 MB |
> | `build()` 本身 | 2.3–2.7 s | 1.3–1.4 s（排序与候选进池消失） |
>
> 建段改为串行：表间并行会把两张大表的解析峰值叠加（实测 498 MB）。归并期间的 RSS 里约
> 100 MB 是段的文件页（可回收）。
>
> **DAT 构建的预留与两游标也已实现**（同日）：三数组按 6 × 键数预留，不再翻倍扩容；子节点
> ≥ 4 的节点从稀疏游标起找 base。实测：简拼 DAT 0.67 s → **26 ms**，主表 DAT 220 → 193 ms，
> `build()` 1.35 → 0.63 s，merged 从写到可用 1.65 → 0.75 s；首次全量构建峰值 RSS 305–317 MB。
> 产物字节变了，对拍改为语义：`wdat_dump` 导出的主表与简拼 TSV 与旧产物逐行相同，dat_size
> 也未变（2 714 921 / 431 721）。剩余项只有 MaxW（0.4 s，现为 `build()` 大头）合并进构建一趟。

> 打点结论（2026-10-14，`build_dev` 雾凇 7 张子表，release，3 次一致；`RUST_LOG=wind_dict=debug`
> 可复现，阶段计时已留在 `datformat.rs` 的 debug 日志里）：
>
> | 阶段 | 耗时 | RSS |
> |---|---:|---:|
> | 解析 7 张子表（并行）+ HashMap 聚合 | ~2.0 s | 到 349 MB |
> | 填 `WdatWriter`（逐 code 排序 + add） | ~0.5 s | 331 MB |
> | `build()`：排序键 | 0.27 s | |
> | `build()`：主表候选进池 / 主表 DAT（52.5 万键，dat_size 271 万） | 0.18 s / 0.45–0.55 s | 峰值 **430 MB** |
> | `build()`：简拼区候选进池 / 简拼 DAT（18.4 万键，dat_size 43 万） | 0.3 s / **0.72–0.79 s** | 406 MB |
> | `build()`：MaxW 两段 | 0.4 s | |
> | `emit()` 写盘 | 0.1 s | |
>
> 三个结论改写了下面的方案：**内存大头在 `build()` 之前**（约 270 MB 是 owned 字符串、
> HashMap 聚合与写入器里的键），DAT 构建只再叠约 80 MB 瞬时（base/check/free 三数组翻倍扩容
> 的拷贝）；**写盘不是瓶颈**；**简拼 DAT 比主表 DAT 慢**，键少四倍却多花一半时间——简拼键短、
> 分支高，`find_base` 从空闲链表头起逐个试、每个候选都要查全部子节点，典型的高分支退化。

**目标**：`WdatWriter` 不再持有全部条目的 owned 字符串；构建峰值从「条目数 × 约 0.7 KB」降到「唯一码数 × 几十字节 + 固定缓冲」。

**机制**（按打点结论修订；原「两遍扫源」改为「流式写入器 + 由段归并喂入」，与 S4 合并实施）：

- `WdatStreamWriter`：要求 code **按序喂入**（S4 的 k 路归并天然按序），text 字节直接进池、
  不建 `String`；码存进一个紧凑字节缓冲加偏移表而不是 52 万个 `String`；条目行与叶行按现有
  22 B / 8 B 定长表累积。写入器常驻 ≈ 池 19 MB + 条目 14 MB + 码缓冲 8 MB，取代现在的约
  270 MB。简拼条目以 `(简拼, 码偏移, 权重)` 三元组收集后排序，不再嵌套 HashMap。
- 现有 `WdatWriter::build` 改为「排序后喂给流式写入器」，两条路径共用同一套 DAT 构建与池，
  小词库与测试零改动；同一输入两条路径逐字节相同，这是对拍的基础。
- DAT 构建：三数组按「键数 × 经验系数」预留，避免翻倍扩容的拷贝峰值；高分支节点（子节点
  ≥ 8）的 base 从独立的「稀疏游标」起找而不是从空闲链表头起，小节点继续填头部的空洞——
  darts-clone 的两游标做法。**这一条会改变 base 的取值**，产物字节不再与旧路径相同，对拍
  改为语义等价：全量 `for_each_entry` 多重集相同 + 抽样前缀查询逐项相同。
- MaxW 与 DAT 构建合并为一趟留到最后，优先级低。

原方案保留作对照：

- 第一遍只收**码**：`Vec<Box<str>>` 或 arena 里的码串，排序去重后建 DAT（base / check / MaxW 都只依赖码集与每码最大权重，最大权重在第一遍顺带记）。
- 第二遍按码序重新扫源，把每码的条目（text、weight、order、boundary）直接编码进 EntryRecords 与 StringPool 的文件流；StringPool 去重改用 `HashMap<u64, u32>`（文本哈希 → 池偏移）而不是持有文本本身，碰撞时回读校验。
- 两遍之间源必须可重扫：源是 yaml 时重解析一次（解析本身是流式的，`parse_rime_entries_parallel` 改成按块回调）；源是已有 wdat 段时（S4 的归并路径）用 `for_each_entry` 顺扫。
- 对外接口保留 `add*` 系列给小词库与测试用（走旧的全内存路径），新增 `WdatStreamWriter::build(codes_pass, entries_pass)` 给大构建路径。

**改动面**：`wind-dict/src/datformat.rs`、`codetable.rs`（流式解析回调）、`cached.rs::write_cache`、`manager.rs` merged 构建。

**测试**：纯重构，按仓规矩**md5 对拍**：同一源经旧路径与新路径产出的 wdat 逐字节相同（含 AbbrevSection、MaxW、CharMap、Meta）。基准各跑 3 次记峰值 RSS 与耗时。

**退出判据**（按打点修订）：雾凇 merged 构建的 RSS 增量（相对构建前基线）从约 370 MB 降到
120 MB 以内；`build()` 从 2.3–2.7 s 降到 1.5 s 以内（简拼 DAT 不再慢于主表）；语义等价对拍通过。

## S4 Rime 子表逐段缓存，merged 由段归并生成

> 状态：**已实现**（与 S3 同一提交，见上）。Layered 直接挂子表段的配置项未做（按需）。
> 与旧算法的唯一已知差异：音节起点 ≥64 字节的码没有边界位图（`syllable_boundary_mask`
> 整体返回 0），从段推不出简拼，而旧解析器按音节数 ≥2 仍会生成——≥13 个音节的词组实际
> 极少，构建日志里按条计数。简拼首字母按 char 取（ê / ü 之类非 ASCII 音节首字母），
> 对拍夹具含此例。源 yaml 不存在的子表跳过、不走 wdat-only 旁文件（与旧路径一致，且那份
> `.wdat` 不在 merged 的指纹里）。

**目标**：改一张子表只重解析一张；merged 的重建不再经过 yaml 解析与 HashMap 聚合。

**机制**：

- 每张 import_tables 子表各自产出一个 wdat 段（走 `CachedDict::load_at_with` 同一条路径，S2 命名，S3 构建），指纹各管各的。子表解析仍可并行。
- merged 的指纹改为各子表段指纹的有序摘要（`derived_fingerprint`，与 `.wridx` 同一机制），判新鲜只看文件名是否存在。
- merged 的构建改为**k 路归并**：各子表段按码序 `for_each_entry` 顺扫（wdat 的 DAT 遍历天然按码序），归并时同码合并条目、按权重降序重编 order、同 `(text, code)` 跨表取最大权重（与现有 `ReverseIndex::build` 的聚合语义一致，等价性由 `union_of_dicts_equals_merged_dict_for_*` 两个测试延伸锁定）；简拼区同理按简拼码归并取最大权重。归并输出直接喂 S3 的流式写入器。
- 子表段本身也可以直接作为 `CachedDict::Layered` 的层挂给拼音引擎，不生成 merged。**不默认这么做**：Layered 每层每次查询各付一次 DAT 开销，雾凇 7 张子表会把每键成本抬高约 0.5 ms。保留为配置项 `pinyin.merged = false` 供低磁盘设备取舍。
- 子表段与 merged 的回收都归 S2 的 manifest。

**改动面**：`manager.rs::load_rime_pinyin_dict`、`rime_source_paths`；`datformat.rs` 增加按码序的流式迭代器（现有 `for_each_entry` 若不保证码序则补一个）。

**测试**：

- 归并产物与旧路径（整份解析聚合）产出的 wdat md5 相同。
- 只改一张子表：只有那张子表段与 merged 重建，其余段文件 mtime 不变。
- 子表缺失（`default_enabled` 但文件不存在）的指纹稳定，与现状一致。

**退出判据**：改 `ext.dict.yaml` 一行后，拼音方案重建耗时从约 4 s 降到「ext 单表解析 + 归并」，目标 1.5 s 内；峰值按 S3。

## 实施顺序与依赖（按打点修订）

S3 与 S4 合并为一个阶段实施：先做各子表自己的段（S4 前半，直接复用 `load_at_with`，零新代码），
再做「从段 k 路归并 → 流式写入器 → merged」（S3 的写入器 + S4 的归并），最后做 DAT 构建的
两游标与预留（S3 的时间项）。每一步都可独立提交。


```
S1 指纹快路径   ──┐
                  ├──> S2 内容寻址 + 回收 ──> S4 子表逐段 + 归并
S3 流式构建     ──┘                          ↑
                                              └── 依赖 S3 的流式写入器
```

S1 与 S3 互不依赖，可并行。S2 依赖 S1 的 `.st` 机制。S4 同时依赖 S2 与 S3。

每阶段独立提交、独立可发布；每阶段都要跑 `cargo test -p wind-dict -p wind-engine --no-fail-fast` 与真实雾凇数据（`build/data/schemas/pinyin`）的 REPL 对拍。

## 风险台账

- S1：`.st` 的 mtime 粒度在部分文件系统上是秒级，一秒内两次改写源文件会漏检。缓解：`.st` 里同时记 len；逃生口 `schema.rebuildCache`。
- S2：缓存目录体积在回收前会短暂翻倍（新旧两份并存 24 小时）。磁盘紧张的设备可把余量调小。
- S3：两遍扫描让 yaml 解析做两次，CPU 多一份解析时间。解析在整个构建里占多少没有现成数据，S3 开工前先用雾凇数据打点；若解析占比过半，改为第一遍解析时把条目以紧凑编码（码索引 + 文本池偏移）暂存到临时文件，第二遍顺读临时文件而不是重解析。S4 之后 merged 的源是段，不再解析 yaml。
- S4：Rime 源里跨子表同 `(text, code)` 权重不同的聚合语义必须与旧路径逐位一致，否则候选序变化。对拍是硬门槛，不过就不合。
