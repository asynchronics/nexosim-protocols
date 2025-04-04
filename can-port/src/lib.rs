//! CAN port model for [NeXosim][NX]-based simulations.
//!
//! This model
//! * listens the specified CAN ports injecting data from it into the
//!   simulation,
//! * outputs data from the simulation to the specified CAN ports.
//!
//! Note: data sent by the CAN port is injected back into the simulation.
//!
//! [NX]: https://github.com/asynchronics/nexosim
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

use std::fmt;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::{io::AsRawFd, prelude::RawFd};
use std::time::Duration;

use mio::event::Source;
use mio::{Interest, Registry, Token, unix::SourceFd};

use schematic::Config;

use socketcan::{BlockingCan, CanFrame, CanSocket, Error as CanError, Socket};

#[cfg(feature = "tracing")]
use tracing::info;

use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::Output;

use nexosim_io_utils::port::{IoPort, IoThread};

/// A Socket wrapped for MIO eventing.
// Taken with changes from socketcan-rs.
#[derive(Debug)]
struct MioSocket<T: Socket>(T);

impl<T: Socket> MioSocket<T> {
    /// Creates new socket.
    fn new(socket: T) -> Self {
        Self(socket)
    }

    /// Gets a reference.
    fn get_ref(&self) -> &T {
        &self.0
    }

    /// Gets a mutable reference.
    fn get_mut_ref(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Socket> AsRawFd for MioSocket<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl<T: Socket> Source for MioSocket<T> {
    fn register(&mut self, registry: &Registry, token: Token, interests: Interest) -> Result<()> {
        SourceFd(&self.0.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(&mut self, registry: &Registry, token: Token, interests: Interest) -> Result<()> {
        SourceFd(&self.0.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> Result<()> {
        SourceFd(&self.0.as_raw_fd()).deregister(registry)
    }
}

/// CAN port model instance config.
#[derive(Config, Debug)]
pub struct CanPortConfig {
    /// List of CAN interfaces.
    #[setting(default = vec!["vcan0".into(), "vcan1".into()])]
    pub interfaces: Vec<String>,

    /// Time shift for scheduling events at the present moment.
    ///
    /// If no value is provided, `period` is used.
    pub delta: Option<u64>,

    /// Activation period for cyclic activities inside the simulation.
    ///
    /// If no value is provided, cyclic activities are not scheduled
    /// automatically.
    pub period: Option<u64>,
}

/// CAN data exchanged inside the simulation.
#[derive(Clone, Copy, Debug)]
pub struct CanData {
    /// CAN interface.
    pub interface: usize,

    /// CAN frame.
    pub frame: CanFrame,
}

struct CanPortInner {
    sockets: Vec<MioSocket<CanSocket>>,
}

impl CanPortInner {
    fn new(interfaces: &[String]) -> Self {
        let mut sockets = Vec::with_capacity(interfaces.len());

        for interface in interfaces.iter() {
            let socket = MioSocket::new(CanSocket::open(interface).unwrap());
            socket.get_ref().set_nonblocking(true).unwrap();
            sockets.push(socket);
        }

        Self { sockets }
    }
}

impl IoPort<MioSocket<CanSocket>, CanData, CanData> for CanPortInner {
    fn register(&mut self, registry: &Registry) -> Token {
        for (i, socket) in self.sockets.iter_mut().enumerate() {
            registry
                .register(socket, Token(i), Interest::READABLE)
                .unwrap();
        }
        Token(self.sockets.len())
    }

    fn read(&mut self, token: Token) -> Result<CanData> {
        let Token(i) = token;
        self.sockets.get(i).map_or(
            Err(Error::new(ErrorKind::InvalidInput, "Unknown event.")),
            |socket| {
                socket.get_ref().read_frame().map(|frame| CanData {
                    interface: i,
                    frame,
                })
            },
        )
    }

    fn write(&mut self, data: &CanData) -> Result<()> {
        self.sockets.get_mut(data.interface).map_or(
            Err(Error::new(ErrorKind::InvalidInput, "Unknown interface.")),
            |socket| {
                socket
                    .get_mut_ref()
                    .transmit(&data.frame)
                    .map_err(|err| match err {
                        CanError::Io(err) => err,
                        CanError::Can(err) => Error::new(ErrorKind::Other, err),
                    })
            },
        )
    }
}

/// CAN port model.
///
/// This model
/// * listens the specified CAN ports and injects into the simulation values
///   read from it as CAN frames,
/// * outputs CAN frames from the simulation to the CAN port.
pub struct CanPort {
    /// CAN frame -- output port.
    pub frame_out: Output<CanData>,

    /// Model instance configuration.
    config: CanPortConfig,

    /// I/O thread.
    io_thread: IoThread<CanData, CanData>,
}

impl CanPort {
    /// Creates a new CAN port model.
    fn new(
        frame_out: Output<CanData>,
        config: CanPortConfig,
        io_thread: IoThread<CanData, CanData>,
    ) -> Self {
        Self {
            frame_out,
            config,
            io_thread,
        }
    }

    /// Transmits CAN frame -- input port.
    pub fn frame_in(&mut self, data: CanData) {
        #[cfg(feature = "tracing")]
        info!(
            "Will transmit CAN frame to the CAN interface {}: {:?}.",
            self.config.interfaces[data.interface], data.frame
        );
        self.io_thread.send(data).unwrap();
    }

    /// Forwards the CAN frame received on the serial port.
    pub async fn process(&mut self) {
        while let Ok(data) = self.io_thread.try_recv() {
            #[cfg(feature = "tracing")]
            info!(
                "Received CAN frame on the CAN interface {}: {:?}.",
                self.config.interfaces[data.interface], data.frame
            );
            self.frame_out.send(data).await;
        }
    }
}

impl Model for CanPort {
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

impl fmt::Debug for CanPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CanPort").finish_non_exhaustive()
    }
}

/// CAN port model prototype.
#[allow(missing_debug_implementations)]
pub struct ProtoCanPort {
    /// Received CAN frames -- output port.
    pub frame_out: Output<CanData>,

    /// CAN port model instance configuration.
    config: CanPortConfig,
}

impl ProtoCanPort {
    /// Creates a new CAN port model prototype.
    pub fn new(config: CanPortConfig) -> Self {
        Self {
            frame_out: Output::default(),
            config,
        }
    }
}

impl ProtoModel for ProtoCanPort {
    type Model = CanPort;

    fn build(self, _: &mut BuildContext<Self>) -> Self::Model {
        let interfaces = CanPortInner::new(&self.config.interfaces);

        Self::Model::new(self.frame_out, self.config, IoThread::new(interfaces))
    }
}

impl fmt::Debug for ProtoCanPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoCanPort").finish_non_exhaustive()
    }
}
