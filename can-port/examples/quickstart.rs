//! Example: basic CAN port model usage.
//!
//! Before running an example, execute `can-setup.sh` from `examples` directory.
//!
//! This example demonstrates in particular:
//!
//! * CAN port model,
//! * infinite simulation,
//! * simulation halting,
//! * system clock.
//!
use std::error::Error;
use std::thread::{self, sleep};
use std::time::Duration;

use schematic::{ConfigLoader, Format};

use socketcan::{BlockingCan, CanFrame, CanSocket, EmbeddedFrame, Id, Socket, StandardId};

use thread_guard::ThreadGuard;

use nexosim::ports::EventQueue;
use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
use nexosim::time::{AutoSystemClock, MonotonicTime};

use nexosim_can_port::{CanPortConfig, ProtoCanPort};

/// For CAN ports setup see `can-setup.sh`.
///
/// CAN interfaces.
const CAN_INTERFACES: &[&str] = &["vcan0", "vcan1"];

/// Pulse data ID.
const ID: u16 = 0x100;

/// Activation period, in milliseconds, for cyclic activities inside the simulation.
const PERIOD: u64 = 10;
/// Time shift, in milliseconds, for scheduling events at the present moment.
const DELTA: u64 = 5;

fn main() -> Result<(), Box<dyn Error>> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    // The CAN port model.
    let mut loader = ConfigLoader::<CanPortConfig>::new();
    loader
        .code(format!("interfaces = {CAN_INTERFACES:?}"), Format::Toml)
        .unwrap();
    loader
        .code(format!("delta = {DELTA}"), Format::Toml)
        .unwrap();
    loader
        .code(format!("period = {PERIOD}"), Format::Toml)
        .unwrap();
    let mut can = ProtoCanPort::new(loader.load().unwrap().config);

    // Mailboxes.
    let can_mbox = Mailbox::new();

    // Model handles for simulation.
    let frames = EventQueue::new();
    can.frame_out.connect_sink(&frames);
    let mut frames = frames.into_reader();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = SimInit::new()
        .add_model(can, can_mbox, "can")
        .set_clock(AutoSystemClock::new())
        .init(t0)?
        .0;

    let mut sim_scheduler = simu.scheduler();

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
            println!("Simulation thread result: {res:?}.");
        },
    );

    // Frame to be sent.
    let frame = CanFrame::new(Id::Standard(StandardId::new(ID).unwrap()), &[0xFF]).unwrap();

    // Send frame to CAN interface.
    let mut socket = CanSocket::open(CAN_INTERFACES[0]).unwrap();
    socket.transmit(&frame)?;

    // Wait for update method to forward frame.
    sleep(Duration::from_millis(PERIOD + 2 * DELTA));

    // Receive the frame from the simulation.
    let received = frames.next().unwrap();
    assert_eq!(0, received.interface);
    assert_eq!(frame.id(), received.frame.id());
    assert_eq!(frame.data(), received.frame.data());

    // Stop the simulation.
    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}
