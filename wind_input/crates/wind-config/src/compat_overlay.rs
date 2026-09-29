//! 兼容规则的分层叠加引擎（原始键值层面）。
//!
//! # 整体原则：用户层只记差异，逐字段叠加在系统层之上
//!
//! 层序 `data/compat.toml` < `data_custom` < 用户层。**每一层都是相对下一层的差异**，
//! 同名进程内逐字段叠加：
//! - 条目里写了的字段覆盖下层的值，没写的字段继承下层；
//! - 想取消下层设定的字段，用 `unset = ["字段名", …]` 显式表达（回到「没设置 / 跟随全局」）；
//! - 想关掉下层打开的开关，显式写 `false`；
//! - `disabled = true` 把整条规则禁用（它也只是一个普通字段，可被更高层用 `disabled = false` 重新启用）；
//! - 没有「整条替换」这种合并语义。整条替换只存在于导入的 replace 模式（先清空用户层）。
//!
//! 为什么在**原始键值**上做，而不是在 `AppCompatRule` 结构体上：结构体的裸 `bool` / `i32`
//! 字段序列化时会省略默认值，「没写」和「显式写了 false」在结构体里是同一个东西；而叠加需要
//! 区分它们（用户显式关掉出厂打开的开关）。所以先在原始表上叠加，最后才一次性反序列化成结构体。
//!
//! # 两条必须守住的纪律
//!
//! 1. **写错的值 = 没写**：叠加之前先 [`sanitize`]。若让错值先参与叠加、事后才被容错回落成
//!    `None`，它会把下层的好值一起盖没（连出厂的作用域清单整行丢失都实测出现过）。
//! 2. **用户层里不认识的东西原样保留**：认不出的键、认不出的值、认不出的顶层段，在写回时
//!    都不能被悄悄删掉（新版本写下的内容被旧版本读写时尤其如此）。读不进来的内容
//!    （[`Raw::skipped`]）则由写入路径当作「文件已损坏」处理，先留 `.bad`。
//!
//! 设计见 `docs/design/compat-settings-ui.md` 第 11 节。

use crate::compat_schema::{COMPAT_FIELDS, Kind, known_keys};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// 一条规则的原始键值。
pub(crate) type Obj = Map<String, Value>;

/// 不属于「兼容取值」的键：`process` 是主键，`comment` 只是文档，`disabled` 与 `unset`
/// 是叠加语法本身。它们不进 `overridden`，也不允许被补丁 / 还原当成普通字段触碰。
pub(crate) const META_KEYS: [&str; 4] = ["process", "comment", "disabled", "unset"];

/// 三段规则的原始键值，外加原样保留的其它顶层内容。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Raw {
    pub(crate) apps: Vec<Obj>,
    pub(crate) initial_mode_scope: Vec<Obj>,
    pub(crate) commit_newline: Vec<Obj>,
    /// 三段之外的顶层键（比如更新版本新增的段）：不认识但**原样保留**，写回时照写。
    pub(crate) extra: toml::Table,
    /// 读不进来（因而写回时会丢）的内容的说明：三段之一不是数组表（`[apps]` 写成了单括号）、
    /// 数组里有不是表的元素。写入路径见到它要当作「文件已损坏」，先留 `.bad` 再重建。
    pub(crate) skipped: Vec<String>,
}

fn to_json(v: &toml::Value) -> Value {
    match v {
        // 日期时间没有对应的 JSON 类型；写成它的文本形式，至少保住内容且写回仍是合法 TOML。
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(to_json).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), to_json(v)))
                .collect::<Obj>(),
        ),
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    }
}

/// 同一段里同名进程的多行，按先后顺序叠成一行（后写的赢）。与运行时逐行叠加的结果一致，
/// 但让后续的编辑与视图只面对「每个进程一行」。
fn coalesce(rows: Vec<Obj>) -> Vec<Obj> {
    let mut out: Vec<Obj> = Vec::new();
    for r in rows {
        let name = process_of(&r).to_string();
        let same = (!name.trim().is_empty())
            .then(|| out.iter().position(|o| same_process(process_of(o), &name)))
            .flatten();
        match same {
            Some(i) => out[i] = compose(&out[i], &r),
            None => out.push(r),
        }
    }
    out
}

/// 解析 compat.toml 全文成原始键值。语法错 ⇒ `Err`（带行列号）。
pub(crate) fn parse_raw(text: &str) -> Result<Raw, String> {
    let root: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut raw = Raw::default();
    let Some(top) = root.as_table() else {
        return Ok(raw);
    };
    for (name, value) in top {
        let dst = match name.as_str() {
            "apps" => &mut raw.apps,
            "initial_mode_scope" => &mut raw.initial_mode_scope,
            "commit_newline" => &mut raw.commit_newline,
            _ => {
                raw.extra.insert(name.clone(), value.clone());
                continue;
            }
        };
        let Some(items) = value.as_array() else {
            let msg = format!("{name} 不是数组表（应写成 [[{name}]]）");
            tracing::warn!("compat.toml: {msg}，整段跳过");
            raw.skipped.push(msg);
            continue;
        };
        for (i, item) in items.iter().enumerate() {
            match item {
                toml::Value::Table(t) => dst.push(
                    t.iter()
                        .map(|(k, v)| (k.clone(), to_json(v)))
                        .collect::<Obj>(),
                ),
                _ => {
                    let msg = format!("{name}[{i}] 不是表");
                    tracing::warn!("compat.toml: {msg}，已跳过");
                    raw.skipped.push(msg);
                }
            }
        }
    }
    raw.apps = coalesce(std::mem::take(&mut raw.apps));
    raw.initial_mode_scope = coalesce(std::mem::take(&mut raw.initial_mode_scope));
    raw.commit_newline = coalesce(std::mem::take(&mut raw.commit_newline));
    Ok(raw)
}

