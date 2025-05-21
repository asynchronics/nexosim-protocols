//! Example: a simulation that receives data from a CAN port.
//!
//! Before running an example, execute `can-setup.sh`.
//!
//! This example demonstrates in particular:
//!
//! * CAN port model,
//! * infinite simulation,
//! * blocking event queue,
//! * simulation halting,
//! * system clock,
//! * observable state.
//!
//! ```text
//!                           ┏━━━━━━━━━━━━━━━━━━━━━┓
//!                           ┃ Simulation          ┃
//! ┌╌╌╌╌╌╌╌╌╌╌╌╌┐            ┃   ┌──────────┐mode  ┃
//! ┆ External   ┆   pulses   ┃   │          ├──────╂┐ EventQueue
//! ┆ thread     ├╌╌╌╌╌╌╌╌╌╌╌╌╂╌╌►│ Counter  │count ┃├───────────────────►
//! ┆            ┆ [CAN port] ┃   │          ├──────╂┘
//! └╌╌╌╌╌╌╌╌╌╌╌╌┘            ┃   └──────────┘      ┃
//!                           ┗━━━━━━━━━━━━━━━━━━━━━┛
//! ```
use std::sync::mpsc::channel;
use std::thread::{self, sleep};
use std::time::Duration;

use schematic::{ConfigLoader, Format};

use socketcan::{BlockingCan, CanFrame, CanSocket, EmbeddedFrame, Id, Socket, StandardId};

use thread_guard::ThreadGuard;

use nexosim::model::{Context, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};
use nexosim_util::observable::Observable;

use nexosim_can_port::{CanData, CanPort, CanPortConfig, ProtoCanPort};

/// For CAN ports setup see `can-setup.sh`.
///
/// CAN interfaces.
const CAN_INTERFACES: &[&str] = &["vcan0", "vcan1"];

/// Reader timeout.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Pulse data ID.
const PULSE_ID: u16 = 0x100;

/// Detection data ID.
const STAT_ID: u16 = 0x200;

/// Activation period, in milliseconds, for cyclic activities inside the simulation.
const PERIOD: u64 = 10;
/// Time shift, in milliseconds, for scheduling events at the present moment.
const DELTA: u64 = 5;

/// Counter switch on delay.
const SWITCH_ON_DELAY: Duration = Duration::from_secs(1);

/// Number of detections.
const N: u64 = 10;

/// Counter mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Off,
    On,
}

/// Simulation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Mode(Mode),
    Count(u64),
}

/// The `Counter` Model.
pub struct Counter {
    /// Operation mode.
    pub mode: Output<Mode>,

    /// Pulses count.
    pub count: Output<u64>,

    /// Internal state.
    state: Observable<Mode>,

    /// Counter.
    acc: Observable<u64>,
}

impl Counter {
    /// Creates a new `Counter` model.
    fn new() -> Self {
        let mode = Output::default();
        let count = Output::default();
        Self {
            mode: mode.clone(),
            count: count.clone(),
            state: Observable::new(mode),
            acc: Observable::new(count),
        }
    }

    /// Power -- input port.
    pub async fn power_in(&mut self, on: bool, cx: &mut Context<Self>) {
        match *self.state {
            Mode::Off if on => cx
                .schedule_event(SWITCH_ON_DELAY, Self::switch_on, ())
                .unwrap(),
            Mode::On if !on => self.switch_off().await,
            _ => (),
        };
    }

    /// Pulse -- input port.
    pub async fn pulse(&mut self) {
        self.acc.modify(|x| *x += 1).await;
    }

    /// Switches `Counter` on.
    async fn switch_on(&mut self) {
        self.state.set(Mode::On).await;
    }

    /// Switches `Counter` off.
    async fn switch_off(&mut self) {
        self.state.set(Mode::Off).await;
    }
}

