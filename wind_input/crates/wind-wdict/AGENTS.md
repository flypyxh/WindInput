<!-- Parent: ../../AGENTS.md -->
<!-- Updated: 2026-10-14 -->

# wind-wdict

## Purpose

wdict（`.wdict.yaml`）交换格式的读写（phrases / words / shadow / freq 各段），以及词条文本域的转义（`escape_text_field` / `unescape_text_field`，命令栏语法条目只保护分隔符）。原是 `wind-store::wdict`；P1b 抽成独立 crate，让 `wind-dict` 不必为几个文本函数依赖用户库（`wind-store → wind-userdata → wind-dict` 才能单向成立）。`wind-store` 用 `pub use wind_wdict as wdict;` 保留原路径，旧调用方不改。

## Key Files

| File | Description |
|------|-------------|
| `src/lib.rs` | 全部格式代码与测试（原 `wind-store/src/wdict.rs` 原样搬入） |

## For AI Agents

### Working In This Directory

- **不要依赖 `wind-store` / `wind-dict`**：本 crate 在依赖图底层，只依赖 `wind-utils` 与 `wind-cmdbar`。往这里加依赖前先看会不会回绕成环。
- 转义规则与命令栏判据的取舍说明见文件头注释；文本域与编码域用的是两对不同的函数。

### Testing Requirements

- `cargo test -p wind-wdict`（纯 Rust，任意 host 可跑）。

## Dependencies

### Internal

- `wind-utils` — `normalize_input`
- `wind-cmdbar` — `is_cmdbar_grammar` / `contains_marker_call`

## 全局约束

提交前跑 `cargo fmt -p wind-wdict`。
