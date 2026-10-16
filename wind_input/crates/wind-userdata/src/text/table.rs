//! 通用小表（设计 §2.4）：带 `columns:` 表头的 Rime 风格 TSV，按列名读写。短语、Shadow 这类
//! 用户逐条操作、规模小的表用它：内存持有解析结果（读零 IO），写时按组补丁、整文件提交
//! （tmp + fsync + 复核指纹 + rename），未改动的行逐字节不变；手改文件经 `sync` 按指纹重新解析。
//! 不走日志 + delta 引擎。

use super::format::ParseError;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    /// 该行在「按 `\n` 切开的全部行」里的下标（含头部）。
    pub line: usize,
    /// 按表头列序，已反转义；短行补空串。
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableDoc {
    pub columns: Vec<String>,
    pub kind: Option<String>,
    pub escape: bool,
    pub rows: Vec<TableRow>,
}

impl TableDoc {
    fn empty(columns: &[&str], kind: &str) -> TableDoc {
        TableDoc {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            kind: Some(kind.to_string()),
            escape: true,
            rows: Vec::new(),
        }
    }

    /// 按列名取一行的字段（文件里没有该列 ⇒ 空串）。
    pub fn get<'a>(&self, row: &'a TableRow, name: &str) -> &'a str {
        col(self, name)
            .and_then(|i| row.fields.get(i))
            .map_or("", String::as_str)
    }
}

pub fn col(doc: &TableDoc, name: &str) -> Option<usize> {
    doc.columns.iter().position(|c| c == name)
}

#[derive(Deserialize, Default)]
struct RawHead {
    #[serde(default)]
    columns: Option<Vec<String>>,
    #[serde(default)]
    wind: Option<RawWind>,
}

#[derive(Deserialize, Default)]
struct RawWind {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    escape: Option<bool>,
}

/// 读：按 escape 设置反转义。命令栏文本例外——它的反斜杠归命令栏 lexer 管，`\n` / `\t` 是字面
/// 序列，这里原样保留（`escape_text_field` 对它只把真实换行 / 制表符写成 `\n` / `\t`，lexer 读到
/// 两者等价；而 `unescape_text_field` 对它不是逆函数，用了就会在第一次保存后改写用户的文本）。
pub(crate) fn unescape(escape: bool, s: &str) -> String {
    if escape && !wind_wdict::is_cmdbar_text(s) {
        wind_wdict::unescape_text_field(s)
    } else {
        s.to_string()
    }
}

fn escape_field(escape: bool, s: &str) -> String {
    if escape {
        wind_wdict::escape_text_field(s)
    } else {
        s.to_string()
    }
}

