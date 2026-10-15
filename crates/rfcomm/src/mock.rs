//! Scripted link for tests.

use std::collections::VecDeque;
use std::time::Duration;

use crate::{ByteLink, RfcommError};

type Responder = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>;

/// Replays scripted incoming bytes and records every write. An optional responder is
/// called with each write and returns the chunks the "headset" answers with.
#[derive(Default)]
pub struct MockLink {
    incoming: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    responder: Option<Responder>,
    disconnected: bool,
}

impl MockLink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue bytes the next reads will return (one chunk per read at most).
    pub fn push_incoming(&mut self, bytes: Vec<u8>) {
        self.incoming.push_back(bytes);
    }

    /// Answer every write with the chunks `f` returns.
    pub fn set_responder(&mut self, f: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static) {
        self.responder = Some(Box::new(f));
    }

    /// After the queued bytes are read, reads and writes fail with `Disconnected`.
    pub fn disconnect(&mut self) {
        self.disconnected = true;
    }

    /// Every write so far, in order.
    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }
}

impl ByteLink for MockLink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), RfcommError> {
        if self.disconnected {
            return Err(RfcommError::Disconnected);
        }
        self.writes.push(bytes.to_vec());
        if let Some(r) = self.responder.as_mut() {
            self.incoming.extend(r(bytes));
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout_ms: u32) -> Result<usize, RfcommError> {
        if let Some(mut chunk) = self.incoming.pop_front() {
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            if n < chunk.len() {
                self.incoming.push_front(chunk.split_off(n));
            }
            return Ok(n);
        }
        if self.disconnected {
            return Err(RfcommError::Disconnected);
        }
        std::thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(2))));
        Ok(0)
    }
}
