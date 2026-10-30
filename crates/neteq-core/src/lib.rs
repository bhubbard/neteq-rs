//! # neteq-core
//!
//! A pure Rust implementation of WebRTC's NetEQ adaptive jitter buffer,
//! time-stretching (WSOLA), packet loss concealment (PLC), and comfort noise generation (CNG).
//!
//! Designed specifically for real-time audio applications such as streaming voice AI agents,
//! WebSockets audio pipelines, and VoIP endpoints without bringing in the massive C++ libWebRTC codebase.

pub mod cng;
pub mod decision_logic;
pub mod delay_manager;
pub mod metrics;
pub mod neteq;
pub mod packet;
pub mod packet_buffer;
pub mod plc;
pub mod statistics;
pub mod sync_buffer;
pub mod time_stretch;

pub use cng::{frame_rms, zero_crossing_rate, ComfortNoiseGenerator, VoiceActivityDetector};
pub use decision_logic::{DecisionLogic, DecisionLogicConfig, NetEqOperation};
pub use delay_manager::{DelayDecision, DelayManager, DelayManagerConfig};
pub use metrics::{
    analyze_audio_quality, detect_boundary_clicks, estimate_mos_lq, estimate_r_factor,
    segmental_snr_db, snr_db, AudioQualityReport,
};
pub use neteq::{NetEq, NetEqConfig, NetEqError};
pub use packet::{rtp_sequence_cmp, rtp_timestamp_cmp, AudioPacket};
pub use packet_buffer::PacketBuffer;
pub use plc::PacketLossConcealment;
pub use statistics::{NetEqStats, StatisticsTracker};
pub use sync_buffer::SyncBuffer;
pub use time_stretch::TimeStretch;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_jitter_buffer_pipeline() {
        let mut neteq = NetEq::new(NetEqConfig::default()).expect("Failed to initialize NetEq");
        let frame_size = 160; // 10ms at 16kHz

        // Generate synthetic 200 Hz tone
        let mut sine_frames = Vec::new();
        for f in 0..20 {
            let mut frame = Vec::with_capacity(frame_size);
            for s in 0..frame_size {
                let t = ((f * frame_size + s) as f32) / 16000.0;
                let sample = (f32::sin(2.0 * std::f32::consts::PI * 200.0 * t) * 15000.0) as i16;
                frame.push(sample);
            }
            sine_frames.push(frame);
        }

        // 1. Deliver first 5 frames sequentially
        for (i, frame) in sine_frames.iter().enumerate().take(5) {
            let ts = (i * frame_size) as u32;
            neteq
                .insert_packet(AudioPacket::new(ts, i as u16, frame.clone()))
                .expect("Failed to insert packet");
        }

        let mut out = vec![0i16; frame_size];

        // Read out first 3 frames (should be normal)
        for _ in 0..3 {
            let op = neteq.get_audio(&mut out).expect("Failed get_audio");
            assert!(op == NetEqOperation::Normal || op == NetEqOperation::Accelerate || op == NetEqOperation::PreemptiveExpand);
        }

        // 2. Introduce packet loss: skip frame 5, 6
        // Playout reads should trigger Packet Loss Concealment (Expand)
        for _ in 0..4 {
            let _ = neteq.get_audio(&mut out);
        }

        // 3. Resume streaming with frame 7 & 8
        for (i, frame) in sine_frames.iter().enumerate().take(10).skip(7) {
            let ts = (i * frame_size) as u32;
            let _ = neteq.insert_packet(AudioPacket::new(ts, i as u16, frame.clone()));
        }

        let op_resume = neteq.get_audio(&mut out).expect("Failed get_audio on resume");
        assert!(op_resume == NetEqOperation::Merge || op_resume == NetEqOperation::Normal);

        let stats = neteq.stats();
        assert!(stats.packets_received > 0);
        assert!(stats.concealed_frames > 0);
    }
}
