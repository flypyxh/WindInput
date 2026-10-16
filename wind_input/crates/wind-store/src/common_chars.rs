//! 常用字表的**用户增删**（候选右键「设为生僻字 / 设为常用字」+ 词库管理界面）。
//!
//! ## 只存用户碰过的字，不存整表
//!
//! ```text
//! 出厂 data/schemas/common_chars.txt（8104 字）
//!   ↓ 用户目录整份覆盖（Config::resolve_schema_resource，高级用户手写，可选）
//! 基表
//!   ↓ 本表的稀疏覆盖（普通用户点右键）
//! 最终常用字集
//! ```
//!
//! 与 [`crate::quick_format`] 同一条纪律：**GUI 调整绝不回写 `common_chars.txt`**。
//! 回写会抢走高级用户手写文件的所有权，更糟的是让普通用户点两下右键就永久脱离出厂
//! 更新，而他毫不知情——通用规范汉字表将来升版（改字、补字），整份覆盖过的用户拿不到。
//!
//! **稀疏还顺带解决了「出厂新增的字算常用还是生僻」**：没被碰过的字压根不在本表里，
//! 它的判定完全由基表决定，出厂表升版时自动跟随。若改存完整集合，就得再定一条
//! 「新出现的字默认算哪边」的规则——怎么定都会让人意外。
//!
//! ## 键不带方案：这是全局字级属性
//!
//! | | 作用域 | 键 |
//! |---|---|---|
//! | shadow（候选调整） | 一次输入：这个方案、这个码 | `(方案, 输入码)` |
//! | 本模块 | **全局**：这个字在所有方案、所有码下 | `字` |
//!
//! 「某个字常不常用」是语言学属性，与五笔/拼音无关（2026-08-24 与用户确认）。
//! 右键菜单的文案要把这个差异说出来（「设为生僻字（全局）」对「隐藏此候选」），
//! 否则用户会两个都试一遍，再困惑于为什么表现不一样。

use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一条用户覆盖。
///
/// `common` 两个方向都要存，不能只存「被踢出常用的字」：用户既会把常用字降级
/// （某个字他从不用、老是挡路），也会把生僻字升级（专业用字、人名用字）。
/// 只存一个方向的话另一个方向就没有落点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommonCharOverride {
    /// 被覆盖的**字素簇**（用户眼里的一个字符）。
    ///
    /// 类型是 `String` 而不是 `char`：`👨‍👩‍👧` 是 5 个码位、`🇨🇳` 是 2 个，屏幕上都只有一个
    /// 图形。字段名保持 `ch` 是为了**向后兼容**——备份包里已有的 `{"ch":"槮",...}` 照旧
    /// 解析得出来（JSON 字符串 → String 恒成立），换名字才会让老备份读不回来。
    pub ch: String,
    /// `true` = 用户强制判为常用；`false` = 用户强制判为生僻。
    pub common: bool,
}

/// 表 value 的序列化形态。
///
/// 只有一个字段却仍用结构体而非裸 bool：将来要加「来源」（右键点的 / 导入的）或
/// 「时间」时，裸 `true` 无处扩展，而多一层对象是向后兼容的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// 旧 redb 表的 value 形态（P4a 迁移解码 / 测试造旧库用）。
pub(crate) struct CommonCharRecord {
    pub(crate) common: bool,
}

/// 文件形态：`common: [字…]`、`uncommon: [字…]`（P4a 起真源 `<text_dir>/common_chars.yaml`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CommonFile {
    #[serde(default)]
    common: Vec<String>,
    #[serde(default)]
    uncommon: Vec<String>,
}

const COMMON_HEADER: &str =
    "# 常用字覆盖（WindInput 维护，可手改）：common = 强制常用，uncommon = 强制生僻\n";

fn parse_common(s: &str) -> anyhow::Result<BTreeMap<String, bool>> {
    if s.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let f: CommonFile =
        serde_yaml::from_str(s).map_err(|e| anyhow::anyhow!("常用字覆盖文件解析失败：{e}"))?;
    let mut m = BTreeMap::new();
    for c in f.uncommon {
        if !c.is_empty() {
            m.insert(c, false);
        }
    }
    for c in f.common {
        if !c.is_empty() {
            m.insert(c, true);
        }
    }
    Ok(m)
}

fn render_common(m: &BTreeMap<String, bool>) -> String {
    let f = CommonFile {
        common: m
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect(),
        uncommon: m
            .iter()
            .filter(|(_, v)| !**v)
            .map(|(k, _)| k.clone())
            .collect(),
    };
    format!(
        "{COMMON_HEADER}{}",
        serde_yaml::to_string(&f).unwrap_or_default()
    )
}

impl Store {
    pub fn common_chars_path(&self) -> std::path::PathBuf {
        self.user_text_dir().join("common_chars.yaml")
    }

    pub(crate) fn common_doc(&self) -> &wind_userdata::text::doc::TextDoc {
        self.common_doc
            .get_or_init(|| wind_userdata::text::doc::TextDoc::open(self.common_chars_path()))
    }

