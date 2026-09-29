//! 兼容规则的分层管理：视图 + 字段级差异写入 + 禁用 + 还原（设置端 `compat.*` RPC 的纯逻辑层）。
//!
//! 只写**用户层**；系统层（`data/compat.toml`、`data_custom`）永不写。用户层记的是**相对系统层的
//! 差异**（原则与语法见 [`crate::compat_overlay`]），所以：
//! - 改一个字段只写这一个字段，系统规则的其余字段继续继承（出厂新增的修复自动送达）；
//! - 改成与系统一致的值，这个键（乃至整条）自动消失；
//! - 「还原」= 删掉用户层的痕迹，「清除」= 显式 `unset`（回到跟随全局），两者不同。
//!
//! 三段（`[[apps]]` / `[[initial_mode_scope]]` / `[[commit_newline]]`）共用一套逻辑，只有字段校验
//! 用到各自的结构体类型。设计见 `docs/design/compat-settings-ui.md`。

use crate::app_compat::{
    AppCompatRule, COMPAT_FILE_NAME, CommitNewlineRule, InitialModeScopeRule, USER_COMPAT_HEADER,
    load_raw, write_atomic,
};
use crate::compat_overlay::{
    FieldEdit, META_KEYS, Obj, Raw, apply_edits, compose, is_disabled, normalize, overlay,
    overlay_raw, parse_raw, process_of, render_raw, same_process,
};
use crate::compat_schema::{COMPAT_FIELDS, Kind, known_keys};
use serde::Serialize;
use serde::de::DeserializeOwned;
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

const ALL_SECTIONS: [Section; 3] = [
    Section::Apps,
    Section::InitialModeScope,
    Section::CommitNewline,
];

/// 按段选出对应的结构体类型，在 `$body` 里以 `$T` 引用。
macro_rules! with_section {
    ($sec:expr, $T:ident => $body:expr) => {
        match $sec {
            Section::Apps => {
                type $T = AppCompatRule;
                $body
            }
            Section::InitialModeScope => {
                type $T = InitialModeScopeRule;
                $body
            }
            Section::CommitNewline => {
                type $T = CommitNewlineRule;
                $body
            }
        }
    };
}

/// 一条规则在分层视图里的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    /// 只来自系统层，未被动过（或用户层的差异全是冗余）。
    System,
    /// 系统规则被用户改写（用户层有真正的差异）。
    Modified,
    /// 用户新增（系统层没有这个进程）。
    User,
    /// 被禁用（用户层或更下层写了 `disabled = true`）。
    Disabled,
}

/// 一条规则的视图。
#[derive(Debug, Clone, Serialize)]
pub struct RuleView {
    pub process: String,
    pub state: RuleState,
    /// 叠加后的生效内容（运行时真正看到的；对象，只含非默认字段；不含 `disabled`）。
    pub effective: Value,
    /// 系统层的原貌；用户新增的规则为 `None`。
    pub system: Option<Value>,
    /// 用户层真正起作用的差异字段名（含 `unset` 里的）。用户新增的规则 = 它的全部字段。
    pub overridden: Vec<String>,
    /// 用户层里是否有这一条（`state = system` 时也可能为 true：全是冗余差异的旧文件）。
    pub has_user_entry: bool,
    /// 用户层里这一条的原始内容（含 `disabled` / `unset`）；没有则 `None`。
    /// 禁用状态下用它才能看出「禁用前用户改过哪些字段」。
    pub user: Option<Value>,
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

/// 一行经结构体往返后的样子（运行时真正看到的；省略默认值；不含 `unset`）。
fn typed_obj<T: DeserializeOwned + Serialize>(row: &Obj) -> Obj {
    let mut r = row.clone();
    r.remove("unset");
    match serde_json::from_value::<T>(Value::Object(r.clone())) {
        Ok(t) => match serde_json::to_value(&t) {
            Ok(Value::Object(o)) => o,
            _ => r,
        },
        Err(_) => r,
    }
}

/// 补丁里一个键的静态校验：不许碰元键、必须是登记过的字段、JSON 类型要与控件类型相符。
///
/// 只靠反序列化是不够的：容错反序列化会把错类型 / 错取值悄悄吞成「跟随全局」，
/// 键名拼错则被 serde 直接忽略——两者都会让调用方以为写成功了。
fn check_patch_key(section: &str, key: &str, v: &Value) -> Result<(), String> {
    if META_KEYS.contains(&key) {
        return Err(format!("字段 {key} 不能通过补丁修改"));
    }
    let meta = COMPAT_FIELDS
        .iter()
        .find(|f| f.section == section && f.key == key)
        .ok_or_else(|| format!("未知字段 {key}"))?;
    if v.is_null() {
        return Ok(());
    }
    let ok = match meta.kind {
        Kind::Bool | Kind::TriBool => v.is_boolean(),
        Kind::Int => v.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        // 枚举不许空串：空串会被当成「没写」吞掉，想清除请传 null。
        Kind::Enum => v.as_str().is_some_and(|t| !t.trim().is_empty()),
        Kind::Text => v.is_string(),
        Kind::TextList => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("字段 {key} 的值 {v} 类型不对"))
    }
}

/// 值域校验：单独把这个键放进一个最小行里反序列化，看容错反序列化有没有把它回落掉。
///
/// 容错反序列化对值域外的取值（`first_show_mode = "bogus"`）不报错、只记一条「回落」，
/// 所以必须检查回落记录才能识破。别名（`en` / `zh`）与带空白的文本运行时本就接受，不算无效。
fn probe_value<T: DeserializeOwned>(key: &str, v: &Value) -> Result<(), String> {
    let mut o = Obj::new();
    o.insert("process".into(), Value::String("probe.exe".into()));
    o.insert(key.to_string(), v.clone());
    crate::tolerant_de::clear_fallbacks();
    let parsed = serde_json::from_value::<T>(Value::Object(o));
    let fallbacks = crate::tolerant_de::take_fallbacks();
    parsed.map_err(|e| format!("字段 {key} 的值 {v} 无效: {e}"))?;
    if fallbacks.is_empty() {
        Ok(())
    } else {
        Err(format!("字段 {key} 的值 {v} 无效"))
    }
}

/// 补丁 → 字段编辑。`null` = 清除（回到跟随全局）；非 null 先过静态校验与值域校验。
fn patch_to_edits<T: DeserializeOwned>(
    section: &str,
    patch: &Map<String, Value>,
) -> Result<Vec<FieldEdit>, String> {
    let mut edits = Vec::new();
    for (k, v) in patch {
        check_patch_key(section, k, v)?;
        if v.is_null() {
            edits.push(FieldEdit::Clear(k.clone()));
        } else {
            probe_value::<T>(k, v)?;
            edits.push(FieldEdit::Set(k.clone(), v.clone()));
        }
    }
    Ok(edits)
}

