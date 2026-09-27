//! 辅助码的运行时来源：`[engine.aux_code].files` 按序解析出的各层，查询时顺序拼接。
//!
//! 不预先合并成一张表：方案来源的码来自「系统词库 + 用户词库」，用户层会随造词后台重建，
//! 只能在每次筛选时取当下的视图（`EngineManager::text_codes`）。文件来源仍是进来时读一次的
//! 静态表。设计见 `docs/design/aux-code-schema-source.md` §4、§5。

use wind_aux_code::{AuxCodeLookup, AuxCodeTable};
use wind_engine::{AuxSource, EngineManager, TextCodeView};

pub(crate) struct AuxCodeRuntime {
    /// 由哪组来源建成——缓存键。来源变了（改了 override、换了方案）就重建。
    key: Vec<AuxSource>,
    /// 模式指示用的名字：首个来源的名（码表 `# name:` 或方案名）。
    name: String,
    layers: Vec<Layer>,
}

enum Layer {
    Table(AuxCodeTable),
    Schema(String),
}

impl AuxCodeRuntime {
    /// 文件来源同步读（码表小，与改动前同一时机）；方案来源只记 id，数据在查询时取。
    pub(crate) fn build(sources: &[AuxSource], engine: &EngineManager) -> Self {
        let mut name = String::new();
        let layers = sources
            .iter()
            .map(|s| match s {
                AuxSource::File(p) => {
                    let t = wind_aux_code::load_from_file(p);
                    if name.is_empty() {
                        name = t.name.clone();
                    }
                    Layer::Table(t)
                }
                AuxSource::Schema(id) => {
                    if name.is_empty() {
                        name = engine.schema_name(id);
                    }
                    Layer::Schema(id.clone())
                }
            })
            .collect();
        Self {
            key: sources.to_vec(),
            name,
            layers,
        }
    }

    pub(crate) fn matches(&self, sources: &[AuxSource]) -> bool {
        self.key == sources
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn schema_ids(&self) -> impl Iterator<Item = &str> {
        self.layers.iter().filter_map(|l| match l {
            Layer::Schema(id) => Some(id.as_str()),
            Layer::Table(_) => None,
        })
    }

    /// 本次筛选用的查询对象：方案层取当下视图（用户层过期会在这里触发后台重建）。
    pub(crate) fn lookup(&self, engine: &EngineManager) -> AuxLookupNow<'_> {
        AuxLookupNow {
            layers: self
                .layers
                .iter()
                .map(|l| match l {
                    Layer::Table(t) => Now::Table(t),
                    Layer::Schema(id) => Now::View(engine.text_codes(id)),
                })
                .collect(),
        }
    }
}

pub(crate) struct AuxLookupNow<'a> {
    layers: Vec<Now<'a>>,
}

enum Now<'a> {
    Table(&'a AuxCodeTable),
    View(TextCodeView),
}

impl AuxCodeLookup for AuxLookupNow<'_> {
    fn any_code(&self, ch: char, pred: &mut dyn FnMut(&str) -> bool) -> bool {
        let mut buf = [0u8; 4];
        let text: &str = ch.encode_utf8(&mut buf);
        self.layers.iter().any(|l| match l {
            Now::Table(t) => t.any_code(ch, pred),
            Now::View(v) => v.any_code(text, pred),
        })
    }

    fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| match l {
            Now::Table(t) => AuxCodeLookup::is_empty(*t),
            Now::View(v) => !v.has_any(),
        })
    }
}
