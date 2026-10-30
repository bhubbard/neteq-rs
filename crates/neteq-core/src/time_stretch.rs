use std::f32::consts::PI;

pub struct TimeStretch {
    sample_rate: u32,
    min_pitch_lag: usize,
    max_pitch_lag: usize,
}

impl TimeStretch {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000);
        // Human speech fundamental frequency typically 60 Hz to 400 Hz
        let min_pitch_lag = ((sr / 400) as usize).max(16); // ~40 samples @ 16kHz
        let max_pitch_lag = ((sr / 60) as usize).max(min_pitch_lag + 1); // ~266 samples @ 16kHz

        Self {
            sample_rate: sr,
            min_pitch_lag,
            max_pitch_lag,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn min_pitch_lag(&self) -> usize {
        self.min_pitch_lag
    }

    pub fn max_pitch_lag(&self) -> usize {
        self.max_pitch_lag
    }

    /// Estimate pitch lag and normalized correlation clarity $r \in [-1.0, 1.0]$
    /// using Normalized Cross-Correlation (NCC) with $O(1)$ prefix-sum window energy lookup.
    ///
    /// Inspects the most recent samples (tail of `samples`) so pitch tracking reflects
    /// the current vocal cord period and is invariant to amplitude modulation.
    pub fn estimate_pitch_lag_and_clarity(&self, samples: &[i16]) -> (usize, f32) {
        let n = samples.len();
        if n < self.min_pitch_lag * 2 {
            return (self.min_pitch_lag.min(n / 2).max(1), 0.0);
        }

        let effective_max_lag = self.max_pitch_lag.min(n / 2);
        if effective_max_lag < self.min_pitch_lag {
            return (self.min_pitch_lag, 0.0);
        }

        // Focus correlation on the most recent 2 * effective_max_lag samples
        let window_total = (effective_max_lag * 2).min(n);
        let tail = &samples[n - window_total..];
        let search_len = window_total - effective_max_lag;

        // Build prefix sum of squared samples for O(1) window energy queries
        let mut prefix_energy = Vec::with_capacity(window_total + 1);
        prefix_energy.push(0.0f64);
        let mut acc = 0.0f64;
        for &s in tail {
            let v = s as f64;
            acc += v * v;
            prefix_energy.push(acc);
        }

        // Reference window is the most recent `search_len` samples: tail[window_total - search_len .. window_total]
        let ref_start = window_total - search_len;
        let energy_ref = prefix_energy[window_total] - prefix_energy[ref_start];
        if energy_ref < 1e-3 {
            return (self.min_pitch_lag, 0.0);
        }

        let mut ncc_by_lag = vec![-2.0f64; effective_max_lag + 1];
        let mut max_ncc = -2.0f64;
        let mut best_lag = self.min_pitch_lag;

        for (lag, slot) in ncc_by_lag
            .iter_mut()
            .enumerate()
            .take(effective_max_lag + 1)
            .skip(self.min_pitch_lag)
        {
            let cand_start = ref_start - lag;
            let energy_cand = prefix_energy[cand_start + search_len] - prefix_energy[cand_start];
            if energy_cand < 1e-3 {
                continue;
            }

            let mut dot = 0.0f64;
            for i in 0..search_len {
                dot += (tail[ref_start + i] as f64) * (tail[cand_start + i] as f64);
            }

            let denom = (energy_ref * energy_cand).sqrt();
            let ncc = if denom > 1e-6 { dot / denom } else { 0.0 };
            *slot = ncc;

            if ncc > max_ncc {
                max_ncc = ncc;
                best_lag = lag;
            }
        }

        // McLeod Pitch Method (MPM) subharmonic disambiguation:
        // A periodic signal with fundamental period T_0 also exhibits equal NCC peaks at 2*T_0, 3*T_0...
        // Select the shortest-lag local maximum whose NCC is within 92% of the global maximum.
        if max_ncc > 0.25 {
            let threshold = 0.92 * max_ncc;
            for lag in self.min_pitch_lag..=effective_max_lag {
                let cur = ncc_by_lag[lag];
                if cur < threshold {
                    continue;
                }
                let prev = if lag > self.min_pitch_lag {
                    ncc_by_lag[lag - 1]
                } else {
                    cur
                };
                let next = if lag < effective_max_lag {
                    ncc_by_lag[lag + 1]
                } else {
                    cur
                };
                if cur >= prev && cur >= next {
                    best_lag = lag;
                    max_ncc = cur;
                    break;
                }
            }
        }

        let clarity = max_ncc.clamp(-1.0, 1.0) as f32;
        (best_lag, clarity)
    }

    /// Estimate pitch lag using normalized cross-correlation
    pub fn estimate_pitch_lag(&self, samples: &[i16]) -> usize {
        self.estimate_pitch_lag_and_clarity(samples).0
    }

    /// Estimate pitch lag combining recent sync-buffer `history` with `input` frame
    /// so that short frames (e.g. 10ms = 160 samples at 16kHz) can resolve low-pitched voices down to 60 Hz.
    pub fn estimate_pitch_lag_with_history(&self, history: &[i16], input: &[i16]) -> usize {
        if history.is_empty() || input.len() >= self.max_pitch_lag * 2 {
            return self.estimate_pitch_lag(input);
        }
        let needed_hist = (self.max_pitch_lag * 2).saturating_sub(input.len()).min(history.len());
        let mut combined = Vec::with_capacity(needed_hist + input.len());
        combined.extend_from_slice(&history[history.len() - needed_hist..]);
        combined.extend_from_slice(input);
        self.estimate_pitch_lag(&combined)
    }

    /// Raised-cosine (Hann) crossfade weight $\alpha \in [0, 1]$ for smooth phase-aligned blending.
    #[inline]
    fn hann_alpha(i: usize, len: usize) -> f32 {
        if len == 0 {
            return 0.5;
        }
        0.5 * (1.0 - (PI * (i as f32) / (len as f32)).cos())
    }

    /// Accelerate (shorten) speech signal by one pitch cycle using WSOLA with raised-cosine crossfade.
    pub fn accelerate(&self, input: &[i16]) -> Vec<i16> {
        self.accelerate_with_history(&[], input)
    }

    /// Accelerate speech signal using optional prior history for accurate low-frequency pitch estimation.
    pub fn accelerate_with_history(&self, history: &[i16], input: &[i16]) -> Vec<i16> {
        if input.len() < self.min_pitch_lag {
            return input.to_vec();
        }
        let raw_lag = self.estimate_pitch_lag_with_history(history, input);
        let max_usable_lag = input.len() / 2;
        if max_usable_lag < 8 {
            return input.to_vec();
        }
        let lag = raw_lag.min(max_usable_lag).max(8);

        let overlap_len = lag;
        let mut output = Vec::with_capacity(input.len().saturating_sub(overlap_len));

        // 1. Copy prefix before overlap window
        let split_point = (input.len() - 2 * lag) / 2;
        output.extend_from_slice(&input[0..split_point]);

        // 2. Overlap and raised-cosine crossfade across the pitch period
        for i in 0..overlap_len {
            let idx_a = split_point + i;
            let idx_b = split_point + lag + i;

            if idx_b < input.len() {
                let alpha = Self::hann_alpha(i, overlap_len);
                let blended = ((1.0 - alpha) * (input[idx_a] as f32)
                    + alpha * (input[idx_b] as f32))
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                output.push(blended);
            }
        }

        // 3. Append remaining tail
        let tail_start = split_point + lag + overlap_len;
        if tail_start < input.len() {
            output.extend_from_slice(&input[tail_start..]);
        }

        output
    }

    /// Fast-accelerate speech signal (up to 2 pitch cycles when buffer occupancy is critically high).
    pub fn fast_accelerate(&self, history: &[i16], input: &[i16]) -> Vec<i16> {
        let once = self.accelerate_with_history(history, input);
        if once.len() >= self.min_pitch_lag * 2 {
            self.accelerate_with_history(history, &once)
        } else {
            once
        }
    }

    /// Preemptive Expand (lengthen) speech signal by one pitch cycle using WSOLA with raised-cosine crossfade.
    pub fn preemptive_expand(&self, input: &[i16]) -> Vec<i16> {
        self.preemptive_expand_with_history(&[], input)
    }

    /// Preemptive Expand speech signal using optional prior history for accurate low-frequency pitch estimation.
    pub fn preemptive_expand_with_history(&self, history: &[i16], input: &[i16]) -> Vec<i16> {
        if input.len() < self.min_pitch_lag {
            return input.to_vec();
        }
        let raw_lag = self.estimate_pitch_lag_with_history(history, input);
        let max_usable_lag = input.len() / 2;
        if max_usable_lag < 8 {
            return input.to_vec();
        }
        let lag = raw_lag.min(max_usable_lag).max(8);

        let overlap_len = lag;
        let mut output = Vec::with_capacity(input.len() + overlap_len);

        let split_point = (input.len() / 2).max(lag);

        // 1. Copy prefix up to split point
        output.extend_from_slice(&input[0..split_point]);

        // 2. Insert phase-aligned pitch period via raised-cosine overlap-add
        for i in 0..overlap_len {
            let idx_a = split_point.saturating_sub(lag) + i;
            let idx_b = split_point + i;

            if idx_b < input.len() {
                let alpha = Self::hann_alpha(i, overlap_len);
                let blended = ((1.0 - alpha) * (input[idx_a] as f32)
                    + alpha * (input[idx_b] as f32))
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                output.push(blended);
            }
        }

        // 3. Append remaining signal
        output.extend_from_slice(&input[split_point..]);

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pitch_estimation_and_stretching() {
        let ts = TimeStretch::new(16000);
        let mut samples = Vec::new();

        // 100 Hz sine wave: period = 160 samples
        for i in 0..1600 {
            let s = ((2.0 * std::f32::consts::PI * 100.0 * (i as f32 / 16000.0)).sin() * 20000.0)
                as i16;
            samples.push(s);
        }

        let (lag, clarity) = ts.estimate_pitch_lag_and_clarity(&samples);
        assert!((lag as i32 - 160).abs() <= 2, "Expected lag ~160, got {}", lag);
        assert!(clarity > 0.95, "Expected high periodic clarity > 0.95, got {}", clarity);

        let accelerated = ts.accelerate(&samples);
        assert_eq!(accelerated.len(), samples.len() - lag);

        let expanded = ts.preemptive_expand(&samples);
        assert_eq!(expanded.len(), samples.len() + lag);
    }

    #[test]
    fn test_ncc_pitch_estimation_under_amplitude_modulation() {
        // Verify that Normalized Cross-Correlation accurately finds the 125 Hz period (128 samples @ 16kHz)
        // even when signal amplitude decays steeply across the window.
        let ts = TimeStretch::new(16000);
        let true_lag = 128usize; // 125 Hz
        let samples: Vec<i16> = (0..1024)
            .map(|i| {
                let env = 25000.0 * (-0.0025 * (i as f32)).exp();
                let phase = 2.0 * std::f32::consts::PI * (i as f32) / (true_lag as f32);
                (env * (phase.sin() + 0.3 * (2.0 * phase).sin())) as i16
            })
            .collect();

        let (est_lag, clarity) = ts.estimate_pitch_lag_and_clarity(&samples);
        assert!(
            (est_lag as i32 - true_lag as i32).abs() <= 2,
            "NCC pitch estimator should find fundamental lag {} despite decaying envelope, got {}",
            true_lag,
            est_lag
        );
        assert!(clarity > 0.85);
    }
}
