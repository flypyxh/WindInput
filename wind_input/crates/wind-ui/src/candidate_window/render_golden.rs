//! 渲染 golden 对拍：出厂主题 × 出厂注释 / 气泡样例 × 常态 / 选中 / 悬停 × 四种排布。
//!
//! # 守的是什么
//!
//! 分段着色（`docs/design/text-span-colors.md` §13.2）要求「出厂外观逐字节不变」。
//! 这件事单靠「新代码的某条断言」证明不了——新代码录的参照只能证明「和自己一样」。
//! 所以参照必须在**动渲染层之前**录下、入库，之后每一步改动都拿它对拍。
//!
//! 每个用例录三样东西，Linux mock 后端下全部确定：
//! - **View 树转储**（[`View::debug_dump`]）：布局、颜色、字号、字重、裁剪、旋转逐字段；
//! - **绘制调用日志**：mock `TextRenderer` 记下的每一次文字绘制（文本、坐标、样式、颜色）；
//! - **像素摘要**：mock 不出字形，但背景 / 边框 / 圆角 / 阴影是 tiny-skia 真光栅化，
//!   对整块缓冲取 FNV-1a 摘要（不用 `DefaultHasher`：它的算法不作承诺，换工具链会假红）。
//!
//! # 更新参照
//!
//! 只在**有意**改变出厂外观时才重录：`WIND_UI_BLESS_GOLDEN=1 cargo test -p wind-ui --lib
//! render_golden`，然后在提交里说明为什么变。改渲染层时 golden 红了，默认是回归。
//!
//! 重录记录：2026-09-28 `_base` 配了气泡的角色色（设计 §18），出厂外观有意改变。重录的差异
//! **只有**气泡文字多出颜色区间（`runs=` / `draw_runs`），布局、字号、像素摘要逐字节不变；
//! 候选窗部分（含注释）与分段着色之前逐字节相同——注释出厂不上色。
//!
//! 重录记录：2026-10-12 `ViewImage` 新增 `scale` 字段（SVG center/tile 单元 = 文档尺寸 × DPI），
//! 差异**只有**转储里翻页箭头图多出 `scale: 1.0`，像素摘要与其余各行逐字节不变。

use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use wind_ui_types::{SpanStyle, StyledText, TooltipDoc, TooltipLine, TooltipSection};

/// 出厂主题：`data/themes` 下的全部主题（含两个抽象 base：它们同样可加载，且是其余主题的
/// 公共祖先）。按目录枚举而非硬编码，新加的出厂主题缺 golden 文件时对拍直接报出来。
fn themes() -> Vec<String> {
    let ids = wind_theme::list_theme_ids(&themes_dir());
    for must in [
        "_base",
        "_qingfeng",
        "default",
        "amber",
        "jade",
        "violet",
        "msime",
    ] {
        assert!(
            ids.iter().any(|i| i == must),
            "出厂主题枚举漏了 {must}：{ids:?}"
        );
    }
    ids
}

/// 排布：`(名字, vertical, rotated, upright, 内联编码)`。前三位的合法组合只有四个（见
/// `set_orientation`）；内联编码另走 `preedit_view` 的 build 回调，直立态下它也逐格扶正。
const LAYOUTS: &[(&str, bool, bool, bool, bool)] = &[
    ("横排", false, false, false, false),
    ("竖排", true, false, false, false),
    ("旋转", false, true, false, false),
    ("直立", false, true, true, false),
    ("横排·内联编码", false, false, false, true),
    ("直立·内联编码", false, true, true, true),
];

fn themes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/themes")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/render_golden")
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// 一段带角色的文字（协调器模板引擎产出的片段形态）。
fn piece(out: &mut StyledText, text: &str, role: &'static str, in_title: bool) {
    out.push(
        text,
        &SpanStyle {
            role: Some(role),
            in_title,
            ..Default::default()
        },
    );
}

fn role_text(text: &str, role: &'static str) -> StyledText {
    let mut t = StyledText::new();
    piece(&mut t, text, role, false);
    t
}

