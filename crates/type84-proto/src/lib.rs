//! Pure codec for the configuration protocol ("HFD") of the
//! IO by Red Square Type 84 Magnetic keyboard (USB 0c45:80d6).
//!
//! This crate performs no I/O. It knows how to build request frames, how to
//! recognise and decode response and notification frames, how to split a
//! logical block read into packets, and how to decode every block the board
//! exposes into typed models. See `docs/PROTOCOL.md` for the specification.
//!
//! Writes: the Hall/RT table (SET_MAGNETIC_AXIS_RT, 0x27, [`hall`]), the Fn
//! layer (SET_FN_KEY, 0x26, [`fn_layer`]) and the lighting effect and
//! per-key colours (SET_LED_EFFECT 0x23, SET_CUSTOM_LED_DATA 0x24, [`led`]).
//! See [`opcode::WriteOp`]. No other write opcode exists in this crate.

pub mod device_info;
pub mod dks;
pub mod fn_layer;
pub mod frame;
pub mod hall;
pub mod hid_usage;
pub mod keymap;
pub mod layout;
pub mod led;
pub mod macros;
pub mod opcode;
pub mod rt;
pub mod settings;
pub mod transfer;

pub use frame::{Frame, FRAME_LEN, PAYLOAD_LEN};
pub use opcode::{
    is_allowed_read_opcode, is_allowed_write_opcode, ReadOp, WriteOp, READ_ALLOWLIST,
    WRITE_ALLOWLIST,
};
