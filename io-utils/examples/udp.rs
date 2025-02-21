//! Example: an I/O thread to communicate using UDP.

use std::error::Error;
use std::io::{ErrorKind, Result as IoResult};
use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
use std::thread::{self, sleep};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};

use nexosim_util::joiners::ThreadJoiner;

use nexosim_io_utils::port::{IoPort, IoThread, TryRecvError};

const IO_THREAD_ADDR: &str = "127.0.0.1:34254";
const ECHO_THREAD_ADDR: &str = "127.0.0.1:34255";
const BUF_SIZE: usize = 65536;

/// Data to be sent through the interface.
#[derive(Clone, Debug, PartialEq)]
struct Data {
    addr: SocketAddr,
    bytes: Bytes,
}

/// UDP port.
struct Udp {
    socket: UdpSocket,
    buffer: Vec<u8>,
}

impl Udp {
    /// Creates new UDP port bound to the provided address.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            socket: UdpSocket::bind(addr).unwrap(),
            buffer: vec![0; BUF_SIZE],
        }
    }
}

impl IoPort<UdpSocket, Data, Data> for Udp {
    fn register(&mut self, registry: &Registry) -> Token {
        registry
            .register(&mut self.socket, Token(0), Interest::READABLE)
            .unwrap();
        Token(1)
    }

    fn read(&mut self, token: Token) -> IoResult<Data> {
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
                Err(std::io::Error::new(
                    ErrorKind::Other,
                    format!(
                        "Not all bytes written: had to write {}, but wrote {}.",
                        data.bytes.len(),
                        len
                    ),
                ))
            } else {
                Ok(())
            }
        })?
    }
}

/// Uses I/O thread to send data to echo UDP server.
fn main() -> Result<(), Box<dyn Error>> {
    // UDP I/O port.
    let udp = Udp::new(IO_THREAD_ADDR.parse()?);

    // I/O thread handling I/O port operations.
    let mut io_thread = IoThread::new(udp);

    // Echo UDP server.
    let echo_thread = ThreadJoiner::new(thread::spawn(|| -> std::io::Result<Bytes> {
        let socket = StdUdpSocket::bind(ECHO_THREAD_ADDR)?;
        let mut buf = [0; BUF_SIZE];
        let (len, addr) = socket.recv_from(&mut buf)?;
        sleep(Duration::from_secs(2));
        socket.send_to(&buf[..len], addr)?;
        Ok(BytesMut::from(&buf[..len]).into())
    }));

    // Data to be sent.
    let data = Data {
        addr: ECHO_THREAD_ADDR.parse().unwrap(),
        bytes: BytesMut::from([1_u8, 2, 3].as_slice()).into(),
    };

    // Wait to be sure that server has been started, in real-life some
    // synchronization should be done instead of a sleep.
    sleep(Duration::from_secs(1));

    // Send data via UDP.
    io_thread.send(data.clone())?;

    // It is not possible to return value from a for loop, so we are using a
    // counter.
    let mut counter = 5;
    // Try to receive data echoed by the server.
    let echoed = loop {
        if counter <= 0 {
            break Err(TryRecvError::Empty);
        }
        match io_thread.try_recv() {
            Ok(data) => break Ok(data),
            Err(TryRecvError::Empty) => {}
            Err(error) => break Err(error),
        }
        counter -= 1;
        sleep(Duration::from_secs(1));
    }?;

    assert_eq!(data, echoed);
    assert_eq!(data.bytes, echo_thread.join().unwrap()?);
    Ok(())
}
