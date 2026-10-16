//! 文本目标的文件层（设计 §3.5 第 1 步的读、写、复核）。引擎侧的折回 / 同步 / 整文件替换在
//! `engine.rs`（要动视图与 state），这里只管文件：指纹、读、带复核的提交。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Hook = Box<dyn Fn() + Send + Sync>;

static BEFORE_RECHECK: Mutex<Vec<(u64, std::sync::Arc<Hook>)>> = Mutex::new(Vec::new());
static NEXT_HOOK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 测试钩子：提交时「tmp 已写好、复核原文件指纹之前」调用——借此模拟编辑器恰在折回期间保存。
/// 进程级、可并存（并发的测试各装各的，钩子里自己按线程过滤）；守卫 drop 时只卸下这一个。
#[doc(hidden)]
pub fn add_before_rename_hook(h: Hook) -> HookGuard {
    let id = NEXT_HOOK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    BEFORE_RECHECK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, std::sync::Arc::new(h)));
    HookGuard(id)
}

#[doc(hidden)]
pub struct HookGuard(u64);

impl Drop for HookGuard {
    fn drop(&mut self) {
        BEFORE_RECHECK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != self.0);
    }
}

/// 内容指纹：`"{len:x}-{crc32:08x}"`；文件不存在为 `"absent"`。
pub fn fingerprint(bytes: Option<&[u8]>) -> String {
    match bytes {
        Some(b) => format!("{:x}-{:08x}", b.len(), crc32fast::hash(b)),
        None => "absent".to_string(),
    }
}

/// 读文件：不存在 ⇒ `Ok(None)`；其他错误（权限、被占用）⇒ `Err`（调用方本轮跳过，不当成删除）。
pub fn read_text(path: &Path) -> anyhow::Result<Option<(Vec<u8>, i64)>> {
    match std::fs::read(path) {
        Ok(b) => {
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Ok(Some((b, mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("读不了 {}：{e}", path.display())),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// UTC `YYYYMMDD-HHMMSS`（不引 chrono：civil-from-days 换算）。
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// 把解析不了的文件改名留存为 `<文件名>.corrupt-<UTC 时间>`（设计 §2.5：坏文件不被覆盖）。
/// 文件不存在 ⇒ `Ok(None)`；同一秒已有同名留存 ⇒ 名字后加 `-2`、`-3`…
pub fn preserve_corrupt(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let base = with_suffix(path, &format!(".corrupt-{}", utc_stamp()));
    let mut dest = base.clone();
    let mut n = 2;
    while dest.exists() {
        dest = with_suffix(&base, &format!("-{n}"));
        n += 1;
    }
    std::fs::rename(path, &dest)?;
    tracing::warn!("{} 无法解析，已另存为 {}", path.display(), dest.display());
    Ok(Some(dest))
}

/// §3.5 第 1 步的写回：tmp → fsync → （`expect_fp` 给出时）复核原文件指纹未变 → 原文件存在则留
/// `.bak` → rename → fsync 目录（Windows 对目标再 `sync_all`）。复核不过 ⇒ 删 tmp、`Err`。
pub fn commit_text(path: &Path, bytes: &[u8], expect_fp: Option<&str>) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无父目录: {}", path.display()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = with_suffix(path, ".tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    // 拷出来再调：钩子可能阻塞（模拟慢 fsync），别让它占着锁卡住别的测试的提交
    let hooks: Vec<_> = BEFORE_RECHECK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(_, h)| h.clone())
        .collect();
    for h in hooks {
        h();
    }
    if let Some(fp) = expect_fp {
        let now = read_text(path)?;
        if fingerprint(now.as_ref().map(|(b, _)| b.as_slice())) != fp {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("{} 在折回期间被修改，本轮放弃", path.display());
        }
    }
    if path.is_file() {
        std::fs::copy(path, with_suffix(path, ".bak"))?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = crate::state::fsync_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn preserve_corrupt_renames_with_stamp_and_never_collides() {
        let d = std::env::temp_dir().join(format!("wind_preserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.toml");
        assert!(super::preserve_corrupt(&f).unwrap().is_none());
        std::fs::write(&f, "1").unwrap();
        let p1 = super::preserve_corrupt(&f).unwrap().unwrap();
        std::fs::write(&f, "2").unwrap();
        let p2 = super::preserve_corrupt(&f).unwrap().unwrap();
        assert_ne!(p1, p2);
        assert!(!f.exists());
        let n = p1.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            n.starts_with("a.toml.corrupt-20") && n.len() == "a.toml.corrupt-".len() + 15,
            "{n}"
        );
        assert_eq!(std::fs::read_to_string(&p2).unwrap(), "2");
        let _ = std::fs::remove_dir_all(&d);
    }

    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 钩子是进程级的，并发的测试各装各的：一个测试卸下自己的钩子，不能顺手卸掉别人的。
    #[test]
    fn removing_one_hook_keeps_the_others() {
        let d = std::env::temp_dir().join(format!("wind_fold_hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let me = std::thread::current().id();
        let (a, b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (a2, b2) = (a.clone(), b.clone());
        let ga = add_before_rename_hook(Box::new(move || {
            if std::thread::current().id() == me {
                a2.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let gb = add_before_rename_hook(Box::new(move || {
            if std::thread::current().id() == me {
                b2.fetch_add(1, Ordering::SeqCst);
            }
        }));
        drop(gb);
        commit_text(&d.join("a.yaml"), b"x\n", None).unwrap();
        drop(ga);
        commit_text(&d.join("a.yaml"), b"y\n", None).unwrap();
        assert_eq!((a.load(Ordering::SeqCst), b.load(Ordering::SeqCst)), (1, 0));
        let _ = std::fs::remove_dir_all(&d);
    }
}