    /// 当前有效的覆盖：文件手改坏了按最后一次好数据（§2.5）。
    fn common_map(&self) -> BTreeMap<String, bool> {
        self.common_good.get(self.common_doc(), &parse_common)
    }

    /// 读改写全部覆盖，一次提交；全空则删文件。文件解析不了 ⇒ Err、什么都不写（M-1）。
    fn modify_common(&self, f: &mut dyn FnMut(&mut BTreeMap<String, bool>)) -> anyhow::Result<()> {
        let doc = self.common_doc();
        doc.modify(&mut |cur| {
            let mut m = cur
                .map(parse_common)
                .transpose()
                .map_err(|e| crate::last_good::broken_file_error(doc, e))?
                .unwrap_or_default();
            f(&mut m);
            Ok((!m.is_empty()).then(|| render_common(&m)))
        })?;
        // 刚写成功的内容就是最新的好数据：之后文件被改坏也能按它服务
        self.common_map();
        Ok(())
    }

    /// 写一条覆盖（同字重复写即改写方向）。
    pub fn set_common_char_override(&self, key: &str, common: bool) -> anyhow::Result<()> {
        self.modify_common(&mut |m| {
            m.insert(key.to_string(), common);
        })
    }

    /// **一次提交**写一批覆盖：`Some(common)` 写入、`None` 删除。返回真正写入的条数。
    ///
    /// 整类批量（按 Unicode 块设常用/生僻）用它：调用方每写一条还要回灌一次运行时镜像，
    /// 那把锁正是候选过滤每次按键都要拿的，逐条写会把它拿上几百次。
    pub fn apply_common_char_overrides(
        &self,
        items: &[(String, Option<bool>)],
    ) -> anyhow::Result<usize> {
        let mut written = 0usize;
        self.modify_common(&mut |m| {
            written = 0;
            for (key, dir) in items {
                match dir {
                    Some(common) => {
                        m.insert(key.clone(), *common);
                        written += 1;
                    }
                    None => {
                        m.remove(key);
                    }
                }
            }
        })?;
        Ok(written)
    }

    /// 撤销某字的覆盖，回到出厂判定。`false` = 本就没有覆盖。
    ///
    /// 与「设为常用字」**不是**一回事：出厂判生僻的字，撤销覆盖后仍是生僻。
    pub fn remove_common_char_override(&self, key: &str) -> anyhow::Result<bool> {
        let mut existed = false;
        self.modify_common(&mut |m| existed = m.remove(key).is_some())?;
        Ok(existed)
    }

    /// 某字是否有覆盖，以及方向。`None` = 未覆盖（由基表判定）。
    pub fn get_common_char_override(&self, key: &str) -> anyhow::Result<Option<bool>> {
        Ok(self.common_map().get(key).copied())
    }

    /// 全部覆盖，按字的 UTF-8 字节序升序（对单字符即码位序）。
    pub fn list_common_char_overrides(&self) -> anyhow::Result<Vec<CommonCharOverride>> {
        Ok(self
            .common_map()
            .into_iter()
            .map(|(ch, common)| CommonCharOverride { ch, common })
            .collect())
    }

    /// 清空全部覆盖（词库管理界面的「全部恢复出厂」）。返回清掉的条数。
    pub fn clear_common_char_overrides(&self) -> anyhow::Result<usize> {
        let n = self.common_map().len();
        crate::last_good::preserve_if_unparsable(self.common_doc(), &parse_common)?;
        self.common_doc().replace(None)?;
        Ok(n)
    }

    /// 导出为 JSONL（备份 / 迁移）。每行 `{"ch":"槮","common":true}`。
    pub fn export_common_chars_jsonl(&self) -> anyhow::Result<String> {
        let mut s = String::new();
        for o in self.list_common_char_overrides()? {
            s.push_str(&serde_json::to_string(&o)?);
            s.push('\n');
        }
        Ok(s)
    }