fn view_in<T: DeserializeOwned + Serialize>(
    sec_name: &str,
    sys: &[Obj],
    user: &[Obj],
) -> Vec<RuleView> {
    overlay(sys.to_vec(), user)
        .into_iter()
        .map(|eff| {
            let name = process_of(&eff).to_string();
            let s = sys.iter().find(|r| same_process(process_of(r), &name));
            let u = user.iter().find(|r| same_process(process_of(r), &name));
            // 规范化后还有内容，才算「用户层有真正的差异」。
            let mut normalized = u.cloned();
            let has_diff = normalized
                .as_mut()
                .is_some_and(|row| normalize::<T>(sec_name, s, row, true));
            let state = if is_disabled(&eff) {
                RuleState::Disabled
            } else if s.is_none() {
                RuleState::User
            } else if has_diff {
                RuleState::Modified
            } else {
                RuleState::System
            };
            let mut effective = typed_obj::<T>(&eff);
            effective.remove("disabled");
            let system = s.map(|r| {
                let mut o = typed_obj::<T>(r);
                o.remove("disabled");
                Value::Object(o)
            });
            let mut overridden = BTreeSet::new();
            if has_diff && let Some(row) = &normalized {
                for k in row.keys().filter(|k| !META_KEYS.contains(&k.as_str())) {
                    overridden.insert(k.clone());
                }
                if let Some(items) = row.get("unset").and_then(Value::as_array) {
                    overridden.extend(items.iter().filter_map(Value::as_str).map(str::to_string));
                }
            }
            RuleView {
                process: name,
                state,
                effective: Value::Object(effective),
                system,
                overridden: overridden.into_iter().collect(),
                has_user_entry: u.is_some(),
                user: u.map(|r| Value::Object(r.clone())),
            }
        })
        .collect()
}

/// 系统层（`data` + `data_custom` 叠加）与用户层的两层快照。
#[derive(Clone)]
pub struct Layers {
    system: Raw,
    user: Raw,
}

fn rows(raw: &Raw, sec: Section) -> &Vec<Obj> {
    match sec {
        Section::Apps => &raw.apps,
        Section::InitialModeScope => &raw.initial_mode_scope,
        Section::CommitNewline => &raw.commit_newline,
    }
}

/// 严格读用户层：不存在 ⇒ 空；读到但语法错 ⇒ `Err`（带路径与行列号）。
///
/// 运行时加载（`AppCompat::load`）对语法错是整份跳过的；管理界面的写操作不能这样——
/// 静默按空集重写会抹掉用户已有的全部覆盖。
fn read_user(path: &Path) -> Result<Raw, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_raw(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Raw::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Layers {
    pub fn load(
        data_dir: Option<&Path>,
        custom_dir: Option<&Path>,
        user_dir: Option<&Path>,
    ) -> Result<Self, String> {
        let mut system = Raw::default();
        for dir in [data_dir, custom_dir].into_iter().flatten() {
            if let Some(layer) = load_raw(&dir.join(COMPAT_FILE_NAME)) {
                system = overlay_raw(system, &layer);
            }
        }
        let user = match user_dir {
            Some(u) => read_user(&u.join(COMPAT_FILE_NAME))?,
            None => Raw::default(),
        };
        Ok(Self { system, user })
    }

    pub fn view(&self, sec: Section) -> Vec<RuleView> {
        with_section!(sec, T => view_in::<T>(sec.as_str(), rows(&self.system, sec), rows(&self.user, sec)))
    }

    /// 单条规则的视图（进程名不区分大小写）。
    pub fn view_of(&self, sec: Section, process: &str) -> Option<RuleView> {
        self.view(sec)
            .into_iter()
            .find(|v| same_process(&v.process, process))
    }

    fn exists(&self, sec: Section, process: &str) -> bool {
        rows(&self.system, sec)
            .iter()
            .chain(rows(&self.user, sec))
            .any(|r| same_process(process_of(r), process))
    }

    fn apply(&mut self, sec: Section, process: &str, edits: &[FieldEdit]) {
        let (sys, user) = match sec {
            Section::Apps => (&self.system.apps, &mut self.user.apps),
            Section::InitialModeScope => (
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
            ),
            Section::CommitNewline => (&self.system.commit_newline, &mut self.user.commit_newline),
        };
        with_section!(sec, T => apply_edits::<T>(sec.as_str(), sys, user, process, edits, true));
    }

    /// 按字段补丁写入：只写补丁里的字段（`null` = 清除），其余字段照旧继承系统层。
    pub fn upsert(
        &mut self,
        sec: Section,
        process: &str,
        patch: &Map<String, Value>,
    ) -> Result<(), String> {
        let process = validate_process(process)?;
        let edits = with_section!(sec, T => patch_to_edits::<T>(sec.as_str(), patch))?;
        self.apply(sec, &process, &edits);
        Ok(())
    }

    /// 还原单个字段：删掉用户层关于它的一切痕迹（值与 `unset`），继承系统层。
    pub fn reset_field(&mut self, sec: Section, process: &str, key: &str) -> Result<(), String> {
        if META_KEYS.contains(&key) {
            return Err(format!("字段 {key} 不可还原"));
        }
        let process = validate_process(process)?;
        if !self.exists(sec, &process) {
            return Err("无此规则".into());
        }
        self.apply(sec, &process, &[FieldEdit::Inherit(key.to_string())]);
        Ok(())
    }

    /// 禁用 / 启用。启用 = 去掉用户层的禁用差异；系统层自己就禁用的规则则写显式 `disabled = false`。
    pub fn set_disabled(&mut self, sec: Section, process: &str, flag: bool) -> Result<(), String> {
        let process = validate_process(process)?;
        if !self.exists(sec, &process) {
            return Err("无此规则".into());
        }
        self.apply(
            sec,
            &process,
            &[FieldEdit::Set("disabled".into(), Value::Bool(flag))],
        );
        Ok(())
    }

    /// 还原单条：删掉用户层同名条目。用户新增的规则等于删除。返回是否删除过。
    pub fn reset(&mut self, sec: Section, process: &str) -> bool {
        let list = match sec {
            Section::Apps => &mut self.user.apps,
            Section::InitialModeScope => &mut self.user.initial_mode_scope,
            Section::CommitNewline => &mut self.user.commit_newline,
        };
        let before = list.len();
        list.retain(|r| !same_process(process_of(r), process));
        list.len() != before
    }

    /// 清空用户层三段。
    pub fn reset_all(&mut self) {
        self.user = Raw::default();
    }

    /// 用户层渲染成文件全文（含固定文件头）。三段一并渲染，防止整份重写时漏段。
    pub fn render_user(&self) -> String {
        render_raw(USER_COMPAT_HEADER, &self.user)
    }

    /// 落盘用户层：唯一临时名 → 写满 → fsync → rename（见 [`write_atomic`]）。
    /// 调用方须在读到写完的全程持有 [`crate::app_compat::lock_user_compat`]。
    pub fn save(&self, user_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(user_dir)?;
        write_atomic(&user_dir.join(COMPAT_FILE_NAME), &self.render_user())
    }
}

// ───────────────────────── 导出 / 导入 ─────────────────────────

/// 导出范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// 仅用户层（我的差异）。方便分享，也是默认。
    User,
    /// 系统层与用户层叠加后的全部生效规则（剔除已禁用的）。
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
    /// 按字段叠加到用户层同名规则上，用户层里没被提到的条目和字段保留。
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
    /// `add` / `override` / `disable` / `unchanged`；`remove` 是 replace 模式下会被清掉的
    /// 用户条目（导入文本里没提到它）。
    pub action: &'static str,
}

