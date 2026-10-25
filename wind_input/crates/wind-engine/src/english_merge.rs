//! 英文候选混入（供**单一来源引擎**复用的查询与合并）
//!
//! 把「查一次英文词库，把结果混进另一个引擎的候选列表」独立出来，供纯拼音方案与纯码表
//! 方案（五笔等）复用。接线点在 [`crate::manager::EngineManager`]——那里是
//! `convert` / `recheck_auto_commit` / `handle_top_code` **三条通路的共同收口**，
//! 无需逐引擎改造，也无需包一层转发全部 trait 方法的装饰器（`Engine` 有二十来个带默认
//! 实现的方法，漏转发一个就是静默改行为）。
//!
//! ⚠️ 本模块**不是引擎无关的**（曾经是）：配置已按引擎拆成两份，见下面「配置按引擎分
//! 两份」。收口的是三条通路，不是引擎类型——新增引擎类型时
//! `manager::english_merge_cfg` 的穷尽匹配会强制在那里做决定。
//!
//! ## 与 `MixedEngine` 那套的分工——不是重复实现
//!
//! 混输的英文**取哪些**与本模块同一个入口（[`select`]、否决判据 [`has_any`]），但合并走
//! `MixedEngine::merge_sort_dedup`：那里与 `MixedEngine::truncation_tier` 的**三方档位仲裁**
//! （码表精确 / 短语 / 英文精确+码表前缀 / 其余）耦合，而那套档位的语义又与
//! `convert_overflow` 的截断归属绑在一起。本模块的 [`merge`] 服务的场景只有**一路**基础候选，
//! 三方仲裁无从谈起——故合并另写一份简单的，而不是把混输那套拆通用。
//!
//! ⇒ 混输方案**两份配置都不读**（见 `manager::EngineManager::english_merge_cfg` 的分流），
//! 读自己的 `schema.mix.*`，否则两套英文各混一遍，档位与配额双重失真。
//!
//! ## 配置按引擎分两份，本模块只认折叠后的形态
//!
//! `[schema.codetable.english_merge]`（六项，且经方案级 `[engine.codetable.english_merge]`
//! 折叠）与 `[schema.pinyin.english_merge]`（无 `block_commit`）是**两件独立的事**
//! ——理由见 `wind_config::CodetableEnglishMerge` 的文档。管理器把它们折叠成
//! [`Effective`] 再交给本模块，故这里的函数不关心取值来自哪一份。
//!
//! ## ★ 英文必须有保底席位，否则开关等于没做
//!
//! 混输里英文精确与码表前缀补全同在档 2，档内按合并顺序、码表先入，码表洪水下英文曾被整片
//! 挤掉（现由 `MixedEngine::truncate_with_english_seats` 保底，见
//! `english_survives_codetable_flood`）。
//!
//! 那在混输里影响尚且有限（码表候选数有限），在**纯拼音下却是致命的**：拼音一次就能吐满
//! `max_candidates`（`ConvertOptions::admit` 的实测表里 `ni` 装满 100 条），英文若追加在
//! 尾部再截断，会被**全部**丢掉——用户打开开关后什么也看不到，且日志、设置页均无痕迹。
//!
//! 故本模块反过来做：**取回来的每一条都保证活到最后**（[`merge`] 先腾座后追加）。
//!
//! 「英文条数」本身由 [`select`] 限住（精确命中至多 [`EXACT_MAX`] 条，码表 / 混输再受
//! `max_count` 封顶），不再需要按 `max_candidates` 比例分席——用户要的是「打 hello 能看见 hello」，
//! 不是「hello 的二十个前缀词把候选页占满」。
//!
//! ## 位次不在本模块决定
//!
//! 本模块只管「查哪些、谁活下来」。英文候选**排第几**由协调器
//! `place_english_after_common_exact` 在所有排序（含自动调频）跑完之后统一定位：
//! 排在「中文常用精确解」之后、其余候选之前；码表 / 混输 / 拼音各有两项位次下限（完整单词 / 前缀补全）。放在这里做不到——引擎看不见协调器随后并入的
//! 短语候选，也看不见调频的位置提升。

use crate::engine::Engine;
use wind_candidate::Candidate;

/// 扫描窗口：向英文引擎取多少条原始命中，用来从中挑出精确匹配。
///
/// 英文引擎是前缀匹配（`hen` → hen / hence / Henderson…），且**精确整串恒居首**
/// （`CodeTableEngine` 的 `整串精确匹配应居首` 守门断言）。取 8 条是给「同码多形态」
/// （`she` / `She`）留余量，不是给前缀留的——前缀在下面被整片丢弃。
const EXACT_SCAN: usize = 8;

/// 无精确命中、且基础候选为空时，前缀回退至多给几条。见 [`lookup_prefix_fallback`]。
const PREFIX_FALLBACK_MAX: usize = 5;

/// **英文输入态**下的最小长度下限：中文侧无候选时，`min_length` 最低压到这个值。
///
/// `min_length` 的存在理由与「丢弃前缀扩展」**是同一个**——打中文时别冒出一堆英文噪音。
/// 中文侧一条候选都没有时那个理由不成立，故两者该一起放宽；只放宽前缀而仍卡长度，
/// 会留下 `wi` / `vi` / `hi` 这类**两道闸门叠出来的空码**（拼音不成音节 + 2 码不查英文）。
///
/// ## 为什么下限是 2 而不是 1
///
/// 单字母的补全没有信息量（`w` 能补出几百个词），且 1 码时用户往往才刚开始打字、意图
/// 未定。另外 1 码在拼音下通常并不空——`w` 有几十条简拼候选，真正空的只有 `v` 这类
/// 非声母字母，为它放开一个字母不划算。
const EMPTY_BASE_MIN_LENGTH: usize = 2;

