use std::f32::consts::PI;

use crate::time_stretch::TimeStretch;

pub struct PacketLossConcealment {
    time_stretch: TimeStretch,
    consecutive_expands: usize,
    decay_factor: f32,
    phase_offset: usize,
}

impl PacketLossConcealment {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            time_stretch: TimeStretch::new(sample_rate),
            consecutive_expands: 0,
            decay_factor: 0.94, // Progressive energy decay per concealed frame
            phase_offset: 0,
        }
    }

    /// Reset consecutive expand counter and phase tracking on normal packet receipt
    pub fn reset(&mut self) {
        self.consecutive_expands = 0;
        self.phase_offset = 0;
    }

    /// Generate concealed audio frame of `output_len` samples using history buffer.
    ///
    /// Preserves pitch-period phase continuity across multiple consecutive lost frames
    /// and applies smooth per-sample exponential decay with raised-cosine loop crossfading
    /// to eliminate boundary clicks.
    pub fn generate_concealment(&mut self, history: &[i16], output_len: usize) -> Vec<i16> {
        let prev_expands = self.consecutive_expands;
        self.consecutive_expands += 1;

        if history.is_empty() || output_len == 0 {
            return vec![0; output_len];
        }

        // Estimate pitch period and voicing clarity from history
        let (pitch_lag, _clarity) = self.time_stretch.estimate_pitch_lag_and_clarity(history);
        let lag = pitch_lag.max(20).min(history.len());

        // Extract pitch cycle template from the tail of history
        let template_start = history.len().saturating_sub(lag);
        let template = &history[template_start..];
        let t_len = template.len().max(1);

        // Crossfade length at the loop wrap boundary to avoid discontinuity between template[t_len-1] and template[0]
        let xfade_len = (t_len / 8).clamp(1, 16);
        let prefix_start = template_start.saturating_sub(xfade_len);
        let pre_template = &history[prefix_start..template_start];

        let mut output = Vec::with_capacity(output_len);
        for i in 0..output_len {
            let pos = (self.phase_offset + i) % t_len;
            let raw_sample = if pos < pre_template.len() {
                // Smoothly blend loop wrap boundary using the samples immediately preceding template_start
                let alpha = 0.5 * (1.0 - (PI * (pos as f32) / (pre_template.len() as f32)).cos());
                (1.0 - alpha) * (pre_template[pos] as f32) + alpha * (template[pos] as f32)
            } else {
                template[pos] as f32
            };

            // Continuous per-sample exponential envelope attenuation across consecutive frames
            let fractional_frame = (prev_expands as f32) + ((i + 1) as f32) / (output_len as f32);
            let attenuation = self.decay_factor.powf(fractional_frame);
            let decayed = (raw_sample * attenuation)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            output.push(decayed);
        }

        self.phase_offset = (self.phase_offset + output_len) % t_len;
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
            let s = ((2.0 * std::f32::consts::PI * (i as f32) / 100.0).sin() * 12000.0) as i16;
            history.push(s);
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
