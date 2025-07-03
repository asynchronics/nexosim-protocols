//! # Byte stream decoding utilities.
//!
//! This module contains primitives for byte stream decoding implementation.
//!
//! ## `ByteDecoderModel` model
//!
//! The main type is [`ByteDecoderModel`] model that accepts byte stream input
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
//! use serde::{self, Deserialize, Serialize};
//!
//! use nexosim_byte_utils::decode::{BufDecoder, BufDecoderResult, ByteDecoderModel, ProtoByteDecoder};
//!
//! #[derive(Default, Serialize, Deserialize)]
//! pub struct AaDecoder {}
//!
//! impl BufDecoder<()> for AaDecoder {
//!     type DecoderEnv = ();
//!
//!     fn decode<B: Buf>(&mut self, buf: &mut B, _: &()) -> BufDecoderResult<()> {
//!         while buf.has_remaining() {
//!             if buf.get_u8() == 0xAA {
//!                 return BufDecoderResult::Decoded(());
//!             }
//!         }
//!         BufDecoderResult::Empty
//!     }
//! }
//!
//! // Decoder model prototype.
//! pub type ProtoDecoder = ProtoByteDecoder<(), AaDecoder>;
//!
//! // Decoder model.
//! pub type Decoder = ByteDecoderModel<(), AaDecoder>;
//!
//! let decoder = ProtoDecoder::default();
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
//! use nexosim_byte_utils::decode::{ByteDelimitedDecoder, ByteDelimitedDecoderEnv, ProtoByteDecoder};
//!
//! let decoder = ProtoByteDecoder::new(
//!     ByteDelimitedDecoder::<(), (), ()>::new(0xFF, 0xAA),
//!     ByteDelimitedDecoderEnv::<(), (), ()>::new(|_, _| {}),
//! );
//! ```
//!
//! For a more interesting example see an implementation of the KISS protocol
//! decoder in [`kiss_decoder`] module.
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use buf_list::BufList;

use bytes::{Buf, Bytes};

use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{self, Deserialize, Deserializer, Serialize, Serializer};

use nexosim::Model;
use nexosim::model::{Context, ProtoModel};
use nexosim::ports::Output;

/// Buffer list wrapper implementing serialization.
struct Buffer(BufList);

impl Deref for Buffer {
    type Target = BufList;

    fn deref(&self) -> &BufList {
        &self.0
    }
}

impl DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut BufList {
        &mut self.0
    }
}

impl Serialize for Buffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut buf: Vec<u8> = Vec::new();
        for chunk in self.iter() {
            buf.extend(chunk);
        }
        let mut state = serializer.serialize_struct("Buffer", 1)?;
        state.serialize_field("buf", &buf)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Buffer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Buf,
        }

        struct BufferVisitor;

        impl<'de> Visitor<'de> for BufferVisitor {
            type Value = Buffer;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Buffer")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Buffer, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let buf: Vec<u8> = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let mut list = BufList::new();
                list.push_chunk(Bytes::from(buf));
                Ok(Buffer(list))
            }

            fn visit_map<V>(self, mut map: V) -> Result<Buffer, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut buf = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Buf => {
                            if buf.is_some() {
                                return Err(de::Error::duplicate_field("buf"));
                            }
                            buf = Some(map.next_value()?);
                        }
                    }
                }

                let buf: Vec<u8> = buf.ok_or_else(|| de::Error::missing_field("buf"))?;
                let mut list = BufList::new();
                list.push_chunk(Bytes::from(buf));
                Ok(Buffer(list))
            }
        }

        const FIELDS: &[&str] = &["buf"];
        deserializer.deserialize_struct("Buffer", FIELDS, BufferVisitor)
    }
}

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
pub trait BufDecoder<T>: Serialize + DeserializeOwned {
    /// Decoder environment.
    type DecoderEnv: Send;

    /// Decodes part of the input buffer consuming it.
    fn decode<B: Buf>(
        &mut self,
        buf: &mut B,
        decoder_env: &Self::DecoderEnv,
    ) -> BufDecoderResult<T>;
}

