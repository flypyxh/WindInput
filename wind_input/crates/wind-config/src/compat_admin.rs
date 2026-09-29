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
use serde_json::{Map, Value, json};
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
#[derive(Clone)]
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

// ───────────────────────── 导出 / 导入 ─────────────────────────

/// 导出范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// 仅用户层（我的改动）。方便分享，也是默认。
    User,
    /// 系统层与用户层合并后的全部生效规则（剔除已禁用的）。
    Effective,
}

impl ExportScope {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "effective" => Some(Self::Effective),
            _ => None,
        }
    }
}

/// 导入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// 同名整条覆盖，用户层里没被提到的条目保留。
    Merge,
    /// 先清空用户层再导入。
    Replace,
}

impl ImportMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "merge" => Some(Self::Merge),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

/// 导入预览里的一条。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportItem {
    pub section: &'static str,
    pub process: String,
    /// `add` / `override` / `disable` / `unchanged`。
    pub action: &'static str,
}

/// 导入预览：会发生什么，以及导入文本里有哪些会被忽略 / 回落的内容。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub items: Vec<ImportItem>,
    /// 不认识的键（`<段>.<键>`）或整段。它们不会生效。
    pub ignored_keys: Vec<String>,
    /// 值写错、被容错回落成「跟随全局」的原值。
    pub fallbacks: Vec<String>,
}

/// 严格解析导入文本：语法错 ⇒ `Err`（带行号）。同时收集不认识的键与被回落的值。
fn parse_incoming(text: &str) -> Result<(AppCompatFile, Vec<String>, Vec<String>), String> {
    crate::tolerant_de::clear_fallbacks();
    let file: AppCompatFile =
        toml::from_str(text).map_err(|e| format!("导入内容不是合法的 TOML: {e}"))?;
    let fallbacks = crate::tolerant_de::take_fallbacks();
    let raw: toml::Value =
        toml::from_str(text).map_err(|e| format!("导入内容不是合法的 TOML: {e}"))?;
    let mut ignored = Vec::new();
    if let Some(top) = raw.as_table() {
        for (name, value) in top {
            let Some(sec) = Section::parse(name) else {
                ignored.push(name.clone());
                continue;
            };
            let known = crate::compat_schema::known_keys(sec.as_str());
            for tbl in value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_table())
            {
                for k in tbl.keys() {
                    if !META_KEYS.contains(&k.as_str()) && !known.contains(&k.as_str()) {
                        ignored.push(format!("{}.{k}", sec.as_str()));
                    }
                }
            }
        }
    }
    ignored.sort();
    ignored.dedup();
    Ok((file, ignored, fallbacks))
}

/// 把一段导入规则落进用户层：整条覆盖同名条目，再与系统层比对去冗余。
fn import_section<T: Rule>(system: &[T], user: &mut Vec<T>, incoming: Vec<T>) {
    for rule in incoming.into_iter().filter(|r| !r.process().is_empty()) {
        commit_entry(system, user, rule);
    }
}

impl Layers {
    /// 导出成 `compat.toml` 文本。内容与落盘走同一个渲染函数，保证互相能导入。
    pub fn export(&self, scope: ExportScope) -> Result<String, toml::ser::Error> {
        match scope {
            ExportScope::User => self.render_user(),
            ExportScope::Effective => {
                let mut apps = merge_rules(self.system.apps.clone(), self.user.apps.clone());
                let mut scopes = merge_mode_scope(
                    self.system.initial_mode_scope.clone(),
                    self.user.initial_mode_scope.clone(),
                );
                let mut newline = merge_commit_newline(
                    self.system.commit_newline.clone(),
                    self.user.commit_newline.clone(),
                );
                apps.retain(|r| !r.disabled);
                scopes.retain(|r| !r.disabled);
                newline.retain(|r| !r.disabled);
                render_user_compat(&apps, &scopes, &newline)
            }
        }
    }

