//! Linux 外部宿主形态（`linux-host`）下的系统能力：剪贴板、打开路径。
//!
//! 与 macOS 的 `pbcopy` / `open` 同一路数——服务进程不链接任何桌面库，全部经外部命令：
//! Wayland 会话用 `wl-clipboard`，X11 会话用 `xclip`（其次 `xsel`），打开走 `xdg-open`。
//! 工具缺失时**如实报错**，不返回假成功（同 `screenshot::copy_bgra_to_clipboard` 的既有约定）。
//!
//! ⚠ 服务进程由 systemd 用户单元拉起时可能没有 `WAYLAND_DISPLAY` / `DISPLAY`，此时选不到
//! 后端；安装单元须 `import-environment`，或由 addon 拉起服务以继承会话环境。
//!
//! 剪贴板命令一律限时（[`TIMEOUT`]）：X11 选区与 Wayland 剪贴板都要持有方应答，持有方挂住时
//! `xclip -o` / `wl-paste` 会一直等，调用线程跟着挂。超时即杀子进程并回收，按失败处理。

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 剪贴板命令的最长等待。正常情况下几十毫秒内返回。
const TIMEOUT: Duration = Duration::from_secs(2);

/// 子进程退出后，等旁线程把管道读完 / 写完的宽限。子进程自己 fork 出去的后代还占着管道时
/// 读写线程会一直阻塞，到点就不等它了（线程随管道关闭自行结束）。
const PIPE_GRACE: Duration = Duration::from_millis(200);

/// 剪贴板后端。按会话类型择一：有 `WAYLAND_DISPLAY` 优先 wl-clipboard，否则 X11 工具。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    WlClipboard,
    Xclip,
    Xsel,
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn pick_backend() -> Option<Backend> {
    pick_backend_with(env_set, have)
}

/// 选后端的判据本体：环境变量与命令是否存在都由参数给出，测试不必改进程环境。
fn pick_backend_with(env: impl Fn(&str) -> bool, have: impl Fn(&str) -> bool) -> Option<Backend> {
    if env("WAYLAND_DISPLAY") && have("wl-copy") {
        return Some(Backend::WlClipboard);
    }
    if env("DISPLAY") {
        if have("xclip") {
            return Some(Backend::Xclip);
        }
        if have("xsel") {
            return Some(Backend::Xsel);
        }
    }
    None
}

fn no_backend() -> anyhow::Error {
    anyhow::anyhow!(
        "找不到可用的剪贴板工具（Wayland 需 wl-clipboard，X11 需 xclip 或 xsel），或会话环境变量缺失"
    )
}

/// 等子进程退出，最多 `timeout`；到点就杀掉并回收（不留僵尸），报错。
fn wait_timeout(child: &mut Child, name: &str, timeout: Duration) -> anyhow::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{name} 超过 {timeout:?} 未结束，已终止");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn program_name(cmd: &Command) -> String {
    format!("{:?}", cmd.get_program())
}

fn pipe_into(cmd: Command, data: &[u8]) -> anyhow::Result<()> {
    pipe_into_within(cmd, data, TIMEOUT)
}

fn pipe_into_within(mut cmd: Command, data: &[u8], timeout: Duration) -> anyhow::Result<()> {
    let name = program_name(&cmd);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("{name} 启动失败: {e}"))?;
    // 写在旁线程：对端不读时 write_all 会卡在写满的管道上，限时就无从谈起。写完 drop 掉
    // stdin 让对端读到 EOF；xclip / wl-copy 随后自行转入后台持有选区，wait 很快返回。
    let mut stdin = child.stdin.take();
    let data = data.to_vec();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = stdin.as_mut().map_or(Ok(()), |s| s.write_all(&data));
        drop(stdin);
        let _ = tx.send(r);
    });
    let status = wait_timeout(&mut child, &name, timeout)?;
    match rx.recv_timeout(PIPE_GRACE) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => anyhow::bail!("{name} 写入失败: {e}"),
        Err(_) => anyhow::bail!("{name} 已退出，但数据没写完"),
    }
    if !status.success() {
        anyhow::bail!("{name} 退出码非零: {status}");
    }
    Ok(())
}

