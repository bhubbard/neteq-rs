use crate::packet::{rtp_timestamp_cmp, AudioPacket};
use std::collections::VecDeque;

pub struct PacketBuffer {
    max_packets: usize,
    buffer: VecDeque<AudioPacket>,
}

impl PacketBuffer {
    pub fn new(max_packets: usize) -> Self {
        Self {
            max_packets,
            buffer: VecDeque::new(),
        }
    }

    /// Insert an audio packet into the buffer maintaining RFC 1982 wraparound-safe timestamp ordering
    pub fn insert(&mut self, packet: AudioPacket) -> Result<(), &'static str> {
        // Discard duplicates
        if self.buffer.iter().any(|p| p.timestamp == packet.timestamp && p.is_sid == packet.is_sid) {
            return Err("Duplicate packet discarded");
        }

        if self.buffer.len() >= self.max_packets {
            // Buffer is full: discard oldest packet
            self.buffer.pop_front();
        }

        // Find position to insert maintaining sorted order by wraparound-safe RTP timestamp
        let pos = self
            .buffer
            .binary_search_by(|p| rtp_timestamp_cmp(p.timestamp, packet.timestamp))
            .unwrap_or_else(|e| e);

        self.buffer.insert(pos, packet);
        Ok(())
    }

    /// Peek the oldest packet timestamp in the buffer
    pub fn peek_timestamp(&self) -> Option<u32> {
        self.buffer.front().map(|p| p.timestamp)
    }

    /// Peek a reference to the oldest packet in the buffer
    pub fn peek_packet(&self) -> Option<&AudioPacket> {
        self.buffer.front()
    }

    /// Pop the next packet from the front of the buffer
    pub fn get_next_packet(&mut self) -> Option<AudioPacket> {
        self.buffer.pop_front()
    }

    /// Retrieve packet at specific timestamp
    pub fn extract_packet_at(&mut self, timestamp: u32) -> Option<AudioPacket> {
        if let Some(pos) = self.buffer.iter().position(|p| p.timestamp == timestamp) {
            self.buffer.remove(pos)
        } else {
            None
        }
    }

    /// Flush packets with timestamps strictly older than `playout_ts`
    pub fn flush_older_than(&mut self, playout_ts: u32) -> usize {
        let mut flushed = 0;
        while let Some(front) = self.buffer.front() {
            // Considering wraparound safety for 32-bit timestamps:
            // if front.timestamp < playout_ts (within 2^31 window)
            let diff = playout_ts.wrapping_sub(front.timestamp);
            if diff > 0 && diff < (1 << 31) {
                self.buffer.pop_front();
                flushed += 1;
            } else {
                break;
            }
        }
        flushed
    }

    pub fn num_packets(&self) -> usize {
        self.buffer.len()
    }

    pub fn total_samples(&self) -> usize {
        self.buffer
            .iter()
            .filter(|p| !p.is_sid)
            .map(|p| p.num_samples())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_buffer_insert_and_order() {
        let mut pb = PacketBuffer::new(50);
        pb.insert(AudioPacket::new(320, 2, vec![0; 160])).unwrap();
        pb.insert(AudioPacket::new(160, 1, vec![0; 160])).unwrap(); // Out of order arrival
        pb.insert(AudioPacket::new(480, 3, vec![0; 160])).unwrap();

        assert_eq!(pb.num_packets(), 3);
        assert_eq!(pb.peek_timestamp(), Some(160));

        let p1 = pb.get_next_packet().unwrap();
        assert_eq!(p1.timestamp, 160);

        let p2 = pb.get_next_packet().unwrap();
        assert_eq!(p2.timestamp, 320);
    }

    #[test]
    fn test_packet_buffer_wraparound_ordering() {
        let mut pb = PacketBuffer::new(50);
        let ts0 = u32::MAX - 319;
        let ts1 = u32::MAX - 159;
        let ts2 = ts1.wrapping_add(160); // Wraps around past 0 -> 1

        pb.insert(AudioPacket::new(ts2, 102, vec![0; 160])).unwrap();
        pb.insert(AudioPacket::new(ts0, 100, vec![0; 160])).unwrap();
        pb.insert(AudioPacket::new(ts1, 101, vec![0; 160])).unwrap();

        assert_eq!(pb.get_next_packet().unwrap().timestamp, ts0);
        assert_eq!(pb.get_next_packet().unwrap().timestamp, ts1);
        assert_eq!(pb.get_next_packet().unwrap().timestamp, ts2);
    }
}
