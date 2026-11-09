//! Opcodes. All read opcodes are modelled in [`ReadOp`]. The write opcodes
//! in [`WriteOp`] are exactly SET_LED_EFFECT (0x23), SET_CUSTOM_LED_DATA
//! (0x24), SET_FN_KEY (0x26) and SET_MAGNETIC_AXIS_RT (0x27). Every other
//! write opcode intentionally does not exist in this crate.

use std::time::Duration;

/// Every opcode a host frame may carry in this milestone (PROTOCOL.md §5:
/// reads 0x10–0x18 and 0x1C). 0x1F is defined by the vendor but never sent,
/// so it is not allowed.
pub const READ_ALLOWLIST: [u8; 10] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x1C];

/// True when `op` is one of the allowed read opcodes.
pub fn is_allowed_read_opcode(op: u8) -> bool {
    READ_ALLOWLIST.contains(&op)
}

/// Read commands understood by the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ReadOp {
    DeviceInfo = 0x10,
    GameMode = 0x11,
    Keymap = 0x12,
    LedEffect = 0x13,
    CustomLed = 0x14,
    Macro = 0x15,
    FnKeymap = 0x16,
    AxisRt = 0x17,
    Dks = 0x18,
    DefaultFnKeymap = 0x1C,
}

impl ReadOp {
    pub const ALL: [ReadOp; 10] = [
        ReadOp::DeviceInfo,
        ReadOp::GameMode,
        ReadOp::Keymap,
        ReadOp::LedEffect,
        ReadOp::CustomLed,
        ReadOp::Macro,
        ReadOp::FnKeymap,
        ReadOp::AxisRt,
        ReadOp::Dks,
        ReadOp::DefaultFnKeymap,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<ReadOp> {
        ReadOp::ALL.into_iter().find(|op| op.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            ReadOp::DeviceInfo => "GET_DEVICE_INFO",
            ReadOp::GameMode => "GET_GAME_MODE",
            ReadOp::Keymap => "GET_KEY",
            ReadOp::LedEffect => "GET_LED_EFFECT",
            ReadOp::CustomLed => "GET_CUSTOM_LED_DATA",
            ReadOp::Macro => "GET_MACRO",
            ReadOp::FnKeymap => "GET_FN_KEY",
            ReadOp::AxisRt => "GET_MAGNETIC_AXIS_RT",
            ReadOp::Dks => "GET_MAGNETIC_AXIS_DKS_DATA",
            ReadOp::DefaultFnKeymap => "GET_DEFAULT_FN_KEY_MATRIX",
        }
    }

    /// Size of the whole logical block. For GET_MACRO this is the size of
    /// the offset table; macro bodies are read with addressed transfers.
    pub fn block_size(self) -> usize {
        match self {
            ReadOp::DeviceInfo => 48,
            ReadOp::GameMode => 56,
            ReadOp::Keymap | ReadOp::FnKeymap | ReadOp::DefaultFnKeymap => 512,
            ReadOp::LedEffect => 16,
            ReadOp::CustomLed => 512,
            ReadOp::Macro => crate::macros::TABLE_SIZE,
            ReadOp::AxisRt | ReadOp::Dks => 1024,
        }
    }

    /// Per-packet response timeout (PROTOCOL.md §1.6). Reads use the
    /// vendor's values or longer, never shorter.
    pub fn timeout(self) -> Duration {
        match self {
            ReadOp::DeviceInfo | ReadOp::LedEffect => Duration::from_millis(1000),
            _ => Duration::from_millis(2000),
        }
    }

    /// Whether the response must echo the request address (PROTOCOL.md §1.6).
    pub fn checks_addr(self) -> bool {
        matches!(
            self,
            ReadOp::DeviceInfo | ReadOp::Keymap | ReadOp::FnKeymap | ReadOp::AxisRt
        )
    }
}

/// SET_MAGNETIC_AXIS_RT, the Hall/RT table (VERIFIED on FW 1.17).
pub const HALL_WRITE_OPCODE: u8 = 0x27;

/// Every write opcode this crate can encode. Each one was added explicitly
/// (PROTOCOL.md §4); everything else, including SET_KEY (0x22, layer 0),
/// SET_GAME_MODE (0x21), SET_MACRO (0x25) and SET_MAGNETIC_AXIS_DKS_DATA
/// (0x28), does not exist here.
pub const WRITE_ALLOWLIST: [u8; 4] = [0x23, 0x24, 0x26, HALL_WRITE_OPCODE];

/// True for the opcodes of [`WriteOp`].
pub fn is_allowed_write_opcode(op: u8) -> bool {
    WRITE_ALLOWLIST.contains(&op)
}

/// Write commands this crate can encode (PROTOCOL.md §4). A write session
/// additionally opts in to a subset of them per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum WriteOp {
    /// SET_LED_EFFECT, 16 bytes (§4.5). IMPLEMENTED, UNVERIFIED on hardware.
    LedEffect = 0x23,
    /// SET_CUSTOM_LED_DATA, 512 bytes (§4.6). IMPLEMENTED, UNVERIFIED.
    CustomLed = 0x24,
    /// SET_FN_KEY, the Fn layer (layer 1), 512 bytes (§4.2). IMPLEMENTED,
    /// UNVERIFIED.
    FnKeymap = 0x26,
    /// SET_MAGNETIC_AXIS_RT, 1024 bytes (§4.3). VERIFIED (FW 1.17).
    AxisRt = HALL_WRITE_OPCODE,
}

