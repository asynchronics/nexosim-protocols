//! Example: a simulation that receives data from a serial port.
//!
//! To run an example, execute `serial-setup.sh` in another shell first.
//!
//! This example demonstrates in particular:
//!
//! * serial port model,
//! * infinite simulation,
//! * blocking event queue,
//! * simulation halting,
//! * system clock,
//! * observable state.
//!
//! ```text
//!                              ┏━━━━━━━━━━━━━━━━━━━━━┓
//!                              ┃ Simulation          ┃
//! ┌╌╌╌╌╌╌╌╌╌╌╌╌┐               ┃   ┌──────────┐mode  ┃
//! ┆ External   ┆    pulses     ┃   │          ├──────╂┐BlockingEventQueue
//! ┆ thread     ├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╂╌╌►│ Counter  │count ┃├───────────────────►
//! ┆            ┆ [serial port] ┃   │          ├──────╂┘
//! └╌╌╌╌╌╌╌╌╌╌╌╌┘               ┃   └──────────┘      ┃
//!                              ┗━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::thread::{self, sleep};
use std::time::Duration;

use confique::{Config, Partial};

use nexosim::model::{Context, Model};
use nexosim::ports::{BlockingEventQueue, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};
use nexosim_util::joiners::{SimulationJoiner, ThreadJoiner};
use nexosim_util::observables::ObservableValue;

use nexosim_byte_utils::decoding::{ByteStreamDecoder, SimpleDelimiterDecoder};
use nexosim_serial_port::{ProtoSerialPort, SerialPortConfig};

/// For serial ports setup see `serial-setup.sh`.
///
/// Simulation serial port.
const INTERNAL_PORT_PATH: &str = "/tmp/ttyS20";
/// Serial port used to send data.
const EXTERNAL_PORT_PATH: &str = "/tmp/ttyS21";

/// Activation period, in milliseconds, for cyclic activities inside the simulation.
const PERIOD: u64 = 10;
/// Time shift, in milliseconds, for scheduling events at the present moment.
const DELTA: u64 = 5;

const SWITCH_ON_DELAY: Duration = Duration::from_secs(1);
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
    state: ObservableValue<Mode>,

    /// Counter.
    acc: ObservableValue<u64>,
}

impl Counter {
    /// Creates a new `Counter` model.
    fn new() -> Self {
        let mode = Output::default();
        let count = Output::default();
        Self {
            mode: mode.clone(),
            count: count.clone(),
            state: ObservableValue::new(mode),
            acc: ObservableValue::new(count),
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
    let mut serial = ProtoSerialPort::new(get_serial_port_cfg(INTERNAL_PORT_PATH));

    // The decoder model.
    // The accepted pulse packet is 0xFFXXAA, where XX is any byte.
    let mut decoder = ByteStreamDecoder::new(SimpleDelimiterDecoder::<()>::new(0xFF, 0xAA, |_| {}));

    // The counter model.
    let mut counter = Counter::new();

    // Mailboxes.
    let serial_mbox = Mailbox::new();
    let decoder_mbox = Mailbox::new();
    let counter_mbox = Mailbox::new();

    // Connections.
    serial.received_data.connect(
        ByteStreamDecoder::<(), SimpleDelimiterDecoder<()>>::input_bytes,
        &decoder_mbox,
    );
    decoder.decoded_data.connect(Counter::pulse, &counter_mbox);

    // Model handles for simulation.
    let counter_addr = counter_mbox.address();
    let observer = BlockingEventQueue::new();
    counter
        .mode
        .map_connect_sink(|m| Event::Mode(*m), &observer);
    counter
        .count
        .map_connect_sink(|c| Event::Count(*c), &observer);
    let mut observer = observer.into_reader();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let (mut simu, scheduler) = SimInit::new()
        .add_model(serial, serial_mbox, "serial")
        .add_model(decoder, decoder_mbox, "decoder")
        .add_model(counter, counter_mbox, "counter")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    // Simulation thread.
    let simulation_handle = SimulationJoiner::new(
        scheduler.clone(),
        thread::spawn(move || {
            // ---------- Simulation.  ----------
            simu.step_unbounded()
        }),
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

    let mut external_port = serialport::new(EXTERNAL_PORT_PATH, 0).open().unwrap();
    // Thread sending data to the serial port.
    let sender_thread = ThreadJoiner::new(thread::spawn(move || {
        for i in 0..N {
            if i % 5 == 1 {
                sleep(Duration::from_secs(1));
            }
            external_port.write_all(&[0xFF]).unwrap();
            if i % 5 == 2 {
                sleep(Duration::from_secs(1));
            }
            external_port.write_all(&[0x55]).unwrap();
            if i % 5 == 3 {
                sleep(Duration::from_secs(1));
            }
            external_port.write_all(&[0xAA]).unwrap();
        }
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
    match simulation_handle.halt().unwrap() {
        Err(ExecutionError::Halted) => {}
        Err(e) => return Err(e.into()),
        _ => {}
    }

    sender_thread.join().unwrap();

    Ok(())
}

/// Gets serial port configuration.
fn get_serial_port_cfg(path: &str) -> SerialPortConfig {
    let mut partial_cfg = <SerialPortConfig as Config>::Partial::empty();
    partial_cfg.port_path = Some(path.to_string());
    partial_cfg.delta = Some(DELTA);
    partial_cfg.period = Some(PERIOD);
    SerialPortConfig::builder()
        .preloaded(partial_cfg)
        .load()
        .unwrap()
}
