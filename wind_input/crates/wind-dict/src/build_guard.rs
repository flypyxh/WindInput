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
//! 构建前在缓存旁写 `<cache>.building`（内容：输入键 + 已登记次数 + 持有者身份），构建阶段
//! 完成后删掉。进程若死在中间，标记留在盘上。下次启动：
//! - 标记的输入键与本次相同、且已连续失败 [`MAX_ATTEMPTS`] 次 → **跳过构建**，调用方降级
//!   （该词库本次不可用），并给出可读原因；
//! - 标记的持有者**仍然活着** → 那是别的进程/线程正在建，不是死亡，**不计数**、照常放行
//!   （各自构建，最后写入者生效）。只有持有者已死，它那次登记才算一次失败。
//!   持有者身份 = pid + 进程启动时间（防 pid 复用）；判活失败时保守当作活着——
//!   宁可少退避一次，也不能把并发的在建构建误判成死亡而让词库不可用。
//!   启动时间取不到的平台（Linux / Windows / macOS 以外）写 `pid 0`，读端见 0 一律当活着；
//!   只有旧格式（无持有者行）才当已死；
//! - 输入键不同（源文件变了）→ 计数归零，重新尝试；
//! - 手动删掉标记文件 → 重新尝试。
//!
//! 允许一次重试，是因为内存不足常是瞬时的（当时开着别的大程序）。
//!
//! # 已知的偏保守取舍
//!
//! - 标记只有一个持有者行：活着的持有者被后来者覆盖后，若前者随后死在构建里，这次死亡
//!   不会被记上（少记一次）。代价只是多重试一次，换来的是不必维护持有者列表。
//! - Linux 判活看 `/proc/<pid>/stat`：已退出但尚未被父进程回收的**僵尸进程**仍在那里、
//!   启动时间也对得上，会被当成活着，直到被回收。同样只会少记一次死亡。

use std::path::{Path, PathBuf};

/// 同一份输入连续死在构建中多少次后停止尝试。
pub const MAX_ATTEMPTS: u32 = 2;

fn marker_path(cache: &Path) -> PathBuf {
    let mut s = cache.as_os_str().to_os_string();
    s.push(".building");
    PathBuf::from(s)
}

/// 进程身份：pid + 启动时间。只比 pid 会被复用的 pid 骗过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holder {
    pid: u32,
    start: u64,
}

impl Holder {
    /// 取不到启动时间时记 `start = 0`（＝判不了），读端据此当活着。
    fn current() -> Holder {
        let pid = std::process::id();
        Holder {
            pid,
            start: process_start_time(pid).unwrap_or(0),
        }
    }

    /// 持有者是否仍在运行。**判不了就当活着**（见模块文档）。
    fn is_alive(self) -> bool {
        if self.start == 0 {
            return true;
        }
        match process_start_time(self.pid) {
            Some(start) => start == self.start,
            None => !process_definitely_gone(self.pid),
        }
    }
}

/// 进程启动时间（单位各平台自定，只用于相等比较）。取不到返回 `None`。
#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 字段可含空格与括号，从最后一个 ')' 之后数：state 是第 3 字段，starttime 是第 22 个。
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(windows)]
fn process_start_time(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: 只查询、句柄当场关闭。
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let mut code = 0u32;
        let times = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        // 已退出但句柄仍被别人持有的进程还能打开，要看退出码。
        let running = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(h);
        if !times {
            return None;
        }
        // 已退出：返回一个不可能等于任何记录值的启动时间，让比较判为已死。
        if !running {
            return Some(u64::MAX);
        }
        Some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<u64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: 缓冲区即 info 本身，长度如实传入；只读查询。
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

/// 取不到启动时间时，能否**确定**该进程已不存在。确定不了一律 `false`（=当作活着）。
#[cfg(target_os = "linux")]
fn process_definitely_gone(pid: u32) -> bool {
    // /proc 本身可读而该 pid 目录不存在 ⇒ 进程确实没了。
    Path::new("/proc/self").exists() && !Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(windows)]
fn process_definitely_gone(pid: u32) -> bool {
    use windows::Win32::Foundation::ERROR_INVALID_PARAMETER;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: 只探测能否打开；成功则立即关闭。
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let _ = windows::Win32::Foundation::CloseHandle(h);
                false
            }
            // pid 不存在时 OpenProcess 报 ERROR_INVALID_PARAMETER；拒绝访问等则判不了。
            Err(e) => e.code() == ERROR_INVALID_PARAMETER.to_hresult(),
        }
    }
}

