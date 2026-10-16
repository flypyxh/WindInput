<!-- Parent: ../../AGENTS.md -->
<!-- Updated: 2026-10-16 -->

# wind-store

## Purpose

用户数据门面。**全部用户数据的真源都在用户目录 `<userdata>/data/`（`StateLoc.text_dir`，常量 `DATA_DIR_NAME`）**，复制用户目录即带走全部数据（设计 `docs/design/user-data-storage-redesign.md` §2.1 / §2.4 / §2.5）：
- 用户词 `data/<schema>/user.dict.yaml`，学到的数据 `temp.dict.yaml` / `draft.dict.yaml` / `freq.table.yaml` / `assoc.table.yaml`，都是 `wind-userdata` 引擎的文本目标，门面委托 `Runtime`（方法签名不变）；学到的数据按 `learned_text.rs` 的规则自动绑定。
- Shadow `data/<schema>/shadow.dict.yaml`、短语 `data/phrases.dict.yaml` 是文本小表（`TextTable`，内存持有、按组补丁）。
- 快捷格式 `data/quick_format.toml`、常用字 `data/common_chars.yaml`、字符集 `data/charsets/<key>.yaml`（`TextDoc`，写入带指纹复核，坏了按最后一次好数据服务）；补全 `data/completion.table.yaml`、统计 `data/stats/*.toml`（机写文件，坏了 `.bak` 自愈或另存重建，`machine_file.rs`）。
- redb 只供旧库链（P1–P4a）迁移读；迁完（`meta.toml` 的 `format_version` ≥ 5）不再打开，并改名 `userdata.redb.legacy`。

三个查询层 `StoreUserLayer` / `StoreTempLayer` / `StoreDraftLayer` 也在本 crate，都是引擎层的薄包装。

## Key Files

| File | Description |
|------|-------------|
| `src/store.rs` | `Store` 核心：打开引擎、按规则绑定文本、跑旧库迁移；redb `Database` 只在旧库链未完成时打开（`with_db` 只剩迁移读与测试造旧库用） |
| `src/learned_text.rs` | 学到的数据（temp / draft / freq / assoc）落 `data/<schema>/` 的绑定规则 |
| `src/legacy_retire.rs` | 旧库链迁完：删 `migrated-*` 标记、`userdata.redb` 改名 `.legacy`（不覆盖已有） |
| `src/machine_file.rs` / `src/last_good.rs` | 机写文件的 `.bak` 自愈 / 另存重建；手改小表的「最后一次好数据」 |
| `src/user_words.rs` | 用户词：门面委托 `Runtime`（文本 + 引擎）；`count` / `created_at` 恒 0；`sync_user_text(s)` / `reload_user_text_paths`（`dict.reload`）/ `flush_user_data`（`dict.flush`）；旧表编解码只剩迁移用 |
| `src/temp_words.rs` | 临时学习词：门面委托 `Runtime`；`promote_temp_word` 先写用户词再删临时词 |
| `src/abbrev_index.rs` | 用户词 / 临时词简拼：包引擎层 `search_abbrev`（redb 倒排表 P2 起退役） |
| `src/draft_words.rs` / `src/assoc_history.rs` | 草稿、联想历史：门面委托 `Runtime`；`dec_draft` 只剩迁移读旧表用 |
| `src/store_layer.rs` | 三个 `DictLayer`：包 `UserDataUserLayer` / `UserDataTempLayer` / `UserDataDraftLayer` |
| `src/migrate_p1.rs` | 首次打开时把旧表 temp_words / draft_words / freq / assoc_history 合并进引擎，完成写 `state/migrated-p1`；`legacy_counts` 供 `examples/p1_migrate_check.rs` 比对 |
| `src/migrate_p2.rs` | 首次打开时把旧表 user_words 迁成文本（文件不在整写、在则逐条合并），完成写 `state/migrated-p2`；`examples/p2_migrate_check.rs` 逐码比对 |
| `src/phrases.rs` | 用户短语（**全局，不分方案**）：叠加模型——系统短语只在内存（`set_system_phrases`，协调器每次启动交进来），文件只记用户行与 `overrides_system=1` 的修改行；`set_phrase_enabled` 软删除 |
| `src/shadow.rs` | `ShadowRecord`（pinned/deleted）规则落 `user/<schema>/shadow.dict.yaml`：同编码一组行，pin 在前（newest first）、delete 在后，`position` 是槽位；`get_shadow_rules` 只读内存（按键路径零 IO）；`cand_id` 精准去重（动态短语 R2） |
| `src/migrate_p3.rs` | Shadow 打开时迁（标记 `migrated-p3-shadow`）；短语在首次 `set_system_phrases` 时迁（要 TOML 区分系统行改没改过，标记 `migrated-p3-phrases`）；`format_version_reached` 汇总 1–4 |
| `src/freq.rs` | `FreqRecord`；门面委托 `Runtime`；`FreqProfile` 拼音衰减分；legacy `FreqTracker`（过渡期文件式，待移除）；`dec_freq` 只剩迁移读旧表用 |
| `src/migration.rs` | 版本迁移框架；当前 `CURRENT_VERSION=1`，全新库直接打版本号 |
| `src/stat_collector.rs` / `src/stats.rs` | 每日打字统计：按月 `data/stats/YYYY-MM.toml`，元数据 `data/stats/meta.toml`（缺失 / 坏了由每日数据重算，不归零）；收集器 30 秒写一次 |
| `src/quick_format.rs` / `src/common_chars.rs` / `src/charsets.rs` | 覆盖表：`data/quick_format.toml`、`data/common_chars.yaml`、`data/charsets/<key>.yaml`；备份包里快捷格式仍是 YAML |
| `src/completion.rs` | 补全历史（邮箱后缀 / 网址 / 快捷输入）：内存持有、去抖 2 秒落 `data/completion.table.yaml` |
| `src/migrate_p4.rs` | 剩余旧表迁成文件（每类目标已存在即跳过），标记 `migrated-p4`；`legacy_put_*` 测试造旧库用 |

