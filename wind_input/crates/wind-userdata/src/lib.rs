//! 用户数据引擎：快照 + 日志 + 增量表（设计 `docs/design/user-data-storage-redesign.md` §3）。
//!
//! 三种物理形态各管一类变化速度：不可变快照段（mmap）、按设备的追加日志（崩溃恢复与同步源）、
//! 内存增量表（未折回的操作）。文本层是同一台引擎多一步「按行补丁写回」（P2）。
pub mod delta;
pub mod device;
pub mod engine;
pub mod issues;
pub mod journal;
pub mod layer;
pub mod record;
pub mod runtime;
pub mod snapshot;
pub mod state;
pub mod text;
