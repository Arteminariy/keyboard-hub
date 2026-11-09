//! Block transfer engine (PROTOCOL.md §1.6): reads, plus the write engine
//! used for the single allowed write (the Hall/RT table, §4.3).
//!
//! The engine is transport-agnostic: it talks to anything implementing
//! [`FrameIo`]. It sends one packet at a time, waits for the matching
//! response, skips notifications and stale frames, retries on timeout and
//! reassembles the payloads.

use std::error::Error as StdError;
use std::time::{Duration, Instant};

use crate::frame::{
    classify_input, encode_read_request, encode_write_request, Frame, Input, Notification,
    PAYLOAD_LEN,
};
use crate::opcode::{ReadOp, WriteOp};

/// Number of resends of the same packet after a timeout.
pub const MAX_RETRIES: u32 = 3;

/// Upper bound on frames discarded while draining stale input.
const MAX_DRAIN: usize = 256;

pub type IoError = Box<dyn StdError + Send + Sync + 'static>;

/// Minimal frame-level transport.
pub trait FrameIo {
    /// Send one 64-byte request frame.
    fn send_request(&mut self, frame: &Frame) -> Result<(), IoError>;
    /// Receive one input report. `Ok(None)` means the timeout expired.
    /// A zero timeout must not block.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, IoError>;
}

