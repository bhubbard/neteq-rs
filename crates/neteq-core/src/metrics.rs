//! Objective audio quality and VoIP network telemetry metrics (ITU-T G.107 E-Model MOS-LQ,
//! SNR, Segmental SNR, and frame-boundary click/discontinuity detection).

use serde::{Deserialize, Serialize};

use crate::cng::{frame_rms, zero_crossing_rate};

/// Summary report of objective audio and network playout quality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioQualityReport {
    /// ITU-T G.107 transmission rating factor $R \in [0.0, 100.0]$.
    pub r_factor: f64,
    /// Estimated listening-quality Mean Opinion Score $\text{MOS-LQ} \in [1.0, 4.5]$.
    pub mos_lq: f64,
    /// Root-Mean-Square level in dBov ($0\text{ dBov} = 32768$).
    pub rms_dbov: f64,
    /// Average zero-crossing rate in $[0.0, 1.0]$.
    pub zero_crossing_rate: f32,
    /// Indices of frame boundaries where an acoustic discontinuity (click) was detected.
    pub boundary_click_frames: Vec<usize>,
}

/// Computes full-signal Signal-to-Noise Ratio (SNR) in decibels (dB) between a reference and degraded PCM signal.
pub fn snr_db(reference: &[i16], degraded: &[i16]) -> f64 {
    let n = reference.len().min(degraded.len());
    if n == 0 {
        return 0.0;
    }

    let mut sig_energy = 0.0f64;
    let mut err_energy = 0.0f64;
    for i in 0..n {
        let r = reference[i] as f64;
        let d = degraded[i] as f64;
        let e = r - d;
        sig_energy += r * r;
        err_energy += e * e;
    }

    if err_energy < 1e-9 {
        return 99.9;
    }
    if sig_energy < 1e-9 {
        return -99.9;
    }

    10.0 * (sig_energy / err_energy).log10()
}

/// Computes Segmental Signal-to-Noise Ratio (SegSNR) in dB across active speech frames of length `frame_size`.
/// Clamps per-frame SNR to $[-10\text{ dB}, +35\text{ dB}]$ per ITU-T P.56 / speech processing conventions.
pub fn segmental_snr_db(reference: &[i16], degraded: &[i16], frame_size: usize) -> f64 {
    let n = reference.len().min(degraded.len());
    let fs = frame_size.max(1);
    let num_frames = n / fs;
    if num_frames == 0 {
        return snr_db(reference, degraded);
    }

    let mut sum_snr = 0.0f64;
    let mut active_frames = 0usize;

    for f in 0..num_frames {
        let start = f * fs;
        let end = start + fs;
        let ref_slice = &reference[start..end];
        let deg_slice = &degraded[start..end];

        let rms = frame_rms(ref_slice);
        if rms < 25.0 {
            continue;
        }

        let frame_snr = snr_db(ref_slice, deg_slice).clamp(-10.0, 35.0);
        sum_snr += frame_snr;
        active_frames += 1;
    }

    if active_frames == 0 {
        0.0
    } else {
        sum_snr / (active_frames as f64)
    }
}

/// Detects unnatural acoustic discontinuities (clicks/pops) at frame boundaries `k * frame_size`.
///
/// Compares the first-difference step `|x[b] - x[b-1]|` at each frame boundary `b = k * frame_size`
/// against the local interior median/RMS first-difference across the adjacent frames.
/// Returns the list of frame indices $k \ge 1$ where the boundary jump exceeds `threshold_ratio` (typically `6.0`)
/// times the local interior step RMS and exceeds a minimum audible step (`1500` PCM units).
pub fn detect_boundary_clicks(
    samples: &[i16],
    frame_size: usize,
    threshold_ratio: f32,
) -> Vec<usize> {
    let fs = frame_size.max(2);
    let num_frames = samples.len() / fs;
    if num_frames < 2 {
        return Vec::new();
    }

    let ratio = threshold_ratio.max(2.0) as f64;
    let mut click_frames = Vec::new();

    for k in 1..num_frames {
        let b = k * fs;
        let boundary_jump = ((samples[b] as i32) - (samples[b - 1] as i32)).unsigned_abs() as f64;
        if boundary_jump < 1500.0 {
            continue;
        }

        // Compute interior sample-to-sample step RMS in the window [b - fs/2 .. b + fs/2] excluding b
        let win_start = b.saturating_sub(fs / 2).max(1);
        let win_end = (b + fs / 2).min(samples.len());
        let mut sum_sq_diff = 0.0f64;
        let mut count = 0usize;

        for idx in win_start..win_end {
            if idx == b {
                continue;
            }
            let d = (samples[idx] as f64) - (samples[idx - 1] as f64);
            sum_sq_diff += d * d;
            count += 1;
        }

        if count > 0 {
            let local_diff_rms = (sum_sq_diff / (count as f64)).sqrt().max(50.0);
            if boundary_jump > ratio * local_diff_rms {
                click_frames.push(k);
            }
        }
    }

    click_frames
}

