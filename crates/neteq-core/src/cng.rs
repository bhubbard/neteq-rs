use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Computes the Root-Mean-Square (RMS) amplitude of a 16-bit PCM frame.
pub fn frame_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / (samples.len() as f64)).sqrt() as f32
}

/// Computes the Zero-Crossing Rate (ZCR) in $[0.0, 1.0]$ of a 16-bit PCM frame.
pub fn zero_crossing_rate(samples: &[i16]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mut crossings = 0usize;
    for w in samples.windows(2) {
        if (w[0] >= 0 && w[1] < 0) || (w[0] < 0 && w[1] >= 0) {
            crossings += 1;
        }
    }
    crossings as f32 / ((samples.len() - 1) as f32)
}

/// Lightweight energy and zero-crossing Voice Activity Detector (VAD)
/// used to distinguish active voiced/unvoiced speech from background room noise.
pub struct VoiceActivityDetector {
    noise_floor_rms: f32,
    speech_hangover_frames: usize,
    current_hangover: usize,
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceActivityDetector {
    pub fn new() -> Self {
        Self {
            noise_floor_rms: 35.0,
            speech_hangover_frames: 4,
            current_hangover: 0,
        }
    }

    /// Evaluates whether `samples` contains active speech (`true`) or background noise/silence (`false`),
    /// updating the adaptive noise floor estimate.
    pub fn is_speech(&mut self, samples: &[i16]) -> bool {
        let rms = frame_rms(samples);
        let zcr = zero_crossing_rate(samples);

        // Adaptive noise floor tracking: fast downward adaptation, slow upward adaptation during silence
        if rms < self.noise_floor_rms {
            self.noise_floor_rms = (0.85 * self.noise_floor_rms + 0.15 * rms).max(5.0);
        } else if rms < self.noise_floor_rms * 2.2 {
            self.noise_floor_rms = (0.97 * self.noise_floor_rms + 0.03 * rms).min(350.0);
        }

        let energy_active = rms > (self.noise_floor_rms * 2.5).max(120.0);
        let unvoiced_sibilant = rms > (self.noise_floor_rms * 1.8).max(80.0) && zcr > 0.25;

        if energy_active || unvoiced_sibilant {
            self.current_hangover = self.speech_hangover_frames;
            true
        } else if self.current_hangover > 0 {
            self.current_hangover -= 1;
            true
        } else {
            false
        }
    }

    pub fn noise_floor_rms(&self) -> f32 {
        self.noise_floor_rms
    }
}

pub struct ComfortNoiseGenerator {
    rng: StdRng,
    noise_energy: f32,
    filter_state: f32,
}

impl ComfortNoiseGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            noise_energy: 35.0, // Typical comfort noise amplitude (-60 to -50 dBFS)
            filter_state: 0.0,
        }
    }

    /// Update background noise energy estimate based on observed speech silence
    pub fn update_energy(&mut self, rms: f32) {
        if rms > 1.0 && rms < 400.0 {
            // Smoothly track background noise floor
            self.noise_energy = 0.95 * self.noise_energy + 0.05 * rms;
        }
    }

    /// Directly set target comfort noise RMS from an RFC 3389 SID packet
    pub fn update_from_sid_rms(&mut self, rms: f32) {
        if rms.is_finite() && rms >= 0.0 {
            self.noise_energy = rms.clamp(1.0, 2000.0);
        }
    }

    pub fn noise_energy(&self) -> f32 {
        self.noise_energy
    }

    /// Generate comfort noise frame of length `num_samples`
    pub fn generate(&mut self, num_samples: usize) -> Vec<i16> {
        let mut output = Vec::with_capacity(num_samples);

        for _ in 0..num_samples {
            let white_noise: f32 = self.rng.gen_range(-1.0..1.0);
            // First-order low-pass filter (pink-like spectrum)
            self.filter_state = 0.8 * self.filter_state + 0.2 * white_noise;
            let sample = (self.filter_state * self.noise_energy)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            output.push(sample);
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cng_generation() {
        let mut cng = ComfortNoiseGenerator::new(42);
        let noise = cng.generate(160);
        assert_eq!(noise.len(), 160);
        // Ensure not all zeros and within reasonable amplitude
        assert!(noise.iter().any(|&x| x != 0));
        assert!(noise.iter().all(|&x| x.abs() < 500));
    }

    #[test]
    fn test_vad_speech_vs_silence() {
        let mut vad = VoiceActivityDetector::new();
        let silence = vec![15i16; 160];
        assert!(!vad.is_speech(&silence));

        let voiced: Vec<i16> = (0..160)
            .map(|i| ((i as f32 * 0.2).sin() * 6000.0) as i16)
            .collect();
        assert!(vad.is_speech(&voiced));
    }
}
