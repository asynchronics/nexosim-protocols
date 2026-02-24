use std::array;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use bytes::Bytes;

use nexosim::time::MonotonicTime;
use serde::{Deserialize, Serialize};

use crate::codegen::ygw;

/// An encoded [`YamcsValue`].
///
/// This is an intentionally opaque type that encodes a value according to the
/// Yamcs gateway protocol.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncodedYamcsValue(pub(crate) ygw::value::V);

/// An error returned when decoding a [`YamcsValue`] fails.
#[derive(Debug)]
pub struct DecodeError;

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not interpret the value as the requested type")
    }
}

impl Error for DecodeError {}

/// A parameter value that can be exchanged with a Yamcs instance.
pub trait YamcsValue: Sized {
    /// Encode this value.
    fn encode(self) -> EncodedYamcsValue;
    /// Attempts to decode this value.
    fn decode(value: EncodedYamcsValue) -> Result<Self, DecodeError>;
}

/// One of the pre-defined basic types supported by Yamcs.
pub trait BasicYamcsValue: YamcsValue {
    /// The name of a type knwown to Yamcs.
    fn ptype() -> &'static str;
}

// Derives `YmacsValue` for simple types.
macro_rules! derive_ymacs_value {
    ($ty:ty, $variant:ident) => {
        impl YamcsValue for $ty {
            fn encode(self) -> EncodedYamcsValue {
                EncodedYamcsValue(ygw::value::V::$variant(self.try_into().unwrap()))
            }

            fn decode(value: EncodedYamcsValue) -> Result<Self, DecodeError> {
                if let ygw::value::V::$variant(v) = value.0 {
                    Ok(v.try_into().unwrap())
                } else {
                    Err(DecodeError)
                }
            }
        }
    };
}

// Derives `BasicYamcsValue` for basic types.
macro_rules! derive_basic_ymacs_value {
    ($ty:ty, $ptype:literal) => {
        impl BasicYamcsValue for $ty {
            fn ptype() -> &'static str {
                $ptype
            }
        }
    };
}

derive_ymacs_value!(u8, Uint32Value);
derive_ymacs_value!(i8, Sint32Value);
derive_ymacs_value!(u16, Uint32Value);
derive_ymacs_value!(i16, Sint32Value);

derive_ymacs_value!(u32, Uint32Value);
derive_basic_ymacs_value!(u32, "uint32");
derive_ymacs_value!(i32, Sint32Value);
derive_basic_ymacs_value!(i32, "sint32");
derive_ymacs_value!(u64, Uint64Value);
derive_basic_ymacs_value!(u64, "uint64");
derive_ymacs_value!(i64, Sint64Value);
derive_basic_ymacs_value!(i64, "sint64");
derive_ymacs_value!(f32, FloatValue);
derive_basic_ymacs_value!(f32, "float");
derive_ymacs_value!(f64, DoubleValue);
derive_basic_ymacs_value!(f64, "double");
derive_ymacs_value!(bool, BooleanValue);
derive_basic_ymacs_value!(bool, "boolean");
derive_ymacs_value!(String, StringValue);
derive_basic_ymacs_value!(String, "string");
derive_ymacs_value!(Bytes, BinaryValue);
derive_basic_ymacs_value!(Bytes, "binary");

#[cfg(target_pointer_width = "32")]
derive_ymacs_value!(usize, Uint32Value);
#[cfg(target_pointer_width = "32")]
derive_basic_ymacs_value!(usize, "uint32");
#[cfg(target_pointer_width = "32")]
derive_ymacs_value!(isize, Sint32Value);
#[cfg(target_pointer_width = "32")]
derive_basic_ymacs_value!(isize, "sint32");
#[cfg(target_pointer_width = "64")]
derive_ymacs_value!(usize, Uint64Value);
#[cfg(target_pointer_width = "64")]
derive_basic_ymacs_value!(usize, "uint64");
#[cfg(target_pointer_width = "64")]
derive_ymacs_value!(isize, Sint64Value);
#[cfg(target_pointer_width = "64")]
derive_basic_ymacs_value!(isize, "sint64");

