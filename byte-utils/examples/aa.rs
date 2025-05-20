//! Example: a simple pulse decoder.
//!
//! This example demonstrates in particular:
//!
//! * `ByteStreamDecoder` model usage,
//! * `BufDecoder` implementation.
//!
//! ```text
//!                        ┌───────────┐
//!                bytes   │           │ pulses
//! Byte stream ●─────────►│  Decoder  ├────────►
//!                        │           │
//!                        └───────────┘
//! ```

use bytes::Buf;

use nexosim::ports::EventQueue;
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

use nexosim_byte_utils::decode::{BufDecoder, BufDecoderResult, ByteStreamDecoder};

/// Simple pulse decoder.
#[derive(Default)]
pub struct AaDecoder {}

impl BufDecoder<()> for AaDecoder {
    fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<()> {
        while buf.has_remaining() {
            if buf.get_u8() == 0xAA {
                return BufDecoderResult::Decoded(());
            }
        }
        BufDecoderResult::Empty
    }
}

/// Decoder model.
pub type Decoder = ByteStreamDecoder<(), AaDecoder>;

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    let mut decoder = Decoder::default();

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

    // Send data with no pulses encoded.
    simu.process_event(Decoder::bytes_in, vec![0x00].into(), &decoder_addr)?;
    assert_eq!(decoded.next(), None);

    // Send data with two pulses encoded.
    simu.process_event(
        Decoder::bytes_in,
        vec![0x01, 0xAA, 0xAA].into(),
        &decoder_addr,
    )?;
    for _ in 0..2 {
        assert_eq!(decoded.next(), Some(()));
    }
    assert_eq!(decoded.next(), None);

    Ok(())
}
