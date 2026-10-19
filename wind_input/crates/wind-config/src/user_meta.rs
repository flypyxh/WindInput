//! `<userdata>/meta.toml`：用户目录内数据格式的版本与目录 id（设计 §2.4、§7.1）。
//!
//! **降级保护必须先于任何新格式发布**：新版本写出新格式后用户回退，旧版本若照常在
//! 旁边继续写 redb，两边就分叉。读到比自己认识的更高的版本号 ⇒ 只读模式，一个字节都不写。

use std::path::Path;

/// 本版本认识的格式版本。1 = 纯 redb；2 = P1：临时词 / 草稿 / 词频 / 联想历史在运行时引擎
/// （`local_dir()/state/`）；3 = P2：用户词在 `user/<schema>/user.dict.yaml`；4 = P3：短语与 Shadow
/// 在 `user/phrases.dict.yaml` / `user/<schema>/shadow.dict.yaml`；5 = P4a：快捷格式 / 常用字 /
/// 字符集用户层；5 起用户目录装下全部数据（`data/` 下含学到的数据、补全、统计，P5 定稿的布局），
/// 旧库改名 `userdata.redb.legacy`。2–4 只在开发分支上出现过。每个 P 阶段迁移完成 +1。
pub const KNOWN_FORMAT_VERSION: u32 = 5;

/// 说明行（设计 `user-dict-libraries-and-external-edit.md` §5；`data/` 下文件的文案在
/// `wind_userdata::text::banner`）。
const META_BANNER: &str = "# 清风输入法 · 用户数据的格式版本。由程序自动维护，请勿手动修改。";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserMeta {
    pub format_version: u32,
    /// 随数据目录生成的随机 id（32 位十六进制）。随目录漫游 / 复制；与本机 `device.toml`
    /// 里记的 `dir_id` 对不上就说明目录被复制或换了目录（设计 §3.7）。
    pub dir_id: String,
}

#[derive(Debug)]
pub enum MetaCheck {
    Ok(UserMeta),
    /// 数据由更新版本写入：只读模式，`found` 给提示用。
    Incompatible {
        found: u32,
        known: u32,
    },
    /// 文件在但读不出 / 解析不出：不覆盖（那可能是更新版本的格式），按只读处理。
    Unreadable(String),
}

pub fn load_or_init(user_dir: &Path) -> MetaCheck {
    let path = user_dir.join("meta.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<UserMeta>(&text) {
            Ok(m) if m.format_version > KNOWN_FORMAT_VERSION => MetaCheck::Incompatible {
                found: m.format_version,
                known: KNOWN_FORMAT_VERSION,
            },
            Ok(m) => MetaCheck::Ok(m),
            Err(e) => MetaCheck::Unreadable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 旁边已有旧版 redb：记 1，等迁移完成由协调器升级；否则（全新目录，或旧库已改名
            // `.legacy`、meta.toml 被删）直接记最新——没有 redb 可迁。
            let fresh = !user_dir.join("userdata.redb").exists();
            let m = UserMeta {
                format_version: if fresh { KNOWN_FORMAT_VERSION } else { 1 },
                dir_id: uuid::Uuid::new_v4().simple().to_string(),
            };
            let _ = std::fs::create_dir_all(user_dir);
            match toml::to_string(&m) {
                Ok(s) => {
                    if write_meta(user_dir, &s).is_ok() {
                        MetaCheck::Ok(m)
                    } else {
                        // 写不进用户目录：不阻断启动，但没有稳定的 dir_id，按只读对待更稳
                        MetaCheck::Unreadable("meta.toml 写入失败".into())
                    }
                }
                Err(e) => MetaCheck::Unreadable(e.to_string()),
            }
        }
        Err(e) => MetaCheck::Unreadable(e.to_string()),
    }
}
/// 只读取 `meta.toml`（不存在 / 读不出 ⇒ None），不新建。
pub fn read(user_dir: &Path) -> Option<UserMeta> {
    toml::from_str(&std::fs::read_to_string(user_dir.join("meta.toml")).ok()?).ok()
}

/// 迁移成功后把 `format_version` 升到 `to`（现值 ≥ `to` 不动，`dir_id` 保留）。旧版本读到更高
/// 的版本即进只读（§7.1），不会在新格式旁边继续写 redb。
pub fn raise_format_version(user_dir: &Path, to: u32) -> std::io::Result<()> {
    let path = user_dir.join("meta.toml");
    let text = std::fs::read_to_string(&path)?;
    let mut m: UserMeta = toml::from_str(&text).map_err(std::io::Error::other)?;
    if m.format_version >= to {
        return Ok(());
    }
    m.format_version = to;
    let s = toml::to_string(&m).map_err(std::io::Error::other)?;
    write_meta(user_dir, &s)
}