pub(crate) fn process_of(o: &Obj) -> &str {
    o.get("process").and_then(Value::as_str).unwrap_or("")
}

/// 进程名相等：两边 trim、不区分大小写（与运行时查表一致，运行时在读入时就 trim）。
pub(crate) fn same_process(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// `unset` 清单：接受字符串数组，也宽容地接受单个字符串（`unset = "first_show_mode"`）。
/// 不是这两种形态返回 `None`。清单里的非字符串元素丢弃，`process` / `unset` 自身不能被 unset。
fn parse_unset(v: &Value) -> Option<Vec<String>> {
    let list: Vec<String> = match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => return None,
    };
    Some(
        list.into_iter()
            .filter(|k| !matches!(k.as_str(), "process" | "unset"))
            .collect(),
    )
}

fn unset_of(o: &Obj) -> Vec<String> {
    o.get("unset").and_then(parse_unset).unwrap_or_default()
}

/// 把 `layer` 这一条叠加到 `base` 上，得到新的一条。
///
/// 1. `layer.unset` 里列的字段先从 `base` 里拿掉（含 `disabled`：`unset = ["disabled"]` 即解除下层的禁用）；
/// 2. `layer` 自己写了的字段覆盖 `base`（同一条里既 `unset` 又写了同名字段 = 写了的赢）；
/// 3. `unset` 清单向上**累积**（`base` 的并 `layer` 的，再减去 `layer` 自己写了的字段）：
///    叠加结果同样是一份「相对更下一层的差异」，更高层还要能读到它。
/// 4. `process` 保留 `base` 的写法（系统层的大小写），没有才取 `layer` 的。
pub(crate) fn compose(base: &Obj, layer: &Obj) -> Obj {
    let mut out = base.clone();
    let layer_unset = unset_of(layer);
    for k in &layer_unset {
        out.remove(k);
    }
    let mut carried: BTreeSet<String> = unset_of(base).into_iter().collect();
    carried.extend(layer_unset);
    for (k, v) in layer {
        match k.as_str() {
            "unset" => {}
            "process" => {
                out.entry("process").or_insert_with(|| v.clone());
            }
            _ => {
                out.insert(k.clone(), v.clone());
                carried.remove(k);
            }
        }
    }
    if carried.is_empty() {
        out.remove("unset");
    } else {
        out.insert(
            "unset".into(),
            Value::Array(carried.into_iter().map(Value::String).collect()),
        );
    }
    out
}

/// 把 `layer` 一段整体叠加到 `base` 一段上：同名进程逐字段叠加，其余保持，新进程追加。
/// 进程名为空（缺失 / 类型写错）的行作废，不套给任何人。
pub(crate) fn overlay(mut base: Vec<Obj>, layer: &[Obj]) -> Vec<Obj> {
    for row in layer.iter().filter(|r| !process_of(r).trim().is_empty()) {
        match base
            .iter()
            .position(|b| same_process(process_of(b), process_of(row)))
        {
            Some(i) => base[i] = compose(&base[i], row),
            None => base.push(compose(&Obj::new(), row)),
        }
    }
    base
}

/// 三段一起叠加（只带三段，`extra` / `skipped` 对叠加结果没有意义）。
pub(crate) fn overlay_raw(base: Raw, layer: &Raw) -> Raw {
    Raw {
        apps: overlay(base.apps, &layer.apps),
        initial_mode_scope: overlay(base.initial_mode_scope, &layer.initial_mode_scope),
        commit_newline: overlay(base.commit_newline, &layer.commit_newline),
        ..Default::default()
    }
}

/// 该行是否被禁用（只认布尔 `true`；写错类型按未禁用处理，与结构体上的容错一致）。
pub(crate) fn is_disabled(o: &Obj) -> bool {
    o.get("disabled").and_then(Value::as_bool).unwrap_or(false)
}

