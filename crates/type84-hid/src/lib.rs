//! hidapi transport for the IO by Red Square Type 84 Magnetic.
//!
//! The default path ([`Hid::open_config`], [`ReadOnlyTransport`]) is
//! read-only. Writes (Hall/RT table, Fn layer, lighting) live in the
//! separate opt-in [`write::WriteSession`] (see that module and
//! `docs/SAFETY.md`); nothing on the read-only path can reach it.
//!
//! Safety rules enforced here (PROTOCOL.md §1, §5):
//! - Only the vendor configuration collection (usage page 0xFF68, usage
//!   0x61) is ever opened. A node that also exposes any other collection is
//!   refused.
//! - Every outgoing frame passes [`check_read_frame`] immediately before the
//!   write call: request mark 0xAA, an opcode from the read allowlist, zero
//!   reserved header bytes and an all-zero payload. Anything else is refused
//!   without touching the device.
//! - No feature reports, no raw handle access, no write commands on the
//!   read-only transport.

use std::ffi::{CStr, CString};
use std::time::Duration;

use type84_proto::frame::{Frame, FRAME_LEN, HEADER_LEN, REQUEST_MARK};
use type84_proto::opcode::is_allowed_read_opcode;
use type84_proto::transfer::{FrameIo, IoError};

#[cfg(feature = "mock")]
pub mod mock;
pub mod write;
pub use write::{
    check_session_frame, ApplyError, CustomLedTarget, EffectTarget, FnLayerTarget, HallTarget,
    HallWriteOptIn, HallWriteReport, HallWriteRequest, StepReport, WriteOptIn, WriteReport,
    WriteSession, WriteStep, WriteTarget,
};

pub const VENDOR_ID: u16 = 0x0C45;
pub const PRODUCT_ID: u16 = 0x80D6;
pub const CONFIG_USAGE_PAGE: u16 = 0xFF68;
pub const CONFIG_USAGE: u16 = 0x0061;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("hidapi: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("refused to send frame: {0}")]
    Refused(&'static str),
    #[error("refused to send frame: opcode {0:#04x} is not allowed on this channel")]
    OpcodeNotAllowed(u8),
    #[error("no Type 84 configuration interface (VID 0x0C45 PID 0x80D6, usage page 0xFF68 usage 0x61) found")]
    NotFound,
    #[error("more than one Type 84 configuration interface found ({0}); unplug the extra board")]
    Ambiguous(usize),
    #[error("configuration node {path} also exposes usage page {other:#06x}; refusing to open it")]
    SharedNode { path: String, other: u16 },
    #[error("short write: {written} of {expected} bytes")]
    ShortWrite { written: usize, expected: usize },
}

/// Validate a host frame against the read-only policy. Must be called
/// before every write.
pub fn check_read_frame(frame: &Frame) -> Result<(), Error> {
    if frame[0] != REQUEST_MARK {
        return Err(Error::Refused("byte 0 is not the 0xAA request mark"));
    }
    if !is_allowed_read_opcode(frame[1]) {
        return Err(Error::OpcodeNotAllowed(frame[1]));
    }
    if frame[5] != 0 || frame[7] != 0 || frame[6] > 1 {
        return Err(Error::Refused("reserved header bytes must be zero"));
    }
    if frame[HEADER_LEN..].iter().any(|&b| b != 0) {
        return Err(Error::Refused(
            "read requests must carry an all-zero payload",
        ));
    }
    Ok(())
}

/// The two byte-level operations the transport needs. Implemented for
/// `hidapi::HidDevice` and for test doubles.
pub trait RawHid {
    fn write(&self, data: &[u8]) -> Result<usize, hidapi::HidError>;
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, hidapi::HidError>;
}

impl RawHid for hidapi::HidDevice {
    fn write(&self, data: &[u8]) -> Result<usize, hidapi::HidError> {
        hidapi::HidDevice::write(self, data)
    }
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, hidapi::HidError> {
        hidapi::HidDevice::read_timeout(self, buf, timeout_ms)
    }
}

/// Transport that can only send read requests. The device handle is private
/// and never handed out.
pub struct ReadOnlyTransport<D: RawHid> {
    dev: D,
}

impl<D: RawHid> ReadOnlyTransport<D> {
    /// Wrap an already-selected device handle.
    pub fn new(dev: D) -> Self {
        ReadOnlyTransport { dev }
    }

    /// Send one request frame after the allowlist check.
    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), Error> {
        check_read_frame(frame)?;
        // hidapi wants the report ID in byte 0; 0x00 means "no report ID"
        // and is stripped, so exactly 64 frame bytes go on the wire.
        let mut buf = [0u8; FRAME_LEN + 1];
        buf[1..].copy_from_slice(frame);
        debug_assert!(is_allowed_read_opcode(buf[2]));
        let n = self.dev.write(&buf)?;
        if n < FRAME_LEN {
            return Err(Error::ShortWrite {
                written: n,
                expected: FRAME_LEN + 1,
            });
        }
        Ok(())
    }

    /// Read one input report. `None` on timeout.
    pub fn read_frame(&mut self, timeout: Duration) -> Result<Option<Frame>, Error> {
        let ms = if timeout.is_zero() {
            0
        } else {
            timeout.as_millis().clamp(1, i32::MAX as u128) as i32
        };
        let mut buf = [0u8; FRAME_LEN + 1];
        let n = self.dev.read_timeout(&mut buf, ms)?;
        if n == 0 {
            return Ok(None);
        }
        let mut f = [0u8; FRAME_LEN];
        let n = n.min(FRAME_LEN);
        f[..n].copy_from_slice(&buf[..n]);
        Ok(Some(f))
    }
}

