//! An example demonstrating a simple UPD I/O interfacing model using a
//! background thread for communication with the UDP socket.
//!
//! The example uses a loop-back with a 1s latency to demonstrate both writing
//! and reading operations.
#![allow(deprecated)]

use std::error::Error;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::thread::{self, sleep};
use std::time::Duration;

use bytes::Bytes;
use nexosim::model::Context;
use serde::{Deserialize, Serialize};
use thread_guard::ThreadGuard;

use nexosim::model::{Model, ProtoModel, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_slot};
use nexosim::simulation::{Mailbox, ModelInjector, SimInit};
use nexosim::time::{AutoSystemClock, MonotonicTime, PeriodicTicker};

use nexosim_io_utils::port::{IoThread, SendError};
use nexosim_io_utils::udp::{Data, Udp};

/// Simulation tick period.
const TICK_PERIOD: Duration = Duration::from_millis(100);

/// Client address.
const IO_THREAD_ADDR: &str = "127.0.0.1:34254";

/// Echo server address.
const ECHO_THREAD_ADDR: &str = "127.0.0.1:34255";

/// Buffer size.
const BUF_SIZE: usize = 65536;

/// Environment of the UDP model.
///
/// The environment manages the UDP socket and receives UDP packets
/// asynchronously.
struct UdpModelEnv {
    io_thread: IoThread<Data>,
    target_addr: SocketAddr,
}

impl UdpModelEnv {
    fn new(
        self_addr: SocketAddr,
        target_addr: SocketAddr,
        injector: ModelInjector<UdpModel>,
    ) -> Self {
        let udp = Udp::new(self_addr, BUF_SIZE);

        // The environment is a thread handling I/O operations in the background.
        let io_thread = IoThread::new(
            udp,
            injector,
            *schedulable!(UdpModel::recv),
            *schedulable!(UdpModel::disconnected),
        );

        Self {
            io_thread,
            target_addr,
        }
    }

    fn send(&mut self, bytes: Bytes) -> Result<(), SendError> {
        let data = Data {
            addr: self.target_addr,
            bytes,
        };
        self.io_thread.send(data)
    }
}

struct ProtoUdpModel {
    pub bytes: Output<Bytes>,

    self_addr: SocketAddr,
    target_addr: SocketAddr,
}

impl ProtoUdpModel {
    fn new(self_addr: SocketAddr, target_addr: SocketAddr) -> Self {
        Self {
            bytes: Output::default(),
            self_addr,
            target_addr,
        }
    }
}

impl ProtoModel for ProtoUdpModel {
    type Model = UdpModel;

    fn build(
        self,
        cx: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as Model>::Env) {
        let env = UdpModelEnv::new(self.self_addr, self.target_addr, cx.injector());

        (UdpModel { bytes: self.bytes }, env)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct UdpModel {
    bytes: Output<Bytes>,
}

#[Model(type Env=UdpModelEnv)]
impl UdpModel {
    /// Public input.
    #[nexosim(schedulable)]
    pub fn send(&mut self, bytes: Bytes, _: &Context<Self>, env: &mut UdpModelEnv) {
        println!("bytes to be sent: {:?}", bytes);
        env.send(bytes)
            .expect("Encountered UDP error when sending bytes");
    }

    /// Private method, used by the environment only.
    #[nexosim(schedulable)]
    async fn recv(&mut self, Data { bytes, .. }: Data) {
        println!("received bytes: {:?}", bytes);
        // Forward to output port.
        self.bytes.send(bytes).await;
    }

    /// Private method, called when disconnected, we do nothing.
    #[nexosim(schedulable)]
    async fn disconnected(&mut self, _: ()) {}
}

/// Uses I/O thread to send data to echo UDP server.
fn main() -> Result<(), Box<dyn Error>> {
    // UDP model and mailbox.
    let mut udp_model = ProtoUdpModel::new(IO_THREAD_ADDR.parse()?, ECHO_THREAD_ADDR.parse()?);
    let udp_model_mbox = Mailbox::new();

    // Bench.
    let mut bench = SimInit::new();

    // Read handle.
    let (sink, mut bytes_out) = event_slot(SinkState::Enabled);
    udp_model.bytes.connect_sink(sink);

    // Write handle.
    let bytes_in = EventSource::new()
        .connect(UdpModel::send, &udp_model_mbox)
        .register(&mut bench);

    // Assembly and initialization.
    let mut simu = bench
        .add_model(udp_model, udp_model_mbox, "UDP model")
        .with_clock(AutoSystemClock::new(), PeriodicTicker::new(TICK_PERIOD))
        .init(MonotonicTime::EPOCH)?;
    let scheduler = simu.scheduler();

    // Set up an echo UDP server with an artificial 1s latency.
    let _echo_thread = ThreadGuard::new(thread::spawn(move || {
        let socket = UdpSocket::bind(ECHO_THREAD_ADDR).unwrap();
        let mut buf = [0; BUF_SIZE];
        let (len, addr) = socket.recv_from(&mut buf).unwrap();
        sleep(Duration::from_secs(1));
        socket.send_to(&buf[..len], addr).unwrap();
    }));

    // Schedule the data for sending in 1s.
    let msg: &[u8] = &[1, 2, 3];
    scheduler.schedule_event(Duration::from_secs(1), &bytes_in, msg.into())?;

    // Run the simulator for 3s.
    simu.step_until(Duration::from_secs(3))?;

    // Make sure the bytes were received back.
    assert_eq!(bytes_out.try_read().as_ref().map(|b| b.as_ref()), Some(msg));

    Ok(())
}
