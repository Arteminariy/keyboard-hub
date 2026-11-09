//! Hall/RT table write encoding: SET_MAGNETIC_AXIS_RT (0x27), 1024 bytes
//! (PROTOCOL.md §3.4 record layout, §4.3 write procedure).
//!
//! Hardware status: the encoding follows the vendor hub byte for byte and
//! its packet headers equal the headers the board echoes on reads, but no
//! write has been performed on hardware yet (UNVERIFIED).
//!
//! Policy:
//! - Writes always send the whole 1024-byte block (read-modify-write).
//! - A typed per-key patch may only touch the 84 physical keys. It keeps
//!   `axis_type`, flag bits 1..7 and every non-physical record exactly as
//!   read from the board.
//! - A raw block restored from a backup may also change non-physical
//!   records (the case's actuation switch rewrites them, PROTOCOL.md §9.1).
//! - Physical keys must hold vendor-UI-valid values: actuation 0.10–3.20 mm,
//!   RT sensitivities 0.01–3.20 mm each, or both 0 (RT off).

use serde::{Deserialize, Serialize};

use crate::frame::{Frame, HEADER_LEN, REQUEST_MARK};
use crate::layout;
use crate::opcode::WriteOp;
use crate::rt::{mm_to_raw, AxisRecord, RtScale, FLAG_WHOLE_TRAVEL, RECORDS, RECORD_LEN, SIZE};
use crate::transfer::{plan_chunks, write_frames};

pub const ACTUATION_MIN_MM: f64 = 0.10;
pub const ACTUATION_MAX_MM: f64 = 3.20;
pub const SENSITIVITY_MIN_MM: f64 = 0.01;
pub const SENSITIVITY_MAX_MM: f64 = 3.20;

/// Number of 0x27 frames one full-table write takes (18 × 56 + 1 × 16).
pub const WRITE_FRAME_COUNT: usize = 19;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HallError {
    #[error("Hall block must be {SIZE} bytes, got {0}")]
    Size(usize),
    #[error("key index {0} is not one of the 84 physical keys")]
    NotPhysical(usize),
    #[error("key index {0} is listed more than once")]
    Duplicate(usize),
    #[error("{label} (index {index}): actuation raw {raw} ({mm:.3} mm) is outside 0.10–3.20 mm")]
    Actuation {
        index: usize,
        label: String,
        raw: u16,
        mm: f64,
    },
    #[error("{label} (index {index}): RT {which} raw {raw} ({mm:.3} mm) is outside 0.01–3.20 mm")]
    Sensitivity {
        index: usize,
        label: String,
        which: &'static str,
        raw: u16,
        mm: f64,
    },
    #[error("{label} (index {index}): RT press and release must both be 0 (off) or both non-zero")]
    HalfRt { index: usize, label: String },
    #[error("{label} (index {index}): value is not a finite number")]
    NotFinite { index: usize, label: String },
    #[error("non-physical record {0} would change; a per-key patch must write it back as read")]
    NonPhysicalChanged(usize),
    #[error("malformed write frames: {0}")]
    Frames(String),
}

/// RT sensitivities in millimetres. Present = RT on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RtSensitivity {
    pub press_mm: f64,
    pub release_mm: f64,
}

/// Typed per-key setting, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KeyPatch {
    pub actuation_mm: f64,
    /// `None` = Rapid Trigger off (both sensitivities written as 0).
    pub rt: Option<RtSensitivity>,
    /// Flags bit0, UI «на всём ходе» / "whole travel".
    pub whole_travel: bool,
}

/// What may happen to the 44 non-physical records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonPhysicalPolicy {
    /// They must be written back exactly as read (per-key patches, presets).
    PreserveAsRead,
    /// They may take any value (restoring a full raw block from a backup).
    AllowFromBackup,
}

fn label_of(index: usize) -> String {
    layout::label(index)
        .map(str::to_string)
        .unwrap_or_else(|| format!("idx {index}"))
}

/// The whole per-key Hall table: 128 records indexed by key index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HallTable {
    pub records: Vec<AxisRecord>,
}