/// 导入预览：会发生什么，以及导入文本里有哪些会被忽略 / 回落 / 拒绝的内容。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub items: Vec<ImportItem>,
    /// 不认识的键（`<段>.<键>`）或整段。它们不会生效。
    pub ignored_keys: Vec<String>,
    /// 值写错、被丢弃的原值。
    pub fallbacks: Vec<String>,
    /// 被整条拒绝、不会导入的规则及原因（进程名非法等）。
    pub rejected: Vec<String>,
}

/// 导入文本里的一条规则（已剔除不认识的键与无效取值）。
struct IncomingRule {
    section: Section,
    process: String,
    fields: Obj,
}

struct Incoming {
    rules: Vec<IncomingRule>,
    ignored: Vec<String>,
    fallbacks: Vec<String>,
    rejected: Vec<String>,
}

/// 严格解析导入文本：语法错 ⇒ `Err`（带行号）。同时收集不认识的键、无效取值与被拒绝的规则。
///
/// 导入的每个键都要过与补丁同样的值域校验，但**不整条拒绝**：无效取值只丢这一个键并报告，
/// 因为分享的片段里混进一个写错的值很常见。否则写错的值会原样落进用户层文件。
fn parse_incoming(text: &str) -> Result<Incoming, String> {
    let raw = parse_raw(text).map_err(|e| format!("导入内容不是合法的 TOML: {e}"))?;
    let mut inc = Incoming {
        rules: Vec::new(),
        ignored: Vec::new(),
        fallbacks: Vec::new(),
        rejected: Vec::new(),
    };
    // 顶层不认识的整段也要报告（parse_raw 只认三段，别的被跳过）。
    if let Ok(root) = toml::from_str::<toml::Table>(text) {
        for name in root.keys() {
            if Section::parse(name).is_none() {
                inc.ignored.push(name.clone());
            }
        }
    }
    for sec in ALL_SECTIONS {
        let known = known_keys(sec.as_str());
        for row in rows(&raw, sec) {
            let process = match row.get("process").and_then(Value::as_str) {
                Some(p) => match validate_process(p) {
                    Ok(p) => p,
                    Err(e) => {
                        inc.rejected.push(format!("{}: {p:?} {e}", sec.as_str()));
                        continue;
                    }
                },
                None => {
                    inc.rejected.push(format!("{}: 缺少 process", sec.as_str()));
                    continue;
                }
            };
            let mut fields = Obj::new();
            fields.insert("process".into(), Value::String(process.clone()));
            for (k, v) in row {
                if k == "process" {
                    continue;
                }
                match k.as_str() {
                    "comment" => {
                        if v.is_string() {
                            fields.insert(k.clone(), v.clone());
                        }
                    }
                    "disabled" => {
                        if v.is_boolean() {
                            fields.insert(k.clone(), v.clone());
                        } else {
                            inc.fallbacks.push(v.to_string());
                        }
                    }
                    "unset" => {
                        let mut keep = Vec::new();
                        for item in v.as_array().into_iter().flatten() {
                            match item.as_str() {
                                Some(name) if known.contains(&name) => {
                                    keep.push(Value::String(name.to_string()))
                                }
                                Some(name) => {
                                    inc.ignored.push(format!("{}.unset:{name}", sec.as_str()))
                                }
                                None => inc.fallbacks.push(item.to_string()),
                            }
                        }
                        if !keep.is_empty() {
                            fields.insert(k.clone(), Value::Array(keep));
                        }
                    }
                    _ if !known.contains(&k.as_str()) => {
                        inc.ignored.push(format!("{}.{k}", sec.as_str()));
                    }
                    _ => {
                        let checked = check_patch_key(sec.as_str(), k, v)
                            .and_then(|_| with_section!(sec, T => probe_value::<T>(k, v)));
                        match checked {
                            Ok(()) => {
                                fields.insert(k.clone(), v.clone());
                            }
                            Err(_) => inc.fallbacks.push(v.to_string()),
                        }
                    }
                }
            }
            inc.rules.push(IncomingRule {
                section: sec,
                process,
                fields,
            });
        }
    }
    inc.ignored.sort();
    inc.ignored.dedup();
    Ok(inc)
}

struct ImportPlan {
    after: Layers,
    items: Vec<ImportItem>,
}

impl Layers {
    /// 导出成 `compat.toml` 文本。用户层导出的是差异（可直接放进用户层），生效导出是叠加后的全貌。
    pub fn export(&self, scope: ExportScope) -> String {
        match scope {
            ExportScope::User => self.render_user(),
            ExportScope::Effective => {
                let mut raw = Raw::default();
                for sec in ALL_SECTIONS {
                    let eff: Vec<Obj> =
                        overlay(rows(&self.system, sec).clone(), rows(&self.user, sec))
                            .into_iter()
                            .filter(|r| !is_disabled(r))
                            .map(|mut r| {
                                r.remove("unset");
                                r.remove("disabled");
                                r
                            })
                            .collect();
                    match sec {
                        Section::Apps => raw.apps = eff,
                        Section::InitialModeScope => raw.initial_mode_scope = eff,
                        Section::CommitNewline => raw.commit_newline = eff,
                    }
                }
                render_raw(
                    "# 生效中的全部应用兼容规则（系统层与用户层叠加后的结果）\n\n",
                    &raw,
                )
            }
        }
    }

    /// 把一条导入规则**叠加**到用户层同名差异行上（不是整条替换），再规范化。
    fn import_rule(&mut self, r: &IncomingRule) {
        let (sys, user) = match r.section {
            Section::Apps => (&self.system.apps, &mut self.user.apps),
            Section::InitialModeScope => (
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
            ),
            Section::CommitNewline => (&self.system.commit_newline, &mut self.user.commit_newline),
        };
        let idx = user
            .iter()
            .position(|u| same_process(process_of(u), &r.process));
        let base = match idx {
            Some(i) => user[i].clone(),
            None => {
                let mut o = Obj::new();
                let name = sys
                    .iter()
                    .find(|s| same_process(process_of(s), &r.process))
                    .map(process_of)
                    .unwrap_or(&r.process);
                o.insert("process".into(), Value::String(name.to_string()));
                o
            }
        };
        let mut row = compose(&base, &r.fields);
        let sys_row = sys.iter().find(|s| same_process(process_of(s), &r.process));
        let keep = with_section!(r.section, T => normalize::<T>(r.section.as_str(), sys_row, &mut row, true));
        match (idx, keep) {
            (Some(i), true) => user[i] = row,
            (Some(i), false) => {
                user.remove(i);
            }
            (None, true) => user.push(row),
            (None, false) => {}
        }
    }

