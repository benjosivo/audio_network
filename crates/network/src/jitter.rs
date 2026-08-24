//! Turns arriving (possibly reordered/lost/duplicated) [`AudioPacket`]s into
//! a steady stream of audio frames for playback.
//!
//! `push` is called from the UDP receive thread; `read` is called from the
//! audio output callback. They only ever touch the buffer through a shared
//! `Mutex` (see `apps/receiver`), so both sides here assume single-threaded,
//! non-reentrant access.

use protocol::AudioPacket;

#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    /// Playback doesn't start until this many packets are buffered.
    pub target_packets: usize,
    /// Ring capacity. Packets that would land more than this many slots
    /// ahead of the next one to play are dropped rather than accepted.
    pub max_packets: usize,
}

impl JitterConfig {
    /// Derives a config from a target buffer depth in milliseconds and the
    /// duration of one packet in milliseconds.
    pub fn from_millis(target_jitter_ms: u64, packet_duration_ms: f64) -> Self {
        let target = ((target_jitter_ms as f64) / packet_duration_ms).ceil().max(1.0) as usize;
        Self { target_packets: target, max_packets: (target * 3).max(target + 4) }
    }
}

#[derive(Debug, Clone, Default)]
pub struct JitterStats {
    pub received: u64,
    pub late_dropped: u64,
    pub duplicate_dropped: u64,
    pub underruns: u64,
}

pub struct JitterBuffer {
    cfg: JitterConfig,
    channels: usize,
    frames_per_packet: usize,
    slots: Vec<Option<AudioPacket>>,
    next_play_seq: Option<u32>,
    play_frame_cursor: usize,
    started: bool,
    stats: JitterStats,
    last_frame: Vec<f32>,
    drop_scratch: Vec<f32>,
}

impl JitterBuffer {
    pub fn new(cfg: JitterConfig, channels: u8, frames_per_packet: usize) -> Self {
        let channels = channels.max(1) as usize;
        Self {
            slots: vec![None; cfg.max_packets.max(1)],
            channels,
            frames_per_packet: frames_per_packet.max(1),
            next_play_seq: None,
            play_frame_cursor: 0,
            started: false,
            stats: JitterStats::default(),
            last_frame: vec![0.0; channels],
            drop_scratch: vec![0.0; channels],
            cfg,
        }
    }

    /// Accepts one packet off the network. Drops it (bumping the relevant
    /// stat) if it's a duplicate, already played, or so far ahead it would
    /// overrun the ring.
    pub fn push(&mut self, pkt: AudioPacket) {
        self.stats.received += 1;
        let cap = self.slots.len() as u32;

        if self.next_play_seq.is_none() {
            self.next_play_seq = Some(pkt.seq);
        }
        let mut next = self.next_play_seq.unwrap();
        let diff = pkt.seq.wrapping_sub(next) as i32;

        if self.started && diff < 0 {
            self.stats.late_dropped += 1;
            return;
        }
        if !self.started && diff < 0 {
            // Reordering during prefill: this packet is actually earlier
            // than anything seen so far, so it's the real start of stream.
            self.next_play_seq = Some(pkt.seq);
            next = pkt.seq;
        }

        let diff = pkt.seq.wrapping_sub(next) as i32;
        if diff as u32 >= cap {
            self.stats.late_dropped += 1;
            return;
        }

        let slot = (pkt.seq % cap) as usize;
        if self.slots[slot].as_ref().is_some_and(|p| p.seq == pkt.seq) {
            self.stats.duplicate_dropped += 1;
            return;
        }
        self.slots[slot] = Some(pkt);

        if !self.started && self.occupancy() >= self.cfg.target_packets {
            self.started = true;
        }
    }

    /// Fills `out` (interleaved, `out.len()` should be a multiple of the
    /// channel count) with the next slice of audio. Silence until prefilled;
    /// zero-fills (and counts an underrun) across any gap in arrivals.
    pub fn read(&mut self, out: &mut [f32]) {
        let ch = self.channels;
        out.fill(0.0);
        if !self.started {
            return;
        }

        let mut written = 0;
        // Gentle drift correction: at most one frame nudge per `read` call,
        // so sender/receiver clock drift (tens of ppm) doesn't slowly grow
        // or drain the buffer over a long session.
        let occ = self.occupancy();
        if occ > self.cfg.target_packets + 2 {
            self.take_frame_into_scratch();
        } else if occ + 2 < self.cfg.target_packets && out.len() >= ch {
            out[..ch].copy_from_slice(&self.last_frame);
            written = ch;
        }

        while written + ch <= out.len() {
            let dst_end = written + ch;
            self.take_frame(written, dst_end, out);
            written = dst_end;
        }
    }