## For AI Agents

### Working In This Directory

- **临时词 / 草稿 / 词频 / 联想历史不在 redb 里了**（P1b）：这四组门面方法委托 `self.runtime()`，**不经 `with_db`**——按键路径不再碰 redb Mutex（`temp_hot_path_does_not_touch_redb` / `draft_freq_assoc_hot_paths_do_not_touch_redb` 用 `pause()` 守着）。旧表 `TEMP_WORDS` / `TEMP_ABBREV` / `DRAFT_WORDS` / `FREQ` / `ASSOC_HIST` 只读保留到 P4，**不要再往里写**；`words_generation` 系列代次仍由门面推进，时机与旧实现逐条一致。
- **状态目录**：`Store::open(path)` 把引擎状态放在 `<path>.state/`，redb 文件此前不存在时先清空它（测试反复用同一路径不串数据）；生产走 `Store::open_with_state(path, StateLoc)`（协调器传 `Config::state_dir()/runtime/<dir_id>`——按用户目录分开，换目录不串数据），从不清空。迁移**合并**而非替换：重做时引擎已有条目为准、count 取大，上一轮失败后的写入不丢；`migrated_p1()` 为真协调器才升 `format_version`。
- **迁移是否完成只认 `meta.toml` 的 `format_version`**（≥ 5 = 旧库链迁完，不再打开 redb、不重迁）。`state/migrated-*` 标记只在旧库链进行中作断点续做，写到 5 时删除。
- **数据目录**：`StateLoc.text_dir`（生产 `<userdata>/data`，派生模式 `<redb>.user/`，随新库清空）。`Store` 打开时把「目录下有文本的 ∪ 引擎里有用户词的」方案都 `bind_text`；其余在首次访问时 `ensure_user_text` 按需绑定（文本隔离 / 读不了只 WARN，绑定照样登记）。`lowercase` 恒 false，`syllable_codes` = 方案为 `pinyin`。改动停手 30 秒由后台节拍（`spawn_userdata_ticker`，协调器起）写回；手改文件在切方案 / 配置重载 / `dict.reload` 时 `sync`，变了推进代次。
- **P4a 起运行期不再写 redb**：它只供 P1–P4a 的迁移读旧表（以及 `schema_version`）。页缓存回收那套机制与测试（`drop_page_cache.rs` 等，负载用 `legacy_put_completions` / `legacy_scan_completions`）原样保留到 P4b 删 redb 时一起删。
- **小表写入是同步的整文件提交**（几毫秒）：短语 / Shadow 都是用户逐条的显式动作；批量（导入、还原、迁移）在内存里攒好再**一次**提交，别逐条调 `pin_shadow` / `add_phrase`。手改文件经 `sync_user_texts`（返回值含 `"phrases"` 时协调器重建短语层）生效。

- **redb 只剩迁移读**：旧库链迁完后 `db` 为 `None`，`with_db` 返回 `"store is paused"`。新代码不要再经 `with_db` 读写用户数据；仍要碰旧表时（迁移）必须经 `with_db`，不要直接访问 `Store.db`。

