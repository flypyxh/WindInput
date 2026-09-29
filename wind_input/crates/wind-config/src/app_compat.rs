//! 应用兼容性规则
//!
//! 与 Go 版本 `wind_input/pkg/config/compat.go` 对齐：按进程名为特定应用提供候选窗
//! 定位 / 光标获取等兼容修正。文件格式为 TOML 的 `[[apps]]` 数组表，加载顺序：
//! 系统预置（`{data_dir}/compat.toml`）→ 定制版（`data_custom/compat.toml`）→
//! 用户覆盖（`{user_config_dir}/compat.toml`）。
//!
//! **整体原则：每一层都是相对下一层的差异，逐字段叠加**（同名进程内：写了的字段覆盖、没写的继承、
//! `unset` 取消、`disabled` 禁用）。没有「同名整条替换」，也没有哪个字段享有特殊的继承待遇——
//! 叠加引擎与全部语义见 [`crate::compat_overlay`]，本模块负责把叠加结果变成运行时结构体。

use crate::compat_overlay::{
    FieldEdit, Raw, apply_edits, materialize, overlay_raw, parse_raw, render_raw,
};
use crate::config::SmartMethod;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// 默认兼容规则文件名。
pub const COMPAT_FILE_NAME: &str = "compat.toml";

/// 写回用户层 compat.toml 时的固定文件头。
///
/// 用户层由右键菜单自动管理，每次切换都会**整份重写**（TOML 序列化不保留注释），
/// 故必须在文件里就把这件事讲明白，否则用户手写的说明被吞掉时无从得知原因。
/// 完整的字段文档留在系统层 `data/compat.toml`——那份不会被程序改写。
pub(crate) const USER_COMPAT_HEADER: &str = "\
# 用户层应用兼容规则（相对系统层 data/compat.toml 的差异）
#
# ⚠ 本文件由输入法（右键菜单 / 设置页）自动管理，每次改动都会整份重写，
#   手写的注释与排版不会保留。需要长期留存的说明请写在系统层 compat.toml。
#
# 叠加语义：每条规则只记与系统层不同的字段，逐字段叠加在同名进程的系统规则之上：
#   - 写了的字段覆盖系统值，没写的字段继承系统值；
#   - unset = [\"字段名\", …] 取消系统设定的字段（回到「跟随全局」）；
#   - 显式写 false 可以关掉系统打开的开关；
#   - disabled = true 禁用整条系统规则。
# 字段说明见系统层 data/compat.toml 顶部注释。

";

/// `skip_serializing_if` 用：省略默认为 false 的开关，避免写回时铺满一堆 `= false`。
fn is_false(b: &bool) -> bool {
    !*b
}

/// 用户层 `compat.toml` 的进程内写锁。
///
/// 写用户层是「读整份 → 改 → 整份写回」，两个写入方（右键菜单在协调器线程、设置端 RPC 在
/// 各自的连接线程）交错时，后写的会把先写的改动整份覆盖掉。所有写入路径
/// （[`update_user_raw`]、`compat_admin::rpc` 的写方法）必须在读到写完的**全程**持有它。
static USER_COMPAT_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn lock_user_compat() -> std::sync::MutexGuard<'static, ()> {
    // 持锁线程 panic 只会让锁中毒，数据本身在磁盘上，不该让之后所有写入都失败。
    USER_COMPAT_WRITE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 落盘：唯一临时名 → 写满 → `fsync` → rename。
///
/// 只 rename 不 `fsync` 的话，断电后可能留下 0 长度或半截的正式文件；而运行时对语法错的
/// compat.toml 是**整份静默跳过**，下一次菜单操作还会按空集重写，等于用户层全丢。
/// 临时名带 pid 与序号，两个写入方即便没经过 [`lock_user_compat`] 也不会写同一个临时文件。
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| COMPAT_FILE_NAME.to_string());
    let tmp = path.with_file_name(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 候选窗首显策略：新组合的候选窗**何时**显示。
///
/// 背景：宿主插入组合内容后要 reflow 才能给出正确的光标坐标，而 reflow 需要时间
/// （实测首帧 GetTextExt 到稳定值要 85~95ms）。这三档是「快」与「准」之间的取舍。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstShowMode {
    /// 等宿主 reflow 后的权威坐标才显示。最准，代价是 85~95ms 首显延迟，
    /// 快速连打时候选窗只来得及显示几毫秒，观感「迟钝」。
    ///
    /// 2026-08-03 起不再是默认档。它的「准」有很大一部分是**碰巧**的：Excel 那类
    /// 慢宿主上它靠 `caret_pending` 的 600ms 延长兜住，宿主再慢 50ms 一样会错位
    /// （实测 Excel 需要 808ms 的那次它就没兜住）。真正解决错位的是首帧信任门，
    /// 而那条判据 `fast` 同样享有。
    Wait,
    /// 仍等坐标，但等到「可信」即放行：DLL 在首帧 reflow 期间连发几条试探坐标，
    /// 取第一条「与上一轮权威坐标不同」的采用（宿主未 reflow 时返回的正是上一轮那个
    /// 位置，一旦变化即说明新位置已就绪）。连续快速输入时更进一步——直接采信首条。
    /// 实测 EverEdit ~3ms、WPS ~11ms 出候选窗。
    ///
    /// **默认档**（2026-08-03 起）。此前不敢作默认，是因为它在焦点切换/鼠标移动光标
    /// 之后的首帧会拿一份属于别处的旧坐标去定位；首帧信任门补上这个洞之后
    /// （`caret_cache_verified`，见 `docs/redesign/candidate-window-positioning.md`
    /// 第 6 层），它在「坐标不可信」的那一刻会自动退回去等真值，其余时候保持 25ms
    /// 短兜底。实测常规连打首帧中位 7ms，焦点后首帧中位 105ms 且位置正确。
    #[default]
    Fast,
    /// 完全不等，首帧直接沿用上一次的坐标。最快，但只要光标位置变动过
    /// （手动移动、换行、文本重排）那个位置就是错的，会先错位显示再跳回。
    Instant,
}

impl FirstShowMode {
    /// 配置串 → 枚举。无法识别返回 `None`。
    ///
    /// ⚠ 2026-09-02 由「回落默认档」改为返回 `Option`：`first_show_mode` 现在有了
    /// 「跟随全局」这一档（per-app 规则的 `None`），而全局默认值本身也变成了可配置的
    /// `ui.candidate.first_show_mode`。回落动作因此只应发生在**全局层那一处**
    /// （`Self::from_config(s).unwrap_or_default()`），per-app 层认不出的值退化为
    /// 「没配」＝跟随全局——仍然满足「写错了和没写行为一致」，且不会把一个拼错的值
    /// 悄悄固化成对该应用的显式覆盖。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wait" => Some(Self::Wait),
            "fast" => Some(Self::Fast),
            "instant" => Some(Self::Instant),
            _ => None,
        }
    }
    /// 枚举 → 配置串（写回 compat.toml 用）。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Fast => "fast",
            Self::Instant => "instant",
        }
    }
}

/// 上屏文本里的换行**用什么字符表达**。
///
/// # 为什么必须按应用配，而不能定一个全局正确值
///
/// 「换行」在不同的文本模型里根本不是同一个东西：
///
/// · **富文本模型**（Word / WPS / RichEdit / TOM）——文档存储里的段落边界**就是** CR。
///   LF 在这个模型里不是「另一种换行写法」，而是一个**不构成换行**的控制字符。
///   2026-08-23 真机现场：同一段带换行的文本，记事本与 WPS 正常分段，Word 里每个换行
///   处渲染成一段类似 Tab 的空白（见 7c9da37a）。⇒ 对这类宿主转 CR 不是偏好，而是
///   唯一能成为换行的形式，「保留原样」在它们身上根本不成立。
///
/// · **字节即存储的宿主**（VS Code / 终端 / 浏览器 textarea / Edit 控件）——写进去什么
///   就存什么。这里做任何转换都是在**改写用户的数据**：t112 的「正则直通后行尾从 `\n`
///   变成 `\r`」正是这么来的，而且这种失败是静默的，用户往往很久之后才发现。
///
/// ⇒ 没有哪个值对所有宿主都对，故本项按应用配置（见 [`CommitNewlineRule`]），出厂默认
///   [`Keep`](Self::Keep)：**不知道宿主要什么的时候，不动用户的数据**。漏配一个富文本
///   宿主的代价是用户立刻看得见的显示异常（能反馈、加名单即可），漏配的反方向则是静默
///   改写行尾——可见的失败优于静默的失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewlineStyle {
    /// 原样透传，一个字符都不改。**出厂默认**。
    #[default]
    Keep,
    /// 一律折成单个 CR（`\r`）——Windows 富文本模型的段落分隔符。
    Cr,
    /// 一律折成单个 LF（`\n`）。
    Lf,
    /// 一律折成 CRLF（`\r\n`）。
    Crlf,
}

impl NewlineStyle {
    /// 按本档位改写 `text` 里的换行。
    ///
    /// 三种行尾都认（`\r\n` / 孤立 `\n` / 孤立 `\r`），且 **`\r\n` 算一个换行**——
    /// 逐字符转会把一个换行变成两个（同 `key.type` 的 `split_type_text`）。
    ///
    /// [`Keep`](Self::Keep) 与「文本里压根没有换行」两种情况零拷贝返回。
    pub fn apply<'a>(self, text: &'a str) -> Cow<'a, str> {
        let eol = match self {
            Self::Keep => return Cow::Borrowed(text),
            Self::Cr => "\r",
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        };
        if !text.contains(['\r', '\n']) {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len() + 8);
        let mut it = text.chars().peekable();
        while let Some(ch) = it.next() {
            match ch {
                '\r' => {
                    // CRLF：吃掉紧随的 LF，整体只产出一个换行。
                    if it.peek() == Some(&'\n') {
                        it.next();
                    }
                    out.push_str(eol);
                }
                '\n' => out.push_str(eol),
                c => out.push(c),
            }
        }
        Cow::Owned(out)
    }

    /// 配置串 → 枚举。无法识别返回 `None`——同 [`FirstShowMode::from_config`] 的理由：
    /// per-app 层认不出的值退化为「没配」＝跟随全局，不把拼错的值固化成显式覆盖。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep" => Some(Self::Keep),
            "cr" => Some(Self::Cr),
            "lf" => Some(Self::Lf),
            "crlf" => Some(Self::Crlf),
            _ => None,
        }
    }

    /// 枚举 → 配置串（写回 compat.toml 用）。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Cr => "cr",
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }
}

/// 应用独立的初始中英状态取值。
///
/// 语义是**初始值而非锁定**：进入该应用时套用，用户随后可自由手动切换，
/// 停留在该应用期间不再被改写（详见 `Coordinator::initial_chinese_mode_for`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialMode {
    English,
    Chinese,
}

impl InitialMode {
    /// 配置串 → 枚举。无法识别返回 `None`（＝不干预），不 panic 也不回落到某一档：
    /// 「用户拼错了」与「用户想要英文」是两回事，后者必须是显式写对才成立。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "english" | "en" => Some(Self::English),
            "chinese" | "zh" => Some(Self::Chinese),
            _ => None,
        }
    }
    /// 枚举 → 配置串（写回 compat.toml 用）。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Chinese => "chinese",
        }
    }
    /// 落到 `chinese_mode` / `chinese_punct` 这类布尔状态。
    pub fn is_chinese(self) -> bool {
        matches!(self, Self::Chinese)
    }
    /// 布尔 → 枚举（菜单写盘时把当前状态反写成规则用）。
    pub fn from_chinese(chinese: bool) -> Self {
        if chinese {
            Self::Chinese
        } else {
            Self::English
        }
    }
}

/// 应用独立的候选窗定位方式。
///
/// 与全局 `ui.candidate.position_mode` 同语义，但**按应用覆盖**：少数宿主报的 caret
/// 坐标就是不准（坐标系错、多进程窗口偏移、自绘控件根本不报），全局改成固定又会连累
/// 其余一切正常的应用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidatePositionMode {
    /// 跟随光标（与全局默认同义）。显式写它 = 「这个应用**不要**跟随全局的 fixed」。
    FollowCaret,
    /// 固定在 `candidate_x/candidate_y`（该应用**自己的**一份坐标）。
    Fixed,
}

impl CandidatePositionMode {
    /// 配置串 → 枚举。无法识别返回 `None`（＝跟随全局），理由同
    /// [`FirstShowMode::from_config`]：拼错不该固化成显式覆盖。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "follow_caret" => Some(Self::FollowCaret),
            "fixed" => Some(Self::Fixed),
            _ => None,
        }
    }
    /// 枚举 → 配置串（写回 compat.toml 用）。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::FollowCaret => "follow_caret",
            Self::Fixed => "fixed",
        }
    }
    /// 是否固定位置。
    pub fn is_fixed(self) -> bool {
        matches!(self, Self::Fixed)
    }
}

/// 容错反序列化 `Option<CandidatePositionMode>`：无法识别的值退化为 `None`（＝跟随全局）。
/// ⚠ 不可改用 derive，理由见 [`de_initial_mode`]（一个字段拼错会整份 compat.toml 失效）。
fn de_candidate_position_mode<'de, D>(d: D) -> Result<Option<CandidatePositionMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(
        d,
        "candidate_position_mode",
        CandidatePositionMode::from_config,
    )
}

/// 状态气泡的锚点（C2-33 / GH#148）：`screen_*` = 前台窗口所在显示器的**工作区**，
/// `window_*` = 前台窗口的可见边框。几何换算在 UI 层（wind-ui 的 `anchor_origin`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusAnchor {
    ScreenCenter,
    ScreenTopLeft,
    ScreenTopRight,
    ScreenBottomLeft,
    ScreenBottomRight,
    WindowCenter,
    WindowBottomLeft,
}

impl StatusAnchor {
    /// 全部锚点，顺序即菜单顺序。
    pub const ALL: [StatusAnchor; 7] = [
        Self::ScreenCenter,
        Self::ScreenTopLeft,
        Self::ScreenTopRight,
        Self::ScreenBottomLeft,
        Self::ScreenBottomRight,
        Self::WindowCenter,
        Self::WindowBottomLeft,
    ];

    /// 枚举 → 配置串。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::ScreenCenter => "screen_center",
            Self::ScreenTopLeft => "screen_top_left",
            Self::ScreenTopRight => "screen_top_right",
            Self::ScreenBottomLeft => "screen_bottom_left",
            Self::ScreenBottomRight => "screen_bottom_right",
            Self::WindowCenter => "window_center",
            Self::WindowBottomLeft => "window_bottom_left",
        }
    }

    /// 配置串 → 枚举；认不出返回 `None`。查的是 [`Self::ALL`] + [`Self::as_config`]，值域只有一份。
    pub fn from_config(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|a| a.as_config().eq_ignore_ascii_case(s))
    }
}

/// 状态气泡定位方式：全局 `ui.status.position_mode` 与按应用 `status_position_mode` 共用取值
/// （见 [`STATUS_POSITION_MODES`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusPositionMode {
    /// 跟随光标（出厂默认）。光标坐标不可信时按 [`StatusFallback`] 兜底。
    FollowCaret,
    /// 固定屏幕坐标（全局 `custom_x/y`；按应用 `status_x/y`）。
    Fixed,
    /// 固定在某个锚点，不读光标。
    Anchor(StatusAnchor),
}

/// [`StatusPositionMode`] 的全部配置串，同时是配置注册表 `ui.status.position_mode` 的值域。
pub const STATUS_POSITION_MODES: [&str; 9] = [
    "follow_caret",
    "fixed",
    "screen_center",
    "screen_top_left",
    "screen_top_right",
    "screen_bottom_left",
    "screen_bottom_right",
    "window_center",
    "window_bottom_left",
];

impl StatusPositionMode {
    /// 配置串 → 枚举；认不出返回 `None`（调用方回落：全局取出厂默认，按应用取跟随全局）。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "follow_caret" => Some(Self::FollowCaret),
            "fixed" => Some(Self::Fixed),
            other => StatusAnchor::from_config(other).map(Self::Anchor),
        }
    }
    /// 枚举 → 配置串。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::FollowCaret => "follow_caret",
            Self::Fixed => "fixed",
            Self::Anchor(a) => a.as_config(),
        }
    }
}

