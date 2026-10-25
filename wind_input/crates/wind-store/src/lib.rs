//! wind-store：用户数据门面。真源是用户目录 `data/` 下的文本文件（引擎文本目标、小表、
//! 机写小文件）；redb 只供旧库迁移读。

pub mod abbrev_index;
pub mod assoc_history;
mod banner_backfill;
pub mod charsets;
pub mod common_chars;
pub mod completion;
pub mod dict_export;
pub mod draft_words;
pub mod external_edit;
pub mod freq;
pub mod import_formats;
mod last_good;
mod learned_text;
pub mod legacy_retire;
mod machine_file;
pub mod migrate_p1;
pub mod migrate_p2;
pub mod migrate_p3;
pub mod migrate_p4;
pub mod migration;
pub mod phrase_text;
pub mod phrases;
pub mod quick_format;
pub mod shadow;
pub mod stat_collector;
pub mod stats;
pub mod stats_report;
pub mod store;
pub mod store_layer;
pub mod temp_words;
pub mod user_dicts;
pub mod user_words;
pub use wind_userdata::engine::check_schema_id;
pub use wind_userdata::issues;
pub use wind_userdata::text::banner;
pub use wind_wdict as wdict;

pub use store::Store;

/// 用户目录下装用户数据文件的子目录名（设计 §2.1，P5 起；此前叫 `user`）。
pub const DATA_DIR_NAME: &str = "data";
/// 用户误读表：数据目录根下的文件名（引擎读它、`data.locate` 认它，都按这个名字）。
pub const MISREAD_FILE: &str = "pinyin_misread.yaml";
pub use store_layer::{StoreDraftLayer, StoreTempLayer, StoreUserLayer};
