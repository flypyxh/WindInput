//! 字体仓库：按「族名链 + 字重」解析出一条回退链，按字符挑出覆盖它的字体。
//!
//! 移植自姊妹仓 wind-ui-rust `src/text/linux/store.rs`（同一作者，MIT/Apache-2.0）。与原版
//! 的差别：链的键是**一串**族名（`ui.font` 方案的默认链 / 脚本指派链，见 `script::FontPlan`）
//! 而非单个族名；不做斜体；语言提示默认 `zh-cn`；彩色 emoji 字体（CBDT / sbix / COLR /
//! OpenType-SVG）另有一条专门的查找路径（[`Store::color_face_for`] / [`Store::emoji_face_for`]）
//! 与簇整形（[`Store::shape`]）；另可按文件加载拆字字根字体。
//!
//! 进程级共享（一个 `Mutex` 包住）：同一个字体文件不论被几个窗口、几条链引用都只映射
//! 一次。字体文件以 `mmap` 只读映射且**永不卸载**——映射页是文件后备的共享页，不计入
//! 私有内存；动辄 20MB 的 CJK 字体真正触到的只是用过的那些字形所在的页。

use std::collections::HashMap;
use std::ffi::{c_int, c_long, c_void};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::fontconfig;

pub(crate) type FaceId = u32;
pub(crate) type ChainId = usize;

/// 一个已加载的字体。度量均为字体单位（`units_per_em` 为分母）。
pub(crate) struct FaceData {
    pub face: ttf_parser::Face<'static>,
    pub upem: f32,
    /// 基线以上高度（正）。
    pub ascent: f32,
    /// 基线以下高度（正）。
    pub descent: f32,
    pub line_gap: f32,
    /// 字体自身的字重（CSS 刻度）——判断要不要合成粗体。
    pub weight: u16,
    /// 有没有矢量轮廓（`glyf` / `CFF` / `CFF2`）。
    pub has_outlines: bool,
    /// 有没有彩色字形表（`CBDT` / `sbix` / `COLR` / `SVG `），见 `color.rs`。Noto Color Emoji
    /// 这类纯位图字体没有轮廓，只能走彩色路径。
    pub color: bool,
}

impl FaceData {
    fn new(face: ttf_parser::Face<'static>) -> Self {
        let upem = face.units_per_em().max(1) as f32;
        // hhea 的 ascender/descender 是桌面上最通行的行距来源；ttf-parser 已按
        // USE_TYPO_METRICS 标志在 hhea 与 OS/2 typo 之间择一。
        let mut ascent = face.ascender() as f32;
        let mut descent = -(face.descender() as f32);
        if ascent + descent <= 0.0 {
            ascent = upem * 0.8;
            descent = upem * 0.2;
        }
        let t = face.tables();
        let has_outlines = t.glyf.is_some() || t.cff.is_some() || t.cff2.is_some();
        let color = t.cbdt.is_some() || t.sbix.is_some() || t.colr.is_some() || t.svg.is_some();
        Self {
            upem,
            ascent,
            descent,
            line_gap: face.line_gap().max(0) as f32,
            weight: face.weight().to_number(),
            has_outlines,
            color,
            face,
        }
    }
}

enum Source {
    Fc(fontconfig::Candidate),
    File(PathBuf, u32),
}

struct Cand {
    src: Source,
    /// `None` = 还没加载过；`Some(None)` = 加载失败（或不可用），不再重试。
    loaded: Option<Option<FaceId>>,
}

impl Cand {
    fn path(&self) -> (&Path, u32) {
        match &self.src {
            Source::Fc(c) => (&c.path, c.index),
            Source::File(p, i) => (p, *i),
        }
    }
    fn may_have(&self, c: char) -> bool {
        match &self.src {
            Source::Fc(fc) => fc.has_char(c),
            Source::File(..) => true,
        }
    }
}

struct Chain {
    cands: Vec<Cand>,
    by_char: HashMap<char, Option<(FaceId, u16)>>,
    /// [`Store::color_face_for`] 的结果缓存。
    color_by_char: HashMap<char, Option<FaceId>>,
}

/// 一个 emoji 簇的整形结果：[(字形, 前进宽度（字体单位）)]。
pub(crate) type ShapedCluster = Vec<(u16, i32)>;

