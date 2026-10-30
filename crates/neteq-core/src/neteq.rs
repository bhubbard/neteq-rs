use crate::cng::{frame_rms, ComfortNoiseGenerator, VoiceActivityDetector};
use crate::decision_logic::{DecisionLogic, DecisionLogicConfig, NetEqOperation};
use crate::delay_manager::{DelayManager, DelayManagerConfig};
use crate::packet::AudioPacket;
use crate::packet_buffer::PacketBuffer;
use crate::plc::PacketLossConcealment;
use crate::statistics::{NetEqStats, StatisticsTracker};
use crate::sync_buffer::SyncBuffer;
use crate::time_stretch::TimeStretch;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetEqError {
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Buffer error: {0}")]
    BufferError(String),
    #[error("Output buffer error: {0}")]
    OutputError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetEqConfig {
    /// Audio sample rate in Hz (e.g. 16000, 24000, 48000)
    pub sample_rate: u32,
    /// Number of audio channels (typically 1 for voice)
    pub channels: usize,
    /// Frame size in milliseconds (typically 10ms or 20ms)
    pub frame_size_ms: usize,
    /// Delay manager configuration
    pub delay_config: DelayManagerConfig,
    /// Decision logic configuration
    pub decision_config: DecisionLogicConfig,
    /// Maximum number of packets retained in packet buffer
    pub max_packet_buffer_size: usize,
    /// Maximum history buffer duration in milliseconds
    pub max_history_ms: usize,
}

impl Default for NetEqConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            frame_size_ms: 10,
            delay_config: DelayManagerConfig::default(),
            decision_config: DecisionLogicConfig::default(),
            max_packet_buffer_size: 100,
            max_history_ms: 200,
        }
    }
}

pub struct NetEq {
    config: NetEqConfig,
    frame_size_samples: usize,
    packet_buffer: PacketBuffer,
    sync_buffer: SyncBuffer,
    delay_manager: DelayManager,
    decision_logic: DecisionLogic,
    time_stretch: TimeStretch,
    plc: PacketLossConcealment,
    cng: ComfortNoiseGenerator,
    vad: VoiceActivityDetector,
    statistics: StatisticsTracker,
    playout_timestamp: u32,
    initialized: bool,
    tick: u64,
    last_op: NetEqOperation,
}

impl NetEq {
    /// Create a new NetEQ engine instance
    pub fn new(config: NetEqConfig) -> Result<Self, NetEqError> {
        if config.sample_rate == 0 {
            return Err(NetEqError::InvalidConfig("sample_rate must be > 0".into()));
        }
        if config.frame_size_ms == 0 {
            return Err(NetEqError::InvalidConfig("frame_size_ms must be > 0".into()));
        }

        let frame_size_samples = ((config.sample_rate as usize) * config.frame_size_ms) / 1000;
        let max_history_samples = ((config.sample_rate as usize) * config.max_history_ms) / 1000;

        Ok(Self {
            frame_size_samples,
            packet_buffer: PacketBuffer::new(config.max_packet_buffer_size),
            sync_buffer: SyncBuffer::new(max_history_samples),
            delay_manager: DelayManager::new(config.delay_config.clone()),
            decision_logic: DecisionLogic::new(config.decision_config.clone()),
            time_stretch: TimeStretch::new(config.sample_rate),
            plc: PacketLossConcealment::new(config.sample_rate),
            cng: ComfortNoiseGenerator::new(1337),
            vad: VoiceActivityDetector::default(),
            statistics: StatisticsTracker::new(config.sample_rate, frame_size_samples),
            playout_timestamp: 0,
            initialized: false,
            tick: 0,
            last_op: NetEqOperation::Normal,
            config,
        })
    }

