//! # Byte stream decoding utilities.
//!
//! This module contains primitives for byte stream decoding implementation.
//!
//! ## `ByteStreamDecoder` model
//!
//! The main type is [`ByteStreamDecoder`] model that accepts byte stream input
//! and outputs the parsed data. The parsed data is expected to include variant
//! for decoding errors that are not ignored. This model is generic over
//! [`BufDecoder`] that implements decoding functionality. Its method
//! [`BufDecoder::decode`] operates on an implementer of the
//! [`bytes::Buf`](https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html)
//! trait. The decoded result can have one of the following values:
//! * [`BufDecoderResult::Empty`] meaning that buffer has been exhausted and
//!   ignored,
//! * [`BufDecoderResult::Partial`] meaning that buffer has been exhausted and
//!   part of messages parsed,
//! * [`BufDecoderResult::Ignored`] meaning that part of the buffer has been
//!   consumed and ignored,
//! * [`BufDecoderResult::Decoded`] meaning that part of the buffer has been
//!   consumed and decoded.
//!
//! The following example shows a model that produces a pulse for every `0xAA`
//! byte in the input stream and ignores all the other bytes. To make its usage
//! easier a new type is defined.
//!
//! ```rust
//! use bytes::Buf;
//!
//! use nexosim_byte_utils::decode::{BufDecoder, BufDecoderResult, ByteStreamDecoder};
//! #[derive(Default)]
//! pub struct AaDecoder {}
//!
//! impl BufDecoder<()> for AaDecoder {
//!     fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<()> {
//!         while buf.has_remaining() {
//!             if buf.get_u8() == 0xAA {
//!                 return BufDecoderResult::Decoded(());
//!             }
//!         }
//!         BufDecoderResult::Empty
//!     }
//! }
//!
//! pub type Decoder = ByteStreamDecoder<(), AaDecoder>;
//!
//! let decoder = Decoder::default();
//! ```
//!
//! ## `ByteDelimitedDecoder`
//!
//! [`ByteDelimitedDecoder`] can serve as a more complicated example. In its
//! simplest usage it can decode data separated by delimiter bytes as in the
//! following example, which shows how to generate a pulse for every byte
//! sequence of the form `[0xFF, ..., 0xAA]`, where `...` is any non-empty
//! sequence of bytes.
//!
//! ```rust
//! use nexosim_byte_utils::decode::{ByteDelimitedDecoder, ByteStreamDecoder};
//!
//! let decoder = ByteStreamDecoder::new(ByteDelimitedDecoder::<()>::new(0xFF, 0xAA, |_| {}));
//! ```
//!
//! For a more interesting example see an implementation of the KISS protocol
//! decoder in [`kiss_decoder`] module.
use std::fmt;

use buf_list::BufList;

use bytes::{Buf, Bytes};

use nexosim::model::Model;
use nexosim::ports::Output;

/// Buffer decoding result.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufDecoderResult<T> {
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
    /// Decodes part of the input buffer consuming it.
    fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<T>;
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

impl<T, D> Default for ByteStreamDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Default + Send + 'static,
{
    fn default() -> Self {
        Self::new(D::default())
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

/// Result of byte stream transformation.
#[derive(Clone, Debug)]
pub enum TransformResult<T> {
    /// No bytes.
    None,

    /// One byte.
    One(u8),

    /// Many bytes.
    Many(Bytes),

    /// Transmission aborted.
    Abort(T),
}

/// Trait for byte stream transformer (e.g. de-escaper).
pub trait ByteTransformer<T> {
    /// Transforms byte.
    fn transform(&mut self, previous: &[u8], byte: u8) -> TransformResult<T>;
}

/// Default byte transformer.
impl<T> ByteTransformer<T> for () {
    fn transform(&mut self, _: &[u8], byte: u8) -> TransformResult<T> {
        TransformResult::One(byte)
    }
}

/// Decoder callback type.
pub type DecodeCallback<T> = Box<dyn FnMut(&[u8]) -> T + Send + 'static>;

/// Packet decoder.
pub struct ByteDelimitedDecoder<T, S = ()>
where
    T: Clone + Send + 'static,
    S: ByteTransformer<T>,
{
    /// Packet start delimiter.
    start: u8,

    /// Packet end delimiter.
    end: u8,

    /// Byte stream transformer (e.g. de-escaper).
    transformer: S,

    /// Decoder callback.
    decode_callback: DecodeCallback<T>,

    /// Packet decoding is in progress.
    is_decoding: bool,

    /// Decoder buffer.
    buf: Vec<u8>,
}

impl<T, S> ByteDelimitedDecoder<T, S>
where
    T: Clone + Send + 'static,
    S: ByteTransformer<T> + Default,
{
    /// Creates new packet decoder.
    pub fn new<F>(start: u8, end: u8, decode: F) -> Self
    where
        F: Fn(&[u8]) -> T + Send + 'static,
    {
        Self::with_transformer(start, end, S::default(), decode)
    }
}

impl<T, S> ByteDelimitedDecoder<T, S>
where
    T: Clone + Send + 'static,
    S: ByteTransformer<T>,
{
    /// Creates new packet decoder.
    pub fn with_transformer<F>(start: u8, end: u8, transformer: S, decode: F) -> Self
    where
        F: Fn(&[u8]) -> T + Send + 'static,
    {
        Self {
            start,
            end,
            transformer,
            decode_callback: Box::new(decode),
            is_decoding: false,
            buf: Vec::with_capacity(1024),
        }
    }
}

impl<T, S> BufDecoder<T> for ByteDelimitedDecoder<T, S>
where
    T: Clone + Send + 'static,
    S: ByteTransformer<T>,
{
    fn decode<B: Buf>(&mut self, buf: &mut B) -> BufDecoderResult<T> {
        loop {
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
                match self.transformer.transform(&self.buf, buf.get_u8()) {
                    TransformResult::None => {}
                    TransformResult::One(byte) => self.buf.push(byte),
                    TransformResult::Many(bytes) => self.buf.extend(bytes),
                    TransformResult::Abort(data) => {
                        self.is_decoding = false;
                        return BufDecoderResult::Decoded(data);
                    }
                }
            }
            if !buf.has_remaining() {
                return BufDecoderResult::Partial;
            }
            self.is_decoding = false;
            if !self.buf.is_empty() {
                break;
            }
        }
        buf.advance(1);
        BufDecoderResult::Decoded((self.decode_callback)(&self.buf))
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for ByteDelimitedDecoder<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteDelimitedDecoder")
            .finish_non_exhaustive()
    }
}

/// # KISS protocol decoder.
///
/// This module implements [KISS
/// protocol](https://en.wikipedia.org/wiki/KISS_(amateur_radio_protocol)) data
/// decoding KISS.
///
/// The following example shows a decoder that decodes very non-empty correctly
/// encoded byte sequence as a pulse.
///
/// ```rust
/// use nexosim_byte_utils::decode::kiss_decoder::{FromKiss, KissDecoder};
///
/// #[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// pub enum Data {
///     Pulse,
///     Aborted,
/// }
///
/// impl FromKiss for Data {
///     fn abort_variant(_: &[u8], _: u8) -> Self {
///         Data::Aborted
///     }
/// }
///
/// pub fn decode(_: &[u8]) -> Data {
///     Data::Pulse
/// }
///
/// let mut decoder = KissDecoder::<Data>::with_decode_callback(decode);
/// ```
pub mod kiss_decoder {
    use std::fmt;
    use std::marker::PhantomData;

