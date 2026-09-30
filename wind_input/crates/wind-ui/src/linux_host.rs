//! Linux 外部宿主形态（`linux-host`）下的系统能力：剪贴板、打开路径。
//!
//! 与 macOS 的 `pbcopy` / `open` 同一路数——服务进程不链接任何桌面库，全部经外部命令：
//! Wayland 会话用 `wl-clipboard`，X11 会话用 `xclip`（其次 `xsel`），打开走 `xdg-open`。
//! 工具缺失时**如实报错**，不返回假成功（同 `screenshot::copy_bgra_to_clipboard` 的既有约定）。
//!
//! ⚠ 服务进程由 systemd 用户单元拉起时可能没有 `WAYLAND_DISPLAY` / `DISPLAY`，此时选不到
//! 后端；安装单元须 `import-environment`，或由 addon 拉起服务以继承会话环境。

use std::io::Write;
use std::process::{Command, Stdio};

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
    if env_set("WAYLAND_DISPLAY") && have("wl-copy") {
        return Some(Backend::WlClipboard);
    }
    if env_set("DISPLAY") {
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

fn pipe_into(mut cmd: Command, data: &[u8]) -> anyhow::Result<()> {
    let name = format!("{:?}", cmd.get_program());
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("{name} 启动失败: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(data)
            .map_err(|e| anyhow::anyhow!("{name} 写入失败: {e}"))?;
    }
    // 关掉 stdin 让对端读到 EOF；xclip/wl-copy 随后自行转入后台持有选区，wait 很快返回。
    drop(child.stdin.take());
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("{name} 退出码非零: {status}");
    }
    Ok(())
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
    let mut cmd = match backend {
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
    match cmd.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
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

/// 按默认应用打开路径 / URL（不等待）。
pub fn open(target: &str) -> std::io::Result<()> {
    Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_session_env_means_no_backend() {
        // 无 WAYLAND_DISPLAY / DISPLAY 时任何工具都不该被选中（避免在无头环境里误起进程）。
        // 只在环境确实干净时断言，否则（开发机在桌面会话里）不做判断。
        if !env_set("WAYLAND_DISPLAY") && !env_set("DISPLAY") {
            assert_eq!(pick_backend(), None);
            assert!(set_text("x").is_err());
            assert_eq!(get_text(), "");
        }
    }
}