    /// Insert an incoming audio packet into the jitter buffer
    pub fn insert_packet(&mut self, packet: AudioPacket) -> Result<(), NetEqError> {
        self.statistics.record_packet_received();
        self.delay_manager.update_packet_arrival_with_rtp(
            self.tick,
            packet.timestamp,
            self.frame_size_samples,
        );

        // If this is an explicit RFC 3389 SID packet, update comfort noise floor
        if packet.is_sid {
            if packet.payload.len() == 1 {
                // Single-sample RMS indicator or level
                self.cng.update_from_sid_rms(packet.payload[0].unsigned_abs() as f32);
            } else if !packet.payload.is_empty() {
                self.cng.update_from_sid_rms(frame_rms(&packet.payload));
            }
            return Ok(());
        }

        if self.initialized {
            // Check if packet is too old (older than current playout timestamp)
            let diff = self.playout_timestamp.wrapping_sub(packet.timestamp);
            if diff > 0 && diff < (1 << 31) && diff > (self.frame_size_samples as u32 * 2) {
                // Packet arrived too late for playout; discard
                self.statistics.record_packet_discarded();
                return Ok(());
            }
        }

        self.packet_buffer
            .insert(packet)
            .map_err(|e| NetEqError::BufferError(e.to_string()))?;

        Ok(())
    }

    /// Extract `output.len()` 16-bit PCM audio samples.
    ///
    /// Dynamically resizes jitter buffer, time-stretches audio, or conceals lost frames as needed.
    pub fn get_audio(&mut self, output: &mut [i16]) -> Result<NetEqOperation, NetEqError> {
        if output.is_empty() {
            return Ok(self.last_op);
        }

        self.tick = self.tick.wrapping_add(1);

        // Keep generating audio frames until the sync buffer has enough samples
        while self.sync_buffer.future_len() < output.len() {
            let buffer_frames = self
                .packet_buffer
                .total_samples()
                .checked_div(self.frame_size_samples)
                .unwrap_or(0);

            // First packet initialization
            if !self.initialized {
                if let Some(front_ts) = self.packet_buffer.peek_timestamp() {
                    self.playout_timestamp = front_ts;
                    self.initialized = true;
                } else {
                    // No packets received yet: output silence or comfort noise
                    let noise = self.cng.generate(self.frame_size_samples);
                    self.sync_buffer.push_future(&noise);
                    self.last_op = NetEqOperation::ComfortNoise;
                    self.statistics.record_operation(self.last_op);
                    continue;
                }
            }

            // Flush packets older than playout timestamp
            let flushed = self.packet_buffer.flush_older_than(self.playout_timestamp);
            for _ in 0..flushed {
                self.statistics.record_packet_discarded();
            }

            // Check if packet matching playout timestamp is available
            let has_packet = match self.packet_buffer.peek_timestamp() {
                Some(ts) => {
                    let diff = ts.wrapping_sub(self.playout_timestamp);
                    let tolerance = if matches!(
                        self.last_op,
                        NetEqOperation::Expand | NetEqOperation::ComfortNoise
                    ) {
                        (self.decision_logic.consecutive_expands() as u32)
                            .saturating_mul(self.frame_size_samples as u32)
                            .saturating_add(self.config.decision_config.timestamp_tolerance)
                    } else {
                        (self.frame_size_samples as u32)
                            .saturating_sub(1)
                            .min(self.config.decision_config.timestamp_tolerance)
                    };
                    diff <= tolerance
                }
                None => false,
            };

            let op = self.decision_logic.get_decision(
                has_packet,
                &self.delay_manager,
                buffer_frames,
            );
            self.last_op = op;
            self.statistics.record_operation(op);

            match op {
                NetEqOperation::Normal => {
                    if let Some(packet) = self.packet_buffer.get_next_packet() {
                        if !self.vad.is_speech(&packet.payload) {
                            self.cng.update_energy(frame_rms(&packet.payload));
                        }
                        self.sync_buffer.push_future(&packet.payload);
                        self.playout_timestamp =
                            packet.timestamp.wrapping_add(packet.payload.len() as u32);
                        self.plc.reset();
                    }
                }
                NetEqOperation::Accelerate => {
                    if let Some(packet) = self.packet_buffer.get_next_packet() {
                        if !self.vad.is_speech(&packet.payload) {
                            self.cng.update_energy(frame_rms(&packet.payload));
                        }
                        let history = self.sync_buffer.all_history();
                        let target = self.delay_manager.target_delay_frames();
                        let accelerated = if buffer_frames > target.saturating_mul(2) + 4 {
                            self.time_stretch
                                .fast_accelerate(&history, &packet.payload)
                        } else {
                            self.time_stretch
                                .accelerate_with_history(&history, &packet.payload)
                        };
                        self.sync_buffer.push_future(&accelerated);
                        self.playout_timestamp =
                            packet.timestamp.wrapping_add(packet.payload.len() as u32);
                        self.plc.reset();
                    }
                }
                NetEqOperation::PreemptiveExpand => {
                    if let Some(packet) = self.packet_buffer.get_next_packet() {
                        if !self.vad.is_speech(&packet.payload) {
                            self.cng.update_energy(frame_rms(&packet.payload));
                        }
                        let history = self.sync_buffer.all_history();
                        let expanded = self
                            .time_stretch
                            .preemptive_expand_with_history(&history, &packet.payload);
                        self.sync_buffer.push_future(&expanded);
                        self.playout_timestamp =
                            packet.timestamp.wrapping_add(packet.payload.len() as u32);
                        self.plc.reset();
                    }
                }
                NetEqOperation::Expand => {
                    // Conceal lost frame using pitch continuation from history
                    let history = self.sync_buffer.all_history();
                    let concealed = self.plc.generate_concealment(&history, self.frame_size_samples);
                    self.sync_buffer.push_future(&concealed);
                    // Only advance playout_timestamp when bridging a gap toward an already-buffered
                    // future packet; when the buffer is empty (jitter underrun), hold playout_timestamp
                    // so delayed packets arriving shortly after are buffered rather than discarded as late.
                    if !self.packet_buffer.is_empty() {
                        self.playout_timestamp =
                            self.playout_timestamp.wrapping_add(self.frame_size_samples as u32);
                    }
                }
                NetEqOperation::ComfortNoise => {
                    let noise = self.cng.generate(self.frame_size_samples);
                    self.sync_buffer.push_future(&noise);
                    if !self.packet_buffer.is_empty() {
                        self.playout_timestamp =
                            self.playout_timestamp.wrapping_add(self.frame_size_samples as u32);
                    }
                }
                NetEqOperation::Merge => {
                    if let Some(packet) = self.packet_buffer.get_next_packet() {
                        if !self.vad.is_speech(&packet.payload) {
                            self.cng.update_energy(frame_rms(&packet.payload));
                        }
                        let merge_len = (self.frame_size_samples / 2).min(packet.payload.len());
                        let history = self.sync_buffer.get_history(merge_len);
                        let mut merged_payload = packet.payload.clone();

                        if history.len() == merge_len && merge_len > 0 {
                            for i in 0..merge_len {
                                let phase = (i as f32) / (merge_len as f32);
                                let alpha = 0.5 * (1.0 - (std::f32::consts::PI * phase).cos());
                                let blended = ((1.0 - alpha) * (history[i] as f32)
                                    + alpha * (packet.payload[i] as f32))
                                    .round() as i16;
                                merged_payload[i] = blended;
                            }
                        }

                        self.sync_buffer.push_future(&merged_payload);
                        self.playout_timestamp =
                            packet.timestamp.wrapping_add(packet.payload.len() as u32);
                        self.plc.reset();
                    }
                }
            }
        }

        // Extract requested samples into output buffer
        let extracted = self.sync_buffer.extract(output);
        if extracted < output.len() {
            // Pad remaining with silence if sync buffer somehow couldn't fill
            output[extracted..].fill(0);
        }

        Ok(self.last_op)
    }