    /// Byte delimiter.
    pub const FEND: u8 = 0xC0;

    /// Escape byte.
    pub const FESC: u8 = 0xDB;

    /// Transformed byte delimiter.
    pub const TFEND: u8 = 0xDC;

    /// Transformed escape byte.
    pub const TFESC: u8 = 0xDD;

    /// KISS protocol decoder.
    pub type KissDecoder<
        T,
        const GFEND: u8 = FEND,
        const GFESC: u8 = FESC,
        const GTFEND: u8 = TFEND,
        const GTFESC: u8 = TFESC,
    > = super::ByteStreamDecoder<
        T,
        super::ByteDelimitedDecoder<T, KissTransformer<T, GFEND, GFESC, GTFEND, GTFESC>>,
    >;

    impl<
        T: FromKiss + Clone + Send + 'static,
        const GFEND: u8,
        const GFESC: u8,
        const GTFEND: u8,
        const GTFESC: u8,
    > KissDecoder<T, GFEND, GFESC, GTFEND, GTFESC>
    {
        /// Creates new KISS decoder.
        pub fn with_decode_callback<F>(decode: F) -> Self
        where
            F: Fn(&[u8]) -> T + Send + 'static,
        {
            super::ByteStreamDecoder::new(super::ByteDelimitedDecoder::new(GFEND, GFEND, decode))
        }
    }

    /// Trait for data that can be parsed from KISS protocol.
    pub trait FromKiss {
        /// Data variant parsed in case of message abort (i.e. wrong escape
        /// sequence).
        fn abort_variant(previous: &[u8], byte: u8) -> Self;
    }

    /// KISS byte stream transformer that handles byte escaping.
    pub struct KissTransformer<
        T: FromKiss,
        const GFEND: u8 = FEND,
        const GFESC: u8 = FESC,
        const GTFEND: u8 = TFEND,
        const GTFESC: u8 = TFESC,
    > {
        /// Flag showing that the previous byte is FESD.
        is_esc: bool,

        /// Phantom data of type T.
        _phantom: PhantomData<T>,
    }

    impl<T: FromKiss, const GFEND: u8, const GFESC: u8, const GTFEND: u8, const GTFESC: u8>
        super::ByteTransformer<T> for KissTransformer<T, GFEND, GFESC, GTFEND, GTFESC>
    {
        fn transform(&mut self, previous: &[u8], byte: u8) -> super::TransformResult<T> {
            if self.is_esc {
                self.is_esc = false;
                // Matching is not possible here because of generics,
                // see `rustc --explain E015`.
                if byte == GTFEND {
                    super::TransformResult::One(GFEND)
                } else if byte == GTFESC {
                    super::TransformResult::One(GFESC)
                } else {
                    super::TransformResult::Abort(T::abort_variant(previous, byte))
                }
            } else if byte == GFESC {
                self.is_esc = true;
                super::TransformResult::None
            } else {
                super::TransformResult::One(byte)
            }
        }
    }

    impl<T: FromKiss, const GFEND: u8, const GFESC: u8, const GTFEND: u8, const GTFESC: u8> Default
        for KissTransformer<T, GFEND, GFESC, GTFEND, GTFESC>
    {
        fn default() -> Self {
            Self {
                is_esc: false,
                _phantom: PhantomData,
            }
        }
    }

    impl<T: FromKiss, const GFEND: u8, const GFESC: u8, const GTFEND: u8, const GTFESC: u8>
        fmt::Debug for KissTransformer<T, GFEND, GFESC, GTFEND, GTFESC>
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("KissTransformer").finish_non_exhaustive()
        }
    }
}