impl HallTable {
    pub fn from_block(block: &[u8]) -> Result<Self, HallError> {
        if block.len() != SIZE {
            return Err(HallError::Size(block.len()));
        }
        Ok(HallTable {
            records: block
                .chunks_exact(RECORD_LEN)
                .map(AxisRecord::decode)
                .collect(),
        })
    }

    pub fn to_block(&self) -> [u8; SIZE] {
        let mut out = [0u8; SIZE];
        for (i, r) in self.records.iter().enumerate().take(RECORDS) {
            out[i * RECORD_LEN..(i + 1) * RECORD_LEN].copy_from_slice(&r.encode());
        }
        out
    }

    /// Indices whose 8-byte record differs.
    pub fn changed_indices(&self, other: &HallTable) -> Vec<usize> {
        (0..RECORDS)
            .filter(|&i| self.records[i] != other.records[i])
            .collect()
    }

    /// Typed view of one key.
    pub fn key_patch(&self, index: usize, scale: RtScale) -> KeyPatch {
        let r = &self.records[index];
        KeyPatch {
            actuation_mm: r.actuation_mm(scale),
            rt: r.rt_enabled().then(|| RtSensitivity {
                press_mm: r.press_mm(scale),
                release_mm: r.release_mm(scale),
            }),
            whole_travel: r.whole_travel(),
        }
    }

    /// Apply typed per-key settings to this table (the fresh read of the
    /// board). Only physical keys may be listed. `axis_type`, flag bits 1..7
    /// and all unlisted records are kept. The result is validated with
    /// [`NonPhysicalPolicy::PreserveAsRead`].
    pub fn apply_patch(
        &self,
        patch: &[(u8, KeyPatch)],
        scale: RtScale,
    ) -> Result<HallTable, HallError> {
        let mut out = self.clone();
        let mut seen = [false; RECORDS];
        for &(index, p) in patch {
            let i = index as usize;
            if i >= RECORDS || !layout::is_physical(i) {
                return Err(HallError::NotPhysical(i));
            }
            if std::mem::replace(&mut seen[i], true) {
                return Err(HallError::Duplicate(i));
            }
            let finite = p.actuation_mm.is_finite()
                && p.rt
                    .is_none_or(|r| r.press_mm.is_finite() && r.release_mm.is_finite());
            if !finite {
                return Err(HallError::NotFinite {
                    index: i,
                    label: label_of(i),
                });
            }
            let r = &mut out.records[i];
            r.actuation_raw = mm_to_raw(p.actuation_mm.max(0.0), scale.trigger);
            match p.rt {
                Some(s) => {
                    r.press_raw = mm_to_raw(s.press_mm.max(0.0), scale.sensitivity);
                    r.release_raw = mm_to_raw(s.release_mm.max(0.0), scale.sensitivity);
                    // RT on with a value that rounds to 0 would silently turn RT off.
                    for (which, raw, mm) in [
                        ("press", r.press_raw, s.press_mm),
                        ("release", r.release_raw, s.release_mm),
                    ] {
                        if raw == 0 {
                            return Err(HallError::Sensitivity {
                                index: i,
                                label: label_of(i),
                                which,
                                raw,
                                mm,
                            });
                        }
                    }
                }
                None => {
                    r.press_raw = 0;
                    r.release_raw = 0;
                }
            }
            r.flags =
                (r.flags & !FLAG_WHOLE_TRAVEL) | if p.whole_travel { FLAG_WHOLE_TRAVEL } else { 0 };
        }
        validate_write(self, &out, scale, NonPhysicalPolicy::PreserveAsRead)?;
        Ok(out)
    }
}

