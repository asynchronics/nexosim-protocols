# NeXosim byte protocol utilities

This crate contains byte and stream related utilities and models useful in
protocol implementation for [NeXosim][NX]-based simulations.

[NX]: https://github.com/asynchronics/nexosim

## Documentation

The API documentation is relatively exhaustive and includes a practical
overview which should provide all necessary information to get started.

An example of usage can be found in [serial port model][SPM].

[SPM]: https://github.com/asynchronics/nexosim-protocols/blob/main/serial-port/src/lib.rs

See also [NeXosim documentation][NXAPI].

[NXAPI]: https://docs.rs/nexosim

## Usage

To use the latest version, add to your `Cargo.toml`:

```toml
[dependencies]
nexosim-byte-utils = { git = "https://github.com/asynchronics/nexosim-protocols.git" }
```

## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.


## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
