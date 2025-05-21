# NeXosim CAN port model

This crate contains CAN port model for [NeXosim][NX]-based simulations.

[NX]: https://github.com/asynchronics/nexosim

## Model overview

This model
* listens the specified CAN ports injecting data from it into the
  simulation,
* outputs data from the simulation to the specified CAN ports.

**Note: data sent by the CAN port is injected back into the simulation.**

## Ports

```text
            ┌───────────┐
  frame_in  │  CAN      │ frame_out
●──────────►│  port     ├───────────►
            │           │
            └───────────┘
```
### Input ports

| Name                | Event type   | Description                             |
|---------------------|--------------|-----------------------------------------|
| `frame_in`          | `CanData`    | CAN frame to be written to the CAN port |

### Ouput ports

| Name                | Ouput type         | Description                      |
|---------------------|--------------------|----------------------------------|
| `frame_out`         | `Bytes`            | CAN frame read from the CAN port |

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

## Configuration

The `CanPort` model is configurable using the [`schematic`][schematic] crate.

[schematic]: https://moonrepo.github.io/schematic/

An example of instantiation of a new model follows:

```rust
use schematic::{ConfigLoader, Format};

use nexosim_can_port::{ProtoCanPort, CanPort, CanPortConfig};

/// CAN interfaces.
const CAN_INTERFACES: &[&str] = &["vcan0", "vcan1"];

/// Activation period, in milliseconds, for cyclic activities inside the simulation.
const PERIOD: u64 = 10;

/// Time shift, in milliseconds, for scheduling events at the present moment.
const DELTA: u64 = 5;

let mut loader = ConfigLoader::<CanPortConfig>::new();
loader
    .code(format!("interfaces = {:?}", CAN_INTERFACES), Format::Toml)
    .unwrap();
loader
    .code(format!("delta = {}", DELTA), Format::Toml)
    .unwrap();
loader
    .code(format!("period = {}", PERIOD), Format::Toml)
    .unwrap();
let cfg = loader.load().unwrap().config;

let serial = ProtoCanPort::new(cfg);
```

## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.


## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
