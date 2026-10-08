use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPacket {
    /// RTP timestamp (in sample units, e.g. increments by 160 per 10ms frame at 16kHz)
    pub timestamp: u32,
    /// 16-bit packet sequence number
    pub sequence_number: u16,
    /// 16-bit PCM audio samples
    pub payload: Vec<i16>,
    /// Payload type (e.g. 0 for PCMU, 111 for Opus, etc.)
    pub payload_type: u8,
}

impl AudioPacket {
    pub fn new(timestamp: u32, sequence_number: u16, payload: Vec<i16>) -> Self {
        Self {
            timestamp,
            sequence_number,
            payload,
            payload_type: 0,
        }
    }

    pub fn num_samples(&self) -> usize {
        self.payload.len()
    }
}

impl PartialEq for AudioPacket {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp && self.sequence_number == other.sequence_number
    }
}

impl Eq for AudioPacket {}

impl PartialOrd for AudioPacket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AudioPacket {
    fn cmp(&self, other: &Self) -> Ordering {
        // Handle 32-bit timestamp wraparound logic if needed, otherwise linear ordering
        self.timestamp.cmp(&other.timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_ordering() {
        let p1 = AudioPacket::new(160, 1, vec![0; 160]);
        let p2 = AudioPacket::new(320, 2, vec![0; 160]);
        assert!(p1 < p2);
    }
}