    /// Extract `output.len()` floating-point audio samples (-1.0 to 1.0)
    pub fn get_audio_f32(&mut self, output: &mut [f32]) -> Result<NetEqOperation, NetEqError> {
        let mut temp_i16 = vec![0i16; output.len()];
        let op = self.get_audio(&mut temp_i16)?;

        for (src, dst) in temp_i16.iter().zip(output.iter_mut()) {
            *dst = (*src as f32) / 32768.0;
        }

        Ok(op)
    }

    /// Retrieve real-time performance statistics
    pub fn stats(&self) -> NetEqStats {
        let buffer_frames = self
            .packet_buffer
            .total_samples()
            .checked_div(self.frame_size_samples)
            .unwrap_or(0);

        self.statistics.snapshot_with_jitter(
            self.delay_manager.target_delay_frames(),
            buffer_frames,
            self.delay_manager.jitter_frames(),
        )
    }

    /// Set target delay frames manually (e.g. for testing or latency profile override)
    pub fn set_target_delay_frames(&mut self, frames: usize) {
        self.delay_manager.set_target_delay_frames(frames);
    }

    /// Reset NetEQ internal state while retaining configuration
    pub fn reset(&mut self) {
        self.packet_buffer.clear();
        self.sync_buffer.flush();
        self.plc.reset();
        self.decision_logic.reset();
        self.delay_manager.reset();
        self.statistics.reset();
        self.initialized = false;
        self.playout_timestamp = 0;
        self.tick = 0;
        self.last_op = NetEqOperation::Normal;
    }

