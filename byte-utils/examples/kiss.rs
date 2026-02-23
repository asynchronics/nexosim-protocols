//! Example: a simple pulse decoder on top of the KISS protocol.
//!
//! This example demonstrates in particular:
//!
//! * `KissModel` model usage.
//!
//! ```text
//!                        ┌───────────┐
//!                bytes   │           │ pulses
//! Byte stream ●─────────►│  Decoder  ├────────►
//!                        │           │
//!                        └───────────┘
//! ```

use nexosim::ports::{EventSinkReader, EventSource, SinkState, event_queue};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

use nexosim_byte_utils::decode::kiss_decoder::{
    FEND, FESC, FromKiss, KissModel, ProtoKissModel, TFEND,
};

/// Decoded data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Data {
    Pulse,
    Aborted,
}

impl FromKiss for Data {
    fn abort_variant(_: &[u8], _: u8) -> Self {
        Data::Aborted
    }
}

/// Treat any correct frame as a pulse.
pub fn decode(_: &mut (), _: &[u8]) -> Data {
    Data::Pulse
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.
    let mut decoder = ProtoKissModel::<Data, ()>::new(decode);

    // Mailboxes.
    let decoder_mbox = Mailbox::new();

    // Bench.
    let mut bench = SimInit::new();

    // Model handles for simulation.
    let (sink, mut decoded) = event_queue(SinkState::Enabled);
    decoder.data_out.connect_sink(sink);

    let bytes_in = EventSource::new()
        .connect(KissModel::bytes_in, &decoder_mbox)
        .register(&mut bench);

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = bench.add_model(decoder, decoder_mbox, "decoder").init(t0)?;

    // ----------
    // Simulation.
    // ----------

    // Send data with no frame encoded.
    simu.process_event(&bytes_in, vec![0x00].into())?;
    assert_eq!(decoded.try_read(), None);

    // Send data with two correct frames.
    simu.process_event(
        &bytes_in,
        vec![FEND, 0xAA, FEND, FEND, FEND, 0x01, FEND].into(),
    )?;
    for _ in 0..2 {
        assert_eq!(decoded.try_read(), Some(Data::Pulse));
    }
    assert_eq!(decoded.try_read(), None);

    // Send beginning of a frame.
    simu.process_event(&bytes_in, vec![FEND, 0xAA].into())?;
    assert_eq!(decoded.try_read(), None);

    // Finish the frame.
    simu.process_event(&bytes_in, vec![FEND].into())?;
    assert_eq!(decoded.try_read(), Some(Data::Pulse));

    // Send data with an escaped byte
    simu.process_event(&bytes_in, vec![FEND, FESC, TFEND, 0xAA, FEND].into())?;
    assert_eq!(decoded.try_read(), Some(Data::Pulse));

    // Abort transmition.
    simu.process_event(&bytes_in, vec![FEND, 0xAA, FESC, FESC, 0xBB, FEND].into())?;
    assert_eq!(decoded.try_read(), Some(Data::Aborted));

    // Ignoring last FESC.
    simu.process_event(&bytes_in, vec![FEND, 0xAA, FESC, FEND].into())?;
    assert_eq!(decoded.try_read(), Some(Data::Pulse));

    assert_eq!(decoded.try_read(), None);

    Ok(())
}