/// emoji 簇整形缓存的条目上限，满则整体清空（同测量缓存的做法）。
const SHAPE_CACHE_CAP: usize = 2048;
/// 解析系统 emoji 字体时最多加载几个 fontconfig 候选（排在前面的才是 `emoji` 的映射目标，
/// 后面是按覆盖度补上的普通字体，没必要逐个映射进来）。
const EMOJI_CANDS_MAX: usize = 4;

#[derive(Default)]
pub(crate) struct Store {
    faces: Vec<FaceData>,
    by_path: HashMap<(PathBuf, u32), Option<FaceId>>,
    chains: Vec<Chain>,
    chain_keys: HashMap<(Vec<String>, u16), ChainId>,
    scanned: Option<Vec<PathBuf>>,
    /// 系统的彩色 emoji 字体（fontconfig `emoji` 族），首次遇到 emoji 簇时解析。
    emoji: Option<Vec<FaceId>>,
    /// 彩色字体的整形器（按需建，只给 emoji 簇用）。
    shapers: HashMap<FaceId, rustybuzz::Face<'static>>,
    /// emoji 簇整形结果：字体 → 簇文本 → [(字形, 前进宽度（字体单位）)]（`None` = 字体缺字）。
    shaped: HashMap<FaceId, HashMap<String, Option<ShapedCluster>>>,
    shaped_len: usize,
}

pub(crate) fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    let m = STORE.get_or_init(|| Mutex::new(Store::default()));
    // 持锁期间 panic 只可能来自本模块的逻辑错误；毒化后照用内部数据即可（缓存而已）。
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

/// fontconfig 的通用族名（别名，不是真实家族）。它们查不到「存在」，但一定能匹配出字体。
pub(crate) fn is_generic_family(name: &str) -> bool {
    [
        "sans-serif",
        "sans",
        "serif",
        "monospace",
        "mono",
        "system-ui",
        "emoji",
    ]
    .iter()
    .any(|g| g.eq_ignore_ascii_case(name.trim()))
}

impl Store {
    pub fn face(&self, id: FaceId) -> &FaceData {
        &self.faces[id as usize]
    }

    /// 取（或建）一条回退链。`families[0]` 是 base family（空表 = 通用 `sans-serif`），
    /// 其余是用户声明的回退顺序。
    ///
    /// 链的组成：base 的最佳匹配 → 各声明回退族（**仅当系统里真有它**：不存在的名字
    /// fontconfig 也会替换出一款字体，那会抢在后面真正想要的字体之前）→ base 的 fontconfig
    /// 回退序列（按语言提示排序，CJK 字体在 `zh-cn` 下靠前）。
    pub fn chain(&mut self, families: &[String], weight: u16) -> ChainId {
        let key = (families.to_vec(), weight);
        if let Some(&id) = self.chain_keys.get(&key) {
            return id;
        }
        let base = families.first().map_or("sans-serif", |s| s.as_str());
        let lang = default_lang();
        let cands: Vec<Cand> = match fontconfig::sort(base, weight, lang) {
            Some(list) if !list.is_empty() => {
                let mut rest = list.into_iter();
                let mut v: Vec<fontconfig::Candidate> = rest.next().into_iter().collect();
                for fam in families.iter().skip(1) {
                    if !is_generic_family(fam) && fontconfig::family_exists(fam) != Some(true) {
                        tracing::warn!("回退字体「{fam}」不在系统字体集里（fontconfig），已跳过");
                        continue;
                    }
                    if let Some(c) =
                        fontconfig::sort(fam, weight, lang).and_then(|l| l.into_iter().next())
                    {
                        v.push(c);
                    }
                }
                v.extend(rest);
                // 同一个 face 只留第一次出现（base 的回退序列里多半还有它们）。
                let mut seen = std::collections::HashSet::new();
                v.into_iter()
                    .filter(|c| seen.insert((c.path.clone(), c.index)))
                    .map(|c| Cand {
                        src: Source::Fc(c),
                        loaded: None,
                    })
                    .collect()
            }
            _ => {
                static NOTICE: std::sync::Once = std::sync::Once::new();
                NOTICE.call_once(|| {
                    tracing::info!(
                        "fontconfig 不可用（available={}），改为扫描字体目录选字",
                        fontconfig::available()
                    );
                });
                self.scan_fallback(families)
                    .into_iter()
                    .map(|p| Cand {
                        src: Source::File(p, 0),
                        loaded: None,
                    })
                    .collect()
            }
        };
        let id = self.chains.len();
        self.chains.push(Chain {
            cands,
            by_char: HashMap::new(),
            color_by_char: HashMap::new(),
        });
        self.chain_keys.insert(key, id);
        match self.primary(id) {
            Some(f) => {
                let (path, index) = self
                    .by_path
                    .iter()
                    .find(|(_, v)| **v == Some(f))
                    .map(|(k, _)| (k.0.display().to_string(), k.1))
                    .unwrap_or_default();
                tracing::debug!(
                    "字体链 {families:?} weight={weight} lang={lang} → 主字体 {path}#{index}，候选 {} 个",
                    self.chains[id].cands.len()
                );
            }
            None => tracing::warn!(
                "字体链 {families:?} 解析不出任何可用字体（fontconfig={}），文字将画不出来",
                fontconfig::available()
            ),
        }
        id
    }

