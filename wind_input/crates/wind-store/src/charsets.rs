//! 字符类的**用户层**（设置页「字符集分类」+ 外部编辑回读 + 备份还原）。
//!
//! ## 文件是真相源（P4a 起，设计 §2.4）
//!
//! ```text
//! 出厂 data/charsets/*.yaml（只读，随安装包）
//!   ↓ 定制 data_custom/charsets/*.yaml（只读）
//!   ↓ 用户层 <userdata>/user/charsets/<key>.yaml（自建类、对出厂类的字段覆盖与成员增删）
//! registry
//! ```
//!
//! 曾经放进 redb，唯一的理由是「程序和人同写一个文件，后保存者赢、另一方静默丢失」。现在程序
//! 的写入是显式的整份替换（设置页保存、编辑副本回读，「这份就是新内容」），手改文件经
//! `sync_charsets` 在配置重载 / `dict.reload` 时生效；编辑副本 → 回读的流程照旧。仍存在的一种
//! 冲突：设置页保存与编辑器同时保存同一个类，后写者胜（设置页的写入是显式整份替换，不复核）。
//!
//! ## 内容就是 yaml 文本本身
//!
//! 本 crate **不认识** `CharsetDoc`（`wind-config` 依赖方向反着），也不该认识：格式
//! 只在 `wind_config::charset_def` 里一份，解析与写出都在那边。这里存的是那份文本的
//! 字节，备份时原样进包、编辑时原样导出，一个格式三处用，不会有「库里的形态」与
//! 「文件里的形态」两套东西要对齐。
//!
//! 键是类的 `key`（也是 `exclude_blocks` / `include_blocks` 里写的那个名字），不带方案
//! ——字符类与 [`crate::common_chars`] 同为全局属性，理由见那边的模块文档。

use crate::store::Store;

impl Store {
    /// 字符集用户层目录：`<text_dir>/charsets/`（P4a 起真源）。
    pub fn charsets_dir(&self) -> std::path::PathBuf {
        self.user_text_dir().join("charsets")
    }

    /// 某类的用户层文件。`key` 是一个路径分量（越界 / 非法 ⇒ Err）。
    pub fn charset_path(&self, key: &str) -> anyhow::Result<std::path::PathBuf> {
        wind_userdata::engine::check_schema_id(key)
            .map_err(|_| anyhow::anyhow!("非法字符类 key：{key:?}"))?;
        Ok(self.charsets_dir().join(format!("{key}.yaml")))
    }

    /// 读一个类的用户层文本；`None` = 用户没动过这个类。
    pub fn get_charset_doc(&self, key: &str) -> anyhow::Result<Option<String>> {
        let p = self.charset_path(key)?;
        Ok(wind_userdata::text::fold::read_text(&p)?
            .map(|(b, _)| String::from_utf8_lossy(&b).into_owned()))
    }

    /// 整份写入（同 key 覆盖；显式回读 / 设置页保存都是「这份就是新内容」，不复核指纹）。
    /// 文本合法性由调用方负责——本层不解析。
    pub fn set_charset_doc(&self, key: &str, text: &str) -> anyhow::Result<()> {
        let p = self.charset_path(key)?;
        wind_userdata::text::fold::commit_text(&p, text.as_bytes(), None)?;
        self.remember_charsets_fp();
        Ok(())
    }

    /// 删掉一个类的用户层（自建类 = 整个类消失；出厂类 = 回到出厂）。`false` = 本就没有。
    pub fn remove_charset_doc(&self, key: &str) -> anyhow::Result<bool> {
        let p = self.charset_path(key)?;
        let existed = match std::fs::remove_file(&p) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        self.remember_charsets_fp();
        Ok(existed)
    }

