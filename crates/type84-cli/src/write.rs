//! Helpers for the guarded Hall/RT write: the automatic pre-write backup
//! and the human-readable per-key diff. The write itself happens only in
//! `type84_hid::WriteSession::apply_hall_table`.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use type84_proto::layout;
use type84_proto::rt::{AxisRecord, RtScale, FLAG_RAMPAGE, FLAG_WHOLE_TRAVEL, RECORDS, RECORD_LEN};
use type84_proto::transfer::{FrameIo, IoError};

use crate::backup::Backup;
use crate::snapshot::Reader;

/// Where automatic pre-write backups go (relative to the working directory,
/// like `type84 backup`).
pub fn auto_backup_dir() -> PathBuf {
    PathBuf::from("backups").join("auto")
}

/// Save `b` under `dir` with a fresh, never-reused name. Never overwrites.
pub fn save_backup_new(b: &Backup, dir: &Path, tag: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let json = serde_json::to_string_pretty(b)?;
    for n in 0..100 {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let path = dir.join(format!(
            "type84-{ts}-fw{}-{tag}{suffix}.json",
            b.device.firmware
        ));
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        };
        f.write_all(json.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        Backup::load(&path).with_context(|| format!("re-reading {}", path.display()))?;
        return Ok(path);
    }
    anyhow::bail!(
        "could not find a free backup file name in {}",
        dir.display()
    )
}

/// The pre-write backup step for `WriteSession::apply_hall_table`: full
/// read through the session's read-only view, saved under `dir`.
pub fn auto_backup<'a>(
    iface: &'a type84_hid::Interface,
    dir: &'a Path,
    verbose: bool,
) -> impl FnMut(&mut dyn FrameIo) -> Result<PathBuf, IoError> + 'a {
    move |io: &mut dyn FrameIo| {
        let mut run = || -> Result<PathBuf> {
            let snap = Reader::new(&mut *io, verbose).snapshot()?;
            let b = Backup::from_snapshot(&snap, iface)?;
            save_backup_new(&b, dir, "prewrite")
        };
        run().map_err(IoError::from)
    }
}

/// One Hall record in words.
pub fn describe(r: &AxisRecord, s: RtScale) -> String {
    let rt = if r.rt_enabled() {
        format!("RT {:.2}/{:.2}", r.press_mm(s), r.release_mm(s))
    } else {
        "RT off".to_string()
    };
    let mut out = format!(
        "{:.2} mm, {rt}, whole travel {}",
        r.actuation_mm(s),
        if r.whole_travel() { "on" } else { "off" }
    );
    if r.flags & !(FLAG_WHOLE_TRAVEL | FLAG_RAMPAGE) != 0 || r.rampage() || r.axis_type != 0 {
        out.push_str(&format!(" [flags {:#04x} axis {}]", r.flags, r.axis_type));
    }
    out
}

/// Per-key diff between two Hall blocks.
#[derive(Debug, Clone, Default)]
pub struct HallDiff {
    /// `label: old → new`, physical keys only, in layout order.
    pub lines: Vec<String>,
    pub physical_changed: usize,
    pub non_physical_changed: Vec<usize>,
}

pub fn hall_diff(old: &[u8], new: &[u8], s: RtScale) -> HallDiff {
    let rec = |b: &[u8], i: usize| AxisRecord::decode(&b[i * RECORD_LEN..(i + 1) * RECORD_LEN]);
    let mut d = HallDiff::default();
    for k in layout::keys() {
        let i = k.index as usize;
        let (a, b) = (rec(old, i), rec(new, i));
        if a != b {
            d.physical_changed += 1;
            d.lines.push(format!(
                "  {:<10} {} → {}",
                format!("{}:", k.label),
                describe(&a, s),
                describe(&b, s)
            ));
        }
    }
    d.non_physical_changed = (0..RECORDS)
        .filter(|&i| !layout::is_physical(i) && rec(old, i) != rec(new, i))
        .collect();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_names_keys() {
        let mut old = vec![0u8; 1024];
        for i in 0..128 {
            old[i * 8 + 2] = 120;
        }
        let mut new = old.clone();
        new[34 * 8..35 * 8].copy_from_slice(&[0, 1, 80, 0, 30, 0, 30, 0]);
        new[29 * 8 + 2] = 200;
        let d = hall_diff(&old, &new, RtScale::from_precision(0));
        assert_eq!(d.physical_changed, 1);
        assert_eq!(d.non_physical_changed, vec![29]);
        assert_eq!(d.lines.len(), 1);
        assert!(d.lines[0].contains("W:"), "{:?}", d.lines);
        assert!(
            d.lines[0].contains(
                "1.20 mm, RT off, whole travel off → 0.80 mm, RT 0.30/0.30, whole travel on"
            ),
            "{:?}",
            d.lines
        );
    }

    /// Minimal simulated board answering every read opcode from memory.
    struct FakeBoard {
        images: std::collections::HashMap<u8, Vec<u8>>,
        queue: std::collections::VecDeque<type84_proto::Frame>,
    }

    impl FrameIo for FakeBoard {
        fn send_request(&mut self, f: &type84_proto::Frame) -> Result<(), IoError> {
            assert!(
                type84_proto::is_allowed_read_opcode(f[1]),
                "backup must only read"
            );
            let img = &self.images[&f[1]];
            let addr = u16::from_le_bytes([f[3], f[4]]) as usize;
            let mut r = *f;
            r[0] = 0x55;
            r[8..8 + f[2] as usize].copy_from_slice(&img[addr..addr + f[2] as usize]);
            self.queue.push_back(r);
            Ok(())
        }
        fn recv(
            &mut self,
            _t: std::time::Duration,
        ) -> Result<Option<type84_proto::Frame>, IoError> {
            Ok(self.queue.pop_front())
        }
    }

    #[test]
    fn auto_backup_writes_fresh_files_only() {
        let mut images = std::collections::HashMap::new();
        for op in type84_proto::ReadOp::ALL {
            images.insert(op.code(), vec![0u8; op.block_size()]);
        }
        let info = hex::decode("00000092450cd6801701000066010c110a0000000f").unwrap();
        images.get_mut(&0x10).unwrap()[..info.len()].copy_from_slice(&info);
        let mut board = FakeBoard {
            images,
            queue: Default::default(),
        };
        let iface = type84_hid::Interface {
            path: std::ffi::CString::new("/dev/null").unwrap(),
            usage_page: 0xFF68,
            usage: 0x61,
            interface_number: 2,
            product: None,
            manufacturer: None,
            serial: None,
            release: 0,
        };
        let dir =
            std::env::temp_dir().join(format!("type84-auto-backup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut step = auto_backup(&iface, &dir, false);
        let a = step(&mut board).unwrap();
        let b = step(&mut board).unwrap();
        assert_ne!(a, b, "a second backup never reuses the first file name");
        assert!(a
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("prewrite"));
        Backup::load(&a).unwrap();
        Backup::load(&b).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
