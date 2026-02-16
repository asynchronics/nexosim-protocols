#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{
    Data, DataEnum, DeriveInput, Field, Fields, parse_macro_input, punctuated::Punctuated,
    token::Comma,
};

/// Derive YamcsValue for struct with named fields.
fn derive_struct_named(ident: Ident, fields: Punctuated<Field, Comma>) -> TokenStream {
    let fields: Vec<_> = fields
        .into_iter()
        .map(|field| field.ident.unwrap())
        .collect();

    quote!(
        impl nexosim_yamcs_bridge::YamcsValue for #ident {
            fn encode(self) -> nexosim_yamcs_bridge::EncodedYamcsValue {
                let aggr = [ #((stringify!(#fields), self.#fields.encode())),* ];

                nexosim_yamcs_bridge::make_aggregate(aggr)
            }

            fn decode(value: nexosim_yamcs_bridge::EncodedYamcsValue) -> Result<Self, nexosim_yamcs_bridge::DecodeError> {
                let field_idents = [ #(stringify!(#fields)),* ];
                let mut encoded_values = nexosim_yamcs_bridge::break_aggregate(value, field_idents)?.into_iter();

                Ok(
                    Self {
                        #(#fields: <_ as nexosim_yamcs_bridge::YamcsValue>::decode(encoded_values.next().unwrap())?),*
                    }
                )
            }
        }
    ).into()
}

/// Derive YamcsValue for struct with unnamed fields.
fn derive_struct_unnamed(ident: Ident, fields_num: usize) -> TokenStream {
    let mut fields = Vec::with_capacity(fields_num);

    for i in 0..fields_num {
        fields.push(Ident::new(format!("_{i}").as_str(), Span::call_site()));
    }

    quote!(
        impl nexosim_yamcs_bridge::YamcsValue for #ident {
            fn encode(self) -> nexosim_yamcs_bridge::EncodedYamcsValue {
                let #ident(#(#fields),*) = self;
                let aggr = [ #((stringify!(#fields), #fields.encode())),* ];

                nexosim_yamcs_bridge::make_aggregate(aggr)
            }

            fn decode(value: nexosim_yamcs_bridge::EncodedYamcsValue) -> Result<Self, nexosim_yamcs_bridge::DecodeError> {
                let field_idents = [ #(stringify!(#fields)),* ];
                let mut encoded_values = nexosim_yamcs_bridge::break_aggregate(value, field_idents)?.into_iter();

                #(let #fields = <_ as nexosim_yamcs_bridge::YamcsValue>::decode(encoded_values.next().unwrap())?;)*
                Ok(
                    Self (
                        #(#fields),*
                    ))
            }
        }
    ).into()
}

/// Derive YamcsValue for struct with one unnamed field.
fn derive_struct_wrapper(ident: Ident) -> TokenStream {
    quote!(
        impl nexosim_yamcs_bridge::YamcsValue for #ident {
            fn encode(self) -> nexosim_yamcs_bridge::EncodedYamcsValue {
                let #ident(field) = self;
                field.encode()
            }

            fn decode(value: nexosim_yamcs_bridge::EncodedYamcsValue) -> Result<Self, nexosim_yamcs_bridge::DecodeError> {
                let field = <_ as nexosim_yamcs_bridge::YamcsValue>::decode(value)?;
                Ok(
                    Self (
                        field
                    ))
            }
        }
    ).into()
}

/// Derive YamcsValue for unit struct.
fn derive_struct_unit(ident: Ident) -> TokenStream {
    quote!(
        impl nexosim_yamcs_bridge::YamcsValue for #ident {
            fn encode(self) -> nexosim_yamcs_bridge::EncodedYamcsValue {
                nexosim_yamcs_bridge::make_aggregate([])
            }

            fn decode(value: nexosim_yamcs_bridge::EncodedYamcsValue) -> Result<Self, nexosim_yamcs_bridge::DecodeError> {
                Ok(Self)
            }
        }
    ).into()
}

/// Derive YamcsValue for enum with simple variants.
fn derive_enum(ident: Ident, data: DataEnum) -> TokenStream {
    let variants: Vec<_> = data
        .variants
        .into_iter()
        .map(|variant| {
            if !matches!(variant.fields, Fields::Unit) {
                panic!(
                    "Only units with simple variants are suported (variant {} is not simple).",
                    variant.ident
                );
            }
            variant.ident
        })
        .collect();
    let consts: Vec<_> = variants
        .iter()
        .map(|variant| {
            Ident::new(
                variant.to_string().to_uppercase().as_str(),
                Span::call_site(),
            )
        })
        .collect();
    quote!(
        impl nexosim_yamcs_bridge::YamcsValue for #ident {
            fn encode(self) -> nexosim_yamcs_bridge::EncodedYamcsValue {
                match self {
                    #(Self::#variants => nexosim_yamcs_bridge::make_enumerated(stringify!(#variants), Self::#variants as i64)),*
                }
            }

            fn decode(value: nexosim_yamcs_bridge::EncodedYamcsValue) -> Result<Self, nexosim_yamcs_bridge::DecodeError> {
                #(const #consts: i64 = #ident::#variants as i64;)*
                let (name, discriminant) = nexosim_yamcs_bridge::break_enumerated(value)?;
                match (name.as_str(), discriminant) {
                    #((stringify!(#variants), #consts) => Ok(Self::#variants),)*
                    (_, _) => Err(nexosim_yamcs_bridge::DecodeError),
                }
            }
        }
    ).into()
}

/// Derive YamcsValue.
#[proc_macro_derive(YamcsValue)]
pub fn yamcs_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let ident = input.ident;

    match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => derive_struct_named(ident, fields.named),
            Fields::Unnamed(fields) => {
                let len = fields.unnamed.len();
                if len != 1 {
                    derive_struct_unnamed(ident, len)
                } else {
                    derive_struct_wrapper(ident)
                }
            }
            Fields::Unit => derive_struct_unit(ident),
        },
        Data::Enum(data) => derive_enum(ident, data),
        Data::Union(_) => panic!("Unions are not supported."),
    }
}
