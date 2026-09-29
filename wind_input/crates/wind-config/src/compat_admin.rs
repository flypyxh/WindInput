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
    write_atomic,
};
use crate::compat_schema::{COMPAT_FIELDS, Kind};
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
    /// 用户层里是否有这一条。`state = system` 时也可能为 true（与系统完全相同的冗余拷贝），
    /// 「还原」按钮据此判断有没有东西可还原。
    pub has_user_entry: bool,
    /// 用户层里这一条的原始内容（含 `disabled`）；没有则 `None`。
    /// 禁用状态下用它才能看出「禁用前用户改过哪些字段」。
    pub user: Option<Value>,
}

/// 三段规则的共同面：泛型写操作只依赖这几个访问器和 serde。
pub(crate) trait Rule: Clone + Default + Serialize + serde::de::DeserializeOwned {
    fn process(&self) -> &str;
    fn set_process(&mut self, p: &str);
    fn disabled(&self) -> bool;
    fn set_disabled(&mut self, v: bool);
    /// 按运行时合并（`merge_rules`）同一套规则从低层继承。只有 `[[apps]]` 有宿主协议级字段，
    /// 其余两段没有可继承的东西，默认什么也不做。
    fn inherit_from(&mut self, _base: &Self) {}
}

macro_rules! impl_rule {
    ($t:ty $(, inherit: |$s:ident, $b:ident| $body:expr)?) => {
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
            $(
                fn inherit_from(&mut self, base: &Self) {
                    let ($s, $b) = (self, base);
                    $body
                }
            )?
        }
    };
}
impl_rule!(AppCompatRule, inherit: |s, b| s.inherit_protocol_from(b));
impl_rule!(InitialModeScopeRule);
impl_rule!(CommitNewlineRule);

/// 不属于「兼容取值」的键：不进 `overridden`，也不允许被补丁 / 还原触碰。
pub(crate) const META_KEYS: [&str; 3] = ["process", "comment", "disabled"];

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
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
        // 窗口类名运行时不区分大小写、也与顺序无关：只改这两样不算「已修改」。
        if let Some(Value::Array(items)) = m.get_mut("classes") {
            let mut v: Vec<String> = items
                .iter()
                .filter_map(|x| x.as_str().map(str::to_ascii_lowercase))
                .collect();
            v.sort();
            v.dedup();
            *items = v.into_iter().map(Value::String).collect();
        }
        m
    };
    strip(a) == strip(b)
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
        Kind::Bool | Kind::TriBool | Kind::InheritBool => v.is_boolean(),
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

/// 把补丁应用到 `base`。`null` 表示清除该字段。
///
/// 逐键试算：容错反序列化对值域外的取值（`first_show_mode = "bogus"`）不报错、只记一条
/// 「回落」，所以每个键都要单独反序列化一次并检查回落记录，才能把它明确拒绝掉。
/// 别名（`en` / `zh`）与带空白的文本运行时本就接受，不算无效。
fn apply_patch_to<T: Rule>(
    section: &str,
    base: &T,
    patch: &Map<String, Value>,
) -> Result<T, String> {
    let mut obj = to_obj(base);
    for (k, v) in patch {
        check_patch_key(section, k, v)?;
        if v.is_null() {
            obj.remove(k);
            continue;
        }
        let mut probe = obj.clone();
        probe.insert(k.clone(), v.clone());
        crate::tolerant_de::clear_fallbacks();
        let parsed = serde_json::from_value::<T>(Value::Object(probe));
        let fallbacks = crate::tolerant_de::take_fallbacks();
        if let Err(e) = parsed {
            return Err(format!("字段 {k} 的值 {v} 无效: {e}"));
        }
        if !fallbacks.is_empty() {
            return Err(format!("字段 {k} 的值 {v} 无效"));
        }
        obj.insert(k.clone(), v.clone());
    }
    serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())
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
        Some(s) => {
            // 运行时看到的是**继承之后**的结果：协议字段置空 = 继承出厂值。只比继承之前的
            // 条目，会把「协议字段置 null」误判成有差异而留下一条隐形空壳，而空壳会把以后
            // 出厂新增的修复整条挡掉。
            let mut effective = entry.clone();
            effective.inherit_from(s);
            !entry.disabled() && same_content(&effective, s)
        }
        None => is_empty(&entry),
    };
    if !redundant {
        user.push(entry);
    }
}

