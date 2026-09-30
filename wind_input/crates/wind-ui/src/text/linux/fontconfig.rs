//! fontconfig 的运行期绑定（`dlopen`）。
//!
//! 移植自姊妹仓 wind-ui-rust `src/text/linux/fontconfig.rs`（同一作者，MIT/Apache-2.0），
//! 在其「按序回退」之外补了按名查询（[`family_exists`] / [`find_face`]）。
//!
//! 为什么不在编译期链接：链接要求构建机装 `libfontconfig1-dev`（`libfontconfig.so` 那个
//! 无版本号的符号链接只在 -dev 包里），每台 CI 机都得为此多装一个包；而运行期桌面系统上
//! `libfontconfig.so.1` 几乎必然存在。`dlopen` 让「编译」与「有没有 fontconfig」脱钩：
//! 取不到就返回 `None`，由调用方退回扫字体目录（见 `store.rs`）。

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
/// `FcResultMatch`。
const FC_RESULT_MATCH: c_int = 0;
/// `FcMatchPattern`。
const FC_MATCH_PATTERN: c_int = 0;

type Ptr = *mut c_void;

/// `FcFontSet` 的内存布局（fontconfig 公开头文件里就是这三个字段，ABI 自 2.0 起未变）。
#[repr(C)]
struct FcFontSet {
    nfont: c_int,
    sfont: c_int,
    fonts: *mut Ptr,
}

struct Api {
    name_parse: unsafe extern "C" fn(*const u8) -> Ptr,
    config_substitute: unsafe extern "C" fn(Ptr, Ptr, c_int) -> c_int,
    default_substitute: unsafe extern "C" fn(Ptr),
    font_sort: unsafe extern "C" fn(Ptr, Ptr, c_int, *mut Ptr, *mut c_int) -> *mut FcFontSet,
    font_list: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> *mut FcFontSet,
    font_set_destroy: unsafe extern "C" fn(*mut FcFontSet),
    pattern_create: unsafe extern "C" fn() -> Ptr,
    pattern_add_string: unsafe extern "C" fn(Ptr, *const c_char, *const u8) -> c_int,
    pattern_destroy: unsafe extern "C" fn(Ptr),
    pattern_get_string: unsafe extern "C" fn(Ptr, *const c_char, c_int, *mut *const u8) -> c_int,
    pattern_get_integer: unsafe extern "C" fn(Ptr, *const c_char, c_int, *mut c_int) -> c_int,
    pattern_get_charset: unsafe extern "C" fn(Ptr, *const c_char, c_int, *mut Ptr) -> c_int,
    object_set_create: unsafe extern "C" fn() -> Ptr,
    object_set_add: unsafe extern "C" fn(Ptr, *const c_char) -> c_int,
    object_set_destroy: unsafe extern "C" fn(Ptr),
    charset_copy: unsafe extern "C" fn(Ptr) -> Ptr,
    charset_has_char: unsafe extern "C" fn(Ptr, u32) -> c_int,
    charset_destroy: unsafe extern "C" fn(Ptr),
    /// 2.11.91 才有；老版本上缺席时按线性表近似（见 [`fc_weight`]）。
    weight_from_opentype: Option<unsafe extern "C" fn(c_int) -> c_int>,
    /// 同上，反方向（[`find_face`] 报字重用）。
    weight_to_opentype: Option<unsafe extern "C" fn(c_int) -> c_int>,
    config: Ptr,
}

// fontconfig ≥ 2.10 的查询 API 是线程安全的；`config` 在初始化后只读。
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

