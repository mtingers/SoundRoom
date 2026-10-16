use super::{Interest, PeerTls, Waiter};
use crate::{service::DeliveryLease, Error};
use soundroom_client::websocket::server_frame;
use std::{
    io::Write,
    time::{Duration, Instant},
};
/// One active frame, charged to the outbox until completion or destruction.
pub(super) struct FrameWrite {
    bytes: Vec<u8>,
    offset: usize,
    end: usize,
    wanted: Interest,
    deadline: Instant,
    _lease: Option<DeliveryLease>,
}

impl FrameWrite {
    pub(super) fn new(
        opcode: u8,
        payload: &[u8],
        lease: Option<DeliveryLease>,
    ) -> Result<Self, Error> {
        let bytes = server_frame(opcode, payload)?;
        let end = bytes.len().min(16384);
        Ok(Self {
            bytes,
            offset: 0,
            end,
            wanted: Interest::Write,
            deadline: Instant::now() + Duration::from_secs(2),
            _lease: lease,
        })
    }
    pub(super) fn wanted(&self) -> Interest {
        self.wanted
    }
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn progress(&mut self, stream: &mut PeerTls) -> Result<bool, Error> {
        if Instant::now() >= self.deadline {
            return Err(Error::Timeout);
        }
        if self.offset == self.bytes.len() {
            return Ok(true);
        }
        self.step(|bytes| {
            stream.get_ref().reset();
            let result = stream.write(bytes);
            let interest = stream.get_ref().blocked(Interest::Write);
            (result, interest)
        })
    }
    fn step(
        &mut self,
        write: impl FnOnce(&[u8]) -> (std::io::Result<usize>, Interest),
    ) -> Result<bool, Error> {
        // A WouldBlock retry uses exactly the same plaintext slice. Track
        // underlying TLS reads/writes so WANT_READ cannot busy-poll POLLOUT.
        let (result, wanted) = write(&self.bytes[self.offset..self.end]);
        match result {
            Ok(0) => Err(Error::Disconnected),
            Ok(count) => {
                self.offset += count;
                self.end = (self.offset + 16384).min(self.bytes.len());
                self.wanted = Interest::Write;
                Ok(self.offset == self.bytes.len())
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                self.wanted = wanted;
                Ok(false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn finish(
        &mut self,
        stream: &mut PeerTls,
        waiter: &Waiter,
        deadline: Instant,
    ) -> Result<(), Error> {
        self.deadline = self.deadline.min(deadline);
        loop {
            if self.progress(stream)? {
                return Ok(());
            }
            waiter.wait(
                stream.get_ref(),
                self.wanted == Interest::Read,
                self.wanted == Interest::Write,
                self.deadline,
            )?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[test]
    fn blocked_tls_retry_keeps_exact_slice_and_underlying_read_interest() {
        let mut frame = FrameWrite::new(2, b"abcdef", None).unwrap();
        let original = frame.bytes.clone();
        assert!(!frame
            .step(|bytes| {
                assert_eq!(bytes, original);
                (Err(std::io::ErrorKind::WouldBlock.into()), Interest::Read)
            })
            .unwrap());
        assert_eq!(frame.wanted(), Interest::Read);
        assert!(!frame
            .step(|bytes| {
                assert_eq!(bytes, original);
                (Ok(3), Interest::Write)
            })
            .unwrap());
        assert!(frame
            .step(|bytes| {
                assert_eq!(bytes, &original[3..]);
                (Ok(bytes.len()), Interest::Write)
            })
            .unwrap());
    }
    #[test]
    fn active_frame_retains_original_queue_charge_until_destroyed() {
        let global = Arc::new(AtomicUsize::new(0));
        let (out, inbox) = crate::service::Outbox::pair(global.clone());
        out.send_bytes(vec![1; 32], true).unwrap();
        let data = inbox.priority.try_recv().unwrap();
        let mut frame = FrameWrite::new(2, &data, Some(inbox.delivery_lease(data.len()))).unwrap();
        drop(data);
        drop(inbox);
        assert_eq!(global.load(Ordering::Acquire), 32);
        frame
            .step(|_| (Err(std::io::ErrorKind::WouldBlock.into()), Interest::Write))
            .unwrap();
        assert_eq!(global.load(Ordering::Acquire), 32);
        drop(frame);
        assert_eq!(global.load(Ordering::Acquire), 0);
    }
}