fn upsert_in<T: Rule>(
    section: &str,
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
    let entry = apply_patch_to(section, &base, patch)?;
    commit_entry(system, user, entry);
    Ok(())
}

fn reset_field_in<T: Rule>(
    section: &str,
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
    upsert_in(section, system, user, &process, &patch)
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
                has_user_entry: usr.is_some(),
                user: usr.map(|u| Value::Object(to_obj(u))),
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
        // 系统层（含 data_custom）里被 `disabled` 的规则运行时就是不存在的：`load_layered` 对
        // 合并结果统一过滤。视图必须与它一致，否则列表里显示「生效」、实际根本没有。
        system.apps.retain(|r| !r.disabled);
        system.initial_mode_scope.retain(|r| !r.disabled);
        system.commit_newline.retain(|r| !r.disabled);
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
            Section::Apps => upsert_in(
                sec.as_str(),
                &self.system.apps,
                &mut self.user.apps,
                process,
                patch,
            ),
            Section::InitialModeScope => upsert_in(
                sec.as_str(),
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                process,
                patch,
            ),
            Section::CommitNewline => upsert_in(
                sec.as_str(),
                &self.system.commit_newline,
                &mut self.user.commit_newline,
                process,
                patch,
            ),
        }
    }

    pub fn reset_field(&mut self, sec: Section, process: &str, key: &str) -> Result<(), String> {
        match sec {
            Section::Apps => reset_field_in(
                sec.as_str(),
                &self.system.apps,
                &mut self.user.apps,
                process,
                key,
            ),
            Section::InitialModeScope => reset_field_in(
                sec.as_str(),
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                process,
                key,
            ),
            Section::CommitNewline => reset_field_in(
                sec.as_str(),
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

    /// 落盘用户层：唯一临时名 → 写满 → fsync → rename（见 [`write_atomic`]）。
    /// 调用方须在读到写完的全程持有 [`crate::app_compat::lock_user_compat`]。
    pub fn save(&self, user_dir: &Path) -> std::io::Result<()> {
        let text = self
            .render_user()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::create_dir_all(user_dir)?;
        write_atomic(&user_dir.join(COMPAT_FILE_NAME), &text)
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
    /// 值写错、被容错回落成「跟随全局」的原值。
    pub fallbacks: Vec<String>,
    /// 被整条拒绝、不会导入的规则及原因（进程名非法、字段类型错等）。
    pub rejected: Vec<String>,
}

/// 导入文本里的一条规则：保留**原始键值**，不经容错反序列化。
///
/// 必须保留原始键：`caret_use_top = false` 序列化时会被省略，若只看反序列化后的结构体，
/// 「显式写 false 来覆盖出厂的 true」这层意思就丢了。
struct IncomingRule {
    section: Section,
    process: String,
    fields: Map<String, Value>,
}

struct Incoming {
    rules: Vec<IncomingRule>,
    ignored: Vec<String>,
    rejected: Vec<String>,
}

/// 严格解析导入文本：语法错 ⇒ `Err`（带行号）。同时收集不认识的键与被拒绝的规则。
fn parse_incoming(text: &str) -> Result<Incoming, String> {
    let raw: toml::Value =
        toml::from_str(text).map_err(|e| format!("导入内容不是合法的 TOML: {e}"))?;
    let mut inc = Incoming {
        rules: Vec::new(),
        ignored: Vec::new(),
        rejected: Vec::new(),
    };
    if let Some(top) = raw.as_table() {
        for (name, value) in top {
            let Some(sec) = Section::parse(name) else {
                inc.ignored.push(name.clone());
                continue;
            };
            let known = crate::compat_schema::known_keys(sec.as_str());
            for tbl in value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_table())
            {
                let process = match tbl.get("process").and_then(toml::Value::as_str) {
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
                let mut fields = Map::new();
                for (k, v) in tbl {
                    if k == "process" {
                        continue;
                    }
                    if !META_KEYS.contains(&k.as_str()) && !known.contains(&k.as_str()) {
                        inc.ignored.push(format!("{}.{k}", sec.as_str()));
                        continue;
                    }
                    match serde_json::to_value(v) {
                        Ok(j) => {
                            fields.insert(k.clone(), j);
                        }
                        Err(e) => inc
                            .rejected
                            .push(format!("{}.{process}.{k}: {e}", sec.as_str())),
                    }
                }
                inc.rules.push(IncomingRule {
                    section: sec,
                    process,
                    fields,
                });
            }
        }
    }
    inc.ignored.sort();
    inc.ignored.dedup();
    Ok(inc)
}

/// 把导入的一条规则**按字段合并**到同名规则上：底子是用户条目，其次是系统条目的完整拷贝
/// （写时复制），导入里写了的键覆盖它、没写的键保留。
///
/// 不能整条覆盖：论坛里常见的分享片段只写一两个字段，整条覆盖会让系统规则里的其余字段
/// （出厂的宿主修复）静默丢失。想整条替换请用 replace 模式（先清空用户层，此时底子就是系统条目）。
/// 值域外的取值走容错反序列化回落成「未配」，由调用方从回落记录里告警；结构性错误（类型错）
/// 则整条拒绝。
fn import_one<T: Rule>(
    system: &[T],
    user: &mut Vec<T>,
    process: &str,
    fields: &Map<String, Value>,
) -> Result<(), String> {
    let base = base_for(system, user, process).unwrap_or_else(|| {
        let mut t = T::default();
        t.set_process(process);
        t
    });
    let mut obj = to_obj(&base);
    for (k, v) in fields {
        obj.insert(k.clone(), v.clone());
    }
    let entry: T = serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())?;
    commit_entry(system, user, entry);
    Ok(())
}

struct ImportPlan {
    after: Layers,
    items: Vec<ImportItem>,
    fallbacks: Vec<String>,
    rejected: Vec<String>,
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

    fn import_rule(&mut self, r: &IncomingRule) -> Result<(), String> {
        match r.section {
            Section::Apps => import_one(
                &self.system.apps,
                &mut self.user.apps,
                &r.process,
                &r.fields,
            ),
            Section::InitialModeScope => import_one(
                &self.system.initial_mode_scope,
                &mut self.user.initial_mode_scope,
                &r.process,
                &r.fields,
            ),
            Section::CommitNewline => import_one(
                &self.system.commit_newline,
                &mut self.user.commit_newline,
                &r.process,
                &r.fields,
            ),
        }
    }

    /// 在副本上试算导入结果。不改 `self`。
    fn plan_import(&self, incoming: &Incoming, mode: ImportMode) -> ImportPlan {
        let mut after = self.clone();
        if mode == ImportMode::Replace {
            after.reset_all();
        }
        let mut rejected = incoming.rejected.clone();
        let mut items = Vec::new();
        crate::tolerant_de::clear_fallbacks();
        for r in &incoming.rules {
            if let Err(e) = after.import_rule(r) {
                rejected.push(format!("{}.{}: {e}", r.section.as_str(), r.process));
                continue;
            }
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
        let fallbacks = crate::tolerant_de::take_fallbacks();
        if mode == ImportMode::Replace {
            for sec in [
                Section::Apps,
                Section::InitialModeScope,
                Section::CommitNewline,
            ] {
                for v in self.view(sec).into_iter().filter(|v| v.has_user_entry) {
                    let mentioned = incoming
                        .rules
                        .iter()
                        .any(|r| r.section == sec && same(&r.process, &v.process));
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
        ImportPlan {
            after,
            items,
            fallbacks,
            rejected,
        }
    }

    /// 导入预览：不落盘，不改 `self`。
    pub fn import_preview(&self, text: &str, mode: ImportMode) -> Result<ImportPreview, String> {
        let incoming = parse_incoming(text)?;
        let plan = self.plan_import(&incoming, mode);
        Ok(ImportPreview {
            items: plan.items,
            ignored_keys: incoming.ignored,
            fallbacks: plan.fallbacks,
            rejected: plan.rejected,
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
            fallbacks: plan.fallbacks,
            rejected: plan.rejected,
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
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
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

    /// M1：对协议字段写 null，不得留下「视图显示系统、文件里却躺着空壳」的隐形条目。
    /// 空壳会把以后出厂新增的修复整条挡掉（`update_user_rule` 文档里记过同款事故）。
    #[test]
    fn null_on_protocol_field_leaves_no_ghost_entry() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Feishu.exe",
            &patch(json!({"composition_start_pair_guard": null})),
        )
        .unwrap();
        assert!(
            l.user.apps.is_empty(),
            "协议字段置 null 后与系统一致，不应留用户条目: {:?}",
            l.user.apps
        );
        assert_eq!(state_of(&l, "Feishu.exe"), RuleState::System);
    }

    /// M1 的反方向：显式关闭协议字段（Some(false)）必须留得住，运行时读到的是 false。
    #[test]
    fn explicit_false_on_protocol_field_is_kept_and_wins_at_runtime() {
        let mut l = layers(SYS, "");
        l.upsert(
            Section::Apps,
            "Feishu.exe",
            &patch(json!({"composition_start_pair_guard": false})),
        )
        .unwrap();
        assert_eq!(l.user.apps.len(), 1, "显式关闭不能被当成冗余删掉");
        assert_eq!(l.user.apps[0].composition_start_pair_guard, Some(false));
        assert_eq!(state_of(&l, "Feishu.exe"), RuleState::Modified);
    }

    /// M2：导入稀疏片段不得让出厂规则里的其它字段静默丢失。
    #[test]
    fn import_sparse_snippet_keeps_the_system_fields() {
        let sys =
            "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\nstale_probe_guard = true\n";
        let mut l = layers(sys, "");
        l.import_apply(
            "[[apps]]\nprocess = \"weixin.exe\"\nfirst_show_mode = \"wait\"\n",
            ImportMode::Merge,
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.effective["caret_use_top"], json!(true), "出厂修复不能丢");
        assert_eq!(
            v.effective["stale_probe_guard"],
            json!(true),
            "出厂修复不能丢"
        );
        assert_eq!(v.effective["first_show_mode"], json!("wait"));
    }

    /// M2：导入片段里显式写 false 必须能覆盖出厂的 true（不能因为 false 被序列化省略而丢）。
    #[test]
    fn import_explicit_false_overrides_a_system_true() {
        let sys = "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\n";
        let mut l = layers(sys, "");
        l.import_apply(
            "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = false\n",
            ImportMode::Merge,
        )
        .unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Modified);
        assert!(
            v.effective.get("caret_use_top").is_none(),
            "false 应生效: {:?}",
            v.effective
        );
    }

    /// S1：data_custom 层禁用的规则，视图里不得再显示成「生效」（运行时它确实没有）。
    #[test]
    fn rule_disabled_in_custom_layer_is_absent_in_view_and_at_runtime() {
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
        let l = Layers::load(Some(&d), Some(&c), None).unwrap();
        assert!(
            l.view_of(Section::Apps, "Weixin.exe").is_none(),
            "视图与运行时必须一致"
        );
        let rt = crate::app_compat::AppCompat::load_layered(Some(&d), Some(&c), None);
        assert!(rt.get_rule("Weixin.exe").is_none());
        cleanup(&d, &c);
    }

    /// S2：错值必须被拒绝，而不是被容错反序列化吞成「跟随全局」后假装成功。
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

    /// S3：补丁不能碰 disabled / comment（disabled 走 setDisabled；只改 comment 会被冗余判定丢掉）。
    #[test]
    fn patch_rejects_meta_keys() {
        for p in [json!({"disabled": true}), json!({"comment": "x"})] {
            let mut l = layers(SYS, "");
            assert!(
                l.upsert(Section::Apps, "Weixin.exe", &patch(p.clone()))
                    .is_err(),
                "{p}"
            );
        }
    }

    /// S4：导入的进程名同样要校验；非法名字不得落盘。
    #[test]
    fn import_skips_invalid_process_names() {
        let mut l = layers(SYS, "");
        l.import_apply(
            "[[apps]]\nprocess = \" a/b.exe \"\nauto_pair = true\n\n[[apps]]\nprocess = \"ok.exe\"\nauto_pair = true\n",
            ImportMode::Merge,
        )
        .unwrap();
        assert!(
            l.user.apps.iter().all(|r| !r.process.contains('/')),
            "{:?}",
            l.user.apps
        );
        assert!(l.view_of(Section::Apps, "ok.exe").is_some());
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

    /// S4：已存的名字带空白时，reset 也要能删掉它（两边都 trim）。
    #[test]
    fn reset_finds_entries_stored_with_padding() {
        let mut l = layers(SYS, "[[apps]]\nprocess = \" x.exe \"\nauto_pair = true\n");
        assert!(l.reset(Section::Apps, "x.exe"));
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

    /// M3：右键菜单的写入路径遇到损坏的用户层时，可以重建，但必须先把原文件留一份。
    #[test]
    fn menu_write_on_broken_file_keeps_a_copy_of_the_original() {
        let dir = tmp("menu_bad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let broken = "[[apps\nprocess=";
        std::fs::write(dir.join(COMPAT_FILE_NAME), broken).unwrap();
        crate::app_compat::set_user_first_show_mode(
            &dir,
            "a.exe",
            Some(crate::app_compat::FirstShowMode::Wait),
        )
        .unwrap();
        let kept = std::fs::read_to_string(dir.join(format!("{COMPAT_FILE_NAME}.bad")))
            .expect("损坏的原文件应被留作 .bad");
        assert_eq!(kept, broken);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 可选 4：classes 只改大小写 / 顺序，运行时等价，不应算「已修改」。
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

    /// S10：视图要能让界面判断「用户层有没有这一条」「禁用前用户改过什么」。
    #[test]
    fn view_exposes_whether_a_user_entry_exists_and_its_raw_content() {
        let mut l = layers(SYS, "");
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert!(!v.has_user_entry);
        assert!(v.user.is_none());

        l.upsert(
            Section::Apps,
            "Weixin.exe",
            &patch(json!({"auto_pair": true})),
        )
        .unwrap();
        l.set_disabled(Section::Apps, "Weixin.exe", true).unwrap();
        let v = l.view_of(Section::Apps, "Weixin.exe").unwrap();
        assert_eq!(v.state, RuleState::Disabled);
        assert!(v.has_user_entry);
        let user = v.user.expect("禁用状态下要能看到用户层原始条目");
        assert_eq!(user["auto_pair"], json!(true), "禁用前的改写要可见");
        assert_eq!(user["disabled"], json!(true));
    }

    /// 导入里字段类型错的规则整条拒绝并报出来，而不是悄悄丢或写坏。
    #[test]
    fn import_reports_rejected_rules() {
        let text = "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = \"CabinetWClass\"\n\n[[apps]]\nprocess = \" a/b.exe \"\nauto_pair = true\n";
        let p = layers(SYS, "")
            .import_preview(text, ImportMode::Merge)
            .unwrap();
        assert_eq!(p.rejected.len(), 2, "{:?}", p.rejected);
        assert!(
            p.items.is_empty(),
            "被拒绝的不应出现在动作清单里: {:?}",
            p.items
        );
    }

    /// 导入同一份内容第二次应当全是「无变化」（幂等），不能每次都报「覆盖」。
    #[test]
    fn importing_the_same_content_twice_is_idempotent() {
        let text = "[[apps]]\nprocess = \"Weixin.exe\"\nauto_pair = true\n";
        let mut l = layers(SYS, "");
        let first = l.import_apply(text, ImportMode::Merge).unwrap();
        assert_eq!(first.items[0].action, "override");
        let second = l.import_apply(text, ImportMode::Merge).unwrap();
        assert_eq!(second.items[0].action, "unchanged", "{:?}", second.items);
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
