//! 手改小表（快捷格式、常用字覆盖）的「最后一次好数据」（设计 §2.5 第 2 条）：文件改坏了不当成
//! 空表，继续按上一次解析成功的内容服务，并登记隔离；修好后自动恢复。

use std::sync::{Arc, Mutex};
use wind_userdata::issues::{self, IssueKind};
use wind_userdata::text::doc::TextDoc;

struct Seen<T> {
    /// 最近一次解析过的内容（同一份内容不重复解析、不重复告警）。
    content: Option<Arc<String>>,
    good: Option<T>,
}

pub(crate) struct LastGood<T> {
    inner: Mutex<Seen<T>>,
}

impl<T> Default for LastGood<T> {
    fn default() -> Self {
        LastGood {
            inner: Mutex::new(Seen {
                content: None,
                good: None,
            }),
        }
    }
}

impl<T: Clone + Default> LastGood<T> {
    /// 当前有效内容：文件不在 ⇒ 默认值；解析成功 ⇒ 新值；解析失败 ⇒ 上一次的好值（没有则默认值）。
    pub(crate) fn get(&self, doc: &TextDoc, parse: &dyn Fn(&str) -> anyhow::Result<T>) -> T {
        let cur = doc.get();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = cur else {
            issues::clear(doc.path());
            g.content = None;
            g.good = None;
            return T::default();
        };
        let fresh = g.content.as_ref().is_none_or(|k| !Arc::ptr_eq(k, &c));
        if fresh {
            g.content = Some(c.clone());
            match parse(&c) {
                Ok(v) => {
                    issues::clear(doc.path());
                    g.good = Some(v);
                }
                Err(e) => {
                    tracing::warn!(
                        "{} 无法解析（{e:#}），按上一次的内容提供，修好后自动恢复",
                        doc.path().display()
                    );
                    issues::report(doc.path(), IssueKind::Isolated);
                }
            }
        }
        g.good.clone().unwrap_or_default()
    }
}

/// 显式整体替换（还原 / 清空）之前：当前文件解析不了 ⇒ 另存为 `.corrupt-<时间>` 并登记（§2.5：
/// 坏文件不被覆盖、也不被删）。之后 `doc` 重读（文件已不在）。
pub(crate) fn preserve_if_unparsable<T>(
    doc: &TextDoc,
    parse: &dyn Fn(&str) -> anyhow::Result<T>,
) -> anyhow::Result<()> {
    doc.sync();
    let Some(c) = doc.get() else {
        return Ok(());
    };
    if parse(&c).is_ok() {
        return Ok(());
    }
    if let Some(kept) = wind_userdata::text::fold::preserve_corrupt(doc.path())? {
        issues::report(
            doc.path(),
            IssueKind::Rebuilt {
                kept_as: kept
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
        );
    }
    doc.sync();
    Ok(())
}

/// 写入时文件解析不了的统一报错（设置页显示给用户）。
pub(crate) fn broken_file_error(doc: &TextDoc, e: anyhow::Error) -> anyhow::Error {
    issues::report(doc.path(), IssueKind::Isolated);
    anyhow::anyhow!("文件 {} 格式有误，请先修复（{e:#}）", doc.path().display())
}
