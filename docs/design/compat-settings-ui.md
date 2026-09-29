# 应用兼容性统一管理界面（设计）

状态：设计已确认，待评审 spec、待写实现计划。日期：2026-09-29。

## 1. 目标与非目标

**目标**：在设置端（`wind-setting`）新增「应用兼容性」页，统一管理 `compat.toml`，取代手改文件。

1. 区分系统默认与用户配置，可还原到系统默认（字段级 / 单条 / 全部）。
2. 后续新增兼容字段时，GUI 自动适配，不需要逐字段手写界面。
3. 用一个帮助页说明每个选项的功能与解决的问题。
4. 支持手动添加规则、导入、导出。

**范围**：`[[apps]]`（27 个字段）、`[[initial_mode_scope]]`、`[[commit_newline]]` 三段都管。

**非目标**：不做 markdown 渲染；不改变运行时的匹配维度（仍按进程名，窗口类只用于 `initial_mode_scope`）；不引入文件监听。

## 2. 现状（设计依据）

- 核心 `wind-config/src/app_compat.rs`：`AppCompatRule` 27 字段，全部容错反序列化。三层加载：`data/compat.toml` < `data_custom` < 用户层。同名进程整条覆盖，唯独 `ProtocolFields` 的 4 个字段（`composition_start_pair_guard`、`pin_anchor_when_start_drifts`、`ignore_host_ime_close`、`host_drawn_candidates`）用户未写时继承。
- 没有「禁用 / 删除系统条目」的语法；没有 compat 的 schema（字段含义只在注释里）；`wind-rpc` 没有任何 compat 方法。
- 重载只有 `Coordinator::reload_app_compat`（`handle_menu.rs:901`），且只被右键菜单路径调用；同 pid 的 `active_compat` 缓存不会自动刷新。
- 用户层写入是 `update_user_rule` → `render_user_compat` 整份重写，非原子；TOML 语法错误时整份文件被静默跳过。
- 设置端是纯 Rust 原生 GUI（`windui`），无 Wails / 前端包。manifest 驱动只覆盖全局配置；没有帮助页组件，没有进程选择器。

## 3. 架构：核心 RPC + 字段元数据（方案 A）

合并、禁用、还原语义**只在核心实现一份**；设置端只经 `compat.*` RPC 读写，不直接碰文件，也不复制合并逻辑。字段元数据由核心提供，设置端表单与帮助页读同一份。

### 3.1 规则状态

| 状态 | 含义 |
|---|---|
| `system` | 只来自系统层，未被动过 |
| `modified` | 系统规则被用户改写 |
| `user` | 用户新增 |
| `disabled` | 系统规则被用户禁用，合并后该进程无规则 |

### 3.2 两个必须先解决的语义

1. **禁用语法**：用户层条目允许 `disabled = true`，合并后该进程整条视为不存在。三段都加此字段。`is_empty_override` 与 `build_lookup` 需据此调整。
2. **写时复制**：现有整条覆盖会让「只改系统规则一个字段」丢掉其余字段（协议字段除外）。因此修改系统规则时，核心先把系统规则完整复制为用户条目再改。字段级还原 = 把该字段设回系统值；若整条与系统一致则删除用户条目。

### 3.3 RPC

| 方法 | 作用 |
|---|---|
| `compat.schema` | 字段元数据（见 3.4） |
| `compat.list` | 合并视图：每条带状态、有效值、系统值、用户改写字段集；参数 `section` 区分三段 |
| `compat.upsert` | 按进程名 + 字段补丁写入，内部写时复制 |
| `compat.setDisabled` | 禁用 / 启用一条系统规则 |
| `compat.reset` / `compat.resetAll` | 单条还原 / 清空用户层三段 |
| `compat.export` | 生成文本，范围：`user`（仅我的改动，默认）/ `effective`（含系统） |
| `compat.import` | `dryRun` 预览；`mode`：`merge`（同名覆盖）/ `replace`（先清空用户层） |

每次写入后核心自动 `reload_app_compat` 并使当前焦点进程的 `active_compat` 缓存失效（需处理 `pid_names` 复用，见 `revalidate_pid_name`），并广播 `compat.changed`。

### 3.4 字段元数据

核心 `wind-config` 新增 `compat_schema`（静态注册表），每字段登记：

| 项 | 说明 |
|---|---|
| `key` / `kind` | 字段名 / 控件类型：开关、三态（跟随全局·开·关）、枚举、整数、定位组 |
| `group` | 分组：候选窗定位、状态气泡、初始状态、宿主协议等 |
| `label` / `summary` | 界面名称 / 一句话功能 |
| `problem` | 解决什么问题（帮助页主体） |
| `hosts` | 已知需要它的软件 |
| `depends_on` | 联动条件，如 `candidate_x/y` 仅 `fixed` 模式有意义 |
| `protocol` / `advanced` | 是否协议字段（决定继承与还原行为）/ 是否折叠进「高级」 |

