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
use std::io::{ErrorKind, Read, Result as IoResult, Write};
use std::time::Duration;

use bytes::{Bytes, BytesMut};

use schematic::Config;

use mio::{Interest, Registry, Token};
use mio_serial::{SerialPortBuilderExt, SerialStream};

#[cfg(feature = "tracing")]
use tracing::info;

use nexosim::model::{Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::Output;

use nexosim_io_utils::port::{IoPort, IoThread};

/// Serial port model instance configuration.
#[derive(Config, Debug)]
pub struct SerialPortConfig {
    /// Baud rate.
    ///
    /// Zero value shall be used for software TTY interfaces.
    #[setting(default = 0)]
    pub baud_rate: u32,

    /// Serial port path.
    pub port_path: String,

    /// Internal buffer size.
    ///
    /// Input is read and forwarded to the simulation by blocks up to buffer
    /// size.
    #[setting(default = 256)]
    pub buffer_size: usize,

    /// Delay for the first scheduled data forwarding, in milliseconds.
    ///
    /// If no value is provided, `period` is used.
    pub delta: Option<u64>,

    /// Period at which data from the serial port is forwarded into the
    /// simulation, in milliseconds.
    ///
    /// If no value is provided, periodic activities are not scheduled
    /// automatically.
    pub period: Option<u64>,
}

struct SerialPortInner {
    port: SerialStream,
    buffer: Vec<u8>,
}

impl SerialPortInner {
    fn new(port_path: &str, baud_rate: u32, buffer_size: usize) -> Self {
        // Until read_buf (RFC 2930) is stabilized we need an initialized
        // buffer.
        Self {
            port: mio_serial::new(port_path, baud_rate)
                .open_native_async()
                .unwrap(),
            buffer: vec![0; buffer_size],
        }
    }
}

impl IoPort<SerialStream, Bytes, Bytes> for SerialPortInner {
    fn register(&mut self, registry: &Registry) -> Token {
        registry
            .register(&mut self.port, Token(0), Interest::READABLE)
            .unwrap();
        Token(1)
    }

    fn read(&mut self, token: Token) -> IoResult<Bytes> {
        if token == Token(0) {
            self.port
                .read(&mut self.buffer)
                .map(|len| BytesMut::from(&self.buffer[..len]).into())
        } else {
            // Unknown event: should never happen.
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "Unknown event.",
            ))
        }
    }

    fn write(&mut self, data: &Bytes) -> IoResult<()> {
        self.port.write(data).map(|len| {
            if len != data.len() {
                Err(std::io::Error::new(
                    ErrorKind::Other,
                    format!(
                        "Not all bytes written: had to write {}, but wrote {}.",
                        data.len(),
                        len
                    ),
                ))
            } else {
                Ok(())
            }
        })?
    }
}

/// Serial port model.
///
/// This model:
/// * listens to the configured serial port and forwards its data to the model
///   output,
/// * forwards data from the model input to the serial port.
pub struct SerialPort {
    /// Data from serial port -- output port.
    pub bytes_out: Output<Bytes>,

    /// Model instance configuration.
    config: SerialPortConfig,

    io_thread: IoThread<Bytes, Bytes>,
}

impl SerialPort {
    /// Creates a new serial port model.
    fn new(
        bytes_out: Output<Bytes>,
        config: SerialPortConfig,
        io_thread: IoThread<Bytes, Bytes>,
    ) -> Self {
        Self {
            bytes_out,
            config,
            io_thread,
        }
    }

    /// Sends raw bytes to the serial port -- input port.
    pub async fn bytes_in(&mut self, data: Bytes) {
        #[cfg(feature = "tracing")]
        info!(
            "Will send data to the serial port {}: {:X}.",
            self.config.port_path, data
        );
        self.io_thread.send(data).unwrap();
    }

    /// Forwards the raw bytes received on the serial port.
    pub async fn process(&mut self) {
        while let Ok(data) = self.io_thread.try_recv() {
            #[cfg(feature = "tracing")]
            info!(
                "Received data on the serial port {}: {:X}.",
                self.config.port_path, data
            );
            self.bytes_out.send(data).await;
        }
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

impl fmt::Debug for SerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SerialPort").finish_non_exhaustive()
    }
}

/// Serial port model prototype.
pub struct ProtoSerialPort {
    /// Data from serial port -- output port.
    pub bytes_out: Output<Bytes>,

    /// Serial port model instance config.
    config: SerialPortConfig,
}

impl ProtoSerialPort {
    /// Creates a new serial port model prototype.
    pub fn new(config: SerialPortConfig) -> Self {
        Self {
            config,
            bytes_out: Output::new(),
        }
    }
}

impl ProtoModel for ProtoSerialPort {
    type Model = SerialPort;

    fn build(self, _: &mut nexosim::model::BuildContext<Self>) -> Self::Model {
        let port = SerialPortInner::new(
            &self.config.port_path,
            self.config.baud_rate,
            self.config.buffer_size,
        );

        Self::Model::new(self.bytes_out, self.config, IoThread::new(port))
    }
}

impl fmt::Debug for ProtoSerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoSerialPort").finish_non_exhaustive()
    }
}
