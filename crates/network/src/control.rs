//! TCP control channel: reliable, low-volume [`ControlMessage`] exchange.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};

use protocol::{read_control_message, write_control_message, ControlMessage, ProtocolOrIo};

pub struct ControlListener {
    listener: TcpListener,
}

impl ControlListener {
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self { listener: TcpListener::bind(addr)? })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Blocks until a peer connects.
    pub fn accept(&self) -> io::Result<ControlConnection> {
        let (stream, peer) = self.listener.accept()?;
        stream.set_nodelay(true)?;
        Ok(ControlConnection { stream, peer })
    }
}

pub struct ControlConnection {
    stream: TcpStream,
    pub peer: SocketAddr,
}

impl ControlConnection {
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        let peer = stream.peer_addr()?;
        Ok(Self { stream, peer })
    }

    /// A connection's lifecycle doubles as a liveness signal: disconnect
    /// means the peer is gone.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { stream: self.stream.try_clone()?, peer: self.peer })
    }

    pub fn send(&mut self, msg: &ControlMessage) -> Result<(), ProtocolOrIo> {
        write_control_message(&mut self.stream, msg)
    }

    /// Blocks until one message arrives.
    pub fn recv(&mut self) -> Result<ControlMessage, ProtocolOrIo> {
        read_control_message(&mut self.stream)
    }
}