/// 状态气泡的兜底位置：只在 `follow_caret` 且光标坐标不可信时生效。全局
/// `ui.status.fallback_position` 与按应用 `status_fallback_position` 共用取值
/// （见 [`STATUS_FALLBACK_POSITIONS`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFallback {
    /// 最近一次有效坐标（出厂默认 = 本功能引入前的行为）。
    Last,
    /// 不显示。
    Hide,
    /// 显示在锚点。
    Anchor(StatusAnchor),
}

/// [`StatusFallback`] 的全部配置串，同时是配置注册表 `ui.status.fallback_position` 的值域。
pub const STATUS_FALLBACK_POSITIONS: [&str; 9] = [
    "last",
    "hide",
    "screen_center",
    "screen_top_left",
    "screen_top_right",
    "screen_bottom_left",
    "screen_bottom_right",
    "window_center",
    "window_bottom_left",
];

impl StatusFallback {
    /// 配置串 → 枚举；认不出返回 `None`。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "last" => Some(Self::Last),
            "hide" => Some(Self::Hide),
            other => StatusAnchor::from_config(other).map(Self::Anchor),
        }
    }
    /// 枚举 → 配置串。
    pub fn as_config(self) -> &'static str {
        match self {
            Self::Last => "last",
            Self::Hide => "hide",
            Self::Anchor(a) => a.as_config(),
        }
    }
}

// 两个带载荷的枚举在 compat.toml 里都是**扁平字符串**（`"screen_center"`），derive 表达不了，
// 故手写：序列化走 `as_config`，反序列化走下面的容错函数（不实现 `Deserialize`，免得有人
// 绕过容错直接 derive 进别的结构）。
impl Serialize for StatusPositionMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_config())
    }
}

impl Serialize for StatusFallback {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_config())
    }
}

/// compat.toml 字符串枚举字段的通用容错：类型写错（非字符串）或值域外的字符串一律回落
/// `None`（= 跟随全局）并 WARN、记入回落清单。理由同 [`de_initial_mode`]；与它不同的是
/// **类型错也吞**（同 [`de_app_schema`]）：`load_file` 没有段级降级。
fn de_opt_str_enum<'de, D, T>(
    d: D,
    field: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = match Option::<toml::Value>::deserialize(d)? {
        None => return Ok(None),
        Some(toml::Value::String(s)) => s,
        Some(other) => {
            let raw = other.to_string();
            tracing::warn!("compat.toml: {field} = {raw} 不是字符串，本项按「跟随全局」处理");
            crate::tolerant_de::record_fallback(&raw);
            return Ok(None);
        }
    };
    match parse(&raw) {
        Some(v) => Ok(Some(v)),
        None => {
            tracing::warn!(
                "compat.toml: {field} = \"{raw}\" 不在取值范围内，本项按「跟随全局」处理"
            );
            crate::tolerant_de::record_fallback(&raw);
            Ok(None)
        }
    }
}

fn de_status_position_mode<'de, D>(d: D) -> Result<Option<StatusPositionMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(d, "status_position_mode", StatusPositionMode::from_config)
}

fn de_status_fallback<'de, D>(d: D) -> Result<Option<StatusFallback>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(d, "status_fallback_position", StatusFallback::from_config)
}

/// 容错反序列化 `Option<InitialMode>`：无法识别的值退化为 `None`（＝不干预）。
///
/// ⚠ 不能直接 `#[derive(Deserialize)]` 让 serde 自己认字符串：`load_file` 解析失败时
/// 返回 `None` 会**整份 compat.toml 静默跳过**，于是一个字段拼错就让该文件里所有应用的
/// 所有规则一起失效，且日志里毫无痕迹。单字段容错把爆炸半径限制在这一个字段内。
fn de_initial_mode<'de, D>(d: D) -> Result<Option<InitialMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(d, "initial_mode / initial_punct", InitialMode::from_config)
}

/// 容错反序列化 `Option<FirstShowMode>`：无法识别的值退化为 `None`（＝跟随全局）。
///
/// ⚠ 与 [`de_initial_mode`] 同理，不能直接让 serde 认字符串：`load_file` 解析失败会
/// **整份 compat.toml 静默跳过**，一个字段拼错就让该文件里所有应用的所有规则一起失效。
fn de_first_show_mode<'de, D>(d: D) -> Result<Option<FirstShowMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(d, "first_show_mode", FirstShowMode::from_config)
}

/// 容错反序列化 `Option<NewlineStyle>`：无法识别的值退化为 `None`（＝跟随全局）。
///
/// ⚠ 与 [`de_first_show_mode`] 同理，不能直接让 serde 认这个枚举：`load_file` 解析失败会
/// **整份 compat.toml 静默跳过**，用户照着 compat.toml 的注释手加一条规则、`style` 拼错
/// 一个字母，就会让该文件里所有应用的所有规则一起失效——而症状与 compat.toml 毫无关联，
/// 极难归因。
fn de_newline_style<'de, D>(d: D) -> Result<Option<NewlineStyle>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    de_opt_str_enum(d, "commit_newline.style", NewlineStyle::from_config)
}

/// [`AppCompatRule::schema`] 里「记住本应用上次用的方案」的保留值。
///
/// `@` 前缀是保留标记空间：方案 id 来自文件名，不以 `@` 开头。以 `@` 开头的值只认这一个，
/// 其余在解析时按未配置处理（见 [`de_app_schema`]）。
pub const APP_SCHEMA_REMEMBER: &str = "@remember";

/// [`AppCompatRule::schema`] 解析后的语义视图，见 [`AppCompatRule::app_schema`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSchema<'a> {
    /// 固定为该方案 id（是否在 `schema.available` 内由协调器按当时的 available 判）。
    Fixed(&'a str),
    /// 记住本应用上次用的方案（记忆表在 `state.toml` 的 `app_schemas`）。
    Remember,
}

/// 容错反序列化 [`AppCompatRule::schema`]：类型写错（`1` / 数组）、空串、`@` 开头但不是
/// [`APP_SCHEMA_REMEMBER`] 的值一律回落 `None`（＝跟随全局）并 WARN。
///
/// ⚠ 理由同 [`de_initial_mode`]：`load_file` 没有段级降级，不在字段上吞掉就是整份
/// compat.toml 静默失效。「id 是否在 `schema.available` 内」**不在这里判**：available 可热重载，
/// 解析层只收字符串，由协调器按当时的 available 校验。
fn de_app_schema<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = match Option::<toml::Value>::deserialize(d)? {
        None => return Ok(None),
        Some(toml::Value::String(s)) => s,
        Some(other) => {
            let raw = other.to_string();
            tracing::warn!("compat.toml: schema = {raw} 不是字符串，本项按「跟随全局」处理");
            crate::tolerant_de::record_fallback(&raw);
            return Ok(None);
        }
    };
    let v = raw.trim();
    if v.is_empty() {
        return Ok(None);
    }
    if v.starts_with('@') && v != APP_SCHEMA_REMEMBER {
        tracing::warn!(
            "compat.toml: schema = \"{v}\" 不是已知标记（只认 {APP_SCHEMA_REMEMBER}），\
             本项按「跟随全局」处理"
        );
        crate::tolerant_de::record_fallback(v);
        return Ok(None);
    }
    Ok(Some(v.to_string()))
}

/// 容错反序列化规则的 `process`：类型写错（`process = 1`）时**本条规则作废**（回落空串，
/// 查找表与各段构建都跳过空进程名），同文件其它规则照常生效。
///
/// 为什么是「本条作废」而不是「整份失败」或「猜一个名字」：`load_file` 没有段级降级，
/// 整份失败 = 所有应用的所有规则一起静默失效；而进程名是规则的主键，认不出就不知道它
/// 该套给谁，唯一安全的答案是不套给任何人。
fn de_process<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<toml::Value>::deserialize(d)? {
        None => Ok(String::new()),
        Some(toml::Value::String(s)) => Ok(s),
        Some(other) => {
            let raw = other.to_string();
            tracing::warn!("compat.toml: process = {raw} 不是字符串，本条规则作废");
            crate::tolerant_de::record_fallback(&raw);
            Ok(String::new())
        }
    }
}

/// 容错反序列化规则的 `comment`（仅文档用途）：类型写错回落空串并 WARN，规则本身照常生效。
fn de_comment<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<toml::Value>::deserialize(d)? {
        None => Ok(String::new()),
        Some(toml::Value::String(s)) => Ok(s),
        Some(other) => {
            let raw = other.to_string();
            tracing::warn!("compat.toml: comment = {raw} 不是字符串，本项忽略");
            crate::tolerant_de::record_fallback(&raw);
            Ok(String::new())
        }
    }
}

