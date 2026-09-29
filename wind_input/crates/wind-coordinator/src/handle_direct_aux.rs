//! 直接辅助码（双拼）：不按引导键，输入末 1～2 位自动当辅码，字形对得上的字词提到前面。
//! 设计见 `docs/design/aux-code-direct.md`；切分 / 匹配 / 并入的纯逻辑在
//! `wind_aux_code::direct`，本模块只做协调器那一半：
//!
//! - **门卫**（§4）：主输入路、双拼、`enabled` + `direct`、前缀恰好切成完整双拼音节、
//!   辅码来源就绪（方案来源的系统层未就绪 → 本键原样、派后台构建，与引导键进入同一处理）；
//! - **前缀解码**：对前缀单独调一次引擎（`convert_with_opts`，引擎无状态，不碰本次会话），
//!   走与主候选同一条加工链（展开 / 常用字标记 / 显示序 / 检索范围 / 单字 / 调频 / shadow），
//!   所以「命中项之间的顺序」就是用户单打前缀时看到的顺序；
//! - **标记**：命中项的 `consumed_length` 改成整串（上屏连辅码一起吃掉），`code` 保留前缀
//!   的拼音码（调频与学习记在前缀下，「释读」记 `shidu`）；组码区形态存 `direct_aux_body`。
//!
//! ★ 调用点钉在 `build_candidates` 的 `apply_shadow` **之前**、全部重排之后：翻页扩容
//! （`expand_candidates`）只重跑 `build_candidates`，放到 `update_candidates` 会在翻页时丢失。

use crate::coordinator::{Coordinator, State};
use std::sync::Arc;
use wind_aux_code::DirectPhraseRule;
use wind_candidate::{Candidate, CandidateSource, candidate_display_order};

/// 「上一个整音节输入」的主候选快照（`State.direct_aux_prev`）：连打时前缀恰是几键前的整个
/// 输入（`uidup` / `uidupl` 的前缀 `uidu` 就是敲到第 4 键时的输入），那一键的候选早已算好，
/// 直接取用即可，不必对前缀再调一次引擎（设计附录 A「取上一次按键的候选，免重算」）。
///
/// 存的是那一键**并入命中项之前、shadow 之前**的主候选：已走完展开 / 常用字 / 显示序 / 检索
/// 范围 / 单字 / 调频（调频的码就是那时的输入 = 现在的前缀），也就是用户单打前缀时所见的
/// 顺序（shadow 在取用时按前缀的码补上）。只在输入恰好切成完整双拼音节时才更新；奇数键不动它，
/// 留给紧随其后的偶数键。组合复位（上屏 / 清空）时丢弃。
pub(crate) struct DirectAuxPrev {
    input: String,
    candidates: Vec<Candidate>,
    /// 那一键的双拼音节分段（`ui'du`），组码区用。
    preedit: String,
    /// 那一键的 shadow 归一码（双拼下是全拼码），前缀 shadow 用。
    shadow_code: String,
}

impl Coordinator {
    /// 把直接辅助码的命中项并入 `candidates`（主候选，已排序去重过滤、尚未 shadow）。
    /// 任一门卫不过 / 无命中 → 候选原样。组码区形态写进 `state.direct_aux_body`（无命中清空）。
    pub(crate) fn apply_direct_aux(
        &self,
        state: &mut State,
        candidates: &mut Vec<Candidate>,
        limit: usize,
    ) {
        state.direct_aux_body.clear();
        // 主输入路：临拼 / 混输 / 引导键辅助码态都不做（引导键态本就筛的是现成候选表）。
        if state.active.is_some() {
            state.direct_aux_prev = None;
            return;
        }
        // 两件事各要一道双拼音节判定（纯内存；全拼 / 码表 / 混输恒 None，零额外成本）：
        // 本键输入整串成音节 → 存快照给后面的键当前缀；前缀成音节 → 本键做直接辅助。
        let input = state.input_buffer.clone();
        let whole = self
            .engine_mgr
            .shuangpin_full_syllable_count(&input)
            .is_some();
        let split = wind_aux_code::split_direct(&input);
        let syllables = split.and_then(|s| self.engine_mgr.shuangpin_full_syllable_count(s.prefix));
        if !whole && syllables.is_none() {
            return;
        }
        let settings = self.engine_mgr.aux_code_settings();
        if !settings.direct || settings.sources.is_empty() {
            state.direct_aux_prev = None;
            return;
        }
        let pre_merge = whole.then(|| candidates.clone());
        if let (Some(split), Some(syllables)) = (split, syllables) {
            self.merge_direct_hits_into(state, candidates, limit, split, syllables, &settings);
        }
        // 奇数键（或前缀以外的不成音节输入）不动快照，留给后面的键。
        if let Some(list) = pre_merge {
            state.direct_aux_prev = Some(DirectAuxPrev {
                input,
                candidates: list,
                preedit: state.preedit_split_body.clone(),
                shadow_code: state.shadow_code.clone(),
            });
        }
    }

