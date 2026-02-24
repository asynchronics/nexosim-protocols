# Yamcs bridge model helper macros

This crate provides a procedural macro to derive the `YamcsValue` trait for
custom C-like `enum` types and custom `struct` types which members implement
`YamcsValue`.

This crate is not meant to be used directly. Instead, you should activate the
`derive` feature in the `nexosim-yamcs-bridge` crate by adding to your
`Cargo.toml`:

```toml
[dependencies]
nexosim-yamcs-bridge = { version = "0.2.0", features = ["derive"] }
```

The use of this macro is [documented in the `nexosim-yamcs-bridge`][NYG] crate.

[NYG]: https://docs.rs/nexosim-yamcs-bridge