/// 精确命中至多保留几条。
///
/// 同一个码可能对应多条词条（大小写形态、`she` / `she'd` 那种撇号变体在**本表里不同码**
/// 故不在此列）。2 条足够，且候选面上英文再多也没有意义。
const EXACT_MAX: usize = 2;

/// 最小触发长度的回退值（配置为 0 时）。与 `manager::build_engine` 给
/// `schema.mix.min_english_length` 的回退**同值**，两处口径一致。
const DEFAULT_MIN_LENGTH: usize = 3;

/// 本次转换**生效**的英文混入参数。
///
/// 来源有三个且字段不同——码表侧是 `schema.codetable.english_merge`（经方案级
/// `[engine.codetable.english_merge]` 折叠），拼音侧是 `schema.pinyin.english_merge`
/// （无 `block_commit`），混输侧是 `schema.mix` 的 `english_*` 与
/// `auto_commit_block_on_english`（`MixedEngine` 构建时折叠）。本结构是它们折叠后的
/// **共同形态**，让下游三条通路只认一种。
///
/// ⚠️ 拼音侧构造时 `block_commit` 恒为 `false`：那一项否决的是满码自动上屏 / 顶码上屏，
/// 拼音方案两者都没有。写成 `false` 不是"默认关"，是"这条路上没有可否决的动作"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub enable: bool,
    pub min_length: usize,
    /// **前缀级**否决：有英文词以当前串开头即否决上屏（[`has_any`]）。
    pub block_commit: bool,
    /// **成词级**否决：整串恰是一个英文完整单词时否决上屏（[`has_word`]）。前缀级开着时被它涵盖。
    pub block_commit_on_word: bool,
    /// 中文侧有候选时，输入达到几码起按长度给前缀补全（`complete_min_length`）。
    /// 0 = 不按长度补。双拼方案由管理器折成 0（见 [`Self::complete_only_non_pinyin`]）。
    pub complete_min_length: usize,
    /// 补全再叠一条拼音判据：整串切不成完整全拼音节才补（[`prefix_complete_applies`]）。
    /// 只有拼音侧为 `true`；码表 / 混输够长就补。
    pub complete_only_non_pinyin: bool,
    /// 中文侧有候选时英文合计至多几条（完整单词优先）。已解析回退值（≥1）。
    pub max_count: usize,
}

impl Effective {
    /// 上屏否决（[`has_any`]）的长度门槛：英文从几码起**可能**出现在候选里就从几码起否决。
    ///
    /// 完整单词从 `min_length` 起出；开了按长度补全时补全从 `complete_min_length` 起出，
    /// 取两者较小者。门槛再低就是「英文还不会露面却先挡住上屏」。
    pub fn veto_min_length(&self) -> usize {
        let base = min_length_or_default(self.min_length);
        if self.complete_min_length > 0 {
            base.min(self.complete_min_length)
        } else {
            base
        }
    }
}

/// 中文侧有候选时英文条数上限的回退值（配置为 0 时）。码表 / 混输同口径。
const DEFAULT_MAX_COUNT: usize = 2;

/// 配置值 0 视作「用回退值」，口径同 [`min_length_or_default`]。
pub fn max_count_or_default(configured: usize) -> usize {
    if configured > 0 {
        configured
    } else {
        DEFAULT_MAX_COUNT
    }
}

/// 按中文侧有无候选，取这次要进候选列表的英文。码表、拼音（经 `manager::convert`）与混输
/// （`MixedEngine`）共用这一个入口，三处口径一致。
///
/// | 中文侧 | 给什么 |
/// |---|---|
/// | 空 | [`lookup_prefix_fallback`]：前缀补全一页（不受 `max_count` 约束） |
/// | 有 | [`lookup`] 的完整单词；补全开着且适用时再补前缀词；合计不超 `max_count` |
///
/// 补全按 `complete_min_length` 判（够长就补，补到 `max_count` 为止）；拼音另要求整串
/// 不是完整拼音（[`prefix_complete_applies`]）。
pub fn select(
    english: &dyn Engine,
    input: &str,
    base_empty: bool,
    cfg: &Effective,
) -> Vec<Candidate> {
    if base_empty {
        return lookup_prefix_fallback(english, input, cfg.min_length);
    }
    let by_length = cfg.complete_min_length > 0
        && input.chars().count() >= cfg.complete_min_length
        && (!cfg.complete_only_non_pinyin || prefix_complete_applies(input));
    // 按长度补全时，完整单词的门槛一并放到补全门槛：否则 `complete_min_length` 比 `min_length`
    // 小时，打 `go` 补出 good / google 却不出 go 本身。
    let exact_gate = if by_length {
        cfg.veto_min_length()
    } else {
        cfg.min_length
    };
    let mut v = lookup(english, input, exact_gate);
    let room = if by_length {
        cfg.max_count.saturating_sub(v.len())
    } else {
        0
    };
    if room > 0 {
        let extra = lookup_prefix_complete(english, input, &v, room);
        v.extend(extra);
    }
    v.truncate(cfg.max_count);
    v
}

