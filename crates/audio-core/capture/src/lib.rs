//! Audio source capture, behind one backend-agnostic trait.
//!
//! A "source" can be a WASAPI loopback capture (what's currently playing on
//! the machine's default output device) or a physical input device
//! (mic/line-in) — [`AudioSource`] doesn't care which; callers pick a
//! concrete implementation and get interleaved `f32` samples out either way.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use rtrb::Producer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no default audio device found")]
    NoDefaultDevice,
    #[error("device error: {0}")]
    Device(String),
    #[error("unsupported stream config: {0}")]
    UnsupportedConfig(String),
    #[error("failed to start capture stream: {0}")]
    StreamStart(String),
}

/// RAII handle for a running capture stream: dropping it stops capture.
pub trait AudioStream: Send {}

/// A source of live interleaved `f32` audio.
pub trait AudioSource: Send {
    /// Human-readable label for logs/CLI output.
    fn name(&self) -> String;

    /// Starts capture on a dedicated thread, which pushes interleaved
    /// samples into `producer` until the returned [`AudioStream`] handle is
    /// dropped. The negotiated format is returned once the device is
    /// actually opened (not knowable ahead of time for WASAPI loopback,
    /// which follows whatever the output device's mix format currently is).
    fn start(self: Box<Self>, producer: Producer<f32>) -> Result<(Box<dyn AudioStream>, AudioFormat), CaptureError>;
}

#[cfg(windows)]
pub mod wasapi_loopback;

pub mod cpal_input;

/// Ring buffer capacity (in samples) handed to a source's `start()`.
/// Generous fixed size (~3 MB) so capture never has to allocate mid-stream;
/// sized for roughly 2 seconds at 192 kHz stereo, far more than the jitter
/// buffer or packetizer will ever actually let build up.
pub const CAPTURE_RING_CAPACITY_SAMPLES: usize = 192_000 * 2 * 2;

/// Shared [`AudioStream`] handle for sources that run entirely on their own
/// dedicated OS thread (true of every backend here: `cpal`'s stream object
/// and WASAPI's COM client both need to stay on the thread that created
/// them). Dropping it signals the thread to stop and joins it.
pub(crate) struct ThreadCaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl ThreadCaptureHandle {
    pub(crate) fn new(stop: Arc<AtomicBool>, join: thread::JoinHandle<()>) -> Self {
        Self { stop, join: Some(join) }
    }
}

impl AudioStream for ThreadCaptureHandle {}

impl Drop for ThreadCaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
