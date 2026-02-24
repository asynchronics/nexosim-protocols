//! Example: a simulation that receives data from a serial port.
//!
//! Before running an example, execute `serial-setup.sh` in another shell.
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
//! ┆ External   ┆    pulses►    ┃   │          ├──────╂┐ EventQueue
//! ┆ threads    ┆◄╌╌╌╌╌╌╌╌╌╌╌╌╌╌╂╌╌►│ Counter  │count ┃├───────────────────►
//! ┆            ┆   ◄count      ┃   │          ├──────╂┘
//! └╌╌╌╌╌╌╌╌╌╌╌╌┘ [serial port] ┃   └──────────┘      ┃
//!                              ┗━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::thread::{self, sleep};
use std::time::Duration;

use schematic::{ConfigLoader, Format};

use serde::{Deserialize, Serialize};

use thread_guard::ThreadGuard;

use nexosim::model::{Context, Model, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_queue};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime, PeriodicTicker};
use nexosim_util::observable::Observable;

use nexosim_byte_utils::decode::{
    ByteDecoderModel, ByteDelimitedDecoder, ByteDelimitedDecoderEnv, ProtoByteDecoder,
};
use nexosim_serial_port::{ProtoSerialPort, SerialPort, SerialPortConfig};

/// For serial ports setup see `serial-setup.sh`.
///
/// Simulation serial port.
const INTERNAL_PORT_PATH: &str = "/tmp/ttyS20";
/// Serial port used to send data.
const EXTERNAL_PORT_PATH: &str = "/tmp/ttyS21";

/// Counter switch on delay.
const SWITCH_ON_DELAY: Duration = Duration::from_secs(1);

/// Number of detections.
const N: u8 = 10;

/// Counter mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Off,
    On,
}

/// Simulation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Mode(Mode),
    Count(u8),
}

/// The `Counter` Model.
#[derive(Serialize, Deserialize)]
pub struct Counter {
    /// Operation mode.
    pub mode: Output<Mode>,

    /// Pulses count.
    pub count: Output<u8>,

    /// Internal state.
    state: Observable<Mode>,

    /// Counter.
    acc: Observable<u8>,
}

#[Model]
impl Counter {
    /// Creates a new `Counter` model.
    fn new() -> Self {
        let mode = Output::default();
        let count = Output::default();
        Self {
            mode: mode.clone(),
            count: count.clone(),
            state: Observable::with_default(mode),
            acc: Observable::with_default(count),
        }
    }

    /// Power -- input port.
    pub async fn power_in(&mut self, on: bool, cx: &Context<Self>) {
        match *self.state {
            Mode::Off if on => cx
                .schedule_event(SWITCH_ON_DELAY, schedulable!(Self::switch_on), ())
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
    #[nexosim(schedulable)]
    async fn switch_on(&mut self) {
        self.state.set(Mode::On).await;
    }

    /// Switches `Counter` off.
    async fn switch_off(&mut self) {
        self.state.set(Mode::Off).await;
    }
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    // The serial port model.
    let mut serial = ProtoSerialPort::new(get_serial_port_cfg(INTERNAL_PORT_PATH));

    // The decoder model.
    //
    // The accepted pulse packet is 0xFFXXAA, where XX is any non-empty sequence
    // of bytes.
    let mut decoder = ProtoByteDecoder::new(
        ByteDelimitedDecoder::<(), (), ()>::new(0xFF, 0xAA),
        ByteDelimitedDecoderEnv::<(), (), ()>::new(|_, _| {}),
    );

    // The counter model.
    let mut counter = Counter::new();

    // Mailboxes.
    let serial_mbox = Mailbox::new();
    let decoder_mbox = Mailbox::new();
    let counter_mbox = Mailbox::new();

    // Bench.
    let mut bench = SimInit::new();

    // Connections.
    serial.bytes_out.connect(
        ByteDecoderModel::<(), ByteDelimitedDecoder<(), (), ()>>::bytes_in,
        &decoder_mbox,
    );
    decoder.data_out.connect(Counter::pulse, &counter_mbox);
    counter
        .count
        .map_connect(|c| (vec![*c]).into(), SerialPort::bytes_in, &serial_mbox);

    // Model handles for simulation.
    let (sink, mut observer) = event_queue(SinkState::Enabled);
    counter
        .mode
        .map_connect_sink(|m| Event::Mode(*m), sink.clone());
    counter.count.map_connect_sink(|c| Event::Count(*c), sink);

    let power_in = EventSource::new()
        .connect(Counter::power_in, &counter_mbox)
        .register(&mut bench);

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = bench
        .add_model(serial, serial_mbox, "serial")
        .add_model(decoder, decoder_mbox, "decoder")
        .add_model(counter, counter_mbox, "counter")
        .with_clock(
            AutoSystemClock::new(),
            PeriodicTicker::new(Duration::from_millis(100)),
        )
        .init(t0)?;

    let scheduler = simu.scheduler();

    // Simulation thread.
    let sim_scheduler = scheduler.clone();
    let simulation_handle = ThreadGuard::with_actions(
        thread::spawn(move || {
            // ---------- Simulation.  ----------
            simu.run()
        }),
        move |_| {
            sim_scheduler.halt();
        },
        |_, res| {
            println!("Simulation thread result: {res:?}.");
        },
    );

    // Switch the counter on.
    scheduler.schedule_event(Duration::from_millis(1), &power_in, true)?;

    // Wait until counter mode is `On`.
    loop {
        let event = observer.read();
        match event {
            Some(Event::Mode(Mode::On)) => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    let mut receiver_port = serialport::new(EXTERNAL_PORT_PATH, 0).open().unwrap();
    let mut sender_port = receiver_port.try_clone().unwrap();

    // Thread receiving data from the serial port.
    let receiver_thread = ThreadGuard::new(thread::spawn(move || {
        let mut buffer = [0; 10];
        let mut count = 0;
        for _ in 0..N {
            sleep(Duration::from_secs(1));
            if let Ok(n) = receiver_port.read(&mut buffer)
                && n > 0
            {
                count = buffer[n - 1];
                if count >= N {
                    break;
                }
            }
        }
        count
    }));

    // Thread sending data to the serial port.
    let sender_thread = ThreadGuard::new(thread::spawn(move || {
        for i in 0..N {
            if i % 5 == 1 {
                sleep(Duration::from_secs(1));
            }
            sender_port.write_all(&[0xFF]).unwrap();
            if i % 5 == 2 {
                sleep(Duration::from_secs(1));
            }
            sender_port.write_all(&[0x55]).unwrap();
            if i % 5 == 3 {
                sender_port.write_all(&[0xBE]).unwrap();
                sleep(Duration::from_secs(1));
            }
            sender_port.write_all(&[0xAA]).unwrap();
        }
    }));

    // Wait for `N` detections.
    loop {
        // This call is blocking.
        match observer.read() {
            Some(Event::Count(c)) if c >= N => {
                break;
            }
            None => panic!("Unexpected timeout or simulation halt!"),
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

    sender_thread.join().unwrap();

    Ok(())
}

/// Gets serial port configuration.
fn get_serial_port_cfg(path: &str) -> SerialPortConfig {
    let mut loader = ConfigLoader::<SerialPortConfig>::new();
    loader
        .code(format!("portPath = \"{path}\""), Format::Toml)
        .unwrap();
    loader.load().unwrap().config
}