pub fn parse_table(bytes: &[u8], _path: &Path) -> Result<TableDoc, ParseError> {
    let s = std::str::from_utf8(bytes).map_err(|_| ParseError::NotUtf8)?;
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let (body_offset, comment_cutoff) = wind_dict::rime_body_layout(s).ok_or(ParseError::NoBody)?;
    let yaml = s[..body_offset]
        .trim_start_matches("---")
        .trim_end()
        .trim_end_matches("...");
    let raw: RawHead = if yaml.trim().is_empty() {
        RawHead::default()
    } else {
        serde_yaml::from_str(yaml).map_err(|e| ParseError::BadHeader(e.to_string()))?
    };
    let columns = raw
        .columns
        .ok_or_else(|| ParseError::BadHeader("缺 columns".into()))?;
    let wind = raw.wind.unwrap_or_default();
    let escape = wind.escape.unwrap_or(false);
    let mut rows = Vec::new();
    let mut offset = 0usize;
    for (idx, raw_line) in s.split('\n').enumerate() {
        let start = offset;
        offset += raw_line.len() + 1;
        if start < body_offset {
            continue;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        // `#` 开头：`# no comment` 之前一律是注释；之后只有含 tab 的才是数据行（这类表至少两列，
        // 数据行必有 tab）——用户手写的 `# 说明` 不该被读成一行数据。
        let in_comment_zone = comment_cutoff.is_none_or(|c| start <= c);
        if line.starts_with('#') && (in_comment_zone || !line.contains('\t')) {
            continue;
        }
        let mut fields: Vec<String> = line
            .split('\t')
            .take(columns.len())
            .map(|f| unescape(escape, f))
            .collect();
        fields.resize(columns.len(), String::new());
        rows.push(TableRow { line: idx, fields });
    }
    Ok(TableDoc {
        columns,
        kind: wind.kind,
        escape,
        rows,
    })
}

/// 整文件渲染：LF、无 BOM、`escape: true`，正文以 `# no comment` 开头（首列可以 `#` 开头）。
pub fn render_table(columns: &[&str], kind: &str, rows: &[Vec<String>]) -> String {
    let mut s = format!(
        "---\ncolumns: [{}]\nwind: {{kind: {kind}, escape: true}}\n...\n# no comment\n",
        columns.join(", ")
    );
    for r in rows {
        let line: Vec<String> = r.iter().map(|f| escape_field(true, f)).collect();
        s.push_str(&line.join("\t"));
        s.push('\n');
    }
    s
}

/// 一组编辑：分组键的取值（按 `group_cols` 序）与该组的新行全集（按 `columns` 列序）。
pub type GroupEdit = (Vec<String>, Vec<Vec<String>>);

/// 按组补丁：原文件里该组的行全部删掉，新行插在原第一行的位置（没有该组 ⇒ 追加到末尾）；其余
/// 字节不变。新行按**文件的**列序、文件的 escape 设置输出（文件里没有的列丢弃）；`original` 为空
/// ⇒ 先渲染表头。
pub fn patch_groups(
    original: &[u8],
    path: &Path,
    group_cols: &[&str],
    edits: &[GroupEdit],
    columns: &[&str],
    kind: &str,
) -> Result<Vec<u8>, ParseError> {
    let fresh;
    let original: &[u8] = if original.is_empty() {
        fresh = render_table(columns, kind, &[]);
        fresh.as_bytes()
    } else {
        original
    };
    let doc = parse_table(original, path)?;
    let s = std::str::from_utf8(original).map_err(|_| ParseError::NotUtf8)?;
    let (bom, body) = match s.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", s),
    };
    let (_, comment_cutoff) = wind_dict::rime_body_layout(body).ok_or(ParseError::NoBody)?;
    let crlf = body
        .find('\n')
        .is_some_and(|i| i > 0 && body.as_bytes()[i - 1] == b'\r');
    let nl = if crlf { "\r\n" } else { "\n" };
    let gidx: Vec<usize> = group_cols
        .iter()
        .map(|c| col(&doc, c).ok_or_else(|| ParseError::BadHeader(format!("缺列 {c}"))))
        .collect::<Result<_, _>>()?;
    // 新行（按 columns 列序）→ 文件列序的一行文本
    let render_row = |r: &Vec<String>| -> String {
        doc.columns
            .iter()
            .map(|fc| {
                let v = columns
                    .iter()
                    .position(|c| c == fc)
                    .and_then(|i| r.get(i))
                    .map_or("", String::as_str);
                escape_field(doc.escape, v)
            })
            .collect::<Vec<_>>()
            .join("\t")
    };
    let ends_with_nl = body.ends_with('\n');
    let mut lines: Vec<Option<String>> = body.split('\n').map(|l| Some(l.to_string())).collect();
    if ends_with_nl {
        lines.pop();
    }
    let mut appended: Vec<String> = Vec::new();
    for (key, new_rows) in edits {
        let hits: Vec<usize> = doc
            .rows
            .iter()
            .filter(|r| gidx.iter().zip(key).all(|(&i, k)| &r.fields[i] == k))
            .map(|r| r.line)
            .collect();
        let rendered: Vec<String> = new_rows.iter().map(render_row).collect();
        match hits.first() {
            Some(&first) => {
                let cr = if lines[first].as_deref().is_some_and(|l| l.ends_with('\r')) {
                    "\r"
                } else {
                    ""
                };
                for &i in &hits {
                    lines[i] = None;
                }
                if !rendered.is_empty() {
                    let joined: Vec<String> = rendered.iter().map(|l| format!("{l}{cr}")).collect();
                    lines[first] = Some(joined.join("\n"));
                }
            }
            None => appended.extend(rendered),
        }
    }
    let mut out = String::with_capacity(original.len() + appended.len() * 32);
    out.push_str(bom);
    let kept: Vec<&String> = lines.iter().flatten().collect();
    for (i, l) in kept.iter().enumerate() {
        out.push_str(l);
        if i + 1 < kept.len() || ends_with_nl {
            out.push('\n');
        }
    }
    if !appended.is_empty() && !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    if comment_cutoff.is_none() && appended.iter().any(|r| r.starts_with('#')) {
        out.push_str("# no comment");
        out.push_str(nl);
    }
    for r in appended {
        out.push_str(&r);
        out.push_str(nl);
    }
    Ok(out.into_bytes())
}

