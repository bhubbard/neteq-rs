use crate::packet::AudioPacket;
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

    /// Insert an audio packet into the buffer maintaining timestamp ordering
    pub fn insert(&mut self, packet: AudioPacket) -> Result<(), &'static str> {
        // Discard duplicates
        if self.buffer.iter().any(|p| p.timestamp == packet.timestamp) {
            return Err("Duplicate packet discarded");
        }

        if self.buffer.len() >= self.max_packets {
            // Buffer is full: discard oldest or reject
            self.buffer.pop_front();
        }

        // Find position to insert maintaining sorted order by timestamp
        let pos = self
            .buffer
            .binary_search_by(|p| p.timestamp.cmp(&packet.timestamp))
            .unwrap_or_else(|e| e);

        self.buffer.insert(pos, packet);
        Ok(())
    }

    /// Peek the oldest packet timestamp in the buffer
    pub fn peek_timestamp(&self) -> Option<u32> {
        self.buffer.front().map(|p| p.timestamp)
    }

    /// Check if buffer contains a packet matching or ready for `playout_ts`
    pub fn get_next_packet(&mut self) -> Option<AudioPacket> {
        self.buffer.pop_front()
    }

    /// Retrieve packet at specific timestamp
    pub fn extract_packet_at(&mut self, timestamp: u32) -> Option<AudioPacket> {
        if let Some(pos) = self.buffer.iter().position(|p| p.timestamp == timestamp) {
            Some(self.buffer.remove(pos).unwrap())
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
        self.buffer.iter().map(|p| p.num_samples()).sum()
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
}
