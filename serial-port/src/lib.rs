//! Serial port model for [NeXosim][NX]-based simulations.
//!
//! This model
//! * listens the specified serial ports injecting data from it into the
//!   simulation,
//! * outputs data from the simulation to the specified serial port.
//!
//! [NX]: https://github.com/asynchronics/nexosim
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bytes::{Bytes, BytesMut};

use confique::Config;

use mio::{Events, Interest, Poll, Token, Waker};
use mio_serial::SerialPortBuilderExt;

#[cfg(feature = "tracing")]
use tracing::info;

use nexosim::model::{Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::Output;

use nexosim_util::joiners::ThreadJoiner;

/// Serial port model instance config.
#[derive(Config, Debug)]
pub struct SerialPortConfig {
    /// Baud rate.
    ///
    /// Zero value shall be used for software TTY interfaces.
    #[config(default = 0)]
    pub baud_rate: u32,

    /// Serial port path.
    #[config(default = "/tmp/ttyS20")]
    pub port_path: String,

    /// Internal buffer size.
    ///
    /// Input is read and injected into the simulation by blocks up to buffer
    /// size.
    #[config(default = 256)]
    pub buffer_size: usize,

    /// Time shift, in milliseconds, for scheduling events at the present moment.
    ///
    /// If no value is provided, `period` is used.
    pub delta: Option<u64>,

    /// Activation period, in milliseconds, for cyclic activities inside the simulation.
    ///
    /// If no value is provided, cyclic activities are not scheduled
    /// automatically.
    pub period: Option<u64>,
}

/// Serial port model.
///
/// This model
/// * listens the specified serial port and injects into the simulation values
///   read from it as raw bytes,
/// * outputs raw bytes from the simulation to the serial port.
pub struct SerialPort {
    /// Data from serial port -- output port.
    pub received_data: Output<Bytes>,

    /// Data receiver to the simulation.
    receiver: Receiver<Bytes>,

    /// Data transmitter from the simulation.
    transmitter: Sender<Bytes>,

    /// Model instance config.
    config: SerialPortConfig,

    /// I/O thread waker.
    waker: Arc<Waker>,

    /// The simulation halt flag.
    is_halted: Arc<AtomicBool>,

    /// I/O thread guard.
    io_thread: Option<ThreadJoiner<()>>,
}

impl SerialPort {
    /// Creates a new serial port model.
    fn new(
        receiver: Receiver<Bytes>,
        transmitter: Sender<Bytes>,
        received_data: Output<Bytes>,
        config: SerialPortConfig,
        waker: Arc<Waker>,
        is_halted: Arc<AtomicBool>,
        io_thread: JoinHandle<()>,
    ) -> Self {
        Self {
            received_data,
            receiver,
            transmitter,
            config,
            waker,
            is_halted,
            io_thread: Some(ThreadJoiner::new(io_thread)),
        }
    }

    /// Processes the data received on the serial port.
    async fn process(&mut self) {
        while let Ok(data) = self.receiver.try_recv() {
            #[cfg(feature = "tracing")]
            info!(
                "Received data on the serial port {}: {:X}.",
                self.config.port_path, data
            );
            self.received_data.send(data).await;
        }
    }

    /// Sends raw bytes to the serial port -- input port.
    pub async fn send_bytes(&mut self, data: Bytes) {
        #[cfg(feature = "tracing")]
        info!(
            "Will send data to the serial port {}: {:X}.",
            self.config.port_path, data
        );
        self.transmitter.send(data).unwrap();
        self.waker.wake().unwrap();
    }
}

impl Model for SerialPort {
    async fn init(self, context: &mut Context<Self>) -> InitializedModel<Self> {
        if let Some(period) = self.config.period {
            let delta = match self.config.delta {
                Some(delta) => delta,
                None => period,
            };
            context
                .schedule_periodic_event(
                    Duration::from_millis(delta),
                    Duration::from_millis(period),
                    Self::process,
                    (),
                )
                .unwrap();
        }

        self.into()
    }
}

impl Drop for SerialPort {
    fn drop(&mut self) {
        self.is_halted.store(true, Ordering::Relaxed);
        let _ = self.waker.wake();
        let _ = self.io_thread.take().unwrap().join();
    }
}

impl fmt::Debug for SerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SerialPort").finish_non_exhaustive()
    }
}

/// Serial port model prototype.
pub struct ProtoSerialPort {
    /// Data from serial port -- output port.
    pub received_data: Output<Bytes>,

    /// Serial port model instance config.
    config: SerialPortConfig,
}

impl ProtoSerialPort {
    /// Creates a new serial port model prototype.
    pub fn new(config: SerialPortConfig) -> Self {
        Self {
            config,
            received_data: Output::new(),
        }
    }

    /// Starts the I/O thread.
    fn start_io(
        port_path: &str,
        baud_rate: u32,
        tx: Sender<Bytes>,
        rx: Receiver<Bytes>,
        buffer_size: usize,
        is_halted: Arc<AtomicBool>,
    ) -> (JoinHandle<()>, Arc<Waker>) {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(256);
        let mut port = mio_serial::new(port_path, baud_rate)
            .open_native_async()
            .unwrap();

        let serial = Token(0);
        let wake = Token(1);

        let waker = Arc::new(Waker::new(poll.registry(), wake).unwrap());
        poll.registry()
            .register(&mut port, serial, Interest::READABLE)
            .unwrap();

        // I/O thread.
        let io_thread = thread::spawn(move || {
            'poll: loop {
                // This call is blocking.
                poll.poll(&mut events, None).unwrap();

                for event in events.iter() {
                    if event.token() == serial {
                        loop {
                            // Until read_buf (RFC 2930) is stabilized we need an initialized
                            // buffer.
                            let mut message = BytesMut::zeroed(buffer_size);
                            match port.read(&mut message) {
                                Ok(len) => {
                                    message.truncate(len);
                                    if tx.send(message.into()).is_err() {
                                        break 'poll;
                                    }
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    break;
                                }
                                _ => break 'poll,
                            }
                        }
                    } else if event.token() == wake {
                        if is_halted.load(Ordering::Relaxed) {
                            break 'poll;
                        }
                        while let Ok(data) = rx.try_recv() {
                            if port.write(&data).is_err() {
                                break 'poll;
                            }
                        }
                    } else {
                        // Unknown event: should never happen.
                        break 'poll;
                    }
                }
            }
        });

        (io_thread, waker)
    }
}

impl ProtoModel for ProtoSerialPort {
    type Model = SerialPort;

    fn build(self, _: &mut nexosim::model::BuildContext<Self>) -> Self::Model {
        let (rtx, rrx) = channel();
        let (ttx, trx) = channel();

        let is_halted = Arc::new(AtomicBool::new(false));

        let (io_thread, waker) = Self::start_io(
            &self.config.port_path,
            self.config.baud_rate,
            rtx,
            trx,
            self.config.buffer_size,
            is_halted.clone(),
        );

        Self::Model::new(
            rrx,
            ttx,
            self.received_data,
            self.config,
            waker,
            is_halted,
            io_thread,
        )
    }
}

impl fmt::Debug for ProtoSerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoSerialPort").finish_non_exhaustive()
    }
}
