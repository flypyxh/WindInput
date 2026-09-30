//! Linux 文本后端的单测。依赖本机字体：至少一款 CJK 字体（`fc-list :lang=zh`）与一款
//! 纯拉丁字体（DejaVu Sans，几乎所有发行版默认装）——缺哪款，对应用例会带着说明失败，
//! 不静默跳过。

use super::*;

/// 本机的简中字体（Noto Sans CJK SC / 思源黑体）：缺字回退、渲染用例的前提。
const CJK: &str = "Noto Sans CJK SC";
/// 只有拉丁字形的字体：缺字回退用例用它当 base。
const LATIN: &str = "DejaVu Sans";

fn require(r: &TextRenderer, family: &str) {
    assert_eq!(
        r.family_exists(family),
        Some(true),
        "本用例需要系统装有字体「{family}」（fontconfig 查不到）"
    );
}

/// 缓冲区里「不是背景」的像素数。
fn inked(buf: &[u8], bg: &[u8]) -> usize {
    buf.chunks(4)
        .zip(bg.chunks(4))
        .filter(|(a, b)| a != b)
        .count()
}

fn white(w: u32, h: u32) -> Vec<u8> {
    [255u8, 255, 255, 255].repeat((w * h) as usize)
}

#[test]
fn measure_cjk_is_about_one_em_per_char() {
    let r = TextRenderer::new(CJK, 20.0).unwrap();
    require(&r, CJK);
    let m = r.measure_text("你好世界");
    // CJK 表意文字是全角：每字约 1em。
    assert!((m.width - 80.0).abs() < 4.0, "{:?}", m.width);
    assert!(m.height >= 20.0 && m.height < 40.0, "{:?}", m.height);
}

#[test]
fn measure_latin_is_narrower_than_cjk_and_scales() {
    let r = TextRenderer::new(CJK, 20.0).unwrap();
    let latin = r.measure_text("Hello");
    let cjk = r.measure_text("你好世界吗");
    assert!(latin.width > 5.0 * 20.0 * 0.3, "{latin:?}");
    assert!(latin.width < cjk.width, "{latin:?} vs {cjk:?}");
    let big = r.measure_text_sized("Hello", 40.0);
    assert!(
        (big.width - 2.0 * latin.width).abs() < 1.0,
        "{big:?} vs {latin:?}"
    );
    // 宽度随内容单调：多一个字就宽一截。
    assert!(r.measure_text("Hello!").width > latin.width);
}

#[test]
fn measure_empty_is_zero_width() {
    let r = TextRenderer::new(CJK, 16.0).unwrap();
    let m = r.measure_text_sized("", 20.0);
    assert_eq!(m.width, 0.0);
    assert!((m.height - 24.0).abs() < 0.01); // 20*1.2，与另两个后端同口径
}

/// 尾随空白计入宽度（DirectWrite `widthIncludingTrailingWhitespace` 同口径）：
/// 预编辑区按前缀测量定光标，空格不计宽的话敲空格光标不动。
#[test]
fn trailing_whitespace_counts() {
    let r = TextRenderer::new(CJK, 16.0).unwrap();
    assert!(r.measure_text("a ").width > r.measure_text("a").width);
}

/// `\n` 硬换行：高度翻倍、宽取最宽一行（candidate_window 的多行候选依赖）。
#[test]
fn hard_newline_doubles_height() {
    let r = TextRenderer::new(CJK, 16.0).unwrap();
    let one = r.measure_text("你好");
    let two = r.measure_text("你好\n世界啊");
    assert!(
        (two.height - 2.0 * one.height).abs() < 0.01,
        "{one:?} {two:?}"
    );
    assert!((two.width - r.measure_text("世界啊").width).abs() < 0.01);
}

/// 行高只看基准字体，不随行内回退字体起伏（同 dwrite 的 UNIFORM 行距约定）。
#[test]
fn line_height_independent_of_fallback_glyphs() {
    let r = TextRenderer::new(LATIN, 16.0).unwrap();
    require(&r, LATIN);
    assert_eq!(
        r.measure_text("abc").height,
        r.measure_text("abc中文").height
    );
}

