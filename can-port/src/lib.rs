#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]
#![allow(deprecated)]

use std::fmt;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::{io::AsRawFd, prelude::RawFd};

use mio::event::Source;
use mio::{Interest, Registry, Token, unix::SourceFd};

use schematic::Config;

use serde::{Deserialize, Serialize};

use socketcan::{
    BlockingCan, CanFrame, CanSocket, EmbeddedFrame, Error as CanError, Frame, Socket,
};

#[cfg(feature = "tracing")]
use tracing::{debug, info};

use nexosim::model::{self, BuildContext, Context, ProtoModel};
use nexosim::model::{Model, schedulable};
use nexosim::ports::Output;
use nexosim::simulation::ModelInjector;

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
}

/// CAN data exchanged inside the simulation.
#[derive(Clone, Copy, Debug)]
pub struct CanData {
    /// CAN interface.
    pub interface: usize,

    /// CAN frame.
    pub frame: CanFrame,
}

/// Inner implementation of I/O port.
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

impl IoPort<MioSocket<CanSocket>, SerializableCanData, CanData> for CanPortInner {
    fn register(&mut self, registry: &Registry) -> Token {
        for (i, socket) in self.sockets.iter_mut().enumerate() {
            registry
                .register(socket, Token(i), Interest::READABLE)
                .unwrap();
        }
        Token(self.sockets.len())
    }

    fn read(&mut self, token: Token) -> Result<SerializableCanData> {
        let Token(i) = token;
        self.sockets.get(i).map_or(
            Err(Error::new(ErrorKind::InvalidInput, "Unknown event.")),
            |socket| {
                socket.get_ref().read_frame().map(|frame| {
                    CanData {
                        interface: i,
                        frame,
                    }
                    .into()
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
                        CanError::Can(err) => Error::other(err),
                    })
            },
        )
    }
}

/// CAN port model environment.
pub struct CanPortEnv {
    /// Model instance configuration.
    config: CanPortConfig,

    /// Model injector.
    injector: ModelInjector<CanPort>,

    /// I/O thread.
    io_thread: IoThread<CanData>,
}

impl fmt::Debug for CanPortEnv {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CanPortEnv").finish_non_exhaustive()
    }
}

/// CAN port model prototype.
#[allow(missing_debug_implementations)]
pub struct ProtoCanPort {
    /// Received CAN frames -- output port.
    pub frame_out: Output<CanData>,

    /// Disconnection event -- output port.
    pub disconnected: Output<()>,

    /// CAN port model instance configuration.
    config: CanPortConfig,
}

impl ProtoCanPort {
    /// Creates a new CAN port model prototype.
    pub fn new(config: CanPortConfig) -> Self {
        Self {
            frame_out: Output::default(),
            disconnected: Output::default(),
            config,
        }
    }
}

impl ProtoModel for ProtoCanPort {
    type Model = CanPort;

    fn build(
        self,
        cx: &mut BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as model::Model>::Env) {
        let interfaces = CanPortInner::new(&self.config.interfaces);

        (
            Self::Model {
                frame_out: self.frame_out,
                disconnected: self.disconnected,
            },
            CanPortEnv {
                config: self.config,
                injector: cx.injector(),
                io_thread: IoThread::new(
                    interfaces,
                    cx.injector(),
                    *schedulable!(CanPort::frame_out),
                    *schedulable!(CanPort::disconnected),
                ),
            },
        )
    }
}

impl fmt::Debug for ProtoCanPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoCanPort").finish_non_exhaustive()
    }
}

/// CAN port model.
///
/// This model
/// * listens the specified CAN ports and injects into the simulation values
///   read from it as CAN frames,
/// * outputs CAN frames from the simulation to the CAN port.
#[derive(Serialize, Deserialize)]
pub struct CanPort {
    /// CAN frame -- output port.
    frame_out: Output<CanData>,

    /// Disconnection event -- output port.
    disconnected: Output<()>,
}

#[Model(type Env = CanPortEnv)]
impl CanPort {
    /// Transmits CAN frame -- input port.
    pub fn frame_in(&mut self, data: CanData, _: &Context<Self>, env: &mut CanPortEnv) {
        #[cfg(feature = "tracing")]
        debug!(
            "Sending CAN frame to CAN interface {}: {:?}.",
            env.config.interfaces[data.interface], data.frame
        );
        env.io_thread.send(data).unwrap();
    }

    /// Reconnects CAN interface.
    pub async fn reconnect(&mut self, _: (), _: &Context<Self>, env: &mut CanPortEnv) {
        let interfaces = CanPortInner::new(&env.config.interfaces);
        env.io_thread = IoThread::new(
            interfaces,
            env.injector.clone(),
            *schedulable!(CanPort::frame_out),
            *schedulable!(CanPort::disconnected),
        );
    }

    /// Private port forwarding received CAN frames.
    #[nexosim(schedulable)]
    async fn frame_out(
        &mut self,
        data: SerializableCanData,
        _: &Context<Self>,
        #[cfg(feature = "tracing")] env: &mut CanPortEnv,
    ) {
        let data: CanData = data.into();

        #[cfg(feature = "tracing")]
        debug!(
            "Receiving CAN frame on CAN interface {}: {:?}.",
            env.config.interfaces[data.interface], data.frame
        );
        self.frame_out.send(data).await;
    }

    /// Private port forwarding disconnection event.
    #[nexosim(schedulable)]
    async fn disconnected(&mut self, _: (), _: &Context<Self>, _: &mut CanPortEnv) {
        #[cfg(feature = "tracing")]
        info!("CAN is disconnected.");
        self.disconnected.send(()).await;
    }
}

impl fmt::Debug for CanPort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CanPort").finish_non_exhaustive()
    }
}

/// A workaround for the lack of `Serialize` and `Deserialize` implementations
/// on `CanFrame`.
///
/// This may become unnecessary if/when NeXosim allows non-serializable event
/// injection.
#[derive(Copy, Clone, Default, Serialize, Deserialize)]
struct SerializableCanData {
    interface: usize,
    can_id: u32,
    can_len: usize,
    can_data: [u8; 8],
}

impl From<CanData> for SerializableCanData {
    fn from(data: CanData) -> Self {
        let frame = data.frame;
        let can_len = frame.len();
        let mut can_data = [0u8; 8];
        can_data[0..can_len].copy_from_slice(frame.data());

        Self {
            interface: data.interface,
            can_id: frame.raw_id(),
            can_len,
            can_data,
        }
    }
}

impl From<SerializableCanData> for CanData {
    fn from(data: SerializableCanData) -> Self {
        let frame = CanFrame::from_raw_id(data.can_id, &data.can_data[0..data.can_len]).unwrap();

        CanData {
            interface: data.interface,
            frame,
        }
    }
}
