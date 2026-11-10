//! Sender-side mixing (per-source volume/mute, summed into one shared mix)
//! and receiver-side local master gain.
//!
//! Master gain is intentionally *not* part of `Mixer`: each receiver applies
//! its own [`MasterGain`] locally to what it plays, so it never crosses the
//! network. Only per-source volume/mute is shared (mixed once on the
//! sender, identical for every receiver).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub mod output;

pub type SourceId = u32;

/// One sender-side mixing-console channel: a source's volume/mute fader.
/// Lock-free so a future control-message handler thread can update it
/// without contending with the real-time mixing thread.
pub struct SourceChannel {
    pub id: SourceId,
    volume_bits: AtomicU32,
    muted: AtomicBool,
}

impl SourceChannel {
    fn new(id: SourceId, initial_volume: f32) -> Self {
        Self { id, volume_bits: AtomicU32::new(initial_volume.to_bits()), muted: AtomicBool::new(false) }
    }

    pub fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    fn gain(&self) -> f32 {
        if self.is_muted() { 0.0 } else { self.volume() }
    }
}

/// Combines multiple sources (today, milestone 1: just one) into a single
/// interleaved output buffer using each source's [`SourceChannel`] gain.
#[derive(Default)]
pub struct Mixer {
    sources: Vec<Arc<SourceChannel>>,
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(&mut self, id: SourceId) -> Arc<SourceChannel> {
        self.add_source_with_volume(id, 1.0)
    }

    pub fn add_source_with_volume(&mut self, id: SourceId, initial_volume: f32) -> Arc<SourceChannel> {
        let ch = Arc::new(SourceChannel::new(id, initial_volume));
        self.sources.push(ch.clone());
        ch
    }

    /// Sums every `(source_id, samples)` pair in `inputs` into `out`
    /// (already-interleaved, same length as each input buffer), applying
    /// each source's current gain. `out` is zeroed first.
    ///
    /// Panics if `inputs` references a source id that was never added via
    /// `add_source` — this is a programming error (mixer topology is set up
    /// once at startup), not a runtime condition to recover from.
    pub fn process(&self, inputs: &[(SourceId, &[f32])], out: &mut [f32]) {
        out.fill(0.0);
        for (id, buf) in inputs {
            let ch = self.sources.iter().find(|c| c.id == *id).expect("unknown source id passed to Mixer::process");
            let gain = ch.gain();
            for (o, s) in out.iter_mut().zip(buf.iter()) {
                *o += s * gain;
            }
        }
    }
}

/// Receiver-local master volume/mute. Applied in-place to decoded audio
/// right before it reaches the output device. Never sent over the network.
pub struct MasterGain {
    volume_bits: AtomicU32,
    muted: AtomicBool,
}

impl MasterGain {
    pub fn new(initial_volume: f32, initial_muted: bool) -> Self {
        Self {
            volume_bits: AtomicU32::new(initial_volume.clamp(0.0, 2.0).to_bits()),
            muted: AtomicBool::new(initial_muted),
        }
    }

    pub fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn apply(&self, buf: &mut [f32]) {
        let gain = if self.is_muted() { 0.0 } else { self.volume() };
        for s in buf.iter_mut() {
            *s *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_source_passthrough_at_unity_gain() {
        let mut mixer = Mixer::new();
        mixer.add_source(1);
        let input = [0.1_f32, 0.2, -0.3];
        let mut out = [0.0; 3];
        mixer.process(&[(1, &input)], &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn mute_silences_a_source() {
        let mut mixer = Mixer::new();
        let ch = mixer.add_source(1);
        ch.set_muted(true);
        let input = [0.5_f32; 4];
        let mut out = [1.0; 4];
        mixer.process(&[(1, &input)], &mut out);
        assert_eq!(out, [0.0; 4]);
    }

    #[test]
    fn master_gain_applies_in_place() {
        let master = MasterGain::new(0.5, false);
        let mut buf = [1.0_f32, 1.0, 1.0];
        master.apply(&mut buf);
        assert_eq!(buf, [0.5, 0.5, 0.5]);
        master.set_muted(true);
        master.apply(&mut buf);
        assert_eq!(buf, [0.0, 0.0, 0.0]);
    }
}