#[cfg(target_os = "macos")]
fn process_definitely_gone(pid: u32) -> bool {
    // SAFETY: 信号 0 只做存在性检查，不投递任何信号。
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_definitely_gone(_pid: u32) -> bool {
    false
}

/// 盘上的标记。`count` 是已登记的尝试次数（**含**持有者自己那次）；
/// `holder` 缺失（旧格式）当作已死；持有者 `start == 0` 表示写端判不了，当作活着。
struct Marker {
    key: String,
    count: u32,
    holder: Option<Holder>,
}

/// 读标记。不存在或内容残缺都当作没有记录。
fn read_marker(marker: &Path) -> Option<Marker> {
    let raw = std::fs::read_to_string(marker).ok()?;
    // 标记是本进程自己写的，但缓存目录可能被用户拷来拷去；孤立 \r 会让整份成一行、
    // 计数读不出 ⇒ 当作无标记放行，退避失效。
    let normalized = wind_utils::text::normalize_input(&raw);
    let mut lines = normalized.lines();
    let key = lines.next()?.to_string();
    let count = lines.next()?.trim().parse().ok()?;
    let holder = lines.next().and_then(|l| {
        let (pid, start) = l.trim().split_once(' ')?;
        Some(Holder {
            pid: pid.parse().ok()?,
            start: start.parse().ok()?,
        })
    });
    Some(Marker { key, count, holder })
}

/// 进程内按标记路径串行化 `begin` 的读-改-写，免得同进程两个线程读到同一计数。
fn marker_lock(marker: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(marker.to_path_buf())
        .or_default()
        .clone()
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
    let lock = marker_lock(&marker);
    let _serial = lock.lock().unwrap_or_else(|e| e.into_inner());
    // prev = 已确认死在构建中的次数。持有者还活着 ⇒ 它那次登记是「在建」而非「死亡」。
    let prev = match read_marker(&marker) {
        Some(m) if m.key == key => {
            let holder_alive = m.holder.is_some_and(Holder::is_alive);
            if holder_alive {
                m.count.saturating_sub(1)
            } else {
                m.count
            }
        }
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
    let h = Holder::current();
    let holder = format!("{} {}\n", h.pid, h.start);
    if let Err(e) = std::fs::write(&marker, format!("{key}\n{}\n{holder}", prev + 1)) {
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

    /// 模拟一次「进程死在构建中」：登记后不 finish，再把持有者改成一个已死的进程。
    fn die_in_build(cache: &Path, key: &str) {
        drop(proceed(begin(cache, key)));
        let marker = marker_path(cache);
        let m = read_marker(&marker).expect("begin 应已写入标记");
        std::fs::write(
            &marker,
            format!("{}\n{}\n{} 1\n", m.key, m.count, dead_pid()),
        )
        .unwrap();
    }

    /// 一个肯定已退出的进程的 pid：起一个立即结束的子进程并回收。
    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    /// 连续死在构建里 MAX_ATTEMPTS 次后，同一份输入不再尝试。
    #[test]
    fn skips_after_repeated_deaths_with_same_input() {
        let (_d, cache) = cache_in("skips");
        for _ in 0..MAX_ATTEMPTS {
            die_in_build(&cache, "k1");
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
            die_in_build(&cache, "k1");
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
            die_in_build(&cache, "k1");
        }
        std::fs::remove_file(marker_path(&cache)).unwrap();
        proceed(begin(&cache, "k1")).finish();
    }

    /// ★ 回归（审查 H1）：持有者还活着的登记是「别人在建」，不是死亡。
    /// 同 key 两个活着的 begin 都不 finish，第三次仍须放行。
    #[test]
    fn live_concurrent_builds_are_not_counted_as_deaths() {
        let (_d, cache) = cache_in("live");
        let a = proceed(begin(&cache, "k1"));
        let b = proceed(begin(&cache, "k1"));
        let c = proceed(begin(&cache, "k1"));
        drop((a, b));
        c.finish();
    }

    /// 持有者已死一次还不够，第二次死才跳过。
    #[test]
    fn skips_only_after_holder_died_max_times() {
        let (_d, cache) = cache_in("dead_twice");
        die_in_build(&cache, "k1");
        drop(proceed(begin(&cache, "k1"))); // 第一次死后仍放行
        let marker = marker_path(&cache);
        let m = read_marker(&marker).unwrap();
        std::fs::write(
            &marker,
            format!("{}\n{}\n{} 1\n", m.key, m.count, dead_pid()),
        )
        .unwrap();
        assert!(matches!(begin(&cache, "k1"), Gate::Skip { .. }));
    }

    /// 旧格式标记（无持有者行）按已死处理，与改动前语义一致。
    #[test]
    fn legacy_marker_without_holder_counts_as_dead() {
        let (_d, cache) = cache_in("legacy");
        std::fs::write(marker_path(&cache), format!("k1\n{MAX_ATTEMPTS}\n")).unwrap();
        assert!(matches!(begin(&cache, "k1"), Gate::Skip { .. }));
    }

    /// 写端判不了启动时间（未知平台）时记 `pid 0`：读端须当作活着、不计数，
    /// 否则这些平台上并发的在建构建会被当成死亡（审查 H1 复现）。
    #[test]
    fn holder_with_unknown_start_counts_as_alive() {
        let (_d, cache) = cache_in("unknown_start");
        std::fs::write(
            marker_path(&cache),
            format!("k1\n{MAX_ATTEMPTS}\n{} 0\n", dead_pid()),
        )
        .unwrap();
        proceed(begin(&cache, "k1")).finish();
    }

    #[test]
    fn current_process_is_alive_and_dead_child_is_not() {
        let me = Holder::current();
        assert_ne!(me.start, 0, "Linux / Windows / macOS 应能取到自身启动时间");
        assert!(me.is_alive());
        assert!(
            !Holder {
                pid: dead_pid(),
                start: 1
            }
            .is_alive()
        );
        assert!(
            !Holder {
                start: me.start ^ 1,
                ..me
            }
            .is_alive(),
            "pid 复用要识破"
        );
    }
}