    /// 在副本上试算导入结果，返回（导入后的层, 逐条动作）。不改 `self`。
    fn plan_import(&self, incoming: AppCompatFile, mode: ImportMode) -> (Layers, Vec<ImportItem>) {
        let mut after = self.clone();
        if mode == ImportMode::Replace {
            after.reset_all();
        }
        let mut touched: Vec<(Section, String, bool)> = Vec::new();
        for r in &incoming.apps {
            touched.push((Section::Apps, r.process.clone(), r.disabled));
        }
        for r in &incoming.initial_mode_scope {
            touched.push((Section::InitialModeScope, r.process.clone(), r.disabled));
        }
        for r in &incoming.commit_newline {
            touched.push((Section::CommitNewline, r.process.clone(), r.disabled));
        }
        import_section(&after.system.apps, &mut after.user.apps, incoming.apps);
        import_section(
            &after.system.initial_mode_scope,
            &mut after.user.initial_mode_scope,
            incoming.initial_mode_scope,
        );
        import_section(
            &after.system.commit_newline,
            &mut after.user.commit_newline,
            incoming.commit_newline,
        );
        let mut items = Vec::new();
        for (sec, process, disabled) in touched.into_iter().filter(|(_, p, _)| !p.is_empty()) {
            let before = self.view_of(sec, &process);
            let now = after.view_of(sec, &process);
            let action = if disabled {
                "disable"
            } else {
                match (&before, &now) {
                    (None, Some(_)) => "add",
                    (Some(b), Some(n)) if b.effective != n.effective || b.state != n.state => {
                        "override"
                    }
                    _ => "unchanged",
                }
            };
            items.push(ImportItem {
                section: sec.as_str(),
                process,
                action,
            });
        }
        (after, items)
    }

    /// 导入预览：不落盘，不改 `self`。
    pub fn import_preview(&self, text: &str, mode: ImportMode) -> Result<ImportPreview, String> {
        let (file, ignored_keys, fallbacks) = parse_incoming(text)?;
        let (_, items) = self.plan_import(file, mode);
        Ok(ImportPreview {
            items,
            ignored_keys,
            fallbacks,
        })
    }

    /// 应用导入到 `self`（调用方随后负责 `save`）。
    pub fn import_apply(&mut self, text: &str, mode: ImportMode) -> Result<ImportPreview, String> {
        let (file, ignored_keys, fallbacks) = parse_incoming(text)?;
        let (after, items) = self.plan_import(file, mode);
        *self = after;
        Ok(ImportPreview {
            items,
            ignored_keys,
            fallbacks,
        })
    }
}

// ───────────────────────── RPC 分派 ─────────────────────────

/// 一次 RPC 的结果。`wrote` 告诉宿主是否需要重载规则表。
pub struct RpcOutcome {
    pub value: Value,
    pub wrote: bool,
}

