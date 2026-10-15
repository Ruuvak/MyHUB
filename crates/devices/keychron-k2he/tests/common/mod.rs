//! Shared test helpers: a mock link that answers from the golden replies of the real
//! keyboard (fixtures of keychron-proto). Nothing here opens a device.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use keychron_k2he::{LinkError, READ_BUF_LEN, REPORT_LEN, RawLink};
use keychron_proto::Frame;

const READ: &str = include_str!("../../../../keychron-proto/tests/fixtures/k2he_read.txt");
const KEYMAP: &str = include_str!("../../../../keychron-proto/tests/fixtures/k2he_keymap.txt");
const HE: &str = include_str!("../../../../keychron-proto/tests/fixtures/k2he_he.txt");

pub fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

pub fn pad(bytes: &[u8]) -> Frame {
    let mut f = [0u8; 32];
    f[..bytes.len()].copy_from_slice(bytes);
    f
}

/// Answers to one write: the reports to queue, in order. The first argument is the
/// 0-based number of the write.
pub type Responder = Box<dyn FnMut(usize, &Frame) -> Vec<Vec<u8>>>;

/// What the mock link holds. The test keeps a [`Handle`] to it, because the session owns
/// the link and has no accessor for it.
#[derive(Default)]
pub struct Shared {
    pub inbox: VecDeque<Vec<u8>>,
    pub written: Vec<[u8; REPORT_LEN]>,
}

pub type Handle = Rc<RefCell<Shared>>;

pub struct Mock {
    shared: Handle,
    responder: Responder,
}

impl Mock {
    pub fn new(responder: impl FnMut(usize, &Frame) -> Vec<Vec<u8>> + 'static) -> Self {
        Mock {
            shared: Handle::default(),
            responder: Box::new(responder),
        }
    }

    /// Answers every request with its golden reply.
    pub fn golden() -> Self {
        Mock::new(|_, f| vec![golden(f).to_vec()])
    }

    pub fn handle(&self) -> Handle {
        Rc::clone(&self.shared)
    }
}

impl RawLink for Mock {
    fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), LinkError> {
        let n = self.shared.borrow().written.len();
        self.shared.borrow_mut().written.push(*report);
        let frame: Frame = report[1..].try_into().unwrap();
        let replies = (self.responder)(n, &frame);
        self.shared.borrow_mut().inbox.extend(replies);
        Ok(())
    }

    fn read_report(
        &mut self,
        buf: &mut [u8; READ_BUF_LEN],
        _timeout_ms: i32,
    ) -> Result<usize, LinkError> {
        match self.shared.borrow_mut().inbox.pop_front() {
            Some(r) => {
                buf[..r.len()].copy_from_slice(&r);
                Ok(r.len())
            }
            None => Ok(0),
        }
    }
}

/// The fixture reply for a request given as hex, e.g. `"a9 10"`.
pub fn reply_for(request: &str) -> Frame {
    golden(&pad(&hex(request)))
}

fn fixture_reply(frame: &Frame) -> Option<Frame> {
    READ.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .find_map(|l| {
            let (req, rep) = l.split_once("=>")?;
            let req = hex(req);
            (frame[..req.len()] == req[..] && frame[req.len()..].iter().all(|&b| b == 0))
                .then(|| pad(&hex(rep)))
        })
}

fn keymap_image() -> Vec<u8> {
    let mut img = Vec::new();
    for layer in 0..4 {
        for row in 0..6 {
            let key = format!("L{layer} R{row}:");
            let line = KEYMAP.lines().find(|l| l.starts_with(&key)).unwrap();
            for w in line[key.len()..].split_whitespace() {
                img.extend_from_slice(&u16::from_str_radix(w, 16).unwrap().to_be_bytes());
            }
        }
    }
    img
}

fn he_fact(name: &str) -> Vec<u8> {
    let key = format!("{name}:");
    let line = HE.lines().find(|l| l.starts_with(&key)).unwrap();
    hex(&line[key.len()..])
}

/// 860-byte profile buffer: global word from the golden facts (profile 1 is rapid
/// trigger), profile 1 named "Gaming" (UTF-16LE at offset 828), the rest zero.
fn profile_buffer(profile: u8) -> Vec<u8> {
    let mut b = vec![0u8; 860];
    let global = if profile == 1 {
        he_fact("global_profile1")
    } else {
        he_fact("global_profile0")
    };
    b[..4].copy_from_slice(&global);
    if profile == 1 {
        for (i, u) in "Gaming".encode_utf16().enumerate() {
            b[828 + 2 * i..830 + 2 * i].copy_from_slice(&u.to_le_bytes());
        }
    }
    b
}

/// The reply of the golden keyboard to any request the snapshot sends. Requests with a
/// fixture line get that line; chunked reads the fixtures only sample are built here.
pub fn golden(f: &Frame) -> Frame {
    if let Some(r) = fixture_reply(f) {
        // A9 12 fixture lines only echo the request; the buffer below carries the data.
        if !(f[0] == 0xA9 && f[1] == 0x12) {
            return r;
        }
    }
    let mut r = [0u8; 32];
    match (f[0], f[1]) {
        (0xA9, 0x12) => {
            let (profile, off, len) = (
                f[2],
                usize::from(f[3]) | usize::from(f[4]) << 8,
                usize::from(f[5]),
            );
            r[..6].copy_from_slice(&f[..6]);
            r[6..6 + len].copy_from_slice(&profile_buffer(profile)[off..off + len]);
        }
        (0x12, _) => {
            let (off, len) = (
                usize::from(f[1]) << 8 | usize::from(f[2]),
                usize::from(f[3]),
            );
            r[..4].copy_from_slice(&f[..4]);
            r[4..4 + len].copy_from_slice(&keymap_image()[off..off + len]);
        }
        (0xA8, 0x09) => {
            r[..2].copy_from_slice(&f[..2]);
            for t in r[3..3 + 3 * usize::from(f[3])].chunks_mut(3) {
                t.copy_from_slice(&[0x2B, 0xFF, 0xFF]);
            }
        }
        (0xA8, 0x0C | 0x0E) => r[..2].copy_from_slice(&f[..2]),
        _ => panic!("no golden reply for {:02x?}", &f[..6]),
    }
    r
}
