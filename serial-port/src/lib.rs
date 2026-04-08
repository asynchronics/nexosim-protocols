#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]
#![allow(deprecated)]

use std::fmt;
use std::io::{ErrorKind, Read, Result as IoResult, Write};

use bytes::{Bytes, BytesMut};

use schematic::Config;

use mio::{Interest, Registry, Token};
use mio_serial::{SerialPortBuilderExt, SerialStream};

use serde::{Deserialize, Serialize};

#[cfg(feature = "tracing")]
use tracing::info;

use nexosim::model::{self, Context, Model, ProtoModel, schedulable};
use nexosim::ports::Output;
use nexosim::simulation::ModelInjector;

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
}

/// Inner implementation of I/O port.
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
            self.port.read(&mut self.buffer).map(|len| {
                if len == 0 {
                    // Serial port disappeared.
                    return Err(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "End of file reached for Serial/TTY device.",
                    ));
                }
                Ok(BytesMut::from(&self.buffer[..len]).into())
            })?
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
                Err(std::io::Error::other(format!(
                    "Not all bytes written: had to write {}, but wrote {}.",
                    data.len(),
                    len
                )))
            } else {
                Ok(())
            }
        })?
    }
}

/// Serial port model environment.
pub struct SerialPortEnv {
    /// Model instance configuration.
    config: SerialPortConfig,

    /// Model injector.
    injector: ModelInjector<SerialPort>,

    /// I/O thread.
    io_thread: Option<IoThread<Bytes>>,
}

impl fmt::Debug for SerialPortEnv {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SerialPortEnv").finish_non_exhaustive()
    }
}

/// Serial port model prototype.
pub struct ProtoSerialPort {
    /// Data from serial port -- output port.
    pub bytes_out: Output<Bytes>,

    /// Disconnection event -- output port.
    pub disconnected: Output<()>,

    /// Serial port model instance config.
    config: SerialPortConfig,
}

impl ProtoSerialPort {
    /// Creates a new serial port model prototype.
    pub fn new(config: SerialPortConfig) -> Self {
        Self {
            config,
            bytes_out: Output::new(),
            disconnected: Output::new(),
        }
    }
}

impl ProtoModel for ProtoSerialPort {
    type Model = SerialPort;

    fn build(
        self,
        cx: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as model::Model>::Env) {
        let port = SerialPortInner::new(
            &self.config.port_path,
            self.config.baud_rate,
            self.config.buffer_size,
        );

        (
            Self::Model {
                bytes_out: self.bytes_out,
                disconnected: self.disconnected,
            },
            SerialPortEnv {
                config: self.config,
                injector: cx.injector(),
                io_thread: Some(IoThread::new(
                    port,
                    cx.injector(),
                    *schedulable!(SerialPort::bytes_out),
                    *schedulable!(SerialPort::disconnected),
                )),
            },
        )
    }
}

impl fmt::Debug for ProtoSerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoSerialPort").finish_non_exhaustive()
    }
}

/// Serial port model.
///
/// This model:
///
/// * listens to the configured serial port and forwards its data to the model
///   output,
/// * forwards data from the model input to the serial port.
#[derive(Serialize, Deserialize)]
pub struct SerialPort {
    /// Data from serial port -- output port.
    bytes_out: Output<Bytes>,

    /// Disconnection event -- output port.
    disconnected: Output<()>,
}

#[Model(type Env = SerialPortEnv)]
impl SerialPort {
    /// Sends raw bytes to the serial port -- input port.
    pub async fn bytes_in(&mut self, data: Bytes, _: &Context<Self>, env: &mut SerialPortEnv) {
        #[cfg(feature = "tracing")]
        info!(
            "Sending data to the serial port {}: {:X}.",
            env.config.port_path, data
        );
        if let Some(io_thread) = &mut env.io_thread {
            io_thread.send(data).unwrap();
        }
    }

    /// Reconnects serial interface.
    pub async fn reconnect(&mut self, _: (), _: &Context<Self>, env: &mut SerialPortEnv) {
        std::mem::drop(env.io_thread.take());
        let port = SerialPortInner::new(
            &env.config.port_path,
            env.config.baud_rate,
            env.config.buffer_size,
        );
        env.io_thread = Some(IoThread::new(
            port,
            env.injector.clone(),
            *schedulable!(SerialPort::bytes_out),
            *schedulable!(SerialPort::disconnected),
        ));
    }

    /// Private port forwarding the raw bytes received on the serial port.
    #[nexosim(schedulable)]
    async fn bytes_out(
        &mut self,
        data: Bytes,
        _: &Context<Self>,
        #[cfg(feature = "tracing")] env: &mut SerialPortEnv,
    ) {
        #[cfg(feature = "tracing")]
        info!(
            "Receiving data from the serial port {}: {:X}.",
            env.config.port_path, data
        );
        self.bytes_out.send(data).await;
    }

    /// Private port forwarding information on disconnection.
    #[nexosim(schedulable)]
    async fn disconnected(
        &mut self,
        _: (),
        _: &Context<Self>,
        #[cfg(feature = "tracing")] env: &mut SerialPortEnv,
    ) {
        #[cfg(feature = "tracing")]
        info!("Serial port {} disconnected.", env.config.port_path);
        self.disconnected.send(()).await;
    }
}

impl fmt::Debug for SerialPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SerialPort").finish_non_exhaustive()
    }
}