    fn merge_direct_hits_into(
        &self,
        state: &mut State,
        candidates: &mut Vec<Candidate>,
        limit: usize,
        split: wind_aux_code::DirectSplit<'_>,
        syllables: usize,
        settings: &wind_engine::AuxCodeSettings,
    ) {
        let rt = self.ensure_aux_code_runtime(&settings.sources);
        let lookup = Arc::new(rt.lookup(&self.engine_mgr));
        // 方案来源的系统层（反查索引）未就绪：本键不做、派后台构建。按键线程绝不现建。
        let unready = lookup.unready_schemas();
        if !unready.is_empty() {
            for id in unready {
                self.spawn_index_warm(id, false);
            }
            tracing::debug!("direct aux: 方案来源索引未就绪，本键不做直接辅助");
            return;
        }
        let prefix = split.prefix;
        let snapshot = state
            .direct_aux_prev
            .as_ref()
            .filter(|p| p.input == prefix)
            .map(|p| {
                (
                    p.candidates.clone(),
                    p.preedit.clone(),
                    p.shadow_code.clone(),
                )
            });
        let (mut pool, preedit, shadow_code) = match snapshot {
            Some(s) => s,
            // 没有现成快照（退格改了前缀、光标中间编辑、刚开启…）：对前缀单独解码一次，
            // 走与主候选同一条加工链。
            None => self.decode_direct_prefix(state, prefix, limit, split.aux, settings, &lookup),
        };
        // 用户在前缀下隐藏的词，当辅码命中项也不该冒出来（置顶同理保留其次序）。
        let prefix_shadow = if shadow_code.is_empty() {
            prefix
        } else {
            &shadow_code
        };
        self.apply_shadow(&mut pool, prefix_shadow);

        let input_len = state.input_buffer.len();
        let hits: Vec<Candidate> = pool
            .into_iter()
            .filter(|c| {
                c.source == CandidateSource::Pinyin
                    && wind_aux_code::is_direct_source(c, prefix.len())
                    && wind_aux_code::direct_matches(
                        &c.text,
                        &*lookup,
                        split.aux,
                        DirectPhraseRule::Any,
                        settings.max_phrase_len,
                    )
            })
            .map(|mut c| {
                c.consumed_length = input_len;
                c.is_direct_aux = true;
                c
            })
            .collect();
        if hits.is_empty() {
            return;
        }
        let placement = wind_aux_code::direct_placement(split.aux.len(), syllables);
        *candidates = wind_aux_code::merge_direct_hits(
            std::mem::take(candidates),
            hits,
            input_len,
            placement,
        );
        // 组码区：前缀按双拼音节切（引擎给的击键分段，如 `ui'du`）+ 空格 + 辅码。与引导键模式
        // 同口径地用空白隔开辅码；仍是「缓冲按序插入分隔符」的形态，光标换算照常成立。
        let body = if preedit.is_empty() { prefix } else { &preedit };
        state.direct_aux_body = format!("{body} {}", split.aux);
    }