fn load() -> Option<Api> {
    // 环境变量逃生口：排查「是不是 fontconfig 选错了字」时关掉它，走目录扫描对照。
    if std::env::var_os("WIND_NO_FONTCONFIG").is_some() {
        tracing::info!("WIND_NO_FONTCONFIG 已设置，不加载 fontconfig");
        return None;
    }
    unsafe {
        let Some(lib) = [c"libfontconfig.so.1", c"libfontconfig.so"]
            .iter()
            .map(|n| dlopen(n.as_ptr(), RTLD_NOW))
            .find(|h| !h.is_null())
        else {
            tracing::warn!("dlopen libfontconfig.so.1 失败，改为扫描字体目录选字");
            return None;
        };
        macro_rules! sym {
            ($name:literal) => {{
                let p = dlsym(lib, $name.as_ptr());
                if p.is_null() {
                    tracing::warn!("fontconfig 缺少符号 {:?}，改为扫描字体目录选字", $name);
                    return None;
                }
                fn_ptr(p)
            }};
        }
        let init_load_config_and_fonts: unsafe extern "C" fn() -> Ptr =
            sym!(c"FcInitLoadConfigAndFonts");
        let config = init_load_config_and_fonts();
        if config.is_null() {
            tracing::warn!("FcInitLoadConfigAndFonts 失败，改为扫描字体目录选字");
            return None;
        }
        let opt = |name: &CStr| {
            let p = dlsym(lib, name.as_ptr());
            (!p.is_null()).then_some(p)
        };
        Some(Api {
            name_parse: sym!(c"FcNameParse"),
            config_substitute: sym!(c"FcConfigSubstitute"),
            default_substitute: sym!(c"FcDefaultSubstitute"),
            font_sort: sym!(c"FcFontSort"),
            font_list: sym!(c"FcFontList"),
            font_set_destroy: sym!(c"FcFontSetDestroy"),
            pattern_create: sym!(c"FcPatternCreate"),
            pattern_add_string: sym!(c"FcPatternAddString"),
            pattern_destroy: sym!(c"FcPatternDestroy"),
            pattern_get_string: sym!(c"FcPatternGetString"),
            pattern_get_integer: sym!(c"FcPatternGetInteger"),
            pattern_get_charset: sym!(c"FcPatternGetCharSet"),
            object_set_create: sym!(c"FcObjectSetCreate"),
            object_set_add: sym!(c"FcObjectSetAdd"),
            object_set_destroy: sym!(c"FcObjectSetDestroy"),
            charset_copy: sym!(c"FcCharSetCopy"),
            charset_has_char: sym!(c"FcCharSetHasChar"),
            charset_destroy: sym!(c"FcCharSetDestroy"),
            weight_from_opentype: opt(c"FcWeightFromOpenType").map(|p| fn_ptr(p)),
            weight_to_opentype: opt(c"FcWeightToOpenType").map(|p| fn_ptr(p)),
            config,
        })
    }
}

/// `dlsym` 取到的地址 → 函数指针。目标类型由调用处的字段类型推出。
///
/// 安全：调用方保证 `p` 非空、且确实是签名为 `F` 的 C 函数（符号名与 fontconfig 公开
/// 头文件的声明一一对应）。`F` 必须是指针大小的函数指针类型。
unsafe fn fn_ptr<F: Copy>(p: *mut c_void) -> F {
    debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
    unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) }
}

/// fontconfig 是否可用。
pub(crate) fn available() -> bool {
    api().is_some()
}

/// 一个候选字体：文件 + 集合内序号 + 覆盖字符集。
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub index: u32,
    charset: Ptr,
}

// charset 是 `FcCharSetCopy` 出来的引用计数副本，只读查询，跨线程安全。
unsafe impl Send for Candidate {}

impl Candidate {
    /// 该字体是否覆盖 `c`。拿不到字符集时按「覆盖」答，交给后面的 cmap 查询兜底。
    pub fn has_char(&self, c: char) -> bool {
        match api() {
            Some(a) if !self.charset.is_null() => unsafe {
                (a.charset_has_char)(self.charset, c as u32) != 0
            },
            _ => true,
        }
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        if let Some(a) = api()
            && !self.charset.is_null()
        {
            unsafe { (a.charset_destroy)(self.charset) };
        }
    }
}