    /// 链上第一个能加载的字体——行度量（行高、基线）以它为准。
    pub fn primary(&mut self, chain: ChainId) -> Option<FaceId> {
        (0..self.chains[chain].cands.len()).find_map(|i| self.load_cand(chain, i))
    }

    /// 为字符 `c` 挑字体：沿链找第一个 cmap 里有它、且有矢量轮廓的**非彩色**字体；没有才用
    /// 链上的彩色字体（这条路径走的是文本呈现的字——❤、©、带 VS15 的 emoji——彩色 emoji 字体
    /// 排在前面也不该抢走它们）。都没有时落回主字体的 `.notdef`（通常是个方框）——好过整字
    /// 消失、让人以为输入没生效，与 DirectWrite / CoreText 对无字形字符的表现一致。
    pub fn glyph_for(&mut self, chain: ChainId, c: char) -> Option<(FaceId, u16)> {
        if let Some(hit) = self.chains[chain].by_char.get(&c) {
            return *hit;
        }
        let (mut found, mut color_hit) = (None, None);
        for i in 0..self.chains[chain].cands.len() {
            if !self.chains[chain].cands[i].may_have(c) {
                continue;
            }
            let Some(id) = self.load_cand(chain, i) else {
                continue;
            };
            let f = &self.faces[id as usize];
            if !f.has_outlines && !f.color {
                continue;
            }
            if let Some(g) = f.face.glyph_index(c)
                && g.0 != 0
            {
                if !f.color {
                    found = Some((id, g.0));
                    break;
                }
                color_hit = color_hit.or(Some((id, g.0)));
            }
        }
        found = found.or(color_hit);
        if found.is_none() {
            found = self.primary(chain).map(|id| (id, 0));
        }
        self.chains[chain].by_char.insert(c, found);
        found
    }

    /// 链上第一个 cmap 里有 `c` 的**彩色**字体（emoji 簇优先用它：用户在方案里给 emoji 指派的
    /// 字体、或 fontconfig 回退序列里排进来的 Noto Color Emoji）。
    pub fn color_face_for(&mut self, chain: ChainId, c: char) -> Option<FaceId> {
        if let Some(hit) = self.chains[chain].color_by_char.get(&c) {
            return *hit;
        }
        let mut found = None;
        for i in 0..self.chains[chain].cands.len() {
            if !self.chains[chain].cands[i].may_have(c) {
                continue;
            }
            let Some(id) = self.load_cand(chain, i) else {
                continue;
            };
            let f = &self.faces[id as usize];
            if f.color && f.face.glyph_index(c).is_some_and(|g| g.0 != 0) {
                found = Some(id);
                break;
            }
        }
        self.chains[chain].color_by_char.insert(c, found);
        found
    }

    /// 系统彩色 emoji 字体里第一个 cmap 有 `c` 的（链上没有彩色字体覆盖它时的兜底）。
    pub fn emoji_face_for(&mut self, c: char) -> Option<FaceId> {
        if self.emoji.is_none() {
            self.emoji = Some(self.resolve_emoji());
        }
        let faces = &self.faces;
        self.emoji.as_ref()?.iter().copied().find(|&id| {
            faces[id as usize]
                .face
                .glyph_index(c)
                .is_some_and(|g| g.0 != 0)
        })
    }

