//! Milestone-1 receiver CLI: listens for a UDP audio stream, smooths it
//! through a jitter buffer, and plays it out through the default output
//! device with a local master volume/mute applied.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

use audio_core_mixer::MasterGain;
use network::{JitterBuffer, JitterConfig, UdpAudioSocket};
use protocol::MAX_UDP_PAYLOAD;

#[derive(Parser, Debug)]
#[command(about = "Receives a UDP audio stream and plays it")]
struct Args {
    /// Local address to listen on.
    #[arg(long, default_value = "0.0.0.0:9000")]
    listen: SocketAddr,

    /// Local master volume applied to playback (0.0-2.0). Never sent to the sender.
    #[arg(long, default_value_t = 1.0)]
    master_volume: f32,

    /// Local master mute. Never sent to the sender.
    #[arg(long, default_value_t = false)]
    master_mute: bool,

    /// Target jitter-buffer depth in milliseconds.
    #[arg(long, default_value_t = 40)]
    jitter_ms: u64,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    let args = Args::parse();

    let socket = UdpAudioSocket::bind(args.listen).context("failed to bind UDP socket")?;
    tracing::info!(local = ?socket.local_addr()?, "receiver listening");

    let mut recv_buf = vec![0u8; MAX_UDP_PAYLOAD];
    let (first_packet, first_src) = socket.recv_packet(&mut recv_buf).context("failed to receive first audio packet")?;
    tracing::info!(
        from = ?first_src,
        sample_rate = first_packet.sample_rate,
        channels = first_packet.channels,
        "stream format learned from first packet"
    );

    let channels = first_packet.channels;
    let sample_rate = first_packet.sample_rate;
    let frames_per_packet = first_packet.samples.len() / channels as usize;
    let packet_duration_ms = frames_per_packet as f64 * 1000.0 / sample_rate as f64;
    let jitter_cfg = JitterConfig::from_millis(args.jitter_ms, packet_duration_ms);
    tracing::info!(?jitter_cfg, packet_duration_ms, "jitter buffer configured");

    let jitter_buffer = Arc::new(Mutex::new(JitterBuffer::new(jitter_cfg, channels, frames_per_packet)));
    jitter_buffer.lock().push(first_packet);

    // UDP receive thread: keeps pulling datagrams and feeding the jitter buffer.
    {
        let jitter_buffer = jitter_buffer.clone();
        std::thread::Builder::new()
            .name("udp-recv".into())
            .spawn(move || {
                let mut buf = vec![0u8; MAX_UDP_PAYLOAD];
                loop {
                    match socket.recv_packet(&mut buf) {
                        Ok((pkt, _src)) => jitter_buffer.lock().push(pkt),
                        Err(e) => tracing::warn!(error = %e, "dropped malformed/unreadable datagram"),
                    }
                }
            })
            .expect("failed to spawn UDP receive thread");
    }

    let master_gain = Arc::new(MasterGain::new(args.master_volume, args.master_mute));

    let host = cpal::default_host();
    let device = host.default_output_device().context("no default output device")?;
    tracing::info!(device = ?device.name().unwrap_or_default(), "opening output device");

    let config = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = {
        let jitter_buffer = jitter_buffer.clone();
        let master_gain = master_gain.clone();
        device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    jitter_buffer.lock().read(data);
                    master_gain.apply(data);
                },
                |err| tracing::error!("cpal output stream error: {err}"),
                None,
            )
            .context("failed to build output stream")?
    };
    stream.play().context("failed to start playback")?;
    tracing::info!("playback started");

    loop {
        std::thread::sleep(Duration::from_secs(5));
        let stats = jitter_buffer.lock().stats();
        tracing::info!(
            received = stats.received,
            late_dropped = stats.late_dropped,
            duplicate_dropped = stats.duplicate_dropped,
            underruns = stats.underruns,
            master_volume = master_gain.volume(),
            master_muted = master_gain.is_muted(),
            "receiver stats"
        );
    }
}
