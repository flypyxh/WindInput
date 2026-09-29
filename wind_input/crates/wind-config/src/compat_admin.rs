//! 兼容规则的分层管理：视图 + 写时复制补丁 + 禁用 + 还原（设置端 `compat.*` RPC 的纯逻辑层）。
//!
//! 只写**用户层**；系统层（`data/compat.toml`、`data_custom`）永不写。三段
//! （`[[apps]]` / `[[initial_mode_scope]]` / `[[commit_newline]]`）共用一套泛型实现。
//! 合并语义复用 `app_compat` 里的 `merge_*`，这里不复制。
//!
//! 设计见 `docs/design/compat-settings-ui.md`。

use crate::app_compat::{
    AppCompatFile, AppCompatRule, COMPAT_FILE_NAME, CommitNewlineRule, InitialModeScopeRule,
    load_file, merge_commit_newline, merge_mode_scope, merge_rules, render_user_compat,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// 三段规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Apps,
    InitialModeScope,
    CommitNewline,
}

impl Section {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "apps" => Some(Self::Apps),
            "initial_mode_scope" => Some(Self::InitialModeScope),
            "commit_newline" => Some(Self::CommitNewline),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::InitialModeScope => "initial_mode_scope",
            Self::CommitNewline => "commit_newline",
        }
    }
}

/// 一条规则在分层视图里的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    /// 只来自系统层，未被动过（或用户层有一条与系统完全一致的冗余覆盖）。
    System,
    /// 系统规则被用户改写。
    Modified,
    /// 用户新增。
    User,
    /// 被用户禁用。
    Disabled,
}

/// 一条规则的视图。
#[derive(Debug, Clone, Serialize)]
pub struct RuleView {
    pub process: String,
    pub state: RuleState,
    /// 合并后的生效内容（对象，只含非默认字段；不含 `disabled`）。
    pub effective: Value,
    /// 系统层的原貌；用户新增的规则为 `None`。
    pub system: Option<Value>,
    /// 生效内容与系统层取值不同的字段名（用户新增的规则 = 它的全部字段）。
    pub overridden: Vec<String>,
}

/// 三段规则的共同面：泛型写操作只依赖这几个访问器和 serde。
pub(crate) trait Rule: Clone + Default + Serialize + serde::de::DeserializeOwned {
    fn process(&self) -> &str;
    fn set_process(&mut self, p: &str);
    fn disabled(&self) -> bool;
    fn set_disabled(&mut self, v: bool);
}

macro_rules! impl_rule {
    ($t:ty) => {
        impl Rule for $t {
            fn process(&self) -> &str {
                &self.process
            }
            fn set_process(&mut self, p: &str) {
                self.process = p.to_string();
            }
            fn disabled(&self) -> bool {
                self.disabled
            }
            fn set_disabled(&mut self, v: bool) {
                self.disabled = v;
            }
        }
    };
}
impl_rule!(AppCompatRule);
impl_rule!(InitialModeScopeRule);
impl_rule!(CommitNewlineRule);

/// 不属于「兼容取值」的键：不进 `overridden`，也不允许被补丁 / 还原触碰。
pub(crate) const META_KEYS: [&str; 3] = ["process", "comment", "disabled"];

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// 进程名校验：trim 后非空、不含路径分隔符与控制字符。返回 trim 后的名字。
pub fn validate_process(raw: &str) -> Result<String, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("进程名不能为空".into());
    }
    if p.contains('/') || p.contains('\\') {
        return Err("进程名不能含路径分隔符".into());
    }
    if p.chars().any(|c| c.is_control()) {
        return Err("进程名不能含控制字符".into());
    }
    Ok(p.to_string())
}

