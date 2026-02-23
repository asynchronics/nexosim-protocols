//! # A lightweight UDP wrapper to be used with I/O port.

use std::io::{ErrorKind, Result as IoResult};
use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};
use serde::{Deserialize, Serialize};

use crate::port::IoPort;

/// Data to be sent through the interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Data {
    /// Address on the other side.
    pub addr: SocketAddr,

    /// Bytes sent/received.
    pub bytes: Bytes,
}

/// UDP port.
#[derive(Debug)]
pub struct Udp {
    socket: UdpSocket,
    buffer: Vec<u8>,
}

impl Udp {
    /// Creates new UDP port bound to the provided address.
    pub fn new(addr: SocketAddr, buf_size: usize) -> Self {
        Self {
            socket: UdpSocket::bind(addr).unwrap(),
            buffer: vec![0; buf_size],
        }
    }
}

impl IoPort<UdpSocket, Data, Data> for Udp {
    fn register(&mut self, registry: &Registry) -> Token {
        registry
            .register(&mut self.socket, Token(0), Interest::READABLE)
            .unwrap();
        // Token used for waking up.
        Token(1)
    }

    fn read(&mut self, token: Token) -> IoResult<Data> {
        // Only read token shall be passed as argument.
        if token == Token(0) {
            self.socket
                .recv_from(&mut self.buffer)
                .map(|(len, addr)| Data {
                    addr,
                    bytes: BytesMut::from(&self.buffer[..len]).into(),
                })
        } else {
            // Unknown event: should never happen.
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "Unknown event.",
            ))
        }
    }

    fn write(&mut self, data: &Data) -> IoResult<()> {
        self.socket.send_to(&data.bytes, data.addr).map(|len| {
            if len != data.bytes.len() {
                Err(std::io::Error::other(format!(
                    "Not all bytes written: had to write {}, but wrote {}.",
                    data.bytes.len(),
                    len
                )))
            } else {
                Ok(())
            }
        })?
    }
}