/// 单个应用的兼容性规则。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AppCompatRule {
    /// 进程名（不区分大小写），如 "Weixin.exe"。类型写错 ⇒ 本条作废，见 [`de_process`]。
    #[serde(default, deserialize_with = "de_process")]
    pub process: String,
    /// 说明（仅文档用途）。
    #[serde(
        default,
        deserialize_with = "de_comment",
        skip_serializing_if = "String::is_empty"
    )]
    pub comment: String,
    /// 用户层专用：禁用同名的系统规则（合并后该进程视为没有这条规则）。
    /// 条目里的其它字段照常保留，去掉本标记即恢复到禁用前的状态。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub disabled: bool,
    /// 使用 caret rect 的 top 而非 bottom 定位候选窗。
    /// 适用于 GetTextExt 返回的 height 不稳定的 WebView 应用（如微信 Qt 输入框，
    /// height 在 1↔20px 间跳变 → bottom 漂移 ~20px，但 top 始终稳定）。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub caret_use_top: bool,
    /// 拦截「组合期间上报的 caret rect 仍停在上一次组合位置」的宿主。
    ///
    /// 微信（Qt WebView）实测：用户上屏后移动光标（打空格 / 换行）再输入，它在 composition
    /// 期间报的 rect 仍是**上一次组合**的位置，与真实插入点差 136~419px。而 probe 判据 1
    /// （「≠ 上一轮权威坐标 ⇒ 已 reflow」）对此没有判断力——正确答案和陈旧值**都** ≠ 那个
    /// 基准。开启后，probe 若与「组合前宿主主动上报的空闲坐标」矛盾即判为陈旧、不予采信，
    /// 让兜底用那份空闲坐标首显（见 `Coordinator::handle_caret_probe`）。
    ///
    /// ⚠ **必须逐宿主开启，不能做成全局默认**。曾试过按位置关系写一条通用判据，被真机连
    /// 推翻三次（字宽、换行、终端重排）。根因是两类宿主的正确答案**恰好相反**：
    ///   - 微信：probe 陈旧、组合前缓存新   ⇒ 该信缓存
    ///   - WindTerm：probe 是重排后的新位置、缓存已过时 ⇒ 该信 probe
    ///
    /// 同一份位置关系推不出该信谁，任何位置判据都不可能同时答对两者。这是宿主缺陷，
    /// 按宿主处理——与隔壁 `caret_use_top`（同样为微信而加）同一个理由。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub stale_probe_guard: bool,
    /// 把连续的 `TSF_COMPOSITION`（selection 无效、以组合起点降级）→ `TSF_SELECTION`
    /// 识别为同一次布局采样的两阶段结果，禁止后半帧把当前 caret 误重锁成组合起点。
    ///
    /// QQNT 实测：长组合里 reported compStart 始终稳定，但同一按键后的两帧 caret 会在
    /// 组合起点与当前插入点之间跳变。两点距离超过大偏移阈值后，通用重锁逃生阀会把
    /// `composition_start` 改成组合末端，下一次降级帧又改回起点，形成左右闪烁。
    ///
    /// ⚠ **必须逐宿主开启，不能全局信任非零 compStart**：其它宿主已有 compStart 陈旧、
    /// logical/physical 坐标系混用的实测历史；全局改成“有 compStart 就以它为准”会关闭
    /// 原有 caret 大偏移自愈，甚至把候选窗重锁到异常坐标。协调器还会核对来源顺序、
    /// 前帧降级点、reported compStart 与当前锁四者一致，单独一个非零值不构成放行理由。
    ///
    /// 三态：未写 = 继承下层，`Some(true)` = 开，`Some(false)` = 显式关闭。（所有字段的继承
    /// 都由叠加引擎在原始键值上统一处理，见 [`crate::compat_overlay`]，本字段没有特殊待遇。）
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub composition_start_pair_guard: Option<bool>,
    /// 宿主报的组合起点跟着插入点漂移时，把候选窗锚点**钉在首帧起点**而不是让大偏移
    /// 逃生阀跟着走；`None` / `Some(false)` = 不干预，逃生阀照常。
    ///
    /// 判据在协调器侧：宿主给出的组合矩形若退化成插入点（`left >= caret.x`，宽度只有
    /// 光标那么宽），说明它给不出组合范围、报的起点就是插入点本身。
    ///
    /// ⚠ **必须逐宿主开启——这一条上不同宿主的期望行为是相反的，而数据完全相同**：
    /// - WPS 文字（`wps.exe`）实测 compStart 每帧与 caret 同步递增，打字时 caret 持续
    ///   偏离已锁锚点，逃生阀被这份漂移数据一路骗着重锁：
    ///   `(1830,861) → (1615,903) → (2591,861)`，把锚点从真实起点 1830 推到 2591。
    ///   而删除是**逐字**的、每步 15~30px，永远够不到 3 倍行高的阈值——锚点永久卡住，
    ///   候选窗离组合区 868px。这类宿主需要钉住。
    /// - Excel / WPS 表格（`et.exe`）矩形形态**一模一样**（w=1~2、起点等于插入点），
    ///   但它们每输入一个字就换一次 docMgr、候选窗本就该跟着单元格走，靠的正是逃生阀
    ///   把锚点推过去。给它们钉住反而是回归（2026-09-05 实测确认）。
    ///
    /// ⇒ 数据分不出两者，只能按宿主声明。**不要试图改成全局判据**。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub pin_anchor_when_start_drifts: Option<bool>,
    /// 候选窗首显策略；`None` = 不干预，跟随全局 `ui.candidate.first_show_mode`。
    ///
    /// 三档互斥——做成枚举而不是几个 bool：布尔开关可以同时打开，实测就因此出过一次
    /// 「fast 配了却从未生效」（instant 优先、抢先放行，fast 的判据根本没机会跑），
    /// 日志里 630 条试探坐标一条没被消费。互斥语义要由类型保证。
    ///
    /// **必须是 `Option`**（2026-09-02，同 `initial_mode` 的理由）：全局默认档一旦可配，
    /// 「没配过这个应用」与「显式给这个应用配了 fast」就必须能区分，否则用户改了全局默认，
    /// 所有从未配过的应用会照旧被当成显式 fast——per-app 覆盖凭空长出来，且无从撤销。
    #[serde(
        default,
        deserialize_with = "de_first_show_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_show_mode: Option<FirstShowMode>,
    /// 进入本应用时的初始中英状态；`None` = 不干预，沿用全局逻辑。
    ///
    /// **必须是 `Option` 不能是 `bool`**：`#[serde(default)]` 下的 bool 会让所有未配置
    /// 规则的应用都拿到 `false`，等于给全世界配了「初始英文」。
    #[serde(
        default,
        deserialize_with = "de_initial_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub initial_mode: Option<InitialMode>,
    /// 进入本应用时的初始中英标点；`None` = 不干预。
    ///
    /// 显式值**压过** `input.punct.follow_mode` 的推导，否则用户配了它却恰好开着
    /// follow_mode 时会完全无效且无痕迹。
    #[serde(
        default,
        deserialize_with = "de_initial_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub initial_punct: Option<InitialMode>,
    /// 该进程加入 HostRender 白名单（受限宿主如 Win11 开始菜单 SearchHost.exe，候选窗由
    /// 服务进程渲染后经共享内存转交宿主进程内的 DLL 上屏，绕开普通窗口盖不过的 Band 层级）。
    ///
    /// 原为独立的 `config.toml` 全局列表 `compat.host_render_processes`，现并入按进程名
    /// 匹配的兼容规则表——与 `caret_use_top` 等字段同一套查找路径，不再是第二个真相源。
    /// 消费点须按**事件源 PID 直查** `AppCompat::host_render_processes()` 现算的白名单
    /// （`HostRenderManager::is_process_whitelisted`），不得经 `ActiveCompat` 全局焦点槽缓存
    /// ——开始菜单弹出会连带激活兄弟进程，焦点槽会被污染，详见
    /// `docs/redesign/host-render-windows-port.md` §11.2。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub host_render: bool,
    /// 该应用是否启用符号自动配对；`None` = 不干预，沿用全局 `input.auto_pair.*`。
    ///
    /// **必须是 `Option` 不能是 `bool`**：理由同 `initial_mode`——`#[serde(default)]` 下的
    /// bool 会让所有未配置规则的应用都拿到 `false`，等于给全世界关掉了自动配对。
    ///
    /// 典型用途是表格类宿主（Excel / WPS 表格）：配对后要把光标退回两符号之间，而它们在
    /// 「输入态」下把方向键解释成"确认单元格并移动"，光标回退无法实现（TSF `SetSelection`
    /// 路线已实测失败，见 project_pair_caret_tsf_setselection_rejected）。关掉配对是目前
    /// 唯一可行的兼容策略。
    ///
    /// ⚠ 消费点有**三条**，缺一即半截修复：`active_pairs()`（中文标点态）、
    /// `english_pairs_via_pipeline()`（英文标点流水线）、`push_english_pair_config()`
    /// （纯英文模式由 C++ `_englishPairEngine` 独立处理，协调器根本收不到那些键）。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_pair: Option<bool>,
    /// 该应用的密码框是否强制英文；`None` = 跟随全局 `input.password_force_english`（A2-37 / t197）。
    ///
    /// 典型用途是「宿主把普通输入框误报成密码框」：全局开着保护真密码框，只对误报的那个
    /// 应用关掉；反过来也可以在全局关掉时只给某个应用开。
    ///
    /// **必须是 `Option`**（理由同 `initial_mode`）：`None` 与「显式配了恰好等于全局的值」
    /// 是两件事，后者不随全局开关变。
    ///
    /// ★ 消费点必须只有一个判定函数（协调器 `password_force_english_for_pid`）：服务端的
    /// 抑制态与推给 DLL 的吃键门控出自同一处，否则 core.suppress ⊄ C++.suppress ⇒ 密码框丢键。
    ///
    /// 类型写错（`"yes"` / `1`）只让本字段回落 `None`：compat.toml 没有段级降级，
    /// 不容错就是整份文件静默失效，见 [`crate::tolerant_de::tolerant_opt_bool`]。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub password_force_english: Option<bool>,
    /// 该应用使用的输入方案；`None` = 跟随全局（最近一次全局手切的结果 / `schema.active`）
    /// （C0-7 / C3-3，GH#80）。
    ///
    /// - 具体方案 id ⇒ 焦点跨进程切入时固定切到它；
    /// - [`APP_SCHEMA_REMEMBER`] ⇒ 切入时恢复本应用上次用的方案（无记录则用全局）。
    ///
    /// 规则应用内的手切只改本应用的方案，不写 `schema.active`。消费点在协调器
    /// `app_schema_rule`（按当时的 available 校验）。
    /// 读取请走 [`Self::app_schema`]，不要直接比较字符串。
    #[serde(
        default,
        deserialize_with = "de_app_schema",
        skip_serializing_if = "Option::is_none"
    )]
    pub schema: Option<String>,
    /// 该应用的智能符号替换方案；`None` = 沿用全局 `input.symbol.smart_method`。
    ///
    /// `DeleteReplace`（全局默认）依赖对宿主做删改，在 Tabby 一类终端上会出严重错误；
    /// `HoldComposition` 全程不做删改、兼容性更好。两者本就是现成的全局枚举，这里只是
    /// 让它可以按宿主覆盖。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub smart_method: Option<SmartMethod>,
    /// 光标坐标水平校正（dp，96dpi 基准逻辑像素，正=右）。
    ///
    /// 用于宿主报告的 caret 坐标**系统性偏移**的场景（如 Windows Terminal，其它输入法
    /// 同样偏），与主题里的候选窗偏移不是一回事：那个是候选窗相对光标的**布局**（样式层），
    /// 这个修的是光标坐标本身（兼容层），故候选窗/状态气泡/HUD 等所有消费者一并受益。
    ///
    /// 单位是 dp 而非物理像素：宿主上报的 caret 坐标是物理像素，同一份配置若直接按物理
    /// 像素相加，在不同缩放的显示器（尤其多屏混插 100%/150%/200%）上观感会不一致——按
    /// 目标点所在显示器的 DPI 换算成物理像素在协调器侧完成（`apply_caret_compat`），
    /// 本字段本身只管「用户想要的视觉量」。
    ///
    /// 用 `i32` 而非 `Option`：0 就是"不偏移"，语义无歧义，不存在 bool 那种"默认值污染"。
    ///
    /// ⚠ 消费点有**两处**（`apply_focus_caret` / `handle_caret_update`），与 `caret_use_top`
    /// 同层同处；漏一处的症状是「有时生效有时不生效」。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub caret_offset_x: i32,
    /// 光标坐标垂直校正（dp，96dpi 基准逻辑像素，正=下）。语义见 [`Self::caret_offset_x`]。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub caret_offset_y: i32,
    /// 该应用的候选窗定位方式；`None` = 不干预，沿用全局 `ui.candidate.position_mode`。
    ///
    /// **必须是 `Option`**（同 `first_show_mode`）：全局那一档本身可配，「没配过这个应用」
    /// 与「显式给它配了 follow_caret」必须能区分，否则用户把全局改成 fixed 时，所有
    /// 从未配过的应用都会被当成显式 follow_caret，全局设置凭空失效。
    #[serde(
        default,
        deserialize_with = "de_candidate_position_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub candidate_position_mode: Option<CandidatePositionMode>,
    /// 固定模式下该应用**自己**的候选窗落点（内容左上屏幕坐标，物理像素）。
    ///
    /// 为什么每个应用各存一份而不是共用全局那个坐标：合适的落点取决于该应用窗口在屏幕上
    /// 的位置，共用一份等于只有第一个配的应用是对的。
    ///
    /// `(0,0)` = 「已开固定但还没拖过」，由 UI 落到屏幕默认锚点——与全局
    /// `ui.candidate.custom_x/custom_y` 同一套哨兵约定（含 `avoid_unset_sentinel` 的
    /// 1px 规避），两处**必须**同款，否则「拖到主屏左上角后位置记不住」会只在其中一边复发。
    ///
    /// ⚠ 不分显示器：与全局那份保持同一口径。换屏后落点由 `clamp_to_work_area` 兜住，
    /// 不会飞到不可见区域（工具栏/软键盘那套按屏分桶的模型**不适用**——它们是常驻窗口，
    /// 候选窗是临时浮层且随时可以拖）。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub candidate_x: i32,
    /// 固定模式下该应用自己的候选窗落点 Y。语义见 [`Self::candidate_x`]。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub candidate_y: i32,
    /// 该应用的状态气泡定位方式；`None` = 跟随全局 `ui.status.position_mode`（C2-33 / GH#148）。
    ///
    /// 与 [`Self::candidate_position_mode`] 同构：必须是 `Option`（「没配过」与「显式配了
    /// follow_caret」要能区分），且坐标 [`Self::status_x`]/[`Self::status_y`] 与它**同层取**
    /// ——规则配了定位方式就用规则自己的那份坐标，没配才整套回落全局。消费点在协调器
    /// `status_position`。
    #[serde(
        default,
        deserialize_with = "de_status_position_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_position_mode: Option<StatusPositionMode>,
    /// `fixed` 下该应用自己的状态气泡落点 X（内容左上屏幕坐标，物理像素）。`(0,0)` = 已开固定
    /// 但还没摆过，由 UI 落到光标所在屏——与全局 `ui.status.custom_x/y` 同一套哨兵约定。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub status_x: i32,
    /// `fixed` 下该应用自己的状态气泡落点 Y。语义见 [`Self::status_x`]。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_i32",
        skip_serializing_if = "is_zero_i32"
    )]
    pub status_y: i32,
    /// 该应用光标坐标不可信时状态气泡的兜底位置；`None` = 跟随全局 `ui.status.fallback_position`。
    /// 只在（规则或全局解析出的）定位方式为 `follow_caret` 时生效。
    #[serde(
        default,
        deserialize_with = "de_status_fallback",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_fallback_position: Option<StatusFallback>,
    /// 忽略该宿主「关闭输入法」的请求（写 OPENCLOSE / CONVERSION compartment 关 IME）。
    ///
    /// 背景：WinForms 的 `ImeMode.Disable`、WPF 的 `InputMethod.IsInputMethodEnabled=False`
    /// 内部都是 `ImmSetOpenStatus(false)`，经 IMM→TSF 兼容层落到 OPENCLOSE=0。它关的是
    /// **全局中英状态**而非「本控件不接受输入」，于是用户点一次按钮就被切成英文，回到
    /// 文本框还未必能恢复（宿主的恢复链只在相邻控件都由它托管时才闭合）。实测宿主：
    /// X60_Toolbox（WinForms）、beanfun（WPF）。
    ///
    /// ⚠ **不能做成全局默认**：OPENCLOSE 的值语义是我们对宿主说的唯一真话，gvim 一类
    /// 宿主正确依赖它保存/恢复状态（见 project_tsf_openclose_compartment_semantics 的
    /// 四个衍生缺陷）。全局忽略等于回到「钉死为 1」那个已被推翻的年代。
    ///
    /// ⚠ **只拦「关」，且 Ctrl 按住时放行**：系统热键 Ctrl+Space 与宿主关 IME 走的是
    /// **同一条** compartment 通路，无来源可分。唯一可用的区分是伴随按键——系统热键触发
    /// 时 Ctrl 正被按住，宿主自己写则没有任何按键（同款判据已在 C++ 的 CapsLock 联动
    /// 抑制窗用过并实测过）。判据由 DLL 在发消息时一并交代（`MODE_SWITCH_CTRL_HELD`），
    /// 服务端不去猜。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub ignore_host_ime_close: Option<bool>,
    /// 这个宿主读走候选串时，是否当它在自绘、从而收起我们的候选窗。
    /// `None` = 否（默认），`Some(true)` = 是，`Some(false)` = **显式关闭**。
    ///
    /// ⚠ `Some(false)` 与 `None` **不同义**，别按「反正默认也是否」来理解：
    /// ① 用户层不写 = 继承出厂同名规则（出厂已有 `MapleStory.exe = true`），显式写 `false`
    ///    才挡得住；
    /// ② 查表时 `Some(false)` 会短路掉 `process = "*"` 通配的 `or_else` 回落，`None` 不会
    ///    （`a_wildcard_rule_applies_everywhere_and_a_process_rule_wins` 正是靠这点成立）。
    ///
    /// 「宿主接管候选绘制」有两条来源，语气不同，本字段只管后面那条：
    /// - 宿主**声明**接管（`BeginUIElement` 回 `pbShow=FALSE` / UI-less 线程）——这是事实，
    ///   本开关**管不着**它：宿主明说了不要我们的 UI，照弹就是两头画。
    /// - 宿主没声明、却把候选串**读走了**（`UIELEMENT_FLAG_HOST_READS`）——这是推断，
    ///   本开关管的就是它。
    ///
    /// # ⚠ 2026-09-15：默认从「是」反转成「否」
    ///
    /// 立案时（`9d9c24f5`）本字段是单向**关闭**开关，推断默认生效。当时的自陈是
    /// 「『只有真要画的宿主才会来读候选串』至今没有反证样本」——那份日志里 7 个宿主只有
    /// 新枫之谷读过，另外 6 个一次候选都没出过，构不成负对照。
    ///
    /// 反证样本 2026-09-15 在靶机上一次到齐：**Notepad / Illustrator / EverEdit 三个宿主
    /// 都把候选串整串读走（`GetString(0..4)`），却都不画候选窗**。同机新旧两版 DLL 对照
    /// 显示记事本在旧版里同样读了 96 次 ⇒ 读取是这些宿主的常态，不是自绘的标志。
    ///
    /// 默认值改站「不收窗」这一侧，因为**两种误判的代价不对称**：判错成「宿主在画」
    /// （实际没画）⇒ 两个候选框一个都没有、完全不能用，且用户无从知道要来配什么；
    /// 判错成「宿主没画」（实际在画）⇒ 最多多一个框，字照打，配一行即可收。
    ///
    /// 新枫之谷那类宿主（CUAS 的 IMM32 桥替它读走候选、画出旧版系统候选窗）改由出厂
    /// `data/compat.toml` 里一行 `host_drawn_candidates = true` 保住，语义从 opt-out 变
    /// opt-in，那份修复不丢。
    ///
    /// 仍用 `Option<bool>` 而不是裸 `bool`：要区分「没写」与「显式写了 false」，
    /// 且 `#[serde(default)]` 下的裸 bool 会让用户层规则覆盖掉出厂值（理由同 `initial_mode`）。
    ///
    /// 规则查两层：本进程名一条，以及 `process = "*"` 的通配一条（通配现在的用途是反方向
    /// ——某类宿主普遍需要收窗时一行开到全局）。查表见
    /// `Coordinator::uielement_host_draws_by_inference`。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub host_drawn_candidates: Option<bool>,
}

impl AppCompatRule {
    /// [`Self::schema`] 的语义视图；`None` = 未配置（跟随全局）。
    pub fn app_schema(&self) -> Option<AppSchema<'_>> {
        match self.schema.as_deref()? {
            APP_SCHEMA_REMEMBER => Some(AppSchema::Remember),
            id => Some(AppSchema::Fixed(id)),
        }
    }
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

/// 用户层（右键菜单）写入的入口：读用户层 → 交给 `f` 改 → 整份写回。
///
/// 只读写**用户层**：系统层 `data/compat.toml` 不受影响。文件或目录不存在时自动创建。
/// 用户层里记的是**相对系统层的差异**，`f` 拿到已叠加好的系统层（用来判断哪些值冗余、
/// 哪些字段清除时需要 `unset`）以及「系统层是否真的读到了」。
///
/// 解析失败仍然重建（不让菜单卡死，用户手改坏了 TOML 时仍能通过菜单恢复到可用状态），
/// 但先把损坏的原文件留一份 `.bad`：手改坏了的内容往往还能救，静默抹掉就是白丢。
/// 读到写完的全程持有 [`lock_user_compat`]：右键菜单与设置端 RPC 会交错写同一个文件。
fn update_user_raw(
    user_dir: &Path,
    f: impl FnOnce(&mut Raw, &Raw, bool),
) -> Result<(), std::io::Error> {
    let _guard = lock_user_compat();
    let path = user_dir.join(COMPAT_FILE_NAME);
    let mut user = match std::fs::read_to_string(&path) {
        Ok(text) => match parse_raw(&text) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "用户层 compat.toml 解析失败，将重建（原文件留作 {COMPAT_FILE_NAME}.bad）: {e}"
                );
                let _ = std::fs::copy(&path, user_dir.join(format!("{COMPAT_FILE_NAME}.bad")));
                Raw::default()
            }
        },
        Err(_) => Raw::default(),
    };
    let (system, known) = system_layer_for_menu();
    f(&mut user, &system, known);
    std::fs::create_dir_all(user_dir)?;
    write_atomic(&path, &render_raw(USER_COMPAT_HEADER, &user))?;
    Ok(())
}

/// 菜单路径看到的系统层（`data` ⊕ `data_custom`）。第二个返回值 = 系统层是否**确定**：
/// 只有数据目录本身解析不出来时才算未知——目录在、文件缺失或解析失败，运行时看到的就是
/// 「系统层为空」，这是确定的答案。未知时清除字段不能凭「系统里没有」就省掉 `unset`。
#[cfg(not(test))]
fn system_layer_for_menu() -> (Raw, bool) {
    let data = crate::config::Config::data_dir();
    let known = data.is_some();
    let mut raw = Raw::default();
    for dir in [data, crate::config::Config::custom_data_dir()]
        .into_iter()
        .flatten()
    {
        if let Some(layer) = load_raw(&dir.join(COMPAT_FILE_NAME)) {
            raw = overlay_raw(raw, &layer);
        }
    }
    (raw, known)
}

