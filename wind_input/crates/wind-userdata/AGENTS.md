<!-- Parent: ../../AGENTS.md -->
<!-- Updated: 2026-10-14 -->

# wind-userdata

## Purpose

用户数据引擎（设计：[docs/design/user-data-storage-redesign.md](../../../docs/design/user-data-storage-redesign.md) §3）：
「快照 + 日志 + 增量表」三种物理形态各管一类变化速度。P0 交付引擎与 `DictLayer` 适配，P1a 补上运行时领域 API（`Runtime`）与草稿层；
P1b 起 `wind-store::Store` 持有一个 `Runtime`，临时词 / 草稿 / 词频 / 联想历史的门面方法全部委托到这里
（生产状态目录 `local_dir()/state/runtime/<dir_id>/`，按用户目录分开；测试的 `Store::open` 用 redb 旁的派生目录）。P2 文本层在同一台
引擎上多一步「按行补丁写回」。

## Key Files

| File | Description |
|------|-------------|
| `src/record.rs` | 日志记录帧：`len u32 \| crc32 u32 \| body`，body 含 seq / ts / kind / target / **boundary** / schema / code / text / payload。decode 对半帧与篡改一律 `Err`，绝不 panic |
| `src/journal.rs` | 按设备的追加日志 `.wjl`：一条一次 `write(2)`，无用户态缓冲；open 时截掉坏尾巴、从最后一条完整记录续号；超长字段（>u16）写前拒；裁剪是方法，重写 + rename 后把自己的 fd 换到新文件，**永不 mmap** |
| `src/state.rs` | 折回水位 `state`（toml）与真源级原子写 `atomic_write`（tmp → fsync → rename → fsync 目录）。state 读不出 = 「水位未知」，由快照内嵌 `max_seq` 兜底，绝不整份回放 |
| `src/device.rs` | `device.toml`（device_id / 机器指纹 / dir_id）：任一不符就换号并把旧日志封存为 `.wjl.sealed`；`state/lock` 用 `File::try_lock` 做单写者，拿不到即只读（只读实例走 `load_existing`，不换号不写盘） |
| `src/delta.rs` | 两级增量表：冻结层 `Arc<[Entry]>` 有序 + 尾巴 ≤128 条；同键多操作按 seq 各自保留；声母分组索引（规则同 `wind-store::abbrev_index::group_of`，含无边界兜底组 `\u{1}` + 首字符） |
| `src/snapshot.rs` | 快照段：代号命名的 `<name>-<gen>.wdat`（`WdatStreamWriter`，简拼区按条目建键、含兜底组）+ 定长 sidecar `<name>.meta`（count / created_at / last_used / flags，按 wdat 全局条目序号对齐；头内嵌 `max_seq` 与 gen）。**sidecar 是提交点** |
| `src/engine.rs` | 门面 `UserData`：`open`（锁、设备、state、枚举快照、回放日志）、`apply`（追加 + 发布，写后立即可读）、`fold`（旧快照 + delta → 新快照 → 换入 → 摘 delta → state）、`query_exact` / `query_count` |
| `src/layer.rs` | `UserDataTempLayer` / `UserDataUserLayer`（共用内部 `WordLayer`：同名同摘要、含通配；前缀多取 delta 覆盖条数、`is_prefix` 恒 true；简拼返回声母组 + 兜底组的超集；用户词 `natural_order` = 入库顺序）与 `UserDataDraftLayer`（只精确、按 TTL 过滤、恒沉底） |
| `src/text/` | 文本真源层（P2a）：`format.rs` 解析 `user.dict.yaml`（Rime 列规则复用 `wind_dict::rime_layout`，`wind:` 扩展 `code_spacing` / `escape`）与唯一的 `normalize_key`；`patch.rs` 渲染与按行补丁（除被改行外逐字节不变）；`fold.rs` 文件层（指纹、读、带复核提交） |
| `src/runtime/` | 领域 API `Runtime`：临时词（`temp.rs`）、草稿（`draft.rs`）、词频 + 联想历史（`freq.rs`）、用户词（`user.rs`，文本即真源），语义逐条对照 `wind-store` 同名函数；不依赖 `wind-store` |
| `tests/runtime_{temp,draft,freq,user}.rs` | 领域 API 与 `wind-store` 的行为对拍 |
| `tests/text_target.rs` | 文本目标：折回写回、手改后重建、编辑器竞争、删除 vs 读不了、整文件替换、空闲折回 |
| `tests/engine_crash_paths.rs` | 引擎级：写后可读 / 重开回放、折回后不再回放、state 缺失按快照水位、折回期间新到的操作保留、第二实例只读且不写盘、删 journal/ 不倒号、快照坏整份回放、换号后封存日志仍可见且续号。折回半写崩溃在 `engine.rs` 单测（需 `snapshot::FAIL_BEFORE_META` 故障注入） |

