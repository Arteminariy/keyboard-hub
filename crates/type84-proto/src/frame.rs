//! 64-byte frame encoding and decoding (PROTOCOL.md §1.4, §1.5, §2.3).

use crate::opcode::{ReadOp, WriteOp};

pub const FRAME_LEN: usize = 64;
pub const HEADER_LEN: usize = 8;
pub const PAYLOAD_LEN: usize = FRAME_LEN - HEADER_LEN;

pub const REQUEST_MARK: u8 = 0xAA;
pub const RESPONSE_MARK: u8 = 0x55;

pub type Frame = [u8; FRAME_LEN];

/// Build a read request frame. Read requests always carry an all-zero
/// payload. `len` is the number of payload bytes requested in this packet.
pub fn encode_read_request(op: ReadOp, len: u8, addr: u16, last: bool) -> Frame {
    assert!(
        len as usize <= PAYLOAD_LEN,
        "packet payload length exceeds 56"
    );
    let mut f = [0u8; FRAME_LEN];
    f[0] = REQUEST_MARK;
    f[1] = op.code();
    f[2] = len;
    f[3] = (addr & 0xFF) as u8;
    f[4] = (addr >> 8) as u8;
    f[6] = last as u8;
    f
}

/// Build a write request frame: same header as a read, with `payload`
/// (at most 56 bytes) copied to bytes 8.. and the rest zero-padded.
/// `len` is `payload.len()`.
pub fn encode_write_request(op: WriteOp, addr: u16, last: bool, payload: &[u8]) -> Frame {
    assert!(
        !payload.is_empty() && payload.len() <= PAYLOAD_LEN,
        "write packet payload must be 1..=56 bytes"
    );
    let mut f = [0u8; FRAME_LEN];
    f[0] = REQUEST_MARK;
    f[1] = op.code();
    f[2] = payload.len() as u8;
    f[3] = (addr & 0xFF) as u8;
    f[4] = (addr >> 8) as u8;
    f[6] = last as u8;
    f[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    f
}

/// Header of a response frame (`55 cmd len addr_lo addr_hi h5 h6 h7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHeader {
    pub cmd: u8,
    pub len: u8,
    pub addr: u16,
    pub last: bool,
}

/// Unsolicited frames the board may push at any time (PROTOCOL.md §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    Wake,
    LinkLost,
    DeviceReset,
    Sleep,
    Power {
        battery: u8,
        charging: u8,
    },
    ScreenInfo {
        wanted: bool,
    },
    Calibration,
    /// A 0x55 0xFA/0xFC frame with an unknown type byte.
    OtherDevice {
        cmd: u8,
        kind: u8,
    },
}

/// Classification of any input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Response(ResponseHeader),
    Notification(Notification),
    /// Anything that is neither a response nor a known notification.
    Unknown,
}

pub const NOTIFY_DEVICE: u8 = 0xFA;
pub const NOTIFY_CALIBRATION: u8 = 0xFB;
pub const NOTIFY_LINK: u8 = 0xFC;

pub fn classify_input(f: &[u8]) -> Input {
    if f.len() < 3 {
        return Input::Unknown;
    }
    if f[0] == 0xA6 && f[1] == 0xFF && f[2] == 0x01 {
        return Input::Notification(Notification::Wake);
    }
    if f[0] != RESPONSE_MARK {
        return Input::Unknown;
    }
    match f[1] {
        NOTIFY_LINK => Input::Notification(match f[2] {
            4 => Notification::LinkLost,
            5 => Notification::DeviceReset,
            6 => Notification::Sleep,
            k => Notification::OtherDevice {
                cmd: NOTIFY_LINK,
                kind: k,
            },
        }),
        NOTIFY_DEVICE => Input::Notification(match f[2] {
            2 if f.len() >= 7 => Notification::Power {
                battery: f[5],
                charging: f[6],
            },
            6 if f.len() >= 4 => Notification::ScreenInfo { wanted: f[3] == 1 },
            k => Notification::OtherDevice {
                cmd: NOTIFY_DEVICE,
                kind: k,
            },
        }),
        NOTIFY_CALIBRATION => Input::Notification(Notification::Calibration),
        cmd => {
            if f.len() < HEADER_LEN {
                return Input::Unknown;
            }
            Input::Response(ResponseHeader {
                cmd,
                len: f[2],
                addr: u16::from_le_bytes([f[3], f[4]]),
                last: f[6] != 0,
            })
        }
    }
}

/// Payload slice (bytes 8..8+len) of a response frame.
pub fn response_payload(f: &Frame, len: usize) -> &[u8] {
    &f[HEADER_LEN..HEADER_LEN + len.min(PAYLOAD_LEN)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_request_matches_doc_example() {
        let f = encode_read_request(ReadOp::DeviceInfo, 48, 0, true);
        assert_eq!(&f[..8], &[0xAA, 0x10, 0x30, 0x00, 0x00, 0x00, 0x01, 0x00]);
        assert!(f[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn classify_notifications() {
        assert_eq!(
            classify_input(&[0xA6, 0xFF, 0x01, 0]),
            Input::Notification(Notification::Wake)
        );
        let mut f = [0u8; 64];
        f[0] = 0x55;
        f[1] = 0xFC;
        f[2] = 5;
        assert_eq!(
            classify_input(&f),
            Input::Notification(Notification::DeviceReset)
        );
        f[1] = 0xFA;
        f[2] = 2;
        f[5] = 80;
        f[6] = 1;
        assert_eq!(
            classify_input(&f),
            Input::Notification(Notification::Power {
                battery: 80,
                charging: 1
            })
        );
        f[1] = 0xFB;
        assert_eq!(
            classify_input(&f),
            Input::Notification(Notification::Calibration)
        );
        f[0] = 0x00;
        assert_eq!(classify_input(&f), Input::Unknown);
    }

    #[test]
    fn classify_response_header() {
        let mut f = [0u8; 64];
        f[..8].copy_from_slice(&[0x55, 0x12, 0x08, 0xF8, 0x01, 0x00, 0x01, 0x00]);
        assert_eq!(
            classify_input(&f),
            Input::Response(ResponseHeader {
                cmd: 0x12,
                len: 8,
                addr: 0x1F8,
                last: true
            })
        );
    }
}