    pub fn config(&self) -> &NetEqConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_neteq_normal_streaming() {
        let mut neteq = NetEq::new(NetEqConfig::default()).unwrap();
        let frame_samples = 160;

        // Insert 2 sequential packets (matching target delay of 2 frames)
        for i in 0..2 {
            let ts = (i * frame_samples) as u32;
            let packet = AudioPacket::new(ts, i as u16, vec![1000; frame_samples]);
            neteq.insert_packet(packet).unwrap();
        }

        let mut output = vec![0i16; frame_samples];
        let op = neteq.get_audio(&mut output).unwrap();

        assert_eq!(op, NetEqOperation::Normal);
        assert_eq!(output[0], 1000);

        // Now overload buffer with 10 packets: should trigger Accelerate to drain latency
        for i in 2..12 {
            let ts = (i * frame_samples) as u32;
            let packet = AudioPacket::new(ts, i as u16, vec![1000; frame_samples]);
            neteq.insert_packet(packet).unwrap();
        }
        let op_drain = neteq.get_audio(&mut output).unwrap();
        assert_eq!(op_drain, NetEqOperation::Accelerate);
    }

    #[test]
    fn test_neteq_packet_loss_and_recovery() {
        let mut neteq = NetEq::new(NetEqConfig::default()).unwrap();
        let frame_samples = 160;

        // Send packet 0
        neteq.insert_packet(AudioPacket::new(0, 0, vec![2000; frame_samples])).unwrap();

        let mut out = vec![0i16; frame_samples];
        neteq.get_audio(&mut out).unwrap();
        assert_eq!(out[0], 2000);

        // Don't send packet 1 (Packet Loss): get_audio should conceal (Expand)
        let op_conceal = neteq.get_audio(&mut out).unwrap();
        assert_eq!(op_conceal, NetEqOperation::Expand);

        // Now send packet 2: should Merge seamlessly
        neteq.insert_packet(AudioPacket::new(320, 2, vec![2000; frame_samples])).unwrap();
        let op_merge = neteq.get_audio(&mut out).unwrap();
        assert_eq!(op_merge, NetEqOperation::Merge);
    }

    #[test]
    fn test_neteq_prebuffered_gap_triggers_plc() {
        let mut neteq = NetEq::new(NetEqConfig::default()).unwrap();
        let frame_samples = 160;

        // Both Packet 0 (ts=0) and Packet 2 (ts=320) arrive before playout; Packet 1 (ts=160) was lost
        neteq
            .insert_packet(AudioPacket::new(0, 0, vec![3000; frame_samples]))
            .unwrap();
        neteq
            .insert_packet(AudioPacket::new(320, 2, vec![3000; frame_samples]))
            .unwrap();

        let mut out = vec![0i16; frame_samples];
        // Frame 0: Normal playout of Packet 0
        let op0 = neteq.get_audio(&mut out).unwrap();
        assert_eq!(op0, NetEqOperation::Normal);

        // Frame 1 (ts=160): Must conceal missing Packet 1 via Expand, NOT skip to Packet 2
        let op1 = neteq.get_audio(&mut out).unwrap();
        assert_eq!(op1, NetEqOperation::Expand);

        // Frame 2 (ts=320): Merges smoothly into Packet 2
        let op2 = neteq.get_audio(&mut out).unwrap();
        assert_eq!(op2, NetEqOperation::Merge);
    }

    #[test]
    fn test_neteq_sid_and_f32_output() {
        let mut neteq = NetEq::new(NetEqConfig::default()).unwrap();
        // Insert an RFC 3389 SID packet
        let sid = AudioPacket::new_sid(0, 0, 120);
        neteq.insert_packet(sid).unwrap();

        let mut out_f32 = vec![0.0f32; 160];
        let op = neteq.get_audio_f32(&mut out_f32).unwrap();
        assert_eq!(op, NetEqOperation::ComfortNoise);
        assert!(out_f32.iter().all(|&x| (-1.0..=1.0).contains(&x)));
    }
}
