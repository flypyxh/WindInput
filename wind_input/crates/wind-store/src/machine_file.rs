//! 机器写的小文件（补全历史、统计；设计 §2.5 第 3 条）：解析失败先从 `.bak` 自愈，`.bak` 也不行
//! 就把坏文件另存为 `.corrupt-<时间>` 后从空重建。坏文件本身从不被覆盖。

use std::path::Path;
use wind_userdata::issues::{self, IssueKind};
use wind_userdata::text::fold;

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    s.into()
}

fn kept_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 读机写文件：不存在 ⇒ `Ok(None)`；解析成功 ⇒ `Ok(Some(v))`；解析失败 ⇒ 坏文件另存留存，
/// `.bak` 能解析就把它恢复成正文并返回其内容，否则返回 `Ok(None)`（调用方从空开始）。两种情况都
/// 登记 `Rebuilt`。读不了（IO）⇒ `Err`。
pub(crate) fn load_machine_file<T>(
    path: &Path,
    parse: &dyn Fn(&[u8]) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let Some((bytes, _)) = fold::read_text(path)? else {
        return Ok(None);
    };
    let err = match parse(&bytes) {
        Ok(v) => return Ok(Some(v)),
        Err(e) => e,
    };
    tracing::warn!("{} 无法解析（{err:#}），尝试从备份恢复", path.display());
    let bak = with_suffix(path, ".bak");
    let restored = match fold::read_text(&bak)? {
        Some((b, _)) => parse(&b).ok().map(|v| (b, v)),
        None => None,
    };
    let kept = fold::preserve_corrupt(path)?;
    let kept_as = kept.as_deref().map(kept_name).unwrap_or_default();
    issues::report(path, IssueKind::Rebuilt { kept_as });
    match restored {
        Some((b, v)) => {
            fold::commit_text(path, &b, None)?;
            tracing::warn!("{} 已从备份恢复", path.display());
            Ok(Some(v))
        }
        None => {
            tracing::warn!("{} 备份也不可用，已从空重建", path.display());
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_mf_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn num(b: &[u8]) -> anyhow::Result<u32> {
        Ok(std::str::from_utf8(b)?.trim().parse()?)
    }

    #[test]
    fn good_file_loads_and_missing_is_none() {
        let d = dir("good");
        let f = d.join("a");
        assert_eq!(load_machine_file(&f, &num).unwrap(), None);
        std::fs::write(&f, "7").unwrap();
        assert_eq!(load_machine_file(&f, &num).unwrap(), Some(7));
    }

    #[test]
    fn corrupt_file_heals_from_bak_and_is_kept_aside() {
        let d = dir("heal");
        let f = d.join("a");
        std::fs::write(&f, "x").unwrap();
        std::fs::write(d.join("a.bak"), "5").unwrap();
        assert_eq!(load_machine_file(&f, &num).unwrap(), Some(5));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "5");
        let kept: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("a.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(kept[0].path()).unwrap(), "x");
        assert!(matches!(
            issues::list_under(&f)[0].kind,
            IssueKind::Rebuilt { .. }
        ));
        issues::clear(&f);
    }

    #[test]
    fn corrupt_file_without_bak_starts_empty() {
        let d = dir("empty");
        let f = d.join("a");
        std::fs::write(&f, "x").unwrap();
        std::fs::write(d.join("a.bak"), "y").unwrap();
        assert_eq!(load_machine_file(&f, &num).unwrap(), None);
        assert!(!f.exists());
        issues::clear(&f);
    }
}