    /// 从 JSONL 导入，返回写入条数。坏行跳过（与 [`Self::list_common_char_overrides`]
    /// 同一条纪律：一行坏数据不该让整次还原失败）。
    pub fn import_common_chars_jsonl(&self, text: &str) -> anyhow::Result<usize> {
        let normalized = wind_utils::text::normalize_input(text);
        let text = normalized.as_ref();
        let mut n = 0usize;
        let mut items = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(o) = serde_json::from_str::<CommonCharOverride>(line) else {
                continue;
            };
            items.push((o.ch, Some(o.common)));
            n += 1;
        }
        self.apply_common_char_overrides(&items)?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 审查 M-2：还原前的「清空」遇到改坏的文件 ⇒ 另存留存，不直接删掉。
    #[test]
    fn clearing_broken_file_keeps_it_aside() {
        let s = store("clear_broken");
        s.set_common_char_override("龘", true).unwrap();
        let f = s.common_chars_path();
        std::fs::write(&f, "common: [\n").unwrap();
        s.common_doc().sync();
        s.clear_common_char_overrides().unwrap();
        let kept: Vec<_> = std::fs::read_dir(f.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("common_chars.yaml.corrupt-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            std::fs::read_to_string(kept[0].path()).unwrap(),
            "common: [\n"
        );
        s.set_common_char_override("靐", false).unwrap();
        wind_userdata::issues::clear(&f);
    }

    /// M-1：手改坏了按最后一次好数据服务、拒写、不重写文件。
    #[test]
    fn common_chars_broken_edit_keeps_serving_and_is_not_rewritten() {
        let s = store("broken");
        s.set_common_char_override("龘", true).unwrap();
        let f = s.common_chars_path();
        std::fs::write(&f, "common: [\n").unwrap();
        s.common_doc().sync();
        assert_eq!(s.get_common_char_override("龘").unwrap(), Some(true));
        assert!(s.set_common_char_override("靐", false).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "common: [\n");
        wind_userdata::issues::clear(&f);
    }

    /// 每个测试独立文件：redb 是单写者，共用文件会让并发测试互相阻塞。
    /// `tag` 区分同一测试里需要的第二个库（导入导出对拷）。
    fn store(tag: &str) -> Store {
        let p = std::env::temp_dir().join(format!(
            "wind_common_chars_test_{}_{}_{:?}.redb",
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
        assert_eq!(s.get_common_char_override("槮").unwrap(), None);

        s.set_common_char_override("槮", true).unwrap();
        assert_eq!(s.get_common_char_override("槮").unwrap(), Some(true));

        // 同字改写方向，不是追加第二条。
        s.set_common_char_override("槮", false).unwrap();
        assert_eq!(s.get_common_char_override("槮").unwrap(), Some(false));
        assert_eq!(s.list_common_char_overrides().unwrap().len(), 1);

        assert!(s.remove_common_char_override("槮").unwrap());
        assert_eq!(s.get_common_char_override("槮").unwrap(), None);
        // 再删一次：没有这条，返回 false 而非报错。
        assert!(!s.remove_common_char_override("槮").unwrap());
    }

    #[test]
    fn both_directions_are_storable() {
        let s = store("b");
        // 生僻字升级为常用
        s.set_common_char_override("槮", true).unwrap();
        // 常用字降级为生僻
        s.set_common_char_override("的", false).unwrap();
        let all = s.list_common_char_overrides().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&CommonCharOverride {
            ch: "槮".into(),
            common: true
        }));
        assert!(all.contains(&CommonCharOverride {
            ch: "的".into(),
            common: false
        }));
    }

    #[test]
    fn clear_removes_all() {
        let s = store("c");
        s.set_common_char_override("槮", true).unwrap();
        s.set_common_char_override("的", false).unwrap();
        assert_eq!(s.clear_common_char_overrides().unwrap(), 2);
        assert!(s.list_common_char_overrides().unwrap().is_empty());
    }

    #[test]
    fn jsonl_roundtrip() {
        let s = store("d");
        s.set_common_char_override("槮", true).unwrap();
        s.set_common_char_override("的", false).unwrap();
        let text = s.export_common_chars_jsonl().unwrap();
        assert_eq!(text.lines().count(), 2);

        let s2 = store("d2");
        assert_eq!(s2.import_common_chars_jsonl(&text).unwrap(), 2);
        assert_eq!(s2.get_common_char_override("槮").unwrap(), Some(true));
        assert_eq!(s2.get_common_char_override("的").unwrap(), Some(false));
    }

    #[test]
    fn import_skips_bad_lines() {
        let s = store("e");
        let text =
            "{\"ch\":\"槮\",\"common\":true}\n不是 json\n\n{\"ch\":\"的\",\"common\":false}\n";
        assert_eq!(s.import_common_chars_jsonl(text).unwrap(), 2);
        assert_eq!(s.list_common_char_overrides().unwrap().len(), 2);
    }

    /// 常用字覆盖落用户目录文件。
    #[test]
    fn common_chars_live_in_user_dir() {
        let s = store("file");
        s.set_common_char_override("槮", true).unwrap();
        s.set_common_char_override("的", false).unwrap();
        let f = std::fs::read_to_string(s.common_chars_path()).unwrap();
        assert!(
            f.contains("common:") && f.contains("槮") && f.contains("uncommon:"),
            "{f}"
        );
        assert_eq!(s.get_common_char_override("的").unwrap(), Some(false));
    }

    /// 编辑器正在改文件（复核前被改）⇒ 程序这次写入报错、不覆盖；sync 后再写成功。
    #[test]
    fn override_write_refuses_when_file_changed_underneath() {
        let s = store("race");
        s.set_common_char_override("槮", true).unwrap();
        let f = s.common_chars_path();
        let f2 = f.clone();
        let me = std::thread::current().id();
        let hook = wind_userdata::text::fold::add_before_rename_hook(Box::new(move || {
            if std::thread::current().id() == me {
                std::fs::write(&f2, "common: [甲]\nuncommon: []\n").unwrap();
            }
        }));
        let r = s.set_common_char_override("乙", true);
        drop(hook);
        assert!(r.is_err());
        assert!(std::fs::read_to_string(&f).unwrap().contains("甲"));
        s.sync_user_texts();
        s.set_common_char_override("乙", true).unwrap();
        let f = std::fs::read_to_string(&f).unwrap();
        assert!(f.contains("甲") && f.contains("乙"), "{f}");
    }
}