    pub fn stats(&self) -> JitterStats {
        self.stats.clone()
    }

    fn occupancy(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    fn take_frame_into_scratch(&mut self) {
        let mut scratch = std::mem::take(&mut self.drop_scratch);
        self.take_frame(0, self.channels, &mut scratch);
        self.drop_scratch = scratch;
    }

    /// Writes one frame (`channels` samples) into `out[dst_start..dst_end]`,
    /// advancing playback position by exactly one frame.
    fn take_frame(&mut self, dst_start: usize, dst_end: usize, out: &mut [f32]) {
        let cap = self.slots.len() as u32;
        let next = self.next_play_seq.expect("read() called before any push()");
        let slot = (next % cap) as usize;

        match &self.slots[slot] {
            Some(pkt) if pkt.seq == next => {
                let start = self.play_frame_cursor * self.channels;
                out[dst_start..dst_end].copy_from_slice(&pkt.samples[start..start + self.channels]);
                self.play_frame_cursor += 1;
                if self.play_frame_cursor * self.channels >= pkt.samples.len() {
                    self.slots[slot] = None;
                    self.play_frame_cursor = 0;
                    self.next_play_seq = Some(next.wrapping_add(1));
                }
            }
            _ => {
                // Count one underrun per missing packet-worth of gap, not per
                // frame — a 4-frame gap is one dropout event, not four.
                if self.play_frame_cursor == 0 {
                    self.stats.underruns += 1;
                }
                out[dst_start..dst_end].fill(0.0);
                self.play_frame_cursor += 1;
                if self.play_frame_cursor >= self.frames_per_packet {
                    self.play_frame_cursor = 0;
                    self.next_play_seq = Some(next.wrapping_add(1));
                }
            }
        }
        self.last_frame.copy_from_slice(&out[dst_start..dst_end]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::SampleFormat;

    fn packet(seq: u32, channels: u8, frames: usize, value: f32) -> AudioPacket {
        AudioPacket {
            seq,
            pts_us: seq as u64 * 1000,
            sample_rate: 48_000,
            channels,
            format: SampleFormat::F32,
            samples: vec![value; frames * channels as usize],
        }
    }

    #[test]
    fn silent_until_prefilled() {
        let cfg = JitterConfig { target_packets: 2, max_packets: 8 };
        let mut jb = JitterBuffer::new(cfg, 2, 4);
        jb.push(packet(0, 2, 4, 1.0));
        let mut out = vec![9.0; 8];
        jb.read(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "should stay silent below target_packets");
    }

    #[test]
    fn plays_in_order_after_prefill() {
        let cfg = JitterConfig { target_packets: 2, max_packets: 8 };
        let mut jb = JitterBuffer::new(cfg, 1, 4);
        jb.push(packet(0, 1, 4, 1.0));
        jb.push(packet(1, 1, 4, 2.0));
        let mut out = vec![0.0; 4];
        jb.read(&mut out);
        assert_eq!(out, vec![1.0; 4]);
        let mut out2 = vec![0.0; 4];
        jb.read(&mut out2);
        assert_eq!(out2, vec![2.0; 4]);
    }

    #[test]
    fn missing_packet_zero_fills_and_counts_underrun() {
        let cfg = JitterConfig { target_packets: 2, max_packets: 8 };
        let mut jb = JitterBuffer::new(cfg, 1, 4);
        jb.push(packet(0, 1, 4, 1.0));
        jb.push(packet(2, 1, 4, 3.0)); // seq 1 never arrives
        let mut out = vec![0.0; 4];
        jb.read(&mut out); // plays seq 0
        assert_eq!(out, vec![1.0; 4]);
        let mut out2 = vec![9.0; 4];
        jb.read(&mut out2); // seq 1 missing -> silence + underrun
        assert_eq!(out2, vec![0.0; 4]);
        assert_eq!(jb.stats().underruns, 1);
    }

    #[test]
    fn duplicate_packet_is_dropped() {
        let cfg = JitterConfig { target_packets: 1, max_packets: 8 };
        let mut jb = JitterBuffer::new(cfg, 1, 4);
        jb.push(packet(0, 1, 4, 1.0));
        jb.push(packet(0, 1, 4, 5.0));
        assert_eq!(jb.stats().duplicate_dropped, 1);
    }
}
