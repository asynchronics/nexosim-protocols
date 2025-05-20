# NeXosim CAN port model

This crate contains CAN port model for [NeXosim][NX]-based simulations.

[NX]: https://github.com/asynchronics/nexosim

## Model overview

This model
* listens the specified CAN ports injecting data from it into the
  simulation,
* outputs data from the simulation to the specified CAN ports.

**Note: data sent by the CAN port is injected back into the simulation.**

```text
            ┌───────────┐
  frame_in  │  CAN      │ frame_out
●──────────►│  port     ├───────────►
            │           │
            └───────────┘
```

## Documentation

The API documentation is relatively exhaustive and includes a practical
overview which should provide all necessary information to get started.

An example of usage can be found in `examples` directory.

See also [NeXosim documentation][NXAPI].

[NXAPI]: https://docs.rs/nexosim

## Usage

To use the latest version, add to your `Cargo.toml`:

```toml
[dependencies]
nexosim-can-port = "0.1.0"
```

## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.


## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
