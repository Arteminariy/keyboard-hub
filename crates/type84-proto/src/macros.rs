//! GET_MACRO (0x15): a 400-byte offset table followed by addressed reads of
//! each macro body (PROTOCOL.md §3.7).

use serde::{Deserialize, Serialize};

pub const TABLE_SIZE: usize = 400;
pub const MAX_MACROS: usize = 100;
/// First byte of the macro body area.
pub const AREA_START: usize = TABLE_SIZE;
/// Size of the body area the vendor assumes for this board.
pub const AREA_SIZE: usize = 512;
pub const AREA_END: usize = AREA_START + AREA_SIZE;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum MacroError {
    #[error("macro {id}: offset {offset} outside the body area {AREA_START}..{AREA_END}")]
    BadOffset { id: usize, offset: u32 },
    #[error("macro {id}: {count} actions at offset {offset} overflow the body area")]
    Overflow {
        id: usize,
        offset: u32,
        count: usize,
    },
    #[error("macro {id}: header length field {raw} is odd")]
    OddLength { id: usize, raw: u16 },
}

/// Decode the offset table into 100 absolute offsets (0 = empty slot).
pub fn parse_table(table: &[u8]) -> Vec<u32> {
    table
        .chunks_exact(4)
        .take(MAX_MACROS)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Check that a table entry points into the body area with room for a header.
pub fn check_offset(id: usize, offset: u32) -> Result<(), MacroError> {
    let o = offset as usize;
    if o < AREA_START || o + 4 > AREA_END {
        return Err(MacroError::BadOffset { id, offset });
    }
    Ok(())
}

/// Number of actions encoded in a 4-byte macro header (`u16 len = actions*2`, `u16 0`).
pub fn parse_header(id: usize, offset: u32, header: &[u8]) -> Result<usize, MacroError> {
    let raw = u16::from_le_bytes([header[0], header[1]]);
    if !raw.is_multiple_of(2) {
        return Err(MacroError::OddLength { id, raw });
    }
    let count = raw as usize / 2;
    if offset as usize + 4 + count * 4 > AREA_END {
        return Err(MacroError::Overflow { id, offset, count });
    }
    Ok(count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroAction {
    pub delay_ms: u16,
    pub code: u8,
    pub flags: u8,
}

impl MacroAction {
    pub fn decode(b: &[u8]) -> Self {
        MacroAction {
            delay_ms: u16::from_le_bytes([b[0], b[1]]),
            code: b[2],
            flags: b[3],
        }
    }

    pub fn press(&self) -> bool {
        self.flags & 0x80 != 0
    }

    /// 1/2 = mouse, 3 = keyboard key.
    pub fn kind(&self) -> u8 {
        (self.flags >> 4) & 7
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    pub id: usize,
    pub offset: u32,
    pub actions: Vec<MacroAction>,
}

/// Result of walking the macro area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroWalk {
    pub macros: Vec<Macro>,
    /// Table entries that could not be followed safely.
    pub errors: Vec<MacroError>,
}

/// Walk the table, calling `read(addr, size)` for each header and body.
/// Entries that fail validation are recorded in `errors` and not followed.
pub fn walk<E>(
    table: &[u8],
    mut read: impl FnMut(usize, usize) -> Result<Vec<u8>, E>,
) -> Result<MacroWalk, E> {
    let mut out = MacroWalk {
        macros: Vec::new(),
        errors: Vec::new(),
    };
    for (id, offset) in parse_table(table).into_iter().enumerate() {
        if offset == 0 {
            continue;
        }
        if let Err(e) = check_offset(id, offset) {
            out.errors.push(e);
            continue;
        }
        let header = read(offset as usize, 4)?;
        let count = match parse_header(id, offset, &header) {
            Ok(c) => c,
            Err(e) => {
                out.errors.push(e);
                continue;
            }
        };
        let actions = if count == 0 {
            Vec::new()
        } else {
            read(offset as usize + 4, count * 4)?
                .chunks_exact(4)
                .map(MacroAction::decode)
                .collect()
        };
        out.macros.push(Macro {
            id,
            offset,
            actions,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    fn image() -> Vec<u8> {
        // Table + body area as the board would hold it: macro 0 and macro 5.
        let mut img = vec![0u8; AREA_END];
        img[0..4].copy_from_slice(&400u32.to_le_bytes());
        img[20..24].copy_from_slice(&416u32.to_le_bytes());
        // macro 0: 2 actions (press A, release A after 30 ms)
        img[400..404].copy_from_slice(&[4, 0, 0, 0]);
        img[404..408].copy_from_slice(&[0, 0, 0x04, 0xB0]);
        img[408..412].copy_from_slice(&[30, 0, 0x04, 0x30]);
        // padding to 416, macro 5: 1 action
        img[416..420].copy_from_slice(&[2, 0, 0, 0]);
        img[420..424].copy_from_slice(&[0x10, 0x27, 0x01, 0x90]);
        img
    }

    #[test]
    fn walk_reads_headers_and_bodies() {
        let img = image();
        let mut reads = Vec::new();
        let w = walk::<Infallible>(&img[..TABLE_SIZE], |a, n| {
            reads.push((a, n));
            Ok(img[a..a + n].to_vec())
        })
        .unwrap();
        assert!(w.errors.is_empty());
        assert_eq!(reads, vec![(400, 4), (404, 8), (416, 4), (420, 4)]);
        assert_eq!(w.macros.len(), 2);
        assert_eq!(w.macros[0].id, 0);
        assert_eq!(
            w.macros[0].actions[0],
            MacroAction {
                delay_ms: 0,
                code: 4,
                flags: 0xB0
            }
        );
        assert!(w.macros[0].actions[0].press());
        assert_eq!(w.macros[0].actions[0].kind(), 3);
        assert!(!w.macros[0].actions[1].press());
        assert_eq!(w.macros[1].id, 5);
        assert_eq!(w.macros[1].actions[0].delay_ms, 10000);
        assert_eq!(w.macros[1].actions[0].kind(), 1);
    }

    #[test]
    fn empty_table_reads_nothing() {
        let w = walk::<Infallible>(&[0u8; TABLE_SIZE], |_, _| panic!("no reads expected")).unwrap();
        assert!(w.macros.is_empty() && w.errors.is_empty());
    }

    #[test]
    fn garbage_table_is_not_followed() {
        let mut t = vec![0u8; TABLE_SIZE];
        t[0..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        t[4..8].copy_from_slice(&12u32.to_le_bytes());
        t[8..12].copy_from_slice(&900u32.to_le_bytes()); // valid offset, header overflows
        let w = walk::<Infallible>(&t, |a, n| {
            assert_eq!((a, n), (900, 4));
            Ok(vec![100, 0, 0, 0])
        })
        .unwrap();
        assert!(w.macros.is_empty());
        assert_eq!(w.errors.len(), 3);
        assert!(matches!(w.errors[2], MacroError::Overflow { id: 2, .. }));
    }
}