// Derive `YmacsValue` and `BasicYamcsValue` for `MonotonicTime`.
impl YamcsValue for MonotonicTime {
    fn encode(self) -> EncodedYamcsValue {
        let timestamp = monotonic_time_to_timestamp(&self);

        EncodedYamcsValue(ygw::value::V::TimestampValue(timestamp))
    }

    fn decode(value: EncodedYamcsValue) -> Result<Self, DecodeError> {
        if let ygw::value::V::TimestampValue(timestamp) = value.0 {
            timestamp_to_monotonic_time(&timestamp).ok_or(DecodeError)
        } else {
            Err(DecodeError)
        }
    }
}

impl BasicYamcsValue for MonotonicTime {
    fn ptype() -> &'static str {
        "timestamp"
    }
}

// Derive `YmacsValue` for vectors of values, but not `BasicYamcsValue`.
impl<T: YamcsValue> YamcsValue for Vec<T> {
    fn encode(self) -> EncodedYamcsValue {
        let value: Vec<ygw::Value> = self
            .into_iter()
            .map(|v| ygw::Value {
                v: Some(v.encode().0),
            })
            .collect();

        EncodedYamcsValue(ygw::value::V::ArrayValue(ygw::ArrayValue { value }))
    }

    fn decode(value: EncodedYamcsValue) -> Result<Self, DecodeError> {
        if let ygw::value::V::ArrayValue(ygw::ArrayValue { value }) = value.0 {
            value
                .into_iter()
                .map(|v| {
                    v.v.ok_or(DecodeError)
                        .and_then(|v| <T as YamcsValue>::decode(EncodedYamcsValue(v)))
                })
                .collect()
        } else {
            Err(DecodeError)
        }
    }
}

// Derive `YmacsValue` for arrays, but not `BasicYamcsValue`.
impl<const N: usize, T: YamcsValue> YamcsValue for [T; N] {
    fn encode(self) -> EncodedYamcsValue {
        let value: Vec<ygw::Value> = self
            .into_iter()
            .map(|v| ygw::Value {
                v: Some(v.encode().0),
            })
            .collect();

        EncodedYamcsValue(ygw::value::V::ArrayValue(ygw::ArrayValue { value }))
    }

    fn decode(value: EncodedYamcsValue) -> Result<Self, DecodeError> {
        <Vec<T> as YamcsValue>::decode(value)
            .and_then(|vec| vec.try_into().map_err(|_| DecodeError))
    }
}

/// Converts a `MonotonicTime` to a Yamcs timestamp.
///
/// The conversion is infallible but will if necessary saturate at either end of
/// the range supported by Yamcs timestamps.
pub(crate) fn monotonic_time_to_timestamp(time: &MonotonicTime) -> ygw::Timestamp {
    const NANOS_IN_MILLI: u32 = 1_000_000;
    const PICOS_IN_NANO: u32 = 1_000;
    const MILLIS_IN_SEC: i128 = 1_000;

    let subsec_millis = time.subsec_nanos() / NANOS_IN_MILLI;
    let submilli_nanos = time.subsec_nanos() - subsec_millis * NANOS_IN_MILLI;
    let submilli_picos = submilli_nanos * PICOS_IN_NANO;

    // Compute millis with saturating behavior.
    let millis = time.as_secs() as i128 * MILLIS_IN_SEC + subsec_millis as i128;
    let millis = if millis > i64::MAX as i128 {
        i64::MAX
    } else if millis < i64::MIN as i128 {
        i64::MIN
    } else {
        millis as i64
    };

    ygw::Timestamp {
        millis,
        picos: submilli_picos,
    }
}

