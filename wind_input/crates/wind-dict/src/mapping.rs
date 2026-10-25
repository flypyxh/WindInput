//! 只读数据文件映射的统一入口。
//!
//! 平时就是 `Mmap::map`：词库页按需从磁盘换入，常驻内存小，代价是某个编码第一次被查、
//! 或其页被系统换出后再查时要读盘。
//!
//! 性能测试模式（服务启动时由 `perf_preload` 标记文件打开，见 service 的 `main.rs`）下，
//! 映射后整份拷进进程私有内存再返回，类型仍是 [`Mmap`]，调用点无需区分。用来排除
//! 「mmap 读盘」这个因素，不考虑内存占用。

use memmap2::{Mmap, MmapMut};
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static PRELOAD: AtomicBool = AtomicBool::new(false);
static PRELOADED_BYTES: AtomicU64 = AtomicU64::new(0);

/// 打开 / 关闭性能测试模式。须在加载任何词库之前调用，之后映射的文件才生效。
pub fn set_preload(on: bool) {
    PRELOAD.store(on, Ordering::Relaxed);
}

/// 性能测试模式下已拷进内存的总字节数。
pub fn preloaded_bytes() -> u64 {
    PRELOADED_BYTES.load(Ordering::Relaxed)
}

/// 只读映射整个文件；性能测试模式下改为整份拷进进程私有内存。
pub fn map_file(file: &File) -> std::io::Result<Mmap> {
    let mapped = unsafe { Mmap::map(file)? };
    if !PRELOAD.load(Ordering::Relaxed) || mapped.is_empty() {
        return Ok(mapped);
    }
    to_private(&mapped)
}

/// 拷进匿名内存（`map_anon` 不接受长度 0，调用方已排除）。
fn to_private(src: &[u8]) -> std::io::Result<Mmap> {
    let mut anon = MmapMut::map_anon(src.len())?;
    anon.copy_from_slice(src);
    let total = PRELOADED_BYTES.fetch_add(src.len() as u64, Ordering::Relaxed) + src.len() as u64;
    tracing::debug!(
        "性能测试模式：载入 {} KB，累计 {} MB",
        src.len() / 1024,
        total / (1024 * 1024)
    );
    anon.make_read_only()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_copy_keeps_content() {
        let data = b"wind dict bytes".to_vec();
        let m = to_private(&data).unwrap();
        assert_eq!(&m[..], &data[..]);
    }
}