/// 跑一条命令取 stdout，限时 `timeout`。
fn capture_within(mut cmd: Command, timeout: Duration) -> anyhow::Result<Vec<u8>> {
    let name = program_name(&cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("{name} 启动失败: {e}"))?;
    // 读在旁线程：输出大于管道容量时，不边读边等子进程就会互相卡住。
    let mut stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let r = stdout
            .as_mut()
            .map_or(Ok(0), |s| s.read_to_end(&mut buf))
            .map(|_| buf);
        let _ = tx.send(r);
    });
    let status = wait_timeout(&mut child, &name, timeout)?;
    let out = match rx.recv_timeout(PIPE_GRACE) {
        Ok(r) => r.map_err(|e| anyhow::anyhow!("{name} 读取失败: {e}"))?,
        Err(_) => anyhow::bail!("{name} 已退出，但输出管道仍被占着"),
    };
    if !status.success() {
        anyhow::bail!("{name} 退出码非零: {status}");
    }
    Ok(out)
}

/// 写文本剪贴板。
pub fn set_text(text: &str) -> anyhow::Result<()> {
    match pick_backend().ok_or_else(no_backend)? {
        Backend::WlClipboard => pipe_into(Command::new("wl-copy"), text.as_bytes()),
        Backend::Xclip => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard"]);
            pipe_into(c, text.as_bytes())
        }
        Backend::Xsel => {
            let mut c = Command::new("xsel");
            c.args(["--clipboard", "--input"]);
            pipe_into(c, text.as_bytes())
        }
    }
}

/// 读文本剪贴板；任何失败（无工具、剪贴板为空、非文本）都按空串处理，与 Windows 侧一致。
pub fn get_text() -> String {
    let Some(backend) = pick_backend() else {
        return String::new();
    };
    let cmd = match backend {
        Backend::WlClipboard => {
            let mut c = Command::new("wl-paste");
            c.args(["--no-newline", "--type", "text"]);
            c
        }
        Backend::Xclip => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-o"]);
            c
        }
        Backend::Xsel => {
            let mut c = Command::new("xsel");
            c.args(["--clipboard", "--output"]);
            c
        }
    };
    match capture_within(cmd, TIMEOUT) {
        Ok(out) => String::from_utf8_lossy(&out).into_owned(),
        Err(e) => {
            tracing::debug!("读剪贴板失败: {e}");
            String::new()
        }
    }
}

/// 把 PNG 字节写入图片剪贴板。
pub fn set_png(png: &[u8]) -> anyhow::Result<()> {
    match pick_backend().ok_or_else(no_backend)? {
        Backend::WlClipboard => {
            let mut c = Command::new("wl-copy");
            c.args(["--type", "image/png"]);
            pipe_into(c, png)
        }
        Backend::Xclip => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-t", "image/png"]);
            pipe_into(c, png)
        }
        Backend::Xsel => anyhow::bail!("xsel 不支持图片剪贴板，请安装 xclip 或 wl-clipboard"),
    }
}