/// CSS 字重（100..900）→ fontconfig 字重刻度。
fn fc_weight(a: &Api, css: u16) -> c_int {
    match a.weight_from_opentype {
        Some(f) => unsafe { f(css as c_int) },
        // 老 fontconfig：按官方常量表分段近似（REGULAR=80、MEDIUM=100、DEMIBOLD=180、BOLD=200）。
        None => match css {
            0..=150 => 0,
            151..=250 => 40,
            251..=350 => 50,
            351..=450 => 80,
            451..=550 => 100,
            551..=650 => 180,
            651..=750 => 200,
            751..=850 => 205,
            _ => 210,
        },
    }
}

/// fontconfig 字重刻度 → CSS 字重。
fn css_weight(a: &Api, fc: c_int) -> i32 {
    match a.weight_to_opentype {
        Some(f) => unsafe { f(fc) },
        None => match fc {
            ..=20 => 100,
            21..=45 => 200,
            46..=60 => 300,
            61..=90 => 400,
            91..=140 => 500,
            141..=190 => 600,
            191..=202 => 700,
            203..=207 => 800,
            _ => 900,
        },
    }
}

/// 名字语法里的 `-`、`:`、`,`、`\` 有特殊含义，需转义。
fn escape(family: &str) -> String {
    let mut esc = String::with_capacity(family.len());
    for ch in family.chars() {
        if matches!(ch, '-' | ':' | ',' | '\\') {
            esc.push('\\');
        }
        esc.push(ch);
    }
    esc
}

/// 按族名 / 字重 / 语言查询，返回 fontconfig 排好序的候选列表（首项即最佳匹配，
/// 其后是按覆盖度去重后的回退链）。`None` = fontconfig 不可用或查询失败。
pub(crate) fn sort(family: &str, weight: u16, lang: &str) -> Option<Vec<Candidate>> {
    let a = api()?;
    let query = format!(
        "{}:weight={}:lang={lang}",
        escape(family),
        fc_weight(a, weight)
    );
    sort_query(a, &query)
}

/// 系统配置的彩色 emoji 字体：通用族名 `emoji`（fontconfig ≥ 2.13.9x 的 `45-generic.conf` /
/// `60-generic.conf` 把它映射到 Noto Color Emoji / Twemoji 等）并要求 `color=true`。
/// 返回排序后的候选，调用方加载后自己再核对「真有彩色表」——老 fontconfig 不认识 `emoji`
/// 这个名字时会替换出一款普通字体，`color` 属性也可能缺席。
pub(crate) fn sort_emoji() -> Option<Vec<Candidate>> {
    let a = api()?;
    sort_query(a, "emoji:color=True").or_else(|| sort_query(a, "emoji"))
}

fn sort_query(a: &Api, query: &str) -> Option<Vec<Candidate>> {
    let cq = CString::new(query).ok()?;
    unsafe {
        let pat = (a.name_parse)(cq.as_ptr() as *const u8);
        if pat.is_null() {
            return None;
        }
        (a.config_substitute)(a.config, pat, FC_MATCH_PATTERN);
        (a.default_substitute)(pat);
        let mut res: c_int = 0;
        // trim=1：去掉覆盖度不增加的候选，回退链因此短得多。
        let set = (a.font_sort)(a.config, pat, 1, std::ptr::null_mut(), &mut res);
        (a.pattern_destroy)(pat);
        if set.is_null() {
            return None;
        }
        let fs = &*set;
        let mut out = Vec::new();
        for i in 0..fs.nfont.max(0) as usize {
            let p = *fs.fonts.add(i);
            let Some(path) = get_string(a, p, c"file", 0) else {
                continue;
            };
            let mut index: c_int = 0;
            (a.pattern_get_integer)(p, c"index".as_ptr(), 0, &mut index);
            let mut cs: Ptr = std::ptr::null_mut();
            let charset = if (a.pattern_get_charset)(p, c"charset".as_ptr(), 0, &mut cs)
                == FC_RESULT_MATCH
                && !cs.is_null()
            {
                (a.charset_copy)(cs)
            } else {
                std::ptr::null_mut()
            };
            out.push(Candidate {
                path: PathBuf::from(path),
                index: index.max(0) as u32,
                charset,
            });
        }
        (a.font_set_destroy)(set);
        Some(out)
    }
}