**守护测试**：对比 `AppCompatRule` 序列化字段名与注册表，漏登记即红。这补上「新增字段无兜底」的缺口（当前 `stale_probe_guard`、`password_force_english`、`schema`、`status_*` 就不在 `data/compat.toml` 顶部注释里）。

## 4. 设置端界面

- 新页「应用兼容性」，左右布局。左：规则列表，带搜索与筛选（全部 / 已修改 / 自定义 / 已禁用），状态徽章。右：所选规则详情。
- 顶部分段切换三段：应用规则 / 初始模式作用域 / 上屏换行。
- 通用渲染器：详情按 `group` 分卡片；每字段一行 = 名称 + `summary` + 控件 + 「已改写」标记 + 单字段还原按钮；复合字段按 `depends_on` 成组，条件不满足置灰。`initial_mode_scope.classes` 用列表编辑控件。
- 页头「全部还原」，条目菜单「还原 / 禁用 / 启用 / 删除（仅用户规则）」。
- 复用：`SubNav`、`section`/`setting_row`、`build_confirm_dialog_shell`、dict 页的 `CategorySpec`/`Feedback` 思路；Tab 注册改 `cli.rs` 的 `PAGES`、`pages/mod.rs` 的 `all_tabs`、`icons.rs` 的 `nav_icon` 三处，且顺序一致。

### 4.1 帮助页

页头「帮助」入口，按分组列出每字段的功能、解决的问题、已知宿主、取值说明，数据来自 `compat.schema`。字段行信息角标点击跳到对应条目。纯文本，长说明用分节对话框。

## 5. 手动添加、导入导出

**添加**：输入进程名，校验 = 去首尾空格、不含路径分隔符、不区分大小写；可「从运行中进程选择」（设置端枚举，去重，排除已有）；同名已存在则跳转到该条。新规则先是内存草稿，首次设置字段时才 `upsert` 落盘（空壳规则本就会被剔除）。

**导出**：标准 `compat.toml` 片段，可直接放进用户层；内容由核心生成，与落盘共用同一渲染函数。设置端只负责保存对话框（参考 `dict/state.rs` 的 `do_quick_export`）。

**导入**：选文件 → `dryRun` → 预览对话框（新增 N / 覆盖 M / 禁用 K / 无变化，逐条可看；列出被忽略的未知字段与被回落的错误值）→ 选 `merge` 或 `replace` → 确认写入。写入前备份原用户层为 `compat.toml.bak`。

## 6. 健壮性

- TOML 语法错误：导入与管理页路径**明确报错并给行号**；运行时加载仍容错但补 WARN 日志（现状静默）。
- 落盘改为临时文件 + rename。
- 设置端与右键菜单统一走核心同一入口，核心内一把锁串行化。
- 容错回落的错误值收集为 `warnings`，随 `compat.list` / `compat.import` 返回。

## 7. 测试

- 核心：合并 / 禁用 / 写时复制 / 字段还原 / 导入预览单测；元数据守护测试；`wind-rpc` 的 `FakeCore` 用例；`value_domain_guard` 元测试覆盖新增枚举。
- 设置端：`mock_reply` + `take_mock_calls` 断言 RPC 形状；补 `search/index.rs` 登记（`sections_have_placement` 等）；新汉字需重生成拼音表（本机无 pwsh，用等价流程）；交叉编译 + wine 跑测试并离屏出图。
- 跨仓：`cargo test -p wind-rpc --test wind_setting_assets` 对账。

## 8. 分期（每期独立可提交）

| 期 | 内容 | 仓 |
|---|---|---|
| P1 | `disabled` 语法、写时复制、`compat.schema`、`compat.*` RPC、守护测试、原子写、错误上报 | WindInput |
| P2 | 列表 + 详情编辑、通用渲染器、状态徽章、字段/单条/全部还原 | wind-setting |
| P3 | 手动添加、进程选择、导入导出 + 预览 | wind-setting（少量核心） |
| P4 | 帮助页、搜索索引登记、文档站说明 | wind-setting + WindInputDocs |

## 9. 已定决策与风险

- **禁用 / 启用**：禁用时用户条目保留已有的改写字段并加 `disabled = true`；启用只去掉 `disabled`，恢复到禁用前的状态（可逆）。要回到系统原貌用「还原」。`ProtocolFields` 继承只对未禁用的条目生效。
- **`data_custom` 层**：在视图里与系统层同等对待，只读，状态记为 `system`。
- **`active_compat` 缓存失效**：写入后清掉缓存的 pid 并对已连接 pid 走现有的 `Connected-pid compat refresh` 路径重算，不依赖焦点切换。实现时用靶机日志（`Compat rule for process=` 行）验证。
- **`[[initial_mode_scope]]` / `[[commit_newline]]`**：目前只透传、无写入 API，P1 补齐，同样支持 `disabled`、写时复制与还原。
- **风险**：核心合并语义变化会影响右键菜单的既有写入路径（共用 `update_user_rule`），P1 必须保持其行为不变，并用既有测试兜底。
