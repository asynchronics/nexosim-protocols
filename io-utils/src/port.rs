//! Ports I/O utilities.
//!
//! This module contains utilities useful for external I/O ports support in
//! NeXosim simulation benches.
//!
//! # I/O ports and I/O threads
//!
//! To communicate with the external world a NeXosim model can use an
//! [`IoThread`]. This is a thread guard that spawns a thread in its constructor
//! and joins it in the destructor.
//!
//! The [`IoThread`] structure provides two methods to communicate with the
//! external thread from the model:
//!
//! * [`IoThread::try_recv`] that tries to receive data from the external port,
//! * [`IoThread::send`] that sends data to the external port.
//!
//! The [`IoThread`] constructor accepts an implementor of the [`IoPort`]
//! trait. This trait allows registering of the I/O port in MIO and
//! reading/writing data.
//!
//! #### Examples
//!
//! I/O port that uses UDP for communication with the external world:
//!
//! ```
//! use std::io::{ErrorKind, Result as IoResult};
//! use std::net::SocketAddr;
//!
//! use bytes::{Bytes, BytesMut};
//! use mio::net::UdpSocket;
//! use mio::{Interest, Registry, Token};
//!
//! use nexosim_io_utils::port::{IoPort};
//!
//! /// Data to be sent through the interface.
//! #[derive(Clone, Debug, PartialEq)]
//! struct Data {
//!     addr: SocketAddr,
//!     bytes: Bytes,
//! }
//!
//! /// UDP port.
//! struct Udp {
//!     socket: UdpSocket,
//!     buffer: Vec<u8>,
//! }
//!
//! impl Udp {
//!     /// Creates new UDP port bound to the provided address.
//!     pub fn new(addr: SocketAddr) -> Self {
//!         Self {
//!             socket: UdpSocket::bind(addr).unwrap(),
//!             buffer: vec![0; 256],
//!         }
//!     }
//! }
//!
//! impl IoPort<UdpSocket, Data, Data> for Udp {
//!     fn register(&mut self, registry: &Registry) -> Token {
//!         registry
//!             .register(&mut self.socket, Token(0), Interest::READABLE)
//!             .unwrap();
//!         Token(1)
//!     }
//!
//!     fn read(&mut self, token: Token) -> IoResult<Data> {
//!         if token == Token(0) {
//!             self.socket
//!                 .recv_from(&mut self.buffer)
//!                 .map(|(len, addr)| Data {
//!                     addr,
//!                     bytes: BytesMut::from(&self.buffer[..len]).into(),
//!                 })
//!         } else {
//!             // Unknown event: should never happen.
//!             Err(std::io::Error::new(
//!                 ErrorKind::InvalidInput,
//!                 "Unknown event.",
//!             ))
//!         }
//!     }
//!
//!     fn write(&mut self, data: &Data) -> IoResult<()> {
//!         self.socket.send_to(&data.bytes, data.addr).map(|len| {
//!             if len != data.bytes.len() {
//!                 Err(std::io::Error::new(
//!                     ErrorKind::Other,
//!                     format!(
//!                         "Not all bytes written: had to write {}, but wrote {}.",
//!                         data.bytes.len(),
//!                         len
//!                     ),
//!                 ))
//!             } else {
//!                 Ok(())
//!             }
//!         })?
//!     }
//! }
//! ```

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Result as IoResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{
    Receiver, SendError as MpscSendError, Sender, TryRecvError as MpscTryRecvError, channel,
};
use std::thread;

use mio::event::Source;
use mio::{Events, Poll, Registry, Token, Waker};

use nexosim_util::joiners::ThreadJoiner;

/// I/O port(s) usable by MIO.
pub trait IoPort<S, R, T>
where
    S: Source + ?Sized,
    R: Send,
    T: Send,
{
    /// Registers port(s) in MIO.
    ///
    /// This function should return waker token.
    fn register(&mut self, registry: &Registry) -> Token;

    /// Reads data corresponding to token.
    fn read(&mut self, token: Token) -> IoResult<R>;

    /// Writes data.
    fn write(&mut self, data: &T) -> IoResult<()>;
}