impl WriteOp {
    pub const ALL: [WriteOp; 4] = [
        WriteOp::LedEffect,
        WriteOp::CustomLed,
        WriteOp::FnKeymap,
        WriteOp::AxisRt,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<WriteOp> {
        WriteOp::ALL.into_iter().find(|op| op.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            WriteOp::LedEffect => "SET_LED_EFFECT",
            WriteOp::CustomLed => "SET_CUSTOM_LED_DATA",
            WriteOp::FnKeymap => "SET_FN_KEY",
            WriteOp::AxisRt => "SET_MAGNETIC_AXIS_RT",
        }
    }

    /// The read opcode that returns the same block (`SET = GET + 0x10`).
    pub fn read_op(self) -> ReadOp {
        match self {
            WriteOp::LedEffect => ReadOp::LedEffect,
            WriteOp::CustomLed => ReadOp::CustomLed,
            WriteOp::FnKeymap => ReadOp::FnKeymap,
            WriteOp::AxisRt => ReadOp::AxisRt,
        }
    }

    /// Size of the whole logical block. Writes always send the full block.
    pub fn block_size(self) -> usize {
        self.read_op().block_size()
    }

    /// Per-packet ACK timeout (PROTOCOL.md §1.6: SHORT for the LED effect,
    /// BLOCK for the rest).
    pub fn timeout(self) -> Duration {
        match self {
            WriteOp::LedEffect => Duration::from_millis(1000),
            _ => Duration::from_millis(2000),
        }
    }

    /// Whether the ACK must echo the request address. Same set as the hub
    /// (PROTOCOL.md §1.6): key layers and the RT table, not the LED blocks.
    pub fn checks_addr(self) -> bool {
        matches!(self, WriteOp::FnKeymap | WriteOp::AxisRt)
    }

    /// Delay before the verifying readback: 40 ms after RT (§4.3, also used
    /// for the key layer), 500 ms after lighting (§4.5,
    /// LIGHTING_APPLY_DELAY_MS).
    pub fn settle_delay(self) -> Duration {
        match self {
            WriteOp::LedEffect | WriteOp::CustomLed => Duration::from_millis(500),
            WriteOp::FnKeymap | WriteOp::AxisRt => Duration::from_millis(40),
        }
    }

    /// Number of frames a full-block write takes.
    pub fn frame_count(self) -> usize {
        self.block_size().div_ceil(crate::frame::PAYLOAD_LEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_exactly_the_read_ops() {
        for op in 0u8..=255 {
            let expected = ReadOp::from_code(op).is_some();
            assert_eq!(is_allowed_read_opcode(op), expected, "opcode {op:#04x}");
        }
        for bad in [
            0x0F, 0x19, 0x1A, 0x1B, 0x1F, 0x21, 0x22, 0x27, 0x28, 0x64, 0x68, 0x82, 0x86,
        ] {
            assert!(!is_allowed_read_opcode(bad));
        }
    }

    #[test]
    fn write_allowlist_is_exactly_the_write_ops() {
        let allowed: Vec<u8> = (0u8..=255)
            .filter(|&op| is_allowed_write_opcode(op))
            .collect();
        assert_eq!(allowed, vec![0x23, 0x24, 0x26, 0x27]);
        for op in 0u8..=255 {
            assert_eq!(
                WriteOp::from_code(op).is_some(),
                is_allowed_write_opcode(op)
            );
        }
        for op in WriteOp::ALL {
            assert_eq!(op.code(), op.read_op().code() + 0x10, "{}", op.name());
        }
        // Never: factory reset, game mode, layer 0, macros, DKS, calibration, OTA.
        for bad in [
            0x0F, 0x21, 0x22, 0x25, 0x28, 0x34, 0x41, 0x50, 0x64, 0x69, 0x82, 0x83,
        ] {
            assert!(!is_allowed_write_opcode(bad), "{bad:#04x}");
        }
        assert_eq!(WriteOp::AxisRt.block_size(), 1024);
        assert_eq!(WriteOp::AxisRt.frame_count(), 19);
        assert_eq!(WriteOp::FnKeymap.frame_count(), 10);
        assert_eq!(WriteOp::CustomLed.frame_count(), 10);
        assert_eq!(WriteOp::LedEffect.frame_count(), 1);
    }
}