    /// 解析系统彩色 emoji 字体：fontconfig `emoji:color=true`；没有 fontconfig 时扫字体目录里
    /// 文件名带 `emoji` 的。只留真有彩色字形表的（老 fontconfig 不认 `emoji` 会替换出普通字体）。
    fn resolve_emoji(&mut self) -> Vec<FaceId> {
        let paths: Vec<(PathBuf, u32)> = match fontconfig::sort_emoji() {
            Some(list) if !list.is_empty() => list
                .into_iter()
                .take(EMOJI_CANDS_MAX)
                .map(|c| (c.path.clone(), c.index))
                .collect(),
            _ => self
                .scanned
                .get_or_insert_with(scan_font_dirs)
                .iter()
                .filter(|p| {
                    normalize(&p.file_name().unwrap_or_default().to_string_lossy())
                        .contains("emoji")
                })
                .map(|p| (p.clone(), 0))
                .collect(),
        };
        let mut out: Vec<FaceId> = Vec::new();
        for (p, i) in paths {
            if let Some(id) = self.load_file(&p, i)
                && self.faces[id as usize].color
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        match out.first() {
            Some(_) => tracing::debug!("彩色 emoji 字体：{} 个", out.len()),
            None => tracing::info!("系统里没有彩色 emoji 字体，emoji 将按单色字形或方框画出"),
        }
        out
    }

    /// 用彩色字体整形一个 emoji 簇（GSUB 连字把 ZWJ 组合、国旗、键帽、肤色合成一个字形）。
    /// 返回 [(字形, 前进宽度（字体单位）)]；字体不支持的组合会原样拆成多个字形。字体不收录的
    /// 零宽成分（VS16、ZWJ、标签）落成的 0 号字形已剔除；除此之外还有字落成 0 号字形（字体真
    /// 缺这个字）时返回 `None`，由调用方逐字排、各自找字体，不静默吞字。
    pub fn shape(&mut self, face: FaceId, text: &str) -> Option<ShapedCluster> {
        if let Some(v) = self.shaped.get(&face).and_then(|m| m.get(text)) {
            return v.clone();
        }
        if self.shaped_len >= SHAPE_CACHE_CAP {
            self.shaped.clear();
            self.shaped_len = 0;
        }
        let faces = &self.faces;
        let rb = self
            .shapers
            .entry(face)
            .or_insert_with(|| rustybuzz::Face::from_face(faces[face as usize].face.clone()));
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        let out = rustybuzz::shape(rb, &[], buf);
        // 0 号字形按它来自的字（`cluster` 是 UTF-8 字节下标）分两种：零宽成分丢掉，真缺字作废。
        let missing = |g: &rustybuzz::GlyphInfo| {
            !text
                .get(g.cluster as usize..)
                .and_then(|t| t.chars().next())
                .is_some_and(super::emoji::is_invisible_component)
        };
        let infos = out.glyph_infos();
        let v = (!infos.iter().any(|g| g.glyph_id == 0 && missing(g))).then(|| {
            infos
                .iter()
                .zip(out.glyph_positions())
                .filter(|(g, _)| g.glyph_id != 0)
                .map(|(g, p)| (g.glyph_id as u16, p.x_advance))
                .collect::<Vec<_>>()
        });
        self.shaped
            .entry(face)
            .or_default()
            .insert(text.to_string(), v.clone());
        self.shaped_len += 1;
        v
    }

    /// 按路径加载一个字体文件（拆字字根字体用）。失败返回 `None`。
    pub fn load_file(&mut self, path: &Path, index: u32) -> Option<FaceId> {
        let key = (path.to_path_buf(), index);
        if let Some(&r) = self.by_path.get(&key) {
            return r;
        }
        let r = map_file(path)
            .and_then(|data| ttf_parser::Face::parse(data, index).ok())
            .map(|face| {
                self.faces.push(FaceData::new(face));
                (self.faces.len() - 1) as FaceId
            });
        if r.is_none() {
            tracing::warn!("字体加载失败：{}#{index}", path.display());
        }
        self.by_path.insert(key, r);
        r
    }

    fn load_cand(&mut self, chain: ChainId, i: usize) -> Option<FaceId> {
        if let Some(l) = self.chains[chain].cands[i].loaded {
            return l;
        }
        let (path, index) = self.chains[chain].cands[i].path();
        let (path, index) = (path.to_path_buf(), index);
        let id = self.load_file(&path, index);
        self.chains[chain].cands[i].loaded = Some(id);
        id
    }

    /// 无 fontconfig 时的兜底：扫常见字体目录，按偏好排序。
    fn scan_fallback(&mut self, families: &[String]) -> Vec<PathBuf> {
        let all = self.scanned.get_or_insert_with(scan_font_dirs).clone();
        let wants: Vec<String> = families.iter().map(|f| normalize(f)).collect();
        let rank = |p: &PathBuf| -> usize {
            let name = normalize(&p.file_name().unwrap_or_default().to_string_lossy());
            if let Some(i) = wants.iter().position(|w| !w.is_empty() && name.contains(w)) {
                return i;
            }
            PREFERRED
                .iter()
                .position(|k| name.contains(k))
                .map(|i| i + wants.len())
                .unwrap_or(usize::MAX)
        };
        let mut v: Vec<PathBuf> = all.into_iter().filter(|p| rank(p) != usize::MAX).collect();
        v.sort_by_key(|p| rank(p));
        v
    }
}

/// 无 fontconfig 时的偏好序（小写、去空格与连字符后匹配文件名）。CJK 字体排在前面：
/// 它们同时带拉丁字形，放前面可以让中西文同出一款字体、基线与字重一致。
const PREFERRED: &[&str] = &[
    "notosanscjk",
    "sourcehansans",
    "wqyzenhei",
    "wqymicrohei",
    "droidsansfallback",
    "notosans",
    "dejavusans",
    "liberationsans",
    "ubuntu",
    "cantarell",
];

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn scan_font_dirs() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        roots.push(home.join(".local/share/fonts"));
        roots.push(home.join(".fonts"));
    }
    let mut out = Vec::new();
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref(),
                Some("ttf" | "otf" | "ttc" | "otc")
            ) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// 给 fontconfig 的语言提示：同一个汉字在简/繁/日/韩字体里字形不同，且 `sans-serif` 在
