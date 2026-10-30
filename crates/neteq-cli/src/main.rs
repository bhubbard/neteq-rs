use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use hound::{WavReader, WavSpec, WavWriter};
use neteq_core::{
    analyze_audio_quality, AudioPacket, NetEq, NetEqConfig,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "neteq", author = "NetEQ Contributors", version = "0.1.0")]
#[command(about = "Pure Rust NetEQ adaptive audio jitter buffer, time-stretching, and PLC CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Simulate network streaming through NetEQ with realistic jitter, packet loss, and reordering
    Simulate {
        /// Optional input WAV file (if omitted, synthesizes realistic multi-tone voice harmonic signal)
        #[arg(short, long)]
        input: Option<PathBuf>,

        /// Output WAV file path to write processed audio
        #[arg(short, long, default_value = "neteq_output.wav")]
        output: PathBuf,

        /// Audio sample rate in Hz (used if synthesizing)
        #[arg(short, long, default_value_t = 16000)]
        sample_rate: u32,

        /// Maximum network jitter in milliseconds
        #[arg(short, long, default_value_t = 40.0)]
        jitter_ms: f64,

        /// Packet loss probability (0.0 to 1.0)
        #[arg(short, long, default_value_t = 0.05)]
        loss_rate: f64,

        /// Packet reordering probability (0.0 to 1.0)
        #[arg(short, long, default_value_t = 0.03)]
        reorder_rate: f64,

        /// Duration in seconds to simulate if generating synthetic speech
        #[arg(short, long, default_value_t = 5.0)]
        duration_secs: f32,

        /// Frame size in milliseconds (typically 10ms or 20ms)
        #[arg(long, default_value_t = 10)]
        frame_ms: usize,
    },

    /// Benchmark raw NetEQ processing throughput across operations
    Benchmark {
        /// Duration of audio in simulated seconds to process
        #[arg(short, long, default_value_t = 60.0)]
        seconds: f32,

        /// Sample rate in Hz
        #[arg(short, long, default_value_t = 16000)]
        sample_rate: u32,
    },

    /// Display information on NetEQ architecture and configuration defaults
    Info,
}

#[derive(Clone)]
struct ScheduledPacket {
    delivery_time_ms: f64,
    packet: AudioPacket,
}

impl PartialEq for ScheduledPacket {
    fn eq(&self, other: &Self) -> bool {
        self.delivery_time_ms == other.delivery_time_ms
    }
}

impl Eq for ScheduledPacket {}

impl PartialOrd for ScheduledPacket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledPacket {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse for min-heap by delivery_time_ms
        other
            .delivery_time_ms
            .partial_cmp(&self.delivery_time_ms)
            .unwrap_or(Ordering::Equal)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Simulate {
            input,
            output,
            sample_rate,
            jitter_ms,
            loss_rate,
            reorder_rate,
            duration_secs,
            frame_ms,
        } => run_simulation(
            input,
            output,
            sample_rate,
            jitter_ms,
            loss_rate,
            reorder_rate,
            duration_secs,
            frame_ms,
        ),
        Commands::Benchmark {
            seconds,
            sample_rate,
        } => run_benchmark(seconds, sample_rate),
        Commands::Info => run_info(),
    }
}

