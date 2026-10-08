use std::collections::VecDeque;

/// SyncBuffer maintains the playback audio queue and past audio history.
///
/// In NetEQ, the sync buffer stores:
/// 1. Future audio samples that have been decoded or generated but not yet retrieved by `get_audio()`.
/// 2. Past audio samples (history) that have already been played out, essential for:
///    - Pitch estimation in WSOLA and PLC
///    - Smooth crossfading and overlap-add operations
///    - Merge operations when recovering from packet loss concealment.
pub struct SyncBuffer {
    /// History samples that have already been played out
    history: VecDeque<i16>,
    /// Future samples ready to be extracted by the consumer
    future: VecDeque<i16>,
    /// Maximum number of history samples to retain (typically ~100-200ms)
    max_history_samples: usize,
}

impl SyncBuffer {
    pub fn new(max_history_samples: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(max_history_samples),
            future: VecDeque::new(),
            max_history_samples,
        }
    }

    /// Number of future samples currently available for extraction
    pub fn future_len(&self) -> usize {
        self.future.len()
    }

    /// Number of historical samples retained
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Total samples in buffer (future + history)
    pub fn total_len(&self) -> usize {
        self.future.len() + self.history.len()
    }

    /// Push new samples into the future queue
    pub fn push_future(&mut self, samples: &[i16]) {
        self.future.extend(samples.iter().copied());
    }

    /// Extract up to `output.len()` samples into `output`.
    /// Extracted samples are moved from future into history.
    /// Returns the number of samples actually written to `output`.
    pub fn extract(&mut self, output: &mut [i16]) -> usize {
        let count = output.len().min(self.future.len());
        for i in 0..count {
            if let Some(sample) = self.future.pop_front() {
                output[i] = sample;
                self.push_history_sample(sample);
            }
        }
        count
    }

    /// Internal helper to push a sample to history maintaining max size
    fn push_history_sample(&mut self, sample: i16) {
        if self.history.len() >= self.max_history_samples {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    /// Get a contiguous slice/vector of the most recent history samples
    pub fn get_history(&self, count: usize) -> Vec<i16> {
        let count = count.min(self.history.len());
        let start = self.history.len() - count;
        self.history.range(start..).copied().collect()
    }

    /// Get all current history samples as a vector
    pub fn all_history(&self) -> Vec<i16> {
        self.history.iter().copied().collect()
    }

    /// Overwrite or crossfade the tail of history with the start of new samples,
    /// or replace future samples.
    pub fn clear_future(&mut self) {
        self.future.clear();
    }

    /// Flush both future and history
    pub fn flush(&mut self) {
        self.future.clear();
        self.history.clear();
    }

    /// Prepend samples to future (useful when an operation generates audio that needs immediate playout)
    pub fn prepend_future(&mut self, samples: &[i16]) {
        for &s in samples.iter().rev() {
            self.future.push_front(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_buffer_lifecycle() {
        let mut sb = SyncBuffer::new(500);

        let input_a = vec![100; 160];
        let input_b = vec![200; 160];

        sb.push_future(&input_a);
        sb.push_future(&input_b);

        assert_eq!(sb.future_len(), 320);
        assert_eq!(sb.history_len(), 0);

        let mut out = vec![0i16; 160];
        let extracted = sb.extract(&mut out);

        assert_eq!(extracted, 160);
        assert_eq!(out, vec![100; 160]);
        assert_eq!(sb.future_len(), 160);
        assert_eq!(sb.history_len(), 160);

        let hist = sb.get_history(50);
        assert_eq!(hist.len(), 50);
        assert_eq!(hist[0], 100);
    }

    #[test]
    fn test_sync_buffer_history_overflow() {
        let mut sb = SyncBuffer::new(100);
        let input = vec![42; 250];
        sb.push_future(&input);

        let mut out = vec![0i16; 250];
        sb.extract(&mut out);

        // Max history was 100, should be capped at 100
        assert_eq!(sb.history_len(), 100);
    }
}