- **redb 的读缓存是只涨不落的高水位，全额计进进程私有内存**：redb 2.x 不是 mmap，页是堆上的 `Arc<[u8]>`；每次读未命中就插入，只有越过上限才淘汰，唯一的整体清空是库文件扩容，刷盘时脏页还会被直接晋升进读缓存。上界 ≈ `min(读缓存配额, 库文件大小)`——实测 19 万词的库全表扫后常驻 41 MB 不还，50 万词约 90 MB，这是「导入大词库后内存高」的正主（`tests/redb_cache_high_water.rs`）。回收分两种：**批量写**（导入、清空、备份还原、删方案）成功后立即调 `Store::drop_page_cache`；**全表扫描**（设置页列表 / 导出 / 统计、后台建 `UserTextIndex` / `UserAssocIndex`）做完只标记（`mark_scan_pending`，建索引走带 1 万行门槛的 `mark_scan_pending_rows`），由回收线程在库空闲 3 秒后回收并整理堆——打字中自动造词反复触发重建，当场回收会让按键线程陪等关库重开；连续 60 秒没人碰库则无条件兜底回收一次。`drop_page_cache` **绝不能进按键链路**，它要关库重开 `Database`（关库刷盘，本机约 6 ms）。
- **要条数就用 `count_*`，要一页就用 `for_each_*`，别 `list(..).len()` 或先全收再 `skip/take`**：19 万条的库翻第一页会先造 38 万个 `String`。`for_each_user_word` 的回调收 `UserWordView`（`code`/`text` 直接借 redb 的页），只对真正要留的那几条调 `to_record()`。⚠️ 新增计数/遍历 API 时，**口径必须与对应的 `search_*` 逐条一致**（含「解不出的坏记录算不算」），否则症状是「说有 N 条、翻到最后一页只有 N-1 条」，而且只在库里真有坏记录时才出现——守门在 `tests/counts_match_listings.rs`，它不比具体数字，只比两条路彼此相等。
- **这两件事是分开的**：流式遍历省的是 Rust 侧的物化峰值，**不省 redb 读缓存**（range 迭代照样把走过的叶子页读进缓存，key 与 value 同页）。只改其中一样就以为内存问题解决了，是这条最容易踩的坑。
- **复合 key 编码是范围查询的基础，不可改**：用户词/临时词/词频统一用 `schema\0code\0text` 三段；Shadow 用 `schema\0code` 两段；短语用 `code\0text`（全局，无 schema 前缀）。前缀范围查用 `t.range(scan..)` + 前缀 break，依赖此编码结构——改分隔符或字段顺序会静默读到跨方案数据。

- **短语软删除走 `set_phrase_enabled`，不走 Shadow delete**：短语候选的屏蔽语义是"用户在设置 UI 可恢复的禁用"，存 `PhraseRecord.enabled=false`；`Shadow.delete` 只用于系统词库/用户词候选。两条路径不混用，否则设置 UI 的启用开关无法正确反映状态（见 `shadow.rs` 顶部注释）。

- **Shadow 有两套实现，新代码用 `Store` 路径**：`ShadowStore`（内存+JSON 文件）是 coordinator 过渡期残留；`Store::pin_shadow` / `delete_shadow` / `remove_shadow_rule` 是正式实现（`data/<schema>/shadow.dict.yaml`）。两者共用 `ShadowRecord.apply_*` 方法保持逻辑一致。

- **`FreqTracker` 是 legacy，不要在新代码中引用**。词频记录走 `Store::record_freq` / `get_freq`（引擎 + `data/<schema>/freq.table.yaml`，按方案隔离）。

### Testing Requirements

- `wind-store` 无 `windows` crate 依赖，可在任意 host 直接运行：`cargo test -p wind-store`。
- 各测试在 `std::env::temp_dir()` 创建带唯一名的 `.redb` 临时文件，测后删除；并发运行时文件名已区分，不会互相干扰。

## Dependencies

### Internal

- `wind-userdata` — 运行时引擎（四组数据的后端）
- `wind-dict` / `wind-candidate` — `DictLayer` 与候选类型（`store_layer.rs`）
- `wind-wdict` — 交换格式（`pub use wind_wdict as wdict` 保持原路径）
- `wind-cmdbar` / `wind-utils`

### External

- `redb` — 只供旧库迁移读（P4b 删除）
- `serde` / `toml` / `serde_yaml` — 统计与快捷格式（TOML）、覆盖表（YAML）；`serde_json` 只剩备份包 jsonl 与迁移读
- `uuid` — 设备 ID 生成（Meta 表）
- `tokio` — 异步运行时（stat 相关异步路径）
- `chrono` — 日期处理（每日统计 key）
- `anyhow` / `thiserror` / `tracing` — 错误处理与日志

## 全局约束

提交前跑 `cargo fmt`；日志 INFO 级不得含用户输入/词库内容（见根 `AGENTS.md`）。

<!-- MANUAL: 此行以下为人工补充区，重新生成时保留 -->