/// One packet of a planned transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub addr: u16,
    pub len: u8,
    pub last: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("transport error: {0}")]
    Io(#[source] IoError),
    #[error("{op} at addr {addr:#06x}: no matching response after {attempts} attempts")]
    Timeout {
        op: &'static str,
        addr: u16,
        attempts: u32,
    },
    #[error("{op} at addr {addr:#06x}: response length {got} differs from requested {want}")]
    LengthMismatch {
        op: &'static str,
        addr: u16,
        want: u8,
        got: u8,
    },
    #[error("device reported a reset during {op}; the read must be restarted")]
    DeviceReset { op: &'static str },
    #[error("invalid transfer range: start {start} size {size}")]
    InvalidRange { start: usize, size: usize },
}

/// Something worth logging during a transfer.
#[derive(Debug, Clone, Copy)]
pub enum TransferEvent<'a> {
    Sent(&'a Frame),
    Matched(&'a Frame),
    Skipped(&'a Frame, Input),
    Retry { op: ReadOp, addr: u16, attempt: u32 },
}

/// Result of a block read: reassembled data plus every matched response
/// frame, in order (kept for forensics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRead {
    pub data: Vec<u8>,
    pub frames: Vec<Frame>,
}

/// Split `size` bytes starting at `start` into packets of at most 56 bytes.
pub fn plan_chunks(start: usize, size: usize) -> Result<Vec<Chunk>, TransferError> {
    if size == 0 || start + size > 0x1_0000 {
        return Err(TransferError::InvalidRange { start, size });
    }
    let n = size.div_ceil(PAYLOAD_LEN);
    Ok((0..n)
        .map(|i| {
            let off = i * PAYLOAD_LEN;
            Chunk {
                addr: (start + off) as u16,
                len: (size - off).min(PAYLOAD_LEN) as u8,
                last: i + 1 == n,
            }
        })
        .collect())
}

/// Build every request frame of a block read (useful for tests and dry runs).
pub fn request_frames(op: ReadOp, start: usize, size: usize) -> Result<Vec<Frame>, TransferError> {
    Ok(plan_chunks(start, size)?
        .into_iter()
        .map(|c| encode_read_request(op, c.len, c.addr, c.last))
        .collect())
}

/// Discard any input already queued (stale responses, notifications).
pub fn drain(
    io: &mut dyn FrameIo,
    observer: &mut dyn FnMut(TransferEvent),
) -> Result<(), TransferError> {
    for _ in 0..MAX_DRAIN {
        match io.recv(Duration::ZERO).map_err(TransferError::Io)? {
            Some(f) => {
                let input = classify_input(&f);
                observer(TransferEvent::Skipped(&f, input));
            }
            None => break,
        }
    }
    Ok(())
}

/// Read `size` bytes of block `op` starting at `start`.
pub fn read_block(
    io: &mut dyn FrameIo,
    op: ReadOp,
    start: usize,
    size: usize,
    observer: &mut dyn FnMut(TransferEvent),
) -> Result<BlockRead, TransferError> {
    let chunks = plan_chunks(start, size)?;
    drain(io, observer)?;
    let mut data = Vec::with_capacity(size);
    let mut frames = Vec::with_capacity(chunks.len());
    for c in chunks {
        let req = encode_read_request(op, c.len, c.addr, c.last);
        let resp = exchange(io, op, &req, c, observer)?;
        data.extend_from_slice(&resp[8..8 + c.len as usize]);
        frames.push(resp);
    }
    Ok(BlockRead { data, frames })
}

/// Something worth logging during a block write.
#[derive(Debug, Clone, Copy)]
pub enum WriteEvent<'a> {
    Sent(&'a Frame),
    Acked(&'a Frame),
    Skipped(&'a Frame, Input),
    Retry {
        op: WriteOp,
        addr: u16,
        attempt: u32,
    },
}

/// Build every request frame of a block write: chunks of 56 bytes from
/// `start`, `len` = chunk size, `addr` = byte offset, byte 6 = 1 on the last.
pub fn write_frames(op: WriteOp, start: usize, data: &[u8]) -> Result<Vec<Frame>, TransferError> {
    Ok(plan_chunks(start, data.len())?
        .into_iter()
        .map(|c| {
            let off = c.addr as usize - start;
            encode_write_request(op, c.addr, c.last, &data[off..off + c.len as usize])
        })
        .collect())
}

/// Write `data` as block `op` starting at `start`, one packet in flight,
/// waiting for each ACK (`55 cmd len addr`). Returns the ACK frames.
///
/// A device-reset notification aborts immediately (no retry): the block may
/// then be partially written. Timeouts resend the same packet up to
/// [`MAX_RETRIES`] times, as the vendor hub does; resending a packet is
/// idempotent because it carries its own address and data.
pub fn write_block(
    io: &mut dyn FrameIo,
    op: WriteOp,
    start: usize,
    data: &[u8],
    observer: &mut dyn FnMut(WriteEvent),
) -> Result<Vec<Frame>, TransferError> {
    let frames = write_frames(op, start, data)?;
    for _ in 0..MAX_DRAIN {
        match io.recv(Duration::ZERO).map_err(TransferError::Io)? {
            Some(f) => observer(WriteEvent::Skipped(&f, classify_input(&f))),
            None => break,
        }
    }
    let mut acks = Vec::with_capacity(frames.len());
    for req in &frames {
        acks.push(write_exchange(io, op, req, observer)?);
    }
    Ok(acks)
}

fn write_exchange(
    io: &mut dyn FrameIo,
    op: WriteOp,
    req: &Frame,
    observer: &mut dyn FnMut(WriteEvent),
) -> Result<Frame, TransferError> {
    let addr = u16::from_le_bytes([req[3], req[4]]);
    let len = req[2];
    let timeout = op.timeout();
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            observer(WriteEvent::Retry { op, addr, attempt });
        }
        io.send_request(req).map_err(TransferError::Io)?;
        observer(WriteEvent::Sent(req));
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let Some(f) = io.recv(deadline - now).map_err(TransferError::Io)? else {
                break;
            };
            let input = classify_input(&f);
            match input {
                Input::Response(h)
                    if h.cmd == op.code() && (!op.checks_addr() || h.addr == addr) =>
                {
                    if h.len != len {
                        return Err(TransferError::LengthMismatch {
                            op: op.name(),
                            addr,
                            want: len,
                            got: h.len,
                        });
                    }
                    observer(WriteEvent::Acked(&f));
                    return Ok(f);
                }
                Input::Notification(Notification::DeviceReset) => {
                    observer(WriteEvent::Skipped(&f, input));
                    return Err(TransferError::DeviceReset { op: op.name() });
                }
                _ => observer(WriteEvent::Skipped(&f, input)),
            }
        }
    }
    Err(TransferError::Timeout {
        op: op.name(),
        addr,
        attempts: MAX_RETRIES + 1,
    })
}

fn exchange(
    io: &mut dyn FrameIo,
    op: ReadOp,
    req: &Frame,
    c: Chunk,
    observer: &mut dyn FnMut(TransferEvent),
) -> Result<Frame, TransferError> {
    let timeout = op.timeout();
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            observer(TransferEvent::Retry {
                op,
                addr: c.addr,
                attempt,
            });
        }
        io.send_request(req).map_err(TransferError::Io)?;
        observer(TransferEvent::Sent(req));
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let Some(f) = io.recv(deadline - now).map_err(TransferError::Io)? else {
                break;
            };
            let input = classify_input(&f);
            match input {
                Input::Response(h)
                    if h.cmd == op.code() && (!op.checks_addr() || h.addr == c.addr) =>
                {
                    if h.len != c.len {
                        return Err(TransferError::LengthMismatch {
                            op: op.name(),
                            addr: c.addr,
                            want: c.len,
                            got: h.len,
                        });
                    }
                    observer(TransferEvent::Matched(&f));
                    return Ok(f);
                }
                Input::Notification(Notification::DeviceReset) => {
                    observer(TransferEvent::Skipped(&f, input));
                    return Err(TransferError::DeviceReset { op: op.name() });
                }
                _ => observer(TransferEvent::Skipped(&f, input)),
            }
        }
    }
    Err(TransferError::Timeout {
        op: op.name(),
        addr: c.addr,
        attempts: MAX_RETRIES + 1,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted device: answers each request through a closure.
    pub struct MockIo<F: FnMut(&Frame) -> Vec<Frame>> {
        pub respond: F,
        pub queue: VecDeque<Frame>,
        pub sent: Vec<Frame>,
    }

    impl<F: FnMut(&Frame) -> Vec<Frame>> MockIo<F> {
        pub fn new(respond: F) -> Self {
            MockIo {
                respond,
                queue: VecDeque::new(),
                sent: Vec::new(),
            }
        }
    }

    impl<F: FnMut(&Frame) -> Vec<Frame>> FrameIo for MockIo<F> {
        fn send_request(&mut self, frame: &Frame) -> Result<(), IoError> {
            self.sent.push(*frame);
            let out = (self.respond)(frame);
            self.queue.extend(out);
            Ok(())
        }
        fn recv(&mut self, _timeout: Duration) -> Result<Option<Frame>, IoError> {
            Ok(self.queue.pop_front())
        }
    }

    /// Echo the header and fill the payload with a function of the absolute offset.
    pub fn echo_with(req: &Frame, fill: impl Fn(usize) -> u8) -> Frame {
        let mut r = [0u8; 64];
        r[..8].copy_from_slice(&req[..8]);
        r[0] = 0x55;
        let addr = u16::from_le_bytes([req[3], req[4]]) as usize;
        for i in 0..req[2] as usize {
            r[8 + i] = fill(addr + i);
        }
        r
    }

    fn noop(_: TransferEvent) {}

    #[test]
    fn chunk_plan_512_and_1024() {
        let p = plan_chunks(0, 512).unwrap();
        assert_eq!(p.len(), 10);
        assert_eq!(
            p[9],
            Chunk {
                addr: 0x1F8,
                len: 8,
                last: true
            }
        );
        assert!(p[..9].iter().all(|c| c.len == 56 && !c.last));
        let p = plan_chunks(0, 1024).unwrap();
        assert_eq!(p.len(), 19);
        assert_eq!(
            p[18],
            Chunk {
                addr: 0x3F0,
                len: 16,
                last: true
            }
        );
        assert!(plan_chunks(0xFFF0, 0x20).is_err());
        assert!(plan_chunks(0, 0).is_err());
    }

    #[test]
    fn reassembles_and_skips_noise() {
        let mut io = MockIo::new(|req: &Frame| {
            let mut noise = [0u8; 64];
            noise[0] = 0x55;
            noise[1] = 0xFA;
            noise[2] = 2;
            let mut stale = echo_with(req, |_| 0xEE);
            stale[1] = 0x11; // response for another opcode
            vec![noise, stale, echo_with(req, |o| (o % 251) as u8)]
        });
        let mut skipped = 0;
        let r = read_block(&mut io, ReadOp::AxisRt, 0, 1024, &mut |e| {
            if let TransferEvent::Skipped(..) = e {
                skipped += 1
            }
        })
        .unwrap();
        assert_eq!(r.data.len(), 1024);
        assert!(r
            .data
            .iter()
            .enumerate()
            .all(|(i, &b)| b == (i % 251) as u8));
        assert_eq!(r.frames.len(), 19);
        assert_eq!(skipped, 38);
    }

    #[test]
    fn wrong_addr_is_skipped_for_checked_opcodes() {
        let mut io = MockIo::new(|req: &Frame| {
            let mut wrong = echo_with(req, |_| 1);
            wrong[3] = wrong[3].wrapping_add(1);
            vec![wrong]
        });
        let err = read_block(&mut io, ReadOp::Keymap, 0, 16, &mut noop).unwrap_err();
        assert!(matches!(err, TransferError::Timeout { attempts: 4, .. }));
        assert_eq!(io.sent.len(), 4, "one send plus three retries");
    }

    #[test]
    fn device_reset_aborts() {
        let mut io = MockIo::new(|_req: &Frame| {
            let mut f = [0u8; 64];
            f[0] = 0x55;
            f[1] = 0xFC;
            f[2] = 5;
            vec![f]
        });
        let err = read_block(&mut io, ReadOp::GameMode, 0, 56, &mut noop).unwrap_err();
        assert!(matches!(err, TransferError::DeviceReset { .. }));
    }

    #[test]
    fn retry_then_success() {
        let mut calls = 0;
        let mut io = MockIo::new(move |req: &Frame| {
            calls += 1;
            if calls == 1 {
                vec![]
            } else {
                vec![echo_with(req, |_| 7)]
            }
        });
        let r = read_block(&mut io, ReadOp::LedEffect, 0, 16, &mut noop).unwrap();
        assert_eq!(r.data, vec![7; 16]);
        assert_eq!(io.sent.len(), 2);
    }

    #[test]
    fn write_block_sends_19_frames_and_waits_for_acks() {
        let data: Vec<u8> = (0..1024).map(|i| (i % 253) as u8).collect();
        let mut io = MockIo::new(|req: &Frame| {
            let mut ack = *req;
            ack[0] = 0x55;
            let mut noise = [0u8; 64];
            noise[0] = 0x55;
            noise[1] = 0xFA;
            noise[2] = 2;
            vec![noise, ack]
        });
        let acks = write_block(&mut io, WriteOp::AxisRt, 0, &data, &mut |_| {}).unwrap();
        assert_eq!(acks.len(), 19);
        assert_eq!(io.sent.len(), 19);
        let mut joined = Vec::new();
        for (i, f) in io.sent.iter().enumerate() {
            assert_eq!(f[0], 0xAA);
            assert_eq!(f[1], 0x27);
            let addr = u16::from_le_bytes([f[3], f[4]]) as usize;
            assert_eq!(addr, i * 56);
            assert_eq!(f[6], (i == 18) as u8);
            joined.extend_from_slice(&f[8..8 + f[2] as usize]);
        }
        assert_eq!(joined, data);
        assert_eq!(io.sent[18][2], 16);
        assert!(io.sent[18][8 + 16..].iter().all(|&b| b == 0), "zero padded");
    }

    #[test]
    fn write_block_aborts_on_reset_and_on_wrong_addr() {
        let data = vec![0u8; 1024];
        let mut n = 0;
        let mut io = MockIo::new(move |req: &Frame| {
            n += 1;
            if n == 3 {
                let mut f = [0u8; 64];
                f[0] = 0x55;
                f[1] = 0xFC;
                f[2] = 5;
                return vec![f];
            }
            let mut ack = *req;
            ack[0] = 0x55;
            vec![ack]
        });
        let err = write_block(&mut io, WriteOp::AxisRt, 0, &data, &mut |_| {}).unwrap_err();
        assert!(matches!(err, TransferError::DeviceReset { .. }));
        assert_eq!(io.sent.len(), 3, "no retry after a reset");

        let mut io = MockIo::new(|req: &Frame| {
            let mut ack = *req;
            ack[0] = 0x55;
            ack[3] ^= 1;
            vec![ack]
        });
        let err = write_block(&mut io, WriteOp::AxisRt, 0, &data, &mut |_| {}).unwrap_err();
        assert!(matches!(err, TransferError::Timeout { attempts: 4, .. }));
        assert_eq!(io.sent.len(), 4);
    }

    #[test]
    fn length_mismatch_is_an_error() {
        let mut io = MockIo::new(|req: &Frame| {
            let mut r = echo_with(req, |_| 0);
            r[2] = 10;
            vec![r]
        });
        let err = read_block(&mut io, ReadOp::GameMode, 0, 56, &mut noop).unwrap_err();
        assert!(matches!(
            err,
            TransferError::LengthMismatch {
                want: 56,
                got: 10,
                ..
            }
        ));
    }
}