/// 覆盖前备份用户层文件为 `compat.toml.bak`（不存在则什么也不做）。
fn backup_user_file(user_dir: &Path) -> Result<(), String> {
    let path = user_dir.join(COMPAT_FILE_NAME);
    if path.exists() {
        std::fs::copy(&path, user_dir.join(format!("{COMPAT_FILE_NAME}.bak")))
            .map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

/// `compat.*` RPC 的全部逻辑。宿主只需：调用它、按 `wrote` 决定是否重载。
pub fn rpc(
    method: &str,
    params: &Value,
    data_dir: Option<&Path>,
    custom_dir: Option<&Path>,
    user_dir: Option<&Path>,
) -> Result<RpcOutcome, String> {
    let read = |value: Value| {
        Ok(RpcOutcome {
            value,
            wrote: false,
        })
    };
    if method == "compat.schema" {
        return read(crate::compat_schema::schema_json());
    }
    let text_param = |k: &str| -> Result<&str, String> {
        params
            .get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("缺少参数 {k}"))
    };
    let section = || -> Result<Section, String> {
        Section::parse(text_param("section")?).ok_or_else(|| "section 无效".to_string())
    };
    let need_user_dir = || user_dir.ok_or_else(|| "无用户配置目录".to_string());

    // 先 clear 再加载再 take：`list` 要把「加载时被回落的值」作为告警带回去。
    crate::tolerant_de::clear_fallbacks();
    let mut layers = Layers::load(data_dir, custom_dir, user_dir)?;
    let warnings = crate::tolerant_de::take_fallbacks();

    let wrote = |value: Value| Ok(RpcOutcome { value, wrote: true });
    match method {
        "compat.list" => read(json!({
            "rules": layers.view(section()?),
            "warnings": warnings,
        })),
        "compat.export" => {
            let scope = match params.get("scope").and_then(Value::as_str) {
                None => ExportScope::User,
                Some(s) => ExportScope::parse(s).ok_or_else(|| "scope 无效".to_string())?,
            };
            let content = layers.export(scope).map_err(|e| e.to_string())?;
            read(json!({ "content": content }))
        }
        "compat.upsert" => {
            let dir = need_user_dir()?;
            let (sec, process) = (section()?, text_param("process")?);
            let patch = params
                .get("patch")
                .and_then(Value::as_object)
                .ok_or_else(|| "缺少参数 patch".to_string())?;
            layers.upsert(sec, process, patch)?;
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true, "rule": layers.view_of(sec, process) }))
        }
        "compat.resetField" => {
            let dir = need_user_dir()?;
            let (sec, process) = (section()?, text_param("process")?);
            layers.reset_field(sec, process, text_param("key")?)?;
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true, "rule": layers.view_of(sec, process) }))
        }
        "compat.setDisabled" => {
            let dir = need_user_dir()?;
            let (sec, process) = (section()?, text_param("process")?);
            let flag = params
                .get("disabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| "缺少参数 disabled".to_string())?;
            layers.set_disabled(sec, process, flag)?;
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true, "rule": layers.view_of(sec, process) }))
        }
        "compat.reset" => {
            let dir = need_user_dir()?;
            let (sec, process) = (section()?, text_param("process")?);
            let removed = layers.reset(sec, process);
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true, "removed": removed, "rule": layers.view_of(sec, process) }))
        }
        "compat.resetAll" => {
            let dir = need_user_dir()?;
            backup_user_file(dir)?;
            layers.reset_all();
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true }))
        }
        "compat.import" => {
            let content = text_param("content")?;
            let mode = match params.get("mode").and_then(Value::as_str) {
                None => ImportMode::Merge,
                Some(s) => ImportMode::parse(s).ok_or_else(|| "mode 无效".to_string())?,
            };
            let dry_run = params
                .get("dryRun")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if dry_run {
                let preview = layers.import_preview(content, mode)?;
                let mut v = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
                v["applied"] = json!(false);
                return read(v);
            }
            let dir = need_user_dir()?;
            let preview = layers.import_apply(content, mode)?;
            backup_user_file(dir)?;
            layers.save(dir).map_err(|e| e.to_string())?;
            let mut v = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
            v["applied"] = json!(true);
            wrote(v)
        }
        other => Err(format!("unknown method: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 元数据里登记的每个枚举可选值都必须被真实解析器接受并原样往返；
    /// 否则设置端下拉框里会出现「选了却写不进去」的选项。
    #[test]
    fn every_registered_enum_option_roundtrips_through_the_real_parser() {
        use crate::compat_schema::{COMPAT_FIELDS, Kind};
        for f in COMPAT_FIELDS.iter().filter(|f| f.kind == Kind::Enum) {
            let sec = Section::parse(f.section).unwrap();
            for opt in f.options {
                let mut l = layers("", "");
                l.upsert(sec, "probe.exe", &patch(json!({ f.key: opt })))
                    .unwrap_or_else(|e| panic!("{}.{} = {opt} 被拒绝: {e}", f.section, f.key));
                let v = l.view_of(sec, "probe.exe").unwrap();
                assert_eq!(
                    v.effective[f.key],
                    json!(opt),
                    "{}.{} = {opt} 往返后不一致",
                    f.section,
                    f.key
                );
            }
        }
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

    // ───────────── 导出 / 导入 ─────────────

    const USER_A: &str =
        "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\nauto_pair = true\n";

    #[test]
    fn export_user_contains_only_user_layer() {
        let l = layers(SYS, USER_A);
        let text = l.export(ExportScope::User).unwrap();
        assert!(text.contains("Weixin.exe"), "{text}");
        assert!(
            !text.contains("Feishu.exe"),
            "仅用户层不应含系统规则: {text}"
        );
    }

    #[test]
    fn export_effective_merges_and_drops_disabled() {
        let usr = format!(
            "{USER_A}\n[[apps]]\nprocess = \"Dota.exe\"\ndisabled = true\nhost_render = true\n"
        );
        let l = layers(SYS, &usr);
        let text = l.export(ExportScope::Effective).unwrap();
        assert!(text.contains("Feishu.exe"), "系统规则应在: {text}");
        assert!(
            text.contains("auto_pair = true"),
            "合并后的改写应在: {text}"
        );
        assert!(!text.contains("Dota.exe"), "被禁用的不应导出: {text}");
        assert!(!text.contains("disabled"), "{text}");
    }

    #[test]
    fn export_then_import_replace_roundtrips() {
        let usr = format!(
            "{USER_A}\n[[apps]]\nprocess = \"Dota.exe\"\ndisabled = true\nhost_render = true\n"
        );
        let a = layers(SYS, &usr);
        let text = a.export(ExportScope::User).unwrap();
        let mut b = layers(SYS, "");
        b.import_apply(&text, ImportMode::Replace).unwrap();
        for p in ["Feishu.exe", "Weixin.exe", "Dota.exe"] {
            let (va, vb) = (
                a.view_of(Section::Apps, p).unwrap(),
                b.view_of(Section::Apps, p).unwrap(),
            );
            assert_eq!(va.state, vb.state, "{p}");
            assert_eq!(va.effective, vb.effective, "{p}");
        }
    }

    #[test]
    fn import_preview_classifies_add_override_disable_unchanged() {
        let incoming = "\
[[apps]]
process = \"Brand.exe\"
auto_pair = true

[[apps]]
process = \"Weixin.exe\"
caret_use_top = true
auto_pair = true

[[apps]]
process = \"Dota.exe\"
disabled = true
host_render = true

[[apps]]
process = \"Feishu.exe\"
composition_start_pair_guard = true
";
        let l = layers(SYS, "");
        let p = l.import_preview(incoming, ImportMode::Merge).unwrap();
        let act = |name: &str| p.items.iter().find(|i| i.process == name).map(|i| i.action);
        assert_eq!(act("Brand.exe"), Some("add"));
        assert_eq!(act("Weixin.exe"), Some("override"));
        assert_eq!(act("Dota.exe"), Some("disable"));
        assert_eq!(
            act("Feishu.exe"),
            Some("unchanged"),
            "与系统完全一致 = 无变化"
        );
    }

    #[test]
    fn import_preview_does_not_mutate_layers() {
        let l = layers(SYS, USER_A);
        let before: Vec<_> = l
            .view(Section::Apps)
            .into_iter()
            .map(|v| (v.process, v.state))
            .collect();
        let _ = l
            .import_preview(
                "[[apps]]\nprocess = \"Brand.exe\"\nauto_pair = true\n",
                ImportMode::Replace,
            )
            .unwrap();
        let after: Vec<_> = l
            .view(Section::Apps)
            .into_iter()
            .map(|v| (v.process, v.state))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn import_merge_keeps_unmentioned_user_entries() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \"E.exe\"\nauto_pair = true\n");
        l.import_apply(
            "[[apps]]\nprocess = \"C.exe\"\nauto_pair = true\n",
            ImportMode::Merge,
        )
        .unwrap();
        assert!(
            l.view_of(Section::Apps, "E.exe").is_some(),
            "Merge 不能动没被提到的条目"
        );
        assert!(l.view_of(Section::Apps, "C.exe").is_some());
    }

    #[test]
    fn import_replace_drops_unmentioned_user_entries() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \"E.exe\"\nauto_pair = true\n");
        l.import_apply(
            "[[apps]]\nprocess = \"C.exe\"\nauto_pair = true\n",
            ImportMode::Replace,
        )
        .unwrap();
        assert!(
            l.view_of(Section::Apps, "E.exe").is_none(),
            "Replace 要先清空用户层"
        );
        assert!(l.view_of(Section::Apps, "C.exe").is_some());
    }

    #[test]
    fn import_reports_unknown_keys_and_sections() {
        let text = "[[apps]]\nprocess = \"A.exe\"\nbogus_key = 1\nauto_pair = true\n\n[[mystery]]\nx = 1\n";
        let p = layers(SYS, "")
            .import_preview(text, ImportMode::Merge)
            .unwrap();
        assert!(
            p.ignored_keys.contains(&"apps.bogus_key".to_string()),
            "{:?}",
            p.ignored_keys
        );
        assert!(
            p.ignored_keys.contains(&"mystery".to_string()),
            "{:?}",
            p.ignored_keys
        );
    }

    #[test]
    fn import_reports_value_fallbacks() {
        let text = "[[apps]]\nprocess = \"A.exe\"\nfirst_show_mode = \"bogus\"\nauto_pair = true\n";
        let p = layers(SYS, "")
            .import_preview(text, ImportMode::Merge)
            .unwrap();
        assert!(
            p.fallbacks.iter().any(|f| f.contains("bogus")),
            "{:?}",
            p.fallbacks
        );
    }

    #[test]
    fn import_syntax_error_is_an_error_not_silent() {
        let err = layers(SYS, "")
            .import_preview("[[apps\nprocess=", ImportMode::Merge)
            .unwrap_err();
        assert!(err.contains("line"), "应带行号: {err}");
    }

    // ───────────── RPC ─────────────

    fn rpc_dirs(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let (d, u) = (tmp(&format!("{tag}_d")), tmp(&format!("{tag}_u")));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(COMPAT_FILE_NAME), SYS).unwrap();
        (d, u)
    }

    fn call(m: &str, p: Value, d: &Path, u: &Path) -> Result<RpcOutcome, String> {
        rpc(m, &p, Some(d), None, Some(u))
    }

    fn cleanup(d: &Path, u: &Path) {
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(u);
    }

    #[test]
    fn rpc_list_returns_views_and_warnings() {
        let (d, u) = rpc_dirs("list");
        let r = call("compat.list", json!({"section": "apps"}), &d, &u).unwrap();
        assert!(!r.wrote);
        assert_eq!(r.value["rules"].as_array().unwrap().len(), 3);
        assert!(r.value["warnings"].is_array());
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_unknown_method_and_bad_section_error() {
        let (d, u) = rpc_dirs("bad");
        assert!(call("compat.bogus", json!({}), &d, &u).is_err());
        assert!(call("compat.list", json!({"section": "x"}), &d, &u).is_err());
        assert!(call("compat.list", json!({}), &d, &u).is_err());
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_write_methods_report_wrote_true_and_persist() {
        let (d, u) = rpc_dirs("write");
        let r = call(
            "compat.upsert",
            json!({"section": "apps", "process": "Weixin.exe", "patch": {"auto_pair": true}}),
            &d,
            &u,
        )
        .unwrap();
        assert!(r.wrote);
        assert_eq!(r.value["rule"]["state"], json!("modified"));
        assert!(
            std::fs::read_to_string(u.join(COMPAT_FILE_NAME))
                .unwrap()
                .contains("auto_pair")
        );

        let r = call(
            "compat.setDisabled",
            json!({"section": "apps", "process": "Feishu.exe", "disabled": true}),
            &d,
            &u,
        )
        .unwrap();
        assert!(r.wrote);
        assert_eq!(r.value["rule"]["state"], json!("disabled"));

        let r = call(
            "compat.resetField",
            json!({"section": "apps", "process": "Weixin.exe", "key": "auto_pair"}),
            &d,
            &u,
        )
        .unwrap();
        assert!(r.wrote);
        assert_eq!(r.value["rule"]["state"], json!("system"));

        let r = call(
            "compat.reset",
            json!({"section": "apps", "process": "Feishu.exe"}),
            &d,
            &u,
        )
        .unwrap();
        assert!(r.wrote);
        assert_eq!(r.value["removed"], json!(true));

        // 只读方法不写
        for (m, p) in [
            ("compat.list", json!({"section": "apps"})),
            ("compat.schema", json!({})),
            ("compat.export", json!({"scope": "effective"})),
        ] {
            assert!(!call(m, p, &d, &u).unwrap().wrote, "{m} 不应报告写入");
        }
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_upsert_rejects_bad_value_and_does_not_touch_disk() {
        let (d, u) = rpc_dirs("reject");
        let err = call(
            "compat.upsert",
            json!({"section": "apps", "process": "Weixin.exe", "patch": {"first_show_mode": "bogus"}}),
            &d,
            &u,
        )
        .err()
        .expect("非法取值必须被拒绝");
        assert!(err.contains("first_show_mode"), "{err}");
        assert!(
            !u.join(COMPAT_FILE_NAME).exists(),
            "被拒绝的写入不应产生文件"
        );
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_write_without_user_dir_errors() {
        let (d, u) = rpc_dirs("nodir");
        let r = rpc(
            "compat.upsert",
            &json!({"section": "apps", "process": "A.exe", "patch": {"auto_pair": true}}),
            Some(&d),
            None,
            None,
        );
        assert_eq!(r.err().as_deref(), Some("无用户配置目录"));
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_import_dry_run_does_not_write() {
        let (d, u) = rpc_dirs("dry");
        let r = call(
            "compat.import",
            json!({"content": "[[apps]]\nprocess = \"New.exe\"\nauto_pair = true\n", "dryRun": true}),
            &d,
            &u,
        )
        .unwrap();
        assert!(!r.wrote);
        assert_eq!(r.value["applied"], json!(false));
        assert_eq!(r.value["items"][0]["action"], json!("add"));
        assert!(!u.join(COMPAT_FILE_NAME).exists());
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_import_backs_up_existing_user_file() {
        let (d, u) = rpc_dirs("bak");
        std::fs::create_dir_all(&u).unwrap();
        let original = "[[apps]]\nprocess = \"Old.exe\"\nauto_pair = true\n";
        std::fs::write(u.join(COMPAT_FILE_NAME), original).unwrap();
        let r = call(
            "compat.import",
            json!({"content": "[[apps]]\nprocess = \"New.exe\"\nauto_pair = true\n", "mode": "replace"}),
            &d,
            &u,
        )
        .unwrap();
        assert!(r.wrote);
        assert_eq!(r.value["applied"], json!(true));
        let bak = std::fs::read_to_string(u.join(format!("{COMPAT_FILE_NAME}.bak"))).unwrap();
        assert_eq!(bak, original, "备份必须是导入前的原文");
        assert!(
            std::fs::read_to_string(u.join(COMPAT_FILE_NAME))
                .unwrap()
                .contains("New.exe")
        );
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_reset_all_backs_up_and_clears() {
        let (d, u) = rpc_dirs("all");
        std::fs::create_dir_all(&u).unwrap();
        let original = "[[apps]]\nprocess = \"Old.exe\"\nauto_pair = true\n";
        std::fs::write(u.join(COMPAT_FILE_NAME), original).unwrap();
        let r = call("compat.resetAll", json!({}), &d, &u).unwrap();
        assert!(r.wrote);
        assert_eq!(
            std::fs::read_to_string(u.join(format!("{COMPAT_FILE_NAME}.bak"))).unwrap(),
            original
        );
        assert!(
            !std::fs::read_to_string(u.join(COMPAT_FILE_NAME))
                .unwrap()
                .contains("Old.exe")
        );
        cleanup(&d, &u);
    }

    #[test]
    fn rpc_write_on_broken_user_file_errors_and_keeps_it() {
        let (d, u) = rpc_dirs("broken");
        std::fs::create_dir_all(&u).unwrap();
        let broken = "[[apps\nprocess=";
        std::fs::write(u.join(COMPAT_FILE_NAME), broken).unwrap();
        let err = call(
            "compat.upsert",
            json!({"section": "apps", "process": "A.exe", "patch": {"auto_pair": true}}),
            &d,
            &u,
        )
        .err()
        .expect("用户层语法错必须报错");
        assert!(err.contains("line"), "{err}");
        assert_eq!(
            std::fs::read_to_string(u.join(COMPAT_FILE_NAME)).unwrap(),
            broken,
            "出错时不得改写（更不能按空集重写）用户层"
        );
        cleanup(&d, &u);
    }
}
