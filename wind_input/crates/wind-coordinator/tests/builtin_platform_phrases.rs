//! 出厂短语的**按平台取舍**与 Linux 上 `proc.any` 词条的端到端守门。
//!
//! `system.phrases.toml` 里打开记事本 / 计算器 / 主目录这几条按平台各写一份
//! （`platform = 'windows' / 'darwin' / 'linux'`）。过滤曾经写死只收 `windows`，于是
//! Linux 与 macOS 上出的全是 `notepad.exe` / `calc.exe`——选中必然失败，而 darwin 那几条
//! 从未生效过。
//!
//! ⚠️ 短语读的是**仓库** `data/system.phrases.toml`，不是 `build_dev/data/` 那份部署产物
//! （理由同 `builtin_reverse_phrase.rs`）；端到端那条另需 `build_dev/data` 的五笔方案，
//! 缺失时跳过（判据是耗时 0.00s）。

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn repo_phrases() -> PathBuf {
    repo_root().join("data/system.phrases.toml")
}

fn entries() -> Vec<wind_phrase::SystemPhraseEntry> {
    let p = repo_phrases();
    assert!(p.is_file(), "找不到出厂短语文件：{}", p.display());
    let v = wind_phrase::PhraseLayer::parse_system_entries(&p);
    assert!(
        !v.is_empty(),
        "出厂短语解析出 0 条——解析失败会让下面的断言全部空过"
    );
    v
}

fn texts_of<'a>(v: &'a [wind_phrase::SystemPhraseEntry], code: &str) -> Vec<&'a str> {
    v.iter()
        .filter(|e| e.code == code)
        .map(|e| e.text.as_str())
        .collect()
}

/// 本平台只看得到本平台那一份：非 Windows 上不得出现任何 `.exe` 词条，也不得出现
/// Windows 专属的 `USERPROFILE`。
#[test]
fn builtin_phrases_carry_no_windows_programs_off_windows() {
    let v = entries();
    let bad: Vec<_> = v
        .iter()
        .filter(|e| e.text.contains(".exe") || e.text.contains("USERPROFILE"))
        .map(|e| format!("{} = {}", e.code, e.text))
        .collect();
    if cfg!(windows) {
        assert!(!bad.is_empty(), "Windows 上应仍有 notepad.exe 等词条");
    } else {
        assert!(bad.is_empty(), "本平台出现了 Windows 专属词条：{bad:?}");
    }
}

/// 打开编辑器 / 计算器 / 主目录这三个码在每个桌面平台上都恰好一条，且是本平台的写法。
#[test]
fn launcher_codes_resolve_to_exactly_one_entry_for_this_platform() {
    let v = entries();
    for code in ["cono", "coca", "cohm"] {
        let t = texts_of(&v, code);
        assert_eq!(t.len(), 1, "{code} 在本平台应恰好一条，实际 {t:?}");
        let t = t[0];
        if cfg!(windows) {
            assert!(
                t.contains(".exe") || t.contains("USERPROFILE"),
                "{code}: {t}"
            );
        } else if cfg!(target_os = "macos") {
            assert!(t.contains("\"-a\"") || t.contains("HOME"), "{code}: {t}");
        } else if cfg!(target_os = "linux") {
            assert!(t.contains("proc.any(") || t.contains("HOME"), "{code}: {t}");
        }
    }
}

/// Linux 词条能被命令直通车解析，且 `proc.any` 的每个候选都是裸程序名（不带路径、参数）。
#[cfg(target_os = "linux")]
#[test]
fn linux_launcher_phrases_parse_and_list_bare_program_names() {
    let v = entries();
    for code in ["cono", "coca"] {
        let t = texts_of(&v, code)[0];
        let reg = wind_cmdbar::Registry::full();
        let ctx = wind_cmdbar::MemoryContext::new();
        let ev = wind_cmdbar::evaluate_phrase(t, &ctx, &reg)
            .unwrap_or_else(|e| panic!("{code} 解析失败：{e}"));
        assert!(!ev.primary_display().is_empty(), "{code} 应有显示文本");
        let inner = t
            .split("proc.any(")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .unwrap_or_else(|| panic!("{code} 应使用 proc.any：{t}"));
        let names: Vec<&str> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();
        assert!(names.len() >= 3, "{code} 候选太少：{names:?}");
        for n in &names {
            assert!(
                !n.is_empty() && !n.contains(['/', ' ', '.']),
                "{code} 的候选应是裸程序名：{n:?}"
            );
        }
    }
}