/// 配置值 0 视作「用回退值」，口径同 `schema.mix.min_english_length`。
pub fn min_length_or_default(configured: usize) -> usize {
    if configured > 0 {
        configured
    } else {
        DEFAULT_MIN_LENGTH
    }
}

/// 查英文词库，**只取精确命中**（`code == 小写化输入`）。短输入不查。
///
/// ## ★ 为什么丢弃前缀扩展
///
/// 英文引擎是前缀匹配，`hen` 会带出 hence / henceforth / Henderson / Hendrix，
/// `github` 会带出 GitHub Pages / GitHub Copilot CLI 等 7 条长词组（实测）。这些条目在
/// 本功能里**没有任何使用场景**：用户在打中文时它们是纯噪音；用户真想打某个英文词时，
/// 正确操作是继续敲完（`githu` → `github`），而不是从一屏 GitHub 开头的词组里挑。
///
/// 丢掉前缀后英文候选天然只剩 1~2 条，**配额机制随之取消**（原先按
/// `max_candidates/10` 封顶 5 席的 `seats_for` 已删）——不是放宽，是那个问题不存在了：
/// 当初要配额正是因为前缀扩展会成片涌入。
///
/// ## ⚠️ 与 [`has_any`] 的判据**刻意不同**
///
/// 本函数管「显示哪些」，`has_any` 管「要不要否决上屏」。后者必须仍看**前缀**：
/// 用户打到 `gith` 时精确命中还不存在，若否决判据也收窄成精确，五笔满码就会在第 4 键
/// 把中文顶上屏，`github` 永远敲不完——那正是 `block_commit` 存在的理由。
/// 两者收窄一处、保留一处，是这次改动里最容易被后人「顺手统一」掉的地方。
///
/// ## 清 `is_exact_code`
///
/// 该标志是**码表域**的（语义为「码 == 输入的完全匹配」，服务于码表精确档），而拼音引擎
/// 从不设它。把它跨来源带进拼音候选列表，会让英文在协调器的 `cmp_exact_first`
/// （位置在 `by_weight` **之前**）无条件压过全部中文——真机现象即「打 hen，权重 500 的
/// 英文排在权重 86016 的『很』之前」。英文不是赢了权重，是赢在一个拼音压根没参赛的键上。
///
/// 输入小写化以匹配英文词库（`EnglishEngine` 的 code 列已小写化）。查询失败静默退化为空
/// ——英文只是捎带的增强，不该让它的故障影响主候选（同 `Engine::convert` 永不 panic 的约定）。
pub fn lookup(english: &dyn Engine, input: &str, min_length: usize) -> Vec<Candidate> {
    if input.chars().count() < min_length_or_default(min_length) {
        return Vec::new();
    }
    let lower = input.to_lowercase();
    let Ok(r) = english.convert(&lower, EXACT_SCAN) else {
        return Vec::new();
    };
    r.candidates
        .into_iter()
        .filter(|c| c.code == lower)
        .map(|mut c| {
            // 见函数文档「清 is_exact_code」。
            c.is_exact_code = false;
            c
        })
        .take(EXACT_MAX)
        .collect()
}

/// **英文输入态**的查询：中文侧一条候选都没有时用它，给前缀补全。
///
/// ## 与 [`lookup`] 的分工：判据只有一个
///
/// 调用方（`manager::convert`）按「中文侧有没有候选」二选一，**不再叠加「有没有精确
/// 命中」**：
///
/// | 中文侧 | 含义 | 用哪个 |
/// |---|---|---|
/// | 有候选 | 用户在打中文 | [`lookup`]，只收精确——前缀词是噪音 |
/// | 无候选 | 用户在打英文 | 本函数，给补全——此时补全正是他要的 |
///
/// ## ★ 为什么不能再叠「有没有精确命中」
///
/// 曾经的写法是「无精确命中且中文侧空 ⇒ 才放宽」，结果在「无精确 → 有精确」的那一键上
/// 切换了模式：真机打 `gith` / `githu` 各 5 条候选，打完 `github` 反而**塌缩成 1 条**。
/// 用户看到的是候选数在打完词的瞬间跳变——比原先的闪烁好不了多少。
///
/// 一个判据就够：「中文侧有没有候选」本身已经完整表达了用户在打什么。两个判据只会
/// 在它们不同步的那一步上制造突变。
///
/// ## 这条路解决的原始报障
///
/// 拼音下逐键打 `windows`：`win` / `wind` 是英文词有候选，`windo` 不是词、没候选，
/// `window` 又有——候选窗一闪一闪。拼音对这类串一条中文候选也给不出（`wi` 不成音节），
/// 于是英文一断档整个窗就空掉；五笔下码表仍在出候选，闪烁被掩盖，
/// 所以只有拼音方案报得出来。
///
/// ## 长度门槛一并放宽到 [`EMPTY_BASE_MIN_LENGTH`]
///
/// `min_length` 与「丢弃前缀」是同一个理由的两种表达，故在同一个前提下一起松开。
/// 否则 `wi` / `vi` / `hi` 仍是空码——拼音不成音节、2 码又不查英文，两道闸门叠出来的
/// 空窗，而用户此刻显然在打英文。
pub fn lookup_prefix_fallback(
    english: &dyn Engine,
    input: &str,
    min_length: usize,
) -> Vec<Candidate> {
    // 取「配置值」与「英文输入态下限」的较小者：用户把 min_length 调大是为了**打中文时**
    // 少受打扰，不该连带把「中文侧本来就没候选」时的补全也卡掉。
    let gate = min_length_or_default(min_length).min(EMPTY_BASE_MIN_LENGTH);
    if input.chars().count() < gate {
        return Vec::new();
    }
    let lower = input.to_lowercase();
    let Ok(r) = english.convert(&lower, PREFIX_FALLBACK_MAX) else {
        return Vec::new();
    };
    // 与输入相同（大小写不敏感）的词排最前，其余保持英文引擎的次序。英文引擎本身已把精确
    // 整串排在最前，这里只是不依赖它。
    let (mut out, rest): (Vec<Candidate>, Vec<Candidate>) = r
        .candidates
        .into_iter()
        .map(|mut c| {
            // 同 `lookup`：码表域标志不跨来源带，理由见该函数文档。
            c.is_exact_code = false;
            c
        })
        .partition(|c| c.text.to_lowercase() == lower);
    out.extend(rest);
    out.truncate(PREFIX_FALLBACK_MAX);
    // ★ 次序要落到 weight：上面清了 `is_exact_code`，协调器重排时精确词不再靠
    // `cmp_exact_first` 领先，而是按 weight 与前缀词比——真机 `hi` 曾落到 his / high / him /
    // history 之后。精确词抬到页内最高权重之上；没有精确词时不动。
    // ⚠️ 码表「基础排序」为自然顺序（`base_sort = "natural"`）时协调器不看 weight，本抬权不起
    // 作用，次序按英文引擎原序（它本身就把精确词排在最前）。
    let n_exact = out
        .iter()
        .take_while(|c| c.text.to_lowercase() == lower)
        .count();
    if let Some(top) = out[n_exact..].iter().map(|c| c.weight).max() {
        for c in &mut out[..n_exact] {
            c.weight = c.weight.max(top.saturating_add(1));
        }
    }
    out
}

