//! 文本真源层（设计 §2.2 / §2.3 / §3.5 文本目标）。
pub mod codec;
pub mod doc;
pub mod fold;
pub mod format;
pub mod patch;
pub mod table;

pub use format::{ParseError, TextDict, TextEntry, WindHeader, normalize_key, parse};
pub use patch::{PatchOp, patch, render};
