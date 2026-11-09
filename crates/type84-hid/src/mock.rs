//! Simulated board for tests of code built on this crate (feature `mock`).
//!
//! [`SimBoard`] keeps one image per read opcode and answers frames the way
//! the real board does on FW 1.17: reads return the image, write packets
//! update the image of their read opcode (write opcode - 0x10) and are
//! ACKed with the echoed header, and the effect reads back with byte 4 and
//! the check code zeroed, the colour as `FF FF FF` multicolour and custom
//! 128 as 20. Every handle from [`SimBoard::open`] has its own input queue,
//! like separate hidraw opens; the images are shared.
//!
//! It never touches a real device: it only implements [`RawHid`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use type84_proto::frame::Frame;
use type84_proto::WriteOp;

use crate::RawHid;

#[derive(Debug, Default)]
struct State {
    images: HashMap<u8, Vec<u8>>,
    /// Every write packet received (frames of write opcodes only).
    written: Vec<Frame>,
}

/// Shared simulated board.
#[derive(Debug, Clone, Default)]
pub struct SimBoard {
    state: Arc<Mutex<State>>,
}

impl SimBoard {
    /// A board with these images, keyed by read opcode (0x10 device info,
    /// 0x12 layer 0, 0x13 effect, ...). Missing opcodes read as zeros of
    /// their block size.
    pub fn new(mut images: HashMap<u8, Vec<u8>>) -> Self {
        for op in type84_proto::ReadOp::ALL {
            images
                .entry(op.code())
                .or_insert_with(|| vec![0u8; op.block_size()]);
        }
        SimBoard {
            state: Arc::new(Mutex::new(State {
                images,
                written: Vec::new(),
            })),
        }
    }

    /// A new handle (like opening the hidraw node again).
    pub fn open(&self) -> SimDev {
        SimDev {
            state: self.state.clone(),
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// The current image of a read opcode.
    pub fn image(&self, read_op: u8) -> Vec<u8> {
        self.state.lock().unwrap().images[&read_op].clone()
    }

    /// Replace an image (e.g. the case switch rewriting the Hall table).
    pub fn set_image(&self, read_op: u8, data: Vec<u8>) {
        self.state.lock().unwrap().images.insert(read_op, data);
    }

    /// Number of write packets received so far.
    pub fn write_packets(&self) -> usize {
        self.state.lock().unwrap().written.len()
    }

    /// Write opcodes received, in order, one entry per packet.
    pub fn write_opcodes(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap()
            .written
            .iter()
            .map(|f| f[1])
            .collect()
    }
}

/// One open handle on a [`SimBoard`].
#[derive(Debug)]
pub struct SimDev {
    state: Arc<Mutex<State>>,
    queue: Mutex<VecDeque<Frame>>,
}

impl RawHid for SimDev {
    fn write(&self, data: &[u8]) -> Result<usize, hidapi::HidError> {
        assert_eq!(data.len(), 65, "report id + 64 bytes");
        assert_eq!(data[0], 0);
        let mut f = [0u8; 64];
        f.copy_from_slice(&data[1..]);
        let (op, len) = (f[1], f[2] as usize);
        let addr = u16::from_le_bytes([f[3], f[4]]) as usize;
        let mut r = f;
        r[0] = 0x55;
        let mut st = self.state.lock().unwrap();
        if WriteOp::from_code(op).is_some() {
            st.written.push(f);
            let img = st
                .images
                .get_mut(&(op - 0x10))
                .expect("image of the write opcode");
            img[addr..addr + len].copy_from_slice(&f[8..8 + len]);
            if op == 0x23 {
                for k in [4, 13, 14, 15] {
                    img[k] = 0;
                }
                img[1..4].copy_from_slice(&[0xFF; 3]);
                img[8] = 1;
                if img[0] == 128 {
                    img[0] = 20;
                }
            }
        } else {
            let img = st.images.get(&op).expect("read opcode image");
            r[8..8 + len].copy_from_slice(&img[addr..addr + len]);
        }
        drop(st);
        self.queue.lock().unwrap().push_back(r);
        Ok(65)
    }

    fn read_timeout(&self, buf: &mut [u8], _timeout_ms: i32) -> Result<usize, hidapi::HidError> {
        match self.queue.lock().unwrap().pop_front() {
            Some(f) => {
                buf[..64].copy_from_slice(&f);
                Ok(64)
            }
            None => Ok(0),
        }
    }
}