/// 拼音方案中文侧**有**候选时，这串输入可不可以补英文前缀（长度门槛
/// `schema.pinyin.english_merge.complete_min_length` 另由 [`select`] 判）。
///
/// 判据（维护者定）：整串**任何切法**都切不成完整的全拼音节序列——`templa` / `produc` 补，
/// `shang` / `chang` 不补。「打的不是完整拼音」才说明用户可能在打英文；能切成完整音节的串，前缀词就是
/// [`lookup`] 文档里说的那种噪音。
///
/// 音节表用 [`crate::pinyin::syllable::is_full_pinyin_code`]（标准音节表、DP 判可达，与临时词
/// 晋升同一判据），**不含模糊音拼写**：开了模糊音的人打 `tinzhi` 也会补——宁可多给两条
/// 英文，不为一个可选开关去读引擎的模糊音状态。
///
/// 含非字母（分隔符 `'`、数字、符号）的串不补：那是拼音语法或在打别的东西。
/// 判据只对全拼成立；双拼由调用方（`manager::convert`）排除。
pub fn prefix_complete_applies(input: &str) -> bool {
    if input.is_empty() || !input.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    !crate::pinyin::syllable::is_full_pinyin_code(&input.to_ascii_lowercase(), 0)
}

/// 中文侧有候选时的英文前缀补全：至多 `max` 条，跳过精确命中（`exact` 即 [`lookup`] 的
/// 结果，调用方会把两者拼在一起）。是否该补、补几条由 [`select`] 定。
///
/// 次序取英文引擎的前缀排序（按词频权重，见 `wind_dict::freq_rank`），故 `produc` 先补
/// product。查询失败静默退化为空，口径同 [`lookup`]。
pub fn lookup_prefix_complete(
    english: &dyn Engine,
    input: &str,
    exact: &[Candidate],
    max: usize,
) -> Vec<Candidate> {
    let lower = input.to_lowercase();
    let Ok(r) = english.convert(&lower, EXACT_SCAN + max) else {
        return Vec::new();
    };
    r.candidates
        .into_iter()
        .filter(|c| c.code != lower && !exact.iter().any(|e| e.text == c.text))
        .map(|mut c| {
            // 同 `lookup`：码表域标志不跨来源带。
            c.is_exact_code = false;
            // 协调器据此把它排在整串拼音真词之后（见该字段文档）。
            c.meta.from_prefix_complete = true;
            c
        })
        .take(max)
        .collect()
}

/// 是否存在英文候选。供**上屏否决**判据用（`wind-engine/AGENTS.md`：否决必须叠
/// 「对方确有候选」，只看开关就禁上屏会把「英文被顶掉」修成「谁都上不了屏」）。
///
/// ⚠️ **仍按前缀判**，不复用 [`lookup`] 的精确收窄——理由见该函数文档「与 has_any 的判据
/// 刻意不同」。
///
/// **英文临时词不算**（2026-10-08 起英文引擎挂了临时词层，见 `[schema.english.auto_learn]`）：
/// 临时词是自动造词的暂存区，在英文方案里误打一次 `ggll` 就会进去；让它否决五笔四码自动
/// 上屏，等于一次误操作永久改了主方案的上屏行为。用户词（含已晋升的）照算。临时词排在
/// 同档系统词之前（层级基序 Temp < System），故多取几条再筛；取满仍全是临时词则按「没有」
/// 处理——宁可放行上屏，不误否决。
pub fn has_any(english: &dyn Engine, input: &str, min_length: usize) -> bool {
    /// 筛掉临时词后仍要有一条：取数上限。
    const PROBE: usize = 8;
    if input.chars().count() < min_length_or_default(min_length) {
        return false;
    }
    let lower = input.to_lowercase();
    match english.convert(&lower, PROBE) {
        Ok(r) => r.candidates.iter().any(counts_for_veto),
        Err(_) => false,
    }
}

