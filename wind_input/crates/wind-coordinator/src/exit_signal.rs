//! 退出信号（SIGTERM）：先落盘再退出。注销 / 停服务时 launchd、systemd、fcitx 发的是 SIGTERM，
//! 默认处理会直接结束进程，日志里还没折回的学习数据与统计收集器的内存增量就留在了 state/ 里。

use std::sync::atomic::{AtomicI32, Ordering};

/// 自管道写端：信号处理函数里只做一次 `write(2)`（异步信号安全），其余在普通线程里做。
static PIPE_W: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_term(_: libc::c_int) {
    let fd = PIPE_W.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = 1u8;
        // SAFETY: write(2) 是异步信号安全的；缓冲区是栈上的 1 字节。
        unsafe {
            libc::write(fd, (&b as *const u8).cast(), 1);
        }
    }
}

/// 装 SIGTERM 处理：收到后在专门的线程里跑 `on_term_cb`（调用方在里面落盘并退出）。只装一次；
/// 建管道 / 装处理失败只 WARN，保留系统默认行为。
pub fn install_sigterm(on_term_cb: Box<dyn FnOnce() + Send>) {
    if PIPE_W.load(Ordering::Relaxed) >= 0 {
        return;
    }
    let mut fds = [0i32; 2];
    // SAFETY: fds 是两个 i32 的数组，pipe 写入两个描述符。
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        tracing::warn!("建退出信号管道失败，SIGTERM 时不落盘");
        return;
    }
    PIPE_W.store(fds[1], Ordering::Relaxed);
    // SAFETY: sigaction 结构按 libc 约定零初始化后填处理函数；处理函数只调异步信号安全的 write。
    let ok = unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_term as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut()) == 0
    };
    if !ok {
        tracing::warn!("装 SIGTERM 处理失败，SIGTERM 时不落盘");
        return;
    }
    let rfd = fds[0];
    let _ = std::thread::Builder::new()
        .name("exit-signal".into())
        .spawn(move || {
            let mut b = [0u8; 1];
            loop {
                // SAFETY: 读入 1 字节的栈缓冲区。
                let n = unsafe { libc::read(rfd, b.as_mut_ptr().cast(), 1) };
                if n == 1 {
                    break;
                }
                if n < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                return;
            }
            tracing::info!("收到 SIGTERM：落盘后退出");
            on_term_cb();
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn sigterm_runs_the_callback_on_a_thread() {
        static RAN: AtomicBool = AtomicBool::new(false);
        install_sigterm(Box::new(|| RAN.store(true, Ordering::SeqCst)));
        unsafe {
            libc::raise(libc::SIGTERM);
        }
        for _ in 0..200 {
            if RAN.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("SIGTERM 之后回调没跑");
    }
}