#[test]
fn draw_writes_glyph_pixels_near_origin() {
    let r = TextRenderer::new(CJK, 20.0).unwrap();
    let (w, h) = (200u32, 40u32);
    let bg = white(w, h);
    let mut buf = bg.clone();
    r.draw_text(&mut buf, w, h, 10.0, 5.0, "你好 Hi", [0, 0, 0, 255])
        .unwrap();
    let m = r.measure_text("你好 Hi");
    let dark = inked(&buf, &bg);
    assert!(dark > 100, "应画出可见的字：{dark}");
    // 墨迹全部落在 (x, y) 起、测量宽高的框内（留 2px 抗锯齿余量）。
    for yy in 0..h as usize {
        for xx in 0..w as usize {
            let i = (yy * w as usize + xx) * 4;
            if buf[i..i + 4] != bg[i..i + 4] {
                assert!(
                    xx as f32 >= 8.0 && xx as f32 <= 10.0 + m.width + 2.0,
                    "({xx},{yy}) 超出测量宽度 {m:?}"
                );
                assert!(
                    yy as f32 >= 3.0 && yy as f32 <= 5.0 + m.height + 2.0,
                    "({xx},{yy})"
                );
            }
        }
    }
}

/// 透明底上画字：结果必须是合法预乘像素（每个色通道 ≤ alpha），且 alpha 真的升起来了。
#[test]
fn draw_on_transparent_yields_valid_premultiplied_pixels() {
    let r = TextRenderer::new(CJK, 24.0).unwrap();
    let (w, h) = (80u32, 40u32);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    r.draw_text(&mut buf, w, h, 2.0, 2.0, "中", [200, 100, 50, 255])
        .unwrap();
    let mut opaque = 0;
    for p in buf.chunks(4) {
        assert!(
            p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3],
            "非法预乘像素 {p:?}"
        );
        if p[3] == 255 {
            opaque += 1;
            // BGRA：R 在 [2]。
            assert_eq!((p[2], p[1], p[0]), (200, 100, 50));
        }
    }
    assert!(opaque > 20, "笔画内部应有完全覆盖的像素：{opaque}");
}

/// 半透明文字色：墨迹 alpha 不超过文字色自身的 alpha。
#[test]
fn text_alpha_is_respected() {
    let r = TextRenderer::new(CJK, 24.0).unwrap();
    let (w, h) = (80u32, 40u32);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    r.draw_text(&mut buf, w, h, 2.0, 2.0, "中", [0, 0, 0, 128])
        .unwrap();
    let max_a = buf.chunks(4).map(|p| p[3]).max().unwrap();
    assert!((120..=129).contains(&max_a), "{max_a}");
}

/// 缺字回退：base 是纯拉丁字体，汉字必须从链上别的字体取到**真字形**（gid≠0，不是方框）。
#[test]
fn missing_glyph_falls_back_to_cjk_font() {
    let r = TextRenderer::new(LATIN, 20.0).unwrap();
    require(&r, LATIN);
    let lay = r.layout("A中", &TextStyle::new(20.0));
    let (a, zh) = (
        lay.lines[0][0].glyph.unwrap(),
        lay.lines[0][1].glyph.unwrap(),
    );
    assert_ne!(zh.1, 0, "汉字落到了 .notdef（方框）");
    assert_ne!(a.0, zh.0, "汉字应来自回退字体，而非纯拉丁的主字体");
    // 回退字体的字形也要真的画得出来。
    let (w, h) = (60u32, 40u32);
    let bg = white(w, h);
    let mut buf = bg.clone();
    r.draw_text(&mut buf, w, h, 2.0, 2.0, "中", [0, 0, 0, 255])
        .unwrap();
    assert!(inked(&buf, &bg) > 50);
}

