pub struct TimeStretch {
    sample_rate: u32,
    min_pitch_lag: usize,
    max_pitch_lag: usize,
}

impl TimeStretch {
    pub fn new(sample_rate: u32) -> Self {
        // Human speech fundamental frequency typically 60 Hz to 400 Hz
        let min_pitch_lag = (sample_rate / 400) as usize; // ~40 samples @ 16kHz
        let max_pitch_lag = (sample_rate / 60) as usize;  // ~266 samples @ 16kHz

        Self {
            sample_rate,
            min_pitch_lag,
            max_pitch_lag,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Estimate pitch lag using normalized cross-correlation
    pub fn estimate_pitch_lag(&self, samples: &[i16]) -> usize {
        if samples.len() < self.max_pitch_lag * 2 {
            return self.min_pitch_lag;
        }

        let mut best_lag = self.min_pitch_lag;
        let mut best_corr = i64::MIN;
        let search_len = self.max_pitch_lag;

        for lag in self.min_pitch_lag..=self.max_pitch_lag {
            let mut corr = 0i64;
            for i in 0..search_len {
                corr += (samples[i] as i64) * (samples[i + lag] as i64);
            }
            if corr > best_corr {
                best_corr = corr;
                best_lag = lag;
            }
        }

        best_lag
    }

    /// Accelerate (shorten) speech signal by one pitch cycle using overlap-add
    pub fn accelerate(&self, input: &[i16]) -> Vec<i16> {
        let lag = self.estimate_pitch_lag(input);
        if input.len() <= lag * 2 {
            return input.to_vec();
        }

        let overlap_len = lag;
        let mut output = Vec::with_capacity(input.len().saturating_sub(overlap_len));

        // 1. Copy prefix
        let split_point = input.len() / 2;
        output.extend_from_slice(&input[0..split_point]);

        // 2. Overlap and crossfade across the pitch period
        for i in 0..overlap_len {
            let idx_a = split_point + i;
            let idx_b = split_point + lag + i;

            if idx_b < input.len() {
                let alpha = i as f32 / overlap_len as f32;
                let blended = ((1.0 - alpha) * (input[idx_a] as f32) + alpha * (input[idx_b] as f32)).round() as i16;
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

    /// Preemptive Expand (lengthen) speech signal by one pitch cycle using overlap-add
    pub fn preemptive_expand(&self, input: &[i16]) -> Vec<i16> {
        let lag = self.estimate_pitch_lag(input);
        if input.len() <= lag * 2 {
            return input.to_vec();
        }

        let overlap_len = lag;
        let mut output = Vec::with_capacity(input.len() + overlap_len);

        let split_point = input.len() / 2;

        // 1. Copy prefix up to split point
        output.extend_from_slice(&input[0..split_point]);

        // 2. Insert interpolated pitch period
        for i in 0..overlap_len {
            let idx_a = split_point.saturating_sub(lag) + i;
            let idx_b = split_point + i;

            if idx_b < input.len() {
                let alpha = i as f32 / overlap_len as f32;
                let blended = ((1.0 - alpha) * (input[idx_a] as f32) + alpha * (input[idx_b] as f32)).round() as i16;
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
            let s = ((2.0 * std::f32::consts::PI * 100.0 * (i as f32 / 16000.0)).sin() * 20000.0) as i16;
            samples.push(s);
        }

        let lag = ts.estimate_pitch_lag(&samples);
        assert!((lag as i32 - 160).abs() <= 5);

        let accelerated = ts.accelerate(&samples);
        assert!(accelerated.len() < samples.len());

        let expanded = ts.preemptive_expand(&samples);
        assert!(expanded.len() > samples.len());
    }
}
