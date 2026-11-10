//! Receiver playback through the default output device.
//!
//! An audio endpoint can vanish under a running stream (system sleep, HDMI or
//! USB device removed). cpal only reports the error and leaves the stream
//! dead, so [`PlaybackOutput::poll`] replaces a stream that stopped running.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Consecutive [`PlaybackOutput::poll`] calls without an audio callback before
/// the stream counts as dead. Counted in polls, not wall-clock time, so the
/// time a machine spends asleep never counts toward a stall.
const STALLED_POLLS: u32 = 4;

#[derive(Default)]
struct StallDetector {
    last_count: u64,
    idle_polls: u32,
}

impl StallDetector {
    /// Restarts its count after reporting a stall, so a failing reopen is
    /// retried every [`STALLED_POLLS`] polls rather than on every poll.
    fn is_stalled(&mut self, count: u64) -> bool {
        if count != self.last_count {
            self.last_count = count;
            self.idle_polls = 0;
            return false;
        }
        self.idle_polls += 1;
        if self.idle_polls < STALLED_POLLS {
            return false;
        }
        self.idle_polls = 0;
        true
    }
}

/// A playing output stream that [`poll`](Self::poll) keeps alive. `cpal::Stream`
/// is not `Send` on every platform, so keep this on the thread that opened it.
pub struct PlaybackOutput<F> {
    config: cpal::StreamConfig,
    fill: F,
    callbacks: Arc<AtomicU64>,
    stall: StallDetector,
    stream: Option<cpal::Stream>,
}

impl<F: Fn(&mut [f32]) + Clone + Send + 'static> PlaybackOutput<F> {
    /// Starts playback on the default output device. `fill` runs on the audio
    /// callback and writes interleaved samples into the buffer it is given.
    pub fn open(channels: u16, sample_rate: u32, fill: F) -> Result<Self, String> {
        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let callbacks = Arc::new(AtomicU64::new(0));
        let stream = open_stream(&config, fill.clone(), callbacks.clone())?;
        Ok(Self { config, fill, callbacks, stall: StallDetector::default(), stream: Some(stream) })
    }

    /// Reopens the current default device if the stream has stopped running.
    /// Call this regularly (about twice a second).
    pub fn poll(&mut self) {
        if !self.stall.is_stalled(self.callbacks.load(Ordering::Relaxed)) {
            return;
        }
        tracing::warn!("audio output stalled, reopening default output device");
        drop(self.stream.take()); // Release the dead endpoint before opening a new one.
        match open_stream(&self.config, self.fill.clone(), self.callbacks.clone()) {
            Ok(stream) => self.stream = Some(stream),
            Err(e) => tracing::warn!(error = %e, "could not reopen audio output, will retry"),
        }
    }
}

fn open_stream<F>(config: &cpal::StreamConfig, fill: F, callbacks: Arc<AtomicU64>) -> Result<cpal::Stream, String>
where
    F: Fn(&mut [f32]) + Send + 'static,
{
    // A new host each time: the default device can change while the machine sleeps.
    let device = cpal::default_host().default_output_device().ok_or("no default output device")?;
    tracing::info!(device = ?device.name().unwrap_or_default(), "opening output device");
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                callbacks.fetch_add(1, Ordering::Relaxed);
                fill(data);
            },
            |err| tracing::error!("cpal output stream error: {err}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_stream_never_stalls() {
        let mut detector = StallDetector::default();
        assert!((1..100).all(|count| !detector.is_stalled(count)));
    }

    #[test]
    fn stall_is_reported_after_idle_polls_and_retried() {
        let mut detector = StallDetector::default();
        for _ in 0..2 {
            assert!((1..STALLED_POLLS).all(|_| !detector.is_stalled(0)));
            assert!(detector.is_stalled(0));
        }
    }

    #[test]
    fn callback_progress_clears_idle_polls() {
        let mut detector = StallDetector::default();
        assert!((1..STALLED_POLLS).all(|_| !detector.is_stalled(0)));
        assert!(!detector.is_stalled(1));
        assert!((1..STALLED_POLLS).all(|_| !detector.is_stalled(1)));
    }
}
