# 用户数据存储重设计：P2a 实施计划（文本层：格式、补丁写回、文本目标折回）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `wind-userdata` 能以 `user.dict.yaml` 为真源承载用户词：解析（含 `wind:` 扩展、转义、边界语义）、唯一的匹配键规范化、按行补丁写回（除被改行外逐字节不变）、文本目标的折回与重新同步，以及与 `wind-store` 用户词语义对拍的领域 API 和用户词层。P2b 再把 `wind-store` 的用户词门面接过来、迁移、加 `dict.reload`。

**Architecture:** `wind-dict` 开一个最小公开接口 `rime_layout`（正文起点、`# no comment` 截断点、列下标），文本层复用它而不另抄 Rime 规则。`wind-userdata` 新增 `text` 模块：`format.rs`（解析 + `normalize_key`）、`patch.rs`（渲染新文件、按行补丁）。引擎侧给 (目标, 方案) 绑定文本文件（`TextBinding`），被绑定的目标折回时走 `fold_text`（§3.5 持久化次序：读 + 指纹 → 补丁 → tmp + fsync → 复核指纹 → `.bak` + rename + fsync 目录 → 由文本重建段 → 换入 → state），`sync_text` 在文件被手改后由文本重建段而不回放已折回的记录。快照 sidecar 每行加 `order`（行序），用户词层据此给 `natural_order`。`Runtime` 加用户词 API。

**Tech Stack:** Rust 2024；`serde_yaml`（工作区已有）解析头部 `wind:`；`wind-wdict` 的 `split_spaced_code` / `join_code_by_boundary` / `escape_text_field` / `unescape_text_field`；`crc32fast` 做内容指纹。

**Spec:** `docs/design/user-data-storage-redesign.md` §2.2（词条文件格式）、§2.3（解析与规范化）、§3.5（文本目标的持久化次序与冲突规则）、§3.6（`text_fp`）、§4（用户层）、§11（文本层验证）。对照行为：`wind_input/crates/wind-store/src/user_words.rs`、`store_layer.rs::StoreUserLayer`。

## P2 拆分

| 计划 | 内容 |
|---|---|
| **P2a（本计划）** | `wind-dict::rime_layout`；`text` 模块（解析、规范化、渲染、补丁）；sidecar 加 `order`；引擎文本绑定、`fold_text`、`sync_text`、`rewrite_text`、空闲折回；`Runtime` 用户词 API；用户词层 |
| P2b（另写） | `wind-store` 用户词门面委托（`add/remove/update/list/import/clear/export`、简拼、晋升写用户词）、redb `USER_WORDS` → `user.dict.yaml` 迁移、`format_version` → 3、`dict.reload` / `dict.flush` IPC、启动 / 切方案 / 配置重载时 `sync_text`、用户分库（只读 `*.dict.yaml`） |

## 计划内裁定

- **文本只存 `text / code / weight` 三列**（`columns: [text, code, weight]`）。用户词的**入库顺序**由行序表达（sidecar 新增 `order`，用户词层的 `natural_order` 取它）；`count` / `created_at` 不进文本——`count` 只被晋升求和与联想索引的聚合计数用到，`created_at` 设置页不展示、不排序（`dict.listPaged` 只按 code / text / weight 排）。代价：用户词的「被选次数」在 P2b 迁移后归零；它不驱动排序（`on_word_selected` 生产无调用方）。
- **文本目标的段放在引擎状态目录**（`state/runtime/<dir_id>/<schema>/user-<gen>.wdat`），沿用快照「sidecar 为提交点」机制，不另放 `cache/`。它可随时由文本重建（`sync_text`），丢了不丢数据。代价：「清缓存」删不到它（几 MB）。
- **`sync_text` 总是算内容指纹**（crc32 + 长度），不走「长度 + mtime」快路径：用户词文件几 MB，算一遍毫秒级，且免掉 `rsync -t` 保 mtime 的漏判。代价：每次同步多读一次文件。
- **文本目标不接受 `fold_with` 的批量策略**：清空 / 导入 / 迁移走 `rewrite_text`（整文件渲染 + 重建段 + 推进水位），与补丁写回分开；`fold_with` 对已绑定文本的目标直接报错，免得绕过真源。

## Global Constraints

- 规范化函数唯一：`text::normalize_key(code, text, lowercase) -> (flat_code, boundary, text)`。解析建段、delta 匹配、补丁匹配三处都只用它（§2.3）。
- 读入剥 BOM；写回保持原 BOM 与换行风格（CRLF / LF 按文件第一个换行判定）；新文件写 LF、无 BOM。
- 非 UTF-8 或头部 YAML 解析失败 ⇒ 「隔离」：不往里折回（`fold_text` 返回 Err、delta 保留、state 不推进），WARN 一次；仍尝试按已有段提供查询。
- 补丁写回除被改行外**逐字节不变**（含注释、空行、自定义顺序、行尾空白）。
- 持久化次序严格按 §3.5；`.bak` 保留上一版；重命名失败保留 delta、不推进 state。
- 写回的码一律 `join_code_by_boundary(flat, boundary)`；边界未知（0）输出无空格码。
- `code_spacing: syllable` 时无空格码 ⇒ `boundary = 0`（不是 `0b1`）；未声明 ⇒ 码恒无边界（`boundary = 0`）。
- `escape: true` 时 text 列走 `unescape_text_field` / `escape_text_field`（含命令栏文本分流）。
- INFO 级日志不得含词条内容。
- 提交走 `git commit -F - --only -- <路径>`；新文件先 `git add`；`cargo fmt -p <crate>`。
- 工作目录：worktree `本分支的 worktree`（分支 `feat/user-data-storage`）。测试：`cd wind_input && CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录> cargo test -p <crate> --no-fail-fast`。

## Review Focus

1. **手改过的文件**（BOM、CRLF、注释、空行、自定义顺序）经一轮 Upsert / Delete / SetWeight 后，除被改行外逐字节不变。→ Task 3 `patch_preserves_everything_but_touched_lines`。
2. **规范化**：文件里写 `ni hao` 的行能被运行时 `nihao` 的 Delete 命中；英文大小写在 `lowercase` 方案同理。→ Task 3 `delete_hits_spaced_line_by_flat_key`。
3. **编辑器竞争**：折回读完文件后、rename 之前文件被保存 ⇒ 本轮放弃，delta 与 state 不动，手改与运行时写入都不丢。→ Task 5 `editor_save_during_fold_discards_the_round`。
4. **文件缺失两种情况**：被删除（目录在、文件不在）⇒ 段清空、水位不回退、之后的 Upsert 新建文件；读不了（权限 / 占用）⇒ 本轮跳过、delta 保留。→ Task 5 `deleted_file_vs_unreadable_file`。
5. **边界未知的词**写回后再解析仍为 `boundary = 0`，简拼经无边界兜底组可召回。→ Task 3 `unknown_boundary_roundtrips_as_zero` + Task 6 `user_layer_recalls_unknown_boundary_by_first_char`。

---

### Task 1: `wind-dict::rime_layout` 公开接口