/// 方案回退链：用户声明的回退字体排在系统回退之前。
#[test]
fn plan_fallback_chain_is_honoured() {
    let mut r = TextRenderer::new(LATIN, 20.0).unwrap();
    require(&r, CJK);
    r.set_font_plan(FontPlan::new(
        vec![LATIN.to_string(), CJK.to_string()],
        vec![],
    ));
    let lay = r.layout("中", &TextStyle::new(20.0));
    let (face, gid) = lay.lines[0][0].glyph.unwrap();
    assert_ne!(gid, 0);
    let name = store::with(|st| {
        st.face(face)
            .face
            .names()
            .into_iter()
            .filter(|n| n.name_id == ttf_parser::name_id::FAMILY)
            .find_map(|n| n.to_string())
    });
    assert_eq!(name.as_deref(), Some(CJK));
}

/// 粗体：字重 700 的墨迹明显多于 400（真粗体字面或合成加粗都算）。
#[test]
fn bold_draws_more_ink() {
    let r = TextRenderer::new(CJK, 24.0).unwrap();
    let (w, h) = (160u32, 40u32);
    let bg = white(w, h);
    let ink = |weight| {
        let mut buf = bg.clone();
        let ts = TextStyle::new(24.0).with_weight(weight);
        r.draw(&mut buf, w, h, 2.0, 2.0, "你好ab", &ts, [0, 0, 0, 255])
            .unwrap();
        // 按覆盖度加权，避免只数「非背景像素」时粗体把抗锯齿边缘并掉。
        buf.chunks(4).map(|p| 255 - p[0] as u32).sum::<u32>()
    };
    let (regular, bold) = (ink(400), ink(700));
    assert!(
        bold * 10 > regular * 11,
        "粗体应多出 ≥10% 墨迹：{regular} → {bold}"
    );
}

/// 默认字重作用于未显式指定字重的叶子。
#[test]
fn default_weight_applies_when_leaf_weight_is_zero() {
    let mut r = TextRenderer::new(CJK, 24.0).unwrap();
    let (w, h) = (100u32, 40u32);
    let bg = white(w, h);
    let draw = |r: &TextRenderer| {
        let mut buf = bg.clone();
        r.draw_text(&mut buf, w, h, 2.0, 2.0, "你好", [0, 0, 0, 255])
            .unwrap();
        buf
    };
    let before = draw(&r);
    r.set_default_weight(700);
    assert_ne!(draw(&r), before);
}

/// 全取基色的 `draw_runs` 与 `draw` 逐字节相同：两条路径共用同一次排版与合成。
#[test]
fn base_color_runs_equal_plain_draw() {
    let r = TextRenderer::new(CJK, 24.0).unwrap();
    let (w, h) = (240u32, 48u32);
    let ts = TextStyle::new(24.0);
    let text = "ab 你好 cd";
    let black = [0, 0, 0, 255];
    let bg = white(w, h);
    let mut plain = bg.clone();
    r.draw(&mut plain, w, h, 4.5, 6.0, text, &ts, black)
        .unwrap();
    assert_ne!(plain, bg);
    let runs = [
        ColorRun {
            start: 0,
            end: 2,
            rgba: black,
        },
        ColorRun {
            start: 3,
            end: 9,
            rgba: black,
        },
    ];
    let mut colored = bg.clone();
    r.draw_runs(&mut colored, w, h, 4.5, 6.0, text, &ts, black, &runs)
        .unwrap();
    assert_eq!(plain, colored);
}

