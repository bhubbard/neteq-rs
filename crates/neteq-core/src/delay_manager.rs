use serde::{Deserialize, Serialize};

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
    iat_history: Vec<usize>, // Inter-arrival time histogram / history in frames
    last_arrival_tick: Option<u64>,
}

impl DelayManager {
    pub fn new(config: DelayManagerConfig) -> Self {
        let initial_target = config.min_delay_frames;
        Self {
            config,
            target_delay_frames: initial_target,
            iat_history: Vec::with_capacity(100),
            last_arrival_tick: None,
        }
    }

    /// Record a packet arrival at the given tick/time
    pub fn update_packet_arrival(&mut self, current_tick: u64) {
        if let Some(last_tick) = self.last_arrival_tick {
            let iat = current_tick.saturating_sub(last_tick) as usize;
            if self.iat_history.len() >= 100 {
                self.iat_history.remove(0);
            }
            self.iat_history.push(iat);
            self.recalculate_target_delay();
        }
        self.last_arrival_tick = Some(current_tick);
    }

    /// Recalculate target delay based on 95th percentile of inter-arrival delays
    fn recalculate_target_delay(&mut self) {
        if self.iat_history.is_empty() {
            return;
        }

        let mut sorted = self.iat_history.clone();
        sorted.sort_unstable();

        let idx = ((sorted.len() as f64) * self.config.percentile).floor() as usize;
        let p_val = sorted[idx.min(sorted.len() - 1)];

        let new_target = (p_val + 1).clamp(self.config.min_delay_frames, self.config.max_delay_frames);

        // Smooth adaptation: move gradually towards the new target
        if new_target > self.target_delay_frames {
            self.target_delay_frames += 1;
        } else if new_target < self.target_delay_frames && self.target_delay_frames > self.config.min_delay_frames {
            self.target_delay_frames -= 1;
        }
    }

    /// Evaluate current buffer occupancy and recommend operation
    pub fn evaluate_delay(&self, current_buffer_frames: usize) -> DelayDecision {
        if current_buffer_frames > self.target_delay_frames + 2 {
            // Buffer is significantly higher than target: accelerate to drain excess latency
            DelayDecision::Accelerate
        } else if current_buffer_frames < self.target_delay_frames.saturating_sub(1) && current_buffer_frames > 0 {
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
        self.target_delay_frames = target.clamp(self.config.min_delay_frames, self.config.max_delay_frames);
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
