use std::collections::VecDeque;

use rtp::{MediaTime, SeqNo, Ssrc};

use crate::{CodecPacketizer, PacketError, Packetizer};

pub struct Packetized {
    pub ts: MediaTime,
    pub data: Vec<u8>,
    pub first: bool,
    pub last: bool,
    pub ssrc: Ssrc,

    /// Set when packet is first sent. This is so we can resend.
    pub seq_no: Option<SeqNo>,
}

pub struct PacketizingBuffer {
    pack: CodecPacketizer,
    queue: VecDeque<Packetized>,
    emit_next: usize,
    max_retain: usize,
}

impl PacketizingBuffer {
    pub fn new(pack: CodecPacketizer, max_retain: usize) -> Self {
        PacketizingBuffer {
            pack,
            queue: VecDeque::new(),
            emit_next: 0,
            max_retain,
        }
    }

    pub fn push_sample(
        &mut self,
        ts: MediaTime,
        data: &[u8],
        ssrc: Ssrc,
        mtu: usize,
    ) -> Result<(), PacketError> {
        let chunks = self.pack.packetize(mtu, data)?;
        let len = chunks.len();

        assert!(len <= self.max_retain, "Must retain at least chunked count");

        for (idx, data) in chunks.into_iter().enumerate() {
            let first = idx == 0;
            let last = idx == len - 1;

            let rtp = Packetized {
                ts,
                data,
                first,
                last,
                ssrc,
                seq_no: None,
            };

            self.queue.push_back(rtp);
        }

        // Scale back retained count to max_retain
        while self.queue.len() > self.max_retain {
            self.queue.pop_front();
            self.emit_next -= 1;
        }

        Ok(())
    }

    pub fn poll_next(&mut self) -> Option<&mut Packetized> {
        let next = self.queue.get_mut(self.emit_next)?;
        self.emit_next += 1;
        Some(next)
    }

    pub fn get(&self, seq_no: SeqNo) -> Option<&Packetized> {
        self.queue.iter().find(|r| r.seq_no == Some(seq_no))
    }
}