/// Computes the ITU-T G.107 E-model transmission rating factor $R \in [0.0, 100.0]$
/// from one-way playout delay (`delay_ms`), inter-arrival jitter (`jitter_ms`), and `packet_loss_rate` ($\in [0, 1]$).
pub fn estimate_r_factor(one_way_delay_ms: f64, jitter_ms: f64, packet_loss_rate: f64) -> f64 {
    let r0 = 93.2f64;
    let effective_delay = (one_way_delay_ms.max(0.0)) + 2.0 * (jitter_ms.max(0.0));

    // Delay impairment Id (ITU-T G.107 simplified curve)
    let h = if effective_delay > 177.3 { 1.0 } else { 0.0 };
    let id = 0.024 * effective_delay + 0.11 * (effective_delay - 177.3) * h;

    // Equipment & packet-loss impairment Ie-eff (wideband / PLC-assisted voice curve)
    let loss_pct = (packet_loss_rate.clamp(0.0, 1.0)) * 100.0;
    let bpl = 25.1; // Packet-loss robustness factor with PLC enabled
    let ie_eff = 95.0 * (loss_pct / (loss_pct + bpl));

    (r0 - id - ie_eff).clamp(0.0, 100.0)
}

/// Computes the estimated Listening-Quality Mean Opinion Score ($\text{MOS-LQ} \in [1.0, 4.5]$)
/// from the ITU-T G.107 E-model rating factor $R$.
pub fn estimate_mos_lq(one_way_delay_ms: f64, jitter_ms: f64, packet_loss_rate: f64) -> f64 {
    let r = estimate_r_factor(one_way_delay_ms, jitter_ms, packet_loss_rate);
    if r <= 0.0 {
        1.0
    } else if r >= 100.0 {
        4.5
    } else {
        (1.0 + 0.035 * r + r * (r - 60.0) * (100.0 - r) * 7.0e-6).clamp(1.0, 4.5)
    }
}

/// Evaluates an output audio buffer and network telemetry to produce an `AudioQualityReport`.
pub fn analyze_audio_quality(
    samples: &[i16],
    frame_size: usize,
    one_way_delay_ms: f64,
    jitter_ms: f64,
    packet_loss_rate: f64,
) -> AudioQualityReport {
    let rms = frame_rms(samples) as f64;
    let rms_dbov = if rms > 1e-3 {
        20.0 * (rms / 32768.0).log10()
    } else {
        -100.0
    };
    let zcr = zero_crossing_rate(samples);
    let r_factor = estimate_r_factor(one_way_delay_ms, jitter_ms, packet_loss_rate);
    let mos_lq = estimate_mos_lq(one_way_delay_ms, jitter_ms, packet_loss_rate);
    let boundary_click_frames = detect_boundary_clicks(samples, frame_size, 6.0);

    AudioQualityReport {
        r_factor,
        mos_lq,
        rms_dbov,
        zero_crossing_rate: zcr,
        boundary_click_frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snr_and_segmental_snr() {
        let ref_sig: Vec<i16> = (0..1600)
            .map(|i| ((i as f32 * 0.1).sin() * 10000.0) as i16)
            .collect();
        let noisy: Vec<i16> = ref_sig.iter().map(|&s| s.saturating_add(100)).collect();

        let snr = snr_db(&ref_sig, &noisy);
        let seg_snr = segmental_snr_db(&ref_sig, &noisy, 160);
        assert!(snr > 30.0 && snr < 45.0, "Expected ~37 dB SNR, got {}", snr);
        assert!(seg_snr > 30.0 && seg_snr <= 35.0);
    }

    #[test]
    fn test_boundary_click_detector() {
        let mut clean: Vec<i16> = (0..640)
            .map(|i| ((2.0 * std::f32::consts::PI * (i as f32) / 80.0).sin() * 8000.0) as i16)
            .collect();
        assert!(detect_boundary_clicks(&clean, 160, 6.0).is_empty());

        // Inject a harsh step discontinuity at frame 2 boundary (index 320)
        for sample in clean.iter_mut().skip(320) {
            *sample = sample.saturating_add(12000);
        }
        let clicks = detect_boundary_clicks(&clean, 160, 6.0);
        assert_eq!(clicks, vec![2]);
    }

    #[test]
    fn test_mos_lq_monotonicity() {
        let mos_ideal = estimate_mos_lq(20.0, 2.0, 0.0);
        let mos_moderate = estimate_mos_lq(80.0, 25.0, 0.05);
        let mos_poor = estimate_mos_lq(250.0, 80.0, 0.25);

        assert!(mos_ideal > 4.2);
        assert!(mos_ideal > mos_moderate);
        assert!(mos_moderate > mos_poor);
        assert!(mos_poor >= 1.0);
    }
}