/// tmp → fsync → rename → fsync 目录：断电后 meta.toml 不会是空文件（读不出即只读模式），
/// 随后删迁移标记、旧库改名的前提「版本已写成」才站得住。
fn write_meta(user_dir: &Path, s: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = user_dir.join("meta.toml.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(format!("{META_BANNER}\n{s}").as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, user_dir.join("meta.toml"))?;
    #[cfg(unix)]
    std::fs::File::open(user_dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_meta_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 旁边已有旧版 `userdata.redb`：初次生成的 meta 记 1，等迁移完成再升（否则迁移还没跑就先
    /// 写成最新版本号，迁移失败后回退的旧版本被误判成只读）。
    #[test]
    fn fresh_meta_beside_legacy_redb_starts_at_1() {
        let d = dir("legacy_redb");
        std::fs::write(d.join("userdata.redb"), b"x").unwrap();
        match load_or_init(&d) {
            MetaCheck::Ok(m) => assert_eq!(m.format_version, 1),
            other => panic!("{other:?}"),
        }
    }

    /// 旧库已改名 `.legacy` 后 meta.toml 被删：没有 redb 可迁，记最新（记 1 会新建空 redb）。
    #[test]
    fn retired_legacy_redb_alone_is_not_a_migration_source() {
        let d = dir("legacy_retired");
        std::fs::write(d.join("userdata.redb.legacy"), b"x").unwrap();
        match load_or_init(&d) {
            MetaCheck::Ok(m) => assert_eq!(m.format_version, KNOWN_FORMAT_VERSION),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_meta_is_initialised_with_known_version_and_random_dir_id() {
        let d = dir("init");
        let MetaCheck::Ok(m) = load_or_init(&d) else {
            panic!("应初始化")
        };
        assert_eq!(m.format_version, KNOWN_FORMAT_VERSION);
        assert_eq!(m.dir_id.len(), 32);
        let MetaCheck::Ok(again) = load_or_init(&d) else {
            panic!()
        };
        assert_eq!(again.dir_id, m.dir_id, "第二次读到同一个 dir_id");
    }

    /// 第一行是说明行，读照常；升版本整写时说明行还在。
    #[test]
    fn meta_file_starts_with_banner() {
        let d = dir("banner");
        assert!(matches!(load_or_init(&d), MetaCheck::Ok(_)));
        let first = |d: &Path| {
            std::fs::read_to_string(d.join("meta.toml"))
                .unwrap()
                .lines()
                .next()
                .map(str::to_string)
        };
        assert_eq!(first(&d).as_deref(), Some(META_BANNER));
        assert!(read(&d).is_some());
        std::fs::write(
            d.join("meta.toml"),
            "format_version = 1\ndir_id = \"abc\"\n",
        )
        .unwrap();
        raise_format_version(&d, 2).unwrap();
        assert_eq!(first(&d).as_deref(), Some(META_BANNER));
        assert!(
            META_BANNER.starts_with("# 清风输入法 · ")
                && META_BANNER.ends_with("。由程序自动维护，请勿手动修改。")
        );
    }

    #[test]
    fn newer_version_is_incompatible_and_file_is_left_untouched() {
        let d = dir("newer");
        let text = "format_version = 99\ndir_id = \"abc\"\n";
        std::fs::write(d.join("meta.toml"), text).unwrap();
        assert!(matches!(
            load_or_init(&d),
            MetaCheck::Incompatible {
                found: 99,
                known: KNOWN_FORMAT_VERSION
            }
        ));
        assert_eq!(
            std::fs::read_to_string(d.join("meta.toml")).unwrap(),
            text,
            "只读：一字不改"
        );
    }

    #[test]
    fn unparsable_meta_is_reported_not_overwritten() {
        let d = dir("bad");
        std::fs::write(d.join("meta.toml"), "not toml ==").unwrap();
        assert!(matches!(load_or_init(&d), MetaCheck::Unreadable(_)));
        assert_eq!(
            std::fs::read_to_string(d.join("meta.toml")).unwrap(),
            "not toml =="
        );
    }

    #[test]
    fn raise_format_version_only_moves_up_and_keeps_dir_id() {
        let d = dir("raise");
        std::fs::write(
            d.join("meta.toml"),
            "format_version = 1\ndir_id = \"abc\"\n",
        )
        .unwrap();
        raise_format_version(&d, 2).unwrap();
        let MetaCheck::Ok(m) = load_or_init(&d) else {
            panic!("应可读")
        };
        assert_eq!((m.format_version, m.dir_id.as_str()), (2, "abc"));
        raise_format_version(&d, 1).unwrap();
        let MetaCheck::Ok(m) = load_or_init(&d) else {
            panic!("应可读")
        };
        assert_eq!(m.format_version, 2, "不往回改");
    }
}