    /// 全部用户层，按 key 升序。量级是几十条，全量返回。名字不合法的文件跳过（WARN）。
    pub fn list_charset_docs(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.charsets_dir()) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("yaml") {
                continue;
            }
            let Some(key) = path.file_stem().and_then(|x| x.to_str()) else {
                continue;
            };
            if wind_userdata::engine::check_schema_id(key).is_err() {
                tracing::warn!("字符集用户层：跳过文件名不合法的条目");
                continue;
            }
            if let Some(text) = self.get_charset_doc(key)? {
                out.push((key.to_string(), text));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// 清空用户层（备份「替换」模式用）。返回删掉的条数。
    pub fn clear_charset_docs(&self) -> anyhow::Result<usize> {
        let docs = self.list_charset_docs()?;
        for (k, _) in &docs {
            self.remove_charset_doc(k)?;
        }
        Ok(docs.len())
    }

    /// 用户层目录的整体指纹（各文件名与内容指纹的有序拼接）。
    fn charsets_fp(&self) -> String {
        let mut parts: Vec<String> = self
            .list_charset_docs()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, t)| {
                format!(
                    "{k}={}",
                    wind_userdata::text::fold::fingerprint(Some(t.as_bytes()))
                )
            })
            .collect();
        parts.sort();
        parts.join(";")
    }

    pub(crate) fn remember_charsets_fp(&self) {
        let fp = self.charsets_fp();
        *self.charsets_fp.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
    }

    /// 手改的字符集用户层文件重新载入：目录内容变了返回 true（调用方据此重建字符集注册表）。
    pub fn sync_charsets(&self) -> bool {
        let fp = self.charsets_fp();
        let mut g = self.charsets_fp.lock().unwrap_or_else(|e| e.into_inner());
        let changed = g.as_deref().is_some_and(|old| old != fp);
        *g = Some(fp);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独立文件：redb 是单写者，共用文件会让并发测试互相阻塞。
    fn store(tag: &str) -> Store {
        let p = std::env::temp_dir().join(format!(
            "wind_charsets_test_{}_{}_{:?}.redb",
            std::process::id(),
            tag,
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&p);
        Store::open(&p).unwrap()
    }

    #[test]
    fn set_get_remove_roundtrip() {
        let s = store("a");
        assert_eq!(s.get_charset_doc("emoji").unwrap(), None);

        s.set_charset_doc("emoji", "key: emoji\ndefault: rare\n")
            .unwrap();
        assert_eq!(
            s.get_charset_doc("emoji").unwrap().as_deref(),
            Some("key: emoji\ndefault: rare\n")
        );

        // 同 key 覆盖，不是追加。
        s.set_charset_doc("emoji", "key: emoji\n").unwrap();
        assert_eq!(
            s.get_charset_doc("emoji").unwrap().as_deref(),
            Some("key: emoji\n")
        );

        assert!(s.remove_charset_doc("emoji").unwrap());
        assert!(
            !s.remove_charset_doc("emoji").unwrap(),
            "二次删除返回 false"
        );
        assert_eq!(s.get_charset_doc("emoji").unwrap(), None);
    }

    /// 文本原样进出——多行、中文、`...` 分隔符、行首 `-`，一个字节都不能动。
    /// 备份和编辑态都靠它：这里若做任何规范化，回读时 diff 就对不上出厂。
    #[test]
    fn text_is_stored_verbatim() {
        let s = store("verbatim");
        let text = "---\nkey: 我的符号\nname: 我的符号\nranges: [U+2600-U+26FF]\n...\n★\n-☯\n\n";
        s.set_charset_doc("我的符号", text).unwrap();
        assert_eq!(
            s.get_charset_doc("我的符号").unwrap().as_deref(),
            Some(text)
        );
    }

    #[test]
    fn list_is_sorted_by_key_and_clear_empties_it() {
        let s = store("list");
        s.set_charset_doc("zeta", "key: zeta\n").unwrap();
        s.set_charset_doc("alpha", "key: alpha\n").unwrap();
        s.set_charset_doc("emoji", "key: emoji\n").unwrap();

        let keys: Vec<String> = s
            .list_charset_docs()
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["alpha", "emoji", "zeta"]);

        assert_eq!(s.clear_charset_docs().unwrap(), 3);
        assert!(s.list_charset_docs().unwrap().is_empty());
        assert_eq!(s.clear_charset_docs().unwrap(), 0, "清空空表不报错");
    }

    /// 字符集用户层落用户目录文件；key 不能越出目录。
    #[test]
    fn charset_docs_live_in_user_dir() {
        let s = store("file");
        s.set_charset_doc("rare", "key: rare\nname: 生僻\n")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(s.charset_path("rare").unwrap()).unwrap(),
            "key: rare\nname: 生僻\n"
        );
        assert_eq!(s.list_charset_docs().unwrap().len(), 1);
        assert!(s.remove_charset_doc("rare").unwrap());
        assert!(!s.charset_path("rare").unwrap().exists());
        assert!(s.set_charset_doc("../x", "k").is_err());
    }

    /// 审查 I-3：启动后第一次手改就要被 sync 发现。
    #[test]
    fn first_hand_edit_after_open_is_detected() {
        let p = std::env::temp_dir().join(format!("wind_cs_first_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let s = Store::open(&p).unwrap();
            s.set_charset_doc("rare", "key: rare\n").unwrap();
        }
        let s = Store::open(&p).unwrap();
        std::fs::write(s.charset_path("rare").unwrap(), "key: rare\nname: 改\n").unwrap();
        assert!(s.sync_charsets(), "第一次手改就要报变化");
        assert!(!s.sync_charsets());
    }
}