impl<D: RawHid> FrameIo for ReadOnlyTransport<D> {
    fn send_request(&mut self, frame: &Frame) -> Result<(), IoError> {
        self.write_frame(frame).map_err(|e| Box::new(e) as IoError)
    }
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, IoError> {
        self.read_frame(timeout).map_err(|e| Box::new(e) as IoError)
    }
}

/// One HID collection of the board, as reported by enumeration.
#[derive(Debug, Clone)]
pub struct Interface {
    pub path: CString,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: i32,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub release: u16,
}

impl Interface {
    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub fn is_config(&self) -> bool {
        self.usage_page == CONFIG_USAGE_PAGE && self.usage == CONFIG_USAGE
    }
}

/// Transport bound to a real hidapi device.
pub type Transport = ReadOnlyTransport<hidapi::HidDevice>;

/// Owns the hidapi context. The context itself is never exposed, so the
/// only way to open a device is [`Hid::open_config`].
pub struct Hid {
    api: hidapi::HidApi,
}

impl Hid {
    pub fn new() -> Result<Self, Error> {
        Ok(Hid {
            api: hidapi::HidApi::new()?,
        })
    }

    /// Re-run device discovery so [`Hid::list_interfaces`] and
    /// [`Hid::open_config`] see boards plugged in or removed since the last
    /// call. Enumeration only; opens nothing.
    pub fn refresh(&mut self) -> Result<(), Error> {
        self.api.refresh_devices()?;
        Ok(())
    }

    /// Enumerate every collection with the board's VID/PID. Opens nothing.
    pub fn list_interfaces(&self) -> Vec<Interface> {
        list_interfaces(&self.api)
    }

    /// Find and open the configuration collection as a read-only transport.
    pub fn open_config(&self) -> Result<(Interface, Transport), Error> {
        let iface = select_config(&list_interfaces(&self.api))?;
        let dev = self.api.open_path(&iface.path)?;
        Ok((iface, ReadOnlyTransport::new(dev)))
    }

    /// Open the configuration collection as a write session. This is a
    /// separate entry point from [`Hid::open_config`] and requires an
    /// explicit [`WriteOptIn`] naming the sections it may write. The only
    /// way to write is [`WriteSession::apply_plan`].
    pub fn open_write_session(
        &self,
        opt_in: WriteOptIn,
    ) -> Result<(Interface, WriteSession<hidapi::HidDevice>), Error> {
        let iface = select_config(&list_interfaces(&self.api))?;
        let dev = self.api.open_path(&iface.path)?;
        Ok((iface, WriteSession::new(dev, opt_in)))
    }
}

