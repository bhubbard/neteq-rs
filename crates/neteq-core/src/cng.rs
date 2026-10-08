use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

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
        if rms > 1.0 && rms < 200.0 {
            // Smoothly track background noise floor
            self.noise_energy = 0.95 * self.noise_energy + 0.05 * rms;
        }
    }

    /// Generate comfort noise frame of length `num_samples`
    pub fn generate(&mut self, num_samples: usize) -> Vec<i16> {
        let mut output = Vec::with_capacity(num_samples);

        for _ in 0..num_samples {
            let white_noise: f32 = self.rng.gen_range(-1.0..1.0);
            // First-order low-pass filter (pink-like spectrum)
            self.filter_state = 0.8 * self.filter_state + 0.2 * white_noise;
            let sample = (self.filter_state * self.noise_energy).round() as i16;
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
}