/// Byte stream decoder model prototype.
pub struct ProtoByteDecoder<T: Clone + Send + 'static, D: BufDecoder<T> + Send + 'static> {
    /// Decoded data.
    pub data_out: Output<T>,

    /// Decoder.
    decoder: D,

    /// Decoder environment.
    decoder_env: D::DecoderEnv,
}

impl<T, D> ProtoByteDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    /// Creates new byte stream decoder model.
    pub fn new(decoder: D, decoder_env: D::DecoderEnv) -> Self {
        Self {
            data_out: Output::new(),
            decoder,
            decoder_env,
        }
    }

    fn build(self) -> (ByteDecoderModel<T, D>, D::DecoderEnv) {
        (
            ByteDecoderModel {
                data_out: self.data_out,
                buf: Buffer(BufList::new()),
                decoder: self.decoder,
            },
            self.decoder_env,
        )
    }
}

impl<T, D> Default for ProtoByteDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Default + Send + 'static,
    D::DecoderEnv: Default,
{
    fn default() -> Self {
        Self::new(D::default(), D::DecoderEnv::default())
    }
}

impl<T, D> ProtoModel for ProtoByteDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    type Model = ByteDecoderModel<T, D>;

    fn build(
        self,
        _: &mut nexosim::model::BuildContext<Self>,
    ) -> (ByteDecoderModel<T, D>, D::DecoderEnv) {
        self.build()
    }
}

impl<T, D> fmt::Debug for ProtoByteDecoder<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ProtoByteDecoder").finish_non_exhaustive()
    }
}

/// Byte stream decoder model.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct ByteDecoderModel<T: Clone + Send + 'static, D: BufDecoder<T> + Send + 'static> {
    /// Decoded data.
    pub data_out: Output<T>,

    /// Internal buffer.
    buf: Buffer,

    /// Decoder.
    decoder: D,
}

#[Model(type Env = D::DecoderEnv)]
impl<T, D> ByteDecoderModel<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    /// Input bytes -- input port.
    pub async fn bytes_in(&mut self, data: Bytes, cx: &mut Context<Self>) {
        self.buf.push_chunk(data);
        loop {
            match self.decoder.decode(&mut *self.buf, cx.env()) {
                BufDecoderResult::Decoded(data) => self.data_out.send(data).await,
                BufDecoderResult::Ignored => {}
                _ => break,
            }
        }
    }
}

impl<T, D> fmt::Debug for ByteDecoderModel<T, D>
where
    T: Clone + Send + 'static,
    D: BufDecoder<T> + Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteDecoderModel").finish_non_exhaustive()
    }
}

/// Result of successful byte stream transformation.
#[derive(Clone, Debug)]
pub enum Transformed {
    /// No bytes.
    None,

    /// One byte.
    One(u8),

    /// Many bytes.
    Many(Bytes),
}

/// Result of byte stream transformation.
type TransformResult<T> = Result<Transformed, T>;

/// Transformer callback type.
type TransformCallback<T, S> =
    Box<dyn Fn(&mut S, &[u8], u8) -> TransformResult<T> + Send + 'static>;

/// Decoder callback type.
type DecodeCallback<T, S> = Box<dyn Fn(&mut S, &[u8]) -> T + Send + 'static>;

/// Packet decoder environment.
pub struct ByteDelimitedDecoderEnv<T, S, R>
where
    T: Clone + Send + 'static,
    S: Send + Serialize + for<'a> Deserialize<'a>,
    R: Send + Serialize + for<'a> Deserialize<'a>,
{
    transform: TransformCallback<T, S>,
    decode: DecodeCallback<T, R>,
}

