//! # Ports I/O utilities.
//!
//! This module contains utilities useful for external I/O ports support in
//! NeXosim simulation benches.
//!
//! ## I/O ports and I/O threads
//!
//! To communicate with the external world a NeXosim model can use an
//! [`IoThread`]. This is a thread guard that spawns a thread in its constructor
//! and joins it in the destructor.
//!
//! [`IoThread`] provides the [`IoThread::send`] to send data to the background
//! thread. It automatically forwards data received from the background thread
//! to the model which injector is specified at construction.
//!
//! The [`IoThread`] constructor accepts an implementor of the [`IoPort`] trait.
//! This trait enables the registration of I/O ports and manages communication
//! with them.
//!
//! #### Examples
//!
//! An I/O port that uses UDP for communication with the external world:
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
//!                 Err(std::io::Error::other(
//!                     format!(
//!                         "Only {} bytes of {} have been written.",
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
#![allow(deprecated)]

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Result as IoResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{
    RecvError as MpscRecvError, SendError as MpscSendError, Sender,
    TryRecvError as MpscTryRecvError, channel,
};
use std::thread;

use mio::event::Source;
use mio::{Events, Poll, Registry, Token, Waker};
use nexosim::model::{Model, SchedulableId};
use nexosim::simulation::ModelInjector;

use thread_guard::ThreadGuard;

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

    /// Reads data corresponding to the token.
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

    /// The sender end is disconnected.
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

/// Recv error.
#[derive(Debug)]
pub struct RecvError {}

impl From<MpscRecvError> for RecvError {
    fn from(_: MpscRecvError) -> Self {
        Self {}
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Recv error")
    }
}

impl Error for RecvError {}

/// I/O thread.
pub struct IoThread<T>
where
    T: Send,
{
    /// I/O thread guard.
    _io_thread: ThreadGuard<()>,

    /// Sender to be used by the model.
    transmitter: Sender<T>,

    /// Thread waker.
    waker: Arc<Waker>,
}

impl<T> IoThread<T>
where
    T: Send + 'static,
{
    /// Creates a new I/O thread.
    pub fn new<S, P, R, M>(
        mut port: P,
        injector: ModelInjector<M>,
        data_out: SchedulableId<M, R>,
        disconnected: SchedulableId<M, ()>,
    ) -> Self
    where
        S: Source + ?Sized,
        P: IoPort<S, R, T> + Send + 'static,
        R: Clone + Send + 'static,
        M: Model,
    {
        let (transmitter, rx) = channel();

        let is_halted = Arc::new(AtomicBool::new(false));
        let guard_is_halted = is_halted.clone();

        let mut poll = Poll::new().unwrap();
        let wake = port.register(poll.registry());
        let waker = Arc::new(Waker::new(poll.registry(), wake).unwrap());
        let guard_waker = waker.clone();

        // I/O thread.
        let io_thread = thread::spawn(move || {
            let mut events = Events::with_capacity(256);
            'poll: loop {
                // This call is blocking.
                poll.poll(&mut events, None).unwrap();

                for event in events.iter() {
                    let token = event.token();
                    if token == wake {
                        if is_halted.load(Ordering::Relaxed) {
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
                                    injector.inject_event(&data_out, message);
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    break;
                                }
                                Err(_) => {
                                    break 'poll;
                                }
                            }
                        }
                    }
                }
            }
            injector.inject_event(&disconnected, ());
        });

        Self {
            _io_thread: ThreadGuard::with_pre_action(io_thread, move |_| {
                guard_is_halted.store(true, Ordering::Relaxed);
                let _ = guard_waker.wake();
                // The waker must live long enough for the wake signal to be
                // delivered. Event though a clone is stored in the parent
                // structure, a waker is also returned to the caller to be more
                // resilient towards modifications of the code. Otherwise, the
                // fields order could impact the waking mechanism.
                guard_waker
            }),
            transmitter,
            waker,
        }
    }

    /// Sends data to the I/O thread.
    pub fn send(&mut self, data: T) -> Result<(), SendError> {
        self.transmitter.send(data)?;
        self.waker.wake()?;
        Ok(())
    }
}

impl<T> fmt::Debug for IoThread<T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("IoThread").finish_non_exhaustive()
    }
}
