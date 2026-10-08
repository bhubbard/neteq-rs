use crate::delay_manager::{DelayDecision, DelayManager};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetEqOperation {
    /// Play next decoded frame directly
    Normal,
    /// WSOLA compress frame to reduce latency towards target
    Accelerate,
    /// WSOLA stretch frame to increase buffer cushion towards target
    PreemptiveExpand,
    /// Packet loss concealment: extrapolate voice pitch continuation
    Expand,
    /// Comfort noise generation during silence or extended packet loss
    ComfortNoise,
    /// Crossfade transition when recovering from concealment back to normal
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionLogicConfig {
    /// Maximum consecutive PLC expand operations before falling back to comfort noise
    pub max_consecutive_expands: usize,
    /// Sample threshold (in samples) for timestamp alignment tolerance
    pub timestamp_tolerance: u32,
}

impl Default for DecisionLogicConfig {
    fn default() -> Self {
        Self {
            max_consecutive_expands: 8, // ~80ms at 10ms/frame before comfort noise
            timestamp_tolerance: 160,    // 1 frame tolerance
        }
    }
}

pub struct DecisionLogic {
    config: DecisionLogicConfig,
    prev_operation: NetEqOperation,
    consecutive_expands: usize,
}

impl DecisionLogic {
    pub fn new(config: DecisionLogicConfig) -> Self {
        Self {
            config,
            prev_operation: NetEqOperation::Normal,
            consecutive_expands: 0,
        }
    }

    /// Determine the next operation to perform given:
    /// - whether a packet is available in the buffer
    /// - whether that packet's timestamp matches the required playout timestamp
    /// - delay recommendation from the delay manager
    /// - current buffer frames available
    pub fn get_decision(
        &mut self,
        has_packet: bool,
        delay_manager: &DelayManager,
        current_buffer_frames: usize,
    ) -> NetEqOperation {
        let op = if has_packet {
            // A packet is ready in the buffer
            if self.prev_operation == NetEqOperation::Expand
                || self.prev_operation == NetEqOperation::ComfortNoise
            {
                // Recovering from packet loss or silence: perform smooth merge
                self.consecutive_expands = 0;
                NetEqOperation::Merge
            } else {
                self.consecutive_expands = 0;
                match delay_manager.evaluate_delay(current_buffer_frames) {
                    DelayDecision::Accelerate => NetEqOperation::Accelerate,
                    DelayDecision::PreemptiveExpand => NetEqOperation::PreemptiveExpand,
                    DelayDecision::Normal => NetEqOperation::Normal,
                }
            }
        } else {
            // No packet available: packet is lost or delayed by jitter
            self.consecutive_expands += 1;
            if self.consecutive_expands > self.config.max_consecutive_expands {
                NetEqOperation::ComfortNoise
            } else {
                NetEqOperation::Expand
            }
        };

        self.prev_operation = op;
        op
    }

    pub fn last_operation(&self) -> NetEqOperation {
        self.prev_operation
    }

    pub fn consecutive_expands(&self) -> usize {
        self.consecutive_expands
    }

    pub fn reset(&mut self) {
        self.prev_operation = NetEqOperation::Normal;
        self.consecutive_expands = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delay_manager::DelayManagerConfig;

    #[test]
    fn test_decision_logic_state_transitions() {
        let mut dl = DecisionLogic::new(DecisionLogicConfig::default());
        let dm = DelayManager::new(DelayManagerConfig::default());

        // 1. Normal packet available
        assert_eq!(dl.get_decision(true, &dm, 2), NetEqOperation::Normal);

        // 2. Packet lost: should Expand (PLC)
        assert_eq!(dl.get_decision(false, &dm, 0), NetEqOperation::Expand);
        assert_eq!(dl.consecutive_expands(), 1);

        // 3. Repeat loss until exceeding max_consecutive_expands (8): should transition to ComfortNoise
        for _ in 1..8 {
            assert_eq!(dl.get_decision(false, &dm, 0), NetEqOperation::Expand);
        }
        assert_eq!(dl.get_decision(false, &dm, 0), NetEqOperation::ComfortNoise);

        // 4. Packet arrives again: should Merge to avoid click
        assert_eq!(dl.get_decision(true, &dm, 2), NetEqOperation::Merge);

        // 5. Subsequent packet: Normal
        assert_eq!(dl.get_decision(true, &dm, 2), NetEqOperation::Normal);
    }
}