/// 出厂注释模板 `${code_hint|code_rev|shuangpin}` 渲染出的样子：短编码、空注释、多字词编码。
///
/// ★ 带着协调器真实产出的**片段角色**（P2 起模板引擎恒产出片段）：出厂主题不给注释配角色
/// （§18），这些片段必须全部解析到正文色而被丢弃，候选窗部分才与分段着色之前逐字节相同（§6.4）。
fn candidates() -> Vec<CandidateItem> {
    let c = |text: &str, comment: StyledText| CandidateItem {
        text_styled: Default::default(),
        text: text.to_string(),
        code: String::new(),
        label: String::new(),
        tooltip: Default::default(),
        comment,
        comment_above: Default::default(),
        no_index: false,
    };
    // 下标 0 选中、1 悬停、2/3 常态（一条无注释、一条有）。
    vec![
        c("好", role_text("vb", "code_hint")),
        c("号", role_text("kg", "code_rev")),
        c("浩", StyledText::new()),
        c("你好", role_text("wqvb", "shuangpin")),
    ]
}

/// 出厂气泡段（完整原文关、编码、拼音逐字）渲染出的样子，带协调器产出的片段角色
/// （段名字面 title、段名里的变量 in_title、逐字行 char / literal / readings）。
fn tooltip_doc() -> TooltipDoc {
    let per_char = |ch: &str, readings: &str, raw| {
        let mut t = StyledText::new();
        piece(&mut t, ch, "char", false);
        piece(&mut t, "：", "literal", false);
        piece(&mut t, readings, "readings", false);
        TooltipLine { text: t, raw }
    };
    let mut code_title = StyledText::new();
    piece(&mut code_title, "编码(", "title", true);
    piece(&mut code_title, "五笔", "code_source", true);
    piece(&mut code_title, ")", "title", true);
    let mut pinyin_title = StyledText::new();
    piece(&mut pinyin_title, "拼音", "title", true);
    TooltipDoc {
        sections: vec![
            TooltipSection {
                title: Some(code_title),
                inline: false,
                lines: vec![TooltipLine {
                    text: role_text("wqvb", "word_code"),
                    raw: 0,
                }],
            },
            TooltipSection {
                title: Some(pinyin_title),
                inline: false,
                lines: vec![per_char("你", "nǐ", 0), per_char("好", "hǎo/hào", 1)],
            },
        ],
    }
}

