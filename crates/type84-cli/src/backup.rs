//! Versioned JSON backup format.
//!
//! Every block needed for a restore (PROTOCOL.md §7.1) is stored as raw hex,
//! together with each 64-byte response frame it was assembled from.
//! Decoded sections are informational only; a restore must use the raw data.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use type84_proto::device_info::DeviceInfo;
use type84_proto::led::LedEffect;
use type84_proto::macros::MacroWalk;
use type84_proto::settings::Settings;
use type84_proto::ReadOp;

use crate::snapshot::{RawBlock, Snapshot};

pub const FORMAT: &str = "type84-backup";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub opcode: u8,
    pub command: String,
    pub addr: usize,
    pub size: usize,
    /// Whether a restore writes this block back (PROTOCOL.md §7.2).
    pub restore: bool,
    pub data: String,
    /// Hex of every matched 64-byte response frame, in order.
    pub frames: Vec<String>,
}

impl Block {
    pub fn from_raw(b: &RawBlock, restore: bool) -> Self {
        Block {
            opcode: b.op.code(),
            command: b.op.name().to_string(),
            addr: b.addr,
            size: b.data.len(),
            restore,
            data: hex::encode(&b.data),
            frames: b.frames.iter().map(hex::encode).collect(),
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>> {
        let d = hex::decode(&self.data)
            .with_context(|| format!("{}: data is not hex", self.command))?;
        if d.len() != self.size {
            bail!(
                "{}: data length {} differs from size {}",
                self.command,
                d.len(),
                self.size
            );
        }
        Ok(d)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSection {
    pub hid_path: String,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub usb_release: String,
    pub firmware: String,
    pub info: DeviceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocks {
    pub device_info: Block,
    pub keymap_layer0: Block,
    pub keymap_layer1: Block,
    pub settings: Block,
    pub led_effect: Block,
    pub custom_led: Block,
    pub macro_table: Block,
    pub axis_rt: Block,
    pub dks: Block,
    /// Factory Fn layer, kept for reference; never restored.
    pub default_fn_keymap: Block,
}

impl Blocks {
    /// (name, block) pairs in restore order, then reference-only blocks.
    pub fn named(&self) -> Vec<(&'static str, &Block)> {
        vec![
            ("keymap_layer0", &self.keymap_layer0),
            ("keymap_layer1", &self.keymap_layer1),
            ("settings", &self.settings),
            ("custom_led", &self.custom_led),
            ("led_effect", &self.led_effect),
            ("axis_rt", &self.axis_rt),
            ("dks", &self.dks),
            ("macro_table", &self.macro_table),
            ("device_info", &self.device_info),
            ("default_fn_keymap", &self.default_fn_keymap),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroSection {
    pub walk: MacroWalk,
    /// Raw addressed reads of macro headers and bodies.
    pub reads: Vec<Block>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decoded {
    pub settings: Settings,
    pub led_effect: Option<LedEffect>,
    pub rt_scale: type84_proto::rt::RtScale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub format: String,
    pub format_version: u32,
    pub tool_version: String,
    pub created_at: String,
    pub device: DeviceSection,
    pub blocks: Blocks,
    pub macros: MacroSection,
    pub decoded: Decoded,
}

impl Backup {
    pub fn from_snapshot(s: &Snapshot, iface: &type84_hid::Interface) -> Result<Self> {
        let settings = Settings::decode(&s.settings.data)?;
        Ok(Backup {
            format: FORMAT.to_string(),
            format_version: FORMAT_VERSION,
            tool_version: concat!("type84 ", env!("CARGO_PKG_VERSION")).to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            device: DeviceSection {
                hid_path: iface.path_str(),
                product: iface.product.clone(),
                manufacturer: iface.manufacturer.clone(),
                serial: iface.serial.clone(),
                usb_release: format!("{:#06x}", iface.release),
                firmware: s.info.version_string(),
                info: s.info.clone(),
            },
            blocks: Blocks {
                device_info: Block::from_raw(&s.device_info, false),
                keymap_layer0: Block::from_raw(&s.keymap_layer0, true),
                keymap_layer1: Block::from_raw(&s.keymap_layer1, true),
                settings: Block::from_raw(&s.settings, true),
                led_effect: Block::from_raw(&s.led_effect, true),
                custom_led: Block::from_raw(&s.custom_led, true),
                macro_table: Block::from_raw(&s.macro_table, true),
                axis_rt: Block::from_raw(&s.axis_rt, true),
                dks: Block::from_raw(&s.dks, true),
                default_fn_keymap: Block::from_raw(&s.default_fn_keymap, false),
            },
            macros: MacroSection {
                walk: s.macros.clone(),
                reads: s
                    .macro_reads
                    .iter()
                    .map(|b| Block::from_raw(b, true))
                    .collect(),
            },
            decoded: Decoded {
                settings,
                led_effect: LedEffect::decode(&s.led_effect.data),
                rt_scale: type84_proto::rt::RtScale::from_precision(s.info.rt_precision),
            },
        })
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let b: Backup =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if b.format != FORMAT {
            bail!(
                "{} is not a type84 backup (format = {:?})",
                path.display(),
                b.format
            );
        }
        if b.format_version != FORMAT_VERSION {
            bail!(
                "unsupported backup format_version {} (this tool reads {FORMAT_VERSION})",
                b.format_version
            );
        }
        b.validate()?;
        Ok(b)
    }

    /// Size checks from PROTOCOL.md §7.2.
    pub fn validate(&self) -> Result<()> {
        let expect = [
            (&self.blocks.keymap_layer0, ReadOp::Keymap),
            (&self.blocks.keymap_layer1, ReadOp::FnKeymap),
            (&self.blocks.settings, ReadOp::GameMode),
            (&self.blocks.led_effect, ReadOp::LedEffect),
            (&self.blocks.custom_led, ReadOp::CustomLed),
            (&self.blocks.macro_table, ReadOp::Macro),
            (&self.blocks.axis_rt, ReadOp::AxisRt),
            (&self.blocks.dks, ReadOp::Dks),
            (&self.blocks.device_info, ReadOp::DeviceInfo),
            (&self.blocks.default_fn_keymap, ReadOp::DefaultFnKeymap),
        ];
        for (b, op) in expect {
            if b.opcode != op.code() || b.size != op.block_size() {
                bail!(
                    "block {} has opcode {:#04x} size {}, expected {:#04x} size {}",
                    b.command,
                    b.opcode,
                    b.size,
                    op.code(),
                    op.block_size()
                );
            }
            b.bytes()?;
        }
        if self
            .macros
            .walk
            .macros
            .iter()
            .any(|m| m.id >= type84_proto::macros::MAX_MACROS)
        {
            bail!("macro id out of range");
        }
        Ok(())
    }
}