/// 测试构建里菜单路径的系统层由测试显式注入（读真实数据目录会让测试依赖本机环境）；
/// 没注入 = 系统层未知。
#[cfg(test)]
fn system_layer_for_menu() -> (Raw, bool) {
    TEST_MENU_SYSTEM
        .with(|c| c.borrow().clone())
        .unwrap_or((Raw::default(), false))
}

#[cfg(test)]
thread_local! {
    static TEST_MENU_SYSTEM: std::cell::RefCell<Option<(Raw, bool)>> =
        const { std::cell::RefCell::new(None) };
}

/// 测试：在 `f` 期间让右键菜单的写入路径看到指定的系统层。
#[cfg(test)]
pub(crate) fn with_menu_system<R>(system: Raw, known: bool, f: impl FnOnce() -> R) -> R {
    TEST_MENU_SYSTEM.with(|c| *c.borrow_mut() = Some((system, known)));
    let r = f();
    TEST_MENU_SYSTEM.with(|c| *c.borrow_mut() = None);
    r
}

/// 对某进程的 `[[apps]]` 差异行做一批字段编辑。
fn edit_user_apps(
    user_dir: &Path,
    process: &str,
    edits: &[FieldEdit],
) -> Result<(), std::io::Error> {
    update_user_raw(user_dir, |user, system, known| {
        apply_edits::<AppCompatRule>("apps", &system.apps, &mut user.apps, process, edits, known)
    })
}

/// `Some(v)` ⇒ 设置；`None` ⇒ 清除（回到没设置 / 跟随全局，系统层设了它时写 `unset`）。
fn set_or_clear<T: Serialize>(key: &str, v: Option<T>) -> FieldEdit {
    match v.map(|v| serde_json::to_value(v)) {
        Some(Ok(v)) if !v.is_null() => FieldEdit::Set(key.to_string(), v),
        _ => FieldEdit::Clear(key.to_string()),
    }
}

fn set_value(key: &str, v: impl Into<serde_json::Value>) -> FieldEdit {
    FieldEdit::Set(key.to_string(), v.into())
}

fn clear(key: &str) -> FieldEdit {
    FieldEdit::Clear(key.to_string())
}

/// 设置用户层 compat.toml 中指定进程的首显策略（`None` = 清除，回到跟随全局）。
pub fn set_user_first_show_mode(
    user_dir: &Path,
    process: &str,
    mode: Option<FirstShowMode>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("first_show_mode", mode)])
}

/// 设置用户层 compat.toml 中指定进程的初始中英状态（`None` = 清除）。
pub fn set_user_initial_mode(
    user_dir: &Path,
    process: &str,
    mode: Option<InitialMode>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("initial_mode", mode)])
}

/// 设置用户层 compat.toml 中指定进程的初始中英标点（`None` = 清除）。
pub fn set_user_initial_punct(
    user_dir: &Path,
    process: &str,
    mode: Option<InitialMode>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("initial_punct", mode)])
}

/// 设置用户层 compat.toml 中指定进程是否加入 HostRender 白名单。
/// `false` 写成显式覆盖（系统层打开时它才是「从白名单里去掉」；系统层没有时会被当作冗余丢掉）。
pub fn set_user_host_render(
    user_dir: &Path,
    process: &str,
    enabled: bool,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_value("host_render", enabled)])
}

/// 设置用户层 compat.toml 中指定进程的符号自动配对开关（`None` = 清除）。
pub fn set_user_auto_pair(
    user_dir: &Path,
    process: &str,
    enabled: Option<bool>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("auto_pair", enabled)])
}

/// 设置用户层 compat.toml 中指定进程的密码框强制英文（`None` = 清除）。
pub fn set_user_password_force_english(
    user_dir: &Path,
    process: &str,
    enabled: Option<bool>,
) -> Result<(), std::io::Error> {
    edit_user_apps(
        user_dir,
        process,
        &[set_or_clear("password_force_english", enabled)],
    )
}

/// 设置用户层 compat.toml 中指定进程的输入方案（`None` = 清除）。
pub fn set_user_schema(
    user_dir: &Path,
    process: &str,
    schema: Option<String>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("schema", schema)])
}

/// 设置用户层 compat.toml 中指定进程的候选窗定位方式（`None` = 清除，含坐标）。
///
/// ⚠ 清除定位方式时**一并清坐标**：留着一份孤儿坐标，下次用户重新开固定就会跳到
/// 上一次的老位置，而他刚刚才关掉它——「关了又开，位置从哪来的」无从解释。
pub fn set_user_candidate_position_mode(
    user_dir: &Path,
    process: &str,
    mode: Option<CandidatePositionMode>,
) -> Result<(), std::io::Error> {
    let mut edits = vec![set_or_clear("candidate_position_mode", mode)];
    if mode.is_none() {
        edits.push(clear("candidate_x"));
        edits.push(clear("candidate_y"));
    }
    edit_user_apps(user_dir, process, &edits)
}

/// 记住指定进程的候选窗固定落点：**定位方式与坐标一起写**（内容左上，物理像素）。
///
/// 拖动本身就是「这个应用就固定在这儿」的确认，方式与坐标本是一件事，所以一起写；
/// 这也让用户层的这条差异自成一体，不依赖系统层恰好配了什么。
///
/// ⚠ 调用方须先做 `(0,0)` 哨兵规避，与全局那条落盘路径同源；这里不代劳，
/// 因为规避函数在协调器侧（与状态气泡共用），下沉到配置层会变成第二份实现。
pub fn set_user_candidate_fixed_pos(
    user_dir: &Path,
    process: &str,
    x: i32,
    y: i32,
) -> Result<(), std::io::Error> {
    edit_user_apps(
        user_dir,
        process,
        &[
            set_or_clear(
                "candidate_position_mode",
                Some(CandidatePositionMode::Fixed),
            ),
            set_value("candidate_x", x),
            set_value("candidate_y", y),
        ],
    )
}

/// 设置用户层 compat.toml 中指定进程的「忽略宿主关闭输入法」。
///
/// **`None` = 还原（继承内置规则），不是「清除」**：这一项没有全局设置项，菜单第一档写的是
/// 「跟随内置规则」，它的下层就是出厂 compat.toml。写成清除会给出厂打开的宿主写一条 `unset`，
/// 把内置规则整个去掉——与菜单文案相反。显式 `Some(false)` / `Some(true)` 才是用户的覆盖。
pub fn set_user_ignore_host_ime_close(
    user_dir: &Path,
    process: &str,
    enabled: Option<bool>,
) -> Result<(), std::io::Error> {
    let edit = match enabled {
        Some(v) => set_value("ignore_host_ime_close", v),
        None => FieldEdit::Inherit("ignore_host_ime_close".to_string()),
    };
    edit_user_apps(user_dir, process, &[edit])
}

/// 设置用户层 compat.toml 中指定进程的智能符号替换方案（`None` = 清除）。
pub fn set_user_smart_method(
    user_dir: &Path,
    process: &str,
    method: Option<SmartMethod>,
) -> Result<(), std::io::Error> {
    edit_user_apps(user_dir, process, &[set_or_clear("smart_method", method)])
}

/// 设置用户层 compat.toml 中指定进程的光标坐标校正偏移（像素，正=右/下）。
pub fn set_user_caret_offset(
    user_dir: &Path,
    process: &str,
    dx: i32,
    dy: i32,
) -> Result<(), std::io::Error> {
    edit_user_apps(
        user_dir,
        process,
        &[
            set_value("caret_offset_x", dx),
            set_value("caret_offset_y", dy),
        ],
    )
}

/// 设置用户层 compat.toml 中指定进程的状态气泡定位：**方式与坐标一起写**。`None` = 清除
/// （坐标一并清掉）；非 `fixed` 的方式不用坐标，也清掉，免得留一对孤儿坐标、下次开固定跳到老位置。
///
/// ⚠ `fixed` 的坐标由调用方先做 `(0,0)` 哨兵规避（协调器的 `avoid_unset_sentinel`）。
pub fn set_user_status_position(
    user_dir: &Path,
    process: &str,
    mode: Option<StatusPositionMode>,
    x: i32,
    y: i32,
) -> Result<(), std::io::Error> {
    let mut edits = vec![set_or_clear("status_position_mode", mode)];
    if mode == Some(StatusPositionMode::Fixed) {
        edits.push(set_value("status_x", x));
        edits.push(set_value("status_y", y));
    } else {
        edits.push(clear("status_x"));
        edits.push(clear("status_y"));
    }
    edit_user_apps(user_dir, process, &edits)
}

/// 设置用户层 compat.toml 中指定进程的状态气泡兜底位置（`None` = 清除）。
pub fn set_user_status_fallback(
    user_dir: &Path,
    process: &str,
    fallback: Option<StatusFallback>,
) -> Result<(), std::io::Error> {
    edit_user_apps(
        user_dir,
        process,
        &[set_or_clear("status_fallback_position", fallback)],
    )
}

/// 「初始模式作用域」规则：某进程的 per-app **初始模式**只在哪些**窗口类**上重算。
///
/// 为什么需要它：per-app 规则（`[[apps]]`）的身份是**进程映像名**，而 `explorer.exe`
/// 一个名字同时承载语义相反的两类焦点——桌面是「停留型」，任务栏 / Alt+Tab / 任务视图 /
/// 溢出区 / 资源管理器是「路过或另有用途」。用户为桌面配 `initial_mode = "english"`
/// 时，那些窗口会被一并命中，而它们恰是每次切换应用的必经之路。
/// 实测样本（2026-08-18）：非桌面焦点 169 次、桌面 12 次，**14:1**。
///
/// ★★★ **判据方向必须是白名单「规则在哪生效」，不能是黑名单「哪些是过渡窗口」**。
/// 本段初版正是黑名单（`shell_transient`，列出任务栏那几个类），当天即被实测推翻：
///   17:24:08.579  Client connected to bridge pipe        ← explorer 新起一个 TSF 连接
///   17:24:08.581  handle_focus_gained token=…0002  caret src=last_known
///   17:24:08.583  语言栏图标已发布 label=英             ← 闪
/// 新连接的头一个 focus_gained 拿不到窗口类（TSF 此刻还没有 view，连 caret 都退到
/// last_known），空类名不在黑名单里 ⇒ 判成「不是过渡窗口」⇒ 套上 explorer 的英文规则。
/// 黑名单还有第二个失效面：Windows 每个版本都在新增 XAML 岛窗口类，漏一个就套错一次。
/// 反过来做白名单，两种失效同时消失——「不知道在哪」与「新出现的类」都自动落在作用域外，
/// 也就是**保持现状**，而保持现状恰是这两种情况下唯一安全的答案。
///
/// 独立成段而不是 `[[apps]]` 的字段：它描述的是「规则在哪些窗口上生效」，是进程规则的
/// 适用范围而不是又一个行为开关；分开写也让 `explorer.exe` 的作用域清单与它的 `[[apps]]`
/// 行为规则各自叠加、互不牵连。
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct InitialModeScopeRule {
    /// 进程映像名（不区分大小写），如 `explorer.exe`。缺失或类型写错 ⇒ 本条作废（见 [`de_process`]）。
    #[serde(default, deserialize_with = "de_process")]
    pub process: String,
    /// 说明（仅文档用途）。与 `AppCompatRule::comment` 同理**必须存在于结构体里**：
    /// serde 默认静默忽略未知字段，只声明在 TOML 注释里的话，用户层写回时会被丢掉。
    #[serde(
        default,
        deserialize_with = "de_comment",
        skip_serializing_if = "String::is_empty"
    )]
    pub comment: String,
    /// 用户层专用：禁用同名的系统规则（合并后该进程视为没有这条规则）。
    /// 条目里的其它字段照常保留，去掉本标记即恢复到禁用前的状态。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub disabled: bool,
    /// 该进程下**允许重算初始模式**的顶层窗口类名（不区分大小写）。
    /// 空清单 = 该进程的初始模式规则在任何窗口上都不重算。
    #[serde(default)]
    pub classes: Vec<String>,
}

/// 按应用指定上屏文本的换行形式（值域与理由见 [`NewlineStyle`]）。
///
/// **为什么独立成段而不是做成 `[[apps]]` 的字段**
///
/// 与合并语义无关（所有段都是逐字段叠加，没有哪个字段享有特殊待遇），理由只有两条：
/// 1. 本项与候选窗定位毫无关系，`AppCompatRule` 已有 20+ 字段，再塞一个文本项只会更难读；
/// 2. 本项不参与右键菜单的写回，放进 `[[apps]]` 会让菜单路径的编辑面多一个不相干的字段。
///
/// 代价：同一个进程的配置分散在两个 section。将来再加 per-app 项时，「开新段」与「加字段」
/// 之间没有硬判据，按上面两条逐条对照。
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
pub struct CommitNewlineRule {
    /// 进程映像名（不区分大小写），如 `WINWORD.EXE`。类型写错 ⇒ 本条作废（见 [`de_process`]）。
    #[serde(default, deserialize_with = "de_process")]
    pub process: String,
    /// 说明（仅文档用途）。与 [`AppCompatRule::comment`] 同理**必须存在于结构体里**：
    /// serde 默认静默忽略未知字段，只声明在 TOML 注释里的话，用户层写回时会被丢掉。
    #[serde(
        default,
        deserialize_with = "de_comment",
        skip_serializing_if = "String::is_empty"
    )]
    pub comment: String,
    /// 用户层专用：禁用同名的系统规则（合并后该进程视为没有这条规则）。
    /// 条目里的其它字段照常保留，去掉本标记即恢复到禁用前的状态。
    #[serde(
        default,
        deserialize_with = "crate::tolerant_de::tolerant_bool",
        skip_serializing_if = "is_false"
    )]
    pub disabled: bool,
    /// 该应用上屏时换行用什么字符表达。**认不出的值退化为 `None`＝跟随全局**，
    /// 而不是让整份文件解析失败——理由见 [`de_newline_style`]。
    #[serde(default, deserialize_with = "de_newline_style")]
    pub style: Option<NewlineStyle>,
}

/// 所有应用兼容性规则 + 运行时查找表。
#[derive(Debug, Clone, Default)]
pub struct AppCompat {
    apps: Vec<AppCompatRule>,
    /// 小写进程名 → `apps` 下标。
    lookup: HashMap<String, usize>,
    /// 小写进程名 → 该进程允许重算初始模式的窗口类名集合（小写）。
    /// **进程不在表内 = 不受限制**（绝大多数应用走这条路，零行为变化）。
    mode_scope: HashMap<String, std::collections::HashSet<String>>,
    /// 小写进程名 → 该进程的上屏换行形式。
    /// **进程不在表内 = 跟随全局**（绝大多数应用走这条路）。
    commit_newline: HashMap<String, NewlineStyle>,
}

/// 序列化中间体：承载 TOML 的两个顶层数组表，避免把 `lookup` 暴露给 TOML。
///
/// `pub(crate)` 而非模块私有，只为让 `value_domain_guard` 那条守门元测试够得着——
/// 它要遍历本结构体的每个字符串字段逐个投毒，而集成测试只看得见 pub API。
/// ⛔ 不要因此把它当成对外类型：`compat.toml` 的读写入口仍只有 `load_file` /
/// `render_user_compat`。
#[cfg(test)]
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub(crate) struct AppCompatFile {
    #[serde(default)]
    pub(crate) apps: Vec<AppCompatRule>,
    /// ⚠ 用户层 compat.toml 由右键菜单**整份重写**（`render_user_compat`），本字段
    /// 必须一并渲染回去，否则用户写的覆盖会在下一次菜单开关时被静默删掉。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) initial_mode_scope: Vec<InitialModeScopeRule>,
    /// ⚠ 与 `initial_mode_scope` 同理：`render_user_compat` 是整份重写，本字段漏了
    /// 用户手写的 `[[commit_newline]]` 就会在下一次菜单开关时静默消失。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) commit_newline: Vec<CommitNewlineRule>,
}

impl AppCompat {
    /// 从一组规则构建（含查找表）。作用域清单为空 ⇒ 所有进程都不受限。
    pub fn from_rules(apps: Vec<AppCompatRule>) -> Self {
        Self::from_parts(apps, Vec::new())
    }

