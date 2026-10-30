use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// RFC 1982 serial-number arithmetic comparison for 32-bit RTP timestamps.
/// Correctly orders timestamps across `u32::MAX -> 0` wraparound within a $2^{31}$ window.
#[inline]
pub fn rtp_timestamp_cmp(a: u32, b: u32) -> Ordering {
    if a == b {
        Ordering::Equal
    } else {
        let diff = b.wrapping_sub(a);
        if diff > 0 && diff < (1u32 << 31) {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

/// RFC 1982 serial-number arithmetic comparison for 16-bit RTP sequence numbers.
/// Correctly orders sequence numbers across `u16::MAX -> 0` wraparound within a $2^{15}$ window.
#[inline]
pub fn rtp_sequence_cmp(a: u16, b: u16) -> Ordering {
    if a == b {
        Ordering::Equal
    } else {
        let diff = b.wrapping_sub(a);
        if diff > 0 && diff < (1u16 << 15) {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPacket {
    /// RTP timestamp (in sample units, e.g. increments by 160 per 10ms frame at 16kHz)
    pub timestamp: u32,
    /// 16-bit packet sequence number
    pub sequence_number: u16,
    /// 16-bit PCM audio samples
    pub payload: Vec<i16>,
    /// Payload type (e.g. 0 for PCMU, 13 for RFC 3389 CN/SID, 111 for Opus, etc.)
    pub payload_type: u8,
    /// Whether this packet is a Discontinuous Transmission (DTX) Silence Insertion Descriptor (SID)
    #[serde(default)]
    pub is_sid: bool,
}

impl AudioPacket {
    pub fn new(timestamp: u32, sequence_number: u16, payload: Vec<i16>) -> Self {
        Self {
            timestamp,
            sequence_number,
            payload,
            payload_type: 0,
            is_sid: false,
        }
    }

    /// Creates an RFC 3389 Silence Insertion Descriptor (SID) comfort-noise update packet.
    pub fn new_sid(timestamp: u32, sequence_number: u16, noise_rms: i16) -> Self {
        Self {
            timestamp,
            sequence_number,
            payload: vec![noise_rms],
            payload_type: 13,
            is_sid: true,
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
        match rtp_timestamp_cmp(self.timestamp, other.timestamp) {
            Ordering::Equal => rtp_sequence_cmp(self.sequence_number, other.sequence_number),
            ord => ord,
        }
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

    #[test]
    fn test_packet_ordering_rtp_wraparound() {
        let pre_wrap = AudioPacket::new(u32::MAX - 159, 65535, vec![0; 160]);
        let post_wrap = AudioPacket::new(1, 0, vec![0; 160]);
        assert!(
            pre_wrap < post_wrap,
            "Pre-wraparound RTP packet must sort before post-wraparound packet"
        );
        assert!(post_wrap > pre_wrap);
    }
}
