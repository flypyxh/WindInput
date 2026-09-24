//! 派生缓存构建的「死亡记录」：让一次必败的构建不会在每次启动时重演。
//!
//! # 为什么不是 `try_reserve` / `catch_unwind`
//!
//! 构建期峰值来自几十万次**小**分配（每个词条的 `String`、每个编码的 `Vec`），而不是某一块
//! 可以预先 `try_reserve` 的大缓冲区。其中任何一次失败都会走 std 的 `handle_alloc_error`
//! → 打印 `memory allocation of N bytes failed` → `process::abort()`：**不 unwind、不可捕获**。
//! release 又是 `panic = "abort"`，连 panic 也捕获不了。于是「在进程内接住失败」这条路在
//! 稳定版 Rust 上不存在，只能换个问法：**下次启动时，知道上次死在了构建里**。
//!
//! # 机制
//!
//! 构建前在缓存旁写 `<cache>.building`（内容：输入键 + 已尝试次数），构建阶段完成后删掉。
//! 进程若死在中间，标记留在盘上。下次启动：
//! - 标记的输入键与本次相同、且已连续失败 [`MAX_ATTEMPTS`] 次 → **跳过构建**，调用方降级
//!   （该词库本次不可用），并给出可读原因；
//! - 输入键不同（源文件变了）→ 计数归零，重新尝试；
//! - 手动删掉标记文件 → 重新尝试。
//!
//! 允许一次重试，是因为内存不足常是瞬时的（当时开着别的大程序）。

use std::path::{Path, PathBuf};

/// 同一份输入连续死在构建中多少次后停止尝试。
pub const MAX_ATTEMPTS: u32 = 2;

fn marker_path(cache: &Path) -> PathBuf {
    let mut s = cache.as_os_str().to_os_string();
    s.push(".building");
    PathBuf::from(s)
}

/// 读标记：`(输入键, 已尝试次数)`。不存在或内容残缺都当作没有记录。
fn read_marker(marker: &Path) -> Option<(String, u32)> {
    let text = std::fs::read_to_string(marker).ok()?;
    let mut lines = text.lines();
    let key = lines.next()?.to_string();
    let count = lines.next()?.trim().parse().ok()?;
    Some((key, count))
}

/// 一次已登记的构建尝试。构建阶段结束后必须调 [`BuildAttempt::finish`]；
/// **不调用就等同于进程死在了构建里**（标记留在盘上，下次启动计为一次失败）。
#[must_use = "构建阶段结束后须调用 finish()，否则下次启动会把它记为一次失败"]
pub struct BuildAttempt {
    marker: PathBuf,
}

impl BuildAttempt {
    /// 构建阶段已走完（无论产物是否可用），撤掉死亡记录。
    pub fn finish(self) {
        if let Err(e) = std::fs::remove_file(&self.marker)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "无法删除构建标记 {}: {e}。下次启动会把这次成功的构建误记为一次失败。",
                self.marker.display()
            );
        }
    }
}

/// [`begin`] 的结论。
pub enum Gate {
    /// 可以构建；构建阶段结束后调 `finish()`。
    Proceed(BuildAttempt),
    /// 同一份输入已连续死在构建中，本次跳过。`reason` 可直接写进日志。
    Skip { reason: String },
}

/// 登记一次构建尝试。`key` 须唯一标识这次构建的全部输入（通常是源指纹）。
///
/// 标记写不进去（缓存目录不可写）时照常放行：此时构建产物本身多半也落不了盘，
/// 退避机制失效只是退回改动前的行为，不能因此让词库无法构建。
pub fn begin(cache: &Path, key: &str) -> Gate {
    let marker = marker_path(cache);
    let prev = match read_marker(&marker) {
        Some((k, n)) if k == key => n,
        _ => 0,
    };
    if prev >= MAX_ATTEMPTS {
        return Gate::Skip {
            reason: format!(
                "构建 {} 已连续 {prev} 次在中途导致进程退出（最常见的原因是内存不足），\
                 本次跳过、该词库暂不可用。词库文件变化后会自动重试；释放内存后也可\
                 在设置里重建词库缓存，或删除 {} 强制重试。",
                cache.display(),
                marker.display()
            ),
        };
    }
    if prev > 0 {
        tracing::warn!(
            "上次构建 {} 时进程在中途退出（第 {prev} 次，常见原因是内存不足），本次再试一次。",
            cache.display()
        );
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&marker, format!("{key}\n{}\n", prev + 1)) {
        tracing::warn!(
            "无法写入构建标记 {}: {e}。若本次构建因内存不足崩溃，下次启动无法识别、会再次重试。",
            marker.display()
        );
    }
    Gate::Proceed(BuildAttempt { marker })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独占一个目录（按用例名 + pid），保证并行跑互不干扰。
    fn cache_in(case: &str) -> (TempDir, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("wind_build_guard_{case}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("a.wdat");
        (TempDir(dir), cache)
    }

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn proceed(g: Gate) -> BuildAttempt {
        match g {
            Gate::Proceed(a) => a,
            Gate::Skip { reason } => panic!("不该跳过: {reason}"),
        }
    }

    /// 连续死在构建里 MAX_ATTEMPTS 次后，同一份输入不再尝试。
    #[test]
    fn skips_after_repeated_deaths_with_same_input() {
        let (_d, cache) = cache_in("skips");
        for _ in 0..MAX_ATTEMPTS {
            drop(proceed(begin(&cache, "k1"))); // 模拟进程死在构建中：不 finish
        }
        match begin(&cache, "k1") {
            Gate::Skip { reason } => {
                assert!(reason.contains("内存不足"), "原因要可读: {reason}");
                assert!(
                    reason.contains(".building"),
                    "要告诉用户怎么手动重试: {reason}"
                );
            }
            Gate::Proceed(_) => panic!("同一输入已连续失败，应跳过"),
        }
    }

    /// 源文件变了（输入键不同）就重新尝试。
    #[test]
    fn retries_when_input_changes() {
        let (_d, cache) = cache_in("input_changes");
        for _ in 0..MAX_ATTEMPTS {
            drop(proceed(begin(&cache, "k1")));
        }
        proceed(begin(&cache, "k2")).finish();
    }

    /// 成功的构建不留记录，之后任意次都放行。
    #[test]
    fn finished_build_leaves_no_record() {
        let (_d, cache) = cache_in("finished");
        for _ in 0..MAX_ATTEMPTS + 2 {
            proceed(begin(&cache, "k1")).finish();
        }
        assert!(!marker_path(&cache).exists());
    }

    /// 删掉标记文件即手动重试。
    #[test]
    fn deleting_marker_forces_retry() {
        let (_d, cache) = cache_in("delete");
        for _ in 0..MAX_ATTEMPTS {
            drop(proceed(begin(&cache, "k1")));
        }
        std::fs::remove_file(marker_path(&cache)).unwrap();
        proceed(begin(&cache, "k1")).finish();
    }
}