/// 按默认应用打开路径 / URL（不等待；后台回收，不留僵尸）。
///
/// 不限时：调用方本就不等它，而 xdg-open 在部分桌面上会前台跑到被打开的程序退出，
/// 到点杀它可能连带杀掉被打开的程序。
pub fn open(target: &str) -> std::io::Result<()> {
    crate::manager::spawn_reaped(
        Command::new("xdg-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 `WAYLAND_DISPLAY` / `DISPLAY` 时工具再全也不选（避免在无头环境里误起进程）；
    /// 有会话时按 wl-clipboard → xclip → xsel 的顺序取第一个装了的。
    #[test]
    fn backend_follows_session_env_then_tool_order() {
        let pick = |env: &[&str], tools: &[&str]| {
            pick_backend_with(|n| env.contains(&n), |c| tools.contains(&c))
        };
        let all = ["wl-copy", "xclip", "xsel"];
        assert_eq!(pick(&[], &all), None);
        assert_eq!(
            pick(&["WAYLAND_DISPLAY", "DISPLAY"], &all),
            Some(Backend::WlClipboard)
        );
        // Wayland 会话缺 wl-clipboard：经 XWayland 的 DISPLAY 退到 X11 工具。
        assert_eq!(
            pick(&["WAYLAND_DISPLAY", "DISPLAY"], &["xclip", "xsel"]),
            Some(Backend::Xclip)
        );
        assert_eq!(pick(&["WAYLAND_DISPLAY"], &["xclip", "xsel"]), None);
        assert_eq!(pick(&["DISPLAY"], &["xsel"]), Some(Backend::Xsel));
        assert_eq!(pick(&["DISPLAY"], &["wl-copy"]), None);
    }

    /// 本测试独占的临时目录（进程号 + 用例名，并发用例不撞）。
    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_lh_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 假剪贴板工具：不读 stdin、不写 stdout，起一个后代 `sleep` 一直占着继承来的管道——
    /// 模拟持有方挂住时的 `xclip` / `wl-paste`。故意不 `exec`：杀掉的是 sh，后代还占着管道，
    /// 正好覆盖「子进程死了、管道没关」那条路。
    fn hanging_tool(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\nsleep 5\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// 本进程名下处于 Z（僵尸）态、进程名为 `comm` 的子进程数。
    fn zombies_named(comm: &str) -> usize {
        let me = std::process::id().to_string();
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
            .filter(|stat| {
                // 「pid (comm) state ppid …」；comm 可含空格，按最后一个 ')' 切。
                let Some((head, tail)) = stat.rsplit_once(')') else {
                    return false;
                };
                let name = head.split_once('(').map_or("", |(_, n)| n);
                let mut f = tail.split_whitespace();
                let (state, ppid) = (f.next(), f.next());
                name == comm && state == Some("Z") && ppid == Some(me.as_str())
            })
            .count()
    }

    #[test]
    fn hanging_reader_times_out_and_is_reaped() {
        let dir = scratch("read");
        let tool = hanging_tool(&dir, "wifakepaste");
        let t = Instant::now();
        let r = capture_within(Command::new(&tool), Duration::from_millis(300));
        assert!(r.is_err(), "挂住的读取应当报错");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(zombies_named("wifakepaste"), 0, "超时杀掉的子进程要回收");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 工具自己退出了，但留下的后代还占着 stdout（xclip / wl-copy 会 fork 到后台）：
    /// 读线程等不到 EOF，过了宽限就放弃，不陪着它挂。
    #[test]
    fn descendant_holding_stdout_does_not_hang_reader() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("detach");
        let p = dir.join("wifakedetach");
        std::fs::write(&p, "#!/bin/sh\nsleep 5 &\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let t = Instant::now();
        assert!(capture_within(Command::new(&p), TIMEOUT).is_err());
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hanging_writer_times_out_and_is_reaped() {
        let dir = scratch("write");
        let tool = hanging_tool(&dir, "wifakecopy");
        // 远大于管道容量（64 KiB）：对端不读时 write_all 必然卡住，验证写不阻塞限时。
        let data = vec![b'x'; 1 << 20];
        let t = Instant::now();
        let r = pipe_into_within(Command::new(&tool), &data, Duration::from_millis(300));
        assert!(r.is_err(), "挂住的写入应当报错");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(zombies_named("wifakecopy"), 0, "超时杀掉的子进程要回收");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 正常的工具照常工作：读到 stdout、写进去的字节对端读得到。
    #[test]
    fn well_behaved_tools_round_trip() {
        let dir = scratch("ok");
        let out = dir.join("got");
        let mut c = Command::new("sh");
        c.args(["-c", &format!("cat > '{}'", out.display())]);
        pipe_into_within(c, "清风".as_bytes(), TIMEOUT).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "清风");
        let mut c = Command::new("cat");
        c.arg(&out);
        assert_eq!(capture_within(c, TIMEOUT).unwrap(), "清风".as_bytes());
        let mut c = Command::new("sh");
        c.args(["-c", "exit 3"]);
        assert!(capture_within(c, TIMEOUT).is_err(), "退出码非零按失败");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