fn run_info() -> Result<()> {
    println!("{}", "=======================================================".bright_blue());
    println!("{}", "  NetEQ-rs: Pure Rust Adaptive Audio Jitter Buffer     ".bright_cyan().bold());
    println!("{}", "=======================================================".bright_blue());
    println!();
    println!("Extracts Chromium / WebRTC's gold-standard audio jitter buffer logic into pure Rust:");
    println!("  * {} Adaptive target delay based on 95th-percentile inter-arrival jitter", "Delay Manager:".bold());
    println!("  * {} Pitch-synchronous overlap-add (WSOLA) for speech speedup/slowdown", "Time Stretch:".bold());
    println!("  * {} Autoregressive pitch extrapolation with progressive decay", "Packet Loss Concealment (PLC):".bold());
    println!("  * {} Pink-noise spectral filtered comfort noise for silence/long drops", "Comfort Noise (CNG):".bold());
    println!("  * {} Phase-aligned crossfade when transitioning from PLC to real packets", "Smooth Merging:".bold());
    println!();
    println!("Zero C/C++ dependencies. Suitable for real-time AI voice agents, WebSockets, and VoIP.");
    println!();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_simulation(
    input: Option<PathBuf>,
    output: PathBuf,
    sample_rate: u32,
    jitter_ms: f64,
    loss_rate: f64,
    reorder_rate: f64,
    duration_secs: f32,
    frame_ms: usize,
) -> Result<()> {
    println!("{}", "=== NetEQ Adaptive Jitter Buffer Simulation ===".bright_green().bold());
    println!("Sample Rate: {} Hz | Frame: {} ms", sample_rate, frame_ms);
    println!("Network Profile: Jitter <= {:.1}ms | Loss: {:.1}% | Reorder: {:.1}%", jitter_ms, loss_rate * 100.0, reorder_rate * 100.0);
    println!();

    let frame_samples = ((sample_rate as usize) * frame_ms) / 1000;

    // Load or generate source audio samples
    let (source_samples, effective_sample_rate) = if let Some(in_path) = input {
        println!("Reading input WAV: {}", in_path.display().to_string().cyan());
        let mut reader = WavReader::open(&in_path).context("Failed to open input WAV file")?;
        let spec = reader.spec();
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap_or(0)).collect();
        (samples, spec.sample_rate)
    } else {
        println!("Synthesizing realistic speech harmonic tone ({} seconds)...", duration_secs);
        let total_samples = (sample_rate as f32 * duration_secs) as usize;
        let mut samples = Vec::with_capacity(total_samples);
        for i in 0..total_samples {
            let t = i as f32 / sample_rate as f32;
            // Harmonic combination of fundamental 150Hz + formants (450Hz, 750Hz)
            let s1 = (2.0 * std::f32::consts::PI * 150.0 * t).sin() * 8000.0;
            let s2 = (2.0 * std::f32::consts::PI * 450.0 * t).sin() * 4000.0;
            let s3 = (2.0 * std::f32::consts::PI * 750.0 * t).sin() * 2000.0;
            samples.push((s1 + s2 + s3) as i16);
        }
        (samples, sample_rate)
    };

    let total_frames = source_samples.len() / frame_samples;
    println!("Generated {} audio frames ({:.2} seconds total)", total_frames, source_samples.len() as f32 / effective_sample_rate as f32);

    let neteq_config = NetEqConfig {
        sample_rate: effective_sample_rate,
        frame_size_ms: frame_ms,
        ..Default::default()
    };

    let mut neteq = NetEq::new(neteq_config).context("Failed to initialize NetEQ engine")?;
    let mut rng = StdRng::seed_from_u64(42);

    // Schedule packets over simulated network with jitter, loss, and reordering
    let mut scheduled_network_queue: BinaryHeap<ScheduledPacket> = BinaryHeap::new();
    let mut packets_sent = 0usize;
    let mut packets_dropped_by_network = 0usize;

    for frame_idx in 0..total_frames {
        let nominal_send_time_ms = (frame_idx * frame_ms) as f64;

        // Check packet loss
        if rng.gen_bool(loss_rate.clamp(0.0, 1.0)) {
            packets_dropped_by_network += 1;
            continue;
        }

        // Calculate arrival jitter
        let mut jitter = rng.gen_range(0.0..=jitter_ms);
        if rng.gen_bool(reorder_rate.clamp(0.0, 1.0)) {
            // Significant out-of-order delay
            jitter += jitter_ms * 1.5;
        }

        let arrival_time_ms = nominal_send_time_ms + jitter;
        let ts = (frame_idx * frame_samples) as u32;
        let slice = &source_samples[frame_idx * frame_samples..(frame_idx + 1) * frame_samples];

        scheduled_network_queue.push(ScheduledPacket {
            delivery_time_ms: arrival_time_ms,
            packet: AudioPacket::new(ts, frame_idx as u16, slice.to_vec()),
        });
        packets_sent += 1;
    }

    println!("Packets sent to network: {}, dropped on wire: {}", packets_sent, packets_dropped_by_network);

    // Prepare output WAV writer
    let spec = WavSpec {
        channels: 1,
        sample_rate: effective_sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&output, spec).context("Failed to create output WAV file")?;

    let mut playout_clock_ms = 0.0;
    let clock_step_ms = frame_ms as f64;
    let mut output_frame_buf = vec![0i16; frame_samples];
    let mut all_output_samples = Vec::with_capacity(source_samples.len());

    let start_time = Instant::now();

    // Stream simulation loop
    let simulation_end_ms = (total_frames * frame_ms) as f64 + (jitter_ms * 3.0);
    while playout_clock_ms < simulation_end_ms {
        // Deliver any packets that have arrived by playout_clock_ms
        while let Some(top) = scheduled_network_queue.peek() {
            if top.delivery_time_ms <= playout_clock_ms {
                let scheduled = scheduled_network_queue.pop().unwrap();
                let _ = neteq.insert_packet(scheduled.packet);
            } else {
                break;
            }
        }

        // Playout tick
        let _op = neteq.get_audio(&mut output_frame_buf).context("get_audio error")?;
        for &sample in &output_frame_buf {
            writer.write_sample(sample)?;
            all_output_samples.push(sample);
        }

        playout_clock_ms += clock_step_ms;
    }

    writer.finalize().context("Failed to finalize WAV file")?;
    let elapsed = start_time.elapsed();

    let stats = neteq.stats();
    let total_playout_frames = (stats.normal_frames
        + stats.accelerated_frames
        + stats.preemptive_expanded_frames
        + stats.concealed_frames
        + stats.comfort_noise_frames
        + stats.merged_frames)
        .max(1);
    let effective_loss_rate = packets_dropped_by_network as f64 / total_frames.max(1) as f64;
    let conceal_pct = (stats.concealed_frames as f64 / total_playout_frames as f64) * 100.0;
    let quality = analyze_audio_quality(
        &all_output_samples,
        frame_samples,
        stats.target_delay_ms,
        stats.jitter_ms,
        effective_loss_rate,
    );

    println!();
    println!("{}", "--- Simulation Results ---".bright_yellow().bold());
    println!("Processing Time: {:.2?}", elapsed);
    println!("Output File: {}", output.display().to_string().bright_cyan());
    println!();
    println!("Packets Received:      {}", stats.packets_received);
    println!("Packets Discarded:     {}", stats.packets_discarded);
    println!("Playout Operations:");
    println!("  - Normal:            {}", stats.normal_frames.to_string().green());
    println!("  - Accelerate (WSOLA): {}", stats.accelerated_frames.to_string().blue());
    println!("  - Preemptive Expand: {}", stats.preemptive_expanded_frames.to_string().cyan());
    println!("  - Expand (PLC):      {}", stats.concealed_frames.to_string().yellow());
    println!("  - Comfort Noise:     {}", stats.comfort_noise_frames.to_string().magenta());
    println!("  - Merged:            {}", stats.merged_frames.to_string().green());
    println!();
    println!("Target Delay:          {:.1} ms", stats.target_delay_ms);
    println!("Network Jitter (RFC3550): {:.2} ms", stats.jitter_ms);
    println!("Packet Loss Rate:      {:.2}%", stats.packet_loss_rate * 100.0);
    println!("Acceleration Rate:     {:.2}%", stats.accelerate_rate * 100.0);
    println!("Concealment Rate:      {:.2}%", conceal_pct);
    println!("Estimated MOS-LQ:      {:.2} / 4.50 (R-Factor: {:.1})", quality.mos_lq, quality.r_factor);
    println!("Output Level:          {:.1} dBov | Boundary Clicks: {}", quality.rms_dbov, quality.boundary_click_frames.len());
    println!();
    println!("{}", "Simulation successfully completed!".bright_green().bold());

    Ok(())
}