impl Model for Counter {}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    // The serial port model.
    let mut can = ProtoCanPort::new(get_can_port_cfg(CAN_INTERFACES));

    // The counter model.
    let mut counter = Counter::new();

    // Mailboxes.
    let can_mbox = Mailbox::new();
    let counter_mbox = Mailbox::new();

    // Connections.
    can.frame_out.filter_map_connect(
        |data| match data.frame.id() {
            Id::Standard(id) if id.as_raw() == PULSE_ID => Some(()),
            _ => None,
        },
        Counter::pulse,
        &counter_mbox,
    );
    counter.count.map_connect(
        |c| CanData {
            interface: 0,
            frame: CanFrame::new(
                Id::Standard(StandardId::new(STAT_ID).unwrap()),
                &c.to_le_bytes(),
            )
            .unwrap(),
        },
        CanPort::frame_in,
        &can_mbox,
    );

    // Model handles for simulation.
    let counter_addr = counter_mbox.address();
    let observer = EventQueue::new();
    counter
        .mode
        .map_connect_sink(|m| Event::Mode(*m), &observer);
    counter
        .count
        .map_connect_sink(|c| Event::Count(*c), &observer);
    let mut observer = observer.into_reader_with_timeout(TIMEOUT);

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let (mut simu, scheduler) = SimInit::new()
        .add_model(can, can_mbox, "can")
        .add_model(counter, counter_mbox, "counter")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    let mut sim_scheduler = scheduler.clone();

    // Simulation thread.
    let simulation_handle = ThreadGuard::with_actions(
        thread::spawn(move || {
            // ---------- Simulation.  ----------
            // Infinitely kept alive by the ticker model until halted.
            simu.step_unbounded()
        }),
        move |_| {
            sim_scheduler.halt();
        },
        |_, res| {
            println!("Simulation thread result: {:?}.", res);
        },
    );

    // Switch the counter on.
    scheduler.schedule_event(
        Duration::from_millis(1),
        Counter::power_in,
        true,
        counter_addr,
    )?;

    // Wait until counter mode is `On`.
    loop {
        let event = observer.next();
        match event {
            Some(Event::Mode(Mode::On)) => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Synchronization channels.
    let (tx_0, rx_0) = channel();
    let (tx_1, rx_1) = channel();

    // Threads sending data to the CAN ports.
    let sender_thread_0 = ThreadGuard::new(thread::spawn(move || {
        rx_0.recv().unwrap();
        let mut socket = CanSocket::open(CAN_INTERFACES[0]).unwrap();
        for _ in 0..N / 2 {
            sleep(Duration::from_secs(1));
            socket
                .transmit(
                    &CanFrame::new(Id::Standard(StandardId::new(PULSE_ID).unwrap()), &[0xFF])
                        .unwrap(),
                )
                .unwrap();
        }
    }));
    let sender_thread_1 = ThreadGuard::new(thread::spawn(move || {
        rx_1.recv().unwrap();
        let mut socket = CanSocket::open(CAN_INTERFACES[1]).unwrap();
        for _ in 0..N / 2 {
            socket
                .transmit(
                    &CanFrame::new(Id::Standard(StandardId::new(PULSE_ID).unwrap()), &[0xAA])
                        .unwrap(),
                )
                .unwrap();
            sleep(Duration::from_secs(1));
        }
    }));

    // Thread collecting statistics from CAN TM.
    let receiver_thread = ThreadGuard::new(thread::spawn(move || {
        let socket = CanSocket::open(CAN_INTERFACES[0]).unwrap();
        tx_0.send(()).unwrap();
        tx_1.send(()).unwrap();
        let mut count = 0;
        for _ in 0..N * 2 {
            match socket.read_frame_timeout(Duration::from_secs(2)) {
                Ok(CanFrame::Data(frame))
                    if frame.id() == Id::Standard(StandardId::new(STAT_ID).unwrap()) =>
                {
                    count =
                        u64::from_le_bytes(frame.data()[..size_of::<u64>()].try_into().unwrap());
                    if count >= N {
                        break;
                    }
                }
                _ => {}
            }
        }
        count
    }));

    // Wait until `N` detections.
    loop {
        // This call is blocking.
        let event = observer.next();
        match event {
            Some(Event::Count(c)) if c >= N => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Stop the simulation.
    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => {}
        Err(e) => return Err(e.into()),
        _ => {}
    }

    assert_eq!(N, receiver_thread.join().unwrap());

    sender_thread_0.join().unwrap();
    sender_thread_1.join().unwrap();

    Ok(())
}

/// Gets serial port configuration.
fn get_can_port_cfg(interfaces: &[&str]) -> CanPortConfig {
    let mut loader = ConfigLoader::<CanPortConfig>::new();
    loader
        .code(format!("interfaces = {:?}", interfaces), Format::Toml)
        .unwrap();
    loader
        .code(format!("delta = {}", DELTA), Format::Toml)
        .unwrap();
    loader
        .code(format!("period = {}", PERIOD), Format::Toml)
        .unwrap();
    loader.load().unwrap().config
}