    /// 在副本上试算导入结果。不改 `self`。
    fn plan_import(&self, incoming: &Incoming, mode: ImportMode) -> ImportPlan {
        let mut after = self.clone();
        if mode == ImportMode::Replace {
            after.reset_all();
        }
        let mut items = Vec::new();
        for r in &incoming.rules {
            after.import_rule(r);
            let disabled = r
                .fields
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let before = self.view_of(r.section, &r.process);
            let now = after.view_of(r.section, &r.process);
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
                section: r.section.as_str(),
                process: r.process.clone(),
                action,
            });
        }
        if mode == ImportMode::Replace {
            for sec in ALL_SECTIONS {
                for v in self.view(sec).into_iter().filter(|v| v.has_user_entry) {
                    let mentioned = incoming
                        .rules
                        .iter()
                        .any(|r| r.section == sec && same_process(&r.process, &v.process));
                    if !mentioned {
                        items.push(ImportItem {
                            section: sec.as_str(),
                            process: v.process,
                            action: "remove",
                        });
                    }
                }
            }
        }
        ImportPlan { after, items }
    }

    /// 导入预览：不落盘，不改 `self`。
    pub fn import_preview(&self, text: &str, mode: ImportMode) -> Result<ImportPreview, String> {
        let incoming = parse_incoming(text)?;
        let plan = self.plan_import(&incoming, mode);
        Ok(ImportPreview {
            items: plan.items,
            ignored_keys: incoming.ignored,
            fallbacks: incoming.fallbacks,
            rejected: incoming.rejected,
        })
    }

    /// 应用导入到 `self`（调用方随后负责 `save`）。
    pub fn import_apply(&mut self, text: &str, mode: ImportMode) -> Result<ImportPreview, String> {
        let incoming = parse_incoming(text)?;
        let plan = self.plan_import(&incoming, mode);
        *self = plan.after;
        Ok(ImportPreview {
            items: plan.items,
            ignored_keys: incoming.ignored,
            fallbacks: incoming.fallbacks,
            rejected: incoming.rejected,
        })
    }
}

// ───────────────────────── RPC 分派 ─────────────────────────

/// 一次 RPC 的结果。`wrote` 告诉宿主是否需要重载规则表。
pub struct RpcOutcome {
    pub value: Value,
    pub wrote: bool,
}

/// 最多保留几份备份。
const BACKUP_KEEP: usize = 5;

/// 备份文件名的排序键 `(unix 秒, 同秒序号)`；不是备份文件返回 `None`。
fn backup_key(name: &str) -> Option<(u64, u32)> {
    let rest = name.strip_prefix(&format!("{COMPAT_FILE_NAME}.bak-"))?;
    let mut it = rest.splitn(2, '-');
    let secs = it.next()?.parse().ok()?;
    let n = match it.next() {
        Some(t) => t.parse().ok()?,
        None => 0,
    };
    Some((secs, n))
}

fn list_backups(user_dir: &Path) -> Vec<(String, (u64, u32))> {
    let Ok(rd) = std::fs::read_dir(user_dir) else {
        return Vec::new();
    };
    let mut v: Vec<_> = rd
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| backup_key(&n).map(|k| (n, k)))
        .collect();
    v.sort_by_key(|(_, k)| *k);
    v
}