fn to_obj<T: Rule>(r: &T) -> Map<String, Value> {
    match serde_json::to_value(r) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// 序列化后除 `process` 外一个键都没有 ⇒ 空壳。
///
/// 判据走序列化而不是逐字段比较：可选字段全带 `skip_serializing_if`，新增字段自动纳入。
fn is_empty<T: Rule>(r: &T) -> bool {
    to_obj(r).keys().all(|k| k == "process")
}

/// 两条规则的「生效内容」是否相同（忽略 `comment`：它只是文档）。
fn same_content<T: Rule>(a: &T, b: &T) -> bool {
    let strip = |r: &T| {
        let mut m = to_obj(r);
        m.remove("comment");
        m.remove("process");
        m
    };
    strip(a) == strip(b)
}

/// 把补丁应用到 `base`，并确认每个非 null 的补丁键在往返之后仍然是它。
///
/// 容错反序列化会把错值悄悄吞成默认（`first_show_mode = "bogus"` → 「跟随全局」），
/// 而管理界面的写入必须明确拒绝，否则用户以为改成功了。
/// `bool false` / `0` / 空串 / 空数组在序列化时被省略，属合法值，不算无效。
fn apply_patch_to<T: Rule>(base: &T, patch: &Map<String, Value>) -> Result<T, String> {
    if patch.contains_key("process") {
        return Err("不能通过补丁修改 process".into());
    }
    let mut obj = to_obj(base);
    for (k, v) in patch {
        if v.is_null() {
            obj.remove(k);
        } else {
            obj.insert(k.clone(), v.clone());
        }
    }
    let out: T = serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())?;
    let back = to_obj(&out);
    for (k, v) in patch {
        if v.is_null() {
            continue;
        }
        let ok = match back.get(k) {
            Some(b) => b == v,
            None => {
                matches!(v, Value::Bool(false))
                    || v == &Value::from(0)
                    || v == &Value::String(String::new())
                    || v == &Value::Array(Vec::new())
            }
        };
        if !ok {
            return Err(format!("字段 {k} 的值 {v} 无效"));
        }
    }
    Ok(out)
}

/// 该进程写操作的起点：用户条目 > 系统条目的完整拷贝（写时复制）> `None`。
///
/// 写时复制是必须的：合并语义是「同名整条覆盖」，只写被改的那一个字段会让系统规则的
/// 其余字段（协议字段除外）掉回默认。
fn base_for<T: Rule>(system: &[T], user: &[T], process: &str) -> Option<T> {
    user.iter()
        .find(|r| same(r.process(), process))
        .or_else(|| system.iter().find(|r| same(r.process(), process)))
        .cloned()
}

/// 落回用户层：与系统一致（且未禁用）或空壳 ⇒ 删除条目；否则替换 / 追加。
fn commit_entry<T: Rule>(system: &[T], user: &mut Vec<T>, entry: T) {
    let key = entry.process().to_string();
    user.retain(|r| !same(r.process(), &key));
    let redundant = match system.iter().find(|r| same(r.process(), &key)) {
        Some(s) => !entry.disabled() && same_content(&entry, s),
        None => is_empty(&entry),
    };
    if !redundant {
        user.push(entry);
    }
}

fn upsert_in<T: Rule>(
    system: &[T],
    user: &mut Vec<T>,
    process: &str,
    patch: &Map<String, Value>,
) -> Result<(), String> {
    let process = validate_process(process)?;
    let base = base_for(system, user, &process).unwrap_or_else(|| {
        let mut t = T::default();
        t.set_process(&process);
        t
    });
    let entry = apply_patch_to(&base, patch)?;
    commit_entry(system, user, entry);
    Ok(())
}

fn reset_field_in<T: Rule>(
    system: &[T],
    user: &mut Vec<T>,
    process: &str,
    key: &str,
) -> Result<(), String> {
    if META_KEYS.contains(&key) {
        return Err(format!("字段 {key} 不可还原"));
    }
    let process = validate_process(process)?;
    if base_for(system, user, &process).is_none() {
        return Err("无此规则".into());
    }
    let sys_val = system
        .iter()
        .find(|r| same(r.process(), &process))
        .and_then(|s| to_obj(s).get(key).cloned())
        .unwrap_or(Value::Null);
    let mut patch = Map::new();
    patch.insert(key.to_string(), sys_val);
    upsert_in(system, user, &process, &patch)
}