/// Converts a Yamcs timestamp to a `MonotonicTime`.
///
/// The conversion will if necessary round down the timestamp to a full
/// nanosecond.
///
/// It may fail if for some reason the number of picoseconds exceeds one
/// millisecond.
pub(crate) fn timestamp_to_monotonic_time(timestamp: &ygw::Timestamp) -> Option<MonotonicTime> {
    const NANOS_IN_MILLI: u32 = 1_000_000;
    const PICOS_IN_NANO: u32 = 1_000;
    const MILLIS_IN_SEC: i64 = 1_000;

    fn floor_div(x: i64, y: i64) -> i64 {
        let is_neg = (x < 0) as i64;

        (x + is_neg) / y - is_neg
    }

    let &ygw::Timestamp { millis, picos } = timestamp;
    let secs = floor_div(millis, MILLIS_IN_SEC);
    let subsec_millis = (millis - secs * MILLIS_IN_SEC) as u32;
    let subsec_nanos = picos / PICOS_IN_NANO + subsec_millis * NANOS_IN_MILLI;

    // `subsec_nanos` might be greater than 1s if for some reason
    // `picos` is greater than 1ms. In such case, return `None`.
    MonotonicTime::new(secs, subsec_nanos)
}

/// Transform an array of (identifier, value) pairs into a Yamcs aggregate
/// value.
///
/// This is a low-level utility. In general, it is easier to map aggregate types
/// by automatically deriving `YamcsValue` on custom `struct` type using the
/// `derive` feature.
pub fn make_aggregate<const N: usize>(
    aggregate: [(&'static str, EncodedYamcsValue); N],
) -> EncodedYamcsValue {
    let mut name = Vec::with_capacity(aggregate.len());
    let mut value = Vec::with_capacity(aggregate.len());

    for (ident, val) in aggregate {
        name.push(ident.to_string());
        value.push(ygw::Value { v: Some(val.0) });
    }

    EncodedYamcsValue(ygw::value::V::AggregateValue(ygw::AggregateValue {
        name,
        value,
    }))
}

/// Attempts to break an aggregate value into the set of individual values that
/// compose it.
///
/// This is a low-level utility. In general, it is easier to map aggregate types
/// by automatically deriving `YamcsValue` on custom `struct` type using the
/// `derive` feature.
pub fn break_aggregate<const N: usize>(
    value: EncodedYamcsValue,
    expected_fields: [&'static str; N],
) -> Result<[EncodedYamcsValue; N], DecodeError> {
    let mut values = {
        match value.0 {
            ygw::value::V::AggregateValue(aggr) => aggr
                .name
                .into_iter()
                .zip(aggr.value)
                .map(|(ident, value)| {
                    value
                        .v
                        .map(|v| (ident, EncodedYamcsValue(v)))
                        .ok_or(DecodeError)
                })
                .collect::<Result<HashMap<String, EncodedYamcsValue>, DecodeError>>()?,
            _ => return Err(DecodeError),
        }
    };

    let mut res = array::from_fn(|_| EncodedYamcsValue(ygw::value::V::BooleanValue(false)));
    for i in 0..N {
        res[i] = values.remove(expected_fields[i]).ok_or(DecodeError)?;
    }

    if !values.is_empty() {
        return Err(DecodeError);
    }

    Ok(res)
}

/// Transform a (variant_name, discriminant) pair into a Yamcs enumerated value.
///
/// This is a low-level utility. In general, it is easier to map enumerated
/// types by automatically deriving `YamcsValue` on custom C-style `enum` type
/// using the `derive` feature.
pub fn make_enumerated(variant_name: &'static str, discriminant: i64) -> EncodedYamcsValue {
    EncodedYamcsValue(ygw::value::V::EnumeratedValue(ygw::EnumeratedValue {
        sint64_value: discriminant,
        string_value: variant_name.into(),
    }))
}

/// Attempts to extract an enumerated value, returning the variant name and the
/// discriminant on success.
///
/// This is a low-level utility. In general, it is easier to map enumerated
/// types by automatically deriving `YamcsValue` on custom `enum` type using the
/// `derive` feature.
pub fn break_enumerated(value: EncodedYamcsValue) -> Result<(String, i64), DecodeError> {
    match value.0 {
        ygw::value::V::EnumeratedValue(ygw::EnumeratedValue {
            sint64_value,
            string_value,
        }) => Ok((string_value, sint64_value)),
        _ => Err(DecodeError),
    }
}