/// 备份用户层文件为 `compat.toml.bak-<unix 秒>[-<序号>]`（不存在则什么也不做），只留最近几份。
/// 返回备份的文件名，供界面告诉用户「原文件在哪」。
///
/// 不能只留一份：连续两次导入后，最初的原件就没了，而「导入前的样子」恰恰是用户最可能想找回的。
///
/// 新备份的排序键必须**严格大于**现存的所有备份：曾经按「名字被占了就序号加一」取名，
/// 清理腾出低序号后新备份会复用它、排成最旧，下一次清理就把它自己删掉了。
fn backup_user_file(user_dir: &Path) -> Result<Option<String>, String> {
    let path = user_dir.join(COMPAT_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 时钟回拨时也不能排到旧备份前面：以现存最新的秒数为下限。
    let key = match list_backups(user_dir).last() {
        Some((_, (newest_secs, n))) if *newest_secs >= secs => (*newest_secs, n + 1),
        _ => (secs, 0),
    };
    let name = if key.1 == 0 {
        format!("{COMPAT_FILE_NAME}.bak-{}", key.0)
    } else {
        format!("{COMPAT_FILE_NAME}.bak-{}-{}", key.0, key.1)
    };
    std::fs::copy(&path, user_dir.join(&name))
        .map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
    let all = list_backups(user_dir);
    for (old, _) in all.iter().take(all.len().saturating_sub(BACKUP_KEEP)) {
        let _ = std::fs::remove_file(user_dir.join(old));
    }
    Ok(Some(name))
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

    // 导入的参数先解析：它决定要不要持锁、以及用户层损坏时能不能继续。
    let import_args = if method == "compat.import" {
        let content = text_param("content")?;
        let mode = match params.get("mode").and_then(Value::as_str) {
            None => ImportMode::Merge,
            Some(s) => ImportMode::parse(s).ok_or_else(|| "mode 无效".to_string())?,
        };
        let dry_run = match params.get("dryRun") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("dryRun 必须是布尔值".into()),
        };
        Some((content, mode, dry_run))
    } else {
        None
    };

    // 写方法在「读 → 改 → 写」全程持锁：右键菜单（协调器线程）与各个设置端连接线程都会写
    // 用户层，交错时后写的会把先写的整份覆盖掉。
    let writes = match method {
        "compat.upsert" | "compat.resetField" | "compat.setDisabled" | "compat.reset"
        | "compat.resetAll" => true,
        "compat.import" => import_args.is_some_and(|(_, _, dry)| !dry),
        _ => false,
    };
    let _guard = writes.then(crate::app_compat::lock_user_compat);

    // 「全部还原」与 replace 导入本来就要丢弃用户层现有内容，不该被一份读不出来的旧文件拦住——
    // 那正是用户最需要这两个按钮的时候（原文件会先备份）。其它方法要保留现有内容，只能报错。
    let recoverable = method == "compat.resetAll"
        || import_args.is_some_and(|(_, mode, _)| mode == ImportMode::Replace);

    // 先 clear 再加载再 take：`list` 要把「加载时被回落的值」作为告警带回去。
    crate::tolerant_de::clear_fallbacks();
    let mut layers = match Layers::load(data_dir, custom_dir, user_dir) {
        Ok(l) => l,
        Err(e) if recoverable => {
            tracing::warn!("用户层 compat.toml 无法解析，本次操作将整份替换（原文件先备份）: {e}");
            Layers::load(data_dir, custom_dir, None)?
        }
        Err(e) => return Err(e),
    };
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
            let content = layers.export(scope);
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
            // 什么也没删就别写盘、别让宿主白白重载并广播一次。
            if removed {
                layers.save(dir).map_err(|e| e.to_string())?;
            }
            Ok(RpcOutcome {
                value: json!({ "ok": true, "removed": removed, "rule": layers.view_of(sec, process) }),
                wrote: removed,
            })
        }
        "compat.resetAll" => {
            let dir = need_user_dir()?;
            let backup = backup_user_file(dir)?;
            layers.reset_all();
            layers.save(dir).map_err(|e| e.to_string())?;
            wrote(json!({ "ok": true, "backup": backup }))
        }
        "compat.import" => {
            let (content, mode, dry_run) =
                import_args.ok_or_else(|| "内部错误：缺少导入参数".to_string())?;
            if dry_run {
                let preview = layers.import_preview(content, mode)?;
                let mut v = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
                v["applied"] = json!(false);
                return read(v);
            }
            let dir = need_user_dir()?;
            let preview = layers.import_apply(content, mode)?;
            let backup = backup_user_file(dir)?;
            layers.save(dir).map_err(|e| e.to_string())?;
            let mut v = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
            v["applied"] = json!(true);
            v["backup"] = json!(backup);
            wrote(v)
        }
        other => Err(format!("unknown method: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat_overlay::parse_raw;
    use serde_json::json;

    fn layers(sys: &str, usr: &str) -> Layers {
        Layers {
            system: parse_raw(sys).expect("测试夹具必须是合法 TOML"),
            user: parse_raw(usr).expect("测试夹具必须是合法 TOML"),
        }
    }

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("补丁必须是对象")
    }

    fn state_of(l: &Layers, p: &str) -> RuleState {
        l.view_of(Section::Apps, p).expect("规则应存在").state
    }

    fn user_row(l: &Layers, sec: Section, p: &str) -> Option<Obj> {
        rows(&l.user, sec)
            .iter()
            .find(|r| same_process(process_of(r), p))
            .cloned()
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wind_compat_adm_{tag}_{}", std::process::id()))
    }

    const SYS: &str = "\
[[apps]]
process = \"Feishu.exe\"
composition_start_pair_guard = true

[[apps]]
process = \"Weixin.exe\"
caret_use_top = true
stale_probe_guard = true

[[apps]]
process = \"Dota.exe\"
host_render = true
";

    const USER_A: &str = "[[apps]]\nprocess = \"Weixin.exe\"\nauto_pair = true\n";

    // ───────────── 视图 ─────────────

    #[test]
    fn view_marks_system_modified_user_disabled() {
        let usr = "\
[[apps]]
process = \"Weixin.exe\"
first_show_mode = \"wait\"

[[apps]]
process = \"Dota.exe\"
disabled = true

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
    fn overridden_lists_only_the_meaningful_diff_including_unset() {
        let usr = "[[apps]]\nprocess = \"Weixin.exe\"\nauto_pair = true\ncaret_use_top = true\nunset = [\"stale_probe_guard\"]\n";
        let l = layers(SYS, usr);
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(
            v.overridden,
            vec!["auto_pair".to_string(), "stale_probe_guard".to_string()],
            "caret_use_top 与系统一致是冗余，不算差异；unset 的字段算差异"
        );
        assert!(v.system.is_some());
        assert!(
            v.effective.get("stale_probe_guard").is_none(),
            "unset 后运行时没有它"
        );
        assert_eq!(v.effective["caret_use_top"], json!(true), "没提到的继承");
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

    /// 旧文件里与系统层完全一致的冗余条目：状态是 System，但用户层里确实有这一条（还原按钮据此判断）。
    #[test]
    fn a_redundant_user_entry_is_reported_as_system_but_still_has_a_user_entry() {
        let l = layers(
            SYS,
            "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\n",
        );
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::System);
        assert!(v.has_user_entry);
        assert!(v.overridden.is_empty());
    }

    #[test]
    fn view_exposes_whether_a_user_entry_exists_and_its_raw_content() {
        let mut l = layers(SYS, "");
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert!(!v.has_user_entry && v.user.is_none());
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        l.set_disabled(Section::Apps, "Weixin.exe", true).unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Disabled);
        let user = v.user.expect("禁用状态下要能看到用户层原始条目");
        assert_eq!(user["auto_pair"], json!(true), "禁用前的改写要可见");
        assert_eq!(user["disabled"], json!(true));
    }

    // ───────────── 差异写入 ─────────────

    /// 整体原则的核心：改一个字段只写这一个字段，系统规则的其余字段继续继承。
    #[test]
    fn patch_on_a_system_rule_stores_only_the_diff() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "weixin.exe",
            &patch(json!({"first_show_mode": "wait"})),
        )
        .unwrap();
        let row = user_row(&l, Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(row["first_show_mode"], json!("wait"));
        assert!(
            row.get("caret_use_top").is_none() && row.get("stale_probe_guard").is_none(),
            "不得拷贝系统字段: {row:?}"
        );
        assert_eq!(row["process"], json!("Weixin.exe"), "沿用系统层的写法");
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified);
        assert_eq!(
            v.effective["caret_use_top"],
            json!(true),
            "系统字段照旧生效"
        );
        assert_eq!(v.effective["first_show_mode"], json!("wait"));
    }

    #[test]
    fn patch_equal_to_the_system_value_removes_the_entry() {
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
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe").is_none(),
            "回到与系统一致，条目应消失"
        );
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"caret_use_top": true})),
        )
        .unwrap();
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe").is_none(),
            "写成与系统相同的值也是冗余"
        );
    }

    /// `null` = 清除 = 回到「跟随全局」：系统层设了它就要写 unset，否则只会继承回系统值。
    #[test]
    fn null_on_a_system_field_writes_unset_and_reset_field_undoes_it() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Feishu.exe",
            &patch(json!({"composition_start_pair_guard": null})),
        )
        .unwrap();
        let row = user_row(&l, Section::Apps, "Feishu.exe").unwrap();
        assert_eq!(row["unset"], json!(["composition_start_pair_guard"]));
        let v = l.view_of(Section::Apps, "Feishu.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified, "unset 是真正的差异");
        assert!(v.effective.get("composition_start_pair_guard").is_none());

        l.reset_field(Section::Apps, "Feishu.exe", "composition_start_pair_guard")
            .unwrap();
        assert!(
            user_row(&l, Section::Apps, "Feishu.exe").is_none(),
            "还原 = 继承出厂，痕迹全清"
        );
        assert_eq!(state_of(&l, "Feishu.exe"), RuleState::System);
    }

    #[test]
    fn null_on_a_field_the_system_lacks_leaves_no_trace() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": null})),
        )
        .unwrap();
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe").is_none(),
            "空操作不该留噪声条目"
        );
    }

    #[test]
    fn explicit_false_over_a_system_true_is_kept() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Feishu.exe",
            &patch(json!({"composition_start_pair_guard": false})),
        )
        .unwrap();
        assert_eq!(
            user_row(&l, Section::Apps, "Feishu.exe").unwrap()["composition_start_pair_guard"],
            json!(false)
        );
        let v = l.view_of(Section::Apps, "Feishu.exe").unwrap();
        assert_eq!(
            v.effective["composition_start_pair_guard"],
            json!(false),
            "显式关闭必须生效"
        );
        assert_eq!(v.state, RuleState::Modified);
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
    fn patch_rejects_typos_wrong_types_and_empty_enum() {
        let bad = [
            json!({"caret_use_tpo": false}),
            json!({"auto_pair": 0}),
            json!({"caret_use_top": "yes"}),
            json!({"first_show_mode": ""}),
            json!({"first_show_mode": 0}),
            json!({"caret_offset_x": "12"}),
        ];
        for p in bad {
            let mut l = layers(SYS, "");
            assert!(
                l.upsert(Section::Apps, "Weixin.exe", &patch(p.clone()))
                    .is_err(),
                "补丁 {p} 应被拒绝"
            );
            assert!(l.user.apps.is_empty(), "被拒绝的补丁不得落任何东西: {p}");
        }
    }

    #[test]
    fn patch_rejects_meta_keys() {
        for p in [
            json!({"disabled": true}),
            json!({"comment": "x"}),
            json!({"process": "O.exe"}),
            json!({"unset": ["auto_pair"]}),
        ] {
            let mut l = layers(SYS, "");
            assert!(
                l.upsert(Section::Apps, "Weixin.exe", &patch(p.clone()))
                    .is_err(),
                "{p}"
            );
        }
    }

    #[test]
    fn false_over_nothing_is_redundant_but_accepted() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"auto_pair": true, "caret_use_top": false})),
        )
        .expect("bool false 是合法值");
        let row = user_row(&l, Section::Apps, "Mine.exe").unwrap();
        assert!(
            row.get("caret_use_top").is_none(),
            "系统没有它时 false 与没写等价，属冗余: {row:?}"
        );
        assert_eq!(row["auto_pair"], json!(true));
    }

    #[test]
    fn process_validation_and_matching() {
        assert!(validate_process("").is_err());
        assert!(validate_process("   ").is_err());
        assert!(validate_process("a/b.exe").is_err());
        assert!(validate_process("a\\b.exe").is_err());
        assert!(validate_process("a\u{7}b.exe").is_err());
        assert_eq!(validate_process(" x.exe ").unwrap(), "x.exe");
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "  weixin.EXE ",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        assert_eq!(l.user.apps.len(), 1);
        assert_eq!(
            process_of(&l.user.apps[0]),
            "Weixin.exe",
            "沿用系统层的写法，不新增一条"
        );
    }

    /// 窗口类只改大小写或顺序，运行时等价，不算差异。
    #[test]
    fn classes_differing_only_in_case_or_order_are_not_a_modification() {
        let sys = "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"Progman\", \"WorkerW\"]\n";
        let mut l = layers(sys, "");
        l.upsert(
            Section::InitialModeScope,
            "explorer.exe",
            &patch(json!({"classes": ["workerw", "progman"]})),
        )
        .unwrap();
        assert!(
            l.user.initial_mode_scope.is_empty(),
            "{:?}",
            l.user.initial_mode_scope
        );
    }

    /// 元数据里登记的每个枚举可选值都必须被真实解析器接受并原样往返。
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

    // ───────────── 还原 / 禁用 ─────────────

    #[test]
    fn reset_field_inherits_the_system_value_and_keeps_other_edits() {
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
    fn reset_removes_the_user_entry_and_reports_whether_it_existed() {
        let mut l = layers(SYS, USER_A);
        assert!(l.reset(Section::Apps, "weixin.exe"));
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
        assert!(!l.reset(Section::Apps, "Weixin.exe"));
        let mut u = layers(SYS, "[[apps]]\nprocess = \" x.exe \"\nauto_pair = true\n");
        assert!(u.reset(Section::Apps, "x.exe"), "已存的名字带空白也要能删");
    }

    #[test]
    fn reset_all_clears_all_three_sections() {
        let usr = "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\n\n[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"X\"]\n\n[[commit_newline]]\nprocess = \"W.exe\"\nstyle = \"cr\"\n";
        let mut l = layers(SYS, usr);
        l.reset_all();
        let text = l.render_user();
        assert!(!text.contains("[["), "{text}");
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
        assert_eq!(
            user_row(&l, Section::Apps, "Weixin.exe").unwrap()["auto_pair"],
            json!(true),
            "禁用期间改写还在"
        );
        l.set_disabled(Section::Apps, "Weixin.exe", false).unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified, "启用后回到禁用前的改写状态");
        assert_eq!(v.effective["auto_pair"], json!(true));
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe")
                .unwrap()
                .get("disabled")
                .is_none()
        );
    }

    #[test]
    fn enabling_an_untouched_disabled_rule_removes_the_entry() {
        let mut l = layers(SYS, "");
        l.set_disabled(Section::Apps, "Weixin.exe", true).unwrap();
        l.set_disabled(Section::Apps, "Weixin.exe", false).unwrap();
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe").is_none(),
            "与系统一致就不该留条目"
        );
        assert_eq!(state_of(&l, "Weixin.exe"), RuleState::System);
    }

    #[test]
    fn set_disabled_on_unknown_process_errors_and_user_only_rules_are_allowed() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\n");
        assert_eq!(
            l.set_disabled(Section::Apps, "Nope.exe", true).unwrap_err(),
            "无此规则"
        );
        l.set_disabled(Section::Apps, "Mine.exe", true).unwrap();
        assert_eq!(state_of(&l, "Mine.exe"), RuleState::Disabled);
        l.set_disabled(Section::Apps, "Mine.exe", false).unwrap();
        assert_eq!(state_of(&l, "Mine.exe"), RuleState::User);
    }

    // ───────────── 段之间 ─────────────

    #[test]
    fn sections_are_independent() {
        let sys = "[[apps]]\nprocess = \"explorer.exe\"\nauto_pair = true\n\n[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"CabinetWClass\"]\n";
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
        let sys = "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"CabinetWClass\"]\n\n[[commit_newline]]\nprocess = \"WINWORD.EXE\"\nstyle = \"cr\"\n";
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

    // ───────────── 读写与文件 ─────────────

    #[test]
    fn strict_load_reports_syntax_error_with_path() {
        let dir = tmp("syntax");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(COMPAT_FILE_NAME), "[[apps\nprocess=").unwrap();
        let err = match Layers::load(None, None, Some(&dir)) {
            Err(e) => e,
            Ok(_) => panic!("语法错必须报错，而不是静默按空集处理"),
        };
        assert!(
            err.contains(COMPAT_FILE_NAME) && err.contains("line"),
            "应带路径与行号: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_user_file_is_an_empty_layer() {
        assert!(
            Layers::load(None, None, Some(&tmp("missing")))
                .unwrap()
                .user
                .apps
                .is_empty()
        );
    }

    #[test]
    fn save_is_atomic_and_preserves_other_sections_and_unknown_keys() {
        let dir = tmp("save");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(COMPAT_FILE_NAME),
            "[[commit_newline]]\nprocess = \"WINWORD.EXE\"\nstyle = \"cr\"\n\n[[apps]]\nprocess = \"Old.exe\"\nfuture_key = 3\nauto_pair = true\n",
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
        for needle in [
            "[[commit_newline]]",
            "Mine.exe",
            "Old.exe",
            "future_key = 3",
        ] {
            assert!(text.contains(needle), "写回后丢了 {needle}: {text}");
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ───────────── 导出 / 导入 ─────────────

    #[test]
    fn export_user_contains_only_the_diff() {
        let l = layers(SYS, USER_A);
        let text = l.export(ExportScope::User);
        assert!(text.contains("auto_pair = true"), "{text}");
        assert!(
            !text.contains("caret_use_top") && !text.contains("Feishu"),
            "用户导出只含差异: {text}"
        );
    }

    #[test]
    fn export_effective_expands_the_overlay_and_drops_disabled_and_syntax() {
        let usr = format!(
            "{USER_A}\n[[apps]]\nprocess = \"Dota.exe\"\ndisabled = true\n\n[[apps]]\nprocess = \"Feishu.exe\"\nunset = [\"composition_start_pair_guard\"]\n"
        );
        let text = layers(SYS, &usr).export(ExportScope::Effective);
        assert!(
            text.contains("caret_use_top = true") && text.contains("auto_pair = true"),
            "叠加后的全貌: {text}"
        );
        assert!(!text.contains("Dota"), "被禁用的不导出: {text}");
        assert!(
            !text.contains("disabled") && !text.contains("unset"),
            "叠加语法不该出现在生效导出里: {text}"
        );
        assert!(
            !text.contains("composition_start_pair_guard"),
            "unset 掉的字段不该出现: {text}"
        );
    }

    #[test]
    fn export_then_import_replace_roundtrips() {
        let usr = format!(
            "{USER_A}\n[[apps]]\nprocess = \"Dota.exe\"\ndisabled = true\n\n[[apps]]\nprocess = \"Feishu.exe\"\nunset = [\"composition_start_pair_guard\"]\n"
        );
        let a = layers(SYS, &usr);
        let text = a.export(ExportScope::User);
        let mut b = layers(SYS, "");
        b.import_apply(&text, ImportMode::Replace).unwrap();
        for p in ["Feishu.exe", "Weixin.exe", "Dota.exe"] {
            let (va, vb) = (
                a.view_of(Section::Apps, p).unwrap(),
                b.view_of(Section::Apps, p).unwrap(),
            );
            assert_eq!((va.state, &va.effective), (vb.state, &vb.effective), "{p}");
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
auto_pair = true

[[apps]]
process = \"Dota.exe\"
disabled = true

[[apps]]
process = \"Feishu.exe\"
composition_start_pair_guard = true
";
        let p = layers(SYS, "")
            .import_preview(incoming, ImportMode::Merge)
            .unwrap();
        let act = |n: &str| p.items.iter().find(|i| i.process == n).map(|i| i.action);
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

    /// 导入按字段叠加：稀疏片段不得让系统规则里的其它字段丢失，显式 false 能覆盖出厂的 true。
    #[test]
    fn import_sparse_snippet_keeps_the_system_fields_and_explicit_false_overrides() {
        let mut l = layers(SYS, "");
        l.import_apply(
            "[[apps]]\nprocess = \"weixin.exe\"\nfirst_show_mode = \"wait\"\n",
            ImportMode::Merge,
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.effective["caret_use_top"], json!(true), "出厂修复不能丢");
        assert_eq!(v.effective["stale_probe_guard"], json!(true));
        assert_eq!(v.effective["first_show_mode"], json!("wait"));
        l.import_apply(
            "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = false\n",
            ImportMode::Merge,
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert!(
            v.effective.get("caret_use_top").is_none(),
            "显式 false 应生效: {:?}",
            v.effective
        );
        assert_eq!(
            v.effective["first_show_mode"],
            json!("wait"),
            "之前导入的字段仍在（叠加而非替换）"
        );
    }

    #[test]
    fn import_understands_unset() {
        let mut l = layers(SYS, "");
        l.import_apply(
            "[[apps]]\nprocess = \"Weixin.exe\"\nunset = [\"caret_use_top\"]\n",
            ImportMode::Merge,
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert!(v.effective.get("caret_use_top").is_none());
        assert_eq!(v.state, RuleState::Modified);
    }

    #[test]
    fn import_merge_keeps_unmentioned_user_entries_and_replace_drops_them() {
        let mut m = layers(SYS, "[[apps]]\nprocess = \"E.exe\"\nauto_pair = true\n");
        m.import_apply(
            "[[apps]]\nprocess = \"C.exe\"\nauto_pair = true\n",
            ImportMode::Merge,
        )
        .unwrap();
        assert!(
            m.view_of(Section::Apps, "E.exe").is_some()
                && m.view_of(Section::Apps, "C.exe").is_some()
        );
        let mut r = layers(SYS, "[[apps]]\nprocess = \"E.exe\"\nauto_pair = true\n");
        r.import_apply(
            "[[apps]]\nprocess = \"C.exe\"\nauto_pair = true\n",
            ImportMode::Replace,
        )
        .unwrap();
        assert!(
            r.view_of(Section::Apps, "E.exe").is_none(),
            "Replace 要先清空用户层"
        );
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

    /// 无效取值只丢这一个键并报告，**且不能原样落进用户层文件**。
    #[test]
    fn import_drops_invalid_values_reports_them_and_never_persists_them() {
        let text = "[[apps]]\nprocess = \"A.exe\"\nfirst_show_mode = \"bogus\"\nauto_pair = true\n";
        let mut l = layers(SYS, "");
        let p = l.import_apply(text, ImportMode::Merge).unwrap();
        assert!(
            p.fallbacks.iter().any(|f| f.contains("bogus")),
            "{:?}",
            p.fallbacks
        );
        let row = user_row(&l, Section::Apps, "A.exe").unwrap();
        assert!(
            row.get("first_show_mode").is_none(),
            "无效值不得落盘: {row:?}"
        );
        assert_eq!(row["auto_pair"], json!(true), "同一条里的有效字段照常导入");
    }

    #[test]
    fn import_rejects_invalid_process_names_and_reports_them() {
        let mut l = layers(SYS, "");
        let p = l.import_apply("[[apps]]\nprocess = \" a/b.exe \"\nauto_pair = true\n\n[[apps]]\nprocess = \"ok.exe\"\nauto_pair = true\n", ImportMode::Merge).unwrap();
        assert_eq!(p.rejected.len(), 1, "{:?}", p.rejected);
        assert!(l.user.apps.iter().all(|r| !process_of(r).contains('/')));
        assert!(l.view_of(Section::Apps, "ok.exe").is_some());
    }

    #[test]
    fn import_syntax_error_is_an_error_not_silent() {
        let err = layers(SYS, "")
            .import_preview("[[apps\nprocess=", ImportMode::Merge)
            .unwrap_err();
        assert!(err.contains("line"), "应带行号: {err}");
    }

    #[test]
    fn importing_the_same_content_twice_is_idempotent() {
        let text = "[[apps]]\nprocess = \"Weixin.exe\"\nauto_pair = true\n";
        let mut l = layers(SYS, "");
        assert_eq!(
            l.import_apply(text, ImportMode::Merge).unwrap().items[0].action,
            "override"
        );
        assert_eq!(
            l.import_apply(text, ImportMode::Merge).unwrap().items[0].action,
            "unchanged"
        );
    }

    // ───────────── RPC ─────────────

    /// 用户目录里所有 `compat.toml.bak-*` 备份，按名字排序。
    fn backups(dir: &Path) -> Vec<std::path::PathBuf> {
        let prefix = format!("{COMPAT_FILE_NAME}.bak-");
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(&prefix))
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

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
        let found = backups(&u);
        assert_eq!(found.len(), 1, "应恰好留下一份备份: {found:?}");
        assert_eq!(
            std::fs::read_to_string(&found[0]).unwrap(),
            original,
            "备份必须是导入前的原文"
        );
        assert!(
            r.value["backup"].as_str().is_some(),
            "响应里要告诉界面备份文件名: {}",
            r.value
        );
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
        let found = backups(&u);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(std::fs::read_to_string(&found[0]).unwrap(), original);
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

    // ───────────── 独立审查（2026-09-29）提出的回归 ─────────────

    /// data_custom 层禁用的规则：视图与运行时一致（运行时没有它），且可以在用户层重新启用。
    #[test]
    fn rule_disabled_in_custom_layer_is_disabled_in_view_and_at_runtime() {
        let (d, c) = (tmp("cust_d"), tmp("cust_c"));
        for dir in [&d, &c] {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(d.join(COMPAT_FILE_NAME), SYS).unwrap();
        std::fs::write(
            c.join(COMPAT_FILE_NAME),
            "[[apps]]\nprocess = \"Weixin.exe\"\ndisabled = true\n",
        )
        .unwrap();
        let mut l = Layers::load(Some(&d), Some(&c), None).unwrap();
        assert_eq!(
            state_of(&l, "Weixin.exe"),
            RuleState::Disabled,
            "视图必须反映它被禁用"
        );
        assert!(
            crate::app_compat::AppCompat::load_layered(Some(&d), Some(&c), None)
                .get_rule("Weixin.exe")
                .is_none(),
            "运行时确实没有它"
        );
        l.set_disabled(Section::Apps, "Weixin.exe", false).unwrap();
        assert_eq!(
            state_of(&l, "Weixin.exe"),
            RuleState::Modified,
            "用户层显式 disabled = false 重新启用"
        );
        assert!(
            user_row(&l, Section::Apps, "Weixin.exe")
                .unwrap()
                .get("disabled")
                == Some(&json!(false))
        );
        cleanup(&d, &c);
    }

    /// S2 的反方向：运行时接受的写法（别名、带空白）不得被误拒。
    #[test]
    fn patch_accepts_aliases_and_padded_text() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"initial_mode": "en"})),
        )
        .expect("别名 en 是合法写法");
        l.upsert(
            Section::Apps,
            "Mine.exe",
            &patch(json!({"schema": " wubi86 "})),
        )
        .expect("带空白的方案 id 运行时会 trim，应接受");
    }

    /// S4：replace 模式的预览要让用户看到「哪些会被删掉」。
    #[test]
    fn replace_preview_lists_entries_that_will_be_removed() {
        let l = layers(SYS, "[[apps]]\nprocess = \"Mine.exe\"\nauto_pair = true\n");
        let p = l
            .import_preview(
                "[[apps]]\nprocess = \"Other.exe\"\nauto_pair = true\n",
                ImportMode::Replace,
            )
            .unwrap();
        assert!(
            p.items
                .iter()
                .any(|i| i.process == "Mine.exe" && i.action == "remove"),
            "{:?}",
            p.items
        );
    }

    /// S5：用户层语法坏了，界面仍要有恢复手段：resetAll 与 replace 导入不依赖解析用户层。
    #[test]
    fn reset_all_and_replace_import_recover_from_a_broken_user_file() {
        let (d, u) = rpc_dirs("recover");
        std::fs::create_dir_all(&u).unwrap();
        let broken = "[[apps\nprocess=";
        std::fs::write(u.join(COMPAT_FILE_NAME), broken).unwrap();
        let r = call("compat.resetAll", json!({}), &d, &u).expect("resetAll 必须能恢复");
        assert!(r.wrote);
        assert!(
            std::fs::read_dir(&u)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| std::fs::read_to_string(e.path()).ok().as_deref() == Some(broken)),
            "损坏的原文件必须被备份，而不是被抹掉"
        );

        std::fs::write(u.join(COMPAT_FILE_NAME), broken).unwrap();
        let r = call(
            "compat.import",
            json!({"content": "[[apps]]\nprocess = \"New.exe\"\nauto_pair = true\n", "mode": "replace"}),
            &d,
            &u,
        )
        .expect("replace 导入必须能恢复");
        assert!(r.wrote);
        assert!(
            std::fs::read_to_string(u.join(COMPAT_FILE_NAME))
                .unwrap()
                .contains("New.exe")
        );

        // 合并导入不能：它要保留用户层已有内容，而那份内容读不出来。
        std::fs::write(u.join(COMPAT_FILE_NAME), broken).unwrap();
        assert!(
            call(
                "compat.import",
                json!({"content": "[[apps]]\nprocess = \"New.exe\"\nauto_pair = true\n", "mode": "merge"}),
                &d,
                &u
            )
            .is_err()
        );
        cleanup(&d, &u);
    }

    /// M3：并发写不得互相覆盖（读-改-写整段持锁）。
    #[test]
    fn concurrent_writes_do_not_lose_updates() {
        let (d, u) = rpc_dirs("race");
        let (d, u) = (std::sync::Arc::new(d), std::sync::Arc::new(u));
        let handles: Vec<_> = (0..12)
            .map(|i| {
                let (d, u) = (d.clone(), u.clone());
                std::thread::spawn(move || {
                    call(
                        "compat.upsert",
                        json!({"section": "apps", "process": format!("P{i}.exe"), "patch": {"auto_pair": true}}),
                        &d,
                        &u,
                    )
                    .expect("并发写不应报错");
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let r = call("compat.list", json!({"section": "apps"}), &d, &u).unwrap();
        let users = r.value["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["state"] == json!("user"))
            .count();
        assert_eq!(users, 12, "12 次并发写应全部留下，丢了 {} 条", 12 - users);
        cleanup(&d, &u);
    }

    /// 可选 7：`reset` 什么也没删时不该报告写入（否则白白写盘、重载、广播）。
    #[test]
    fn reset_of_nonexistent_user_entry_reports_no_write() {
        let (d, u) = rpc_dirs("noop_reset");
        let r = call(
            "compat.reset",
            json!({"section": "apps", "process": "Weixin.exe"}),
            &d,
            &u,
        )
        .unwrap();
        assert!(!r.wrote);
        assert_eq!(r.value["removed"], json!(false));
        cleanup(&d, &u);
    }

    /// 备份最多留 5 份，且是最新的 5 份：连续多次导入后，「导入前的样子」仍能找回。
    #[test]
    fn backups_are_rotated_and_keep_the_newest() {
        let (d, u) = rpc_dirs("rotate");
        std::fs::create_dir_all(&u).unwrap();
        for i in 0..8 {
            std::fs::write(
                u.join(COMPAT_FILE_NAME),
                format!("[[apps]]\nprocess = \"V{i}.exe\"\nauto_pair = true\n"),
            )
            .unwrap();
            call(
                "compat.import",
                json!({"content": "[[apps]]\nprocess = \"X.exe\"\nauto_pair = true\n", "mode": "replace"}),
                &d,
                &u,
            )
            .unwrap();
        }
        let found = backups(&u);
        assert_eq!(found.len(), BACKUP_KEEP, "{found:?}");
        let newest = std::fs::read_to_string(found.last().unwrap()).unwrap();
        assert!(
            newest.contains("V7.exe"),
            "最新一份应是最后一次导入前的内容: {newest}"
        );
        cleanup(&d, &u);
    }

    /// dryRun 传了非布尔值必须报错，不能悄悄当成 false 而真的写入。
    #[test]
    fn non_boolean_dry_run_is_an_error_not_a_write() {
        let (d, u) = rpc_dirs("dryrun_type");
        let err = call(
            "compat.import",
            json!({"content": "[[apps]]\nprocess = \"N.exe\"\nauto_pair = true\n", "dryRun": "true"}),
            &d,
            &u,
        )
        .err()
        .expect("dryRun 类型不对必须报错");
        assert!(err.contains("dryRun"), "{err}");
        assert!(!u.join(COMPAT_FILE_NAME).exists());
        cleanup(&d, &u);
    }
}