/// 叠加结果 → 运行时结构体：跳过被禁用的行，剥掉 `unset`，逐行容错反序列化。
///
/// 单行结构性失败只丢这一行并留 WARN，不牵连同文件的其它规则。（字段值域外 / 类型错在这之前
/// 已由 [`sanitize`] 当成「没写」处理掉了，正常走不到这里。）
pub(crate) fn materialize<T: DeserializeOwned>(rows: &[Obj]) -> Vec<T> {
    rows.iter()
        .filter(|r| !is_disabled(r))
        .filter_map(|r| {
            let mut row = r.clone();
            row.remove("unset");
            match serde_json::from_value::<T>(Value::Object(row)) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(
                        "compat.toml: 规则 {:?} 结构不合法，已跳过: {e}",
                        process_of(r)
                    );
                    None
                }
            }
        })
        .collect()
}

// ───────────────────────── 值校验 ─────────────────────────

/// 一个**已登记**字段的值是否有问题（类型不对 / 值域外）。没问题返回 `None`；
/// 未登记的键不归这里管（返回 `None`，由调用方决定原样保留还是拒绝）。
///
/// 值域校验的办法：把这个键单独放进一个最小行里反序列化，看容错反序列化有没有把它回落掉——
/// 它对值域外的取值不报错、只记一条「回落」。别名（`en` / `zh`）与带空白的文本运行时本就接受，
/// 不算有问题。
pub(crate) fn field_value_problem<T: DeserializeOwned>(
    section: &str,
    key: &str,
    v: &Value,
) -> Option<String> {
    let meta = COMPAT_FIELDS
        .iter()
        .find(|f| f.section == section && f.key == key)?;
    if v.is_null() {
        return None;
    }
    let type_ok = match meta.kind {
        Kind::Bool | Kind::TriBool => v.is_boolean(),
        Kind::Int => v.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        // 枚举不许空串：空串会被当成「没写」吞掉，想清除请用 unset / null。
        Kind::Enum => v.as_str().is_some_and(|t| !t.trim().is_empty()),
        Kind::Text => v.is_string(),
        Kind::TextList => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
    };
    if !type_ok {
        return Some(format!("字段 {key} 的值 {v} 类型不对"));
    }
    let mut o = Obj::new();
    o.insert("process".into(), Value::String("probe.exe".into()));
    o.insert(key.to_string(), v.clone());
    crate::tolerant_de::clear_fallbacks();
    let parsed = serde_json::from_value::<T>(Value::Object(o));
    let fallbacks = crate::tolerant_de::take_fallbacks();
    if let Err(e) = parsed {
        return Some(format!("字段 {key} 的值 {v} 无效: {e}"));
    }
    (!fallbacks.is_empty()).then(|| format!("字段 {key} 的值 {v} 无效"))
}

/// 参与叠加之前先把无效的值当成「没写」剔掉。返回（清理后的行，每处剔除的说明）。
///
/// 只清理**会参与叠加的副本**：调用方持有的原始行（要写回文件的那份）不动，
/// 免得旧版本读写时把新版本写下的、自己认不出的值删掉。
pub(crate) fn sanitize<T: DeserializeOwned>(
    section: &str,
    rows: &[Obj],
) -> (Vec<Obj>, Vec<String>) {
    let mut report = Vec::new();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let name = process_of(r).to_string();
        let mut row = r.clone();
        let keys: Vec<String> = row.keys().cloned().collect();
        for k in keys {
            let problem = match k.as_str() {
                "process" | "comment" => None,
                "disabled" => {
                    (!row[&k].is_boolean()).then(|| format!("disabled = {} 不是布尔值", row[&k]))
                }
                "unset" => match parse_unset(&row[&k]) {
                    Some(list) => {
                        if list.is_empty() {
                            row.remove(&k);
                        } else {
                            row.insert(
                                k.clone(),
                                Value::Array(list.into_iter().map(Value::String).collect()),
                            );
                        }
                        None
                    }
                    None => Some(format!("unset = {} 不是字段名清单", row[&k])),
                },
                _ => field_value_problem::<T>(section, &k, &row[&k]),
            };
            if let Some(p) = problem {
                report.push(format!("{section}.{name}: {p}，已忽略"));
                row.remove(&k);
            }
        }
        out.push(row);
    }
    for line in &report {
        tracing::warn!("compat.toml: {line}");
    }
    (out, report)
}

// ───────────────────────── 用户层的写入侧 ─────────────────────────

/// 对用户层一条规则的一项字段编辑。
#[derive(Debug, Clone)]
pub(crate) enum FieldEdit {
    /// 设置为某个值（含显式 `false` / `0`：那是有意义的覆盖，不是「没写」）。
    Set(String, Value),
    /// 清除：回到「没设置 / 跟随全局」。若系统层设了这个字段，还要写进 `unset`，
    /// 否则「没写」只会让它继承回系统值。
    Clear(String),
    /// 还原：把这个字段的用户层痕迹（值与 `unset`）都拿掉，**继承系统层**。
    /// 与 `Clear` 的区别正是「出厂」与「跟随全局」的区别：系统层给 Excel 配了 `first_show_mode = wait`，
    /// 还原后仍是 `wait`；清除后则是跟随全局。
    Inherit(String),
}

fn unset_array(keys: &BTreeSet<String>) -> Value {
    Value::Array(keys.iter().cloned().map(Value::String).collect())
}