/// 英文候选算不算数（否决用）：英文临时词不算，用户词（含已晋升的）照算。理由见 [`has_any`]。
fn counts_for_veto(c: &Candidate) -> bool {
    !c.meta.is_temp_dict || c.meta.is_user_dict
}

/// 整串是否恰是一个英文完整单词（成词级否决用）。门槛与临时词口径同 [`has_any`]。
pub fn has_word(english: &dyn Engine, input: &str, min_length: usize) -> bool {
    /// 精确整串恒居首，同码多形态留余量；同 [`EXACT_SCAN`]。
    const PROBE: usize = EXACT_SCAN;
    if input.chars().count() < min_length_or_default(min_length) {
        return false;
    }
    let lower = input.to_lowercase();
    match english.convert(&lower, PROBE) {
        Ok(r) => r
            .candidates
            .iter()
            .any(|c| c.code == lower && counts_for_veto(c)),
        Err(_) => false,
    }
}

/// 英文要不要否决这次上屏——满码上屏、显示态复评、顶码、满码空码清空**共用这一个判据**，
/// 码表方案（`manager`）与混输（`MixedEngine`）同口径。两级，结构同混输拼音那对
/// （`auto_commit_block_on_pinyin` / `block_commit_on_pinyin_word`）：
///
/// - **前缀级** `block_commit`：列表里有英文，或有英文词以当前串开头（[`has_any`]）；
/// - **成词级** `block_commit_on_word`：列表里有与整串同码的英文，或整串恰是英文单词（[`has_word`]）。
///
/// `shown` 是这次已进候选列表的候选（没有就传空）：列表那一半与查询那一半口径一致，英文
/// 临时词都不算。长度门槛取 [`Effective::veto_min_length`]（英文最早从几码露面）。
pub fn vetoes_commit(
    english: &dyn Engine,
    input: &str,
    shown: &[Candidate],
    cfg: &Effective,
) -> bool {
    use wind_candidate::CandidateSource;
    let real = |c: &&Candidate| c.source == CandidateSource::English && counts_for_veto(c);
    let gate = cfg.veto_min_length();
    if cfg.block_commit && (shown.iter().any(|c| real(&c)) || has_any(english, input, gate)) {
        return true;
    }
    if cfg.block_commit_on_word {
        let lower = input.to_lowercase();
        return shown.iter().filter(real).any(|c| c.code == lower)
            || has_word(english, input, gate);
    }
    false
}

