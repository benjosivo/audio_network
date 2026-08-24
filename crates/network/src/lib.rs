//! Networking for the audio-over-Ethernet app: a UDP audio transport with a
//! jitter buffer for playback timing, and a TCP control channel.

pub mod audio;
pub mod control;
pub mod jitter;

pub use audio::UdpAudioSocket;
pub use control::{ControlConnection, ControlListener};
pub use jitter::{JitterBuffer, JitterConfig, JitterStats};
