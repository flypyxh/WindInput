//! 单文件持有者（设计 §2.4 覆盖表）：一个小文件的内容常驻内存（读零 IO），写入是「持锁 → 按文件
//! 现状重读（手改优先）→ 改 → 带指纹复核的整文件提交」。编辑器恰在提交前保存 ⇒ 本次写入报错，
//! 不静默覆盖。内容的格式由调用方解释（YAML / 原文）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct DocState {
    fp: String,
    content: Option<Arc<String>>,
}

pub struct TextDoc {
    path: PathBuf,
    state: Mutex<DocState>,
}

impl TextDoc {
    /// 读文件（不存在 ⇒ 无内容）。读不了 ⇒ 无内容 + WARN（之后的 sync 会重试）。
    pub fn open(path: PathBuf) -> TextDoc {
        let d = TextDoc {
            path,
            state: Mutex::new(DocState {
                fp: String::new(),
                content: None,
            }),
        };
        d.sync();
        d
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> Option<Arc<String>> {
        self.lock().content.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DocState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 按文件现状更新内存；返回是否换了内容。
    fn reload_locked(&self, st: &mut DocState) -> anyhow::Result<bool> {
        let cur = super::fold::read_text(&self.path)?;
        let fp = super::fold::fingerprint(cur.as_ref().map(|(b, _)| b.as_slice()));
        if fp == st.fp {
            return Ok(false);
        }
        let content = match cur {
            None => None,
            Some((b, _)) => {
                Some(Arc::new(String::from_utf8(b).map_err(|_| {
                    anyhow::anyhow!("{} 不是 UTF-8", self.path.display())
                })?))
            }
        };
        st.fp = fp;
        st.content = content;
        Ok(true)
    }

    /// 文件变了（指纹不同）就重读；返回是否换了内容。读不了只 WARN。
    pub fn sync(&self) -> bool {
        let mut st = self.lock();
        match self.reload_locked(&mut st) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("读不了 {}：{e:#}", self.path.display());
                false
            }
        }
    }

    /// 读改写：`f(当前内容)` 返回新内容（`None` = 删文件）；`f` 报错（如当前内容解析不了）⇒ 原样
    /// 返回 Err、什么都不写（文件、mtime、`.bak` 都不动，§2.5）。写入带指纹复核：复核失败 ⇒ Err、
    /// 内存不变。
    pub fn modify(
        &self,
        f: &mut dyn FnMut(Option<&str>) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<()> {
        let mut st = self.lock();
        self.reload_locked(&mut st)?;
        let next = f(st.content.as_deref().map(String::as_str))?;
        match next {
            Some(s) => {
                super::fold::commit_text(&self.path, s.as_bytes(), Some(&st.fp))?;
                st.fp = super::fold::fingerprint(Some(s.as_bytes()));
                st.content = Some(Arc::new(s));
            }
            None => {
                if st.content.is_some() {
                    match std::fs::remove_file(&self.path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                    super::fold::remove_backup(&self.path);
                }
                st.fp = super::fold::fingerprint(None);
                st.content = None;
            }
        }
        Ok(())
    }

    /// 整文件替换（还原 / 迁移 / 显式回读；不复核）。`None` ⇒ 删文件。
    pub fn replace(&self, content: Option<&str>) -> anyhow::Result<()> {
        let mut st = self.lock();
        match content {
            Some(s) => {
                super::fold::commit_text(&self.path, s.as_bytes(), None)?;
                st.fp = super::fold::fingerprint(Some(s.as_bytes()));
                st.content = Some(Arc::new(s.to_string()));
            }
            None => {
                match std::fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                super::fold::remove_backup(&self.path);
                st.fp = super::fold::fingerprint(None);
                st.content = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wind_doc_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn modify_rereads_and_commits() {
        let d = tmpdir("modify");
        let doc = TextDoc::open(d.join("a.yaml"));
        assert!(doc.get().is_none());
        doc.modify(&mut |cur| {
            assert!(cur.is_none());
            Ok(Some("x: 1\n".into()))
        })
        .unwrap();
        std::fs::write(d.join("a.yaml"), "x: 2\n").unwrap(); // 手改
        doc.modify(&mut |cur| {
            assert_eq!(cur, Some("x: 2\n"));
            Ok(Some(format!("{}y: 3\n", cur.unwrap())))
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("a.yaml")).unwrap(),
            "x: 2\ny: 3\n"
        );
        assert_eq!(
            doc.get().as_deref().map(String::as_str),
            Some("x: 2\ny: 3\n")
        );
    }

    #[test]
    fn concurrent_hand_edit_during_commit_is_refused() {
        let d = tmpdir("race");
        let p = d.join("a.yaml");
        let doc = TextDoc::open(p.clone());
        doc.modify(&mut |_| Ok(Some("v: 1\n".into()))).unwrap();
        let p2 = p.clone();
        // 钩子是进程级的：只在本测试线程上动手，别的测试并发提交时不碰
        let me = std::thread::current().id();
        let hook = crate::text::fold::add_before_rename_hook(Box::new(move || {
            if std::thread::current().id() == me {
                std::fs::write(&p2, "v: hand\n").unwrap();
            }
        }));
        let r = doc.modify(&mut |_| Ok(Some("v: prog\n".into())));
        drop(hook);
        assert!(r.is_err());
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "v: hand\n",
            "手改不被覆盖"
        );
    }

    /// M-1：闭包报错（文件解析不了）⇒ 不提交：文件、mtime、`.bak` 都不动。
    #[test]
    fn modify_error_leaves_file_and_bak_untouched() {
        let d = tmpdir("moderr");
        let p = d.join("x.toml");
        std::fs::write(&p, "a = 1\n").unwrap();
        let doc = TextDoc::open(p.clone());
        doc.modify(&mut |_| Ok(Some("a = 2\n".into()))).unwrap(); // .bak = "a = 1"
        std::fs::write(&p, "a = [\n").unwrap();
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(doc.modify(&mut |_| Err(anyhow::anyhow!("坏"))).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a = [\n");
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime);
        assert_eq!(
            std::fs::read_to_string(d.join("x.toml.bak")).unwrap(),
            "a = 1\n",
            "好的 .bak 不被坏内容覆盖"
        );
    }

    #[test]
    fn none_deletes_and_sync_sees_external_changes() {
        let d = tmpdir("sync");
        let p = d.join("a.yaml");
        let doc = TextDoc::open(p.clone());
        doc.modify(&mut |_| Ok(Some("a\n".into()))).unwrap();
        std::fs::write(&p, "b\n").unwrap();
        assert!(doc.sync());
        assert!(!doc.sync());
        doc.modify(&mut |_| Ok(None)).unwrap();
        assert!(!p.exists() && doc.get().is_none());
        doc.replace(Some("c\n")).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "c\n");
    }
}
