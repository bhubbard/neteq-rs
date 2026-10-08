use crate::decision_logic::NetEqOperation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetEqStats {
    /// Total audio packets inserted into the jitter buffer
    pub packets_received: u64,
    /// Late or duplicate packets discarded
    pub packets_discarded: u64,
    /// Playout operations count
    pub normal_frames: u64,
    pub accelerated_frames: u64,
    pub preemptive_expanded_frames: u64,
    pub concealed_frames: u64,
    pub comfort_noise_frames: u64,
    pub merged_frames: u64,

    /// Current target delay in milliseconds
    pub target_delay_ms: f64,
    /// Estimated current buffer delay in milliseconds
    pub current_delay_ms: f64,
    /// Packet loss / concealment rate (0.0 to 1.0)
    pub packet_loss_rate: f64,
    /// Acceleration rate (0.0 to 1.0)
    pub accelerate_rate: f64,
    /// Preemptive expand rate (0.0 to 1.0)
    pub expand_rate: f64,
}

pub struct StatisticsTracker {
    packets_received: u64,
    packets_discarded: u64,
    normal_frames: u64,
    accelerated_frames: u64,
    preemptive_expanded_frames: u64,
    concealed_frames: u64,
    comfort_noise_frames: u64,
    merged_frames: u64,
    sample_rate: u32,
    frame_size_samples: usize,
}

impl StatisticsTracker {
    pub fn new(sample_rate: u32, frame_size_samples: usize) -> Self {
        Self {
            packets_received: 0,
            packets_discarded: 0,
            normal_frames: 0,
            accelerated_frames: 0,
            preemptive_expanded_frames: 0,
            concealed_frames: 0,
            comfort_noise_frames: 0,
            merged_frames: 0,
            sample_rate,
            frame_size_samples,
        }
    }

    pub fn record_packet_received(&mut self) {
        self.packets_received += 1;
    }

    pub fn record_packet_discarded(&mut self) {
        self.packets_discarded += 1;
    }

    pub fn record_operation(&mut self, op: NetEqOperation) {
        match op {
            NetEqOperation::Normal => self.normal_frames += 1,
            NetEqOperation::Accelerate => self.accelerated_frames += 1,
            NetEqOperation::PreemptiveExpand => self.preemptive_expanded_frames += 1,
            NetEqOperation::Expand => self.concealed_frames += 1,
            NetEqOperation::ComfortNoise => self.comfort_noise_frames += 1,
            NetEqOperation::Merge => self.merged_frames += 1,
        }
    }

    pub fn snapshot(
        &self,
        target_delay_frames: usize,
        current_buffer_frames: usize,
    ) -> NetEqStats {
        let total_frames = self.normal_frames
            + self.accelerated_frames
            + self.preemptive_expanded_frames
            + self.concealed_frames
            + self.comfort_noise_frames
            + self.merged_frames;

        let frame_ms = (self.frame_size_samples as f64 / self.sample_rate as f64) * 1000.0;
        let target_delay_ms = target_delay_frames as f64 * frame_ms;
        let current_delay_ms = current_buffer_frames as f64 * frame_ms;

        let (loss_rate, accelerate_rate, expand_rate) = if total_frames > 0 {
            let tf = total_frames as f64;
            (
                (self.concealed_frames + self.comfort_noise_frames) as f64 / tf,
                self.accelerated_frames as f64 / tf,
                self.preemptive_expanded_frames as f64 / tf,
            )
        } else {
            (0.0, 0.0, 0.0)
        };

        NetEqStats {
            packets_received: self.packets_received,
            packets_discarded: self.packets_discarded,
            normal_frames: self.normal_frames,
            accelerated_frames: self.accelerated_frames,
            preemptive_expanded_frames: self.preemptive_expanded_frames,
            concealed_frames: self.concealed_frames,
            comfort_noise_frames: self.comfort_noise_frames,
            merged_frames: self.merged_frames,
            target_delay_ms,
            current_delay_ms,
            packet_loss_rate: loss_rate,
            accelerate_rate,
            expand_rate,
        }
    }

    pub fn reset(&mut self) {
        self.packets_received = 0;
        self.packets_discarded = 0;
        self.normal_frames = 0;
        self.accelerated_frames = 0;
        self.preemptive_expanded_frames = 0;
        self.concealed_frames = 0;
        self.comfort_noise_frames = 0;
        self.merged_frames = 0;
    }
}