/// 窗口类名运行时不区分大小写、也与顺序无关：只改这两样不算「有差异」。
fn normalize_classes(view: &mut Obj) {
    if let Some(Value::Array(items)) = view.get_mut("classes") {
        let mut v: Vec<String> = items
            .iter()
            .filter_map(|x| x.as_str().map(str::to_ascii_lowercase))
            .collect();
        v.sort();
        v.dedup();
        *items = v.into_iter().map(Value::String).collect();
    }
}

/// 某条用户行叠加到系统行之后，运行时真正看到的内容（经结构体往返，省略默认值）。
///
/// 冗余判定必须比**这个**，而不是比原始键值：`caret_use_top = false` 在系统层没有该字段时
/// 与「没写」运行时完全等价，前者是冗余；而系统层为 `true` 时它就是必要的显式覆盖。
fn effective_view<T: DeserializeOwned + Serialize>(
    sys: Option<&Obj>,
    user: &Obj,
    unset: &BTreeSet<String>,
) -> Obj {
    let mut layer = user.clone();
    layer.remove("unset");
    if !unset.is_empty() {
        layer.insert("unset".into(), unset_array(unset));
    }
    let base = sys.cloned().unwrap_or_default();
    let mut composed = compose(&base, &layer);
    composed.remove("unset");
    composed.remove("comment");
    let mut view = match serde_json::from_value::<T>(Value::Object(composed.clone())) {
        Ok(t) => match serde_json::to_value(&t) {
            Ok(Value::Object(mut o)) => {
                o.remove("process");
                o.remove("comment");
                o
            }
            _ => composed,
        },
        Err(_) => composed,
    };
    normalize_classes(&mut view);
    view
}