    /// 从两段规则构建（含查找表）。换行清单为空 ⇒ 所有进程都跟随全局。
    pub fn from_parts(apps: Vec<AppCompatRule>, scope: Vec<InitialModeScopeRule>) -> Self {
        let mut c = AppCompat {
            apps,
            lookup: HashMap::new(),
            mode_scope: HashMap::new(),
            commit_newline: HashMap::new(),
        };
        c.build_lookup();
        c.build_mode_scope(scope);
        c
    }

    /// 补上 `[[commit_newline]]` 那一段。
    ///
    /// 做成链式而不是给 [`Self::from_parts`] 加第三个参数：那个签名已有跨 crate 的
    /// 调用方，加参数要改一圈与本特性无关的代码。
    pub fn with_commit_newline(mut self, rules: Vec<CommitNewlineRule>) -> Self {
        self.commit_newline = rules
            .into_iter()
            .filter(|r| !r.process.is_empty())
            // `style` 没写或认不出 ⇒ 这条规则什么都没说，等价于不存在（跟随全局）。
            .filter_map(|r| Some((r.process.to_ascii_lowercase(), r.style?)))
            .collect();
        self
    }

    /// 该进程上屏时换行用什么表达；**没配返回 `None` = 跟随全局**。
    ///
    /// 返回 `Option` 而不是回落到 [`NewlineStyle::default()`]：回落动作只应发生在
    /// 全局层那一处（同 [`FirstShowMode::from_config`] 的取舍），否则 per-app 的
    /// 「没配」与「显式配成默认档」就分不开了。
    pub fn commit_newline_for(&self, process_name: &str) -> Option<NewlineStyle> {
        if process_name.is_empty() {
            return None;
        }
        self.commit_newline
            .get(&process_name.to_ascii_lowercase())
            .copied()
    }

    /// 该焦点窗口是否落在「允许重算 per-app 初始模式」的作用域内。
    ///
    /// 返回 false 表示调用方应**保持现状**（不重算初始模式、不推进模式归属）。
    ///
    /// 三种取值来源，必须一并理解：
    /// - 该进程**没配**作用域 ⇒ true。绝大多数应用走这条，行为与本机制引入前完全一致。
    /// - 配了且窗口类命中 ⇒ true。
    /// - 配了但窗口类不命中，**含窗口类为空** ⇒ false。空类名是「拿不到窗口标识」，
    ///   不是「窗口不在清单里」，但对本函数要回答的问题，两者的正确答案都是「别重算」。
    ///   曾把空类名当作「不是过渡窗口」放行，实测当场闪英，见 `InitialModeScopeRule` 注释。
    pub fn initial_mode_applies_to_window(&self, process_name: &str, window_class: &str) -> bool {
        match self.mode_scope.get(&process_name.to_ascii_lowercase()) {
            None => true,
            Some(set) => {
                !window_class.is_empty() && set.contains(&window_class.to_ascii_lowercase())
            }
        }
    }

    /// 按进程名（不区分大小写）查规则，未匹配返回 None。
    pub fn get_rule(&self, process_name: &str) -> Option<&AppCompatRule> {
        self.lookup
            .get(&process_name.to_ascii_lowercase())
            .map(|&i| &self.apps[i])
    }

    /// 现算 HostRender 白名单：所有 `host_render = true` 的进程名（原始大小写）。
    ///
    /// 供 `HostRenderManager::set_whitelist` 消费；调用方须按事件源 PID 直查，
    /// 不得经 `ActiveCompat` 全局焦点槽缓存，理由见 [`AppCompatRule::host_render`]。
    pub fn host_render_processes(&self) -> Vec<String> {
        self.apps
            .iter()
            .filter(|r| r.host_render)
            .map(|r| r.process.clone())
            .collect()
    }

    fn build_lookup(&mut self) {
        self.lookup = self
            .apps
            .iter()
            .enumerate()
            // 空进程名 = 作废的规则（`process` 类型写错，见 `de_process`），不套给任何人。
            .filter(|(_, r)| !r.process.is_empty())
            .map(|(i, r)| (r.process.to_ascii_lowercase(), i))
            .collect();
    }

    fn build_mode_scope(&mut self, rules: Vec<InitialModeScopeRule>) {
        self.mode_scope = rules
            .into_iter()
            .filter(|r| !r.process.is_empty())
            .map(|r| {
                (
                    r.process.to_ascii_lowercase(),
                    r.classes
                        .iter()
                        .map(|c| c.to_ascii_lowercase())
                        .collect::<std::collections::HashSet<_>>(),
                )
            })
            .collect();
    }

    /// 加载兼容规则：系统层（`{data_dir}/compat.toml`）+ 定制层
    /// （`data_custom/compat.toml`）+ 用户层覆盖（`{user_dir}/compat.toml`）。
    /// 任一文件缺失/解析失败均静默跳过。
    ///
    /// 定制层由 [`crate::config::Config::custom_data_dir`] 决定（清单在场才有），不经参数传入：本函数
    /// 有五个跨 crate 调用点，而定制层的位置是进程级事实、不随调用方的 `data_dir` 变。
    /// 测试需要指定定制层时用 [`Self::load_layered`]。
    pub fn load(data_dir: Option<&Path>, user_dir: Option<&Path>) -> Self {
        Self::load_layered(
            data_dir,
            crate::config::Config::custom_data_dir().as_deref(),
            user_dir,
        )
    }

    /// 三层显式加载，层序 `data < data_custom < user`：每一层都是相对下一层的差异，
    /// 在**原始键值**上逐字段叠加（见 [`crate::compat_overlay`]），最后一次性变成运行时结构体。
    ///
    /// 三段（`[[apps]]` / `[[initial_mode_scope]]` / `[[commit_newline]]`）各自独立叠加，
    /// 互不牵连：为某进程写 `[[apps]]` 规则不会让更低层给它配的作用域或换行规则消失。
    pub fn load_layered(
        data_dir: Option<&Path>,
        custom_dir: Option<&Path>,
        user_dir: Option<&Path>,
    ) -> Self {
        let mut raw = Raw::default();
        for dir in [data_dir, custom_dir, user_dir].into_iter().flatten() {
            if let Some(layer) = load_raw(&dir.join(COMPAT_FILE_NAME)) {
                raw = overlay_raw(raw, &layer);
            }
        }
        Self::from_parts(
            materialize::<AppCompatRule>(&raw.apps),
            materialize::<InitialModeScopeRule>(&raw.initial_mode_scope),
        )
        .with_commit_newline(materialize::<CommitNewlineRule>(&raw.commit_newline))
    }
}

/// 读一层 compat.toml 的原始键值；文件不存在返回 `None`（不告警），读取 / 解析失败也返回
/// `None` 但**留 WARN**：整份跳过意味着这一层所有规则一起失效，必须在日志里留下痕迹。
pub(crate) fn load_raw(path: &Path) -> Option<Raw> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("compat.toml 读取失败，整份跳过: {}: {e}", path.display());
            }
            return None;
        }
    };
    match parse_raw(&text) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!("compat.toml 解析失败，整份跳过: {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod commit_newline_rule_tests {
    use super::*;

    fn rule(process: &str, style: NewlineStyle) -> CommitNewlineRule {
        CommitNewlineRule {
            process: process.into(),
            comment: String::new(),
            disabled: false,
            style: Some(style),
        }
    }

    /// 没配 = 跟随全局（`None`），不是「回落到 Keep」——两者在上层的处理不同。
    #[test]
    fn unlisted_process_follows_global() {
        let c =
            AppCompat::default().with_commit_newline(vec![rule("WINWORD.EXE", NewlineStyle::Cr)]);
        assert_eq!(c.commit_newline_for("winword.exe"), Some(NewlineStyle::Cr));
        assert_eq!(c.commit_newline_for("Code.exe"), None);
        assert_eq!(c.commit_newline_for(""), None, "空进程名不该命中任何规则");
    }

    /// 进程名大小写不敏感——两侧都要归一，只归一一侧是常见的半拉子实现。
    #[test]
    fn process_match_is_case_insensitive() {
        let c =
            AppCompat::default().with_commit_newline(vec![rule("WINWORD.EXE", NewlineStyle::Cr)]);
        assert_eq!(c.commit_newline_for("WinWord.Exe"), Some(NewlineStyle::Cr));
        let c =
            AppCompat::default().with_commit_newline(vec![rule("winword.exe", NewlineStyle::Cr)]);
        assert_eq!(c.commit_newline_for("WINWORD.EXE"), Some(NewlineStyle::Cr));
    }

    /// ★★★ 拼错的 `style` 不得让整份 compat.toml 失效。
    ///
    /// `load_file` 是 `toml::from_str(..).ok()` —— 解析失败**静默跳过整份文件**，该层的
    /// `[[apps]]`、`[[initial_mode_scope]]`、`[[commit_newline]]` 会一起消失。而 compat.toml
    /// 的注释明确邀请用户「照下面的格式加一条即可」，拼错一个字母就丢掉全部应用兼容规则、
    /// 且症状与 compat.toml 毫无关联。同一形态的既有测试见
    /// `unknown_mode_in_toml_degrades_to_follow_global`。
    #[test]
    fn unknown_style_in_toml_degrades_to_follow_global() {
        let toml = r#"
            [[apps]]
            process = "Foo.exe"
            caret_use_top = true

            [[commit_newline]]
            process = "Bar.exe"
            style = "rn"
        "#;
        let file: AppCompatFile = toml::from_str(toml).expect("style 拼错不得让整份文件解析失败");
        assert_eq!(file.commit_newline[0].style, None, "认不出 = 跟随全局");
        assert!(
            file.apps[0].caret_use_top,
            "别的段必须不受牵连——这正是整份静默跳过最致命的地方"
        );

        // 缺字段同理：只写 process、不写 style 也不能炸。
        let toml2 = r#"
            [[commit_newline]]
            process = "Baz.exe"
        "#;
        let file2: AppCompatFile = toml::from_str(toml2).expect("缺 style 不得解析失败");
        assert_eq!(file2.commit_newline[0].style, None);

        // 认不出/没写的条目进不了查找表，等于这条规则不存在。
        let c = AppCompat::default().with_commit_newline(file.commit_newline);
        assert_eq!(c.commit_newline_for("bar.exe"), None);
    }
}

#[cfg(test)]
mod newline_style_tests {
    use super::*;

    /// 出厂默认必须是「不动用户的数据」。改这一条等于改产品语义，不是调参。
    #[test]
    fn default_is_keep() {
        assert_eq!(NewlineStyle::default(), NewlineStyle::Keep);
    }

    /// `keep` 一个字符都不许改——混合行尾也原样留着。
    #[test]
    fn keep_passes_everything_through() {
        let cr = char::from_u32(13).unwrap();
        let lf = char::from_u32(10).unwrap();
        let src = format!("a{cr}{lf}b{lf}c{cr}d");
        assert_eq!(NewlineStyle::Keep.apply(&src), src);
    }

    /// 三种行尾都要认，且 **CRLF 算一个换行**：逐字符转会把一个换行变成两个。
    #[test]
    fn every_eol_form_folds_into_one() {
        let cr = char::from_u32(13).unwrap();
        let lf = char::from_u32(10).unwrap();
        let src = format!("a{cr}{lf}b{lf}c{cr}d");

        assert_eq!(
            NewlineStyle::Cr.apply(&src),
            format!("a{cr}b{cr}c{cr}d"),
            "三处换行都该折成单个 CR"
        );
        assert_eq!(NewlineStyle::Lf.apply(&src), format!("a{lf}b{lf}c{lf}d"));
        assert_eq!(
            NewlineStyle::Crlf.apply(&src),
            format!("a{cr}{lf}b{cr}{lf}c{cr}{lf}d"),
            "已经是 CRLF 的那处不能翻倍"
        );
    }

    /// 连续换行是**多个**换行，不许合并——空行是用户的内容。
    #[test]
    fn consecutive_newlines_are_not_merged() {
        let cr = char::from_u32(13).unwrap();
        let lf = char::from_u32(10).unwrap();
        let src = format!("a{lf}{lf}{lf}b");
        assert_eq!(NewlineStyle::Cr.apply(&src), format!("a{cr}{cr}{cr}b"));
        assert_eq!(
            NewlineStyle::Crlf.apply(&format!("a{cr}{lf}{cr}{lf}b")),
            format!("a{cr}{lf}{cr}{lf}b"),
            "两个 CRLF 是两个换行，不是一个"
        );
    }

    /// 首尾换行不能丢，也不该产出多余内容。
    #[test]
    fn leading_and_trailing_newlines_survive() {
        let cr = char::from_u32(13).unwrap();
        let lf = char::from_u32(10).unwrap();
        assert_eq!(
            NewlineStyle::Cr.apply(&format!("{lf}a{lf}")),
            format!("{cr}a{cr}")
        );
    }

    /// 上屏文本绝大多数不含换行（逐字上屏），这条路必须零拷贝。
    #[test]
    fn text_without_newline_is_borrowed() {
        for style in [
            NewlineStyle::Keep,
            NewlineStyle::Cr,
            NewlineStyle::Lf,
            NewlineStyle::Crlf,
        ] {
            assert!(
                matches!(style.apply("你好世界"), Cow::Borrowed(_)),
                "{style:?}: 无换行的文本不该产生分配"
            );
        }
    }

    /// `keep` 即使文本里有换行也不拷贝。
    #[test]
    fn keep_never_allocates() {
        let lf = char::from_u32(10).unwrap();
        let src = format!("a{lf}b");
        assert!(matches!(NewlineStyle::Keep.apply(&src), Cow::Borrowed(_)));
    }

    /// 多字节字符紧贴换行时切片边界必须落在字符边界上（否则 panic）。
    #[test]
    fn multibyte_around_newline() {
        let lf = char::from_u32(10).unwrap();
        let cr = char::from_u32(13).unwrap();
        let src = format!("中{lf}文");
        assert_eq!(NewlineStyle::Cr.apply(&src), format!("中{cr}文"));
    }

