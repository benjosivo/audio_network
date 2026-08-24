//! Tauri backend for the sender GUI: same capture -> mix -> UDP send
//! pipeline as `apps/sender/native`, but restartable (Start/Stop) and
//! driven by commands from the frontend instead of CLI args.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audio_core_capture::cpal_input::{list_input_devices, CpalInputSource};
#[cfg(windows)]
use audio_core_capture::wasapi_loopback::WasapiLoopbackSource;
use audio_core_capture::{AudioSource, CAPTURE_RING_CAPACITY_SAMPLES};
use audio_core_mixer::{Mixer, SourceChannel};
use network::UdpAudioSocket;
use parking_lot::Mutex;
use protocol::{AudioPacket, SampleFormat};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

/// The mixer only ever has one channel here (mirrors the native milestone-1
/// CLI); its id is arbitrary but fixed.
const SOURCE_ID: u32 = 1;

/// Sentinel shown in the source list on Windows for WASAPI loopback capture
/// of whatever's currently playing on the default output device.
const LOOPBACK_SOURCE_LABEL: &str = "System audio (loopback)";

#[derive(Debug, Clone, Deserialize)]
struct StartConfig {
    source: String,
    targets: Vec<String>,
    bind: String,
    gain: f32,
    frames_per_packet: usize,
}

#[derive(Debug, Clone, Serialize)]
struct StartInfo {
    source_name: String,
    sample_rate: u32,
    channels: u8,
    frames_per_packet: usize,
    local_addr: String,
}

#[derive(Debug, Clone, Serialize)]
struct SenderStats {
    packets_sent: u64,
    gain: f32,
    muted: bool,
}

struct SenderSession {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    channel: Arc<SourceChannel>,
}

impl Drop for SenderSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Default)]
struct AppState(Mutex<Option<SenderSession>>);

fn make_source(name: &str) -> Result<Box<dyn AudioSource>, String> {
    if name == LOOPBACK_SOURCE_LABEL {
        #[cfg(windows)]
        {
            return Ok(Box::new(WasapiLoopbackSource::default_device()));
        }
        #[cfg(not(windows))]
        {
            return Err("WASAPI loopback capture is only available on Windows".into());
        }
    }
    if name.trim().is_empty() {
        return Ok(Box::new(CpalInputSource::default_device()));
    }
    Ok(Box::new(CpalInputSource::named(name.to_string())))
}

#[tauri::command]
fn list_sources() -> Vec<String> {
    let mut sources = Vec::new();
    #[cfg(windows)]
    sources.push(LOOPBACK_SOURCE_LABEL.to_string());
    sources.extend(list_input_devices());
    sources
}

#[tauri::command]
fn start_sender(app: AppHandle, state: State<AppState>, cfg: StartConfig) -> Result<StartInfo, String> {
    let mut guard = state.0.lock();
    if guard.is_some() {
        return Err("sender is already running".into());
    }

    let targets: Vec<SocketAddr> = cfg
        .targets
        .iter()
        .map(|s| s.parse::<SocketAddr>().map_err(|_| format!("invalid target address: {s}")))
        .collect::<Result<_, String>>()?;
    if targets.is_empty() {
        return Err("at least one target address is required".into());
    }
    let bind: SocketAddr = cfg.bind.parse().map_err(|_| format!("invalid bind address: {}", cfg.bind))?;

    let source = make_source(&cfg.source)?;
    let source_name = source.name();

    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(CAPTURE_RING_CAPACITY_SAMPLES);
    let (capture_handle, format) = source.start(producer).map_err(|e| e.to_string())?;

    let socket = UdpAudioSocket::bind(bind).map_err(|e| e.to_string())?;
    let local_addr = socket.local_addr().map_err(|e| e.to_string())?;

    let max_frames = AudioPacket::max_frames_per_packet(format.channels);
    let frames_per_packet = cfg.frames_per_packet.clamp(1, max_frames);
    let channels = format.channels as usize;
    let needed_samples = frames_per_packet * channels;

    let mut mixer = Mixer::new();
    let channel = mixer.add_source_with_volume(SOURCE_ID, cfg.gain.clamp(0.0, 2.0));
    let channel_stats = channel.clone();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let app_thread = app.clone();
    let source_name_thread = source_name.clone();

    let join = std::thread::Builder::new()
        .name("sender-gui-stream".into())
        .spawn(move || {
            // Keeps capture alive on this thread; dropping it (on exit) stops it.
            let _capture_handle = capture_handle;
            let mut capture_buf = vec![0.0f32; needed_samples];
            let mut seq: u32 = 0;
            let mut frames_sent: u64 = 0;
            let mut packets_sent: u64 = 0;
            let mut last_report = Instant::now();

            'outer: while !stop_thread.load(Ordering::Relaxed) {
                let mut filled = 0;
                while filled < needed_samples {
                    if stop_thread.load(Ordering::Relaxed) {
                        break 'outer;
                    }
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
                if let Err(e) = socket.send_packet_to_many(&pkt, &targets) {
                    tracing::warn!(error = %e, "failed to send audio packet");
                }

                seq = seq.wrapping_add(1);
                frames_sent += frames_per_packet as u64;
                packets_sent += 1;

                if last_report.elapsed() >= Duration::from_millis(500) {
                    let _ = app_thread.emit(
                        "sender://stats",
                        SenderStats { packets_sent, gain: channel_stats.volume(), muted: channel_stats.is_muted() },
                    );
                    last_report = Instant::now();
                }
            }
            tracing::info!(source = %source_name_thread, "sender stream stopped");
        })
        .map_err(|e| e.to_string())?;

    let info = StartInfo {
        source_name,
        sample_rate: format.sample_rate,
        channels: format.channels,
        frames_per_packet,
        local_addr: local_addr.to_string(),
    };

    *guard = Some(SenderSession { stop, join: Some(join), channel });
    Ok(info)
}

#[tauri::command]
fn stop_sender(state: State<AppState>) -> Result<(), String> {
    let mut guard = state.0.lock();
    *guard = None; // Drop on SenderSession stops the stream thread and joins it.
    Ok(())
}

#[tauri::command]
fn set_gain(state: State<AppState>, gain: f32) -> Result<(), String> {
    let guard = state.0.lock();
    match guard.as_ref() {
        Some(session) => {
            session.channel.set_volume(gain);
            Ok(())
        }
        None => Err("sender is not running".into()),
    }
}

#[tauri::command]
fn set_muted(state: State<AppState>, muted: bool) -> Result<(), String> {
    let guard = state.0.lock();
    match guard.as_ref() {
        Some(session) => {
            session.channel.set_muted(muted);
            Ok(())
        }
        None => Err("sender is not running".into()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![list_sources, start_sender, stop_sender, set_gain, set_muted])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
