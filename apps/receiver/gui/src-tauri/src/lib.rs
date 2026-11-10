//! Tauri backend for the receiver GUI: same UDP -> jitter buffer -> playback
//! pipeline as `apps/receiver/native`, but restartable (Start/Stop) and
//! driven by commands from the frontend instead of CLI args.
//!
//! Starting succeeds as soon as the socket is bound; the stream format isn't
//! known until the first packet arrives, so status after that point is
//! reported asynchronously via `receiver://event` (`waiting` -> `playing`,
//! or `error`) rather than as the `start_receiver` command's return value.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use audio_core_mixer::MasterGain;
use audio_core_mixer::output::PlaybackOutput;
use network::{JitterBuffer, JitterConfig, UdpAudioSocket};
use parking_lot::Mutex;
use protocol::MAX_UDP_PAYLOAD;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Clone, Deserialize)]
struct StartConfig {
    listen: String,
    jitter_ms: u64,
    master_volume: f32,
    master_mute: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ReceiverEvent {
    Waiting,
    Playing {
        from: String,
        sample_rate: u32,
        channels: u8,
        received: u64,
        late_dropped: u64,
        duplicate_dropped: u64,
        underruns: u64,
        master_volume: f32,
        master_muted: bool,
    },
    Error {
        message: String,
    },
    Stopped,
}

struct ReceiverSession {
    stop: Arc<AtomicBool>,
    control_join: Option<JoinHandle<()>>,
    master: Arc<MasterGain>,
}

impl Drop for ReceiverSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.control_join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Default)]
struct AppState(Mutex<Option<ReceiverSession>>);

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[tauri::command]
fn start_receiver(app: AppHandle, state: State<AppState>, cfg: StartConfig) -> Result<String, String> {
    let mut guard = state.0.lock();
    if guard.is_some() {
        return Err("receiver is already running".into());
    }

    let listen: SocketAddr = cfg.listen.parse().map_err(|_| format!("invalid listen address: {}", cfg.listen))?;
    let socket = UdpAudioSocket::bind(listen).map_err(|e| e.to_string())?;
    // Read with a timeout (rather than blocking forever) so this thread can
    // notice the stop flag promptly, both while waiting for a stream to
    // start and for the lifetime of the session afterward.
    socket.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
    let local_addr = socket.local_addr().map_err(|e| e.to_string())?;

    let master = Arc::new(MasterGain::new(cfg.master_volume.clamp(0.0, 2.0), cfg.master_mute));
    let stop = Arc::new(AtomicBool::new(false));

    let stop_ctrl = stop.clone();
    let master_ctrl = master.clone();
    let app_ctrl = app.clone();
    let jitter_ms = cfg.jitter_ms;

    let control_join = std::thread::Builder::new()
        .name("receiver-gui-control".into())
        .spawn(move || {
            let _ = app_ctrl.emit("receiver://event", ReceiverEvent::Waiting);

            let mut recv_buf = vec![0u8; MAX_UDP_PAYLOAD];
            let (first_packet, first_src) = loop {
                if stop_ctrl.load(Ordering::Relaxed) {
                    let _ = app_ctrl.emit("receiver://event", ReceiverEvent::Stopped);
                    return;
                }
                match socket.recv_packet(&mut recv_buf) {
                    Ok(pair) => break pair,
                    Err(e) if is_timeout(&e) => continue,
                    Err(e) => {
                        tracing::warn!(error = %e, "dropped malformed/unreadable datagram while waiting for stream");
                        continue;
                    }
                }
            };

            let channels = first_packet.channels;
            let sample_rate = first_packet.sample_rate;
            let frames_per_packet = first_packet.samples.len() / channels.max(1) as usize;
            let packet_duration_ms = frames_per_packet as f64 * 1000.0 / sample_rate as f64;
            let jitter_cfg = JitterConfig::from_millis(jitter_ms, packet_duration_ms);
            let jitter_buffer = Arc::new(Mutex::new(JitterBuffer::new(jitter_cfg, channels, frames_per_packet)));
            jitter_buffer.lock().push(first_packet);

            let recv_join = {
                let jitter_buffer = jitter_buffer.clone();
                let stop = stop_ctrl.clone();
                std::thread::Builder::new()
                    .name("receiver-gui-udp-recv".into())
                    .spawn(move || {
                        network::raise_current_thread_priority();
                        let mut buf = vec![0u8; MAX_UDP_PAYLOAD];
                        while !stop.load(Ordering::Relaxed) {
                            match socket.recv_packet(&mut buf) {
                                Ok((pkt, _src)) => jitter_buffer.lock().push(pkt),
                                Err(e) if is_timeout(&e) => continue,
                                Err(e) => tracing::warn!(error = %e, "dropped malformed/unreadable datagram"),
                            }
                        }
                    })
                    .expect("failed to spawn UDP receive thread")
            };

            let output = {
                let jitter_buffer = jitter_buffer.clone();
                let master = master_ctrl.clone();
                PlaybackOutput::open(channels as u16, sample_rate, move |data: &mut [f32]| {
                    jitter_buffer.lock().read(data);
                    master.apply(data);
                })
            };
            let mut output = match output {
                Ok(o) => o,
                Err(message) => {
                    stop_ctrl.store(true, Ordering::Relaxed);
                    let _ = recv_join.join();
                    let _ = app_ctrl.emit("receiver://event", ReceiverEvent::Error { message });
                    return;
                }
            };

            while !stop_ctrl.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                output.poll();
                let stats = jitter_buffer.lock().stats();
                let _ = app_ctrl.emit(
                    "receiver://event",
                    ReceiverEvent::Playing {
                        from: first_src.to_string(),
                        sample_rate,
                        channels,
                        received: stats.received,
                        late_dropped: stats.late_dropped,
                        duplicate_dropped: stats.duplicate_dropped,
                        underruns: stats.underruns,
                        master_volume: master_ctrl.volume(),
                        master_muted: master_ctrl.is_muted(),
                    },
                );
            }

            drop(output);
            let _ = recv_join.join();
            let _ = app_ctrl.emit("receiver://event", ReceiverEvent::Stopped);
        })
        .map_err(|e| e.to_string())?;

    *guard = Some(ReceiverSession { stop, control_join: Some(control_join), master });
    Ok(local_addr.to_string())
}

#[tauri::command]
fn stop_receiver(state: State<AppState>) -> Result<(), String> {
    let mut guard = state.0.lock();
    *guard = None; // Drop on ReceiverSession stops the control (+ recv, + stream) threads and joins them.
    Ok(())
}

#[tauri::command]
fn set_master_volume(state: State<AppState>, volume: f32) -> Result<(), String> {
    let guard = state.0.lock();
    match guard.as_ref() {
        Some(session) => {
            session.master.set_volume(volume);
            Ok(())
        }
        None => Err("receiver is not running".into()),
    }
}

#[tauri::command]
fn set_master_mute(state: State<AppState>, muted: bool) -> Result<(), String> {
    let guard = state.0.lock();
    match guard.as_ref() {
        Some(session) => {
            session.master.set_muted(muted);
            Ok(())
        }
        None => Err("receiver is not running".into()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![start_receiver, stop_receiver, set_master_volume, set_master_mute])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
