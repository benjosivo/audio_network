//! UDP transport for [`protocol::AudioPacket`]s.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use protocol::AudioPacket;
use socket2::{Domain, Protocol, Socket, Type};

/// Kernel receive buffer requested for the audio socket. The default (64 KiB
/// on Windows) overflows after ~100 ms if the receive thread is starved by
/// another busy process, silently dropping packets.
const RECV_BUFFER_BYTES: usize = 512 * 1024;

pub struct UdpAudioSocket {
    sock: UdpSocket,
}

impl UdpAudioSocket {
    pub fn bind(local_addr: SocketAddr) -> io::Result<Self> {
        let socket = Socket::new(Domain::for_address(local_addr), Type::DGRAM, Some(Protocol::UDP))?;
        if let Err(e) = socket.set_recv_buffer_size(RECV_BUFFER_BYTES) {
            tracing::warn!(error = %e, "could not enlarge UDP receive buffer");
        }
        socket.bind(&local_addr.into())?;
        Ok(Self { sock: socket.into() })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Sets a timeout for [`recv_packet`](Self::recv_packet). A caller that
    /// needs to poll a stop flag rather than block forever (e.g. a GUI with
    /// a Stop button) should set this and treat the resulting `WouldBlock`/
    /// `TimedOut` error from `recv_packet` as "no packet yet", not a real
    /// failure.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }

    /// Encodes `pkt` once and fans it out to every destination.
    pub fn send_packet_to_many(&self, pkt: &AudioPacket, dests: &[SocketAddr]) -> io::Result<()> {
        let encoded = pkt.encode();
        for dest in dests {
            self.sock.send_to(&encoded, dest)?;
        }
        Ok(())
    }

    /// Blocks until one datagram arrives and decodes it.
    ///
    /// `buf` should be at least [`protocol::MAX_UDP_PAYLOAD`] bytes. A
    /// datagram that fails to decode as an [`AudioPacket`] (e.g. stray
    /// traffic on the port) is surfaced as an `io::Error` for the caller to
    /// log and skip; it does not close the socket.
    pub fn recv_packet(&self, buf: &mut [u8]) -> io::Result<(AudioPacket, SocketAddr)> {
        let (len, src) = self.sock.recv_from(buf)?;
        let pkt = AudioPacket::decode(&buf[..len]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok((pkt, src))
    }
}