fn list_interfaces(api: &hidapi::HidApi) -> Vec<Interface> {
    api.device_list()
        .filter(|d| d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID)
        .map(|d| Interface {
            path: d.path().to_owned(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            interface_number: d.interface_number(),
            product: d.product_string().map(str::to_string),
            manufacturer: d.manufacturer_string().map(str::to_string),
            serial: d.serial_number().map(str::to_string),
            release: d.release_number(),
        })
        .collect()
}

/// Pick the single configuration collection. Pure function over an
/// enumeration result, so the policy is unit-testable.
pub fn select_config(interfaces: &[Interface]) -> Result<Interface, Error> {
    let configs: Vec<&Interface> = interfaces.iter().filter(|i| i.is_config()).collect();
    let mut paths: Vec<&CStr> = configs.iter().map(|i| i.path.as_c_str()).collect();
    paths.sort();
    paths.dedup();
    match paths.len() {
        0 => return Err(Error::NotFound),
        1 => {}
        n => return Err(Error::Ambiguous(n)),
    }
    let chosen = configs[0];
    if let Some(other) = interfaces
        .iter()
        .find(|i| i.path == chosen.path && !i.is_config())
    {
        return Err(Error::SharedNode {
            path: chosen.path_str(),
            other: other.usage_page,
        });
    }
    Ok(chosen.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use type84_proto::frame::encode_read_request;
    use type84_proto::ReadOp;

    #[derive(Default)]
    struct MockDev {
        written: RefCell<Vec<Vec<u8>>>,
    }

    impl RawHid for MockDev {
        fn write(&self, data: &[u8]) -> Result<usize, hidapi::HidError> {
            self.written.borrow_mut().push(data.to_vec());
            Ok(data.len())
        }
        fn read_timeout(&self, _buf: &mut [u8], _ms: i32) -> Result<usize, hidapi::HidError> {
            Ok(0)
        }
    }

    fn raw_frame(op: u8) -> Frame {
        let mut f = [0u8; 64];
        f[0] = 0xAA;
        f[1] = op;
        f[2] = 0x10;
        f[6] = 1;
        f
    }

    #[test]
    fn forbidden_opcodes_never_reach_the_device() {
        let mut t = ReadOnlyTransport::new(MockDev::default());
        for op in 0u8..=255 {
            let r = t.write_frame(&raw_frame(op));
            assert_eq!(r.is_ok(), is_allowed_read_opcode(op), "opcode {op:#04x}");
        }
        for bad in [
            0x0F, 0x19, 0x1A, 0x1B, 0x1F, 0x21, 0x27, 0x64, 0x68, 0x82, 0x86,
        ] {
            assert!(
                matches!(t.write_frame(&raw_frame(bad)), Err(Error::OpcodeNotAllowed(b)) if b == bad)
            );
        }
        // Write-shaped frames (payload, valid Hall chunk header) are refused
        // for every opcode, including 0x27, on the read-only transport.
        for op in 0u8..=255 {
            let mut f = [0u8; 64];
            f[..8].copy_from_slice(&[0xAA, op, 0x38, 0, 0, 0, 0, 0]);
            f[8..].fill(0x5A);
            assert!(
                t.write_frame(&f).is_err(),
                "payload frame, opcode {op:#04x}"
            );
        }
        let written = t.dev.written.borrow();
        assert_eq!(written.len(), 10, "only the ten read opcodes were written");
        for w in written.iter() {
            assert_eq!(w.len(), 65);
            assert_eq!(w[0], 0x00, "report ID prefix");
            assert_eq!(w[1], 0xAA);
            assert!(is_allowed_read_opcode(w[2]));
        }
    }

    #[test]
    fn malformed_read_frames_are_refused() {
        let mut t = ReadOnlyTransport::new(MockDev::default());
        let mut f = encode_read_request(ReadOp::GameMode, 56, 0, true);
        f[8] = 1; // payload data on a read = looks like a write
        assert!(t.write_frame(&f).is_err());
        let mut f = encode_read_request(ReadOp::GameMode, 56, 0, true);
        f[0] = 0x55;
        assert!(t.write_frame(&f).is_err());
        let mut f = encode_read_request(ReadOp::GameMode, 56, 0, true);
        f[5] = 1;
        assert!(t.write_frame(&f).is_err());
        // The 8-byte ISP magic frames start with 0xAA 0x42/0x55: refused by opcode.
        let mut f = [0u8; 64];
        f[..8].copy_from_slice(&[0xAA, 0x55, 0xA5, 0x5A, 0xFF, 0x00, 0x33, 0xCC]);
        assert!(t.write_frame(&f).is_err());
        assert!(t.dev.written.borrow().is_empty());
        assert!(t
            .write_frame(&encode_read_request(ReadOp::GameMode, 56, 0, true))
            .is_ok());
    }

    fn iface(path: &str, page: u16, usage: u16) -> Interface {
        Interface {
            path: CString::new(path).unwrap(),
            usage_page: page,
            usage,
            interface_number: 0,
            product: None,
            manufacturer: None,
            serial: None,
            release: 0,
        }
    }

    #[test]
    fn selection_policy() {
        let ok = vec![
            iface("/dev/hidraw0", 0x0001, 6),
            iface("/dev/hidraw1", 0x000C, 1),
            iface("/dev/hidraw6", 0xFF68, 0x61),
        ];
        assert_eq!(select_config(&ok).unwrap().path_str(), "/dev/hidraw6");
        assert!(matches!(select_config(&ok[..2]), Err(Error::NotFound)));
        let shared = vec![
            iface("/dev/hidraw6", 0xFF67, 0x61),
            iface("/dev/hidraw6", 0xFF68, 0x61),
        ];
        assert!(matches!(
            select_config(&shared),
            Err(Error::SharedNode { other: 0xFF67, .. })
        ));
        let two = vec![
            iface("/dev/hidraw6", 0xFF68, 0x61),
            iface("/dev/hidraw9", 0xFF68, 0x61),
        ];
        assert!(matches!(select_config(&two), Err(Error::Ambiguous(2))));
        let wrong_usage = vec![iface("/dev/hidraw6", 0xFF68, 0x62)];
        assert!(matches!(select_config(&wrong_usage), Err(Error::NotFound)));
    }
}
