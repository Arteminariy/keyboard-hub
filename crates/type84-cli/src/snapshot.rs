//! Reading the full board state (PROTOCOL.md §2 and §7.1).

use anyhow::{bail, Context, Result};
use type84_proto::device_info::DeviceInfo;
use type84_proto::frame::{Frame, Input};
use type84_proto::macros::{self, MacroWalk};
use type84_proto::transfer::{read_block, FrameIo, TransferError, TransferEvent};
use type84_proto::ReadOp;

/// One logical read as it came off the wire.
#[derive(Debug, Clone)]
pub struct RawBlock {
    pub op: ReadOp,
    pub addr: usize,
    pub data: Vec<u8>,
    pub frames: Vec<Frame>,
}

/// Everything a backup needs.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub info: DeviceInfo,
    pub device_info: RawBlock,
    pub keymap_layer0: RawBlock,
    pub keymap_layer1: RawBlock,
    pub settings: RawBlock,
    pub led_effect: RawBlock,
    pub custom_led: RawBlock,
    pub macro_table: RawBlock,
    pub macro_reads: Vec<RawBlock>,
    pub macros: MacroWalk,
    pub axis_rt: RawBlock,
    pub dks: RawBlock,
    pub default_fn_keymap: RawBlock,
}

/// Wraps a transport and logs traffic to stderr when verbose.
pub struct Reader<'a> {
    pub io: &'a mut dyn FrameIo,
    pub verbose: bool,
    pub skipped: usize,
}

impl<'a> Reader<'a> {
    pub fn new(io: &'a mut dyn FrameIo, verbose: bool) -> Self {
        Reader {
            io,
            verbose,
            skipped: 0,
        }
    }

    pub fn read(
        &mut self,
        op: ReadOp,
        addr: usize,
        size: usize,
    ) -> Result<RawBlock, TransferError> {
        let verbose = self.verbose;
        let skipped = &mut self.skipped;
        let mut observer = |e: TransferEvent| match e {
            TransferEvent::Sent(f) if verbose => eprintln!("TX {}", hex::encode(f)),
            TransferEvent::Matched(f) if verbose => eprintln!("RX {}", hex::encode(f)),
            TransferEvent::Skipped(f, input) => {
                *skipped += 1;
                if verbose || matches!(input, Input::Notification(_)) {
                    eprintln!(
                        "note: skipped input frame ({input:?}): {}",
                        hex::encode(&f[..16])
                    );
                }
            }
            TransferEvent::Retry { op, addr, attempt } => {
                eprintln!(
                    "note: timeout on {} addr {addr:#06x}, resend {attempt}",
                    op.name()
                )
            }
            _ => {}
        };
        let r = read_block(self.io, op, addr, size, &mut observer)?;
        Ok(RawBlock {
            op,
            addr,
            data: r.data,
            frames: r.frames,
        })
    }

    pub fn read_full(&mut self, op: ReadOp) -> Result<RawBlock, TransferError> {
        self.read(op, 0, op.block_size())
    }

    pub fn device_info(&mut self) -> Result<(DeviceInfo, RawBlock)> {
        let b = self
            .read_full(ReadOp::DeviceInfo)
            .context("GET_DEVICE_INFO failed")?;
        let info = DeviceInfo::decode(&b.data)?;
        Ok((info, b))
    }

    fn snapshot_once(&mut self) -> Result<Snapshot, TransferError> {
        let b = self.read_full(ReadOp::DeviceInfo)?;
        let info = DeviceInfo::decode(&b.data).expect("48-byte block");
        let device_info = b;
        if !info.is_operational() {
            eprintln!(
                "warning: firmwareStatus = {} (firmware busy); values may be unreliable",
                info.firmware_status
            );
        }
        let keymap_layer0 = self.read_full(ReadOp::Keymap)?;
        let keymap_layer1 = self.read_full(ReadOp::FnKeymap)?;
        let settings = self.read_full(ReadOp::GameMode)?;
        let led_effect = self.read_full(ReadOp::LedEffect)?;
        let custom_led = self.read_full(ReadOp::CustomLed)?;
        let macro_table = self.read_full(ReadOp::Macro)?;
        let mut macro_reads = Vec::new();
        let macros = macros::walk(&macro_table.data, |addr, size| {
            let b = self.read(ReadOp::Macro, addr, size)?;
            let data = b.data.clone();
            macro_reads.push(b);
            Ok::<_, TransferError>(data)
        })?;
        let axis_rt = self.read_full(ReadOp::AxisRt)?;
        let dks = self.read_full(ReadOp::Dks)?;
        let default_fn_keymap = self.read_full(ReadOp::DefaultFnKeymap)?;
        Ok(Snapshot {
            info,
            device_info,
            keymap_layer0,
            keymap_layer1,
            settings,
            led_effect,
            custom_led,
            macro_table,
            macro_reads,
            macros,
            axis_rt,
            dks,
            default_fn_keymap,
        })
    }

    /// Read every block. If the board reports a reset mid-way, start over
    /// once; blocks from before and after a reset are never mixed.
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        for attempt in 0..2 {
            match self.snapshot_once() {
                Ok(s) => return Ok(s),
                Err(TransferError::DeviceReset { op }) if attempt == 0 => {
                    eprintln!(
                        "warning: device reset reported during {op}; restarting the full read"
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }
        bail!("device reset twice during the full read")
    }
}