/// 真按区间上了色：红 `你好` + 蓝 `ab`，各自 x 区间内改动像素以本段色为主。
#[test]
fn draw_runs_colors_each_range() {
    let r = TextRenderer::new(CJK, 32.0).unwrap();
    let (w, h) = (200u32, 60u32);
    let bg = white(w, h);
    let mut buf = bg.clone();
    let ts = TextStyle::new(32.0);
    let x0 = 4.0;
    let split = x0 + r.measure("你好", &ts).width;
    let runs = [
        ColorRun {
            start: 0,
            end: 6,
            rgba: [220, 0, 0, 255],
        },
        ColorRun {
            start: 6,
            end: 8,
            rgba: [0, 0, 220, 255],
        },
    ];
    r.draw_runs(
        &mut buf,
        w,
        h,
        x0,
        4.0,
        "你好ab",
        &ts,
        [0, 0, 0, 255],
        &runs,
    )
    .unwrap();
    let (mut left, mut right) = ((0, 0), (0, 0));
    for i in 0..(w * h) as usize {
        if buf[i * 4..i * 4 + 4] == bg[i * 4..i * 4 + 4] {
            continue;
        }
        let (b, red) = (buf[i * 4] as i32, buf[i * 4 + 2] as i32);
        let side = if ((i as u32 % w) as f32) < split {
            &mut left
        } else {
            &mut right
        };
        if red > b + 30 {
            side.0 += 1;
        } else if b > red + 30 {
            side.1 += 1;
        }
    }
    assert!(
        left.0 > 20 && left.0 > left.1 * 4,
        "左半应以红为主：{left:?}"
    );
    assert!(
        right.1 > 20 && right.1 > right.0 * 4,
        "右半应以蓝为主：{right:?}"
    );
}

/// [`TextRenderer::family_exists`] 的三态各自落对，且大小写不敏感。
#[test]
fn family_exists_tells_present_from_absent() {
    let r = TextRenderer::new(CJK, 16.0).unwrap();
    assert_eq!(r.family_exists(CJK), Some(true));
    assert_eq!(r.family_exists(&CJK.to_lowercase()), Some(true));
    assert_eq!(r.family_exists("这个字族并不存在 ZZZ"), Some(false));
    assert_eq!(r.family_exists("   "), None);
    assert_eq!(
        r.family_exists("sans-serif"),
        Some(true),
        "通用族名是别名，恒可匹配"
    );
}

/// 按 face 全名找回家族名与字重（旧 GDI face name 兜底）。
#[test]
fn find_face_by_full_name() {
    let r = TextRenderer::new(CJK, 16.0).unwrap();
    let (family, weight) = r
        .find_face("DejaVu Sans Bold")
        .expect("DejaVu Sans Bold 应可查到");
    assert_eq!(family, LATIN);
    assert_eq!(weight, 700);
    assert_eq!(r.find_face("并不存在的字体 Bold"), None);
}

/// 零宽格式字符（变体选择符、ZWJ）不占宽、不画方框。
#[test]
fn invisible_format_chars_take_no_space() {
    let r = TextRenderer::new(CJK, 20.0).unwrap();
    assert_eq!(
        r.measure_text("a\u{FE0F}\u{200D}b").width,
        r.measure_text("ab").width
    );
}

/// 测量缓存在换字体后失效：否则新字体下仍拿旧宽度布局。
#[test]
fn set_font_family_invalidates_measure_cache() {
    let mut r = TextRenderer::new(CJK, 20.0).unwrap();
    require(&r, LATIN);
    let cjk = r.measure_text("Hello").width;
    r.set_font_family(LATIN);
    let latin = r.measure_text("Hello").width;
    assert_ne!(cjk, latin, "Noto Sans CJK 与 DejaVu Sans 的拉丁字宽不同");
}

/// 拆字字根：私用区码位走字根字体；路径为空撤掉、坏路径报错且不残留旧字体。
#[test]
fn chaizi_font_set_and_clear() {
    let mut r = TextRenderer::new(CJK, 20.0).unwrap();
    assert!(r.set_chaizi_font("/nonexistent/chaizi.ttf", "").is_err());
    assert!(r.chaizi.is_none());
    assert!(r.set_chaizi_font("", "").is_ok());
    assert!(r.chaizi.is_none());
}

/// 缓冲区过小报错，零尺寸空操作——与另两个后端同契约。
#[test]
fn draw_guards() {
    let r = TextRenderer::new(CJK, 20.0).unwrap();
    let mut small = vec![0u8; 10];
    assert!(
        r.draw_text(&mut small, 10, 10, 0.0, 0.0, "中", [0, 0, 0, 255])
            .is_err()
    );
    assert!(
        r.draw_text(&mut small, 0, 0, 0.0, 0.0, "中", [0, 0, 0, 255])
            .is_ok()
    );
}

