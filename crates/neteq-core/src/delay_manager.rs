use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelayDecision {
    Normal,
    Accelerate,
    PreemptiveExpand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelayManagerConfig {
    /// Minimum allowed target delay in frames (e.g. 2 frames = 20ms at 10ms/frame)
    pub min_delay_frames: usize,
    /// Maximum allowed target delay in frames (e.g. 20 frames = 200ms)
    pub max_delay_frames: usize,
    /// Quantile threshold for jitter tracking (e.g. 95th percentile)
    pub percentile: f64,
}

impl Default for DelayManagerConfig {
    fn default() -> Self {
        Self {
            min_delay_frames: 2,
            max_delay_frames: 20,
            percentile: 0.95,
        }
    }
}

pub struct DelayManager {
    config: DelayManagerConfig,
    target_delay_frames: usize,
    iat_history: VecDeque<usize>, // Ring buffer of inter-arrival / relative transit delays in frames
    last_arrival_tick: Option<u64>,
    last_rtp_timestamp: Option<u32>,
    rfc3550_jitter_frames: f64,
}

impl DelayManager {
    pub fn new(config: DelayManagerConfig) -> Self {
        let initial_target = config.min_delay_frames;
        Self {
            config,
            target_delay_frames: initial_target,
            iat_history: VecDeque::with_capacity(100),
            last_arrival_tick: None,
            last_rtp_timestamp: None,
            rfc3550_jitter_frames: 0.0,
        }
    }

    /// Record a packet arrival at the given tick/time
    pub fn update_packet_arrival(&mut self, current_tick: u64) {
        if let Some(last_tick) = self.last_arrival_tick {
            let iat = current_tick.saturating_sub(last_tick) as usize;
            let transit_diff = (iat as f64 - 1.0).abs();
            self.rfc3550_jitter_frames += (transit_diff - self.rfc3550_jitter_frames) / 16.0;
            self.push_iat_sample(iat);
        }
        self.last_arrival_tick = Some(current_tick);
    }

    /// Record a packet arrival with both playout tick and RTP timestamp to compute RFC 3550 relative transit jitter
    pub fn update_packet_arrival_with_rtp(
        &mut self,
        current_tick: u64,
        rtp_timestamp: u32,
        frame_size_samples: usize,
    ) {
        if let (Some(last_tick), Some(last_ts)) = (self.last_arrival_tick, self.last_rtp_timestamp)
        {
            let arrival_delta_frames = current_tick.saturating_sub(last_tick) as f64;
            let rtp_delta_samples = rtp_timestamp.wrapping_sub(last_ts);
            let rtp_delta_frames = if rtp_delta_samples < (1u32 << 31) && frame_size_samples > 0 {
                (rtp_delta_samples as f64) / (frame_size_samples as f64)
            } else {
                1.0
            };

            // RFC 3550: D(i-1, i) = (R_i - R_{i-1}) - (S_i - S_{i-1})
            let d = (arrival_delta_frames - rtp_delta_frames).abs();
            self.rfc3550_jitter_frames += (d - self.rfc3550_jitter_frames) / 16.0;

            let effective_iat = (arrival_delta_frames.max(d.ceil())).round() as usize;
            self.push_iat_sample(effective_iat);
        }
        self.last_arrival_tick = Some(current_tick);
        self.last_rtp_timestamp = Some(rtp_timestamp);
    }

    fn push_iat_sample(&mut self, iat: usize) {
        if self.iat_history.len() >= 100 {
            self.iat_history.pop_front();
        }
        self.iat_history.push_back(iat);
        self.recalculate_target_delay();
    }

    /// Recalculate target delay based on 95th percentile of inter-arrival delays
    fn recalculate_target_delay(&mut self) {
        if self.iat_history.is_empty() {
            return;
        }

        let mut sorted: Vec<usize> = self.iat_history.iter().copied().collect();
        sorted.sort_unstable();

        let idx = ((sorted.len() as f64) * self.config.percentile).floor() as usize;
        let p_val = sorted[idx.min(sorted.len() - 1)];

        let new_target =
            (p_val + 1).clamp(self.config.min_delay_frames, self.config.max_delay_frames);

        // Smooth adaptation: move gradually towards the new target
        if new_target > self.target_delay_frames {
            self.target_delay_frames += 1;
        } else if new_target < self.target_delay_frames
            && self.target_delay_frames > self.config.min_delay_frames
        {
            self.target_delay_frames -= 1;
        }
    }

    /// Evaluate current buffer occupancy and recommend operation
    pub fn evaluate_delay(&self, current_buffer_frames: usize) -> DelayDecision {
        if current_buffer_frames > self.target_delay_frames + 2 {
            // Buffer is significantly higher than target: accelerate to drain excess latency
            DelayDecision::Accelerate
        } else if current_buffer_frames < self.target_delay_frames.saturating_sub(1)
            && current_buffer_frames > 0
        {
            // Buffer is running lower than target: stretch audio to build up safety margin
            DelayDecision::PreemptiveExpand
        } else {
            DelayDecision::Normal
        }
    }

    pub fn target_delay_frames(&self) -> usize {
        self.target_delay_frames
    }

    pub fn set_target_delay_frames(&mut self, target: usize) {
        self.target_delay_frames =
            target.clamp(self.config.min_delay_frames, self.config.max_delay_frames);
    }

    /// RFC 3550 smoothed inter-arrival jitter estimate in frames
    pub fn jitter_frames(&self) -> f64 {
        self.rfc3550_jitter_frames
    }

    pub fn reset(&mut self) {
        self.target_delay_frames = self.config.min_delay_frames;
        self.iat_history.clear();
        self.last_arrival_tick = None;
        self.last_rtp_timestamp = None;
        self.rfc3550_jitter_frames = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_manager_recommendations() {
        let mut dm = DelayManager::new(DelayManagerConfig::default());
        dm.set_target_delay_frames(4);

        // Buffer = 8 (higher than target 4 + 2): should accelerate
        assert_eq!(dm.evaluate_delay(8), DelayDecision::Accelerate);

        // Buffer = 2 (lower than target 4 - 1): should preemptive expand
        assert_eq!(dm.evaluate_delay(2), DelayDecision::PreemptiveExpand);

        // Buffer = 4 (optimal): normal
        assert_eq!(dm.evaluate_delay(4), DelayDecision::Normal);
    }
}