/// 读 pattern 里 `object` 的第 `n` 个字符串值。
unsafe fn get_string(a: &Api, p: Ptr, object: &CStr, n: c_int) -> Option<String> {
    let mut s: *const u8 = std::ptr::null();
    unsafe {
        if (a.pattern_get_string)(p, object.as_ptr(), n, &mut s) != FC_RESULT_MATCH || s.is_null() {
            return None;
        }
        Some(
            CStr::from_ptr(s as *const c_char)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// `FcFontList`：列出 `object`（`family` / `fullname`）等于 `value` 的全部 face，
/// 对每个 face 调 `each(pattern)`。fontconfig 比对族名时忽略大小写与空白，与 DirectWrite
/// `FindFamilyName` 的大小写不敏感同口径。
fn list(
    object: &CStr,
    value: &str,
    props: &[&CStr],
    mut each: impl FnMut(&Api, Ptr),
) -> Option<()> {
    let a = api()?;
    let cv = CString::new(value).ok()?;
    unsafe {
        let pat = (a.pattern_create)();
        if pat.is_null() {
            return None;
        }
        (a.pattern_add_string)(pat, object.as_ptr(), cv.as_ptr() as *const u8);
        let os = (a.object_set_create)();
        for p in props {
            (a.object_set_add)(os, p.as_ptr());
        }
        let set = (a.font_list)(a.config, pat, os);
        (a.object_set_destroy)(os);
        (a.pattern_destroy)(pat);
        if set.is_null() {
            return None;
        }
        let fs = &*set;
        for i in 0..fs.nfont.max(0) as usize {
            each(a, *fs.fonts.add(i));
        }
        (a.font_set_destroy)(set);
    }
    Some(())
}

/// 系统字体集里有没有这个**家族名**。`None` = fontconfig 不可用（查不了）。
///
/// 用 `FcFontList` 而非 `FcFontMatch`：后者对不存在的名字也会按替换规则给出「最近似」的
/// 字体，正是字体缺失告警要抓的情形。家族名的本地化别名（「思源黑体」之类）也算。
pub(crate) fn family_exists(family: &str) -> Option<bool> {
    let mut n = 0usize;
    list(c"family", family, &[c"family"], |_, _| n += 1)?;
    Some(n > 0)
}

/// 按 face **全名**（`fullname`，如「Noto Sans CJK SC Bold」）找它所属的家族名与 CSS 字重。
/// 同名有多个 face 时取非斜体里最接近常规字重的那个。
pub(crate) fn find_face(name: &str) -> Option<(String, i32)> {
    let mut best: Option<(String, i32, bool)> = None;
    list(
        c"fullname",
        name,
        &[c"family", c"weight", c"slant"],
        |a, p| unsafe {
            let Some(family) = get_string(a, p, c"family", 0) else {
                return;
            };
            let mut w: c_int = 80;
            (a.pattern_get_integer)(p, c"weight".as_ptr(), 0, &mut w);
            let mut slant: c_int = 0;
            (a.pattern_get_integer)(p, c"slant".as_ptr(), 0, &mut slant);
            let (weight, italic) = (css_weight(a, w), slant != 0);
            let better = best.as_ref().is_none_or(|(_, bw, bi)| {
                (italic, (weight - 400).abs()) < (*bi, (*bw - 400).abs())
            });
            if better {
                best = Some((family, weight, italic));
            }
        },
    )?;
    best.map(|(f, w, _)| (f, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_special_chars() {
        assert_eq!(escape("A-B:C,D\\E"), "A\\-B\\:C\\,D\\\\E");
        assert_eq!(escape("Noto Sans CJK SC"), "Noto Sans CJK SC");
    }
}
