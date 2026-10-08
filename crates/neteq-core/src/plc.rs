use crate::time_stretch::TimeStretch;

pub struct PacketLossConcealment {
    time_stretch: TimeStretch,
    consecutive_expands: usize,
    decay_factor: f32,
}

impl PacketLossConcealment {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            time_stretch: TimeStretch::new(sample_rate),
            consecutive_expands: 0,
            decay_factor: 0.96, // Gradual energy decay per concealed frame
        }
    }

    /// Reset consecutive expand counter on normal packet receipt
    pub fn reset(&mut self) {
        self.consecutive_expands = 0;
    }

    /// Generate concealed audio frame of `output_len` samples using history buffer
    pub fn generate_concealment(&mut self, history: &[i16], output_len: usize) -> Vec<i16> {
        self.consecutive_expands += 1;
        let mut output = Vec::with_capacity(output_len);

        if history.is_empty() {
            return vec![0; output_len];
        }

        // Determine pitch period from recent history
        let pitch_lag = self.time_stretch.estimate_pitch_lag(history);
        let lag = pitch_lag.max(20).min(history.len());

        // Extract pitch cycle template from the tail of history
        let template_start = history.len().saturating_sub(lag);
        let template = &history[template_start..];

        // Overall attenuation based on consecutive lost frames
        let attenuation = self.decay_factor.powi(self.consecutive_expands as i32);

        for i in 0..output_len {
            let sample = template[i % template.len()] as f32;
            let decayed = (sample * attenuation).round() as i16;
            output.push(decayed);
        }

        output
    }

    pub fn consecutive_expands(&self) -> usize {
        self.consecutive_expands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plc_generation_and_decay() {
        let mut plc = PacketLossConcealment::new(16000);
        let mut history = Vec::new();
        for i in 0..640 {
            history.push((i % 100) as i16 * 100);
        }

        let f1 = plc.generate_concealment(&history, 160);
        let f2 = plc.generate_concealment(&history, 160);

        assert_eq!(f1.len(), 160);
        assert_eq!(f2.len(), 160);

        // f2 should be attenuated compared to f1
        let energy1: f64 = f1.iter().map(|&x| (x as f64).powi(2)).sum();
        let energy2: f64 = f2.iter().map(|&x| (x as f64).powi(2)).sum();
        assert!(energy2 < energy1);
    }
}