    /// 配置串往返：认得的值原样回来，认不得的退化为「没配」而不是某个默认档。
    #[test]
    fn config_string_roundtrip() {
        for style in [
            NewlineStyle::Keep,
            NewlineStyle::Cr,
            NewlineStyle::Lf,
            NewlineStyle::Crlf,
        ] {
            assert_eq!(NewlineStyle::from_config(style.as_config()), Some(style));
        }
        assert_eq!(
            NewlineStyle::from_config("  CRLF "),
            Some(NewlineStyle::Crlf)
        );
        assert_eq!(
            NewlineStyle::from_config("rn"),
            None,
            "拼错的值必须是 None（跟随全局），不能悄悄固化成显式覆盖"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个新增 per-app 字段的解析。`auto_pair` / `smart_method` 是 `Option`，
    /// **未配置必须是 `None` 而不是 `Some(false)`/`Some(默认值)`**——那是「跟随全局」
    /// 与「显式关掉」的分界，退化成 bool 就等于给所有未配规则的应用都关掉了功能。
    #[test]
    fn parse_per_app_pair_symbol_and_offset() {
        let toml = r#"
            [[apps]]
            process = "EXCEL.EXE"
            auto_pair = false
            caret_offset_x = -2
            caret_offset_y = 3

            [[apps]]
            process = "Tabby.exe"
            smart_method = "hold_composition"

            [[apps]]
            process = "plain.exe"
            caret_use_top = true

            [[apps]]
            process = "QQ.exe"
            composition_start_pair_guard = true
        "#;
        let compat = AppCompat::from_rules(toml::from_str::<AppCompatFile>(toml).unwrap().apps);

        let excel = compat.get_rule("excel.exe").unwrap();
        assert_eq!(excel.auto_pair, Some(false));
        assert_eq!((excel.caret_offset_x, excel.caret_offset_y), (-2, 3));
        assert_eq!(excel.smart_method, None, "未配 = 跟随全局");

        let tabby = compat.get_rule("TABBY.EXE").unwrap();
        assert_eq!(tabby.smart_method, Some(SmartMethod::HoldComposition));
        assert_eq!(tabby.auto_pair, None, "未配 = 跟随全局");

        // 只配了别的字段的规则，三个新字段必须全部是"不干预"，不能被默认值污染。
        let plain = compat.get_rule("plain.exe").unwrap();
        assert_eq!(plain.auto_pair, None);
        assert_eq!(plain.smart_method, None);
        assert_eq!((plain.caret_offset_x, plain.caret_offset_y), (0, 0));

        let qq = compat.get_rule("qq.exe").unwrap();
        assert_eq!(qq.composition_start_pair_guard, Some(true));
    }

    /// `password_force_english` 三态解析：未配 = `None`（跟随全局），显式 true/false 原样保留。
    #[test]
    fn parse_password_force_english_tristate() {
        let toml = r#"
            [[apps]]
            process = "misreport.exe"
            password_force_english = false

            [[apps]]
            process = "strict.exe"
            password_force_english = true

            [[apps]]
            process = "plain.exe"
            caret_use_top = true
        "#;
        let compat = AppCompat::from_rules(toml::from_str::<AppCompatFile>(toml).unwrap().apps);
        assert_eq!(
            compat
                .get_rule("MISREPORT.EXE")
                .unwrap()
                .password_force_english,
            Some(false)
        );
        assert_eq!(
            compat
                .get_rule("strict.exe")
                .unwrap()
                .password_force_english,
            Some(true)
        );
        assert_eq!(
            compat.get_rule("plain.exe").unwrap().password_force_english,
            None,
            "未配 = 跟随全局，不能被 bool 默认值污染"
        );
    }

    /// 类型写错只让本字段回落 `None`，**不得让整份 compat.toml 失效**：`load_file` 没有
    /// 段级降级，一个 `"yes"` 就会连带所有应用的所有规则静默消失。
    #[test]
    fn password_force_english_wrong_type_does_not_sink_the_file() {
        for bad in [r#""yes""#, "1", "[true]"] {
            let toml = format!(
                r#"
                [[apps]]
                process = "typo.exe"
                password_force_english = {bad}

                [[apps]]
                process = "other.exe"
                auto_pair = false
                "#
            );
            let file = toml::from_str::<AppCompatFile>(&toml)
                .unwrap_or_else(|e| panic!("{bad}: 类型写错不得让整份失败：{e}"));
            let compat = AppCompat::from_rules(file.apps);
            assert_eq!(
                compat.get_rule("typo.exe").unwrap().password_force_english,
                None,
                "{bad}: 认不出 = 没配过"
            );
            assert_eq!(
                compat.get_rule("other.exe").unwrap().auto_pair,
                Some(false),
                "{bad}: 同文件其它规则必须照常生效"
            );
        }
    }

    // ── 状态气泡定位（C2-33 / GH#148）────────────────────────────────────────────

    /// 两个值域表与解析/回写同源：表里每一项都能解析、且回写成同一个串。
    #[test]
    fn status_value_tables_roundtrip() {
        for v in STATUS_POSITION_MODES {
            let m = StatusPositionMode::from_config(v).unwrap_or_else(|| panic!("{v}"));
            assert_eq!(m.as_config(), v);
        }
        for v in STATUS_FALLBACK_POSITIONS {
            let f = StatusFallback::from_config(v).unwrap_or_else(|| panic!("{v}"));
            assert_eq!(f.as_config(), v);
        }
        for a in StatusAnchor::ALL {
            assert!(STATUS_POSITION_MODES.contains(&a.as_config()));
            assert!(STATUS_FALLBACK_POSITIONS.contains(&a.as_config()));
        }
        // 两张表只有前两档不同：定位方式没有 last/hide，兜底没有 follow_caret/fixed。
        assert_eq!(StatusPositionMode::from_config("last"), None);
        assert_eq!(StatusFallback::from_config("fixed"), None);
    }

    #[test]
    fn status_rule_fields_parse() {
        let c = parse_rules(
            "[[apps]]\nprocess = \"ai.exe\"\nstatus_position_mode = \"window_bottom_left\"\n\
             status_fallback_position = \"screen_top_right\"\n\n\
             [[apps]]\nprocess = \"fx.exe\"\nstatus_position_mode = \"fixed\"\n\
             status_x = 120\nstatus_y = -40\n",
        );
        let ai = c.get_rule("ai.exe").unwrap();
        assert_eq!(
            ai.status_position_mode,
            Some(StatusPositionMode::Anchor(StatusAnchor::WindowBottomLeft))
        );
        assert_eq!(
            ai.status_fallback_position,
            Some(StatusFallback::Anchor(StatusAnchor::ScreenTopRight))
        );
        let fx = c.get_rule("fx.exe").unwrap();
        assert_eq!(fx.status_position_mode, Some(StatusPositionMode::Fixed));
        assert_eq!((fx.status_x, fx.status_y), (120, -40));
        assert_eq!(fx.status_fallback_position, None);
    }

    /// 锚点拼错 / fallback 拼错 / 类型写错：只让该项回落「跟随全局」，同条规则的其它字段与
    /// 同文件其它规则照常生效。
    #[test]
    fn status_rule_typos_only_drop_that_field() {
        for (field, bad) in [
            ("status_position_mode", r#""screen_centre""#),
            ("status_position_mode", "3"),
            ("status_position_mode", r#""last""#),
            ("status_fallback_position", r#""hidden""#),
            ("status_fallback_position", "true"),
            ("status_fallback_position", r#""fixed""#),
        ] {
            let text = format!(
                "[[apps]]\nprocess = \"typo.exe\"\n{field} = {bad}\nstatus_x = 5\n\
                 initial_mode = \"english\"\n\n\
                 [[apps]]\nprocess = \"other.exe\"\nstatus_position_mode = \"screen_center\"\n"
            );
            let file = toml::from_str::<AppCompatFile>(&text)
                .unwrap_or_else(|e| panic!("{field} = {bad}: 不得整份失败：{e}"));
            let c = AppCompat::from_rules(file.apps);
            let typo = c.get_rule("typo.exe").unwrap();
            assert_eq!(typo.status_position_mode, None, "{field} = {bad}");
            assert_eq!(typo.status_fallback_position, None, "{field} = {bad}");
            assert_eq!(typo.status_x, 5, "{field} = {bad}: 同条其它字段照常");
            assert_eq!(typo.initial_mode, Some(InitialMode::English));
            assert_eq!(
                c.get_rule("other.exe").unwrap().status_position_mode,
                Some(StatusPositionMode::Anchor(StatusAnchor::ScreenCenter)),
                "{field} = {bad}: 同文件其它规则必须照常生效"
            );
        }
    }

    /// 最初的现场：`auto_pair = "yes"` 曾让整份 compat.toml 静默失效。
    #[test]
    fn auto_pair_wrong_type_does_not_sink_the_file() {
        let file = toml::from_str::<AppCompatFile>(
            "[[apps]]\nprocess = \"et.exe\"\nauto_pair = \"yes\"\n\n\
             [[apps]]\nprocess = \"other.exe\"\ncaret_use_top = true\n",
        )
        .expect("auto_pair 写错不得让整份失败");
        let compat = AppCompat::from_rules(file.apps);
        assert_eq!(compat.get_rule("et.exe").unwrap().auto_pair, None);
        assert!(compat.get_rule("other.exe").unwrap().caret_use_top);
    }

    /// 另两段（`[[initial_mode_scope]]` / `[[commit_newline]]`）的 `process` / `comment` 同理：
    /// 类型写错只让那一条作废（或那一项忽略），不得让整份 compat.toml 失效。
    #[test]
    fn side_sections_process_wrong_type_voids_only_that_rule() {
        let file = toml::from_str::<AppCompatFile>(
            "[[apps]]\nprocess = \"other.exe\"\nauto_pair = false\n\n\
             [[initial_mode_scope]]\nprocess = 1\nclasses = [\"X\"]\n\n\
             [[initial_mode_scope]]\ncomment = 2\nprocess = \"explorer.exe\"\nclasses = [\"Progman\"]\n\n\
             [[commit_newline]]\nprocess = true\nstyle = \"cr\"\n\n\
             [[commit_newline]]\nprocess = \"WINWORD.EXE\"\ncomment = [1]\nstyle = \"cr\"\n",
        )
        .expect("process / comment 类型写错不得让整份失败");
        let compat = AppCompat::from_parts(file.apps, file.initial_mode_scope)
            .with_commit_newline(file.commit_newline);
        assert_eq!(compat.get_rule("other.exe").unwrap().auto_pair, Some(false));
        assert!(compat.initial_mode_applies_to_window("explorer.exe", "Progman"));
        assert!(!compat.initial_mode_applies_to_window("explorer.exe", "X"));
        assert_eq!(
            compat.commit_newline_for("winword.exe"),
            Some(NewlineStyle::Cr)
        );
        assert_eq!(compat.commit_newline_for("true"), None);
    }

    fn parse_rules(toml: &str) -> AppCompat {
        AppCompat::from_rules(toml::from_str::<AppCompatFile>(toml).unwrap().apps)
    }

    /// `schema` 三态：未配 = 跟随全局；具体 id = 固定；`@remember` = 记住上次。
    #[test]
    fn parse_app_schema_fixed_and_remember() {
        let compat = parse_rules(
            r#"
            [[apps]]
            process = "code.exe"
            schema = "english"

            [[apps]]
            process = "weixin.exe"
            schema = "@remember"

            [[apps]]
            process = "plain.exe"
            auto_pair = false
            "#,
        );
        assert_eq!(
            compat.get_rule("CODE.EXE").unwrap().app_schema(),
            Some(AppSchema::Fixed("english"))
        );
        assert_eq!(
            compat.get_rule("weixin.exe").unwrap().app_schema(),
            Some(AppSchema::Remember)
        );
        assert_eq!(compat.get_rule("plain.exe").unwrap().schema, None);
        assert_eq!(compat.get_rule("plain.exe").unwrap().app_schema(), None);
    }

    /// `@` 前缀是保留标记空间：只认 `@remember`，其余（拼错、将来的新标记）按未配置处理；
    /// 空串同理。**同文件其它规则必须照常生效**。
    #[test]
    fn unknown_app_schema_marker_degrades_to_none() {
        for bad in [r#""@remeber""#, r#""@global""#, r#""""#, r#""   ""#] {
            let compat = parse_rules(&format!(
                r#"
                [[apps]]
                process = "typo.exe"
                schema = {bad}

                [[apps]]
                process = "other.exe"
                auto_pair = false
                "#
            ));
            assert_eq!(
                compat.get_rule("typo.exe").and_then(|r| r.schema.clone()),
                None,
                "{bad}: 认不出 = 没配过"
            );
            assert_eq!(
                compat.get_rule("other.exe").unwrap().auto_pair,
                Some(false),
                "{bad}: 同文件其它规则必须照常生效"
            );
        }
    }

    /// 类型写错（`schema = 1` / 数组）不得让整份 compat.toml 失效：`load_file` 没有段级降级。
    #[test]
    fn app_schema_wrong_type_does_not_sink_the_file() {
        for bad in ["1", "true", r#"["pinyin"]"#] {
            let text = format!(
                r#"
                [[apps]]
                process = "typo.exe"
                schema = {bad}

                [[apps]]
                process = "other.exe"
                schema = "pinyin"
                "#
            );
            let file = toml::from_str::<AppCompatFile>(&text)
                .unwrap_or_else(|e| panic!("{bad}: 类型写错不得让整份失败：{e}"));
            let compat = AppCompat::from_rules(file.apps);
            assert_eq!(
                compat.get_rule("typo.exe").and_then(|r| r.schema.clone()),
                None
            );
            assert_eq!(
                compat.get_rule("other.exe").unwrap().app_schema(),
                Some(AppSchema::Fixed("pinyin"))
            );
        }
    }

    #[test]
    fn parse_apps_array_and_lookup_case_insensitive() {
        let toml = r#"
            [[apps]]
            process = "Weixin.exe"
            comment = "微信"
            caret_use_top = true
        "#;
        let file: AppCompatFile = toml::from_str(toml).unwrap();
        let compat = AppCompat::from_rules(file.apps);

        // 进程名匹配不区分大小写。
        let rule = compat
            .get_rule("weixin.exe")
            .expect("应命中 Weixin.exe 规则");
        assert!(rule.caret_use_top);
        assert!(compat.get_rule("WEIXIN.EXE").unwrap().caret_use_top);
        // 未配置的进程无规则。
        assert!(compat.get_rule("notepad.exe").is_none());
    }

    #[test]
    fn caret_use_top_defaults_false_when_absent() {
        let toml = r#"
            [[apps]]
            process = "Foo.exe"
        "#;
        let file: AppCompatFile = toml::from_str(toml).unwrap();
        let compat = AppCompat::from_rules(file.apps);
        let rule = compat.get_rule("foo.exe").unwrap();
        assert!(!rule.caret_use_top);
        assert_eq!(rule.composition_start_pair_guard, None);
        // 缺字段 = 不干预（跟随全局），与 initial_mode 同语义。若它退化成 Some(默认档)，
        // 等于给所有未配置的应用都写死了一份 per-app 覆盖，用户改全局默认时全部失效。
        assert_eq!(rule.first_show_mode, None);
        // 缺字段 = 不干预。若这两个退化成 Some(English)，等于给所有未配置的应用
        // 都配上了「初始英文」——这正是字段必须用 Option 而非 bool 的原因。
        assert_eq!(rule.initial_mode, None);
        assert_eq!(rule.initial_punct, None);
    }

    #[test]
    fn initial_mode_parses_both_values() {
        let toml = r#"
            [[apps]]
            process = "Everything.exe"
            initial_mode = "english"
            initial_punct = "chinese"
        "#;
        let file: AppCompatFile = toml::from_str(toml).unwrap();
        let compat = AppCompat::from_rules(file.apps);
        let rule = compat.get_rule("everything.exe").unwrap();
        assert_eq!(rule.initial_mode, Some(InitialMode::English));
        assert_eq!(rule.initial_punct, Some(InitialMode::Chinese));
        assert!(!rule.initial_mode.unwrap().is_chinese());
        assert!(rule.initial_punct.unwrap().is_chinese());
    }

    /// 单字段拼错只让**该字段**退化为「不干预」，不得连累同规则的其它字段、
    /// 也不得让整份 compat.toml 解析失败（`load_file` 失败会静默跳过整个文件，
    /// 于是一个错别字就让所有应用的所有规则一起失效且毫无痕迹）。
    #[test]
    fn unknown_initial_mode_degrades_to_none_without_killing_the_file() {
        let toml = r#"
            [[apps]]
            process = "Everything.exe"
            initial_mode = "englsh"
            caret_use_top = true

            [[apps]]
            process = "Weixin.exe"
            initial_mode = "chinese"
        "#;
        let file: AppCompatFile = toml::from_str(toml).expect("拼错的值不得让整份文件解析失败");
        let compat = AppCompat::from_rules(file.apps);
        let bad = compat.get_rule("everything.exe").unwrap();
        assert_eq!(bad.initial_mode, None, "无法识别 → 不干预");
        assert!(bad.caret_use_top, "同规则的其它字段不受牵连");
        // 后续规则完整存活。
        assert_eq!(
            compat.get_rule("weixin.exe").unwrap().initial_mode,
            Some(InitialMode::Chinese)
        );
    }

    #[test]
    fn mode_parses_from_config_and_rejects_unknown() {
        assert_eq!(
            FirstShowMode::from_config("fast"),
            Some(FirstShowMode::Fast)
        );
        assert_eq!(
            FirstShowMode::from_config(" INSTANT "),
            Some(FirstShowMode::Instant)
        );
        assert_eq!(
            FirstShowMode::from_config("wait"),
            Some(FirstShowMode::Wait)
        );
        // 未知值 = None。在 per-app 层它等于「没配」＝跟随全局；在全局层由调用点
        // `.unwrap_or_default()` 回落到默认档——回落只发生在一处，不再有两处独立事实。
        assert_eq!(FirstShowMode::from_config("turbo"), None);
        assert_eq!(FirstShowMode::Fast.as_config(), "fast");
    }

    /// 拼错的档位名不得让整份 compat.toml 失效，也不得固化成显式覆盖。
    #[test]
    fn unknown_mode_in_toml_degrades_to_follow_global() {
        let toml = r#"
            [[apps]]
            process = "Foo.exe"
            first_show_mode = "turbo"
            caret_use_top = true
        "#;
        let file: AppCompatFile = toml::from_str(toml).expect("单字段拼错不得整份解析失败");
        assert_eq!(file.apps[0].first_show_mode, None, "认不出 = 跟随全局");
        assert!(file.apps[0].caret_use_top, "同一条规则的其它字段仍须生效");
    }

    /// 默认档位是产品决策，单独钉一条，改动时必须显式过这一关。
    ///
    /// 2026-08-03 由 `wait` 改为 `fast`：`fast` 此前不敢作默认，是因为焦点切换/鼠标移动
    /// 光标后的首帧会拿一份属于别处的旧坐标定位；首帧信任门补上该洞后，它在坐标不可信时
    /// 会自动退回去等真值。实测常规连打首帧中位 7ms，焦点后首帧中位 105ms 且位置正确。
    #[test]
    fn default_mode_is_fast() {
        assert_eq!(FirstShowMode::default(), FirstShowMode::Fast);
    }

    /// 全局默认档现在有**两处**表达：枚举的 `#[default]`（认不出的值回落到它）与
    /// `config.toml` 出厂值 `ui.candidate.first_show_mode`（用户实际读到的那一份）。
    /// 两处分叉的表现是「配置文件写着 fast，某些路径却按另一档跑」，没有任何编译信号，
    /// 故把「必须一致」钉成不变量。顺带也验出厂串本身是个合法档位名（拼错即 None）。
    #[test]
    fn global_default_config_value_matches_enum_default() {
        let factory = crate::config::UiCandidateConfig::default().first_show_mode;
        assert_eq!(
            FirstShowMode::from_config(&factory),
            Some(FirstShowMode::default()),
            "config.toml 出厂档 {factory:?} 与枚举 #[default] 不一致"
        );
    }

    /// 缺字段 = 不干预。若退化成 `Some(默认档)`，等于给所有未配置的应用都写死了一份
    /// per-app 覆盖，用户改全局 `ui.candidate.position_mode` 时会全部失效。
    #[test]
    fn new_per_app_fields_default_to_follow_global() {
        let toml = r#"
            [[apps]]
            process = "Foo.exe"
        "#;
        let file: AppCompatFile = toml::from_str(toml).unwrap();
        let compat = AppCompat::from_rules(file.apps);
        let rule = compat.get_rule("foo.exe").unwrap();
        assert_eq!(rule.candidate_position_mode, None);
        assert_eq!(rule.ignore_host_ime_close, None);
        assert_eq!((rule.candidate_x, rule.candidate_y), (0, 0));
    }

    /// 认不出的定位方式退化为 `None`（跟随全局），且**不拖垮同文件其它规则**——
    /// 整份 compat.toml 因一个拼错的值静默失效是本仓反复记过的形态。
    #[test]
    fn bad_candidate_position_mode_degrades_to_none() {
        let toml = r#"
            [[apps]]
            process = "Foo.exe"
            candidate_position_mode = "fixedd"

            [[apps]]
            process = "Bar.exe"
            candidate_position_mode = "fixed"
        "#;
        let file: AppCompatFile = toml::from_str(toml).expect("单字段拼错不得让整份失效");
        let compat = AppCompat::from_rules(file.apps);
        assert_eq!(
            compat.get_rule("foo.exe").unwrap().candidate_position_mode,
            None
        );
        assert_eq!(
            compat.get_rule("bar.exe").unwrap().candidate_position_mode,
            Some(CandidatePositionMode::Fixed)
        );
    }

    // ── [[initial_mode_scope]] ──

    fn sample_scope() -> Vec<InitialModeScopeRule> {
        vec![InitialModeScopeRule {
            process: "explorer.exe".into(),
            comment: String::new(),
            disabled: false,
            classes: vec!["Progman".into(), "WorkerW".into()],
        }]
    }

    #[test]
    fn mode_scope_match_is_case_insensitive_on_both_keys() {
        let c = AppCompat::from_parts(Vec::new(), sample_scope());
        assert!(c.initial_mode_applies_to_window("explorer.exe", "Progman"));
        assert!(c.initial_mode_applies_to_window("EXPLORER.EXE", "progman"));
        // 作用域外：任务栏 / Alt+Tab / 溢出区 —— 保持现状
        assert!(!c.initial_mode_applies_to_window("explorer.exe", "Shell_TrayWnd"));
        assert!(!c.initial_mode_applies_to_window("explorer.exe", "ForegroundStaging"));
    }

    /// ★★★ 未配作用域的进程**完全不受影响**。这条守的是「引入本机制不会波及其它应用」，
    /// 也是把判据从黑名单反转成白名单后唯一可能出的新缺陷（把所有人都关进作用域）。
    #[test]
    fn process_without_scope_entry_is_unrestricted() {
        let c = AppCompat::from_parts(Vec::new(), sample_scope());
        assert!(c.initial_mode_applies_to_window("notepad.exe", "Notepad"));
        // 连窗口类都拿不到时也照常生效——没配作用域就没有任何限制
        assert!(c.initial_mode_applies_to_window("notepad.exe", ""));
        // 空进程名同理（macOS / 取名失败）
        assert!(c.initial_mode_applies_to_window("", ""));
    }

    /// ★★★ 本轮缺陷的钉子：作用域内的进程，窗口类为空时必须**保持现状**。
    ///
    /// 实测现场（2026-08-18 17:24:08）：explorer 新起一个 TSF 连接，其首个 focus_gained
    /// 拿不到窗口类（caret 也退到 last_known），旧的黑名单判据把空类名放行 ⇒ 套上
    /// explorer 的 `initial_mode = "english"` ⇒ 语言栏图标闪英。
    #[test]
    fn empty_window_class_stays_outside_scope() {
        let c = AppCompat::from_parts(Vec::new(), sample_scope());
        assert!(!c.initial_mode_applies_to_window("explorer.exe", ""));
    }

    /// 空 classes = 该进程的初始模式规则在任何窗口上都不重算（不是"不受限"）。
    #[test]
    fn empty_class_list_blocks_everything_for_that_process() {
        let c = AppCompat::from_parts(
            Vec::new(),
            vec![InitialModeScopeRule {
                process: "explorer.exe".into(),
                comment: String::new(),
                disabled: false,
                classes: Vec::new(),
            }],
        );
        assert!(!c.initial_mode_applies_to_window("explorer.exe", "Progman"));
        assert!(c.initial_mode_applies_to_window("notepad.exe", "Notepad"));
    }
}

#[cfg(test)]
mod disabled_rule_tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wind_compat_dis_{tag}_{}", std::process::id()))
    }
    fn write(dir: &std::path::Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(COMPAT_FILE_NAME), text).unwrap();
    }

    #[test]
    fn user_disabled_entry_removes_the_system_rule() {
        let (sys, usr) = (tmp("a_sys"), tmp("a_usr"));
        write(
            &sys,
            "[[apps]]\nprocess = \"Feishu.exe\"\ncomposition_start_pair_guard = true\n\
             [[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\n",
        );
        write(
            &usr,
            "[[apps]]\nprocess = \"feishu.exe\"\ndisabled = true\n",
        );
        let c = AppCompat::load_layered(Some(&sys), None, Some(&usr));
        assert!(
            c.get_rule("Feishu.exe").is_none(),
            "被禁用的进程不应再有规则"
        );
        assert!(c.get_rule("Weixin.exe").is_some(), "禁用只影响指名的进程");
        let _ = std::fs::remove_dir_all(&sys);
        let _ = std::fs::remove_dir_all(&usr);
    }

    #[test]
    fn disabled_is_omitted_when_false_and_written_when_true() {
        let off = toml::to_string(&AppCompatRule {
            process: "a.exe".into(),
            caret_use_top: true,
            ..Default::default()
        })
        .unwrap();
        assert!(!off.contains("disabled"), "false 不应落盘: {off}");
        let on = toml::to_string(&AppCompatRule {
            process: "a.exe".into(),
            disabled: true,
            ..Default::default()
        })
        .unwrap();
        assert!(on.contains("disabled = true"), "true 必须落盘: {on}");
    }

    #[test]
    fn wrong_typed_disabled_falls_back_to_enabled() {
        let (sys, usr) = (tmp("c_sys"), tmp("c_usr"));
        write(
            &sys,
            "[[apps]]\nprocess = \"Feishu.exe\"\ncaret_use_top = true\n",
        );
        write(
            &usr,
            "[[apps]]\nprocess = \"Feishu.exe\"\ndisabled = \"yes\"\ncaret_use_top = true\n",
        );
        let c = AppCompat::load_layered(Some(&sys), None, Some(&usr));
        assert!(
            c.get_rule("Feishu.exe").is_some(),
            "写错类型按未禁用处理，不能整条丢"
        );
        let _ = std::fs::remove_dir_all(&sys);
        let _ = std::fs::remove_dir_all(&usr);
    }

    #[test]
    fn disabled_applies_to_scope_and_newline_sections() {
        let (sys, usr) = (tmp("d_sys"), tmp("d_usr"));
        write(
            &sys,
            "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"CabinetWClass\"]\n\
             [[commit_newline]]\nprocess = \"WINWORD.EXE\"\nstyle = \"cr\"\n",
        );
        write(
            &usr,
            "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\ndisabled = true\n\
             [[commit_newline]]\nprocess = \"WINWORD.EXE\"\ndisabled = true\n",
        );
        let c = AppCompat::load_layered(Some(&sys), None, Some(&usr));
        assert!(c.commit_newline_for("WINWORD.EXE").is_none());
        // 作用域规则被禁用 = 进程不在表内 = 不受限制
        assert!(c.initial_mode_applies_to_window("explorer.exe", "AnyOtherClass"));
        let _ = std::fs::remove_dir_all(&sys);
        let _ = std::fs::remove_dir_all(&usr);
    }
}

/// 分层叠加语义（整体原则：每一层都是相对下一层的差异，逐字段叠加）与菜单写入路径。
#[cfg(test)]
mod layering_tests {
    use super::*;
    use crate::compat_overlay::parse_raw;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_compat_lay_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn write(dir: &std::path::Path, text: &str) {
        std::fs::write(dir.join(COMPAT_FILE_NAME), text).unwrap();
    }
    fn read(dir: &std::path::Path) -> String {
        std::fs::read_to_string(dir.join(COMPAT_FILE_NAME)).unwrap_or_default()
    }
    fn raw(text: &str) -> Raw {
        parse_raw(text).expect("测试夹具必须是合法 TOML")
    }
    fn load(sys: &str, user: &str, tag: &str) -> AppCompat {
        let (d, u) = (tmp(&format!("{tag}_d")), tmp(&format!("{tag}_u")));
        write(&d, sys);
        write(&u, user);
        let c = AppCompat::load_layered(Some(&d), None, Some(&u));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
        c
    }

    const WECHAT: &str =
        "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = true\nstale_probe_guard = true\n";

    /// ★ 靶机实测的真实事故：微信的用户规则只有菜单写的一项 first_show_mode，
    /// 旧的「整条覆盖」让出厂的 caret_use_top / stale_probe_guard 运行时全丢。
    #[test]
    fn a_sparse_user_rule_keeps_every_other_system_field() {
        let c = load(
            WECHAT,
            "[[apps]]\nprocess = \"weixin.exe\"\nfirst_show_mode = \"instant\"\n",
            "wechat",
        );
        let r = c.get_rule("Weixin.exe").unwrap();
        assert!(r.caret_use_top, "出厂的 caret_use_top 不能丢");
        assert!(r.stale_probe_guard, "出厂的 stale_probe_guard 不能丢");
        assert_eq!(r.first_show_mode, Some(FirstShowMode::Instant));
    }

    #[test]
    fn explicit_false_in_the_user_layer_turns_off_a_system_switch() {
        let c = load(
            WECHAT,
            "[[apps]]\nprocess = \"Weixin.exe\"\ncaret_use_top = false\n",
            "off",
        );
        let r = c.get_rule("Weixin.exe").unwrap();
        assert!(!r.caret_use_top, "显式 false 必须能关掉出厂打开的开关");
        assert!(r.stale_probe_guard, "没提到的字段照旧继承");
    }

    #[test]
    fn unset_returns_a_system_field_to_follow_global() {
        let c = load(
            "[[apps]]\nprocess = \"EXCEL.EXE\"\nfirst_show_mode = \"wait\"\nauto_pair = false\n",
            "[[apps]]\nprocess = \"excel.exe\"\nunset = [\"first_show_mode\"]\n",
            "unset",
        );
        let r = c.get_rule("EXCEL.EXE").unwrap();
        assert_eq!(
            r.first_show_mode, None,
            "unset 后回到跟随全局，而不是继承出厂的 wait"
        );
        assert_eq!(r.auto_pair, Some(false), "别的字段不受影响");
    }

    #[test]
    fn three_layers_each_overlay_the_one_below() {
        let (d, c, u) = (tmp("3_d"), tmp("3_c"), tmp("3_u"));
        write(
            &d,
            "[[apps]]\nprocess = \"A.exe\"\ncaret_use_top = true\nauto_pair = true\n",
        );
        write(
            &c,
            "[[apps]]\nprocess = \"A.exe\"\nauto_pair = false\ncaret_offset_x = 4\n",
        );
        write(&u, "[[apps]]\nprocess = \"A.exe\"\ncaret_offset_x = 9\n");
        let rt = AppCompat::load_layered(Some(&d), Some(&c), Some(&u));
        let r = rt.get_rule("A.exe").unwrap();
        assert!(r.caret_use_top, "data 层的字段被继承到最上层");
        assert_eq!(r.auto_pair, Some(false), "custom 层覆盖 data 层");
        assert_eq!(r.caret_offset_x, 9, "user 层覆盖 custom 层");
        for x in [&d, &c, &u] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    /// 三段各自独立叠加：为某进程写 `[[apps]]` 不会让更低层给它配的作用域 / 换行规则消失，反之亦然。
    #[test]
    fn sections_overlay_independently() {
        let sys = "[[apps]]\nprocess = \"WINWORD.EXE\"\ncaret_offset_y = 3\n\n\
                   [[commit_newline]]\nprocess = \"WINWORD.EXE\"\nstyle = \"cr\"\n\n\
                   [[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"Progman\", \"WorkerW\"]\n";
        let user = "[[apps]]\nprocess = \"WINWORD.EXE\"\nauto_pair = false\n\n\
                    [[apps]]\nprocess = \"explorer.exe\"\ncaret_use_top = true\n";
        let c = load(sys, user, "sections");
        assert_eq!(
            c.commit_newline_for("winword.exe"),
            Some(NewlineStyle::Cr),
            "用户改 apps 不能顶掉换行段"
        );
        assert_eq!(c.get_rule("winword.exe").map(|r| r.caret_offset_y), Some(3));
        assert!(
            c.initial_mode_applies_to_window("explorer.exe", "Progman"),
            "作用域段不受 apps 段影响"
        );
        assert!(!c.initial_mode_applies_to_window("explorer.exe", "Shell_TrayWnd"));

        let c2 = load(
            sys,
            "[[commit_newline]]\nprocess = \"winword.exe\"\nstyle = \"crlf\"\n",
            "sections2",
        );
        assert_eq!(
            c2.commit_newline_for("WINWORD.EXE"),
            Some(NewlineStyle::Crlf),
            "换行段逐字段叠加"
        );
        assert_eq!(
            c2.get_rule("WINWORD.EXE").map(|r| r.caret_offset_y),
            Some(3),
            "用户配换行不能顶掉 apps 段"
        );
    }

    /// 列表字段按整个字段替换，不做元素级合并。
    #[test]
    fn a_list_field_is_replaced_as_a_whole() {
        let c = load(
            "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"Progman\", \"WorkerW\"]\n",
            "[[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"CabinetWClass\"]\n",
            "list",
        );
        assert!(c.initial_mode_applies_to_window("explorer.exe", "CabinetWClass"));
        assert!(
            !c.initial_mode_applies_to_window("explorer.exe", "Progman"),
            "整个列表被替换，旧元素不保留"
        );
    }

    /// 一行结构错只丢这一行，同文件其它规则照常；语法错则整份跳过（且有 WARN）。
    #[test]
    fn a_structurally_bad_row_only_loses_itself() {
        let c = load(
            "",
            "[[apps]]\nprocess = \"good.exe\"\ncaret_use_top = true\n\n[[initial_mode_scope]]\nprocess = \"bad.exe\"\nclasses = \"nope\"\n\n[[initial_mode_scope]]\nprocess = \"ok.exe\"\nclasses = [\"A\"]\n",
            "badrow",
        );
        assert!(c.get_rule("good.exe").is_some());
        assert!(
            !c.initial_mode_applies_to_window("ok.exe", "B"),
            "好的那行正常生效"
        );
        assert!(
            c.initial_mode_applies_to_window("bad.exe", "B"),
            "坏的那行作废 = 不受限制"
        );
        let broken = load("", "[[apps\nprocess=", "syntax");
        assert!(broken.get_rule("anything.exe").is_none());
    }

    #[test]
    fn disabling_and_reenabling_across_layers() {
        let (d, c, u) = (tmp("dis_d"), tmp("dis_c"), tmp("dis_u"));
        write(&d, "[[apps]]\nprocess = \"A.exe\"\ncaret_use_top = true\n");
        write(&c, "[[apps]]\nprocess = \"A.exe\"\ndisabled = true\n");
        let off = AppCompat::load_layered(Some(&d), Some(&c), None);
        assert!(off.get_rule("A.exe").is_none(), "custom 层禁用");
        write(&u, "[[apps]]\nprocess = \"A.exe\"\ndisabled = false\n");
        let on = AppCompat::load_layered(Some(&d), Some(&c), Some(&u));
        assert!(
            on.get_rule("A.exe").is_some_and(|r| r.caret_use_top),
            "更高层显式 disabled = false 重新启用，字段照常继承"
        );
        for x in [&d, &c, &u] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    /// 每个登记过的字段都必须能穿过「叠加 → 结构体」而不丢：稀疏的上层条目不得吞掉任何一个下层字段。
    /// 取代旧的「协议字段登记」源码扫描守门测试——现在没有任何字段享有特殊继承待遇，
    /// 这条对全部登记字段一视同仁地钉住。
    #[test]
    fn every_registered_field_survives_layering() {
        use crate::compat_schema::{COMPAT_FIELDS, Kind};
        use serde_json::json;
        for f in COMPAT_FIELDS.iter().filter(|f| f.section == "apps") {
            let sample = match f.kind {
                Kind::Bool | Kind::TriBool => json!(true),
                Kind::Enum => json!(f.options[0]),
                Kind::Int => json!(7),
                Kind::Text => json!("sample"),
                Kind::TextList => json!(["A"]),
            };
            let sys =
                vec![serde_json::from_value(json!({"process": "p.exe", f.key: sample})).unwrap()];
            let user = vec![
                serde_json::from_value(json!({"process": "P.EXE", "comment": "稀疏条目"})).unwrap(),
            ];
            let rules: Vec<AppCompatRule> =
                materialize(&crate::compat_overlay::overlay(sys, &user));
            let out = serde_json::to_value(&rules[0]).unwrap();
            assert!(
                out.get(f.key).is_some(),
                "字段 {} 被稀疏的上层条目吞掉了（或根本没能反序列化）: {out}",
                f.key
            );
        }
    }

    /// 守随发布的系统层 `data/compat.toml`：解析失败会**整份静默跳过**，所有应用的所有规则一起失效。
    #[test]
    fn shipped_system_compat_configures_word_and_scopes_explorer_to_desktop() {
        let data_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../data"));
        assert!(
            load_raw(&data_dir.join(COMPAT_FILE_NAME)).is_some(),
            "随发布的 data/compat.toml 必须能解析（失败会静默吞掉全部规则）"
        );
        let c = AppCompat::load_layered(Some(data_dir), None, None);
        assert_eq!(
            c.commit_newline_for("winword.exe"),
            Some(NewlineStyle::Cr),
            "Word 的段落边界是 CR（2026-08-23 真机实测）"
        );
        assert_eq!(
            c.commit_newline_for("wps.exe"),
            None,
            "WPS 文字实测用 LF 正常分段，不该在出厂名单里"
        );
        assert!(
            c.get_rule("qq.exe")
                .is_some_and(|r| r.composition_start_pair_guard == Some(true))
        );
        for class in ["Progman", "WorkerW"] {
            assert!(
                c.initial_mode_applies_to_window("explorer.exe", class),
                "{class} 是桌面"
            );
        }
        for class in [
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "XamlExplorerHostIslandWindow",
            "ForegroundStaging",
            "TopLevelWindowForOverflowXamlIsland",
            "",
        ] {
            assert!(
                !c.initial_mode_applies_to_window("explorer.exe", class),
                "class={class:?} 不该重算初始模式"
            );
        }
        assert!(c.initial_mode_applies_to_window("notepad.exe", ""));
    }

    // ───────────── 右键菜单写入路径 ─────────────

    fn with_system<R>(text: &str, f: impl FnOnce() -> R) -> R {
        with_menu_system(raw(text), true, f)
    }

    #[test]
    fn menu_set_stores_only_the_changed_field() {
        let dir = tmp("m_set");
        with_system(WECHAT, || {
            set_user_first_show_mode(&dir, "weixin.exe", Some(FirstShowMode::Instant)).unwrap();
        });
        let text = read(&dir);
        assert!(text.contains("first_show_mode = \"instant\""), "{text}");
        assert!(
            !text.contains("caret_use_top") && !text.contains("stale_probe_guard"),
            "不得把系统字段拷进用户层: {text}"
        );
        assert!(
            text.contains("process = \"Weixin.exe\""),
            "沿用系统层写法: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menu_clear_of_a_system_field_writes_unset_and_runtime_follows_global() {
        let (d, u) = (tmp("m_clr_d"), tmp("m_clr_u"));
        let sys =
            "[[apps]]\nprocess = \"EXCEL.EXE\"\nfirst_show_mode = \"wait\"\nauto_pair = false\n";
        write(&d, sys);
        with_system(sys, || {
            set_user_first_show_mode(&u, "excel.exe", None).unwrap()
        });
        assert!(
            read(&u).contains("unset = [\"first_show_mode\"]"),
            "{}",
            read(&u)
        );
        let rt = AppCompat::load_layered(Some(&d), None, Some(&u));
        let r = rt.get_rule("EXCEL.EXE").unwrap();
        assert_eq!(
            r.first_show_mode, None,
            "菜单选「跟随全局」必须真的跟随全局，不能被出厂值顶回来"
        );
        assert_eq!(r.auto_pair, Some(false));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
    }

    #[test]
    fn menu_clear_of_a_field_the_system_lacks_leaves_no_trace() {
        let dir = tmp("m_noop");
        with_system(WECHAT, || {
            set_user_first_show_mode(&dir, "weixin.exe", Some(FirstShowMode::Fast)).unwrap();
            set_user_first_show_mode(&dir, "weixin.exe", None).unwrap();
        });
        assert!(
            !read(&dir).contains("[[apps]]"),
            "清掉最后一项后整条应消失，不留空壳: {}",
            read(&dir)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menu_value_equal_to_the_system_leaves_no_entry() {
        let dir = tmp("m_same");
        with_system(WECHAT, || {
            set_user_host_render(&dir, "Weixin.exe", false).unwrap()
        });
        with_system(
            "[[apps]]\nprocess = \"S.exe\"\nfirst_show_mode = \"wait\"\n",
            || {
                set_user_first_show_mode(&dir, "s.exe", Some(FirstShowMode::Wait)).unwrap();
            },
        );
        assert!(
            !read(&dir).contains("[[apps]]"),
            "与系统一致 / 与默认一致都不该落盘: {}",
            read(&dir)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menu_host_render_false_overrides_a_system_true() {
        let (d, u) = (tmp("m_hr_d"), tmp("m_hr_u"));
        let sys = "[[apps]]\nprocess = \"SearchHost.exe\"\nhost_render = true\n";
        write(&d, sys);
        with_system(sys, || {
            set_user_host_render(&u, "searchhost.exe", false).unwrap()
        });
        let rt = AppCompat::load_layered(Some(&d), None, Some(&u));
        assert!(
            rt.host_render_processes().is_empty(),
            "用户把它从白名单里去掉必须生效"
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
    }

    /// 拖动落盘把定位方式与坐标一起写，让这条差异自成一体，不依赖系统层恰好配了什么。
    #[test]
    fn dragging_persists_position_mode_together_with_coords() {
        let (d, u) = (tmp("m_drag_d"), tmp("m_drag_u"));
        let sys = "[[apps]]\nprocess = \"X60_Toolbox.exe\"\nfirst_show_mode = \"wait\"\n";
        write(&d, sys);
        with_system(sys, || {
            set_user_candidate_fixed_pos(&u, "X60_Toolbox.exe", 320, 480).unwrap()
        });
        let text = read(&u);
        assert!(
            text.contains("candidate_position_mode = \"fixed\""),
            "{text}"
        );
        let r = AppCompat::load_layered(Some(&d), None, Some(&u))
            .get_rule("x60_toolbox.exe")
            .cloned()
            .unwrap();
        assert_eq!(
            r.candidate_position_mode,
            Some(CandidatePositionMode::Fixed)
        );
        assert_eq!((r.candidate_x, r.candidate_y), (320, 480));
        assert_eq!(
            r.first_show_mode,
            Some(FirstShowMode::Wait),
            "拖动不能顶掉系统层的别的字段"
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
    }

    #[test]
    fn clearing_candidate_position_mode_also_clears_coords() {
        let dir = tmp("m_cpm");
        with_system("", || {
            set_user_candidate_fixed_pos(&dir, "a.exe", 10, 20).unwrap();
            set_user_candidate_position_mode(&dir, "a.exe", None).unwrap();
        });
        assert!(
            !read(&dir).contains("candidate_"),
            "清除定位方式要一并清坐标，免得留孤儿坐标: {}",
            read(&dir)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_position_writes_mode_and_coords_together() {
        let dir = tmp("m_sp");
        with_system("", || {
            set_user_status_position(&dir, "a.exe", Some(StatusPositionMode::Fixed), 5, 6).unwrap();
        });
        let t = read(&dir);
        assert!(
            t.contains("status_position_mode = \"fixed\"")
                && t.contains("status_x = 5")
                && t.contains("status_y = 6"),
            "{t}"
        );
        with_system("", || {
            set_user_status_position(&dir, "a.exe", Some(StatusPositionMode::FollowCaret), 5, 6)
                .unwrap();
        });
        let t = read(&dir);
        assert!(
            t.contains("follow_caret") && !t.contains("status_x"),
            "非 fixed 不用坐标，要清掉: {t}"
        );
        with_system("", || {
            set_user_status_position(&dir, "a.exe", None, 0, 0).unwrap()
        });
        assert!(
            !read(&dir).contains("[[apps]]"),
            "清除后整条消失: {}",
            read(&dir)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menu_write_preserves_other_rules_other_sections_and_unknown_keys() {
        let dir = tmp("m_keep");
        write(
            &dir,
            "[[apps]]\nprocess = \"other.exe\"\nauto_pair = true\nfuture_field = 7\n\n\
             [[commit_newline]]\nprocess = \"w.exe\"\nstyle = \"cr\"\n\n\
             [[initial_mode_scope]]\nprocess = \"explorer.exe\"\nclasses = [\"A\"]\n",
        );
        with_system("", || {
            set_user_first_show_mode(&dir, "new.exe", Some(FirstShowMode::Fast)).unwrap()
        });
        let t = read(&dir);
        for needle in [
            "other.exe",
            "future_field = 7",
            "[[commit_newline]]",
            "style = \"cr\"",
            "[[initial_mode_scope]]",
            "new.exe",
        ] {
            assert!(t.contains(needle), "写回后丢了 {needle}: {t}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 各包装函数写出的值都要能被运行时读回来（往返）。
    #[test]
    fn every_menu_wrapper_roundtrips_through_the_runtime_loader() {
        let u = tmp("m_all");
        let d = tmp("m_all_d");
        write(&d, "");
        with_system("", || {
            set_user_initial_mode(&u, "a.exe", Some(InitialMode::English)).unwrap();
            set_user_initial_punct(&u, "a.exe", Some(InitialMode::Chinese)).unwrap();
            set_user_auto_pair(&u, "a.exe", Some(false)).unwrap();
            set_user_password_force_english(&u, "a.exe", Some(true)).unwrap();
            set_user_schema(&u, "a.exe", Some("@remember".into())).unwrap();
            set_user_ignore_host_ime_close(&u, "a.exe", Some(true)).unwrap();
            set_user_smart_method(&u, "a.exe", Some(SmartMethod::HoldComposition)).unwrap();
            set_user_caret_offset(&u, "a.exe", 3, -4).unwrap();
            set_user_status_fallback(&u, "a.exe", Some(StatusFallback::Hide)).unwrap();
        });
        let rt = AppCompat::load_layered(Some(&d), None, Some(&u));
        let r = rt.get_rule("a.exe").expect("菜单写入后运行时必须读得到");
        assert_eq!(r.initial_mode, Some(InitialMode::English));
        assert_eq!(r.initial_punct, Some(InitialMode::Chinese));
        assert_eq!(r.auto_pair, Some(false), "显式 false 必须落盘");
        assert_eq!(r.password_force_english, Some(true));
        assert_eq!(r.app_schema(), Some(AppSchema::Remember));
        assert_eq!(r.ignore_host_ime_close, Some(true));
        assert_eq!(r.smart_method, Some(SmartMethod::HoldComposition));
        assert_eq!((r.caret_offset_x, r.caret_offset_y), (3, -4));
        assert_eq!(r.status_fallback_position, Some(StatusFallback::Hide));
        let _ = std::fs::remove_dir_all(&u);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 菜单「宿主关闭输入法」第一档是「跟随内置规则」：`None` 必须是还原（继承出厂），
    /// 而不是清除——清除会给出厂打开的宿主写 unset，把内置规则整个去掉。
    #[test]
    fn menu_ignore_host_ime_close_none_inherits_the_builtin_rule() {
        let (d, u) = (tmp("m_ihc_d"), tmp("m_ihc_u"));
        let sys = "[[apps]]\nprocess = \"X60_Toolbox.exe\"\nignore_host_ime_close = true\n";
        write(&d, sys);
        with_system(sys, || {
            set_user_ignore_host_ime_close(&u, "x60_toolbox.exe", Some(false)).unwrap();
        });
        let off = AppCompat::load_layered(Some(&d), None, Some(&u));
        assert_eq!(
            off.get_rule("X60_Toolbox.exe")
                .unwrap()
                .ignore_host_ime_close,
            Some(false),
            "用户显式不采纳"
        );
        with_system(sys, || {
            set_user_ignore_host_ime_close(&u, "x60_toolbox.exe", None).unwrap()
        });
        assert!(
            parse_raw(&read(&u))
                .unwrap()
                .apps
                .iter()
                .all(|r| !r.contains_key("unset")),
            "还原不该留 unset: {}",
            read(&u)
        );
        let back = AppCompat::load_layered(Some(&d), None, Some(&u));
        assert_eq!(
            back.get_rule("X60_Toolbox.exe")
                .unwrap()
                .ignore_host_ime_close,
            Some(true),
            "继承回内置规则"
        );
        with_system(sys, || {
            set_user_ignore_host_ime_close(&u, "x60_toolbox.exe", Some(true)).unwrap()
        });
        assert!(
            !read(&u).contains("[[apps]]"),
            "显式写成与内置相同属冗余: {}",
            read(&u)
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&u);
    }

    /// 系统层读不到（菜单路径拿不到系统预置文件）时，清除字段不能凭「系统里没有」就省掉 unset。
    #[test]
    fn menu_clear_with_unknown_system_layer_still_writes_unset() {
        let dir = tmp("m_unknown");
        with_menu_system(Raw::default(), false, || {
            set_user_first_show_mode(&dir, "a.exe", None).unwrap();
        });
        assert!(
            parse_raw(&read(&dir))
                .unwrap()
                .apps
                .iter()
                .any(|r| r.contains_key("unset")),
            "读不到系统层时保守地写 unset: {}",
            read(&dir)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 菜单路径遇到损坏的用户层：仍然重建（不让菜单卡死），但先把原文件留一份 .bad。
    #[test]
    fn menu_write_on_a_broken_file_keeps_a_copy_of_the_original() {
        let dir = tmp("m_bad");
        let broken = "[[apps\nprocess=";
        write(&dir, broken);
        with_system("", || {
            set_user_first_show_mode(&dir, "a.exe", Some(FirstShowMode::Fast)).unwrap()
        });
        assert_eq!(
            std::fs::read_to_string(dir.join(format!("{COMPAT_FILE_NAME}.bad"))).unwrap(),
            broken
        );
        assert!(read(&dir).contains("a.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