    /// 对前缀单独解码（快照缺失时的兜底）：`convert_with_opts` 不碰本次会话（引擎无状态），
    /// 结果走与主候选同一条加工链（shadow 除外，由调用方按前缀的码补）。
    /// 返回（候选, 双拼音节分段, shadow 归一码）。
    fn decode_direct_prefix(
        &self,
        state: &State,
        prefix: &str,
        limit: usize,
        aux: &str,
        settings: &wind_engine::AuxCodeSettings,
        lookup: &Arc<crate::aux_code_source::AuxLookupNow>,
    ) -> (Vec<Candidate>, String, String) {
        let max_phrase_len = settings.max_phrase_len;
        let admit_lookup = lookup.clone();
        let letter: String = aux.chars().take(1).collect();
        let opts = wind_engine::engine::ConvertOptions {
            // 只要吃满前缀的候选，且在截断**之前**丢掉其余（否则同音单字会把词挤出配额）。
            require_full_match: true,
            no_abbrev_quota: true,
            // 按辅码**首字母**准入（2 位辅码的命中集是 1 位的子集，见 `direct_matches`）：
            // 截断前就只剩可能命中的——`ui`（shi）有几百个同音字，全量取一遍要多花十几毫秒。
            admit: Some(Arc::new(move |text: &str| {
                wind_aux_code::direct_matches(
                    text,
                    &*admit_lookup,
                    &letter,
                    DirectPhraseRule::Any,
                    max_phrase_len,
                )
            })),
            ..Default::default()
        };
        let active = self.engine_mgr.active_schema_id();
        let r = self
            .engine_mgr
            .convert_with_opts(&active, prefix, limit, opts);
        let mut pool = self.finalize_candidates(r.candidates, prefix);
        self.mark_common(&mut pool);
        let ignore_weight = self.engine_mgr.active_base_sort_ignores_weight();
        pool.sort_by(|a, b| candidate_display_order(a, b, ignore_weight, false, prefix));
        let mut seen = std::collections::HashSet::new();
        pool.retain(|c| seen.insert(c.text.clone()));
        self.apply_filter(state, &mut pool);
        self.apply_single_char(state, &mut pool);
        let rerank_len = pool.iter().take_while(|c| !c.is_scope_filtered).count();
        self.apply_freq_rerank(&mut pool[..rerank_len], prefix);
        (pool, r.preedit_pinyin, r.shadow_code)
    }
}

#[cfg(test)]
mod tests {
    //! 小词库 + 小码表夹具的无头集成测试：真实的小鹤双拼引擎（布局取入库的
    //! `data/schemas/shuangpin/xiaohe.toml`）、rime 源格式的迷你词库、`字=码` 码表，按键走
    //! `handle_key_event` 入口。真实数据（`build_dev/data`）那组在 `tests/aux_code_direct_real.rs`。
    use crate::coordinator::Coordinator;
    use crate::pipeline::ModeKind;
    use std::sync::Arc;
    use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
    use wind_config::Config;
    use wind_ipc::protocol::EVENT_KEY_DOWN;
    use wind_keys::keymap;
    use wind_store::Store;

    /// 迷你词库。权重让「湿度」天然居首、「释读」垫底——直接辅助码要把它提上来。
    const DICT: &str = "释读\tshi du\t10\n湿度\tshi du\t5000\n适度\tshi du\t3000\n\
                        十度\tshi du\t1000\n试读\tshi du\t500\n\
                        是\tshi\t9000\n十\tshi\t5000\n湿\tshi\t400\n释\tshi\t50\n\
                        读\tdu\t3000\n度\tdu\t2000\n\
                        国庆\tguo qing\t800\n国情\tguo qing\t600\n国\tguo\t5000\n\
                        想\txiang\t5000\n向\txiang\t4000\n像\txiang\t3000\n";
    /// 小鹤形码（取自 flypy_full.txt）。
    const AUX: &str = "释=pl\n湿=dy\n适=zk\n十=al\n试=yg\n读=yd\n度=gy\n是=or\n\
                       国=ky\n庆=gd\n情=xo\n想=mx\n向=pk\n像=rn\n";