struct TableState {
    fp: String,
    doc: Arc<TableDoc>,
    isolated: bool,
}

/// 一个小表文件的持有者：内存里是最近一次成功解析的内容，读零 IO。
pub struct TextTable {
    path: PathBuf,
    columns: &'static [&'static str],
    kind: &'static str,
    state: Mutex<TableState>,
}

impl TextTable {
    /// 读文件并解析：不存在 ⇒ 空表；解析失败 ⇒ 空表 + 隔离（不往里写）+ WARN。
    pub fn open(path: PathBuf, columns: &'static [&'static str], kind: &'static str) -> TextTable {
        let t = TextTable {
            path,
            columns,
            kind,
            state: Mutex::new(TableState {
                fp: String::new(),
                doc: Arc::new(TableDoc::empty(columns, kind)),
                isolated: false,
            }),
        };
        t.sync();
        t
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn doc(&self) -> Arc<TableDoc> {
        self.lock().doc.clone()
    }

    pub fn isolated(&self) -> bool {
        self.lock().isolated
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TableState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 按文件现状更新内存（文件锁外的读取；`st` 由调用方持有）。返回是否换了内容。
    fn reload_locked(&self, st: &mut TableState) -> anyhow::Result<bool> {
        let cur = super::fold::read_text(&self.path)?;
        let fp = super::fold::fingerprint(cur.as_ref().map(|(b, _)| b.as_slice()));
        if fp == st.fp {
            return Ok(false);
        }
        st.fp = fp;
        match cur {
            None => {
                st.doc = Arc::new(TableDoc::empty(self.columns, self.kind));
                st.isolated = false;
                Ok(true)
            }
            Some((bytes, _)) => match parse_table(&bytes, &self.path) {
                Ok(doc) => {
                    st.doc = Arc::new(doc);
                    st.isolated = false;
                    Ok(true)
                }
                Err(e) => {
                    tracing::warn!(
                        "{} 隔离（{e:?}）：不往里写，按上次成功解析的内容提供",
                        self.path.display()
                    );
                    st.isolated = true;
                    Ok(false)
                }
            },
        }
    }

    /// 文件变了（指纹不同）就重新解析；隔离时保留上次成功的内容。返回是否换了内容。
    pub fn sync(&self) -> bool {
        let mut st = self.lock();
        match self.reload_locked(&mut st) {
            Ok(changed) => changed,
            Err(e) => {
                tracing::warn!("读不了 {}：{e:#}", self.path.display());
                false
            }
        }
    }

    /// 按组编辑并提交。先按文件现状重新解析（手改优先）；文件隔离 ⇒ Err、不写。
    pub fn edit(&self, edits: &[GroupEdit], group_cols: &[&str]) -> anyhow::Result<()> {
        let mut st = self.lock();
        self.reload_locked(&mut st)?;
        if st.isolated {
            anyhow::bail!("{} 无法解析，请先修复或删除它", self.path.display());
        }
        let original = match super::fold::read_text(&self.path)? {
            Some((b, _)) => b,
            None => Vec::new(),
        };
        let out = patch_groups(
            &original,
            &self.path,
            group_cols,
            edits,
            self.columns,
            self.kind,
        )
        .map_err(|e| anyhow::anyhow!("{} 补丁失败：{e:?}", self.path.display()))?;
        super::fold::commit_text(&self.path, &out, Some(&st.fp))?;
        self.install(&mut st, &out)
    }

    /// 整文件替换（清空 / 还原 / 迁移）：不复核指纹，隔离的文件也照样替换。
    pub fn replace_all(&self, rows: &[Vec<String>]) -> anyhow::Result<()> {
        let mut st = self.lock();
        let out = render_table(self.columns, self.kind, rows);
        super::fold::commit_text(&self.path, out.as_bytes(), None)?;
        self.install(&mut st, out.as_bytes())
    }

    fn install(&self, st: &mut TableState, bytes: &[u8]) -> anyhow::Result<()> {
        let doc = parse_table(bytes, &self.path)
            .map_err(|e| anyhow::anyhow!("写出的内容解析失败：{e:?}"))?;
        st.fp = super::fold::fingerprint(Some(bytes));
        st.doc = Arc::new(doc);
        st.isolated = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const COLS: &[&str] = &["code", "text", "action", "position", "cand_id"];

    fn row(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wind_tbl_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 空首列（空编码）是数据行；省略的尾列补空串；注释与空行跳过；escape 反转义。
    #[test]
    fn parses_by_column_name_and_keeps_empty_code_rows() {
        let f = "---\ncolumns: [code, text, action, position, cand_id]\nwind: {kind: shadow, escape: true}\n...\n# 注释\n\naa\t恭\tpin\t0\n\t日期\tpin\t1\tphrase:rq:{date}\nbb\t见\\n了\tdelete\n";
        let d = parse_table(f.as_bytes(), Path::new("s")).unwrap();
        assert_eq!(d.kind.as_deref(), Some("shadow"));
        assert_eq!(col(&d, "action"), Some(2));
        let rows: Vec<Vec<&str>> = d
            .rows
            .iter()
            .map(|r| r.fields.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(
            rows,
            vec![
                vec!["aa", "恭", "pin", "0", ""],
                vec!["", "日期", "pin", "1", "phrase:rq:{date}"],
                vec!["bb", "见\n了", "delete", "", ""],
            ]
        );
    }

    /// 按组替换：组内行整体换掉、位置不变；别的行、注释、CRLF、BOM 逐字节不变；新组追加。
    #[test]
    fn patch_groups_replaces_in_place_and_preserves_the_rest() {
        let orig = "\u{feff}---\r\ncolumns: [code, text, action, position, cand_id]\r\nwind: {kind: shadow, escape: true}\r\n...\r\n# 我的规则\r\naa\t恭\tpin\t0\t\r\naa\t工\tdelete\t\t\r\ncc\t从\tpin\t2\t\r\n";
        let out = patch_groups(
            orig.as_bytes(),
            Path::new("s"),
            &["code"],
            &[
                (row(&["aa"]), vec![row(&["aa", "共", "pin", "1", ""])]),
                (row(&["zz"]), vec![row(&["zz", "在", "delete", "", ""])]),
            ],
            COLS,
            "shadow",
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\u{feff}---\r\ncolumns: [code, text, action, position, cand_id]\r\nwind: {kind: shadow, escape: true}\r\n...\r\n# 我的规则\r\naa\t共\tpin\t1\t\r\ncc\t从\tpin\t2\t\r\nzz\t在\tdelete\t\t\r\n"
        );
    }

    /// 新文件：渲染表头（含 `# no comment`），按给定列序输出；新行按文件列序（列序不同也对）。
    #[test]
    fn patch_into_empty_and_foreign_column_order() {
        let out = patch_groups(
            b"",
            Path::new("s"),
            &["code"],
            &[(row(&["#a"]), vec![row(&["#a", "井\t号", "pin", "0", ""])])],
            COLS,
            "shadow",
        )
        .unwrap();
        let d = parse_table(&out, Path::new("s")).unwrap();
        assert_eq!(d.rows[0].fields, row(&["#a", "井\t号", "pin", "0", ""]));
        let foreign =
            "---\ncolumns: [text, code, action]\nwind: {kind: shadow}\n...\n恭\taa\tpin\n";
        let out = patch_groups(
            foreign.as_bytes(),
            Path::new("s"),
            &["code"],
            &[(row(&["bb"]), vec![row(&["bb", "八", "delete", "", ""])])],
            COLS,
            "shadow",
        )
        .unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .ends_with("恭\taa\tpin\n八\tbb\tdelete\n")
        );
    }

    /// 写之前文件被手改过 ⇒ 先按文件重新解析，手改（注释、顺序、新增行）不丢；`# no comment` 之后
    /// 不含 tab 的 `#` 行仍是注释。
    #[test]
    fn write_reparses_hand_edited_file_first() {
        let dir = tmpdir("handedit");
        let path = dir.join("t.dict.yaml");
        let t = TextTable::open(path.clone(), COLS, "shadow");
        t.edit(
            &[(row(&["aa"]), vec![row(&["aa", "恭", "pin", "0", ""])])],
            &["code"],
        )
        .unwrap();
        let mut s = std::fs::read_to_string(&path).unwrap();
        s.push_str("# 手写\nbb\t八\tdelete\t\t\n");
        std::fs::write(&path, &s).unwrap();
        t.edit(
            &[(row(&["cc"]), vec![row(&["cc", "从", "pin", "1", ""])])],
            &["code"],
        )
        .unwrap();
        let s = std::fs::read_to_string(&path).unwrap();
        assert!(
            s.contains("# 手写\nbb\t八\tdelete\t\t\ncc\t从\tpin\t1\t\n"),
            "{s}"
        );
        assert_eq!(t.doc().rows.len(), 3);
    }

    /// 头部坏 ⇒ 写入报错不覆盖；读按最后一次成功解析的内容；整表替换照样成功。
    #[test]
    fn isolated_table_refuses_writes_keeps_last_good() {
        let dir = tmpdir("isolated");
        let path = dir.join("t.dict.yaml");
        let t = TextTable::open(path.clone(), COLS, "shadow");
        t.edit(
            &[(row(&["aa"]), vec![row(&["aa", "恭", "pin", "0", ""])])],
            &["code"],
        )
        .unwrap();
        std::fs::write(&path, "---\ncolumns: [\n...\n").unwrap();
        assert!(!t.sync());
        assert_eq!(t.doc().rows.len(), 1, "保留上次成功解析的内容");
        assert!(t.edit(&[(row(&["bb"]), vec![])], &["code"]).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "---\ncolumns: [\n...\n",
            "不覆盖"
        );
        t.replace_all(&[row(&["cc", "从", "pin", "0", ""])])
            .unwrap();
        assert_eq!(t.doc().rows[0].fields[0], "cc");
        assert!(!t.sync(), "替换后指纹已记，无变化");
    }

    /// 命令栏文本（`$CC(...)` 等）里的 `\n` / `\t` 是字面序列，往返必须逐字不变——第一次保存就把
    /// `C:\new` 读成「C: + 换行 + ew」是写坏用户数据（P3 审查 M1）。
    #[test]
    fn cmdbar_text_roundtrips_byte_exact() {
        let dir = tmpdir("cmdbar");
        let path = dir.join("t.dict.yaml");
        let t = TextTable::open(path.clone(), COLS, "shadow");
        let txt = "$CC(C:\\new\\test.txt)";
        t.edit(
            &[(row(&["cmd"]), vec![row(&["cmd", txt, "pin", "0", ""])])],
            &["code"],
        )
        .unwrap();
        assert_eq!(t.doc().rows[0].fields[1], txt);
        assert!(std::fs::read_to_string(&path).unwrap().contains(txt));
        let t2 = TextTable::open(path, COLS, "shadow");
        assert_eq!(t2.doc().rows[0].fields[1], txt);
    }
}
