# NeXosim serial port model

This crate contains serial port model for [NeXosim][NX]-based simulations.

[NX]: https://github.com/asynchronics/nexosim

## Model overview

This model
 * listens the specified serial ports injecting data from it into the
   simulation,
 * outputs data from the simulation to the specified serial port.

## Ports

```text
            ┌───────────┐
  bytes_in  │  Serial   │ bytes_out
●──────────►│  port     ├───────────►
            │           │
            └───────────┘
```

### Input ports

| Name                | Event type   | Description                            |
|---------------------|--------------|----------------------------------------|
| `bytes_in`          | `Bytes`      | Bytes to be written to the serial port |

### Ouput ports

| Name                | Ouput type         | Description                     |
|---------------------|--------------------|---------------------------------|
| `bytes_out`         | `Bytes`            | Bytes read from the serial port |

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
nexosim-serial-port = "0.1.0"
```

## Configuration

The `SerialPort` model is configurable using the [`schematic`][schematic] crate.

[schematic]: https://moonrepo.github.io/schematic/

An example of instantiation of a new model follows:

```rust
use schematic::{ConfigLoader, Format};

use nexosim_serial_port::{ProtoSerialPort, SerialPort, SerialPortConfig};

/// Serial port path.
const PORT_PATH: &str = "/tmp/ttyS21";

/// Activation period, in milliseconds, for cyclic activities inside the simulation.
const PERIOD: u64 = 10;

/// Time shift, in milliseconds, for scheduling events at the present moment.
const DELTA: u64 = 5;

let mut loader = ConfigLoader::<SerialPortConfig>::new();
loader
    .code(format!("portPath = \"{}\"", PORT_PATH), Format::Toml)
    .unwrap();
loader
    .code(format!("delta = {}", DELTA), Format::Toml)
    .unwrap();
loader
    .code(format!("period = {}", PERIOD), Format::Toml)
    .unwrap();
let cfg = loader.load().unwrap().config;

let serial = ProtoSerialPort::new(cfg);
```

## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.


## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