/// Send error.
#[derive(Debug)]
pub enum SendError {
    /// Receiver end is disconnected.
    Disonnected,

    /// I/O error.
    IoError(std::io::Error),
}

impl<T> From<MpscSendError<T>> for SendError {
    fn from(_: MpscSendError<T>) -> Self {
        Self::Disonnected
    }
}

impl From<std::io::Error> for SendError {
    fn from(error: std::io::Error) -> Self {
        Self::IoError(error)
    }
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Disonnected => write!(f, "sending on a closed channel"),
            Self::IoError(error) => error.fmt(f),
        }
    }
}

impl Error for SendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Disonnected => None,
            Self::IoError(error) => Some(error),
        }
    }
}

/// TryRecv error.
#[derive(Debug)]
pub enum TryRecvError {
    /// No data, would block.
    Empty,

    /// Sender end is disconnected.
    Disconnected,
}

impl From<MpscTryRecvError> for TryRecvError {
    fn from(error: MpscTryRecvError) -> Self {
        match error {
            MpscTryRecvError::Empty => Self::Empty,
            MpscTryRecvError::Disconnected => Self::Disconnected,
        }
    }
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "receiving on an empty channel"),
            Self::Disconnected => write!(f, "receiving on a closed channel"),
        }
    }
}

impl Error for TryRecvError {}

/// I/O thread.
pub struct IoThread<R, T>
where
    R: Send,
    T: Send,
{
    /// I/O thread handle.
    // This field must precede waker in order for drop to work properly.
    _io_thread: ThreadJoiner<()>,

    /// Data receiver.
    receiver: Receiver<R>,

    /// Data sender.
    transmitter: Sender<T>,

    /// Thread waker.
    waker: Arc<Waker>,

    /// Simulation halted flag.
    is_halted: Arc<AtomicBool>,
}

impl<R, T> IoThread<R, T>
where
    R: Send + 'static,
    T: Send + 'static,
{
    /// Creates new I/O thread.
    pub fn new<S, P>(mut port: P) -> Self
    where
        S: Source + ?Sized,
        P: IoPort<S, R, T> + Send + 'static,
    {
        let (tx, receiver) = channel();
        let (transmitter, rx) = channel();

        let is_halted = Arc::new(AtomicBool::new(false));
        let io_is_halted = is_halted.clone();

        let mut poll = Poll::new().unwrap();
        let wake = port.register(poll.registry());
        let waker = Arc::new(Waker::new(poll.registry(), wake).unwrap());

        // I/O thread.
        let io_thread = thread::spawn(move || {
            let mut events = Events::with_capacity(256);
            'poll: loop {
                // This call is blocking.
                poll.poll(&mut events, None).unwrap();

                for event in events.iter() {
                    let token = event.token();
                    if token == wake {
                        if io_is_halted.load(Ordering::Relaxed) {
                            break 'poll;
                        }
                        while let Ok(data) = rx.try_recv() {
                            if port.write(&data).is_err() {
                                break 'poll;
                            }
                        }
                    } else {
                        loop {
                            match port.read(token) {
                                Ok(message) => {
                                    if tx.send(message).is_err() {
                                        break 'poll;
                                    }
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    break;
                                }
                                _ => break 'poll,
                            }
                        }
                    }
                }
            }
        });
        Self {
            _io_thread: ThreadJoiner::new(io_thread),
            receiver,
            transmitter,
            waker,
            is_halted,
        }
    }

    /// Tries to receives data from I/O thread.
    pub fn try_recv(&self) -> Result<R, TryRecvError> {
        Ok(self.receiver.try_recv()?)
    }

    /// Sends data to I/O thread.
    pub fn send(&mut self, data: T) -> Result<(), SendError> {
        self.transmitter.send(data)?;
        self.waker.wake()?;
        Ok(())
    }
}

impl<R, T> Drop for IoThread<R, T>
where
    R: Send,
    T: Send,
{
    fn drop(&mut self) {
        self.is_halted.store(true, Ordering::Relaxed);
        let _ = self.waker.wake();
    }
}

impl<R, T> fmt::Debug for IoThread<R, T>
where
    R: Send,
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("IoThread").finish_non_exhaustive()
    }
}