/// 把英文候选混入 `base`，保证它们活过截断。
///
/// 次序**不做保证**：本函数只决定「谁活下来」，最终显示序由协调器
/// `candidate_display_order` 无条件重排全部候选决定（candidate-sorting-rules.md §6）。
/// 同 `MixedEngine::truncate_with_pinyin_quota` 的分工。
pub fn merge(base: &mut Vec<Candidate>, english: Vec<Candidate>, max_candidates: usize) {
    if english.is_empty() || max_candidates == 0 {
        return;
    }
    // 同文本时保留基础候选（它才是本方案的正主），把英文那条的码位并进去。
    // `absorb_codes_from` 自行挡掉跨来源合并（两套编码不同域），故此处无需再判 source。
    let mut kept: Vec<Candidate> = Vec::with_capacity(english.len());
    for c in english {
        match base.iter_mut().find(|b| b.text == c.text) {
            Some(existing) => existing.absorb_codes_from(&c),
            None => kept.push(c),
        }
    }
    if kept.is_empty() {
        return;
    }
    // ★ **先腾座、后追加**。基础候选通常已经恰好装满 `max_candidates`，
    // 「先 extend 再 truncate」会把英文原样截回去——那正是混输里英文无保底的现场。
    if base.len() + kept.len() > max_candidates {
        base.truncate(max_candidates.saturating_sub(kept.len()));
    }
    base.extend(kept);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ConvertResult, EngineType};
    use wind_candidate::CandidateSource;

    /// 固定吐 N 条英文候选的假引擎。
    struct FakeEnglish(Vec<(&'static str, &'static str)>);

    impl Engine for FakeEnglish {
        fn convert(&self, input: &str, max_candidates: usize) -> anyhow::Result<ConvertResult> {
            let candidates: Vec<Candidate> = self
                .0
                .iter()
                .filter(|(code, _)| code.starts_with(input))
                .take(max_candidates)
                .map(|(code, text)| Candidate {
                    text: (*text).into(),
                    code: (*code).into(),
                    source: CandidateSource::English,
                    ..Default::default()
                })
                .collect();
            Ok(ConvertResult {
                candidates,
                ..Default::default()
            })
        }
        fn reset(&self) {}
        fn engine_type(&self) -> EngineType {
            EngineType::English
        }
    }

    fn pinyin_flood(n: usize) -> Vec<Candidate> {
        (0..n)
            .map(|i| Candidate {
                text: format!("字{i}"),
                code: "he".into(),
                source: CandidateSource::Pinyin,
                ..Default::default()
            })
            .collect()
    }

    fn english_texts(cands: &[Candidate]) -> Vec<&str> {
        cands
            .iter()
            .filter(|c| c.source == CandidateSource::English)
            .map(|c| c.text.as_str())
            .collect()
    }

    /// ★ 本模块存在的理由：拼音把配额吐满时，英文仍须在场。
    ///
    /// 正向对照（`calm`）必须先立——否则「洪水下在场」可能只是因为假引擎压根没产出，
    /// 断言变成空转（对照 `MixedEngine` 那条同名教训）。
    #[test]
    fn english_survives_pinyin_flood() {
        let eng = FakeEnglish(vec![("hello", "hello"), ("help", "help")]);
        let max = 20;

        // 正向对照：无洪水时英文在场。
        let mut calm = pinyin_flood(2);
        merge(&mut calm, lookup(&eng, "hello", 0), max);
        assert!(
            !english_texts(&calm).is_empty(),
            "无洪水时英文应在场——否则下面那条断言测不到东西"
        );

        // 洪水：拼音已装满 max，英文仍须活下来，且总数不超 max。
        let mut flooded = pinyin_flood(max);
        merge(&mut flooded, lookup(&eng, "hello", 0), max);
        assert_eq!(
            english_texts(&flooded),
            vec!["hello"],
            "拼音装满配额时英文被整片截掉 = 开关等于没做"
        );
        assert_eq!(flooded.len(), max, "腾座后总数不得超出 max_candidates");
    }

    /// ★ 只收精确命中：前缀扩展一条都不进列表。
    ///
    /// 真机现象：全拼打 `hen` 混进 hen / hence / henceforth / Henderson / Hendrix 五条，
    /// 后四条纯噪音；混输打 `github` 混进 8 条 GitHub 开头的长词组。
    #[test]
    fn prefix_expansions_are_dropped() {
        let eng = FakeEnglish(vec![
            ("hen", "hen"),
            ("hence", "hence"),
            ("henderson", "Henderson"),
            ("hendrix", "Hendrix"),
        ]);
        let got = lookup(&eng, "hen", 0);
        assert_eq!(
            got.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["hen"],
            "前缀扩展必须整片丢弃，只留精确命中"
        );

        // 没有精确命中时一条都不出——用户该继续敲完，而不是从前缀词里挑。
        assert!(
            lookup(&eng, "hend", 0).is_empty(),
            "`hend` 无精确命中，不得拿 Henderson/Hendrix 顶上"
        );
    }

    /// ★ 上屏否决仍按**前缀**判，不随 `lookup` 一起收窄成精确。
    ///
    /// 收窄了的话，五笔打到第 4 键 `gith` 时精确命中还不存在 ⇒ 否决不生效 ⇒ 满码把中文
    /// 顶上屏 ⇒ `github` 永远敲不完。这正是 `block_commit` 要防的事。
    #[test]
    fn veto_still_matches_prefix() {
        let eng = FakeEnglish(vec![("github", "GitHub")]);
        assert!(
            lookup(&eng, "gith", 0).is_empty(),
            "前提：`gith` 无精确命中（否则下面测不到差别）"
        );
        assert!(
            has_any(&eng, "gith", 0),
            "否决判据必须仍按前缀命中，否则打到一半就被顶上屏"
        );
        // 短输入门槛对否决同样生效。
        assert!(!has_any(&eng, "gi", 0), "两字母不该触发否决");
    }

    /// 英文临时词（自动造词的暂存区）不参与上屏否决：只有临时词命中时按「没有英文候选」
    /// 处理，五笔四码照常自动上屏；临时词与用户词合并的（已晋升 / 手工加过）仍算。
    #[test]
    fn veto_ignores_temp_words() {
        struct Flags(Vec<(&'static str, bool, bool)>); // (code, is_temp, is_user)
        impl Engine for Flags {
            fn convert(&self, input: &str, max: usize) -> anyhow::Result<ConvertResult> {
                let candidates = self
                    .0
                    .iter()
                    .filter(|(c, _, _)| c.starts_with(input))
                    .take(max)
                    .map(|(c, t, u)| {
                        let mut cand = Candidate {
                            text: (*c).into(),
                            code: (*c).into(),
                            source: CandidateSource::English,
                            ..Default::default()
                        };
                        cand.meta.is_temp_dict = *t;
                        cand.meta.is_user_dict = *u;
                        cand
                    })
                    .collect();
                Ok(ConvertResult {
                    candidates,
                    ..Default::default()
                })
            }
            fn reset(&self) {}
            fn engine_type(&self) -> EngineType {
                EngineType::English
            }
        }
        assert!(!has_any(&Flags(vec![("ggll", true, false)]), "ggll", 0));
        assert!(
            has_any(&Flags(vec![("ggll", true, true)]), "ggll", 0),
            "已入用户词库的照算"
        );
        assert!(
            has_any(
                &Flags(vec![("ggllx", true, false), ("ggllz", false, false)]),
                "ggll",
                0
            ),
            "临时词排在前面也要看得到后面的系统词"
        );
    }

    /// 精确命中清掉 `is_exact_code`：那是码表域标志，跨来源带进拼音列表会让英文
    /// 在协调器 `cmp_exact_first`（位置在 `by_weight` 之前）无条件压过全部中文。
    #[test]
    fn exact_hit_clears_codetable_flag() {
        let eng = FakeEnglish(vec![("hen", "hen")]);
        let got = lookup(&eng, "hen", 0);
        assert_eq!(got.len(), 1);
        assert!(
            !got[0].is_exact_code,
            "英文候选不得带着码表域的 is_exact_code 进跨来源列表"
        );
    }

    /// 短输入不查英文：两个字母就刷前缀词会淹掉正常中文输入。
    #[test]
    fn short_input_skips_lookup() {
        let eng = FakeEnglish(vec![("he", "he"), ("hello", "hello")]);
        assert!(
            lookup(&eng, "he", 0).is_empty(),
            "默认回退长度 3，两字母不该查"
        );
        assert!(!lookup(&eng, "hello", 0).is_empty());
        // 显式配置覆盖回退值。
        assert!(!lookup(&eng, "he", 2).is_empty());
    }

    /// 拼音补全判据：整串**任何切法**都切不成完整全拼音节才补。
    #[test]
    fn prefix_complete_only_when_not_full_pinyin() {
        // 切不完整 ⇒ 补。
        for s in ["templa", "configu", "produc", "xiaom"] {
            assert!(prefix_complete_applies(s), "{s} 不是完整拼音，应补");
        }
        // 能切成完整音节 ⇒ 不补。`xian` 这类有多种切法的也算（xi'an / xian）。
        for s in ["chang", "shang", "zhongguo", "xianshi", "nihao"] {
            assert!(!prefix_complete_applies(s), "{s} 是完整拼音，不该补");
        }
        // 带分隔符 / 非字母：用户在用拼音语法或打符号，不补。
        assert!(!prefix_complete_applies("tem'pla"));
        // 大写按小写判（`Shang` 仍是完整拼音）。
        assert!(!prefix_complete_applies("Shang"));
    }

    /// 拼音侧：够长且不是完整拼音才补；完整拼音只给完整单词；长度门槛照码表口径。
    #[test]
    fn select_pinyin_completion_needs_non_pinyin() {
        let eng = FakeEnglish(vec![
            ("templa", "templa"),
            ("template", "template"),
            ("templates", "templates"),
            ("templar", "templar"),
            ("shang", "shang"),
            ("shanghai", "Shanghai"),
        ]);
        let cfg = Effective {
            block_commit: false,
            block_commit_on_word: false,
            complete_min_length: 5,
            complete_only_non_pinyin: true,
            ..codetable_cfg()
        };
        assert_eq!(
            texts_of(&select(&eng, "templa", false, &cfg)),
            vec!["templa", "template"],
            "完整单词优先，合计不超 max_count"
        );
        assert_eq!(
            texts_of(&select(&eng, "shang", false, &cfg)),
            vec!["shang"],
            "完整拼音不补"
        );
        let short = Effective {
            complete_min_length: 7,
            ..cfg
        };
        assert_eq!(
            texts_of(&select(&eng, "templa", false, &short)),
            vec!["templa"]
        );
    }

    /// 前缀补全至多 2 条，且不与精确命中重复。
    #[test]
    fn prefix_complete_caps_and_skips_exact() {
        let eng = FakeEnglish(vec![
            ("hello", "hello"),
            ("hellos", "hellos"),
            ("hellow", "Hellow"),
            ("hellox", "hellox"),
        ]);
        let exact = lookup(&eng, "hello", 0);
        assert_eq!(exact.len(), 1, "前提：精确命中 hello");
        let extra = lookup_prefix_complete(&eng, "hello", &exact, 2);
        let texts: Vec<&str> = extra.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["hellos", "Hellow"], "至多 2 条、跳过精确那条");
        assert!(extra.iter().all(|c| !c.is_exact_code));
        assert!(
            extra.iter().all(|c| c.meta.from_prefix_complete),
            "补全候选要带标记，协调器靠它定位"
        );

        // 无精确命中时照补。
        let none = lookup(&eng, "hell", 0);
        assert!(none.is_empty());
        assert_eq!(lookup_prefix_complete(&eng, "hell", &none, 2).len(), 2);
    }

    /// 前缀兜底页：与输入相同（大小写不敏感）的词排最前，且次序落到 weight。
    #[test]
    fn prefix_fallback_puts_exact_word_first() {
        let eng = FakeEnglish(vec![
            ("his", "his"),
            ("high", "high"),
            ("him", "him"),
            ("history", "history"),
            ("hi", "Hi"),
        ]);
        let got = lookup_prefix_fallback(&eng, "hi", 0);
        assert_eq!(texts_of(&got), vec!["Hi", "his", "high", "him", "history"]);
        assert!(
            got[1..].iter().all(|c| c.weight < got[0].weight),
            "次序要落到 weight，否则协调器重排会冲掉"
        );
    }

    /// 码表侧出厂口径（补全关、上限 2）。
    fn codetable_cfg() -> Effective {
        Effective {
            enable: true,
            min_length: 0,
            block_commit: true,
            block_commit_on_word: true,
            complete_min_length: 0,
            complete_only_non_pinyin: false,
            max_count: max_count_or_default(0),
        }
    }

    fn texts_of(v: &[Candidate]) -> Vec<&str> {
        v.iter().map(|c| c.text.as_str()).collect()
    }

    fn git_words() -> FakeEnglish {
        FakeEnglish(vec![
            ("git", "git"),
            ("github", "GitHub"),
            ("githubpages", "GitHub Pages"),
            ("gitlab", "GitLab"),
        ])
    }

    /// 出厂口径：中文侧有候选时 3 / 4 码只出完整单词，不补前缀（现状不变）。
    #[test]
    fn select_default_only_exact_when_base_present() {
        let eng = git_words();
        let cfg = codetable_cfg();
        assert_eq!(texts_of(&select(&eng, "git", false, &cfg)), vec!["git"]);
        assert!(select(&eng, "gith", false, &cfg).is_empty());
        // 中文侧为空：给前缀补全一页（不受 max_count 约束）。
        assert_eq!(
            texts_of(&select(&eng, "gith", true, &cfg)),
            vec!["GitHub", "GitHub Pages"]
        );
    }

    /// `complete_min_length = 4`：4 码起中文有候选也补，3 码不补。
    #[test]
    fn select_complete_min_length_gates_completion() {
        let eng = git_words();
        let cfg = Effective {
            complete_min_length: 4,
            ..codetable_cfg()
        };
        let got = select(&eng, "gith", false, &cfg);
        assert_eq!(texts_of(&got), vec!["GitHub", "GitHub Pages"]);
        assert!(got.iter().all(|c| c.meta.from_prefix_complete));
        assert_eq!(
            texts_of(&select(&eng, "git", false, &cfg)),
            vec!["git"],
            "3 码不到补全门槛，只给完整单词"
        );
    }

    /// `max_count` 封顶：完整单词优先，补全只填剩下的座位。
    #[test]
    fn select_max_count_caps_exact_first() {
        let eng = git_words();
        let one = Effective {
            complete_min_length: 3,
            max_count: 1,
            ..codetable_cfg()
        };
        assert_eq!(texts_of(&select(&eng, "git", false, &one)), vec!["git"]);
        let three = Effective {
            max_count: 3,
            ..one
        };
        assert_eq!(
            texts_of(&select(&eng, "git", false, &three)),
            vec!["git", "GitHub", "GitHub Pages"]
        );
    }

    /// `complete_min_length` 比 `min_length` 小时，完整单词也从补全门槛起出：
    /// 打 `go` 不能只补出 good / google 而漏掉 go。
    #[test]
    fn select_exact_gate_follows_completion_gate() {
        let eng = FakeEnglish(vec![("go", "go"), ("good", "good"), ("google", "Google")]);
        let cfg = Effective {
            complete_min_length: 2,
            ..codetable_cfg()
        };
        assert_eq!(
            texts_of(&select(&eng, "go", false, &cfg)),
            vec!["go", "good"]
        );
        // 不开补全时仍按 min_length（默认 3）：2 码一条都不出。
        assert!(select(&eng, "go", false, &codetable_cfg()).is_empty());
    }

    /// 两级否决：前缀级涵盖成词级；只开成词级时，英文词的前缀不拦、整词才拦。
    #[test]
    fn veto_two_levels() {
        let eng = git_words();
        let prefix = codetable_cfg();
        assert!(
            vetoes_commit(&eng, "gith", &[], &prefix),
            "前缀级：gith 是 github 的前缀"
        );
        let word_only = Effective {
            block_commit: false,
            ..prefix
        };
        assert!(
            !vetoes_commit(&eng, "gith", &[], &word_only),
            "成词级不管前缀"
        );
        assert!(
            vetoes_commit(&eng, "github", &[], &word_only),
            "整串是英文单词"
        );
        let off = Effective {
            block_commit_on_word: false,
            ..word_only
        };
        assert!(!vetoes_commit(&eng, "github", &[], &off));
    }

    /// 列表那一半同样不算英文临时词（只有临时词进了列表时放行上屏）。
    #[test]
    fn veto_shown_list_ignores_temp_words() {
        let none = FakeEnglish(vec![]);
        let mut temp = Candidate {
            text: "ggll".into(),
            code: "ggll".into(),
            source: CandidateSource::English,
            ..Default::default()
        };
        temp.meta.is_temp_dict = true;
        let cfg = codetable_cfg();
        assert!(!vetoes_commit(
            &none,
            "ggll",
            std::slice::from_ref(&temp),
            &cfg
        ));
        temp.meta.is_user_dict = true;
        assert!(
            vetoes_commit(&none, "ggll", &[temp], &cfg),
            "已入用户词库的照算"
        );
    }

    /// 否决门槛跟着「英文最早从几码露面」走。
    #[test]
    fn veto_gate_follows_completion_gate() {
        let cfg = codetable_cfg();
        assert_eq!(cfg.veto_min_length(), 3);
        let early = Effective {
            complete_min_length: 2,
            ..cfg
        };
        assert_eq!(early.veto_min_length(), 2, "补全 2 码起出，否决也该 2 码起");
        let late = Effective {
            complete_min_length: 5,
            ..cfg
        };
        assert_eq!(late.veto_min_length(), 3, "完整单词仍从 3 码起出");
    }

    /// 同文本不重复入列，且被丢弃那条的码位并进幸存者。
    #[test]
    fn same_text_dedups_into_base() {
        let eng = FakeEnglish(vec![("ok", "OK")]);
        let mut base = vec![Candidate {
            text: "OK".into(),
            code: "ok".into(),
            source: CandidateSource::English,
            ..Default::default()
        }];
        merge(&mut base, lookup(&eng, "ok", 1), 10);
        assert_eq!(base.len(), 1, "同文本不该重复入列");
    }
}