    struct Fixture {
        dir: std::path::PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// `aux` 是 `[engine.aux_code]` 里 `files` 之外的行（enabled / direct）；`scheme` 空 = 全拼。
    fn fixture(tag: &str, scheme: &str, aux: &str) -> (std::path::PathBuf, Fixture) {
        let dir =
            std::env::temp_dir().join(format!("wind_direct_aux_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(schemas.join("aux_code")).unwrap();
        std::fs::create_dir_all(schemas.join("sp")).unwrap();
        std::fs::create_dir_all(schemas.join("shuangpin")).unwrap();
        let layout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../data/schemas/shuangpin/xiaohe.toml");
        std::fs::copy(layout, schemas.join("shuangpin/xiaohe.toml")).unwrap();
        std::fs::write(
            schemas.join("sp/mini.dict.yaml"),
            format!("---\nname: mini\nversion: \"1\"\nsort: by_weight\n...\n{DICT}"),
        )
        .unwrap();
        std::fs::write(schemas.join("aux_code/t.txt"), AUX).unwrap();
        let pinyin = if scheme.is_empty() {
            String::new()
        } else {
            format!(
                "[engine.pinyin]\nscheme = \"{scheme}\"\n[engine.pinyin.shuangpin]\nlayout = \"xiaohe\"\n"
            )
        };
        std::fs::write(
            schemas.join("sp.schema.toml"),
            format!(
                "[schema]\nid = \"sp\"\nname = \"sp\"\n[engine]\ntype = \"pinyin\"\n{pinyin}\
                 [engine.aux_code]\nfiles = [\"aux_code/t.txt\"]\n{aux}\
                 [key_actions]\nbacktick = \"aux_code\"\n\
                 [[dictionaries]]\nid = \"mini\"\npath = \"sp/mini.dict.yaml\"\n\
                 type = \"rime_pinyin\"\ndefault = true\n"
            ),
        )
        .unwrap();
        (dir.clone(), Fixture { dir })
    }

    fn coord(tag: &str, data_dir: &std::path::Path) -> (Arc<Coordinator>, Arc<Store>) {
        let path =
            std::env::temp_dir().join(format!("wind_direct_aux_{tag}_{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Arc::new(Store::open(&path).unwrap());
        let mut cfg = Config::default();
        cfg.schema.available = vec!["sp".into()];
        cfg.schema.active = "sp".into();
        cfg.input.default.chinese_mode = true;
        // 出厂 L2 开着拼音调频，`Config::default()` 是类型默认（关）——不开就验不到记账码。
        cfg.schema.pinyin.frequency.enabled = true;
        let c = Coordinator::new_headless_with_store(cfg, Some(data_dir), store.clone());
        (c, store)
    }

    fn press(c: &Coordinator, vk: u32) -> KeyAction {
        c.handle_key_event(&KeyEventData {
            key_code: vk,
            scan_code: 0,
            modifiers: 0,
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        })
    }

    fn type_str(c: &Coordinator, s: &str) {
        for ch in s.chars() {
            press(c, keymap::VK_A + (ch as u32 - 'a' as u32));
        }
    }

    fn texts(c: &Coordinator) -> Vec<String> {
        c.state
            .lock()
            .unwrap()
            .candidates
            .iter()
            .map(|c| c.text.clone())
            .collect()
    }

    const ON: &str = "enabled = true\ndirect = true\n";

    /// 基线：同一夹具，不开直接辅助码时 `uidup` 的首选不是「释读」——否则下面的正向用例测不出东西。
    #[test]
    fn baseline_without_direct() {
        let (dir, _f) = fixture("base", "shuangpin", "enabled = true\n");
        let (c, _) = coord("base", &dir);
        type_str(&c, "uidu");
        assert_eq!(texts(&c).first().map(String::as_str), Some("湿度"));
        type_str(&c, "p");
        assert_ne!(texts(&c).first().map(String::as_str), Some("释读"));
    }

    /// 奇数长度、前缀 2 音节：命中项排最前；组码区显示「前缀 + 空格 + 辅码」；上屏连辅码一起
    /// 吃掉、组码清空；调频记在前缀的拼音编码下（`shidu`），辅码字母不进词频。
    #[test]
    fn odd_hit_goes_first_and_commits_whole_buffer() {
        let (dir, _f) = fixture("odd", "shuangpin", ON);
        let (c, store) = coord("odd", &dir);
        type_str(&c, "uidup");
        let t = texts(&c);
        assert_eq!(t.first().map(String::as_str), Some("释读"), "{t:?}");
        assert!(t.contains(&"湿度".to_string()), "其余候选去重接后：{t:?}");
        assert_eq!(t.iter().filter(|x| *x == "释读").count(), 1, "不重复");
        assert_eq!(
            c.debug_preedit(),
            "ui'du p",
            "高亮命中项：前缀音节 + 空格 + 辅码"
        );
        let act = press(&c, keymap::VK_SPACE);
        match act {
            KeyAction::InsertText { text, .. } => assert_eq!(text, "释读"),
            other => panic!("应整体上屏：{other:?}"),
        }
        let st = c.state.lock().unwrap();
        assert!(st.input_buffer.is_empty(), "上屏连辅码一起吃掉，组码清空");
        assert!(st.direct_aux_body.is_empty());
        drop(st);
        assert!(
            store.get_freq("pinyin", "shidu", "释读").unwrap().is_some(),
            "词频记在前缀的拼音编码下"
        );
        assert!(
            store
                .get_freq("pinyin", "shidup", "释读")
                .unwrap()
                .is_none()
        );
    }

    /// 连打时前缀取「几键前那一键」的候选快照；快照缺失（退格改了前缀、光标中间编辑…）
    /// 时对前缀单独解码兜底。两条路给出同样的命中。
    #[test]
    fn prefix_snapshot_and_fallback_decode_agree() {
        let (dir, _f) = fixture("snap", "shuangpin", ON);
        let (c, _) = coord("snap", &dir);
        type_str(&c, "uidu");
        {
            let st = c.state.lock().unwrap();
            let prev = st.direct_aux_prev.as_ref().expect("整音节输入应留下快照");
            assert_eq!(prev.input, "uidu");
        }
        type_str(&c, "p");
        let via_snapshot = texts(&c);
        // 奇数键不动快照，留给紧随其后的偶数键。
        assert_eq!(
            c.state
                .lock()
                .unwrap()
                .direct_aux_prev
                .as_ref()
                .map(|p| p.input.clone()),
            Some("uidu".into())
        );
        press(&c, keymap::VK_BACK);
        c.state.lock().unwrap().direct_aux_prev = None;
        type_str(&c, "p");
        assert_eq!(texts(&c), via_snapshot, "兜底解码与快照给出同样的候选");
        assert_eq!(via_snapshot.first().map(String::as_str), Some("释读"));
    }

    /// 高亮移到非命中项时组码区回到普通双拼分段。
    #[test]
    fn preedit_follows_highlight() {
        let (dir, _f) = fixture("hl", "shuangpin", ON);
        let (c, _) = coord("hl", &dir);
        type_str(&c, "uidup");
        assert_eq!(c.debug_preedit(), "ui'du p");
        press(&c, keymap::VK_DOWN);
        let st = c.state.lock().unwrap();
        let hi = c.highlighted_global_index(&st);
        assert!(!st.candidates[hi].is_direct_aux, "第 2 位应是普通候选");
        drop(st);
        assert_eq!(c.debug_preedit(), "ui'du'p", "普通候选：原双拼分段");
    }

    /// 偶数长度：`xlrn` = xiang + `rn`，像=rn。无全音节候选可保留时命中项排最前。
    #[test]
    fn even_two_letter_aux() {
        let (dir, _f) = fixture("even", "shuangpin", ON);
        let (c, _) = coord("even", &dir);
        type_str(&c, "xlrn");
        assert_eq!(
            texts(&c).first().map(String::as_str),
            Some("像"),
            "{:?}",
            texts(&c)
        );
        // 国庆 / 国情：1 位辅码 k 两者都中（国=ky）；g 只中国庆（庆=gd，情=xo）。
        let (c, _) = coord("even2", &dir);
        type_str(&c, "goqkg");
        let t = texts(&c);
        assert_eq!(t.first().map(String::as_str), Some("国庆"), "{t:?}");
        assert!(
            t.iter().position(|x| x == "国情") > Some(1),
            "国情不中 g，不被提前：{t:?}"
        );
    }

    /// 退格按原始按键串回滚：`uidup` 退一格回到 `uidu`，命中与组码区形态一并消失。
    #[test]
    fn backspace_rolls_back_to_plain_prefix() {
        let (dir, _f) = fixture("bs", "shuangpin", ON);
        let (c, _) = coord("bs", &dir);
        type_str(&c, "uidup");
        press(&c, keymap::VK_BACK);
        let st = c.state.lock().unwrap();
        assert_eq!(st.input_buffer, "uidu");
        assert!(st.direct_aux_body.is_empty());
        assert!(st.candidates.iter().all(|c| !c.is_direct_aux));
        assert_eq!(st.candidates.first().map(|c| c.text.as_str()), Some("湿度"));
    }

    /// 翻页扩容只重跑 `build_candidates`：命中项必须还在（插入点放在它里面的理由）。
    #[test]
    fn hits_survive_page_expansion() {
        let (dir, _f) = fixture("page", "shuangpin", ON);
        let (c, _) = coord("page", &dir);
        type_str(&c, "uidup");
        let mut st = c.state.lock().unwrap();
        st.has_more = true;
        c.expand_candidates(&mut st);
        assert_eq!(st.candidates.first().map(|c| c.text.as_str()), Some("释读"));
        assert!(st.candidates[0].is_direct_aux);
    }

    /// 四种不生效：`direct` 关 / `enabled` 关 / 全拼 / 引导键态中。
    #[test]
    fn not_applicable_cases_leave_candidates_alone() {
        for (tag, scheme, aux) in [
            ("off_direct", "shuangpin", "enabled = true\n"),
            ("off_total", "shuangpin", "enabled = false\ndirect = true\n"),
            ("quanpin", "", ON),
        ] {
            let (dir, _f) = fixture(tag, scheme, aux);
            let (c, _) = coord(tag, &dir);
            type_str(&c, if scheme.is_empty() { "shidup" } else { "uidup" });
            let st = c.state.lock().unwrap();
            assert!(
                st.candidates.iter().all(|c| !c.is_direct_aux),
                "{tag}: 不应有直接辅助命中"
            );
            assert!(st.direct_aux_body.is_empty(), "{tag}");
        }
        // 引导键态：直接辅助停用（同一输入，门卫先看 `state.active`）。
        let (dir, _f) = fixture("guide", "shuangpin", ON);
        let (c, _) = coord("guide", &dir);
        type_str(&c, "uidup");
        let mut st = c.state.lock().unwrap();
        st.active = Some(ModeKind::AuxCode);
        let mut cands = vec![wind_candidate::Candidate {
            text: "湿度".into(),
            ..Default::default()
        }];
        c.apply_direct_aux(&mut st, &mut cands, 50);
        assert_eq!(cands.len(), 1);
        assert!(st.direct_aux_body.is_empty());
        st.active = None;
        drop(st);
        // 反向对照：同一 coordinator 在主输入路上确实会命中（上面那条不是因为别的原因空转）。
        assert_eq!(texts(&c).first().map(String::as_str), Some("释读"));
    }

    /// ★ 方案来源（`schema:<id>`）的系统层没就绪：本键不做直接辅助、候选原样，派后台构建；
    /// 建好后下一键即生效。按键线程绝不现建反查索引（与引导键进入同一处理）。
    #[test]
    fn schema_source_not_ready_is_noop_then_warms() {
        let id = format!("zz_da_cold_{}", std::process::id());
        struct CacheGuard(String);
        impl Drop for CacheGuard {
            fn drop(&mut self) {
                if let Some(cache) = Config::cache_dir() {
                    let _ = std::fs::remove_dir_all(cache.join(&self.0));
                }
            }
        }
        let _cg = CacheGuard(id.clone());
        let (dir, _f) = fixture("cold", "shuangpin", ON);
        let schemas = dir.join("schemas");
        std::fs::create_dir_all(schemas.join(&id)).unwrap();
        std::fs::write(
            schemas.join(format!("{id}.schema.toml")),
            format!(
                "[schema]\nid = \"{id}\"\nname = \"形\"\n[engine]\ntype = \"codetable\"\n\
                 [[dictionaries]]\nid = \"{id}_main\"\npath = \"{id}/{id}.dict.yaml\"\n\
                 type = \"rime_codetable\"\ndefault = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            schemas.join(format!("{id}/{id}.dict.yaml")),
            format!(
                "---\nname: {id}\nversion: \"1\"\ncolumns:\n  - code\n  - text\n  - weight\n...\n\
                 pl\t释\t10\ndy\t湿\t10\nyd\t读\t10\ngy\t度\t10\n"
            ),
        )
        .unwrap();
        let sp = schemas.join("sp.schema.toml");
        let text = std::fs::read_to_string(&sp).unwrap();
        let swapped = text.replace(r#"["aux_code/t.txt"]"#, &format!(r#"["schema:{id}"]"#));
        assert_ne!(swapped, text);
        std::fs::write(&sp, swapped).unwrap();
        let (c, _) = coord("cold", &dir);
        type_str(&c, "uidup");
        {
            let st = c.state.lock().unwrap();
            assert!(
                st.candidates.iter().all(|c| !c.is_direct_aux),
                "未就绪：原样"
            );
            assert_ne!(st.candidates.first().map(|c| c.text.as_str()), Some("释读"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while c.engine_mgr.reverse_index_if_ready(&id).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "后台构建 5 秒内没建好"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        press(&c, keymap::VK_BACK);
        type_str(&c, "p");
        assert_eq!(
            texts(&c).first().map(String::as_str),
            Some("释读"),
            "就绪后下一键即生效"
        );
    }

    /// 直接辅助的结果上仍可按反引号进引导键筛选，筛的是当前整张候选表。
    #[test]
    fn guide_key_filters_current_list() {
        let (dir, _f) = fixture("guide2", "shuangpin", ON);
        let (c, _) = coord("guide2", &dir);
        type_str(&c, "uidup");
        press(&c, keymap::VK_BACKTICK);
        let st = c.state.lock().unwrap();
        assert_eq!(st.active, Some(ModeKind::AuxCode));
        assert_eq!(st.candidates.first().map(|c| c.text.as_str()), Some("释读"));
    }
}
