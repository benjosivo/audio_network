//! Wire protocol for the audio-over-Ethernet app.
//!
//! Two independent channels:
//!
//! - **Audio (UDP)**: each datagram is one self-contained [`AudioPacket`],
//!   encoded with a fixed binary header (no serde, to stay allocation-light
//!   on the capture/playback hot path). A lost or reordered datagram should
//!   never block later ones, so audio never uses the byte-stream framing
//!   below.
//! - **Control (TCP)**: a byte stream carrying length-prefixed, bincode-encoded
//!   [`ControlMessage`] values via [`write_control_message`] / [`read_control_message`].
//!   Small, infrequent messages that want reliable delivery (e.g. remote
//!   volume changes) and a connection lifecycle as a liveness signal.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// First byte of every audio datagram; a quick sanity check against stray traffic.
pub const AUDIO_PACKET_MAGIC: u8 = 0xA1;
pub const PROTOCOL_VERSION: u8 = 1;

/// magic(1) + version(1) + seq(4) + pts_us(8) + sample_rate(4) + channels(1) + format(1)
pub const AUDIO_HEADER_LEN: usize = 20;

/// Conservative budget under a 1500-byte Ethernet MTU, leaving headroom for
/// IP/UDP headers and e.g. 802.1Q tags, so audio packets never fragment.
pub const MAX_UDP_PAYLOAD: usize = 1400;

/// Safety cap for a single control message payload.
pub const MAX_CONTROL_PAYLOAD_LEN: usize = 1024 * 1024; // 1 MiB

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("bad audio packet magic: 0x{0:02x}")]
    BadMagic(u8),
    #[error("unsupported protocol version: {0}")]
    UnsupportedVersion(u8),
    #[error("unknown sample format: 0x{0:02x}")]
    UnknownSampleFormat(u8),
    #[error("audio packet too short: {0} bytes (header is {AUDIO_HEADER_LEN})")]
    AudioPacketTooShort(usize),
    #[error("malformed audio payload: {0} trailing bytes is not a multiple of a sample")]
    MalformedAudioPayload(usize),
    #[error("control payload too large: {0} bytes (max {MAX_CONTROL_PAYLOAD_LEN})")]
    ControlPayloadTooLarge(usize),
    #[error("failed to decode control message: {0}")]
    ControlDecode(String),
}

/// Combined error type for reading from a stream/socket (transport or protocol level).
#[derive(Debug, thiserror::Error)]
pub enum ProtocolOrIo {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Proto(#[from] ProtocolError),
}

/// Sample encoding carried in an [`AudioPacket`]. Only `F32` exists today;
/// this is kept explicit (rather than assumed) so a future format can be
/// added without breaking the header shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SampleFormat {
    F32 = 1,
}

impl SampleFormat {
    fn from_u8(v: u8) -> Result<Self, ProtocolError> {
        match v {
            1 => Ok(SampleFormat::F32),
            other => Err(ProtocolError::UnknownSampleFormat(other)),
        }
    }
}

/// One self-contained chunk of interleaved PCM audio, sized to fit in a
/// single UDP datagram.
#[derive(Debug, Clone)]
pub struct AudioPacket {
    /// Monotonically increasing sequence number (per stream). Used for
    /// gap/reorder/duplicate detection in the jitter buffer.
    pub seq: u32,
    /// Sender-relative capture timestamp in microseconds, derived from a
    /// running sample count (not wall-clock time).
    pub pts_us: u64,
    /// Sample rate the packet was captured at. Carried in-band so a receiver
    /// can configure playback from the very first packet, with no separate
    /// format-negotiation step.
    pub sample_rate: u32,
    pub channels: u8,
    pub format: SampleFormat,
    /// Interleaved samples: `[c0_0, c1_0, ..., c0_1, c1_1, ...]`.
    pub samples: Vec<f32>,
}

impl AudioPacket {
    /// Largest number of interleaved frames that fit in [`MAX_UDP_PAYLOAD`]
    /// for a given channel count, assuming `f32` samples.
    pub const fn max_frames_per_packet(channels: u8) -> usize {
        (MAX_UDP_PAYLOAD - AUDIO_HEADER_LEN) / (channels as usize * 4)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(AUDIO_HEADER_LEN + self.samples.len() * 4);
        out.push(AUDIO_PACKET_MAGIC);
        out.push(PROTOCOL_VERSION);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.pts_us.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.push(self.channels);
        out.push(self.format as u8);
        debug_assert_eq!(out.len(), AUDIO_HEADER_LEN);
        for s in &self.samples {
            out.extend_from_slice(&s.to_bits().to_le_bytes());
        }
        out
    }