impl<T, S, R> ByteDelimitedDecoderEnv<T, S, R>
where
    T: Clone + Send + 'static,
    S: Send + Serialize + for<'a> Deserialize<'a>,
    R: Send + Serialize + for<'a> Deserialize<'a>,
{
    /// Creates new byte delimited decoder environment.
    pub fn new<F>(decode: F) -> Self
    where
        F: Fn(&mut R, &[u8]) -> T + Send + 'static,
    {
        Self::with_transform(|_, _, c| TransformResult::Ok(Transformed::One(c)), decode)
    }

    /// Creates new byte delimited decoder environment with character transform
    /// function.
    pub fn with_transform<F, G>(transform: F, decode: G) -> Self
    where
        F: Fn(&mut S, &[u8], u8) -> TransformResult<T> + Send + 'static,
        G: Fn(&mut R, &[u8]) -> T + Send + 'static,
    {
        Self {
            transform: Box::new(transform),
            decode: Box::new(decode),
        }
    }
}

impl<T, S, R> fmt::Debug for ByteDelimitedDecoderEnv<T, S, R>
where
    T: Clone + Send + 'static,
    S: Send + Serialize + for<'a> Deserialize<'a>,
    R: Send + Serialize + for<'a> Deserialize<'a>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteDelimitedDecoderEnv")
            .finish_non_exhaustive()
    }
}

/// Packet decoder.
#[derive(Serialize, Deserialize)]
pub struct ByteDelimitedDecoder<T, S, R>
where
    T: Clone + Send + 'static,
    S: Send + Serialize,
    R: Send + Serialize,
{
    /// Packet start delimiter.
    start: u8,

    /// Packet end delimiter.
    end: u8,

    /// Packet decoding is in progress.
    is_decoding: bool,

    /// Decoder buffer.
    buf: Vec<u8>,

    /// Stream transformer state.
    transformer_state: S,

    /// Stream decoder state.
    decoder_state: R,

    /// Phantom data.
    _t: PhantomData<T>,
}

impl<T, S, R> ByteDelimitedDecoder<T, S, R>
where
    T: Clone + Send + 'static,
    S: Default + Send + Serialize,
    R: Default + Send + Serialize,
{
    /// Creates new packet decoder.
    pub fn new(start: u8, end: u8) -> Self {
        Self {
            start,
            end,
            is_decoding: false,
            buf: Vec::with_capacity(1024),
            transformer_state: S::default(),
            decoder_state: R::default(),
            _t: PhantomData {},
        }
    }
}

