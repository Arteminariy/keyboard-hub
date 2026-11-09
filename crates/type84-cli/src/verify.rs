//! Compare a backup file with the board's current state.

use anyhow::Result;
use type84_proto::keymap::KeyAction;
use type84_proto::layout;
use type84_proto::rt::{AxisRecord, RtScale};

use crate::backup::{Backup, Block};
use crate::render;

fn key_name(i: usize) -> String {
    layout::label(i)
        .map(str::to_string)
        .unwrap_or_else(|| format!("(idx {i})"))
}

fn diff_bytes(old: &[u8], new: &[u8], out: &mut Vec<String>) {
    let diffs: Vec<usize> = (0..old.len().max(new.len()))
        .filter(|&i| old.get(i) != new.get(i))
        .collect();
    for &i in diffs.iter().take(32) {
        out.push(format!(
            "  byte {i}: backup {:02x?} -> board {:02x?}",
            old.get(i),
            new.get(i)
        ));
    }
    if diffs.len() > 32 {
        out.push(format!("  ... {} more differing bytes", diffs.len() - 32));
    }
}

fn diff_keymap(old: &[u8], new: &[u8], out: &mut Vec<String>) {
    for i in 0..128 {
        let (a, b) = (&old[i * 4..i * 4 + 4], &new[i * 4..i * 4 + 4]);
        if a != b {
            let da = KeyAction::decode([a[0], a[1], a[2], a[3]]).describe();
            let db = KeyAction::decode([b[0], b[1], b[2], b[3]]).describe();
            out.push(format!(
                "  {:<10} backup {} ({}) -> board {} ({})",
                key_name(i),
                hex::encode(a),
                da,
                hex::encode(b),
                db
            ));
        }
    }
}

fn diff_rt(old: &[u8], new: &[u8], old_scale: RtScale, new_scale: RtScale, out: &mut Vec<String>) {
    for i in 0..128 {
        let (a, b) = (&old[i * 8..i * 8 + 8], &new[i * 8..i * 8 + 8]);
        if a != b {
            let ra = AxisRecord::decode(a);
            let rb = AxisRecord::decode(b);
            let (ea, aa, pa, la) = render::key_row(&ra, old_scale);
            let (eb, ab, pb, lb) = render::key_row(&rb, new_scale);
            out.push(format!(
                "  {:<10} backup {} (act {aa}, RT {ea} {pa}/{la}, flags {:#04x}) -> board {} (act {ab}, RT {eb} {pb}/{lb}, flags {:#04x})",
                key_name(i),
                hex::encode(a),
                ra.flags,
                hex::encode(b),
                rb.flags
            ));
        }
    }
}

/// Returns the list of difference lines (empty = identical) and whether any
/// difference touches a block a restore would write.
pub fn compare(old: &Backup, new: &Backup) -> Result<(Vec<String>, bool)> {
    let mut lines = Vec::new();
    let mut restore_diff = false;

    let (oi, ni) = (&old.device.info, &new.device.info);
    if (oi.vendor_id, oi.product_id) != (ni.vendor_id, ni.product_id) {
        lines.push(format!(
            "WARNING: backup is from {:04x}:{:04x}, board is {:04x}:{:04x}",
            oi.vendor_id, oi.product_id, ni.vendor_id, ni.product_id
        ));
    }
    if oi.version_hundredths != ni.version_hundredths {
        lines.push(format!(
            "WARNING: firmware differs: backup {} vs board {}",
            oi.version_string(),
            ni.version_string()
        ));
    }

    let old_scale = RtScale::from_precision(oi.rt_precision);
    let new_scale = RtScale::from_precision(ni.rt_precision);
    for ((name, ob), (_, nb)) in old.blocks.named().into_iter().zip(new.blocks.named()) {
        let (a, b) = (ob.bytes()?, nb.bytes()?);
        if a == b {
            continue;
        }
        let mut out = Vec::new();
        match name {
            "keymap_layer0" | "keymap_layer1" | "default_fn_keymap" => {
                diff_keymap(&a, &b, &mut out)
            }
            "axis_rt" => diff_rt(&a, &b, old_scale, new_scale, &mut out),
            _ => diff_bytes(&a, &b, &mut out),
        }
        let tag = if ob.restore { "" } else { " (reference only)" };
        lines.push(format!("{name} ({}) differs{tag}:", ob.command));
        lines.extend(out);
        restore_diff |= ob.restore;
    }

    if old.macros.walk != new.macros.walk || !same_reads(&old.macros.reads, &new.macros.reads) {
        lines.push("macros differ:".into());
        lines.push(format!(
            "  backup: {} macros, board: {} macros",
            old.macros.walk.macros.len(),
            new.macros.walk.macros.len()
        ));
        restore_diff = true;
    }
    Ok((lines, restore_diff))
}