// ───────────────── 端到端：打 coca → 选中 → 真的 spawn 到排在后面的那个 ─────────────────
//
// 只在 Linux 的 `linux-host` 形态（`ext_presenter`）下成立：那里 `proc.run` 在服务进程里
// 直接 spawn，能拿到「程序不存在」；默认形态（mock）与 Windows 一样把启动转给宿主，
// 恒报成功，回落无从谈起。

#[cfg(all(target_os = "linux", ext_presenter))]
mod e2e {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use wind_bridge::handler::{KeyEventData, MessageHandler};
    use wind_config::Config;
    use wind_coordinator::Coordinator;
    use wind_ipc::protocol::EVENT_KEY_DOWN;

    const VK_SPACE: u32 = 0x20;

    fn build_data() -> PathBuf {
        repo_root().join("build_dev/data")
    }

    fn key(vk: u32) -> KeyEventData {
        KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: 0,
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        }
    }

    /// 以 build_dev/data 为底、短语换成仓库那份的数据目录（逐项符号链接，不拷词库）。
    fn data_with_repo_phrases(root: &Path) -> PathBuf {
        let d = root.join("data");
        std::fs::create_dir_all(&d).unwrap();
        for e in std::fs::read_dir(build_data()).unwrap().flatten() {
            if e.file_name() == "system.phrases.toml" {
                continue;
            }
            std::os::unix::fs::symlink(e.path(), d.join(e.file_name())).unwrap();
        }
        std::os::unix::fs::symlink(repo_phrases(), d.join("system.phrases.toml")).unwrap();
        d
    }

    /// 假程序：被启动时把自己的名字追加进 `log`。
    fn fake_program(bin: &Path, name: &str, log: &Path) {
        let p = bin.join(name);
        std::fs::write(
            &p,
            format!("#!/bin/sh\necho {name} >> '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// PATH 里只有排第三、第四的两个计算器：打 `coca` 选中后，应当越过前两个（未安装）
    /// 启动第三个，且**只**启动它。
    ///
    /// 变异判据：把 wind-phrase 的平台过滤改回只收 `windows`，候选变成 `calc.exe`、日志
    /// 为空转红；把 `proc.any` 的首个成功即停去掉，日志多出 mate-calc 转红。
    #[test]
    fn typing_coca_launches_the_first_installed_calculator() {
        if !build_data().join("schemas/wubi86.schema.toml").is_file() {
            eprintln!("跳过：缺 build_dev/data 的五笔方案");
            return;
        }
        let root = std::env::temp_dir().join(format!("wind_plat_phr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = root.join("launched.log");
        // 出厂顺序：gnome-calculator, kcalc, deepin-calculator, mate-calc, …
        fake_program(&bin, "deepin-calculator", &log);
        fake_program(&bin, "mate-calc", &log);
        // SAFETY: 本测试二进制里只有这一条用例读写 PATH（其余用例不起子进程），
        // 且在构造协调器、起任何线程之前设置。
        unsafe { std::env::set_var("PATH", &bin) };

        let data = data_with_repo_phrases(&root);
        let store = Arc::new(wind_store::Store::open(root.join("user.redb")).unwrap());
        let mut cfg = Config::default();
        cfg.schema.available = vec!["wubi86".into()];
        cfg.schema.active = "wubi86".into();
        cfg.input.default.chinese_mode = true;
        let coord = Coordinator::new_headless_with_store(cfg, Some(&data), store);

        for ch in "coca".chars() {
            coord.handle_key_event(&key(ch.to_ascii_uppercase() as u32));
        }
        let texts = coord.debug_all_candidate_texts();
        // 四码唯一时可能已自动上屏执行；没有的话首选应是这条命令，空格选中。
        if !texts.is_empty() {
            assert_eq!(
                texts.first().map(String::as_str),
                Some("打开计算器"),
                "coca 首选应为 Linux 版计算器命令，实际 {texts:?}"
            );
            coord.handle_key_event(&key(VK_SPACE));
        }

        assert!(
            wait_for(|| std::fs::read_to_string(&log).is_ok_and(|s| !s.is_empty())),
            "选中后应启动一个计算器（日志为空）"
        );
        // 给「多启动一个」留出与正例同量级的时间窗，否则只证明了「还没发生」。
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "deepin-calculator\n",
            "应越过未安装的前两个、只启动第一个装了的"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
