//! Networking for the audio-over-Ethernet app: a UDP audio transport with a
//! jitter buffer for playback timing, and a TCP control channel.

pub mod audio;
pub mod control;
pub mod jitter;

/// Raises the calling thread's scheduling priority. Call from the UDP receive
/// thread so a CPU-hungry process can't delay it long enough to underrun the
/// jitter buffer (or, while it holds the buffer lock, block the audio callback).
/// Best effort: failure (e.g. missing privileges on Linux) is only logged.
pub fn raise_current_thread_priority() {
    use thread_priority::{ThreadPriority, set_current_thread_priority};
    if let Err(e) = set_current_thread_priority(ThreadPriority::Max) {
        tracing::debug!(error = ?e, "could not raise thread priority");
    }
}

pub use audio::UdpAudioSocket;
pub use control::{ControlConnection, ControlListener};
pub use jitter::{JitterBuffer, JitterConfig, JitterStats};