fn same_reads(a: &[Block], b: &[Block]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.addr == y.addr && x.data == y.data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Reader;
    use std::collections::{HashMap, VecDeque};
    use std::time::Duration;
    use type84_proto::frame::Frame;
    use type84_proto::transfer::{FrameIo, IoError};
    use type84_proto::ReadOp;

    /// Simulated board: one memory image per opcode, answered packet by packet.
    struct FakeBoard {
        images: HashMap<u8, Vec<u8>>,
        queue: VecDeque<Frame>,
    }

    impl FrameIo for FakeBoard {
        fn send_request(&mut self, f: &Frame) -> Result<(), IoError> {
            assert!(type84_proto::is_allowed_read_opcode(f[1]));
            let img = &self.images[&f[1]];
            let addr = u16::from_le_bytes([f[3], f[4]]) as usize;
            let mut r = *f;
            r[0] = 0x55;
            r[8..8 + f[2] as usize].copy_from_slice(&img[addr..addr + f[2] as usize]);
            self.queue.push_back(r);
            Ok(())
        }
        fn recv(&mut self, _t: Duration) -> Result<Option<Frame>, IoError> {
            Ok(self.queue.pop_front())
        }
    }

    fn board() -> FakeBoard {
        let mut images = HashMap::new();
        for op in ReadOp::ALL {
            images.insert(op.code(), vec![0u8; op.block_size()]);
        }
        let info =
            hex::decode("0000009245 0cd6801701000066010c110a0000000f".replace(' ', "")).unwrap();
        images.get_mut(&0x10).unwrap()[..info.len()].copy_from_slice(&info);
        let mut macro_img = vec![0u8; 912];
        macro_img[8..12].copy_from_slice(&400u32.to_le_bytes());
        macro_img[400..412].copy_from_slice(&[4, 0, 0, 0, 0, 0, 4, 0xB0, 20, 0, 4, 0x30]);
        images.insert(0x15, macro_img);
        let rt = images.get_mut(&0x17).unwrap();
        for i in 0..128 {
            rt[i * 8 + 2] = 150;
        }
        rt[34 * 8..34 * 8 + 8].copy_from_slice(&[0, 1, 80, 0, 30, 0, 30, 0]);
        FakeBoard {
            images,
            queue: VecDeque::new(),
        }
    }

    fn iface() -> type84_hid::Interface {
        type84_hid::Interface {
            path: std::ffi::CString::new("/dev/null").unwrap(),
            usage_page: 0xFF68,
            usage: 0x61,
            interface_number: 1,
            product: Some("test".into()),
            manufacturer: None,
            serial: None,
            release: 0,
        }
    }

    fn take(b: &mut FakeBoard) -> Backup {
        let snap = Reader::new(b, false).snapshot().unwrap();
        Backup::from_snapshot(&snap, &iface()).unwrap()
    }

    #[test]
    fn backup_roundtrip_and_diff() {
        let mut b = board();
        let first = take(&mut b);
        assert_eq!(first.device.firmware, "1.17");
        assert_eq!(first.macros.walk.macros.len(), 1);
        assert_eq!(first.macros.walk.macros[0].id, 2);
        assert_eq!(first.macros.reads.len(), 2);
        assert_eq!(first.blocks.axis_rt.frames.len(), 19);

        // JSON round trip keeps everything and passes validation.
        let json = serde_json::to_string(&first).unwrap();
        let loaded: Backup = serde_json::from_str(&json).unwrap();
        loaded.validate().unwrap();
        let (lines, diff) = compare(&loaded, &take(&mut b)).unwrap();
        assert!(lines.is_empty() && !diff, "{lines:?}");

        // Change W's actuation on the "board": verify must name the key.
        b.images.get_mut(&0x17).unwrap()[34 * 8 + 2] = 81;
        let (lines, diff) = compare(&loaded, &take(&mut b)).unwrap();
        assert!(diff);
        assert!(lines.iter().any(|l| l.contains("axis_rt")));
        assert!(
            lines
                .iter()
                .any(|l| l.trim_start().starts_with("W ") && l.contains("act 0.81")),
            "{lines:?}"
        );
    }
}