## For AI Agents

### Working In This Directory

- **读路径无锁**：按键线程 `UserData::view()` 克隆一个 `Arc<View>` 后不再碰锁。消费方只借用，不把 `Arc<View>` 或其中的 reader 存进长寿结构——否则折回后的旧段释放不掉。
- **写路径一把写锁**：`apply` 持它跨「追加 + 发布」，`fold` 持它跨「取水位 + 取起点」与「换入」。这保证折回起点视图含全部 `seq ≤ 水位` 的操作；折回只摘 `seq ≤ 水位` 的该 (target, schema) 条目，期间新到的留下。改这两处的持锁范围前先看 `fold_keeps_ops_that_arrived_during_fold`。
- **回放水位取两者较大**：快照内嵌 `max_seq`、state 里的 `folded_seq`；快照打不开（不是缺失）的目标水位不可信，整份回放。`next_seq` 取 state、各快照 `max_seq`+1、各 `folded_seq`+1、日志尾（含封存日志）四者最大（`Journal::bump_next_seq`）——删掉 `journal/` 或换号后序号都不会倒退。
- **封存日志照常回放**：换号时旧日志改名 `.wjl.sealed`，open 把它们与当前日志按 seq 合并回放，未折回的记录仍可见；新日志接着旧号编。
- **换段不覆盖**：快照每次折回落一个新代号文件，sidecar 原子写完才算换入；崩溃在两步之间旧段原样可读。别改回「同名覆盖 wdat」——那会让 `reader_pool` 按 (大小, mtime) 复用旧 reader，也会让半写状态把整个目标清零。
- **只读实例零写入**：拿不到锁就不开日志写端、不换号、不截尾（尾巴可能是写者正写的那条）。
- **记录带 boundary**：拍平的码推不回音节边界，而 delta 的简拼分组索引要它。写入端（P1 的协调器）必须传真实位图；位图是**字节**位置，`nihaoma` = `0b100101`（0、2、5），不是 `0b10101`——计划阶段的夹具就错过一次。
- **零条目快照只有 sidecar**：`snapshot::build` 遇零条目只写带水位的 sidecar（gen = 0）并清掉代号文件；`open` 对「sidecar 不存在」给 `None`（零段），对「头坏 / 指向的 wdat 不在 / 条目数对不上」给 `Err`（当零段并整份回放）。
- **叠加语义只有一份**：`engine::apply_ops`（表见 `docs/design/user-data-storage-plan-p1a.md` Task 1）。读、折回、层都经 `overlay_entries` 调它——别在别处再写一份 Upsert/Delete 的 match。
- **批量操作走 `fold_with`**：清空、导入、草稿过期 / 淘汰都是「旧快照 + delta → policy → 新快照」原子换段，水位随之推进；不要逐条写 Delete。按键路径的单条写走 `apply` / `apply_batch`。
- **文本目标**（`bind_text` 绑定的 (目标, 方案)）分两种格式（`TextBinding.format`）：`Words`（用户词 `user.dict.yaml`，计数 / 时间不进文本，只有 Incr / Touch 且文件未被手改时折回不重建段）与 `Table`（学到的数据，`text/codec.rs` 的列规格，计数 / 时间都进文本，按键补丁）。`fold` 自动走 `fold_text`（§3.5 次序：读 + 指纹 → 补丁 → tmp + fsync → 复核原文件指纹 → `.bak` + rename + fsync 目录 → 由文本重建段 → 换入 → state 记 `text_fp`）。`fold_with` 对 `Table` 目标改为由文件当前内容 + 待处理操作整文件写回（文件解析不了 ⇒ 报错不写），对 `Words` 目标报错（清空 / 导入用 `rewrite_text`）；显式清空用 `clear` / `replace_text_entries`（坏文件先另存 `.corrupt-<时间>`）。手改文件后 `sync_text` 由文本重建段，水位不回退。文本目标空闲 `TEXT_IDLE`（30 s）或最老待处理写入超过 `TEXT_MAX_PENDING`（5 min）折回；失败按目标退避，压实不重试退避中的目标。
- **绑定规则与采纳**：`set_text_rule` 设规则后，维护 / 压实 / `fold_with` 前把规则管的目标自动绑定。绑定一个从没归文本管过、文件也不在、但引擎里有数据的目标时，先由当前视图渲染出文本（`adopt_view`）、成功后才登记绑定——绝不由「文件不存在」推出「数据为空」。
- **日志**：一条一次 `write`；后台节拍每 5 秒有新写入就 fsync（锁外做）；写失败的原因记在 `write_error()`（不含内容），设置页经 `storageStatus` 提示。锁 API 报错时退化为只读。
- **文件问题登记**：`issues.rs` 进程级登记坏行数 / 隔离 / 已重建，只记路径与计数。
- **归文本管 = 已绑定，或 state 记过 `text_fp`**：重开后绑定前，这类目标 `fold` / `fold_with` 报错，`maintain` / `compact` 跳过——它们的操作只能绑定后经 `fold_text` 进文本。所以接线时 `bind_text` 要先于对它的显式折回。只读实例 `bind_text` 只登记、不同步。
- **文本折回失败**（隔离 / 读不了 / 编辑器竞争）只拦该目标：WARN + 退避（30 s 起翻倍，封顶 30 min），别的目标与压实照常。
- **小表 `text::table`**（短语 / Shadow 用）：按列名读写的 TSV；`TextTable` 内存持有最近一次成功解析的内容（读零 IO），`edit` 先按文件现状重新解析（手改优先）再按组补丁、整文件提交，文件隔离时拒写、读照旧；`replace_all` 整文件替换。`# no comment` 之后不含 tab 的 `#` 行仍算注释。
- **空闲折回靠节拍**：`spawn_ticker(every)`（只持 `Weak`、只读不起）每拍看 `needs_maintain`，需要就 `spawn_maintain`——没有后续写入时文本也会在空闲后写回。生产由 `Store::spawn_userdata_ticker` 起。
- **拼音文本里单个汉字的无空格码 = 单音节（0b1）**：空格载体表达不了 0b1，一个汉字只对应一个音节；非汉字单字符（emoji）仍为 0。
- **导入分类**：`classify_user_rows` / `user_row_effect` 与 `import_user_rows` 同一判据（权重更大或补上 0 边界 = updated；同批重复键按「本批已合并」判），预览与落盘一致。
- **写出的文件正文首行是 `# no comment`**（文本列在行首，`#` 开头的词不能被读成注释）；往没有它的旧文件追加 `#` 开头的行时补一行。
- **sidecar 每行带 `order`**（28 字节，头 version 2）：文本目标 = 行号，用户词层的 `natural_order` 取它；`order` 与 wdat 叶内序号是两回事（后者仍是 sidecar 定位的依据）。
- **时间单位**：sidecar 与领域 API 是 unix 秒（同 `wind-store`）；日志 `ts_ms` 是毫秒。
- **后台折回**：写入方在写完后调 `spawn_maintain()`（单飞，只读时不起）；阈值 `FOLD_THRESHOLD` 条、日志 `JOURNAL_MAX_BYTES`。
- 日志记录格式 v1 **尚未发布**，改字段直接改编码即可；一旦随版本发出去就要加版本分支。
- 目标键 `"{target}:{schema}"` 既是 state 的 key，也是快照目录 `state/<schema>/<target>.{wdat,meta}`；`schema` 是**数据方案 id**（拼音族折叠为 `pinyin`）。

### Testing Requirements

- `cargo test -p wind-userdata`（Linux 原生可跑；`File::try_lock` 需工具链 ≥ 1.89）。
- 测试目录名带 pid；用 `TMPDIR` 隔离并发会话。`fold_keeps_ops_that_arrived_during_fold` 是竞态用例，改写锁后连跑 10 次。

## Dependencies

### Internal

- `wind-dict` — `WdatStreamWriter` / `WdatReader` / `reader_pool`、`DictLayer` / `LayerType`
- `wind-candidate` — `Candidate`、排序 `better`

### External

- `crc32fast` — 记录帧校验
- `memmap2` — sidecar 映射（快照 wdat 经 `reader_pool`）
- `toml` / `serde` — state、device.toml
- `uuid` — device_id
- `anyhow` / `tracing`

## 全局约束

提交前跑 `cargo fmt -p wind-userdata`；日志 INFO 级不得含词条内容（见根 `AGENTS.md`）。

<!-- MANUAL: 此行以下为人工补充区，重新生成时保留 -->