#[test]
fn embolden_spreads_coverage_rightwards() {
    let mut d = vec![0, 255, 0, 0];
    embolden_rows(&mut d, 4, 1.0);
    assert_eq!(d, vec![0, 255, 255, 0]);
}

// ── 肉眼检查用的 PNG 产物 ──────────────────────────────────────────────────────────
//
// `WIND_UI_PNG_DIR=<目录> cargo test -p wind-ui --features linux-host --lib png_ -- --ignored`

/// 预乘 BGRA → 非预乘 RGBA，存 PNG。
fn save_png(buf: &[u8], w: u32, h: u32, name: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("WIND_UI_PNG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let rgba: Vec<u8> = buf
        .chunks(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let un = |c: u8| {
                (c as u32 * 255)
                    .checked_div(a)
                    .map_or(0, |v| v.min(255) as u8)
            };
            [un(p[2]), un(p[1]), un(p[0]), p[3]]
        })
        .collect();
    let path = dir.join(name);
    image::save_buffer(&path, &rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
    eprintln!("PNG → {}", path.display());
    path
}

#[test]
#[ignore = "产出 PNG 供肉眼检查"]
fn png_text_sample() {
    let (w, h) = (520u32, 150u32);
    let mut buf = white(w, h);
    let r = TextRenderer::new(CJK, 32.0).unwrap();
    r.draw_text(
        &mut buf,
        w,
        h,
        10.0,
        8.0,
        "你好世界 Hello 123",
        [0, 0, 0, 255],
    )
    .unwrap();
    let ts = TextStyle::new(32.0).with_weight(700);
    r.draw(
        &mut buf,
        w,
        h,
        10.0,
        56.0,
        "粗体 Bold 你好",
        &ts,
        [30, 30, 30, 255],
    )
    .unwrap();
    let text = "红色蓝色 abc";
    let runs = [
        ColorRun {
            start: 0,
            end: 6,
            rgba: [220, 0, 0, 255],
        },
        ColorRun {
            start: 6,
            end: 12,
            rgba: [0, 60, 220, 255],
        },
    ];
    r.draw_runs(
        &mut buf,
        w,
        h,
        10.0,
        104.0,
        text,
        &TextStyle::new(28.0),
        [0, 128, 0, 255],
        &runs,
    )
    .unwrap();
    save_png(&buf, w, h, "linux_text_sample.png");
}

/// 整条候选窗路径：出厂主题 + 5 个中文候选，经 `render_frame()`（外部宿主形态的出帧口）。
#[test]
#[ignore = "产出 PNG 供肉眼检查"]
fn png_candidate_window() {
    use crate::candidate_window::{CandidateItem, CandidateWindow, CandidateWindowConfig};
    let themes = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/themes");
    for (name, vertical) in [("default", false), ("default", true)] {
        let theme = wind_theme::load_resolved(&themes, name, false).expect("加载出厂主题");
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut w = CandidateWindow::new(CandidateWindowConfig::default(), tx).unwrap();
        w.set_theme(theme);
        w.set_orientation(vertical, false, false);
        let item = |text: &str| CandidateItem {
            text: text.to_string(),
            code: String::new(),
            label: String::new(),
            tooltip: Default::default(),
            comment: Default::default(),
            comment_above: Default::default(),
            no_index: false,
        };
        let cands = ["你好", "拟好", "你号", "泥浩", "妮"].map(item).to_vec();
        w.set_position(200, 200, 20, true);
        w.update("nihao", 5, "拼", cands, 0, -1, 1, 1);
        let f = w.render_frame().expect("有候选时应出帧");
        // 热路径粗测：首帧付字体加载与字形光栅，之后应全走缓存。
        let t0 = std::time::Instant::now();
        for _ in 0..100 {
            w.render_frame().unwrap();
        }
        eprintln!("render_frame 稳态均值 {:?}", t0.elapsed() / 100);
        assert!(f.buf.chunks(4).any(|p| p[3] != 0));
        save_png(
            &f.buf,
            f.width,
            f.height,
            &format!(
                "linux_candidate_{}.png",
                if vertical { "vertical" } else { "horizontal" }
            ),
        );
    }
}
