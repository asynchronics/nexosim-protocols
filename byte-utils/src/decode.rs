//! Byte stream decoding utilities.
use std::fmt;

use buf_list::BufList;

use bytes::{Buf, Bytes};

use nexosim::model::Model;
use nexosim::ports::Output;

/// Buffer decoding result.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufDecoderResult<T, E> {
    /// An error.
    Error(E),
    /// Input buffer consumed, nothing decoded.
    Empty,
    /// Input buffer consumed, message decoding in progress.
    Partial,
    /// Part of the input ignored, there is more data.
    Ignored,
    /// Part of the input buffer is decoded, there may be more data.
    Decoded(T),
}

/// Buffer decoder trait.
pub trait BufDecoder<T> {
    /// Error type.
    type Error;

    /// Decodes part of the input buffer consuming it.
    fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<T, Self::Error>;
}

/// Byte stream decoder model.
pub struct ByteStreamDecoder<T: Clone + Send + 'static, D: BufDecoder<T> + Send + 'static> {
    /// Decoded data.
    pub data_out: Output<T>,

    /// Internal buffer.
    buf: BufList,

    /// Data decoder.
    decoder: D,
}

impl<T, D> ByteStreamDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    /// Creates new byte stream decoder model.
    pub fn new(decoder: D) -> Self {
        Self {
            data_out: Output::new(),
            buf: BufList::new(),
            decoder,
        }
    }

    /// Input bytes -- input port.
    pub async fn bytes_in(&mut self, data: Bytes) {
        self.buf.push_chunk(data);
        loop {
            match self.decoder.decode(&mut self.buf) {
                BufDecoderResult::Decoded(data) => self.data_out.send(data).await,
                BufDecoderResult::Ignored => {}
                _ => break,
            }
        }
    }
}

impl<T, D> Model for ByteStreamDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
}

impl<T, D> fmt::Debug for ByteStreamDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteStreamDecoder").finish_non_exhaustive()
    }
}

/// Decoder callback type.
pub type DecodeCallback<T> = Box<dyn Fn(&[u8]) -> T + Send + 'static>;

/// Packet decoder.
pub struct ByteDelimitedDecoder<T: Clone + Send + 'static> {
    /// Packet start delimiter.
    start: u8,

    /// Packet end delimiter.
    end: u8,

    /// Decoder callback.
    decode_callback: DecodeCallback<T>,

    /// Packet decoding is in progress.
    is_decoding: bool,

    /// Decoder buffer.
    buf: Vec<u8>,
}

impl<T: Clone + Send + 'static> ByteDelimitedDecoder<T> {
    /// Creates new packet decoder.
    pub fn new<F>(start: u8, end: u8, decode: F) -> Self
    where
        F: Fn(&[u8]) -> T + Send + 'static,
    {
        Self {
            start,
            end,
            decode_callback: Box::new(decode),
            is_decoding: false,
            buf: Vec::with_capacity(1024),
        }
    }
}

impl<T: Clone + Send + 'static> BufDecoder<T> for ByteDelimitedDecoder<T> {
    type Error = ();

    fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<T, Self::Error> {
        if !self.is_decoding {
            self.buf.clear();
            while buf.has_remaining() && buf.chunk()[0] != self.start {
                buf.advance(1);
            }
            if !buf.has_remaining() {
                return BufDecoderResult::Empty;
            }
            buf.advance(1);
            self.is_decoding = true;
        }
        while buf.has_remaining() && buf.chunk()[0] != self.end {
            self.buf.push(buf.get_u8());
        }
        if !buf.has_remaining() {
            return BufDecoderResult::Partial;
        }
        self.is_decoding = false;
        if self.buf.is_empty() {
            return BufDecoderResult::Ignored;
        }
        if self.start != self.end {
            buf.advance(1);
        }
        BufDecoderResult::Decoded((self.decode_callback)(&self.buf))
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for ByteDelimitedDecoder<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteDelimitedDecoder")
            .finish_non_exhaustive()
    }
}