/// CJK 语言下才会优先落到 CJK 字体上（中西文同源、基线一致）。
///
/// 与姊妹仓不同，locale 不是中文时**也**给 `zh-cn`：这是中文输入法的候选窗，DirectWrite 侧
/// 同样把 locale 钉成 `zh-cn`。英文 locale 下若不给，汉字会落到 fontconfig 回退序列里
/// 第一个 CJK 字体上，可能是日文字形。
pub(crate) fn default_lang() -> &'static str {
    static LANG: OnceLock<&'static str> = OnceLock::new();
    LANG.get_or_init(|| {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .map_or("zh-cn", |v| lang_of_locale(&v))
    })
}

fn lang_of_locale(v: &str) -> &'static str {
    let v = v.to_ascii_lowercase().replace('-', "_");
    if v.starts_with("zh_tw") {
        "zh-tw"
    } else if v.starts_with("zh_hk") || v.starts_with("zh_mo") {
        "zh-hk"
    } else {
        "zh-cn"
    }
}

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        off: c_long,
    ) -> *mut c_void;
}

const PROT_READ: c_int = 1;
const MAP_PRIVATE: c_int = 2;

/// 只读映射整个文件，返回永不释放的切片（见模块头：字体不卸载）。
fn map_file(path: &Path) -> Option<&'static [u8]> {
    let f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len() as usize;
    if len == 0 {
        return None;
    }
    // 安全：只读私有映射，长度取自文件元数据；映射在 fd 关闭后依然有效。
    let p = unsafe {
        mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ,
            MAP_PRIVATE,
            f.as_raw_fd(),
            0,
        )
    };
    if p.is_null() || p as isize == -1 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(p as *const u8, len) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_maps_to_fontconfig_lang() {
        assert_eq!(lang_of_locale("zh_CN.UTF-8"), "zh-cn");
        assert_eq!(lang_of_locale("zh_TW.UTF-8"), "zh-tw");
        assert_eq!(lang_of_locale("zh-HK"), "zh-hk");
        assert_eq!(
            lang_of_locale("en_US.UTF-8"),
            "zh-cn",
            "中文输入法：非中文 locale 也按简中"
        );
        assert_eq!(lang_of_locale("C"), "zh-cn");
    }

    #[test]
    fn normalize_strips_separators_and_case() {
        assert_eq!(normalize("Noto Sans-CJK_SC"), "notosanscjksc");
    }

    #[test]
    fn generic_family_names() {
        assert!(is_generic_family("sans-serif"));
        assert!(is_generic_family(" Monospace "));
        assert!(!is_generic_family("Noto Sans CJK SC"));
    }
}