fn set_disabled_in<T: Rule>(
    system: &[T],
    user: &mut Vec<T>,
    process: &str,
    flag: bool,
) -> Result<(), String> {
    let process = validate_process(process)?;
    let mut entry = base_for(system, user, &process).ok_or_else(|| "无此规则".to_string())?;
    entry.set_disabled(flag);
    commit_entry(system, user, entry);
    Ok(())
}

fn reset_in<T: Rule>(user: &mut Vec<T>, process: &str) -> bool {
    let before = user.len();
    user.retain(|r| !same(r.process(), process.trim()));
    user.len() != before
}

fn view_in<T: Rule>(
    system: &[T],
    user: &[T],
    merge: fn(Vec<T>, Vec<T>) -> Vec<T>,
) -> Vec<RuleView> {
    merge(system.to_vec(), user.to_vec())
        .into_iter()
        .filter(|r| !r.process().is_empty())
        .map(|eff| {
            let sys = system.iter().find(|r| same(r.process(), eff.process()));
            let usr = user.iter().find(|r| same(r.process(), eff.process()));
            let state = if usr.is_some_and(|u| u.disabled()) {
                RuleState::Disabled
            } else if let Some(s) = sys {
                if usr.is_some() && !same_content(&eff, s) {
                    RuleState::Modified
                } else {
                    RuleState::System
                }
            } else {
                RuleState::User
            };
            let mut e = to_obj(&eff);
            e.remove("disabled");
            let mut s = sys.map(to_obj).unwrap_or_default();
            s.remove("disabled");
            let mut keys = BTreeSet::new();
            for k in e.keys().chain(s.keys()) {
                if !META_KEYS.contains(&k.as_str()) && e.get(k) != s.get(k) {
                    keys.insert(k.clone());
                }
            }
            RuleView {
                process: eff.process().to_string(),
                state,
                effective: Value::Object(e),
                system: sys.map(|_| Value::Object(s)),
                overridden: keys.into_iter().collect(),
            }
        })
        .collect()
}

/// 系统层（`data` + `data_custom` 合并）与用户层的两层快照。
pub struct Layers {
    system: AppCompatFile,
    user: AppCompatFile,
}

