//! Record of the last lighting effect this tool wrote, so the colour can be
//! known although the board does not report it.
//!
//! On FW 1.17 GET_LED_EFFECT reads the colour back as `FF FF FF` and the
//! colour mode as multicolour, whatever was written (PROTOCOL.md §3.5). A
//! diff against the readback alone would skip a colour change, and an
//! `anim flash` restore would put back white instead of the user's colour.
//!
//! After every successful SET_LED_EFFECT write the tool saves the block it
//! wrote and the block the board then reported to
//! `$XDG_STATE_HOME/type84/lighting-last.json`. Later the record is trusted
//! only while the board still reports exactly that block; any other
//! readback (the vendor hub or an Fn shortcut changed the mode, brightness,
//! speed or direction) makes it stale and the colour unknown. A change of
//! the colour alone by another program cannot be detected.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use type84_hid::write::StepReport;
use type84_proto::led::{self, EFFECT_SIZE};
use type84_proto::WriteOp;

pub const FORMAT: &str = "type84-lighting-last";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightingLast {
    pub format: String,
    pub format_version: u32,
    pub written_at: String,
    /// The 16-byte SET_LED_EFFECT block as written, hex.
    pub written: String,
    /// The 16-byte GET_LED_EFFECT block read back right after, hex.
    pub reported: String,
    /// Human-readable description of `written`.
    pub description: String,
}

impl LightingLast {
    pub fn new(written: &[u8], reported: &[u8]) -> Self {
        LightingLast {
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            written_at: chrono::Local::now().to_rfc3339(),
            written: hex::encode(written),
            reported: hex::encode(reported),
            description: led::describe_effect(written),
        }
    }

    fn blocks(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let w = hex::decode(&self.written).context("lighting state: written is not hex")?;
        let r = hex::decode(&self.reported).context("lighting state: reported is not hex")?;
        if w.len() != EFFECT_SIZE || r.len() != EFFECT_SIZE {
            bail!("lighting state: blocks must be {EFFECT_SIZE} bytes");
        }
        Ok((w, r))
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let s: LightingLast = serde_json::from_str(text)?;
        if s.format != FORMAT || s.format_version != FORMAT_VERSION {
            bail!("not a type84 lighting state file");
        }
        s.blocks()?;
        Ok(s)
    }

    /// Load `path`; `Ok(None)` if it does not exist.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(Some(
                Self::from_json(&t).with_context(|| format!("loading {}", path.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f =
                File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))
    }

    /// The board's effect with the colour and colour mode of the last
    /// write, if the board still reports exactly what it reported then.
    pub fn known_effect(&self, read: &[u8]) -> Option<Vec<u8>> {
        let (w, r) = self.blocks().ok()?;
        if read.len() < EFFECT_SIZE || read[..EFFECT_SIZE] != r[..] {
            return None;
        }
        Some(with_colour_of(read, &w))
    }
}

/// `read` with bytes 1..=3 (colour) and 8 (colour mode) taken from `from`.
pub fn with_colour_of(read: &[u8], from: &[u8]) -> Vec<u8> {
    let mut out = read[..EFFECT_SIZE].to_vec();
    out[1..4].copy_from_slice(&from[1..4]);
    out[8] = from[8];
    out
}

pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join("lighting-last.json")
}

/// What the tool knows about the board's effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Knowledge {
    /// The readback plus the colour from the last write.
    Known(Vec<u8>),
    /// No record, or the board reports something else: colour unknown.
    Unknown { reason: String },
}

impl Knowledge {
    pub fn known(&self) -> Option<&[u8]> {
        match self {
            Knowledge::Known(b) => Some(b),
            Knowledge::Unknown { .. } => None,
        }
    }
}

/// Look up the record at `path` for the effect block `read`. A corrupt file
/// counts as no record.
pub fn knowledge(path: &Path, read: &[u8]) -> Knowledge {
    match LightingLast::load(path) {
        Ok(Some(s)) => match s.known_effect(read) {
            Some(b) => Knowledge::Known(b),
            None => Knowledge::Unknown {
                reason: format!(
                    "the board no longer reports what it reported after the last write ({}); \
                     something else changed the lighting",
                    s.written_at
                ),
            },
        },
        Ok(None) => Knowledge::Unknown {
            reason: format!("no record of a previous write ({})", path.display()),
        },
        Err(e) => Knowledge::Unknown {
            reason: format!("{e:#}"),
        },
    }
}

/// Save the last effect write of a plan, if it wrote one.
pub fn record(path: &Path, steps: &[StepReport]) -> Result<()> {
    if let Some(s) = steps
        .iter()
        .rev()
        .find(|s| s.op == WriteOp::LedEffect && s.frames_written > 0)
    {
        LightingLast::new(&s.written, &s.read_back).save(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_only_while_the_board_reports_the_same_block() {
        let dir = std::env::temp_dir().join(format!("type84-light-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = path(&dir);
        let read = hex::decode("0fffffff000000000105040000000000").unwrap();
        assert!(matches!(knowledge(&p, &read), Knowledge::Unknown { .. }));
        // Wrote circles #FF3B4E single colour; the board reports white multicolour.
        let written = hex::decode("0fff3b4eff000000000505000000aa55").unwrap();
        let reported = hex::decode("0fffffff000000000105050000000000").unwrap();
        LightingLast::new(&written, &reported).save(&p).unwrap();
        let k = knowledge(&p, &reported);
        let b = k.known().unwrap();
        assert_eq!(&b[1..4], &[0xFF, 0x3B, 0x4E]);
        assert_eq!(b[8], 0);
        assert_eq!(b[0], 15);
        assert_eq!(b[9], 5);
        // Brightness changed elsewhere: stale.
        assert!(matches!(knowledge(&p, &read), Knowledge::Unknown { .. }));
        // Corrupt file: unknown, not an error.
        std::fs::write(&p, "{").unwrap();
        assert!(matches!(
            knowledge(&p, &reported),
            Knowledge::Unknown { .. }
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
