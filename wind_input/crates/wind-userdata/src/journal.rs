//! 按设备的追加日志 `.wjl`（设计 §3.2）：一条记录一次 `write(2)`，无用户态缓冲；崩溃最多
//! 丢最后一条；尾部 CRC 不符处截断。永不 mmap；裁剪走重写 + rename。
//!
//! 文件头 40 字节：`"WJNL" | version u16 | reserved u16 | device_id [u8;16] | dir_id [u8;16]`。

use crate::record::Record;
use anyhow::{Context, bail};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"WJNL";
const HEADER_LEN: u64 = 40;

pub struct Journal {
    file: std::fs::File,
    path: std::path::PathBuf,
    next_seq: u64,
    /// 上次 fsync 之后有新写入（后台节拍据此每拍刷一次，§3.2）。
    dirty: bool,
}

#[cfg(test)]
thread_local! {
    /// 故障注入：下一次 `append` 的写入失败（模拟磁盘满 / 目录只读）。
    pub(crate) static FAIL_NEXT_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn header(device_id: [u8; 16], dir_id: [u8; 16]) -> [u8; 40] {
    let mut h = [0u8; 40];
    h[..4].copy_from_slice(MAGIC);
    h[4..6].copy_from_slice(&1u16.to_le_bytes());
    h[8..24].copy_from_slice(&device_id);
    h[24..40].copy_from_slice(&dir_id);
    h
}

/// 顺序扫描：`(seq > from_seq_exclusive 的完整记录, 最后一条完整记录的结束偏移, 其 seq)`。
/// 坏帧（不完整 / CRC 不符）处停，后面的一概不要——那是崩溃留下的尾巴。
fn scan(
    path: &Path,
    from_seq_exclusive: u64,
    collect: bool,
) -> anyhow::Result<(Vec<Record>, u64, u64)> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < HEADER_LEN as usize || &bytes[..4] != MAGIC {
        bail!("不是 .wjl 日志: {}", path.display());
    }
    let mut pos = HEADER_LEN as usize;
    let mut out = Vec::new();
    let mut last_seq = 0u64;
    while pos < bytes.len() {
        let Some(flen) = Record::frame_len(&bytes[pos..]) else {
            break;
        };
        if pos + flen > bytes.len() {
            break;
        }
        let Ok(r) = Record::decode(&bytes[pos..pos + flen]) else {
            break;
        };
        last_seq = r.seq;
        if collect && r.seq > from_seq_exclusive {
            out.push(r);
        }
        pos += flen;
    }
    Ok((out, pos as u64, last_seq))
}