/// 严格读用户层：不存在 ⇒ 空；读到但语法错 ⇒ `Err`（带路径与行列号）。
///
/// 运行时加载（`AppCompat::load`）对语法错是静默跳过整份的；管理界面的写操作不能这样——
/// 静默按空集重写会抹掉用户已有的全部覆盖。
fn read_user(path: &Path) -> Result<AppCompatFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppCompatFile::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Layers {
    pub fn load(
        data_dir: Option<&Path>,
        custom_dir: Option<&Path>,
        user_dir: Option<&Path>,
    ) -> Result<Self, String> {
        let mut system = AppCompatFile::default();
        if let Some(d) = data_dir
            && let Some(f) = load_file(&d.join(COMPAT_FILE_NAME))
        {
            system = f;
        }
        if let Some(c) = custom_dir
            && let Some(f) = load_file(&c.join(COMPAT_FILE_NAME))
        {
            system.apps = merge_rules(system.apps, f.apps);
            system.initial_mode_scope =
                merge_mode_scope(system.initial_mode_scope, f.initial_mode_scope);
            system.commit_newline = merge_commit_newline(system.commit_newline, f.commit_newline);
        }
        let user = match user_dir {
            Some(u) => read_user(&u.join(COMPAT_FILE_NAME))?,
            None => AppCompatFile::default(),
        };
        Ok(Self { system, user })
    }

    pub fn view(&self, sec: Section) -> Vec<RuleView> {
        match sec {
            Section::Apps => view_in(&self.system.apps, &self.user.apps, merge_rules),
            Section::InitialModeScope => view_in(
                &self.system.initial_mode_scope,
                &self.user.initial_mode_scope,
                merge_mode_scope,
            ),
            Section::CommitNewline => view_in(
                &self.system.commit_newline,
                &self.user.commit_newline,
                merge_commit_newline,
            ),
        }
    }

    /// 单条规则的视图（进程名不区分大小写）。
    pub fn view_of(&self, sec: Section, process: &str) -> Option<RuleView> {
        self.view(sec)
            .into_iter()
            .find(|v| same(&v.process, process.trim()))
    }

    pub fn upsert(
        &mut self,
        sec: Section,
        process: &str,
        patch: &Map<String, Value>,
    ) -> Result<(), String> {
        match sec {
            Section::Apps => upsert_in(&self.system.apps, &mut self.user.apps, process, patch),
            Section::InitialModeScope => upsert_in(
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                process,
                patch,
            ),
            Section::CommitNewline => upsert_in(
                &self.system.commit_newline,
                &mut self.user.commit_newline,
                process,
                patch,
            ),
        }
    }

    pub fn reset_field(&mut self, sec: Section, process: &str, key: &str) -> Result<(), String> {
        match sec {
            Section::Apps => reset_field_in(&self.system.apps, &mut self.user.apps, process, key),
            Section::InitialModeScope => reset_field_in(
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                process,
                key,
            ),
            Section::CommitNewline => reset_field_in(
                &self.system.commit_newline,
                &mut self.user.commit_newline,
                process,
                key,
            ),
        }
    }

    pub fn set_disabled(&mut self, sec: Section, process: &str, flag: bool) -> Result<(), String> {
        match sec {
            Section::Apps => set_disabled_in(&self.system.apps, &mut self.user.apps, process, flag),
            Section::InitialModeScope => set_disabled_in(
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                process,
                flag,
            ),
            Section::CommitNewline => set_disabled_in(
                &self.system.commit_newline,
                &mut self.user.commit_newline,
                process,
                flag,
            ),
        }
    }

    /// 还原单条：删掉用户层同名条目。用户新增的规则等于删除。返回是否删除过。
    pub fn reset(&mut self, sec: Section, process: &str) -> bool {
        match sec {
            Section::Apps => reset_in(&mut self.user.apps, process),
            Section::InitialModeScope => reset_in(&mut self.user.initial_mode_scope, process),
            Section::CommitNewline => reset_in(&mut self.user.commit_newline, process),
        }
    }

    /// 清空用户层三段。
    pub fn reset_all(&mut self) {
        self.user.apps.clear();
        self.user.initial_mode_scope.clear();
        self.user.commit_newline.clear();
    }

    /// 用户层渲染成文件全文（含固定文件头）。三段一并渲染，防止整份重写时漏段。
    pub fn render_user(&self) -> Result<String, toml::ser::Error> {
        render_user_compat(
            &self.user.apps,
            &self.user.initial_mode_scope,
            &self.user.commit_newline,
        )
    }

    /// 落盘用户层：先写临时文件再 rename，避免写到一半断电留下半份文件。
    pub fn save(&self, user_dir: &Path) -> std::io::Result<()> {
        let text = self
            .render_user()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::create_dir_all(user_dir)?;
        let path = user_dir.join(COMPAT_FILE_NAME);
        let tmp = user_dir.join(format!("{COMPAT_FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(text: &str) -> AppCompatFile {
        toml::from_str(text).expect("测试夹具必须是合法 TOML")
    }

    fn layers(sys: &str, usr: &str) -> Layers {
        Layers {
            system: file(sys),
            user: file(usr),
        }
    }

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("补丁必须是对象")
    }

    fn state_of(l: &Layers, p: &str) -> RuleState {
        l.view_of(Section::Apps, p).expect("规则应存在").state
    }

    const SYS: &str = "\
[[apps]]
process = \"Feishu.exe\"
composition_start_pair_guard = true

[[apps]]
process = \"Weixin.exe\"
caret_use_top = true

[[apps]]
process = \"Dota.exe\"
host_render = true
";

    #[test]
    fn view_marks_system_modified_user_disabled() {
        let usr = "\
[[apps]]
process = \"Weixin.exe\"
caret_use_top = true
first_show_mode = \"wait\"

[[apps]]
process = \"Dota.exe\"
disabled = true
host_render = true

[[apps]]
process = \"Mine.exe\"
auto_pair = true
";
        let l = layers(SYS, usr);
        assert_eq!(state_of(&l, "Feishu.exe"), RuleState::System);
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::Modified);
        assert_eq!(state_of(&l, "Dota.exe"), RuleState::Disabled);
        assert_eq!(state_of(&l, "Mine.exe"), RuleState::User);
    }

    #[test]
    fn overridden_lists_only_fields_that_differ_from_system() {
        let usr = "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\nauto_pair = true\n";
        let l = layers(SYS, usr);
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.overridden, vec!["auto_pair".to_string()]);
        assert!(v.system.is_some());
    }

    #[test]
    fn user_only_rule_reports_all_its_fields_as_overridden() {
        let l = layers(
            SYS,
            "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\ncaret_use_top = true\n",
        );
        let v = l.view_of(Section::Apps, "Mine.exe").unwrap();
        assert_eq!(
            v.overridden,
            vec!["auto_pair".to_string(), "caret_use_top".to_string()]
        );
        assert!(v.system.is_none());
    }

    #[test]
    fn patch_on_system_rule_copies_the_rest_first() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Feishu.exe",
            &patch(json!({"first_show_mode": "wait"})),
        )
        .unwrap();
        let u = &l.user.apps[0];
        assert_eq!(
            u.composition_start_pair_guard,
            Some(true),
            "写时复制：系统字段必须带过来"
        );
        let v = l.view_of(Section::Apps, "Feishu.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified);
        assert_eq!(v.effective["composition_start_pair_guard"], json!(true));
        assert_eq!(v.effective["first_show_mode"], json!("wait"));
    }

    #[test]
    fn patch_equal_to_system_removes_the_user_entry() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::Modified);
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": null})),
        )
        .unwrap();
        assert!(l.user.apps.is_empty(), "改回系统值后不应留下冗余的用户条目");
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
    }

    #[test]
    fn patch_rejects_invalid_enum_value_and_leaves_layers_untouched() {
        let mut l = layers(SYS, "");
        let err = l
            .upsert(
                Section::Apps,
                "Weixin.exe",
                &patch(json!({"first_show_mode": "bogus"})),
            )
            .unwrap_err();
        assert!(err.contains("first_show_mode"), "错误应指出字段: {err}");
        assert!(l.user.apps.is_empty());
    }

    #[test]
    fn patch_accepts_false_for_bool() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"auto_pair": true, "caret_use_top": false})),
        )
        .expect("bool false 是合法值");
        assert!(l.view_of(Section::Apps, "Mine.exe").is_some());
    }

    #[test]
    fn patch_rejects_wrong_type() {
        let mut l = layers(SYS, "");
        assert!(
            l.upsert(
                Section::Apps,
                "Mine.exe",
                &patch(json!({"caret_offset_x": "12"}))
            )
            .is_err()
        );
    }

    #[test]
    fn patch_null_clears_the_field() {
        let mut l = layers(
            SYS,
            "[[apps]]\nprocess = \"Mine.exe\"\nfirst_show_mode = \"wait\"\nauto_pair = true\n",
        );
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"first_show_mode": null})),
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Mine.exe").unwrap();
        assert!(v.effective.get("first_show_mode").is_none());
        assert_eq!(v.effective["auto_pair"], json!(true));
    }

    #[test]
    fn patch_rejects_changing_process_key() {
        let mut l = layers(SYS, "");
        assert!(
            l.upsert(
                Section::Apps,
                "Weixin.exe",
                &patch(json!({"process": "Other.exe"}))
            )
            .is_err()
        );
    }

    #[test]
    fn process_validation() {
        assert!(validate_process("").is_err());
        assert!(validate_process("   ").is_err());
        assert!(validate_process("a/b.exe").is_err());
        assert!(validate_process("a\\b.exe").is_err());
        assert!(validate_process("a\u{7}b.exe").is_err());
        assert_eq!(validate_process(" x.exe ").unwrap(), "x.exe");
    }

    #[test]
    fn process_is_matched_case_insensitively_and_trimmed() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "  weixin.EXE ",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        assert_eq!(l.user.apps.len(), 1);
        assert_eq!(
            l.user.apps[0].process, "Weixin.exe",
            "沿用系统层的写法，不新增一条"
        );
    }

    #[test]
    fn reset_field_restores_system_value_and_keeps_other_edits() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"caret_use_top": false, "auto_pair": true})),
        )
        .unwrap();
        l.reset_field(Section::Apps, "Weixin.exe", "caret_use_top")
            .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.effective["caret_use_top"], json!(true), "回到系统值");
        assert_eq!(v.effective["auto_pair"], json!(true), "另一个改写保留");
        assert_eq!(v.overridden, vec!["auto_pair".to_string()]);
    }

    #[test]
    fn reset_field_rejects_meta_keys_and_unknown_rules() {
        let mut l = layers(SYS, "");
        assert!(
            l.reset_field(Section::Apps, "Weixin.exe", "process")
                .is_err()
        );
        assert!(
            l.reset_field(Section::Apps, "Nope.exe", "auto_pair")
                .is_err()
        );
    }

    #[test]
    fn set_disabled_roundtrip_restores_prior_edits() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        l.set_disabled(Section::Apps, "Weixin.exe", true).unwrap();
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::Disabled);
        l.set_disabled(Section::Apps, "Weixin.exe", false).unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified, "启用后回到禁用前的改写状态");
        assert_eq!(v.effective["auto_pair"], json!(true));
        assert!(l.user.apps.iter().all(|r| !r.disabled));
    }

    #[test]
    fn enabling_an_untouched_disabled_rule_removes_the_entry() {
        let mut l = layers(SYS, "");
        l.set_disabled(Section::Apps, "Weixin.exe", true).unwrap();
        l.set_disabled(Section::Apps, "Weixin.exe", false).unwrap();
        assert!(l.user.apps.is_empty(), "与系统一致就不该留条目");
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
    }

    #[test]
    fn set_disabled_on_unknown_process_errors() {
        let mut l = layers(SYS, "");
        assert_eq!(
            l.set_disabled(Section::Apps, "Nope.exe", true).unwrap_err(),
            "无此规则"
        );
    }

    #[test]
    fn set_disabled_on_user_only_rule_is_allowed() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\n");
        l.set_disabled(Section::Apps, "Mine.exe", true).unwrap();
        assert_eq!(state_of(&l, "Mine.exe"), RuleState::Disabled);
        l.set_disabled(Section::Apps, "Mine.exe", false).unwrap();
        assert_eq!(state_of(&l, "Mine.exe"), RuleState::User);
    }

    #[test]
    fn reset_removes_user_entry_and_returns_whether_it_existed() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        assert!(l.reset(Section::Apps, "weixin.exe"));
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
        assert!(!l.reset(Section::Apps, "Weixin.exe"));
    }

    #[test]
    fn reset_of_user_only_rule_deletes_it() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\n");
        assert!(l.reset(Section::Apps, "Mine.exe"));
        assert!(l.view_of(Section::Apps, "Mine.exe").is_none());
    }

    #[test]
    fn reset_all_clears_all_three_sections() {
        let usr = "\
[[apps]]
process = \"Mine.exe\"
auto_pair = true

[[initial_mode_scope]]
process = \"explorer.exe\"
classes = [\"CabinetWClass\"]

[[commit_newline]]
process = \"WINWORD.EXE\"
style = \"cr\"
";
        let mut l = layers(SYS, usr);
        l.reset_all();
        let text = l.render_user().unwrap();
        assert!(!text.contains("[[apps]]"), "{text}");
        assert!(!text.contains("[[initial_mode_scope]]"), "{text}");
        assert!(!text.contains("[[commit_newline]]"), "{text}");
    }

    #[test]
    fn sections_are_independent() {
        let sys = "\
[[apps]]
process = \"explorer.exe\"
auto_pair = true

[[initial_mode_scope]]
process = \"explorer.exe\"
classes = [\"CabinetWClass\"]
";
        let mut l = layers(sys, "");
        l.upsert(
            Section::Apps,
            "explorer.exe",
            &patch(json!({"caret_use_top": true})),
        )
        .unwrap();
        assert!(
            l.user.initial_mode_scope.is_empty(),
            "改 apps 不能牵连 initial_mode_scope"
        );
        assert_eq!(
            l.view_of(Section::InitialModeScope, "explorer.exe")
                .unwrap()
                .state,
            RuleState::System
        );
    }

    #[test]
    fn scope_and_newline_sections_support_patch_and_disable() {
        let sys = "\
[[initial_mode_scope]]
process = \"explorer.exe\"
classes = [\"CabinetWClass\"]

[[commit_newline]]
process = \"WINWORD.EXE\"
style = \"cr\"
";
        let mut l = layers(sys, "");
        l.upsert(
            Section::InitialModeScope,
            "explorer.exe",
            &patch(json!({"classes": ["CabinetWClass", "Progman"]})),
        )
        .unwrap();
        assert_eq!(
            l.view_of(Section::InitialModeScope, "explorer.exe")
                .unwrap()
                .state,
            RuleState::Modified
        );
        l.set_disabled(Section::CommitNewline, "WINWORD.EXE", true)
            .unwrap();
        assert_eq!(
            l.view_of(Section::CommitNewline, "WINWORD.EXE")
                .unwrap()
                .state,
            RuleState::Disabled
        );
        assert!(
            l.upsert(
                Section::CommitNewline,
                "WINWORD.EXE",
                &patch(json!({"style": "bogus"}))
            )
            .is_err()
        );
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wind_compat_adm_{tag}_{}", std::process::id()))
    }

    #[test]
    fn strict_load_reports_syntax_error_with_path() {
        let dir = tmp("syntax");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(COMPAT_FILE_NAME), "[[apps\nprocess=").unwrap();
        let err = match Layers::load(None, None, Some(&dir)) {
            Err(e) => e,
            Ok(_) => panic!("语法错必须报错，而不是静默按空集处理"),
        };
        assert!(err.contains(COMPAT_FILE_NAME), "应带路径: {err}");
        assert!(err.contains("line"), "应带行号: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_user_file_is_an_empty_layer() {
        let dir = tmp("missing");
        let l = Layers::load(None, None, Some(&dir)).unwrap();
        assert!(l.user.apps.is_empty());
    }

    #[test]
    fn save_is_atomic_and_preserves_other_sections() {
        let dir = tmp("save");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(COMPAT_FILE_NAME),
            "[[commit_newline]]\nprocess = \"WINWORD.EXE\"\nstyle = \"cr\"\n",
        )
        .unwrap();
        let mut l = Layers::load(None, None, Some(&dir)).unwrap();
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        l.save(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join(COMPAT_FILE_NAME)).unwrap();
        assert!(
            text.contains("[[commit_newline]]"),
            "别的段必须原样带回: {text}"
        );
        assert!(text.contains("Mine.exe"), "{text}");
        assert!(
            !dir.join(format!("{COMPAT_FILE_NAME}.tmp")).exists(),
            "不应残留临时文件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