**Files:**
- Modify: `wind_input/crates/wind-dict/src/codetable.rs`（新增 `pub struct RimeLayout`、`pub fn rime_layout`）
- Modify: `wind_input/crates/wind-dict/src/lib.rs`（`pub use codetable::{RimeLayout, rime_layout};`）
- Test: `codetable.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub struct RimeLayout {
      /// 正文起点（`...` 行之后）的字节偏移，相对整篇内容。
      pub body_offset: usize,
      /// `# no comment` 指令行的起点（相对整篇内容）；其后的 `#` 行是数据。None = 全篇 `#` 行皆注释。
      pub comment_cutoff: Option<usize>,
      pub text_col: usize,
      pub code_col: usize,
      /// None = 声明了 `columns:` 但不含 weight。
      pub weight_col: Option<usize>,
  }
  pub fn rime_layout(content: &str, path: &std::path::Path) -> Option<RimeLayout>
  ```
  无正文标记 `...` 或列解析失败 ⇒ None。注释判定规则（供调用方逐行用）：`trim` 前以 `#` 开头且（无截断点或行起点 ≤ 截断点）。

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn rime_layout_exposes_body_columns_and_comment_cutoff() {
        let content = "---\nname: x\ncolumns:\n  - code\n  - text\n  - weight\n...\n# 注释\nni\t你\t1\n# no comment\n#hash\t井\t2\n";
        let l = rime_layout(content, std::path::Path::new("x.dict.yaml")).unwrap();
        assert_eq!(&content[l.body_offset..l.body_offset + 4], "# 注");
        assert_eq!((l.text_col, l.code_col, l.weight_col), (1, 0, Some(2)));
        let cut = l.comment_cutoff.unwrap();
        assert!(content[cut..].starts_with("# no comment"));
        assert!(rime_layout("no body marker", std::path::Path::new("y")).is_none());
        let d = rime_layout("---\n...\n你好\tnihao\n", std::path::Path::new("z")).unwrap();
        assert_eq!((d.text_col, d.code_col, d.weight_col), (0, 1, Some(2)), "未声明 = librime 默认列序");
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录> cargo test -p wind-dict rime_layout 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`rime_layout` 未定义。

- [ ] **Step 3: 实现**

```rust
/// 文本层（`wind-userdata::text`）复用的 Rime 正文布局。规则只在本文件一份：正文起点
/// （[`rime_body_offset`]）、`# no comment`（[`find_no_comment_directive`]）、列规格
/// （[`resolve_columns`]：`columns:` 声明优先，否则探测）。
pub struct RimeLayout {
    pub body_offset: usize,
    pub comment_cutoff: Option<usize>,
    pub text_col: usize,
    pub code_col: usize,
    pub weight_col: Option<usize>,
}

pub fn rime_layout(content: &str, path: &Path) -> Option<RimeLayout> {
    let body_offset = rime_body_offset(content)?;
    let body = &content[body_offset..];
    let cutoff = find_no_comment_directive(body);
    let spec = resolve_columns(content, body, path, cutoff)?;
    Some(RimeLayout {
        body_offset,
        comment_cutoff: cutoff.map(|c| c + body_offset),
        text_col: spec.text_col,
        code_col: spec.code_col,
        weight_col: spec.weight_col,
    })
}
```

`lib.rs` 加 `pub use codetable::{RimeLayout, rime_layout};`（`codetable` 若非 `pub mod`，就只加这条 `pub use`）。

- [ ] **Step 4: 运行确认通过**：同 Step 2 命令，Expected PASS；再跑 `cargo test -p wind-dict --no-fail-fast` 无新增失败。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-dict && cd .. && git commit -F - --only -- wind_input/crates/wind-dict <<'EOF'
feat(dict): 公开 rime_layout（正文起点、no comment 截断点、列下标）供文本层复用
EOF
```

---

### Task 2: 文本解析与 `normalize_key`

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/mod.rs`、`wind_input/crates/wind-userdata/src/text/format.rs`
- Modify: `wind_input/crates/wind-userdata/src/lib.rs`（`pub mod text;`）、`wind_input/crates/wind-userdata/Cargo.toml`（加 `wind-wdict`、`serde_yaml`）
- Test: `format.rs` tests

**Interfaces:**
- Consumes: Task 1 的 `wind_dict::rime_layout`；`wind_wdict::{split_spaced_code, unescape_text_field}`；`wind_utils::text::normalize_input`（若 `wind-userdata` 尚未依赖 `wind-utils`，一并加）。
- Produces（`text::format`，经 `text/mod.rs` re-export）:
  ```rust
  #[derive(Clone, Debug, Default, PartialEq)]
  pub struct WindHeader { pub kind: Option<String>, pub syllable_codes: bool, pub escape: bool }
  #[derive(Clone, Debug, PartialEq)]
  pub struct TextEntry { pub line: usize, pub code: String, pub text: String, pub boundary: u64, pub weight: i32 }
  #[derive(Clone, Debug)]
  pub struct TextDict {
      pub header: WindHeader,
      pub entries: Vec<TextEntry>,   // 文件顺序
      pub bom: bool,
      pub crlf: bool,
  }
  pub enum ParseError { NotUtf8, BadHeader(String), NoBody }
  pub fn normalize_key(code: &str, text: &str, lowercase: bool) -> (String, u64, String)
  pub fn parse(bytes: &[u8], path: &Path, lowercase: bool) -> Result<TextDict, ParseError>
  ```
  - `normalize_key`：`split_spaced_code(code)` 得扁平码与边界（无空格 ⇒ 0）；`lowercase` 时扁平码小写；text 过 `normalize_input`。**`code_spacing: syllable` 未声明时**，调用方把边界当 0（码不承载音节）。
  - `line` 是该行在「按 `\n` 切开的全部行」里的下标（含头部行），补丁按它定位。
  - 空文件 / 只有头 ⇒ `entries` 为空，不是错误；无 `...` ⇒ `NoBody`；非 UTF-8 ⇒ `NotUtf8`；`---` 与 `...` 之间的头部 YAML 解析失败 ⇒ `BadHeader`。

- [ ] **Step 1: 写失败测试（format.rs tests）**

```rust
    use std::path::Path;

    fn p(s: &str, lower: bool) -> TextDict {
        parse(s.as_bytes(), Path::new("user.dict.yaml"), lower).unwrap()
    }

    const HEAD: &str = "---\nname: pinyin.user\ncolumns: [text, code, weight]\nwind:\n  kind: words\n  code_spacing: syllable\n  escape: true\n...\n";

    #[test]
    fn parses_header_entries_and_boundaries() {
        let d = p(&format!("{HEAD}你好\tni hao\t100\n先\txian\t5\n# 注释\n\n西安\txi an\t\n"), false);
        assert_eq!(d.header.kind.as_deref(), Some("words"));
        assert!(d.header.syllable_codes && d.header.escape);
        let e: Vec<(&str, &str, u64, i32)> = d
            .entries
            .iter()
            .map(|e| (e.code.as_str(), e.text.as_str(), e.boundary, e.weight))
            .collect();
        assert_eq!(
            e,
            vec![("nihao", "你好", 0b101, 100), ("xian", "先", 0, 5), ("xian", "西安", 0b101, 0)],
            "无空格码 = 边界未知（0），不是单音节；空权重 = 0"
        );
        assert_eq!(d.entries[0].line, 8, "行号含头部行");
    }

    #[test]
    fn bom_crlf_escape_and_no_comment() {
        let raw = format!("\u{feff}{}C:\\\\note\tcn\t1\r\n# no comment\r\n#井\tjing\t2\r\n", HEAD.replace('\n', "\r\n"));
        let d = p(&raw, false);
        assert!(d.bom && d.crlf);
        assert_eq!(d.entries[0].text, "C:\\note", "escape: true 时反转义");
        assert_eq!(d.entries[1].text, "#井", "no comment 之后 # 行是数据");
    }

    #[test]
    fn codetable_file_without_syllable_flag_has_no_boundaries() {
        let d = p("---\nname: wubi86.user\n...\n工人\taaww\t3\n", false);
        assert_eq!(d.entries[0].boundary, 0);
        assert!(!d.header.syllable_codes && !d.header.escape);
    }

    #[test]
    fn normalize_key_flattens_spaces_and_lowercases() {
        assert_eq!(normalize_key("ni hao", "你好", false), ("nihao".into(), 0b101, "你好".into()));
        assert_eq!(normalize_key("Hello", "Hello", true).0, "hello");
        assert_eq!(normalize_key("Hello", "Hello", false).0, "Hello");
    }

    #[test]
    fn broken_files_are_classified() {
        assert!(matches!(parse(&[0xff, 0xfe, 0x00], Path::new("x"), false), Err(ParseError::NotUtf8)));
        assert!(matches!(parse(b"---\nname: [\n...\n", Path::new("x"), false), Err(ParseError::BadHeader(_))));
        assert!(matches!(parse(b"just text", Path::new("x"), false), Err(ParseError::NoBody)));
        assert!(p(&HEAD.to_string(), false).entries.is_empty(), "只有头 = 零条目");
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cd wind_input && CARGO_TARGET_DIR=<本 worktree 专用目标目录> TMPDIR=<独立临时目录> cargo test -p wind-userdata text:: 2>&1 | grep -E "error\[|FAILED|test result"`
Expected: 编译失败——`text` 模块不存在。

- [ ] **Step 3: 实现 `text/format.rs`**

```rust
//! 用户词文本文件（设计 §2.2 / §2.3）：Rime `.dict.yaml` + 头部 `wind:` 扩展。列规格与注释规则
//! 复用 `wind_dict::rime_layout`（规则只此一份）；匹配键只经 [`normalize_key`]。

use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindHeader {
    pub kind: Option<String>,
    /// `code_spacing: syllable`：码以空格分音节；无空格码 = 边界未知（0）。
    pub syllable_codes: bool,
    /// `escape: true`：text 列按 `escape_text_field` 规则转义。
    pub escape: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEntry {
    /// 该行在「按 `\n` 切开的全部行」里的下标（含头部）。补丁按它定位。
    pub line: usize,
    /// 规范化后的扁平码。
    pub code: String,
    pub text: String,
    pub boundary: u64,
    pub weight: i32,
}

#[derive(Clone, Debug)]
pub struct TextDict {
    pub header: WindHeader,
    pub entries: Vec<TextEntry>,
    pub bom: bool,
    pub crlf: bool,
}

#[derive(Debug)]
pub enum ParseError {
    NotUtf8,
    BadHeader(String),
    NoBody,
}

#[derive(Deserialize, Default)]
struct RawHeader {
    #[serde(default)]
    wind: Option<RawWind>,
}

#[derive(Deserialize, Default)]
struct RawWind {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    code_spacing: Option<String>,
    #[serde(default)]
    escape: Option<bool>,
}

/// 匹配键规范化（§2.3，唯一实现）：码去空格得扁平码与音节边界（无空格 ⇒ 0），按方案小写；
/// 文本只做输入归一（行尾）。
pub fn normalize_key(code: &str, text: &str, lowercase: bool) -> (String, u64, String) {
    let (mut flat, boundary) = wind_wdict::split_spaced_code(code.trim());
    if lowercase {
        flat = flat.to_lowercase();
    }
    let text = wind_utils::text::normalize_input(text).into_owned();
    (flat, boundary, text)
}

pub fn parse(bytes: &[u8], path: &Path, lowercase: bool) -> Result<TextDict, ParseError> {
    let s = std::str::from_utf8(bytes).map_err(|_| ParseError::NotUtf8)?;
    let bom = s.starts_with('\u{feff}');
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let crlf = s.find('\n').is_some_and(|i| i > 0 && s.as_bytes()[i - 1] == b'\r');
    let layout = wind_dict::rime_layout(s, path).ok_or(ParseError::NoBody)?;
    let head_text = &s[..layout.body_offset];
    let yaml = head_text
        .trim_start_matches("---")
        .trim_end()
        .trim_end_matches("...");
    let raw: RawHeader = if yaml.trim().is_empty() {
        RawHeader::default()
    } else {
        serde_yaml::from_str(yaml).map_err(|e| ParseError::BadHeader(e.to_string()))?
    };
    let wind = raw.wind.unwrap_or_default();
    let header = WindHeader {
        kind: wind.kind,
        syllable_codes: wind.code_spacing.as_deref() == Some("syllable"),
        escape: wind.escape.unwrap_or(false),
    };
    let mut entries = Vec::new();
    let mut offset = 0usize;
    for (idx, raw_line) in s.split('\n').enumerate() {
        let start = offset;
        offset += raw_line.len() + 1;
        if start < layout.body_offset {
            continue;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let is_comment_zone = layout.comment_cutoff.is_none_or(|c| start <= c);
        if line.starts_with('#') && is_comment_zone {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        let (Some(text), Some(code)) = (parts.get(layout.text_col), parts.get(layout.code_col)) else {
            continue;
        };
        if text.is_empty() || code.trim().is_empty() {
            continue;
        }
        let weight = layout
            .weight_col
            .and_then(|i| parts.get(i))
            .and_then(|w| w.trim().parse::<i32>().ok())
            .unwrap_or(0);
        let text = if header.escape {
            wind_wdict::unescape_text_field(text)
        } else {
            (*text).to_string()
        };
        let (code, boundary, text) = normalize_key(code, &text, lowercase);
        entries.push(TextEntry {
            line: idx,
            code,
            text,
            boundary: if header.syllable_codes { boundary } else { 0 },
            weight,
        });
    }
    Ok(TextDict {
        header,
        entries,
        bom,
        crlf,
    })
}
```

（`unescape_text_field` 的实际签名若是 `(&str) -> String` 以外的形状，按它的签名调；`is_none_or` 需 Rust 1.82+，工具链 1.96 可用。）

`text/mod.rs`：

```rust
//! 文本真源层（设计 §2.2 / §2.3 / §3.5 文本目标）。
pub mod format;
pub mod patch;

pub use format::{ParseError, TextDict, TextEntry, WindHeader, normalize_key, parse};
pub use patch::{PatchOp, patch, render};
```

（`patch` 模块 Task 3 才有：本任务先只写 `pub mod format;` 与它那行 `pub use`。）

`Cargo.toml` 加 `wind-wdict = { path = "../wind-wdict" }`、`wind-utils = { path = "../wind-utils" }`（若无）、`serde_yaml = { workspace = true }`。

- [ ] **Step 4: 运行确认通过**：同 Step 2，Expected 5 条 PASS；`cargo test -p wind-userdata --no-fail-fast` 全绿。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/text
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/Cargo.lock wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 用户词文本解析与唯一的匹配键规范化（§2.2 / §2.3）
EOF
```

---

### Task 3: 渲染新文件与按行补丁

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/patch.rs`
- Modify: `wind_input/crates/wind-userdata/src/text/mod.rs`
- Test: `patch.rs` tests

**Interfaces:**
- Consumes: Task 2 的 `parse` / `normalize_key` / `TextDict`；`wind_wdict::{join_code_by_boundary, escape_text_field}`。
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub enum PatchOp {
      Upsert { code: String, text: String, boundary: u64, weight: i32 },
      Delete { code: String, text: String },
      SetWeight { code: String, text: String, weight: i32, ts_secs: i64 },
  }
  /// 整文件渲染（迁移 / 清空 / 导入 / 新建）：LF、无 BOM、`columns: [text, code, weight]`。
  pub fn render(name: &str, syllable_codes: bool, rows: &[(String, String, u64, i32)]) -> String
  /// 按行补丁：返回新内容（字节）。`file_mtime_secs` 用于「SetWeight 早于文件修改时间 ⇒ 视为被手改覆盖，跳过」。
  pub fn patch(
      original: &[u8],
      path: &Path,
      lowercase: bool,
      ops: &[PatchOp],
      file_mtime_secs: i64,
      new_file_name: &str,
      syllable_codes: bool,
  ) -> Result<Vec<u8>, ParseError>
  ```
  ops 里的 `code` 是扁平码（引擎侧存的就是扁平码）；匹配用 `normalize_key` 后的 `(code, text)`。
  语义（§2.3 / §3.5）：
  - `Upsert`：文件里有同键行 ⇒ 只改**第一行**的权重列为 `max(旧, 新)`（无权重列则不动），保留原行位置与其余字节；没有 ⇒ 追加到末尾（按文件的列序拼，码用 `join_code_by_boundary`，`escape` 时转义 text）。
  - `Delete`：删**所有**同键行。
  - `SetWeight`：只改第一行；`ts_secs < file_mtime_secs` ⇒ 跳过（被手改覆盖）；没有该行 ⇒ 空操作。
  - 操作按给出顺序依次作用（同键多操作按 seq 已在调用方排好）。
  - `original` 为空 ⇒ 先 `render(new_file_name, syllable_codes, &[])` 得到头，再追加。
  - 末尾追加时：原文件末尾无换行则先补一个换行（用文件的换行风格）。

- [ ] **Step 1: 写失败测试（patch.rs tests）**

```rust
    use std::path::Path;

    const HEAD_CRLF: &str = "\u{feff}# 我的词库\r\n---\r\nname: pinyin.user\r\ncolumns: [text, code, weight]\r\nwind:\r\n  kind: words\r\n  code_spacing: syllable\r\n  escape: true\r\n...\r\n";

    fn up(code: &str, text: &str, b: u64, w: i32) -> PatchOp {
        PatchOp::Upsert { code: code.into(), text: text.into(), boundary: b, weight: w }
    }

    fn run(orig: &str, ops: &[PatchOp]) -> String {
        String::from_utf8(
            patch(orig.as_bytes(), Path::new("user.dict.yaml"), false, ops, 1_000, "pinyin.user", true).unwrap(),
        )
        .unwrap()
    }

    /// Review Focus 1：除被改行外逐字节不变（BOM、CRLF、注释、空行、自定义顺序、行尾空白）。
    #[test]
    fn patch_preserves_everything_but_touched_lines() {
        let orig = format!("{HEAD_CRLF}# 常用\r\n再见\tzai jian\t5  \r\n\r\n你好\tni hao\t3\r\n先\txian\t1\r\n");
        let out = run(
            &orig,
            &[
                up("nihao", "你好", 0b101, 9),
                PatchOp::Delete { code: "xian".into(), text: "先".into() },
                up("xinci", "新词", 0b101, 7),
            ],
        );
        let want = format!("{HEAD_CRLF}# 常用\r\n再见\tzai jian\t5  \r\n\r\n你好\tni hao\t9\r\n新词\txin ci\t7\r\n");
        assert_eq!(out, want);
    }

    /// Review Focus 2：`ni hao` 行被扁平键 `nihao` 命中；重复行 Delete 全删、SetWeight 只改第一行。
    #[test]
    fn delete_hits_spaced_line_by_flat_key() {
        let orig = "---\nwind:\n  code_spacing: syllable\n...\n你好\tni hao\t3\n你好\tnihao\t4\n好\thao\t1\n";
        let out = run(orig, &[PatchOp::Delete { code: "nihao".into(), text: "你好".into() }]);
        assert_eq!(out, "---\nwind:\n  code_spacing: syllable\n...\n好\thao\t1\n");
        let out = run(
            "---\n...\n好\thao\t1\n好\thao\t1\n",
            &[PatchOp::SetWeight { code: "hao".into(), text: "好".into(), weight: 8, ts_secs: 2_000 }],
        );
        assert_eq!(out, "---\n...\n好\thao\t8\n好\thao\t1\n");
    }

    #[test]
    fn upsert_takes_max_and_setweight_older_than_file_is_skipped() {
        let orig = "---\n...\n好\thao\t10\n";
        assert_eq!(run(orig, &[up("hao", "好", 0, 3)]), orig, "Upsert 取 max：旧 10 > 新 3 不变");
        let stale = PatchOp::SetWeight { code: "hao".into(), text: "好".into(), weight: 1, ts_secs: 500 };
        assert_eq!(run(orig, &[stale]), orig, "早于文件 mtime 的 SetWeight 视为被手改覆盖");
    }

    /// Review Focus 5：边界未知的词写回无空格码，再解析边界仍为 0。
    #[test]
    fn unknown_boundary_roundtrips_as_zero() {
        let out = run("", &[up("xianning", "咸宁", 0, 1), up("xian", "先", 0b1, 1)]);
        let d = crate::text::parse(out.as_bytes(), Path::new("u"), false).unwrap();
        let b: Vec<(String, u64)> = d.entries.iter().map(|e| (e.code.clone(), e.boundary)).collect();
        assert_eq!(b, vec![("xianning".into(), 0), ("xian".into(), 0)]);
        assert!(out.starts_with("---\nname: pinyin.user\n"), "空文件先渲染头");
    }

    #[test]
    fn escape_and_missing_trailing_newline() {
        let out = run("---\nwind:\n  escape: true\n...\n好\thao\t1", &[up("cn", "C:\\note", 0, 1)]);
        assert_eq!(out, "---\nwind:\n  escape: true\n...\n好\thao\t1\nC:\\\\note\tcn\t1\n");
    }

    #[test]
    fn render_writes_a_complete_file() {
        let s = render("pinyin.user", true, &[("nihao".into(), "你好".into(), 0b101, 3)]);
        assert!(s.contains("columns: [text, code, weight]") && s.contains("code_spacing: syllable"));
        assert!(s.ends_with("...\n你好\tni hao\t3\n"));
    }
```

- [ ] **Step 2: 运行确认失败**：`cargo test -p wind-userdata text::patch` → 编译失败（`patch` 未定义）。

- [ ] **Step 3: 实现 `text/patch.rs`**

```rust
//! 渲染与按行补丁写回（设计 §3.5 文本目标第 1 步）。只改被操作的行，其余字节原样保留。

use super::format::{ParseError, normalize_key, parse};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum PatchOp {
    Upsert { code: String, text: String, boundary: u64, weight: i32 },
    Delete { code: String, text: String },
    SetWeight { code: String, text: String, weight: i32, ts_secs: i64 },
}

fn header(name: &str, syllable_codes: bool) -> String {
    let mut h = format!("---\nname: {name}\ncolumns: [text, code, weight]\nwind:\n  kind: words\n");
    if syllable_codes {
        h.push_str("  code_spacing: syllable\n");
    }
    h.push_str("  escape: true\n...\n");
    h
}

pub fn render(name: &str, syllable_codes: bool, rows: &[(String, String, u64, i32)]) -> String {
    let mut s = header(name, syllable_codes);
    for (code, text, boundary, weight) in rows {
        s.push_str(&wind_wdict::escape_text_field(text));
        s.push('\t');
        s.push_str(&wind_wdict::join_code_by_boundary(code, *boundary));
        s.push('\t');
        s.push_str(&weight.to_string());
        s.push('\n');
    }
    s
}

pub fn patch(
    original: &[u8],
    path: &Path,
    lowercase: bool,
    ops: &[PatchOp],
    file_mtime_secs: i64,
    new_file_name: &str,
    syllable_codes: bool,
) -> Result<Vec<u8>, ParseError> {
    let fresh;
    let original: &[u8] = if original.is_empty() {
        fresh = header(new_file_name, syllable_codes);
        fresh.as_bytes()
    } else {
        original
    };
    let dict = parse(original, path, lowercase)?;
    let s = std::str::from_utf8(original).map_err(|_| ParseError::NotUtf8)?;
    let (bom, body) = match s.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", s),
    };
    let nl = if dict.crlf { "\r\n" } else { "\n" };
    let mut lines: Vec<Option<String>> = body.split('\n').map(|l| Some(l.to_string())).collect();
    // 末尾若以换行结束，split 会多出一个空串：记下来，输出时还原
    let ends_with_nl = body.ends_with('\n');
    if ends_with_nl {
        lines.pop();
    }
    let layout = wind_dict::rime_layout(body, path).ok_or(ParseError::NoBody)?;
    // 键 → 该键所在行号（文件序）
    let mut by_key: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for e in &dict.entries {
        by_key.entry((e.code.clone(), e.text.clone())).or_default().push(e.line);
    }
    let mut appended: Vec<String> = Vec::new();
    let set_weight = |line: &mut String, w: i32| {
        let Some(wi) = layout.weight_col else { return };
        let had_cr = line.ends_with('\r');
        let core = line.strip_suffix('\r').unwrap_or(line).to_string();
        let mut parts: Vec<String> = core.split('\t').map(str::to_string).collect();
        if parts.len() <= wi {
            parts.resize(wi + 1, String::new());
        }
        // 保留原字段的尾随空白（如 `5  `）只在没改值时才有意义；改了就写规范值
        parts[wi] = w.to_string();
        *line = parts.join("\t");
        if had_cr {
            line.push('\r');
        }
    };
    let weight_of = |line: &str| -> i32 {
        layout
            .weight_col
            .and_then(|i| line.trim_end_matches('\r').split('\t').nth(i))
            .and_then(|w| w.trim().parse().ok())
            .unwrap_or(0)
    };
    for op in ops {
        match op {
            PatchOp::Upsert { code, text, boundary, weight } => {
                let (c, _, t) = normalize_key(code, text, lowercase);
                let live: Vec<usize> = by_key
                    .get(&(c.clone(), t.clone()))
                    .map(|v| v.iter().copied().filter(|i| lines[*i].is_some()).collect())
                    .unwrap_or_default();
                if let Some(&first) = live.first() {
                    let line = lines[first].as_mut().unwrap();
                    let old = weight_of(line);
                    if *weight > old {
                        set_weight(line, *weight);
                    }
                } else {
                    let mut cols = vec![String::new(); layout.weight_col.map_or(0, |w| w + 1).max(layout.text_col.max(layout.code_col) + 1)];
                    cols[layout.text_col] = if dict.header.escape { wind_wdict::escape_text_field(&t) } else { t.clone() };
                    cols[layout.code_col] = wind_wdict::join_code_by_boundary(&c, *boundary);
                    if let Some(wi) = layout.weight_col {
                        cols[wi] = weight.to_string();
                    }
                    appended.push(cols.join("\t"));
                    // 同一轮里对这个新键的后续操作要能找到它：追加记录先按「虚拟行」登记
                    by_key.entry((c, t)).or_default();
                }
            }
            PatchOp::Delete { code, text } => {
                let (c, _, t) = normalize_key(code, text, lowercase);
                if let Some(v) = by_key.get(&(c.clone(), t.clone())) {
                    for &i in v {
                        lines[i] = None;
                    }
                }
                appended.retain(|l| {
                    let mut it = l.split('\t');
                    let f: Vec<&str> = it.by_ref().collect();
                    let lt = f.get(layout.text_col).copied().unwrap_or("");
                    let lt = if dict.header.escape { wind_wdict::unescape_text_field(lt) } else { lt.to_string() };
                    let lc = f.get(layout.code_col).copied().unwrap_or("");
                    normalize_key(lc, &lt, lowercase).0 != c || lt != t
                });
            }
            PatchOp::SetWeight { code, text, weight, ts_secs } => {
                if *ts_secs < file_mtime_secs {
                    continue;
                }
                let (c, _, t) = normalize_key(code, text, lowercase);
                if let Some(&first) = by_key
                    .get(&(c, t))
                    .and_then(|v| v.iter().find(|i| lines[**i].is_some()))
                {
                    set_weight(lines[first].as_mut().unwrap(), *weight);
                }
            }
        }
    }
    let mut out = String::with_capacity(original.len() + appended.len() * 32);
    out.push_str(bom);
    let kept: Vec<&String> = lines.iter().flatten().collect();
    for (i, l) in kept.iter().enumerate() {
        out.push_str(l);
        if i + 1 < kept.len() || ends_with_nl || !appended.is_empty() {
            out.push('\n');
        }
    }
    for l in &appended {
        out.push_str(l);
        if dict.crlf {
            out.push('\r');
        }
        out.push('\n');
    }
    let _ = nl;
    Ok(out.into_bytes())
}
```

说明（实现时按测试校正，不改测试）：
- `lines` 保留每行原样（CRLF 文件的行仍带 `\r`），只有被改行重写；追加行按文件换行风格补 `\r`。
- 上面的 `Upsert` 分支对「同一轮里先追加再 Upsert / Delete 同一新键」的处理是示意；以 `delete_hits_spaced_line_by_flat_key` 与 `patch_preserves_everything_but_touched_lines` 为准把它写对——不同轮次的同键操作在调用方已按 seq 排好，**同一轮内对同一新键的 Upsert→Upsert 应只追加一行（权重取 max）、Upsert→Delete 应不追加**。若示意写法达不到，改成先把 ops 按键归并成每键一个最终动作（仍按 seq 叠加）再作用到文件。
- `text/mod.rs` 补上 `pub mod patch;` 与 `pub use patch::{PatchOp, patch, render};`。

- [ ] **Step 4: 运行确认通过**：`cargo test -p wind-userdata text::` → 11 条 PASS（Task 2 的 5 + 本任务 6）。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/text/patch.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 用户词文本渲染与按行补丁写回（除被改行外逐字节不变）
EOF
```

---

### Task 4: sidecar 加 `order`，用户词层

**Files:**
- Modify: `wind_input/crates/wind-userdata/src/snapshot.rs`（`SnapshotEntry.order: u32`；sidecar 行 24 → 28 字节，头 version 1 → 2；`build` 用 `e.order`；读出时填 `order`）
- Modify: 所有构造 `SnapshotEntry` 的地方（编译报错逐个补 `order: 0`；`wind-store/src/migrate_p1.rs` 的 `entry` 同理）
- Modify: `wind_input/crates/wind-userdata/src/layer.rs`（`UserDataTempLayer` 泛化：新增 `UserDataUserLayer`）
- Test: `snapshot.rs`、`layer.rs` tests

**Interfaces:**
- Produces:
  - `SnapshotEntry { …, pub order: u32 }`：入库顺序（文本目标 = 行序；运行时目标 0）。sidecar 每行 `count u32 | created_at i64 | last_used i64 | flags u8 | pad [u8;3] | order u32`（28 字节）；头 `version = 2`，读到 version 1 ⇒ `Err`（当零段并整份回放——格式未发布，只影响开发机）。
  - `pub struct UserDataUserLayer`；`UserDataUserLayer::new(ud: Arc<UserData>, schema: &str)`；`name()` = `"user:{schema}"`；`layer_type()` = `LayerType::User`；候选 `meta.is_user_dict = true`、`natural_order = order`（饱和到 `i32::MAX`）；`search` / `search_prefix`（`is_prefix` 恒 true，同 `StoreUserLayer`）/ `search_abbrev` / `search_abbrev_exact` / `search_pattern` / `entries_digest`（`"user:{schema}|enumerates-nothing"`）与临时词层同构。实现上把临时词层的查询代码抽成按 `(Target, LayerType, is_user)` 参数化的内部结构，两个层各包一份，不复制查询逻辑。

- [ ] **Step 1: 写失败测试**

`snapshot.rs` tests：

```rust
    #[test]
    fn order_survives_build_and_open() {
        let d = std::env::temp_dir().join(format!("wind_snap_order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut a = se("ni", "你", 5, 0b1, 0);
        a.order = 7;
        let mut b = se("ni", "泥", 5, 0b1, 0);
        b.order = 3;
        drop(build(&d, "t", 1, vec![a, b].into_iter()).unwrap());
        let s = open(&d, "t").unwrap().unwrap();
        let mut got: Vec<(String, u32)> = s.exact("ni").into_iter().map(|e| (e.text, e.order)).collect();
        got.sort();
        assert_eq!(got, vec![("你".into(), 7), ("泥".into(), 3)]);
        let _ = std::fs::remove_dir_all(&d);
    }
```

（`se()` 辅助里补 `order: 0`。）

`layer.rs` tests：

```rust
    #[test]
    fn user_layer_marks_user_dict_and_uses_line_order() {
        let ud = open_ud("userlayer");
        let mut ops = Vec::new();
        for (text, order_hint) in [("泥", 2u32), ("你", 1)] {
            let _ = order_hint;
            ops.push(crate::engine::Op::new(Target::User, "py", OpKind::Upsert, "ni", text, Payload::Weight(5), 0b1));
        }
        ud.apply_batch(&ops).unwrap();
        let l = UserDataUserLayer::new(ud, "py");
        assert_eq!(l.name(), "user:py");
        assert_eq!(l.layer_type(), LayerType::User);
        let c = l.search("ni", 0);
        assert!(c.iter().all(|c| c.meta.is_user_dict && !c.meta.is_temp_dict));
        assert!(l.search_prefix("n", 0).iter().all(|c| c.is_prefix));
    }
```

（行序进 `natural_order` 由 Task 5 的文本目标端到端测试覆盖：delta 里的新词 order 为 0。）

- [ ] **Step 2: 运行确认失败**：编译失败——`order` 字段 / `UserDataUserLayer` 未定义。

- [ ] **Step 3: 实现**：按 Interfaces 改 `snapshot.rs`（`META_ROW = 28`、头 version 2、`build` 的 `push_code` 用 `e.order` 而**不再**用组内下标——⚠️ 组内下标仍是 `idx_of` 的依据，`push_code` 的 `order` 字段与 sidecar 序号是两回事：保持 `push_code(..., i as u32, ...)` 传组内下标不变，`order` 只进 sidecar 行；读出时 `order` 从 sidecar 取）。`layer.rs` 抽出 `struct LayerCore { ud, target, schema, user: bool }` 承载现有查询方法，`UserDataTempLayer` / `UserDataUserLayer` 各持一个、`DictLayer` 方法转发；`candidate()` 按 `user` 标 `is_user_dict` / `is_temp_dict`，`natural_order` = `if user { order.min(i32::MAX as u32) as i32 } else { 0 }`。`apply_ops` 新建条目 `order: 0`、`overlay_entries` 原样带 order。编译报错的所有 `SnapshotEntry { … }` 补 `order: 0`（含 `wind-store/src/migrate_p1.rs::entry`）。

- [ ] **Step 4: 运行确认通过**：`cargo test -p wind-userdata -p wind-store --no-fail-fast` 全绿。

- [ ] **Step 5: 提交**

```bash
cd wind_input && cargo fmt -p wind-userdata -p wind-store && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata wind_input/crates/wind-store <<'EOF'
feat(userdata): sidecar 记入库顺序；新增用户词层（与临时词层共用查询核心）
EOF
```

---

### Task 5: 文本目标——绑定、`fold_text`、`sync_text`、`rewrite_text`、空闲折回

**Files:**
- Create: `wind_input/crates/wind-userdata/src/text/fold.rs`（`impl UserData` 的文本目标方法）
- Modify: `wind_input/crates/wind-userdata/src/engine.rs`（`text_bindings`、`last_write`、`fold` 路由、`fold_with` 拒绝已绑定目标、`maintain_with` 的空闲规则）、`state.rs`（`TargetState.text_fp` 语义说明）
- Test: `wind_input/crates/wind-userdata/tests/text_target.rs`

**Interfaces:**
- Consumes: Task 2 / 3 / 4。
- Produces:
  ```rust
  #[derive(Clone, Debug)]
  pub struct TextBinding { pub path: PathBuf, pub name: String, pub lowercase: bool, pub syllable_codes: bool }
  pub const TEXT_IDLE: std::time::Duration = std::time::Duration::from_secs(30);
  impl UserData {
      /// 绑定后立即 sync_text 一次。
      pub fn bind_text(&self, target: Target, schema: &str, b: TextBinding) -> anyhow::Result<()>;
      pub fn text_binding(&self, target: Target, schema: &str) -> Option<TextBinding>;
      /// §3.5 文本目标折回。delta 里没有该目标的操作 ⇒ 不写文件。
      pub fn fold_text(&self, target: Target, schema: &str) -> anyhow::Result<()>;
      /// 文件被手改 / 换了 ⇒ 由文本重建段（max_seq = 当前折回水位），delta 不动。返回是否重建。
      pub fn sync_text(&self, target: Target, schema: &str) -> anyhow::Result<bool>;
      /// 整文件替换（清空 / 导入 / 迁移）：先折回待处理操作，再渲染 rows 写盘、重建段、推进水位。
      pub fn rewrite_text(&self, target: Target, schema: &str, rows: &[(String, String, u64, i32)]) -> anyhow::Result<()>;
  }
  ```
  - `fold(target, schema)` 对已绑定文本的目标路由到 `fold_text`；`fold_with` 对已绑定目标返回 Err。
  - `maintain_with`：文本目标的折回条件 = `pending ≥ 1` 且距该目标最后一次写入 ≥ `TEXT_IDLE`（测试用 `maintain_text_idle(Duration)` 注入）；运行时目标照旧按条数。`compact` 先 `fold`（含文本目标）再裁日志。
  - 指纹：`crc32(bytes)` 与长度拼成 `"{len:x}-{crc:08x}"`，文件不存在为 `"absent"`，存在 `TargetState.text_fp`。

  `fold_text` 步骤（§3.5，顺序不得调换）：
  1. 取 `upto` 与视图（同 `fold_with` 的原子取法，持 `fold_lock`）；收集 delta 里该目标 `seq ≤ upto` 的操作转 `PatchOp`（Upsert / Delete / SetWeight；Incr / Touch 不进文本），没有就直接返回 Ok。
  2. 读文件：`NotFound` 且父目录存在 ⇒ 视为空文件（Upsert 会新建）；`NotFound` 且父目录不存在 ⇒ 建目录后同上；其他读错误（权限 / 占用）⇒ `bail!`（本轮跳过，delta 与 state 不动）。记 `fp_before` 与 `mtime`。
  3. `patch(...)`：`ParseError` ⇒ 隔离，WARN 一次、`bail!`。
  4. 写 `<path>.tmp`，`sync_all`。
  5. 再读原文件算指纹，≠ `fp_before` ⇒ 删 tmp、`bail!("文件在折回期间被修改，本轮放弃")`。
  6. 原文件存在则复制为 `<path>.bak`；`rename(tmp, path)`；Unix 下 `File::open(dir).sync_all()`，Windows 下对 `path` 再 `sync_all`（同 `state::atomic_write`）。
  7. 由新内容 `parse` 得条目（`order` = 行号）→ `snapshot::build(state_dir/<schema>, target_name, upto, entries)` → 写锁下换入、摘 delta ≤ upto → state：`folded_seq = upto`、`text_fp = fp(新内容)`。

- [ ] **Step 1: 写失败测试 `tests/text_target.rs`**

```rust
//! 文本目标（设计 §3.5 / §11 文本层）。
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wind_userdata::engine::{Dirs, TextBinding, UserData};
use wind_userdata::record::{OpKind, Payload, Target};

fn setup(name: &str) -> (PathBuf, Arc<UserData>, PathBuf) {
    let d = std::env::temp_dir().join(format!("wind_txt_{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let ud = Arc::new(
        UserData::open(Dirs {
            state_dir: d.join("state"),
            dir_id: "dir".into(),
        })
        .unwrap(),
    );
    let file = d.join("user").join("py").join("user.dict.yaml");
    (d, ud, file)
}

fn bind(ud: &UserData, file: &Path) {
    ud.bind_text(
        Target::User,
        "py",
        TextBinding {
            path: file.to_path_buf(),
            name: "pinyin.user".into(),
            lowercase: false,
            syllable_codes: true,
        },
    )
    .unwrap();
}

fn up(ud: &UserData, code: &str, text: &str, w: i32, b: u64) {
    ud.apply(Target::User, "py", OpKind::Upsert, code, text, Payload::Weight(w), b)
        .unwrap();
}

fn texts(ud: &UserData, code: &str) -> Vec<(String, i32)> {
    ud.query_exact(Target::User, "py", code)
}

#[test]
fn fold_writes_text_and_reads_back_after_reopen() {
    let (d, ud, file) = setup("fold");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    ud.fold(Target::User, "py").unwrap();
    let s = std::fs::read_to_string(&file).unwrap();
    assert!(s.contains("你好\tni hao\t9"), "{s}");
    assert_eq!(ud.pending(Target::User, "py"), 0);
    drop(ud);
    let ud = UserData::open(Dirs { state_dir: d.join("state"), dir_id: "dir".into() }).unwrap();
    bind(&ud, &file);
    assert_eq!(texts(&ud, "nihao"), vec![("你好".into(), 9)]);
}

/// 手改文件 ⇒ sync_text 由文本重建，已折回的记录不回放（删掉的行不复活）；未折回的操作仍叠加。
#[test]
fn hand_edit_wins_and_pending_ops_still_overlay() {
    let (_d, ud, file) = setup("handedit");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    up(&ud, "xian", "先", 1, 0b1);
    ud.fold(Target::User, "py").unwrap();
    let s = std::fs::read_to_string(&file).unwrap().replace("先\txian\t1\n", "");
    std::fs::write(&file, s).unwrap();
    up(&ud, "hao", "好", 2, 0b1);
    assert!(ud.sync_text(Target::User, "py").unwrap(), "指纹变了要重建");
    assert!(texts(&ud, "xian").is_empty(), "手删的行不复活");
    assert_eq!(texts(&ud, "hao"), vec![("好".into(), 2)], "未折回的操作仍叠加");
    assert!(!ud.sync_text(Target::User, "py").unwrap(), "没变就不重建");
}

/// Review Focus 3：折回期间文件被保存 ⇒ 本轮放弃，手改与运行时写入都不丢。
#[test]
fn editor_save_during_fold_discards_the_round() {
    let (_d, ud, file) = setup("race");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    ud.fold(Target::User, "py").unwrap();
    up(&ud, "hao", "好", 2, 0b1);
    wind_userdata::text::fold::set_before_rename_hook(Some(Box::new({
        let file = file.clone();
        move || {
            let s = std::fs::read_to_string(&file).unwrap();
            std::fs::write(&file, format!("{s}手改\tshou gai\t1\n")).unwrap();
        }
    })));
    assert!(ud.fold(Target::User, "py").is_err());
    wind_userdata::text::fold::set_before_rename_hook(None);
    assert_eq!(ud.pending(Target::User, "py"), 1, "delta 保留");
    assert!(std::fs::read_to_string(&file).unwrap().contains("手改"), "手改原样");
    ud.sync_text(Target::User, "py").unwrap();
    ud.fold(Target::User, "py").unwrap();
    let s = std::fs::read_to_string(&file).unwrap();
    assert!(s.contains("手改") && s.contains("好\thao\t2"), "{s}");
}

/// Review Focus 4：被删除 vs 读不了。
#[test]
fn deleted_file_vs_unreadable_file() {
    let (_d, ud, file) = setup("missing");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    ud.fold(Target::User, "py").unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(ud.sync_text(Target::User, "py").unwrap());
    assert!(texts(&ud, "nihao").is_empty(), "文件被删 = 用户明确删除");
    up(&ud, "hao", "好", 2, 0b1);
    ud.fold(Target::User, "py").unwrap();
    assert!(std::fs::read_to_string(&file).unwrap().contains("好\thao\t2"), "之后的写入新建文件");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        up(&ud, "ni", "你", 1, 0b1);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = ud.fold(Target::User, "py");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        if r.is_err() {
            assert_eq!(ud.pending(Target::User, "py"), 1, "读不了 ⇒ 本轮跳过、delta 保留");
        } // root 下权限不生效时 fold 会成功，此分支不做断言
    }
}

#[test]
fn rewrite_replaces_whole_file_and_advances_watermark() {
    let (_d, ud, file) = setup("rewrite");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    ud.rewrite_text(Target::User, "py", &[("hao".into(), "好".into(), 0b1, 3)]).unwrap();
    assert!(texts(&ud, "nihao").is_empty() || std::fs::read_to_string(&file).unwrap().contains("你好"),
        "rewrite 前先折回待处理操作，再整文件替换为 rows");
    assert_eq!(texts(&ud, "hao"), vec![("好".into(), 3)]);
    assert!(ud.fold_with(Target::User, "py", &mut |v| v).is_err(), "已绑定文本的目标不走 fold_with");
}

#[test]
fn text_targets_fold_only_after_idle() {
    let (_d, ud, file) = setup("idle");
    bind(&ud, &file);
    up(&ud, "nihao", "你好", 9, 0b101);
    assert_eq!(ud.maintain_text_idle(std::time::Duration::from_secs(3600)).unwrap(), 0, "未空闲不折回");
    assert_eq!(ud.maintain_text_idle(std::time::Duration::ZERO).unwrap(), 1);
    assert!(file.is_file());
}
```

（`rewrite_replaces_whole_file_and_advances_watermark` 的第一条断言写得宽：`rewrite_text` 的语义是「rows 就是新全集」，所以 `你好` 应当不在——实现后把它收紧成 `assert!(texts(&ud, "nihao").is_empty())` 并在账本记 Ruling。）

- [ ] **Step 2: 运行确认失败**：`cargo test -p wind-userdata --test text_target` → 编译失败。

- [ ] **Step 3: 实现**：按 Interfaces 的步骤写 `text/fold.rs`（`impl UserData`），其中「rename 之前」的测试钩子：

```rust
#[doc(hidden)]
pub fn set_before_rename_hook(h: Option<Box<dyn Fn() + Send + Sync>>) {
    *BEFORE_RENAME.lock().unwrap_or_else(|e| e.into_inner()) = h;
}
static BEFORE_RENAME: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>> = std::sync::Mutex::new(None);
```

在第 5 步「再读原文件算指纹」**之前**调用它（测试借此模拟编辑器保存）。`engine.rs`：`UserData` 加 `text_bindings: Mutex<BTreeMap<(Target, String), TextBinding>>` 与 `last_write: Mutex<BTreeMap<(Target, String), Instant>>`（`apply_batch` 成功后按批内目标更新）；`fold` 先查绑定；`fold_with` 对已绑定目标 `bail!`；`maintain_with` 跳过已绑定目标的条数规则，另加 `pub fn maintain_text_idle(&self, idle: Duration) -> anyhow::Result<usize>`（`maintain_with` 末尾以 `TEXT_IDLE` 调它）。`sync_text`：读文件（`NotFound` ⇒ 内容视为空、指纹 `"absent"`）→ 指纹与 state 相同 ⇒ false；否则 `parse`（失败 ⇒ 隔离 WARN、返回 Err）→ `snapshot::build(..., max_seq = 该目标当前 folded_seq, entries)` → 换入（delta 不动）→ state `text_fp` 更新 → true。`rewrite_text`：`fold_text` → `render` → 同 `fold_text` 的第 4–7 步（不做指纹复核：整文件替换是用户明确动作）。

- [ ] **Step 4: 运行确认通过**：`cargo test -p wind-userdata --no-fail-fast` 全绿；`fold_keeps_ops_that_arrived_during_fold` 连跑 10 次全过。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/text/fold.rs wind_input/crates/wind-userdata/tests/text_target.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 文本目标——按 §3.5 次序补丁写回、手改后由文本重建、整文件替换、空闲折回
EOF
```

---

### Task 6: `Runtime` 用户词 API（对拍 `wind-store` user_words）

**Files:**
- Create: `wind_input/crates/wind-userdata/src/runtime/user.rs`
- Modify: `wind_input/crates/wind-userdata/src/runtime/mod.rs`
- Test: `wind_input/crates/wind-userdata/tests/runtime_user.rs`

**Interfaces:**
- Consumes: Task 4 / 5。
- Produces（语义逐条照 `wind-store/src/user_words.rs`）：
  ```rust
  pub const USER_TEXT_MAX_CHARS: usize = 10_000;
  pub const USER_CODE_MAX_CHARS: usize = 128;
  pub struct UserWord { pub code: String, pub text: String, pub weight: i32, pub boundary: u64, pub order: u32 }
  impl Runtime {
      pub fn add_user_word(&self, schema, code, text, weight: i32, boundary: u64) -> anyhow::Result<()>;   // 已存在取 max 权重、补 0 边界；超长 bail
      pub fn remove_user_word(&self, schema, code, text) -> anyhow::Result<()>;                            // 不存在静默
      pub fn update_user_word_weight(&self, schema, code, text, w: i32) -> anyhow::Result<bool>;           // 不存在 false、不新建
      pub fn user_word(&self, schema, code, text) -> Option<UserWord>;
      pub fn user_words(&self, schema, code) -> Vec<UserWord>;
      pub fn for_each_user_word(&self, schema, prefix, f: &mut dyn FnMut(&UserWord) -> bool);
      pub fn count_user_words(&self, schema) -> usize;
      pub fn clear_user_words(&self, schema) -> anyhow::Result<usize>;                                     // rewrite_text(&[])
      pub fn import_user_rows(&self, schema, rows: &[UserWord]) -> anyhow::Result<usize>;                  // 合并：已存在取 max 权重、补边界；新词按行序追加
  }
  ```
  已绑定文本的方案：写入经 delta（`apply`），读取叠加段 + delta；`clear` / `import` 走 `rewrite_text`。未绑定文本的方案（P2b 迁移前不会出现，防御）：同运行时目标处理。代次推进照 `wind-store`：新词 / 删词 / 改权 / 清空 / 导入 ⇒ `bump_words`。

- [ ] **Step 1: 写失败测试 `tests/runtime_user.rs`**——对照 `wind-store/src/user_words.rs` 的以下用例各写一条（断言照抄其外部行为）：`test_add_get_user_word`、`import_user_words_merges_max_weight`、`add_user_word_rejects_text_over_max_chars`、`add_user_word_rejects_code_over_max_chars`、`test_prefix_remove_update`、`clear_user_words_only_target_schema`、`no_boundary_words_are_recalled_by_first_char_only`（经 `UserDataUserLayer::search_abbrev`；即 Review Focus 5 的 `user_layer_recalls_unknown_boundary_by_first_char`）、`reimport_keeps_existing_order`（行序：重导入已有词不改其行位置）。每条用 `setup` + `bind_text`（同 Task 5 的辅助，抄到本文件），并在末尾 `fold` 后重开再断言一次（文本即真源）。

- [ ] **Step 2: 运行确认失败**：编译失败——`add_user_word` 等未定义。

- [ ] **Step 3: 实现 `runtime/user.rs`**：按 Interfaces；`add_user_word` = `get` 后决定 `Upsert(max(w, old))`（补 0 边界时 boundary 取新值）；`order` 读自条目（delta 新词为 0，折回后 = 行号）。

- [ ] **Step 4: 运行确认通过**：`cargo test -p wind-userdata --no-fail-fast` 全绿。

- [ ] **Step 5: 提交**

```bash
git add wind_input/crates/wind-userdata/src/runtime/user.rs wind_input/crates/wind-userdata/tests/runtime_user.rs
cd wind_input && cargo fmt -p wind-userdata && cd .. && git commit -F - --only -- wind_input/crates/wind-userdata <<'EOF'
feat(userdata): 用户词领域 API（对拍 wind-store user_words，文本即真源）
EOF
```

---

### Task 7: 文档与回归

**Files:**
- Modify: `wind_input/crates/wind-userdata/AGENTS.md`（`text/` 模块、文本目标规则、sidecar `order`、用户词层）、`wind_input/crates/wind-dict/AGENTS.md`（`rime_layout`）、`docs/design/user-data-storage-redesign.md`（§10 P2 拆 P2a / P2b、P2a 标已实现与差异：段在 state、总算指纹、count/created_at 不进文本）

- [ ] **Step 1: 改文档**（事实更新，不重写结构）
- [ ] **Step 2: 全量回归**：九个 crate（同 P1b 的循环，输出到 `<独立临时目录>/p2a-*.log`），Expected 0 失败、通过数 ≥ 4937。
- [ ] **Step 3: 提交**

```bash
git commit -F - --only -- wind_input/crates/wind-userdata/AGENTS.md wind_input/crates/wind-dict/AGENTS.md docs/design/user-data-storage-redesign.md <<'EOF'
docs(userdata): P2a 文档与阶段状态；P2 拆为 P2a / P2b
EOF
```
