//! Milestone-1 sender CLI: captures one audio source (default: WASAPI
//! loopback of the default output device) and streams it over UDP to one or
//! more receivers.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

use audio_core_capture::wasapi_loopback::WasapiLoopbackSource;
use audio_core_capture::{AudioSource, CAPTURE_RING_CAPACITY_SAMPLES};
use audio_core_mixer::Mixer;
use network::UdpAudioSocket;
use protocol::{AudioPacket, SampleFormat};

/// The mixer only ever has one channel in milestone 1; its id is arbitrary
/// but fixed so a future control-message handler has something to address.
const SOURCE_ID: u32 = 1;

#[derive(Parser, Debug)]
#[command(about = "Captures local audio and streams it over UDP")]
struct Args {
    /// Receiver address to stream to, e.g. 192.168.1.50:9000. Repeatable.
    #[arg(long = "target", required = true)]
    targets: Vec<SocketAddr>,

    /// Local address to send from.
    #[arg(long, default_value = "0.0.0.0:0")]
    bind: SocketAddr,

    /// Initial gain applied to the captured source (0.0-2.0).
    #[arg(long, default_value_t = 1.0)]
    gain: f32,

    /// Audio frames per UDP packet (at 48kHz stereo, 160 is ~3.3ms).
    #[arg(long, default_value_t = 160)]
    frames_per_packet: usize,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    let args = Args::parse();

    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(CAPTURE_RING_CAPACITY_SAMPLES);

    let source: Box<dyn AudioSource> = Box::new(WasapiLoopbackSource::default_device());
    let source_name = source.name();
    let (_capture_handle, format) = source.start(producer).context("failed to start audio capture")?;
    tracing::info!(source = %source_name, sample_rate = format.sample_rate, channels = format.channels, "capture started");

    let max_frames = AudioPacket::max_frames_per_packet(format.channels);
    let frames_per_packet = args.frames_per_packet.clamp(1, max_frames);
    if frames_per_packet != args.frames_per_packet {
        tracing::warn!(requested = args.frames_per_packet, using = frames_per_packet, max_frames, "clamped --frames-per-packet to fit one UDP datagram");
    }
    let channels = format.channels as usize;
    let needed_samples = frames_per_packet * channels;

    let mut mixer = Mixer::new();
    let channel = mixer.add_source_with_volume(SOURCE_ID, args.gain);

    let socket = UdpAudioSocket::bind(args.bind).context("failed to bind UDP socket")?;
    tracing::info!(local = ?socket.local_addr()?, targets = ?args.targets, "sender ready");

    let mut capture_buf = vec![0.0f32; needed_samples];
    let mut seq: u32 = 0;
    let mut frames_sent: u64 = 0;
    let mut packets_sent: u64 = 0;
    let mut last_report = Instant::now();

    loop {
        let mut filled = 0;
        while filled < needed_samples {
            match consumer.pop() {
                Ok(sample) => {
                    capture_buf[filled] = sample;
                    filled += 1;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }

        let mut mixed = vec![0.0f32; needed_samples];
        mixer.process(&[(SOURCE_ID, &capture_buf)], &mut mixed);

        let pts_us = frames_sent * 1_000_000 / format.sample_rate as u64;
        let pkt = AudioPacket {
            seq,
            pts_us,
            sample_rate: format.sample_rate,
            channels: format.channels,
            format: SampleFormat::F32,
            samples: mixed,
        };
        if let Err(e) = socket.send_packet_to_many(&pkt, &args.targets) {
            tracing::warn!(error = %e, "failed to send audio packet");
        }

        seq = seq.wrapping_add(1);
        frames_sent += frames_per_packet as u64;
        packets_sent += 1;

        if last_report.elapsed() >= Duration::from_secs(5) {
            tracing::info!(packets_sent, gain = channel.volume(), muted = channel.is_muted(), "sender stats");
            last_report = Instant::now();
        }
    }
}
