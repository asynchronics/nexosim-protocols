//! Example: a simple pulse decoder on top of the KISS protocol.
//!
//! This example demonstrates in particular:
//!
//! * `KissDecoder` model usage.
//!
//! ```text
//!                        ┌───────────┐
//!                bytes   │           │ pulses
//! Byte stream ●─────────►│  Decoder  ├────────►
//!                        │           │
//!                        └───────────┘
//! ```

use nexosim::ports::EventQueue;
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

use nexosim_byte_utils::decode::kiss_decoder::{FEND, FESC, FromKiss, KissDecoder};

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
pub fn decode(_: &[u8]) -> Data {
    Data::Pulse
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    let mut decoder = KissDecoder::<Data>::with_decode_callback(decode);

    // Mailboxes.
    let decoder_mbox = Mailbox::new();

    // Model handles for simulation.
    let decoded = EventQueue::new();
    decoder.data_out.connect_sink(&decoded);
    let mut decoded = decoded.into_reader();
    let decoder_addr = decoder_mbox.address();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = SimInit::new()
        .add_model(decoder, decoder_mbox, "decoder")
        .init(t0)?
        .0;

    // ----------
    // Simulation.
    // ----------

    // Send data with no frame encoded.
    simu.process_event(KissDecoder::bytes_in, vec![0x00].into(), &decoder_addr)?;
    assert_eq!(decoded.next(), None);

    // Send data with two correct frames.
    simu.process_event(
        KissDecoder::bytes_in,
        vec![FEND, 0xAA, FEND, FEND, FEND, 0x01, FEND].into(),
        &decoder_addr,
    )?;
    for _ in 0..2 {
        assert_eq!(decoded.next(), Some(Data::Pulse));
    }
    assert_eq!(decoded.next(), None);

    // Send beginning of a frame.
    simu.process_event(
        KissDecoder::bytes_in,
        vec![FEND, 0xAA].into(),
        &decoder_addr,
    )?;
    assert_eq!(decoded.next(), None);

    // Finish the frame.
    simu.process_event(KissDecoder::bytes_in, vec![FEND].into(), &decoder_addr)?;
    assert_eq!(decoded.next(), Some(Data::Pulse));

    // Abort transmition.
    simu.process_event(
        KissDecoder::bytes_in,
        vec![FEND, 0xAA, FESC, FESC].into(),
        &decoder_addr,
    )?;
    assert_eq!(decoded.next(), Some(Data::Aborted));
    assert_eq!(decoded.next(), None);

    Ok(())
}