impl Journal {
    /// 不存在则新建并写头；存在则校验头、截掉坏尾巴、从最后一条完整记录之后续号。
    pub fn open(path: &Path, device_id: [u8; 16], dir_id: [u8; 16]) -> anyhow::Result<Journal> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let fresh = !path.is_file() || std::fs::metadata(path)?.len() == 0;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let next_seq = if fresh {
            file.write_all(&header(device_id, dir_id))?;
            file.sync_data()?;
            1
        } else {
            let (_, end, last) = scan(path, 0, false)?;
            file.set_len(end)?;
            file.seek(SeekFrom::End(0))?;
            last + 1
        };
        Ok(Journal {
            file,
            path: path.to_path_buf(),
            next_seq,
            dirty: false,
        })
    }

    /// 一条一次 write。`rec.seq` 必须等于 [`Self::next_seq`]（调用方按序分配）。
    pub fn append(&mut self, rec: &Record) -> anyhow::Result<()> {
        if rec.seq != self.next_seq {
            bail!("seq 不连续: 期望 {} 实际 {}", self.next_seq, rec.seq);
        }
        rec.check_lens()?;
        let frame = rec.encode();
        #[cfg(test)]
        if FAIL_NEXT_APPEND.with(|f| f.replace(false)) {
            return Err(std::io::Error::other("注入的写失败")).context("写日志");
        }
        self.file.write_all(&frame).context("写日志")?;
        self.next_seq += 1;
        self.dirty = true;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 有新写入 ⇒ 克隆一个句柄交给调用方在锁外 fdatasync，并清 dirty（刷失败用 [`Self::mark_dirty`]
    /// 放回）。没有新写入 ⇒ `None`。fsync 不在日志锁里做：机械盘上它要几十毫秒，按键路径的追加
    /// 不该陪等（审查 I-5）。
    pub fn take_sync_handle(&mut self) -> anyhow::Result<Option<std::fs::File>> {
        if !self.dirty {
            return Ok(None);
        }
        let h = self.file.try_clone().context("克隆日志句柄")?;
        self.dirty = false;
        Ok(Some(h))
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 有新写入就 fdatasync 一次（§3.2：断电最多丢最近一个节拍）。返回是否真的刷了。
    pub fn sync_if_dirty(&mut self) -> anyhow::Result<bool> {
        if !self.dirty {
            return Ok(false);
        }
        self.file.sync_data().context("刷日志")?;
        self.dirty = false;
        Ok(true)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// 只能往前拨（§3.6：next_seq 取 state 与日志尾两者较大者）；往回拨会让新记录的 seq
    /// 落在已折回水位之下而被跳过。
    pub fn bump_next_seq(&mut self, at_least: u64) {
        if at_least > self.next_seq {
            self.next_seq = at_least;
        }
    }

    pub fn replay(path: &Path, from_seq_exclusive: u64) -> anyhow::Result<Vec<Record>> {
        Ok(scan(path, from_seq_exclusive, true)?.0)
    }

    /// 最后一条完整记录的 seq；空日志 0。
    pub fn tail_seq(path: &Path) -> anyhow::Result<u64> {
        Ok(scan(path, 0, false)?.2)
    }

    /// 裁掉 `seq < keep_from_seq` 的头部：重写到 tmp 再 rename（永不原地截断，见模块注释），
    /// 然后把自己的 fd 换到新文件上。
    pub fn prune_before(&mut self, keep_from_seq: u64) -> anyhow::Result<()> {
        let path = self.path.as_path();
        let bytes = std::fs::read(path)?;
        if bytes.len() < HEADER_LEN as usize {
            return Ok(());
        }
        let (recs, _, _) = scan(path, keep_from_seq.saturating_sub(1), true)?;
        let mut out = Vec::with_capacity(bytes.len());
        out.extend_from_slice(&bytes[..HEADER_LEN as usize]);
        for r in &recs {
            out.extend_from_slice(&r.encode());
        }
        let tmp = path.with_extension("wjl.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_data()?;
        }
        std::fs::rename(&tmp, path)?;
        // rename 换了 inode：旧 fd 还指着被替换掉的文件，续写必须改指新文件。
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        file.seek(SeekFrom::End(0))?;
        self.file = file;
        // 新文件已整份 fsync
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_jnl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }
    fn rec(seq: u64, code: &str) -> Record {
        Record {
            seq,
            ts_ms: 1,
            kind: OpKind::Upsert,
            target: Target::Temp,
            schema: "s".into(),
            code: code.into(),
            text: "t".into(),
            payload: Payload::Weight(1),
            boundary: 0,
        }
    }

    /// 审查 I-5：取刷盘句柄即清 dirty（锁外刷）；刷失败可放回。
    #[test]
    fn sync_handle_is_taken_once_and_can_be_returned() {
        let p = tmp("handle.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert!(j.take_sync_handle().unwrap().is_none());
        j.append(&rec(j.next_seq(), "c")).unwrap();
        let h = j.take_sync_handle().unwrap().expect("有新写入");
        assert!(!j.is_dirty());
        h.sync_data().unwrap();
        j.mark_dirty();
        assert!(j.is_dirty());
    }

    #[test]
    fn sync_if_dirty_only_after_append() {
        let p = tmp("dirty.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert!(!j.is_dirty());
        assert!(!j.sync_if_dirty().unwrap(), "没有新写入不 fsync");
        j.append(&rec(j.next_seq(), "c")).unwrap();
        assert!(j.is_dirty());
        assert!(j.sync_if_dirty().unwrap());
        assert!(!j.is_dirty());
        assert!(!j.sync_if_dirty().unwrap());
    }

    #[test]
    fn append_then_replay_from_watermark() {
        let p = tmp("a.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=5 {
            j.append(&rec(i, "c")).unwrap();
        }
        assert_eq!(j.next_seq(), 6);
        let got = Journal::replay(&p, 3).unwrap();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![4, 5]);
        assert_eq!(Journal::tail_seq(&p).unwrap(), 5);
        drop(j);
        let j2 = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j2.next_seq(), 6, "重开续号");
    }

    /// Review Focus 1：尾部半条记录（断电 / SIGKILL）⇒ 截到最后一条完整记录，不 panic。
    #[test]
    fn truncated_tail_stops_at_last_complete_record() {
        let p = tmp("t.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=3 {
            j.append(&rec(i, "c")).unwrap();
        }
        drop(j);
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.truncate(bytes.len() - 5);
        std::fs::write(&p, &bytes).unwrap();
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(Journal::tail_seq(&p).unwrap(), 2);
        // 重开后在截断点续写，新记录可读
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j.next_seq(), 3);
        j.append(&rec(3, "d")).unwrap();
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[2].code, "d");
    }

    #[test]
    fn prune_rewrites_without_old_head_and_keeps_header() {
        let p = tmp("p.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=6 {
            j.append(&rec(i, "c")).unwrap();
        }
        j.prune_before(4).unwrap();
        drop(j);
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![4, 5, 6]);
        let j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j.next_seq(), 7);
        assert!(!p.with_extension("wjl.tmp").exists(), "tmp 不残留");
    }

    #[test]
    fn bump_next_seq_only_moves_forward() {
        let p = tmp("bump.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        j.bump_next_seq(10);
        assert_eq!(j.next_seq(), 10);
        j.bump_next_seq(3);
        assert_eq!(j.next_seq(), 10, "不往回拨");
        j.append(&rec(10, "c")).unwrap();
        assert_eq!(Journal::tail_seq(&p).unwrap(), 10);
    }

    /// 字段长度编码是 u16：超长的必须在写盘前拒掉，否则写成一条合法 CRC 却解不开的帧，
    /// 下次 open 从它截断，之后的记录全丢。
    #[test]
    fn oversized_field_is_refused_before_it_reaches_disk() {
        let p = tmp("big.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        let mut big = rec(1, "c");
        big.text = "字".repeat(30_000);
        assert!(j.append(&big).is_err(), "超过 u16 的字段要拒");
        assert_eq!(j.next_seq(), 1, "拒掉的不占号");
        j.append(&rec(1, "a")).unwrap();
        j.append(&rec(2, "b")).unwrap();
        drop(j);
        let j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert_eq!(j.next_seq(), 3);
        assert_eq!(Journal::replay(&p, 0).unwrap().len(), 2);
    }

    /// 裁剪走 rename：已打开的 Journal 若还拿着旧 fd，后续追加会写进被替换掉的旧 inode。
    #[test]
    fn append_after_prune_lands_in_the_pruned_file() {
        let p = tmp("pa.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        for i in 1..=4 {
            j.append(&rec(i, "c")).unwrap();
        }
        j.prune_before(3).unwrap();
        j.append(&rec(5, "d")).unwrap();
        let got = Journal::replay(&p, 0).unwrap();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn wrong_header_is_rejected_and_out_of_order_seq_is_refused() {
        let p = tmp("h.wjl");
        std::fs::write(&p, b"NOPE").unwrap();
        assert!(Journal::open(&p, [1; 16], [2; 16]).is_err());
        let p = tmp("seq.wjl");
        let mut j = Journal::open(&p, [1; 16], [2; 16]).unwrap();
        assert!(j.append(&rec(5, "c")).is_err(), "seq 必须连续");
        assert_eq!(j.next_seq(), 1);
    }
}