/// 这一行里**参与运行时**的键：已登记且值有效的字段、布尔的 `disabled`。
/// 认不出的键、写错的值对运行时都没有效果（前者原样保留、后者被当成没写），不算差异。
pub(crate) fn effective_keys<T: DeserializeOwned>(section: &str, row: &Obj) -> Vec<String> {
    let known = known_keys(section);
    row.iter()
        .filter(|(k, v)| {
            if k.as_str() == "disabled" {
                v.is_boolean()
            } else {
                known.contains(&k.as_str()) && field_value_problem::<T>(section, k, v).is_none()
            }
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// 这一行里有没有**参与运行时**的差异（见 [`effective_keys`]，外加非空的 `unset`）：
/// 视图据此判断「已修改」。
pub(crate) fn has_registered_diff<T: DeserializeOwned>(section: &str, row: &Obj) -> bool {
    !effective_keys::<T>(section, row).is_empty() || !unset_of(row).is_empty()
}

/// 规范化一条用户行：去掉不起作用的键与不必要的 `unset`。返回这一行是否还有内容
/// （没有 = 与系统层完全一致 / 空壳，应当删掉这一条）。
///
/// 空壳不是无害的：它让「已修改」状态凭空出现，也让人无从判断还原按钮有没有东西可还原。
/// 「用户层只记差异」这条原则的落地点就在这里——任何写入路径最后都要过它。
///
/// 只修剪**已登记且值有效**的字段：认不出的键、认不出的值（比如新版本写下的枚举值）经结构体
/// 往返本来就「看不见」，按效果判断它必然是冗余，会被静默删掉——用户层里的东西不是我们的，原样留着。
///
/// `prune_unset = false`：调用方**不知道**系统层的真实内容（数据目录解析不出来时），
/// 此时不能凭「系统里没有这个字段」去掉 `unset`——它可能只是没读到。
pub(crate) fn normalize<T: DeserializeOwned + Serialize>(
    section: &str,
    sys: Option<&Obj>,
    user: &mut Obj,
    prune_unset: bool,
) -> bool {
    let mut unset: BTreeSet<String> = unset_of(user)
        .into_iter()
        .filter(|k| !user.contains_key(k))
        .collect();
    user.remove("unset");

    let full = effective_view::<T>(sys, user, &unset);
    // 1) 字段与 disabled：去掉后运行时效果不变的就是冗余。
    for k in effective_keys::<T>(section, user) {
        let mut without = user.clone();
        without.remove(&k);
        if effective_view::<T>(sys, &without, &unset) == full {
            user.remove(&k);
        }
    }
    // 2) unset：去掉后效果不变的（系统层本来就没这个字段）就不必写。
    if prune_unset {
        let full = effective_view::<T>(sys, user, &unset);
        for k in unset.clone() {
            let mut fewer = unset.clone();
            fewer.remove(&k);
            if effective_view::<T>(sys, user, &fewer) == full {
                unset = fewer;
            }
        }
    }

    let has_content = user
        .keys()
        .any(|k| !matches!(k.as_str(), "process" | "comment"))
        || !unset.is_empty();
    if !unset.is_empty() {
        user.insert("unset".into(), unset_array(&unset));
    }
    if !has_content {
        user.remove("comment");
    }
    has_content
}

/// 把一批字段编辑应用到用户层某一段：找到（或新建）该进程的差异行，逐项编辑，规范化，
/// 落回（没内容就删掉这一行）。`sys_rows` 是**已叠加好的系统层**该段。
pub(crate) fn apply_edits<T: DeserializeOwned + Serialize>(
    section: &str,
    sys_rows: &[Obj],
    user_rows: &mut Vec<Obj>,
    process: &str,
    edits: &[FieldEdit],
    system_known: bool,
) {
    let process = process.trim();
    let sys = sys_rows
        .iter()
        .find(|r| same_process(process_of(r), process));
    let idx = user_rows
        .iter()
        .position(|r| same_process(process_of(r), process));
    let mut row = match idx {
        Some(i) => user_rows[i].clone(),
        None => {
            let mut o = Obj::new();
            // 新建行沿用系统层的写法（大小写），没有才用调用方给的。
            let name = sys
                .map(process_of)
                .filter(|n| !n.is_empty())
                .unwrap_or(process);
            o.insert("process".into(), Value::String(name.trim().to_string()));
            o
        }
    };
    let mut unset: BTreeSet<String> = unset_of(&row).into_iter().collect();
    for e in edits {
        match e {
            FieldEdit::Set(k, v) => {
                row.insert(k.clone(), v.clone());
                unset.remove(k);
            }
            FieldEdit::Clear(k) => {
                row.remove(k);
                // 系统层设了它（或压根不知道系统层是什么）才需要显式 unset。
                if !system_known || sys.is_some_and(|s| s.contains_key(k)) {
                    unset.insert(k.clone());
                }
            }
            FieldEdit::Inherit(k) => {
                row.remove(k);
                unset.remove(k);
            }
        }
    }
    row.remove("unset");
    if !unset.is_empty() {
        row.insert("unset".into(), unset_array(&unset));
    }
    let keep = normalize::<T>(section, sys, &mut row, system_known);
    match (idx, keep) {
        (Some(i), true) => user_rows[i] = row,
        (Some(i), false) => {
            user_rows.remove(i);
        }
        (None, true) => user_rows.push(row),
        (None, false) => {}
    }
}

// ───────────────────────── 渲染 ─────────────────────────

/// 一行里键的输出顺序：元键在前，其次按字段登记表的顺序，最后是登记表之外的键（按字母序，
/// 原样保留——新版本写下的字段被旧版本读写时不能被悄悄丢掉）。
fn ordered_keys(section: &str, o: &Obj) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in META_KEYS {
        if o.contains_key(k) {
            out.push(k.to_string());
        }
    }
    for f in COMPAT_FIELDS.iter().filter(|f| f.section == section) {
        if o.contains_key(f.key) {
            out.push(f.key.to_string());
        }
    }
    let mut rest: Vec<String> = o.keys().filter(|k| !out.contains(k)).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

/// TOML 键：纯 ASCII 字母数字 / 下划线 / 连字符可以裸写，其它（非 ASCII、含点、含空格……）必须加引号，
/// 否则手写的一个 `"备注" = "x"` 写回后就成了非法 TOML，整个用户层随之失效。
fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        toml::Value::String(k.to_string()).to_string()
    }
}

fn render_section(out: &mut String, section: &str, rows: &[Obj]) -> Result<(), String> {
    for row in rows {
        out.push_str(&format!("[[{section}]]\n"));
        for k in ordered_keys(section, row) {
            let tv = toml::Value::try_from(&row[&k])
                .map_err(|e| format!("{section} 里的键 {k} 的值无法写成 TOML: {e}"))?;
            out.push_str(&format!("{} = {tv}\n", toml_key(&k)));
        }
        out.push('\n');
    }
    Ok(())
}

/// 把三段原始键值（连同原样保留的其它顶层内容）渲染成用户层 compat.toml 全文（文件头由调用方给）。
///
/// 渲染完**必须自检**：重新解析后与内存里的内容一致才放行，否则拒绝落盘。写回失真的后果是整份
/// 用户层失效并在下一次菜单操作时被重建，宁可这次写入失败。
pub(crate) fn render_raw(header: &str, raw: &Raw) -> Result<String, String> {
    let mut out = String::from(header);
    render_section(&mut out, "apps", &raw.apps)?;
    render_section(&mut out, "initial_mode_scope", &raw.initial_mode_scope)?;
    render_section(&mut out, "commit_newline", &raw.commit_newline)?;
    if !raw.extra.is_empty() {
        out.push_str(
            &toml::to_string(&raw.extra).map_err(|e| format!("其它顶层内容无法写回: {e}"))?,
        );
    }
    let back = parse_raw(&out).map_err(|e| format!("渲染结果无法重新解析: {e}"))?;
    if back.apps != coalesce(raw.apps.clone())
        || back.initial_mode_scope != coalesce(raw.initial_mode_scope.clone())
        || back.commit_newline != coalesce(raw.commit_newline.clone())
        || back.extra != raw.extra
    {
        return Err("写回后的内容与内存中不一致，已拒绝落盘（防止改坏用户文件）".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Obj {
        v.as_object().cloned().expect("测试夹具必须是对象")
    }

    #[test]
    fn layer_fields_override_and_unwritten_fields_are_inherited() {
        let base =
            obj(json!({"process": "A.exe", "caret_use_top": true, "stale_probe_guard": true}));
        let layer = obj(json!({"process": "a.exe", "first_show_mode": "wait"}));
        let out = compose(&base, &layer);
        assert_eq!(out["caret_use_top"], json!(true), "没写的字段必须继承");
        assert_eq!(out["stale_probe_guard"], json!(true));
        assert_eq!(out["first_show_mode"], json!("wait"));
        assert_eq!(out["process"], json!("A.exe"), "保留底层的写法");
    }

    #[test]
    fn explicit_false_overrides_a_true_below() {
        let base = obj(json!({"process": "A.exe", "caret_use_top": true}));
        let layer = obj(json!({"process": "A.exe", "caret_use_top": false}));
        assert_eq!(compose(&base, &layer)["caret_use_top"], json!(false));
    }

    #[test]
    fn unset_removes_a_field_from_below_and_is_carried_upwards() {
        let base = obj(json!({"process": "A.exe", "first_show_mode": "wait", "auto_pair": true}));
        let layer = obj(json!({"process": "A.exe", "unset": ["first_show_mode"]}));
        let out = compose(&base, &layer);
        assert!(out.get("first_show_mode").is_none());
        assert_eq!(out["auto_pair"], json!(true));
        assert_eq!(
            out["unset"],
            json!(["first_show_mode"]),
            "累积上去，更高层还要读到"
        );
    }

    #[test]
    fn a_higher_layer_can_set_a_field_that_a_lower_layer_unset() {
        let mid = obj(json!({"process": "A.exe", "unset": ["first_show_mode"]}));
        let top = obj(json!({"process": "A.exe", "first_show_mode": "fast"}));
        let out = compose(&mid, &top);
        assert_eq!(out["first_show_mode"], json!("fast"));
        assert!(
            out.get("unset").is_none(),
            "被重新设置的字段要从 unset 里拿掉"
        );
    }

    #[test]
    fn writing_and_unsetting_the_same_field_in_one_entry_means_set() {
        let base = obj(json!({"process": "A.exe", "first_show_mode": "wait"}));
        let layer = obj(
            json!({"process": "A.exe", "first_show_mode": "fast", "unset": ["first_show_mode"]}),
        );
        let out = compose(&base, &layer);
        assert_eq!(out["first_show_mode"], json!("fast"));
        assert!(out.get("unset").is_none());
    }

    #[test]
    fn overlay_is_per_process_case_insensitive_and_appends_new_ones() {
        let base = vec![
            obj(json!({"process": "A.exe", "auto_pair": true})),
            obj(json!({"process": "B.exe", "auto_pair": true})),
        ];
        let layer = vec![
            obj(json!({"process": " a.EXE ", "caret_use_top": true})),
            obj(json!({"process": "C.exe", "auto_pair": false})),
        ];
        let out = overlay(base, &layer);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["auto_pair"], json!(true));
        assert_eq!(out[0]["caret_use_top"], json!(true));
        assert_eq!(process_of(&out[2]), "C.exe");
    }

    #[test]
    fn rows_without_a_usable_process_are_dropped() {
        let layer = vec![
            obj(json!({"auto_pair": true})),
            obj(json!({"process": 1})),
            obj(json!({"process": "  "})),
        ];
        assert!(overlay(Vec::new(), &layer).is_empty());
    }

    #[test]
    fn disabled_is_an_ordinary_overlaid_field() {
        let base = obj(json!({"process": "A.exe", "disabled": true, "auto_pair": true}));
        let enabled = compose(&base, &obj(json!({"process": "A.exe", "disabled": false})));
        assert!(!is_disabled(&enabled), "更高层用 disabled = false 重新启用");
        let stays = compose(
            &base,
            &obj(json!({"process": "A.exe", "caret_use_top": true})),
        );
        assert!(is_disabled(&stays), "没写 disabled 就继承下层的禁用");
    }

    #[test]
    fn parse_raw_keeps_explicit_false_and_reads_all_three_sections() {
        let raw = parse_raw(
            "[[apps]]\nprocess = \"a.exe\"\ncaret_use_top = false\n\n[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"X\"]\n\n[[commit_newline]]\nprocess = \"w.exe\"\nstyle = \"cr\"\n",
        )
        .unwrap();
        assert_eq!(
            raw.apps[0]["caret_use_top"],
            json!(false),
            "显式 false 必须保留"
        );
        assert_eq!(raw.initial_mode_scope[0]["classes"], json!(["X"]));
        assert_eq!(raw.commit_newline[0]["style"], json!("cr"));
    }

    #[test]
    fn parse_raw_reports_syntax_errors_with_a_position() {
        let err = parse_raw("[[apps\nprocess=").unwrap_err();
        assert!(err.contains("line"), "{err}");
    }

    #[test]
    fn materialize_skips_disabled_and_strips_unset_and_isolates_bad_rows() {
        use crate::app_compat::AppCompatRule;
        let rows = vec![
            obj(json!({"process": "a.exe", "caret_use_top": true, "unset": ["x"]})),
            obj(json!({"process": "b.exe", "disabled": true, "caret_use_top": true})),
        ];
        let rules: Vec<AppCompatRule> = materialize(&rows);
        assert_eq!(rules.len(), 1);
        assert!(rules[0].caret_use_top);

        use crate::app_compat::InitialModeScopeRule;
        let scopes: Vec<InitialModeScopeRule> = materialize(&[
            obj(json!({"process": "ok.exe", "classes": ["A"]})),
            obj(json!({"process": "bad.exe", "classes": "not-a-list"})),
        ]);
        assert_eq!(scopes.len(), 1, "结构错的行只丢它自己");
        assert_eq!(scopes[0].process, "ok.exe");
    }

    // ───────────── 写入侧 ─────────────

    use crate::app_compat::AppCompatRule;

    fn sys_wechat() -> Obj {
        obj(json!({"process": "Weixin.exe", "caret_use_top": true, "stale_probe_guard": true}))
    }

    #[test]
    fn set_on_a_system_rule_stores_only_the_diff() {
        let sys = vec![sys_wechat()];
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "weixin.exe",
            &[FieldEdit::Set("first_show_mode".into(), json!("instant"))],
            true,
        );
        assert_eq!(user.len(), 1);
        assert_eq!(user[0]["first_show_mode"], json!("instant"));
        assert!(
            user[0].get("caret_use_top").is_none(),
            "只记差异，不拷贝系统字段: {:?}",
            user[0]
        );
        assert_eq!(user[0]["process"], json!("Weixin.exe"), "沿用系统层的写法");
    }

    #[test]
    fn a_value_equal_to_the_system_one_is_redundant_and_the_row_disappears() {
        let sys = vec![sys_wechat()];
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "Weixin.exe",
            &[FieldEdit::Set("caret_use_top".into(), json!(true))],
            true,
        );
        assert!(user.is_empty(), "与系统一致就不该有用户条目: {user:?}");
    }

    #[test]
    fn explicit_false_over_a_system_true_is_kept_but_false_over_nothing_is_dropped() {
        let sys = vec![sys_wechat()];
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "Weixin.exe",
            &[FieldEdit::Set("caret_use_top".into(), json!(false))],
            true,
        );
        assert_eq!(
            user[0]["caret_use_top"],
            json!(false),
            "显式关掉出厂打开的开关"
        );

        let mut user2 = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &[],
            &mut user2,
            "New.exe",
            &[FieldEdit::Set("caret_use_top".into(), json!(false))],
            true,
        );
        assert!(user2.is_empty(), "系统没有它时 false 与没写等价，属冗余");
    }

    #[test]
    fn clear_writes_unset_only_when_the_system_layer_sets_that_field() {
        let sys = vec![obj(
            json!({"process": "EXCEL.EXE", "first_show_mode": "wait"}),
        )];
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "excel.exe",
            &[FieldEdit::Clear("first_show_mode".into())],
            true,
        );
        assert_eq!(user[0]["unset"], json!(["first_show_mode"]));

        let mut user2 = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user2,
            "excel.exe",
            &[FieldEdit::Clear("auto_pair".into())],
            true,
        );
        assert!(
            user2.is_empty(),
            "系统没设的字段清除是空操作，不该留噪声条目: {user2:?}"
        );
    }

    #[test]
    fn clear_with_unknown_system_layer_keeps_the_unset_marker() {
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &[],
            &mut user,
            "x.exe",
            &[FieldEdit::Clear("first_show_mode".into())],
            false,
        );
        assert_eq!(
            user[0]["unset"],
            json!(["first_show_mode"]),
            "读不到系统层时不能凭「没有」丢掉 unset"
        );
    }

    #[test]
    fn setting_again_after_clear_removes_the_unset_marker() {
        let sys = vec![obj(
            json!({"process": "EXCEL.EXE", "first_show_mode": "wait"}),
        )];
        let mut user = Vec::new();
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "excel.exe",
            &[FieldEdit::Clear("first_show_mode".into())],
            true,
        );
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "excel.exe",
            &[FieldEdit::Set("first_show_mode".into(), json!("fast"))],
            true,
        );
        assert_eq!(user[0]["first_show_mode"], json!("fast"));
        assert!(user[0].get("unset").is_none());
    }

    #[test]
    fn editing_one_field_never_disturbs_the_others_in_the_user_row() {
        let sys = vec![sys_wechat()];
        let mut user = vec![obj(
            json!({"process": "Weixin.exe", "auto_pair": true, "unknown_future_key": 1}),
        )];
        apply_edits::<AppCompatRule>(
            "apps",
            &sys,
            &mut user,
            "Weixin.exe",
            &[FieldEdit::Set("first_show_mode".into(), json!("fast"))],
            true,
        );
        assert_eq!(user[0]["auto_pair"], json!(true));
        assert_eq!(
            user[0]["unknown_future_key"],
            json!(1),
            "认不出的键要原样保留"
        );
    }

    #[test]
    fn render_orders_keys_and_roundtrips_through_the_parser() {
        let raw = Raw {
            apps: vec![obj(json!({
                "first_show_mode": "wait", "process": "A.exe", "comment": "带\"引号\"和中文",
                "caret_use_top": false, "unset": ["auto_pair"], "unknown_z": 1, "classes_like": ["a", "b"]
            }))],
            ..Default::default()
        };
        let text = render_raw("# header\n\n", &raw).unwrap();
        let process_at = text.find("process =").unwrap();
        let mode_at = text.find("first_show_mode =").unwrap();
        assert!(process_at < mode_at, "process 必须排在字段前: {text}");
        let back = parse_raw(&text).unwrap();
        assert_eq!(
            back.apps[0], raw.apps[0],
            "渲染再解析必须与原始键值一致: {text}"
        );
        assert_eq!(
            back.apps[0]["caret_use_top"],
            json!(false),
            "显式 false 必须写出来"
        );
    }

    // ───────────── 审查（第二轮）补的回归 ─────────────

    #[test]
    fn parse_raw_preserves_unknown_sections_and_flags_unreadable_content() {
        let raw = parse_raw("[future]\nk = 1\n\n[apps]\nprocess = \"x.exe\"\n\n[[commit_newline]]\nprocess = \"w.exe\"\n").unwrap();
        assert_eq!(
            raw.extra["future"]["k"].as_integer(),
            Some(1),
            "不认识的顶层段原样保留"
        );
        assert_eq!(
            raw.skipped.len(),
            1,
            "`[apps]` 单括号读不进来，必须被记下: {:?}",
            raw.skipped
        );
        assert!(raw.apps.is_empty());
        assert_eq!(raw.commit_newline.len(), 1);
        let raw = parse_raw("apps = [1, 2]\n").unwrap();
        assert!(!raw.skipped.is_empty(), "数组里不是表的元素也要记下");
    }

    #[test]
    fn parse_raw_merges_duplicate_rows_of_one_process_in_order() {
        let raw = parse_raw("[[apps]]\nprocess = \"x.exe\"\na = 1\nb = 2\n\n[[apps]]\nprocess = \" X.EXE \"\na = 9\n").unwrap();
        assert_eq!(raw.apps.len(), 1);
        assert_eq!(raw.apps[0]["a"], json!(9), "后写的赢");
        assert_eq!(raw.apps[0]["b"], json!(2), "先写的其它字段保留");
    }

    #[test]
    fn datetime_values_survive_as_text_and_the_file_stays_valid() {
        let raw =
            parse_raw("[[apps]]\nprocess = \"x.exe\"\nwhen = 2020-01-02T03:04:05Z\n").unwrap();
        assert_eq!(raw.apps[0]["when"], json!("2020-01-02T03:04:05Z"));
        assert!(render_raw("", &raw).is_ok(), "写回必须仍是合法 TOML");
    }

    /// 渲染结果自检：内存里的内容无法无损写成 TOML 时必须拒绝落盘，而不是写一份失真的文件。
    #[test]
    fn render_refuses_content_it_cannot_write_faithfully() {
        let mut row = Obj::new();
        row.insert("process".into(), json!("x.exe"));
        row.insert("bad".into(), Value::Null);
        let raw = Raw {
            apps: vec![row],
            ..Default::default()
        };
        assert!(render_raw("", &raw).is_err());
    }

    #[test]
    fn render_quotes_keys_that_are_not_bare() {
        let mut row = Obj::new();
        row.insert("process".into(), json!("x.exe"));
        row.insert("备注".into(), json!("说明"));
        row.insert("a.b".into(), json!(1));
        row.insert("with space".into(), json!(true));
        let raw = Raw {
            apps: vec![row],
            ..Default::default()
        };
        let text = render_raw("", &raw).unwrap();
        assert_eq!(parse_raw(&text).unwrap().apps, raw.apps, "{text}");
    }

    #[test]
    fn sanitize_drops_invalid_values_and_reports_them_but_keeps_unknown_keys() {
        use crate::app_compat::AppCompatRule;
        let rows = vec![obj(json!({
            "process": "a.exe", "first_show_mode": "wiat", "auto_pair": true, "caret_offset_x": "12",
            "future_key": [1, 2], "disabled": "yes", "unset": "auto_pair"
        }))];
        let (clean, report) = sanitize::<AppCompatRule>("apps", &rows);
        let c = &clean[0];
        assert!(
            c.get("first_show_mode").is_none()
                && c.get("caret_offset_x").is_none()
                && c.get("disabled").is_none()
        );
        assert_eq!(c["auto_pair"], json!(true));
        assert_eq!(c["future_key"], json!([1, 2]), "认不出的键不归清理管");
        assert_eq!(
            c["unset"],
            json!(["auto_pair"]),
            "单个字符串形态的 unset 归一成数组"
        );
        assert_eq!(report.len(), 3, "{report:?}");
        assert_eq!(rows[0]["first_show_mode"], json!("wiat"), "原始行不被改动");
    }
}