/// Check a table before it is written. `base` is the fresh read of the
/// board. Range checks are done on raw values (after rounding).
pub fn validate_write(
    base: &HallTable,
    new: &HallTable,
    scale: RtScale,
    policy: NonPhysicalPolicy,
) -> Result<(), HallError> {
    if base.records.len() != RECORDS || new.records.len() != RECORDS {
        return Err(HallError::Size(new.records.len() * RECORD_LEN));
    }
    let act = (
        mm_to_raw(ACTUATION_MIN_MM, scale.trigger),
        mm_to_raw(ACTUATION_MAX_MM, scale.trigger),
    );
    let sens = (
        mm_to_raw(SENSITIVITY_MIN_MM, scale.sensitivity),
        mm_to_raw(SENSITIVITY_MAX_MM, scale.sensitivity),
    );
    for i in 0..RECORDS {
        let r = &new.records[i];
        if !layout::is_physical(i) {
            if policy == NonPhysicalPolicy::PreserveAsRead && *r != base.records[i] {
                return Err(HallError::NonPhysicalChanged(i));
            }
            continue;
        }
        if r.actuation_raw < act.0 || r.actuation_raw > act.1 {
            return Err(HallError::Actuation {
                index: i,
                label: label_of(i),
                raw: r.actuation_raw,
                mm: r.actuation_mm(scale),
            });
        }
        match (r.press_raw, r.release_raw) {
            (0, 0) => {}
            (0, _) | (_, 0) => {
                return Err(HallError::HalfRt {
                    index: i,
                    label: label_of(i),
                })
            }
            (p, q) => {
                for (which, raw) in [("press", p), ("release", q)] {
                    if raw < sens.0 || raw > sens.1 {
                        return Err(HallError::Sensitivity {
                            index: i,
                            label: label_of(i),
                            which,
                            raw,
                            mm: raw as f64 / scale.sensitivity as f64,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// The exact 0x27 frames that write `table`: 19 packets, `len` = 56
/// (last one 16), `addr` = byte offset, byte 6 = 1 on the last packet.
pub fn encode_write_frames(table: &HallTable) -> Vec<Frame> {
    write_frames(WriteOp::AxisRt, 0, &table.to_block()).expect("1024-byte block always plans")
}

/// Inverse of [`encode_write_frames`]; checks every header strictly.
pub fn decode_write_frames(frames: &[Frame]) -> Result<HallTable, HallError> {
    let plan = plan_chunks(0, SIZE).expect("1024-byte block always plans");
    if frames.len() != plan.len() {
        return Err(HallError::Frames(format!(
            "{} frames, expected {}",
            frames.len(),
            plan.len()
        )));
    }
    let mut block = Vec::with_capacity(SIZE);
    for (i, (f, c)) in frames.iter().zip(&plan).enumerate() {
        let addr = u16::from_le_bytes([f[3], f[4]]);
        let header_ok = f[0] == REQUEST_MARK
            && f[1] == WriteOp::AxisRt.code()
            && f[2] == c.len
            && addr == c.addr
            && f[5] == 0
            && f[6] == c.last as u8
            && f[7] == 0;
        if !header_ok {
            return Err(HallError::Frames(format!(
                "frame {i} header {:02x?}",
                &f[..HEADER_LEN]
            )));
        }
        let end = HEADER_LEN + c.len as usize;
        if f[end..].iter().any(|&b| b != 0) {
            return Err(HallError::Frames(format!("frame {i} has data past len")));
        }
        block.extend_from_slice(&f[HEADER_LEN..end]);
    }
    HallTable::from_block(&block)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = include_str!("../tests/fixtures/type84-20261109-220355-fw1.17.json");
    const SWITCH_LEFT: &str = include_str!("../tests/fixtures/switch-left-1.20mm.json");

    fn axis_rt(json: &str) -> (Vec<u8>, Vec<Vec<u8>>) {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let b = &v["blocks"]["axis_rt"];
        let data = hex::decode(b["data"].as_str().unwrap()).unwrap();
        let frames = b["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| hex::decode(f.as_str().unwrap()).unwrap())
            .collect();
        (data, frames)
    }

    const S: RtScale = RtScale {
        trigger: 100,
        sensitivity: 100,
    };

    #[test]
    fn backup_table_encodes_to_its_raw_block() {
        let (data, read_frames) = axis_rt(TARGET);
        let table = HallTable::from_block(&data).unwrap();
        let frames = encode_write_frames(&table);
        assert_eq!(frames.len(), WRITE_FRAME_COUNT);
        let payload: Vec<u8> = frames
            .iter()
            .flat_map(|f| f[8..8 + f[2] as usize].to_vec())
            .collect();
        assert_eq!(
            payload, data,
            "frame payloads must equal the backup's raw Hall block"
        );
        // Each write frame equals the response frame the board sent for the
        // same chunk on read, except the mark (AA/55) and opcode (27/17):
        // this ties chunking, addr, len and the last-packet flag to hardware.
        assert_eq!(read_frames.len(), WRITE_FRAME_COUNT);
        for (i, (w, r)) in frames.iter().zip(&read_frames).enumerate() {
            assert_eq!((w[0], w[1]), (0xAA, 0x27));
            assert_eq!((r[0], r[1]), (0x55, 0x17));
            assert_eq!(&w[2..], &r[2..], "frame {i}");
        }
        assert_eq!(
            &frames[18][..8],
            &[0xAA, 0x27, 0x10, 0xF0, 0x03, 0x00, 0x01, 0x00]
        );
    }

    #[test]
    fn roundtrip_decode_encode() {
        for json in [TARGET, SWITCH_LEFT] {
            let (data, _) = axis_rt(json);
            let t = HallTable::from_block(&data).unwrap();
            assert_eq!(decode_write_frames(&encode_write_frames(&t)).unwrap(), t);
            assert_eq!(t.to_block().to_vec(), data);
        }
        // Arbitrary bytes too.
        let data: Vec<u8> = (0..1024).map(|i| (i * 7 % 256) as u8).collect();
        let t = HallTable::from_block(&data).unwrap();
        assert_eq!(decode_write_frames(&encode_write_frames(&t)).unwrap(), t);
        let mut bad = encode_write_frames(&t);
        bad[18][6] = 0;
        assert!(decode_write_frames(&bad).is_err());
        let mut bad = encode_write_frames(&t);
        bad[18][8 + 16] = 1;
        assert!(decode_write_frames(&bad).is_err());
    }

    fn target_patch() -> Vec<(u8, KeyPatch)> {
        let (data, _) = axis_rt(TARGET);
        let t = HallTable::from_block(&data).unwrap();
        layout::keys()
            .iter()
            .map(|k| (k.index, t.key_patch(k.index as usize, S)))
            .collect()
    }

    #[test]
    fn patch_from_backup_turns_switch_state_into_target_physical_records() {
        let (target, _) = axis_rt(TARGET);
        let (left, _) = axis_rt(SWITCH_LEFT);
        let base = HallTable::from_block(&left).unwrap();
        let out = base.apply_patch(&target_patch(), S).unwrap();
        let tgt = HallTable::from_block(&target).unwrap();
        for i in 0..RECORDS {
            if layout::is_physical(i) {
                assert_eq!(out.records[i], tgt.records[i], "physical {i}");
            } else {
                assert_eq!(
                    out.records[i], base.records[i],
                    "non-physical {i} preserved as read"
                );
            }
        }
        // Applying the target to itself is the identity.
        assert_eq!(tgt.apply_patch(&target_patch(), S).unwrap(), tgt);
        let w = &out.records[34];
        assert_eq!(w.encode(), [0x00, 0x01, 0x50, 0x00, 0x1E, 0x00, 0x1E, 0x00]);
    }

    #[test]
    fn patch_preserves_axis_type_and_unknown_flags() {
        let mut base = HallTable::from_block(&[0u8; SIZE]).unwrap();
        for r in base.records.iter_mut() {
            r.actuation_raw = 150;
        }
        base.records[34].axis_type = 3;
        base.records[34].flags = 0xFE;
        let p = KeyPatch {
            actuation_mm: 0.8,
            rt: None,
            whole_travel: true,
        };
        let out = base.apply_patch(&[(34, p)], S).unwrap();
        assert_eq!(out.records[34].axis_type, 3);
        assert_eq!(out.records[34].flags, 0xFF);
        assert_eq!(out.records[34].actuation_raw, 80);
        assert_eq!(out.changed_indices(&base), vec![34]);
    }

    #[test]
    fn validation() {
        let mut base = HallTable::from_block(&[0u8; SIZE]).unwrap();
        for r in base.records.iter_mut() {
            r.actuation_raw = 150;
        }
        let ok = |a: f64, rt: Option<(f64, f64)>| {
            let p = KeyPatch {
                actuation_mm: a,
                rt: rt.map(|(p, r)| RtSensitivity {
                    press_mm: p,
                    release_mm: r,
                }),
                whole_travel: false,
            };
            base.apply_patch(&[(34, p)], S)
        };
        assert!(ok(0.10, None).is_ok());
        assert!(ok(3.20, None).is_ok());
        assert!(ok(0.1, Some((0.01, 3.2))).is_ok());
        assert!(matches!(
            ok(0.09, None),
            Err(HallError::Actuation { index: 34, .. })
        ));
        assert!(matches!(ok(3.21, None), Err(HallError::Actuation { .. })));
        assert!(matches!(ok(-1.0, None), Err(HallError::Actuation { .. })));
        assert!(matches!(
            ok(f64::NAN, None),
            Err(HallError::NotFinite { .. })
        ));
        assert!(matches!(
            ok(1.0, Some((0.0, 0.3))),
            Err(HallError::Sensitivity { which: "press", .. })
        ));
        assert!(matches!(
            ok(1.0, Some((0.004, 0.3))),
            Err(HallError::Sensitivity { .. })
        ));
        assert!(matches!(
            ok(1.0, Some((0.3, 3.21))),
            Err(HallError::Sensitivity {
                which: "release",
                ..
            })
        ));
        // Only physical keys.
        let p = KeyPatch {
            actuation_mm: 1.0,
            rt: None,
            whole_travel: false,
        };
        assert_eq!(
            base.apply_patch(&[(29, p)], S),
            Err(HallError::NotPhysical(29))
        );
        assert_eq!(
            base.apply_patch(&[(200, p)], S),
            Err(HallError::NotPhysical(200))
        );
        assert_eq!(
            base.apply_patch(&[(34, p), (34, p)], S),
            Err(HallError::Duplicate(34))
        );

        // Raw tables: non-physical changes need the backup policy; a
        // half-enabled RT record is refused.
        let mut raw = base.clone();
        raw.records[29].actuation_raw = 200;
        assert_eq!(
            validate_write(&base, &raw, S, NonPhysicalPolicy::PreserveAsRead),
            Err(HallError::NonPhysicalChanged(29))
        );
        assert!(validate_write(&base, &raw, S, NonPhysicalPolicy::AllowFromBackup).is_ok());
        raw.records[34].press_raw = 30;
        assert!(matches!(
            validate_write(&base, &raw, S, NonPhysicalPolicy::AllowFromBackup),
            Err(HallError::HalfRt { .. })
        ));
        raw.records[34].release_raw = 30;
        assert!(validate_write(&base, &raw, S, NonPhysicalPolicy::AllowFromBackup).is_ok());
        raw.records[34].actuation_raw = 0;
        assert!(validate_write(&base, &raw, S, NonPhysicalPolicy::AllowFromBackup).is_err());

        // The user's target block is valid as a raw restore over the switch state.
        let (target, _) = axis_rt(TARGET);
        let (left, _) = axis_rt(SWITCH_LEFT);
        let t = HallTable::from_block(&target).unwrap();
        let l = HallTable::from_block(&left).unwrap();
        assert!(validate_write(&l, &t, S, NonPhysicalPolicy::AllowFromBackup).is_ok());
        assert!(validate_write(&l, &t, S, NonPhysicalPolicy::PreserveAsRead).is_err());
    }

    #[test]
    fn precision_scales_are_respected() {
        let base = HallTable::from_block(&[0u8; SIZE]).unwrap();
        let s2 = RtScale::from_precision(2);
        let p = KeyPatch {
            actuation_mm: 0.805,
            rt: Some(RtSensitivity {
                press_mm: 0.305,
                release_mm: 0.3,
            }),
            whole_travel: false,
        };
        let all: Vec<(u8, KeyPatch)> = layout::keys().iter().map(|k| (k.index, p)).collect();
        let out = base.apply_patch(&all, s2).unwrap();
        assert_eq!(out.records[34].actuation_raw, 805);
        assert_eq!(out.records[34].press_raw, 305);
    }
}