fn run_benchmark(seconds: f32, sample_rate: u32) -> Result<()> {
    println!("{}", "=== NetEQ Micro-Benchmark ===".bright_green().bold());
    println!("Simulating {:.1} seconds of real-time audio @ {} Hz", seconds, sample_rate);

    let frame_ms = 10;
    let frame_samples = ((sample_rate as usize) * frame_ms) / 1000;
    let total_frames = ((sample_rate as f32 * seconds) as usize) / frame_samples;

    let mut neteq = NetEq::new(NetEqConfig {
        sample_rate,
        channels: 1,
        frame_size_ms: frame_ms,
        ..Default::default()
    })?;

    // Create synthetic frames
    let test_frame = vec![5000i16; frame_samples];

    let start = Instant::now();

    let mut out = vec![0i16; frame_samples];
    for i in 0..total_frames {
        let ts = (i * frame_samples) as u32;
        let _ = neteq.insert_packet(AudioPacket::new(ts, i as u16, test_frame.clone()));
        let _ = neteq.get_audio(&mut out)?;
    }

    let duration = start.elapsed();
    let total_samples = total_frames * frame_samples;
    let real_time_ratio = (seconds as f64) / duration.as_secs_f64();
    let samples_per_sec = total_samples as f64 / duration.as_secs_f64();

    println!();
    println!("{}", "Benchmark Results:".bright_yellow().bold());
    println!("Total Samples Processed: {}", total_samples);
    println!("Elapsed Time:            {:.3?}", duration);
    println!("Throughput:              {:.2} Million samples/sec", samples_per_sec / 1_000_000.0);
    println!("Real-time Multiplier:    {:.1}x faster than real-time", real_time_ratio);
    println!();

    Ok(())
}