/// 候选窗在给定排布下的用例（选中 0、悬停 1），逐排布拼成文本。
fn render_windows(
    theme: &wind_theme::Resolved,
    layouts: &[(&str, bool, bool, bool, bool)],
    cands: fn() -> Vec<CandidateItem>,
) -> String {
    let mut out = String::new();
    for &(label, vertical, rotated, upright, embedded) in layouts {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut w = CandidateWindow::new(CandidateWindowConfig::default(), tx).unwrap();
        w.scale = 1.0;
        w.set_theme(theme.clone());
        w.set_orientation(vertical, rotated, upright);
        w.set_preedit_embedded(embedded);
        // 选中 0、悬停 1：一帧里三态俱全。模式徽标给一个，直立态下它也逐格扶正。
        w.update("nihao", 3, "拼", cands(), 0, 1, 1, 2);
        let mut root = w.build_tree(false);
        root.layout(0.0, 0.0, &w.text_renderer);
        let (mw, mh) = root.measured_size();
        let (bw, bh) = (mw.ceil().max(1.0) as u32, mh.ceil().max(1.0) as u32);
        let mut buf = vec![0u8; (bw * bh * 4) as usize];
        let _ = w.text_renderer.take_draw_log();
        root.paint(&mut buf, bw, bh, &w.text_renderer);
        out.push_str(&format!("== 候选窗 {label} ==\n"));
        out.push_str(&root.debug_dump());
        out.push_str(&format!(
            "-- 绘制 {bw}x{bh} 像素摘要 {:016x}\n",
            fnv1a(&buf)
        ));
        for l in w.text_renderer.take_draw_log() {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

/// 一个主题（亮或暗）的全部用例，拼成一份文本。
fn render_theme(name: &str, dark: bool) -> String {
    let theme = wind_theme::load_resolved(&themes_dir(), name, dark)
        .unwrap_or_else(|e| panic!("加载出厂主题 {name}（dark={dark}）失败：{e}"));
    let mut out = render_windows(&theme, LAYOUTS, candidates);
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut tip = crate::tooltip::Tooltip::new(tx).unwrap();
    tip.set_theme(&theme);
    let (buf, w, h, log) = tip.golden_frame(&Arc::new(tooltip_doc()));
    out.push_str(&format!(
        "== 气泡 ==\n-- 绘制 {w}x{h} 像素摘要 {:016x}\n",
        fnv1a(&buf)
    ));
    for l in log {
        out.push_str(&l);
        out.push('\n');
    }
    // 主题资源的绝对路径随检出位置变，换成占位符。
    let dir = themes_dir().to_string_lossy().into_owned();
    out.replace(&dir, "<themes>")
}

#[test]
fn factory_render_matches_golden() {
    let bless = std::env::var_os("WIND_UI_BLESS_GOLDEN").is_some();
    let dir = golden_dir();
    let mut failed = Vec::new();
    for name in &themes() {
        for dark in [false, true] {
            let got = render_theme(name, dark);
            let file = dir.join(format!(
                "{name}-{}.txt",
                if dark { "dark" } else { "light" }
            ));
            if bless {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&file, &got).unwrap();
                continue;
            }
            let want = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("读 golden {} 失败：{e}", file.display()));
            if got != want {
                // 首个不同行，省得在上千行里肉眼找。
                let line = got
                    .lines()
                    .zip(want.lines())
                    .position(|(a, b)| a != b)
                    .unwrap_or(got.lines().count().min(want.lines().count()));
                failed.push(format!(
                    "{}：第 {} 行起不同\n  实得 {:?}\n  参照 {:?}",
                    file.display(),
                    line + 1,
                    got.lines().nth(line),
                    want.lines().nth(line),
                ));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "出厂渲染与 golden 不一致（有意改外观才用 WIND_UI_BLESS_GOLDEN=1 重录）：\n{}",
        failed.join("\n")
    );
}

/// 守 golden 本身不是空转：候选文字、注释、气泡文字都必须出现在绘制日志里，
/// 且注释确实有非空的那几条——否则上面的对拍测的是一个什么都没画的世界。
#[test]
fn golden_covers_comments_and_tooltip() {
    let got = render_theme("default", false);
    for needle in [
        "text=\"好\"",
        "text=\"vb\"",
        "text=\"kg\"",
        "text=\"wqvb\"",
        "text=\"[编码(五笔)]\\nwqvb\\n[拼音]\\n你：nǐ\\n好：hǎo/hào\"",
    ] {
        assert!(got.contains(needle), "golden 里找不到 {needle}");
    }
    // 候选窗（含注释）没有任何调用走 runs：注释出厂不上色，片段全部解析到正文色、被丢弃。
    let window = got.split("== 气泡 ==").next().unwrap();
    assert!(!window.contains("draw_runs"), "候选窗不应出现 draw_runs");
    assert!(!window.contains(" runs="), "候选窗 View 树不应带颜色区间");
    // `_base` 的气泡角色色（§18）：编码是 `tooltip_success`。
    assert!(
        got.contains("rgba: [129, 201, 149, 255]"),
        "气泡编码应按 _base 的 word_code 角色画成 tooltip_success"
    );
    // 防空转：输入确实带着片段。
    assert!(!candidates()[0].comment.spans().is_empty());
    assert!(!tooltip_doc().to_styled().spans().is_empty());
    // 直立态注释逐格切：`wqvb` 在直立段里是四个单字母叶子。
    let upright = got.split("== 候选窗 直立 ==").nth(1).unwrap();
    assert!(upright.contains("text=\"w\""), "直立态注释应逐格切开");
}

/// 上方注释条（设计 candidate-comment-above-line.md §3.3）：出厂 `default` 主题下横排 / 竖排
/// 各一帧（选中 0、悬停 1 ⇒ 常态 / 选中 / 悬停俱全；下标 2「浩」没有上段 ⇒ 占位叶子），外加直立态
/// 一帧钉「不支持上方条、上段合回右侧注释」。首录于上方条落地时（此前无此功能，参照即新代码
/// 产物，守的是之后的回归）。
fn above_candidates() -> Vec<CandidateItem> {
    let c = |text: &str, above: StyledText, comment: StyledText| CandidateItem {
        text_styled: Default::default(),
        text: text.to_string(),
        code: String::new(),
        label: String::new(),
        tooltip: Default::default(),
        comment,
        comment_above: above,
        no_index: false,
    };
    vec![
        c(
            "好",
            role_text("hǎo", "pinyin"),
            role_text("vb", "code_hint"),
        ),
        c(
            "号",
            role_text("hào", "pinyin"),
            role_text("kg", "code_rev"),
        ),
        c("浩", StyledText::new(), StyledText::new()),
        c(
            "你好",
            role_text("nǐ hǎo", "pinyin"),
            role_text("wqvb", "shuangpin"),
        ),
    ]
}

fn render_above() -> String {
    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let layouts: &[(&str, bool, bool, bool, bool)] = &[
        ("横排", false, false, false, false),
        ("竖排", true, false, false, false),
        ("直立", false, true, true, false),
    ];
    let dir = themes_dir().to_string_lossy().into_owned();
    render_windows(&theme, layouts, above_candidates).replace(&dir, "<themes>")
}

#[test]
fn comment_above_render_matches_golden() {
    let got = render_above();
    let file = golden_dir().join("comment_above-default-light.txt");
    if std::env::var_os("WIND_UI_BLESS_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&file, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("读 golden {} 失败：{e}", file.display()));
    if got != want {
        let line = got
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(got.lines().count().min(want.lines().count()));
        panic!(
            "上方注释条渲染与 golden 不一致（第 {} 行起）\n  实得 {:?}\n  参照 {:?}",
            line + 1,
            got.lines().nth(line),
            want.lines().nth(line)
        );
    }
}

/// 守上面那份 golden 不是空转：横 / 竖排真有 Column 与上段文字、第 3 条是透明占位；
/// 直立态没有 Column、上段逐格切进了右侧注释。
#[test]
fn comment_above_golden_covers_the_feature() {
    let got = render_above();
    let upright = got.split("== 候选窗 直立 ==").nth(1).unwrap();
    for label in ["横排", "竖排"] {
        let sec = got
            .split(&format!("== 候选窗 {label} =="))
            .nth(1)
            .unwrap()
            .split("== 候选窗")
            .next()
            .unwrap();
        assert!(
            (0..4).all(|i| column_item(sec, i)),
            "{label} 的 4 个 item 都应是 Column"
        );
        for needle in [
            "text=\"hǎo\"",
            "text=\"nǐ hǎo\"",
            "text=Some(\" \") color=[0, 0, 0, 0]",
        ] {
            assert!(sec.contains(needle), "{label} 缺 {needle}");
        }
    }
    assert!(
        !(0..4).any(|i| column_item(upright, i)),
        "直立态 item 不应是 Column"
    );
    assert!(!upright.contains("text=\"hǎo\""), "直立态不应有整段上方条");
    assert!(upright.contains("text=\"ǎ\""), "直立态上段应逐格合进注释");
}

/// 转储里 tag 为 `i` 的候选 item 是不是 Column 容器。
fn column_item(dump: &str, i: usize) -> bool {
    let tag = format!(" tag={i} ");
    dump.lines()
        .any(|l| l.trim_start().starts_with("Column") && l.contains(&tag))
}

// ══ 模糊音提示（设计 fuzzy-pinyin-hint.md）══
//
// 候选正文带分段（`text_styled`）：「苹(ping)果」里 `(ping)` 段角色 `pinyin_hint`，按提示色画。
// 首录于提示落地时（此前无此功能，参照即新代码产物，守的是之后的回归）。

/// 协调器产出的提示正文：`pre` + `(hint)`（角色 `pinyin_hint`）+ `post`。
fn hint_styled(pre: &str, hint: &str, post: &str) -> StyledText {
    let mut t = StyledText::from(pre);
    piece(&mut t, &format!("({hint})"), "pinyin_hint", false);
    t.push(post, &SpanStyle::default());
    t
}

fn hint_cand(text: &str, styled: StyledText) -> CandidateItem {
    CandidateItem {
        text_styled: styled,
        text: text.to_string(),
        code: String::new(),
        label: String::new(),
        tooltip: Default::default(),
        comment: Default::default(),
        comment_above: Default::default(),
        no_index: false,
    }
}

/// 下标 0 选中、1 悬停、2 无提示、3 常态带提示（带注释，提示色与注释色同框可比）。
fn fuzzy_candidates() -> Vec<CandidateItem> {
    vec![
        hint_cand("苹果", hint_styled("苹", "ping", "果")),
        hint_cand("瓶子", hint_styled("瓶", "ping", "子")),
        hint_cand("平安", StyledText::new()),
        CandidateItem {
            comment: role_text("pd", "code_hint"),
            ..hint_cand("频道", hint_styled("频", "pin", "道"))
        },
    ]
}

/// 横排 / 竖排 / 旋转 / 直立四种排布 + 展开网格（2 列 × 2 行，选中 0、悬停 1）。
fn render_fuzzy() -> String {
    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let mut out = render_windows(&theme, &LAYOUTS[..4], fuzzy_candidates);
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut w = CandidateWindow::new(CandidateWindowConfig::default(), tx).unwrap();
    w.scale = 1.0;
    w.set_theme(theme.clone());
    w.set_orientation(false, false, false);
    w.set_grid(Some(wind_ui_types::CandidateGrid {
        cols: 2,
        rows: 2,
        highlight_row: 0,
        index_start: 0,
        index_count: 2,
        tabs: 0,
        active_tab: 0,
    }));
    w.update("ping", 4, "", fuzzy_candidates(), 0, 1, 1, 1);
    let mut root = w.build_tree(false);
    root.layout(0.0, 0.0, &w.text_renderer);
    out.push_str("== 候选窗 网格 ==\n");
    out.push_str(&root.debug_dump());
    let dir = themes_dir().to_string_lossy().into_owned();
    out.replace(&dir, "<themes>")
}

/// 渲染段：`== 候选窗 {label} ==` 到下一段之间。
fn section<'a>(dump: &'a str, label: &str) -> &'a str {
    dump.split(&format!("== 候选窗 {label} =="))
        .nth(1)
        .unwrap_or_else(|| panic!("缺 {label} 段"))
        .split("== 候选窗")
        .next()
        .unwrap()
}

/// `_base` 给 `pinyin_hint` 的亮色档 `#C2410C`。
const HINT_LIGHT: [u8; 4] = [0xC2, 0x41, 0x0C, 255];

#[test]
fn golden_pinyin_hint_matches_golden() {
    let got = render_fuzzy();
    let file = golden_dir().join("pinyin_hint-default-light.txt");
    if std::env::var_os("WIND_UI_BLESS_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&file, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("读 golden {} 失败：{e}", file.display()));
    if got != want {
        let line = got
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(got.lines().count().min(want.lines().count()));
        panic!(
            "模糊音提示渲染与 golden 不一致（第 {} 行起）\n  实得 {:?}\n  参照 {:?}",
            line + 1,
            got.lines().nth(line),
            want.lines().nth(line)
        );
    }
}

/// 常态带提示的那条（下标 3「频(pin)道」）：整段文字一个叶子，`(pin)` 字节区间 [3, 8) 着提示色。
fn assert_plain_hint_run(sec: &str, label: &str) {
    let line = sec
        .lines()
        .find(|l| l.contains("text=Some(\"频(pin)道\")"))
        .unwrap_or_else(|| panic!("{label}：正文应完整显示「频(pin)道」"));
    assert!(
        line.contains(&format!(
            "runs=[ColorRun {{ start: 3, end: 8, rgba: {HINT_LIGHT:?} }}]"
        )),
        "{label}：(pin) 段应着提示色：{line}"
    );
}

#[test]
fn golden_pinyin_hint_horizontal() {
    let got = render_fuzzy();
    let sec = section(&got, "横排");
    assert_plain_hint_run(sec, "横排");
    // 无提示的候选不带颜色区间。
    let plain = sec
        .lines()
        .find(|l| l.contains("text=Some(\"平安\")"))
        .expect("平安");
    assert!(
        !plain.contains("runs="),
        "无提示候选不应带颜色区间：{plain}"
    );
}

#[test]
fn golden_pinyin_hint_vertical() {
    let got = render_fuzzy();
    assert_plain_hint_run(section(&got, "竖排"), "竖排");
    assert_plain_hint_run(section(&got, "旋转"), "旋转");
    // 直立态逐格扶正：`(`、`p`、`i`、`n`、`)` 各一格，每格整格着提示色；`频` / `道` 不着色。
    let up = section(&got, "直立");
    for cell in ["(", "p", "n", ")"] {
        let line = up
            .lines()
            .find(|l| l.contains(&format!("text=Some(\"{cell}\")")))
            .unwrap_or_else(|| panic!("直立：缺格 {cell}"));
        assert!(
            line.contains(&format!("rgba: {HINT_LIGHT:?}")),
            "直立：格 {cell} 应着提示色：{line}"
        );
    }
    for cell in ["频", "道"] {
        let line = up
            .lines()
            .find(|l| l.contains(&format!("text=Some(\"{cell}\")")))
            .unwrap_or_else(|| panic!("直立：缺格 {cell}"));
        assert!(!line.contains("runs="), "直立：格 {cell} 不应着色：{line}");
    }
}

#[test]
fn golden_pinyin_hint_highlighted() {
    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let v = &theme.views;
    let got = render_fuzzy();
    let sec = section(&got, "横排");
    for (text, range, state) in [
        ("苹(ping)果", (3, 9), wind_theme::TextState::Selected),
        ("瓶(ping)子", (3, 9), wind_theme::TextState::Hover),
    ] {
        let want = wind_theme::pinyin_hint_color(
            &v.text,
            &v.comment,
            state,
            [30, 30, 30, 255],
            wind_theme::fallback::COMMENT_TEXT,
            crate::span_runs::candidate_backdrop(v, state),
        );
        let body = wind_theme::body_color(&v.text, state, [30, 30, 30, 255]);
        let line = sec
            .lines()
            .find(|l| l.contains(&format!("text=Some(\"{text}\")")))
            .unwrap_or_else(|| panic!("缺 {text}"));
        if want == body {
            assert!(
                !line.contains("runs="),
                "{state:?}：提示色等于正文色时不建区间：{line}"
            );
        } else {
            assert!(
                line.contains(&format!(
                    "runs=[ColorRun {{ start: {}, end: {}, rgba: {want:?} }}]",
                    range.0, range.1
                )),
                "{state:?}：提示段应按该状态取色 {want:?}：{line}"
            );
        }
    }
    // 防空转：出厂 default 的选中态改了正文色，但选中底是浅色、常态橙对它够醒目
    // ⇒ 沿用常态橙（不回落注释色）。
    let sel = wind_theme::TextState::Selected;
    let sel_bg = crate::span_runs::candidate_backdrop(v, sel).expect("default 选中底可取色");
    assert_ne!(
        wind_theme::body_color(&v.text, sel, [30, 30, 30, 255]),
        wind_theme::body_color(&v.text, wind_theme::TextState::Normal, [30, 30, 30, 255]),
        "前提：default 选中态改了正文色"
    );
    let hint = |v: &wind_theme::RvViews| {
        wind_theme::pinyin_hint_color(
            &v.text,
            &v.comment,
            sel,
            [30, 30, 30, 255],
            wind_theme::fallback::COMMENT_TEXT,
            crate::span_runs::candidate_backdrop(v, sel),
        )
    };
    assert_eq!(hint(v), HINT_LIGHT, "浅色选中底 {sel_bg:?} 上应沿用常态橙");
    // 反向：把选中底换成深色实心 accent ⇒ 对比度不足，回落选中态注释色。
    let mut dark = v.clone();
    dark.item
        .selected
        .get_or_insert_with(Default::default)
        .bg_color = Some([37, 99, 235, 255]);
    let cmt_sel = wind_theme::body_color(&dark.comment, sel, wind_theme::fallback::COMMENT_TEXT);
    assert_eq!(hint(&dark), cmt_sel, "深色选中底上应回落选中态注释色");
}

/// 展开网格：格宽按含提示的实宽算，同一行的格子互不重叠、各行同列对齐。
#[test]
fn golden_pinyin_hint_grid() {
    let got = render_fuzzy();
    let sec = section(&got, "网格");
    assert!(
        sec.contains("text=Some(\"苹(ping)果\")"),
        "网格：提示应完整显示"
    );
    assert_plain_hint_run(sec, "网格");

    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut w = CandidateWindow::new(CandidateWindowConfig::default(), tx).unwrap();
    w.scale = 1.0;
    w.set_theme(theme);
    w.set_orientation(false, false, false);
    w.set_grid(Some(wind_ui_types::CandidateGrid {
        cols: 2,
        rows: 2,
        highlight_row: 0,
        index_start: 0,
        index_count: 2,
        tabs: 0,
        active_tab: 0,
    }));
    // 第 1 列上下两格：一格带长提示、一格不带 ⇒ 列宽按带提示的那格撑开。
    let cands = vec![
        hint_cand("平安", StyledText::new()),
        hint_cand("苹果", hint_styled("苹", "ping", "果")),
        hint_cand("拼音", StyledText::new()),
        hint_cand("品", StyledText::new()),
    ];
    w.update("ping", 4, "", cands, 0, -1, 1, 1);
    let mut root = w.build_tree(false);
    root.layout(0.0, 0.0, &w.text_renderer);
    let mut hits = Vec::new();
    root.collect_hits(&mut hits);
    let rect = |t: i32| hits.iter().find(|(g, _)| *g == t).unwrap().1;
    for row in [0, 2] {
        let (a, b) = (rect(row), rect(row + 1));
        assert!(
            a.x + a.w <= b.x + 0.01,
            "第 {row} 行两格重叠：{a:?} / {b:?}"
        );
    }
    assert!(
        (rect(1).x - rect(3).x).abs() < 0.01 && (rect(1).w - rect(3).w).abs() < 0.01,
        "第 2 列上下格应对齐同宽：{:?} / {:?}",
        rect(1),
        rect(3)
    );
    // 带提示那格里正文叶子完整显示且落在格内。
    let dump = root.debug_dump();
    assert!(dump.contains("text=Some(\"苹(ping)果\")"), "网格：提示被截");
}

/// 窄窗：结果要么完整保留提示、要么整段去掉提示，绝不出现半截 `(pi…`；且存在一档宽度
/// 恰好让「去掉提示后正文完整放下」。
#[test]
fn golden_pinyin_hint_truncated() {
    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let shown = |vmax: i32, cand: CandidateItem| -> String {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut w = CandidateWindow::new(CandidateWindowConfig::default(), tx).unwrap();
        w.scale = 1.0;
        w.set_theme(theme.clone());
        w.set_orientation(true, false, false);
        w.theme.behavior.vertical_max_width = vmax;
        w.update("", 0, "", vec![cand], 0, -1, 1, 1);
        let mut root = w.build_tree(false);
        root.layout(0.0, 0.0, &w.text_renderer);
        let dump = root.debug_dump();
        // 候选正文叶子：序号之后第一个带 text 的叶子。
        let mut texts = dump
            .lines()
            .filter_map(|l| l.split("text=Some(\"").nth(1))
            .map(|r| r.split('"').next().unwrap().to_string());
        let _index = texts.next();
        texts.next().expect("缺正文叶子")
    };
    let mut saw_stripped = false;
    let mut saw_full = false;
    for vmax in (40..=400).step_by(4) {
        let s = shown(vmax, hint_cand("苹果", hint_styled("苹", "ping", "果")));
        match s.as_str() {
            "苹(ping)果" => saw_full = true,
            "苹果" => saw_stripped = true,
            other => assert!(
                !other.contains('(') && !other.contains(')'),
                "vmax={vmax}：截出了半截提示 {other:?}"
            ),
        }
        let long = shown(
            vmax,
            hint_cand(
                "苹果果果果果果果果果",
                hint_styled("苹", "ping", "果果果果果果果果果"),
            ),
        );
        assert!(
            long == "苹(ping)果果果果果果果果果" || (!long.contains('(') && !long.contains(')')),
            "vmax={vmax}：长候选截出了半截提示 {long:?}"
        );
    }
    assert!(saw_full, "应有一档宽度完整显示提示");
    assert!(saw_stripped, "应有一档宽度去掉提示后正文完整");
}

/// 无提示（`text_styled` 空、或不带任何区间）时与改动前逐字节一致：出厂 golden 由
/// `factory_render_matches_golden` 守；这里再钉「无区间的 text_styled 与空等价」。
#[test]
fn golden_no_hint_unchanged() {
    let theme = wind_theme::load_resolved(&themes_dir(), "default", false).unwrap();
    let plain = render_windows(&theme, LAYOUTS, candidates);
    let with_plain_styled = render_windows(&theme, LAYOUTS, || {
        candidates()
            .into_iter()
            .map(|c| CandidateItem {
                text_styled: StyledText::from(c.text.as_str()),
                ..c
            })
            .collect()
    });
    assert_eq!(plain, with_plain_styled);
}