    pub fn decode(datagram: &[u8]) -> Result<Self, ProtocolError> {
        if datagram.len() < AUDIO_HEADER_LEN {
            return Err(ProtocolError::AudioPacketTooShort(datagram.len()));
        }
        if datagram[0] != AUDIO_PACKET_MAGIC {
            return Err(ProtocolError::BadMagic(datagram[0]));
        }
        if datagram[1] != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(datagram[1]));
        }
        let seq = u32::from_le_bytes(datagram[2..6].try_into().unwrap());
        let pts_us = u64::from_le_bytes(datagram[6..14].try_into().unwrap());
        let sample_rate = u32::from_le_bytes(datagram[14..18].try_into().unwrap());
        let channels = datagram[18];
        let format = SampleFormat::from_u8(datagram[19])?;

        let payload = &datagram[AUDIO_HEADER_LEN..];
        if payload.len() % 4 != 0 {
            return Err(ProtocolError::MalformedAudioPayload(payload.len()));
        }
        let samples: Vec<f32> = payload
            .chunks_exact(4)
            .map(|c| f32::from_bits(u32::from_le_bytes(c.try_into().unwrap())))
            .collect();

        Ok(Self { seq, pts_us, sample_rate, channels, format, samples })
    }
}

/// Control messages exchanged between sender and receiver over the TCP
/// control connection.
///
/// Master volume/mute is deliberately NOT here: each receiver applies its
/// own local master gain to what it plays, so it never needs to cross the
/// network. Only the sender-side, shared per-source mix is remotely
/// controllable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Receiver → sender, sent right after connecting.
    Hello { name: String, udp_port: u16 },
    /// Sender → receiver, reply to `Hello`.
    Welcome { name: String },
    /// Either direction: ask the peer for a full state snapshot.
    RequestState,

    // ---- receiver → sender (remote control of the sender's shared mix) ----
    SetSourceVolume { source_id: u32, volume: f32 },
    SetSourceMute { source_id: u32, muted: bool },
}

impl ControlMessage {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        bincode::serialize(self).map_err(|e| ProtocolError::ControlDecode(e.to_string()))
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        bincode::deserialize(payload).map_err(|e| ProtocolError::ControlDecode(e.to_string()))
    }
}

/// Write one length-prefixed control message to a TCP stream.
pub fn write_control_message<W: Write>(writer: &mut W, msg: &ControlMessage) -> Result<(), ProtocolOrIo> {
    let payload = msg.encode().map_err(ProtocolOrIo::Proto)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Read one length-prefixed control message from a TCP stream.
pub fn read_control_message<R: Read>(reader: &mut R) -> Result<ControlMessage, ProtocolOrIo> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_CONTROL_PAYLOAD_LEN {
        return Err(ProtocolOrIo::Proto(ProtocolError::ControlPayloadTooLarge(len)));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(ControlMessage::decode(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_packet_roundtrip() {
        let pkt = AudioPacket {
            seq: 42,
            pts_us: 123_456_789,
            sample_rate: 48_000,
            channels: 2,
            format: SampleFormat::F32,
            samples: vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25],
        };
        let encoded = pkt.encode();
        let decoded = AudioPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.seq, pkt.seq);
        assert_eq!(decoded.pts_us, pkt.pts_us);
        assert_eq!(decoded.sample_rate, pkt.sample_rate);
        assert_eq!(decoded.channels, pkt.channels);
        assert_eq!(decoded.samples, pkt.samples);
    }

    #[test]
    fn control_message_roundtrip() {
        let msg = ControlMessage::SetSourceVolume { source_id: 3, volume: 0.75 };
        let mut buf = Vec::new();
        write_control_message(&mut buf, &msg).unwrap();
        let mut cursor = &buf[..];
        let decoded = read_control_message(&mut cursor).unwrap();
        match decoded {
            ControlMessage::SetSourceVolume { source_id, volume } => {
                assert_eq!(source_id, 3);
                assert_eq!(volume, 0.75);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = vec![0u8; AUDIO_HEADER_LEN];
        bytes[0] = 0xFF;
        assert!(matches!(AudioPacket::decode(&bytes), Err(ProtocolError::BadMagic(0xFF))));
    }
}