impl<T, S, R> BufDecoder<T> for ByteDelimitedDecoder<T, S, R>
where
    T: Clone + Send + 'static,
    S: Send + Serialize + for<'de> Deserialize<'de>,
    R: Send + Serialize + for<'de> Deserialize<'de>,
{
    type DecoderEnv = ByteDelimitedDecoderEnv<T, S, R>;

    fn decode<B: Buf>(
        &mut self,
        buf: &mut B,
        decoder_env: &Self::DecoderEnv,
    ) -> BufDecoderResult<T> {
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
                match (decoder_env.transform)(&mut self.transformer_state, &self.buf, buf.get_u8())
                {
                    Ok(result) => match result {
                        Transformed::None => {}
                        Transformed::One(byte) => self.buf.push(byte),
                        Transformed::Many(bytes) => self.buf.extend(bytes),
                    },
                    Err(data) => {
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
        BufDecoderResult::Decoded((decoder_env.decode)(&mut self.decoder_state, &self.buf))
    }
}

impl<
    T: Clone + Send + 'static,
    S: Send + Serialize + for<'de> Deserialize<'de>,
    R: Send + Serialize + for<'de> Deserialize<'de>,
> fmt::Debug for ByteDelimitedDecoder<T, S, R>
{
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
/// The following example shows a decoder that decodes every non-empty correctly
/// encoded byte sequence as a pulse.
///
/// ```rust
/// use nexosim_byte_utils::decode::kiss_decoder::{FromKiss, ProtoKissModel};
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
/// pub fn decode(_: &mut (),_: &[u8]) -> Data {
///     Data::Pulse
/// }
///
/// let mut decoder = ProtoKissModel::<Data, ()>::new(decode);
/// ```
pub mod kiss_decoder {
    use std::fmt;

    use serde::{self, Deserialize, Serialize};

    use nexosim::model::{self, BuildContext, ProtoModel};
    use nexosim::ports::Output;

    /// Byte delimiter.
    pub const FEND: u8 = 0xC0;

    /// Escape byte.
    pub const FESC: u8 = 0xDB;

    /// Transformed byte delimiter.
    pub const TFEND: u8 = 0xDC;

    /// Transformed escape byte.
    pub const TFESC: u8 = 0xDD;

    /// Trait for data that can be parsed from KISS protocol.
    pub trait FromKiss {
        /// Data variant parsed in case of message abort (i.e. wrong escape
        /// sequence).
        fn abort_variant(previous: &[u8], byte: u8) -> Self;
    }

    /// KISS protocol decoder model prototype.
    pub struct ProtoKissModel<
        T: FromKiss + Clone + Send + 'static,
        S: Default + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        const GFEND: u8 = FEND,
        const GFESC: u8 = FESC,
        const GTFEND: u8 = TFEND,
        const GTFESC: u8 = TFESC,
    > {
        /// Decoded data.
        pub data_out: Output<T>,

        /// Decoder model.
        decoder: super::ProtoByteDecoder<T, super::ByteDelimitedDecoder<T, bool, S>>,
    }

    impl<
        T: FromKiss + Clone + Send + 'static,
        S: Default + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        const GFEND: u8,
        const GFESC: u8,
        const GTFEND: u8,
        const GTFESC: u8,
    > ProtoKissModel<T, S, GFEND, GFESC, GTFEND, GTFESC>
    {
        /// Creates new KISS decoder.
        pub fn new<F>(decode: F) -> Self
        where
            F: Fn(&mut S, &[u8]) -> T + Send + 'static,
        {
            let decoder = super::ProtoByteDecoder::new(
                super::ByteDelimitedDecoder::new(GFEND, GFEND),
                super::ByteDelimitedDecoderEnv::<T, bool, S>::with_transform(
                    |is_esc: &mut bool, previous: &[u8], byte: u8| {
                        if *is_esc {
                            *is_esc = false;
                            // Matching is not possible here because of generics,
                            // see `rustc --explain E015`.
                            if byte == GTFEND {
                                Ok(super::Transformed::One(GFEND))
                            } else if byte == GTFESC {
                                Ok(super::Transformed::One(GFESC))
                            } else {
                                Err(T::abort_variant(previous, byte))
                            }
                        } else if byte == GFESC {
                            *is_esc = true;
                            Ok(super::Transformed::None)
                        } else {
                            Ok(super::Transformed::One(byte))
                        }
                    },
                    decode,
                ),
            );
            Self {
                data_out: decoder.data_out.clone(),
                decoder,
            }
        }
    }

    /// KISS decoder model.
    pub type KissModel<T, S> = super::ByteDecoderModel<T, super::ByteDelimitedDecoder<T, bool, S>>;

    impl<
        T: FromKiss + Clone + Send + 'static,
        S: Default + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        const GFEND: u8,
        const GFESC: u8,
        const GTFEND: u8,
        const GTFESC: u8,
    > ProtoModel for ProtoKissModel<T, S, GFEND, GFESC, GTFEND, GTFESC>
    {
        type Model = KissModel<T, S>;

        fn build(
            self,
            _: &mut BuildContext<Self>,
        ) -> (Self::Model, <Self::Model as model::Model>::Env) {
            self.decoder.build()
        }
    }

    impl<
        T: FromKiss + Clone + Send + 'static,
        S: Default + Send + Serialize + for<'de> Deserialize<'de> + 'static,
        const GFEND: u8,
        const GFESC: u8,
        const GTFEND: u8,
        const GTFESC: u8,
    > fmt::Debug for ProtoKissModel<T, S, GFEND, GFESC, GTFEND, GTFESC>
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("ProtoByteDecoder").finish_non_exhaustive()
        }
    }
}
