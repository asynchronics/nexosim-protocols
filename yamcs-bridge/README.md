# Yamcs bridge model

`YamcsBridge` is a model [NeXosim][NX]-based simulations that enables the
exchange of parameters between [Yamcs][YAMCS] instances and a simulation.

The simulation server implementation is self-contained and does not require any
dependency to be manually installed. However, because it relies on the Yamcs
Gateway protocol, the [Yamcs Gateway][YGW] plugin must be installed in Yamcs to
connect to the bridge.

The server automatically starts when the model is instantiated and shuts down
when the simulation completes. The simulation acts as a single Yamcs Gateway
node.

The server runs on port 7897 by default but can be [configured](#configuration)
to run on a different port.

[NX]: https://github.com/asynchronics/nexosim
[YAMCS]: https://yamcs.org
[YGW]: https://github.com/xpromache/yamcs-gateway

## Ports

```text
          to_yamcs  ┌─────────────┐
───────────────────►│             │
                    │             │  from_yamcs
                    │ YamcsBridge │►◄───────────
 update_from_yamcs  │             │
───────────────────►│             │
                    └─────────────┘
```

### Input ports

| Name       | Event type   | Description                      |
| ---------- | ------------ | -------------------------------- |
| `to_yamcs` | `MsgToYamcs` | A parameter update sent to Yamcs |

### Requestor ports

| Name         | Request type   | Reply type   | Description                           |
| ------------ | -------------- | ------------ | ------------------------------------- |
| `from_yamcs` | `MsgFromYamcs` | `MsgToYamcs` | A parameter update request from Yamcs |

## Configuration

`YamcsBridge` uses the [`schematic`][schematic] crate for configuration.

At the moment, the only configurable parameter is the port on which the server
runs.

[schematic]: https://moonrepo.github.io/schematic/
[toml_config]: config_templates/yamcs_config.toml

## Supported Yamcs parameters

The bridge allows other models to send and receive parameters to/from Yamcs.

### Basic parameters

All basic types supported by the Yamcs Gateway protocol and predefined in the
YGW mission database are supported and mapped to Rust types. Specifically, the
following Rust types implement both the `YamcsValue` and the `BasicYamcsValue`
trait:

| Rust type                | YGW type name | Yamcs data type             |
| ------------------------ | ------------- | --------------------------- |
| `bool`                   | `boolean`     | `BooleanParameterType`      |
| `i32`                    | `sint32`      | `IntegerParameterType`      |
| `u32`                    | `uint32`      | `IntegerParameterType`      |
| `i64`                    | `sint64`      | `IntegerParameterType`      |
| `u64`                    | `uint64`      | `IntegerParameterType`      |
| `f32`                    | `float`       | `FloatParameterType`        |
| `f64`                    | `double`      | `FloatParameterType`        |
| `String`                 | `string`      | `StringParameterType`       |
| `bytes::Bytes`           | `binary`      | `BinaryParameterType`       |
| `nexosim::MonotonicTime` | `timestamp`   | `AbsoluteTimeParameterType` |

**Notes:**

- The `Vec<u8>` type is _not_ mapped to the Yamcs Gateway `binary` type because
  this would conflict with the [automatic implementation](#custom-parameters) of
  `YamcsValue` for `Vec<T>`, which maps `Vec<T>` to arrays.
- Timestamp mapping is not one-to-one: compared to the Yamcs Gateway
  `timestamp` type, the `nexosim::MonotonicTime` covers a wider time span
  but is restricted to nanosecond precision.

### Custom parameters

Additionally, most user-defined Yamcs data types can be mapped to Rust types
that implement the `YamcsValue` trait, which includes:

| Rust type                      | Yamcs data type           | Requires `#derive[YamcsValue]` | Comment                                   |
| ------------------------------ | ------------------------- | ------------------------------ | ----------------------------------------- |
| `i8`                           | `IntegerParameterType`    | No                             | Mapped to `i32` at the YGW protocol level |
| `u8`                           | `IntegerParameterType`    | No                             | Mapped to `u32` at the YGW protocol level |
| `i16`                          | `IntegerParameterType`    | No                             | Mapped to `i32` at the YGW protocol level |
| `u16`                          | `IntegerParameterType`    | No                             | Mapped to `u32` at the YGW protocol level |
| `[T; N]`                       | `ArrayParameterType`      | No                             |                                           |
| `Vec<T>`                       | `ArrayParameterType`      | No                             |                                           |
| `struct MyStruct;`             | `AggregateParameterType`  | Yes                            | Empty aggregate                           |
| `struct MyStruct { ... }`      | `AggregateParameterType`  | Yes                            |                                           |
| `struct MyStruct(T0, T1, ...)` | `AggregateParameterType`  | Yes                            | Fields are named `_0`, `_1`, ...          |
| `struct MyStruct(T)`           | same as `T`               | Yes                            | Treated as transparent                    |
| `enum`                         | `EnumeratedParameterType` | Yes                            | Only C-like enums are supported           |

Custom parameters must be explicitly defined in the mission database, with a
type consistent with its Rust counterpart. In order to link each custom Rust
parameter to its database definition, [registration
methods](#parameter-registration) for custom parameters take the Yamcs type as
additional argument.

The Rust `Vec<T>` and `[T; N]` types automatically implement the
`YamcsValue` trait for any element type `T` that implements `YamcsValue`.

The derive macro defined in the `yamcs-derive` crate can be used to implement
`YamcsValue` for C-like `enum` types and for `struct` types, as long as they
only contain types that themselves implementing `YamcsValue`. Rather than use
directly the `yamcs-derive` crate, the derive macro can be imported by
activating the `derive` feature in the `yamcs-model` dependency:

```toml
[dependencies]
nexosim-yamcs-bridge = { version = "0.2.0", features = ["derive"] }
```

The derive macro can then be used as follows:

```rust
use nexosim_yamcs_bridge::YamcsValue;

#[derive(Clone, YamcsValue)]
struct MyCustomParam {
    some_field: String,
    some_other_field: Vec<i64>,
}
```

## Parameter registration

### Overview

Parameters must be registered before the simulation starts, using one of the
`ProtoYamcsBridge::register_*parameter` methods. These methods return routing
information that needs to be used when connecting models to the Yamcs bridge.

The `ProtoYamcsBridge::register_parameter` and
`ProtoYamcsBridge::register_read_only_parameter` methods can only be used with
[basic Yamcs Gateway type](#basic-parameters), meaning Rust types which
implement the `BasicYamcsValue` trait. Because these types are known to Yamcs,
they do not need to be explicitly defined in the mission database.

The `ProtoYamcsBridge::register_custom_parameter` and
`ProtoYamcsBridge::register_custom_read_only_parameter` methods can be in turn
used for [custom types](#custom-parameters) that only implement the `YamcsValue`
trait. These methods take an additional `ptype` argument which must correspond
to the path to a type defined in the database. In general, this path should be
fully qualified and start at the relevant space system root, e.g.
`/my_space_system/my_ptype`.

### Read-only parameters

`ProtoYamcsBridge::register_read_only_parameter` and
`ProtoYamcsBridge::register_custom_read_only_parameter` return a routing
function. This function is expected as argument to `Output::map_connect` to
route the parameter from the model to the `to_yamcs` port of the Yamcs bridge.

Let us consider for illustration a model with a single output using a custom,
read-only parameter, connected to the Yamcs bridge:

```text
┌─────────┐                         ┌─────────────┐
│         │ param_out      to_yamcs │             │
│ MyModel │►───────────────────────►│ YamcsBridge │
│         │                         │             │
└─────────┘                         └─────────────┘
```

This bench could be implemented as follows:

```rust
use schematic::ConfigLoader;

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::Output;
use nexosim::simulation::Mailbox;

use nexosim_yamcs_bridge::{ProtoYamcsBridge, YamcsBridge, YamcsConfig, YamcsValue};

#[derive(Clone, Default, YamcsValue)]
struct MyParam {
    foo: i32,
    bar: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct MyModel {
    pub param_out: Output<MyParam>,
    // ...
}
#[Model]
impl MyModel {
    // ...
}

// Load default configuration (server at port 7897).
let cfg = ConfigLoader::<YamcsConfig>::new().load().unwrap().config;

// Register our parameter.
let mut yamcs = ProtoYamcsBridge::new(cfg);
let mut model = MyModel::default();

let route_from_model = yamcs
    .register_custom_read_only_parameter(
        // the initial value (mostly irrelevant if propagated in the `Model::init` method):
        MyParam::default(),
        // the relative path in Yamcs under the `/ygw/` namespace:
        "model/param",
        // a short, optional description:
        "A parameter in my model".to_string(),
        // an optional physical unit:
        None,
        // the corresponding type in the mission database:
        "/my_space_system/my_param_t"
    )
    .unwrap();

// Connect our model.
let yamcs_mbox = Mailbox::new();

model.param_out.map_connect(
    route_from_model,
    YamcsBridge::to_yamcs,
    yamcs_mbox.address(),
);
```

Note that all parameter updates sent to Yamcs are automatically tagged with
the current simulation time by the Yamcs bridge.

Beware that the above example implicitly assumes that a custom parameter
`/my_space_system/my_param_t` with a layout compatible with `MyParam` was
defined in the database. This could be done for instance by adding a file
`my_space_system.dt` under the `mdb` folder with the following content:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<SpaceSystem xmlns="http://www.omg.org/spec/XTCE/20180204" name="my_space_system">
    <TelemetryMetaData>
        <ParameterTypeSet>
            <IntegerParameterType name="i32_t" sizeInBits="32" signed="true" />
            <FloatParameterType name="f64_t" sizeInBits="64" />
            <AggregateParameterType name="my_param_t_">
                <MemberList>
                    <Member name="foo" typeRef="i32_t" />
                    <Member name="bar" typeRef="f64_t" />
                </MemberList>
            </AggregateParameterType>
        </ParameterTypeSet>
    </TelemetryMetaData>
</SpaceSystem>
```

This file needs to be identified under the `mdb` option in the appropriate
`etc/yamcs.[instance].yaml` configuration file.

### Read-write parameters

`ProtoYamcsBridge::register_parameter` and
`ProtoYamcsBridge::register_custom_parameter` return 3 routing functions:

1. one function to be used as argument to `Output::map_connect` to route the
   parameter from the model to the `to_yamcs` port of the Yamcs bridge,
2. one function to be used as the first argument to
   `Requestor::filter_map_connect` to route the parameter from the `from_yamcs`
   requestor port to the relevant model parameter setter,
3. one function to be used as the second argument to
   `Requestor::filter_map_connect` to route back the acknowledgement of the
   Yamcs parameter setting request.

Let us consider a model with a single read-write parameter, connected to the
Yamcs bridge:

```text
┌─────────┐ param_out        [set param]►       to_yamcs ┌─────────────┐
│         │►────────────────────────────────────────────►│             │
│         │                                              │             │
│ MyModel │                                              │ YamcsBridge │
│         │ param_in        ◄[set param]      from_yamcs │             │
│         │◄►──────────────────────────────────────────►◄│             │
└─────────┘                  [param ack]►                └─────────────┘
```

Note that there are now two connections: one for sending parameter modifications
to Yamcs and one to receive parameter modification requests from Yamcs.

This bench could be implemented as follows:

```rust
use schematic::ConfigLoader;
use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::Output;
use nexosim::simulation::Mailbox;

use nexosim_yamcs_bridge::{ProtoYamcsBridge, YamcsBridge, YamcsConfig};

#[derive(Default, Serialize, Deserialize)]
struct MyModel {
    pub param_out: Output<f64>,
    // ...
}
#[Model]
impl MyModel {
    // Validate and execute a parameter update request from Yamcs.
    pub async fn param_in(&mut self, param: f64) -> f64 {
        // In this example, we choose to saturate negative values to 0.0.
        let param = param.max(0.0);

        // ... do something with the parameter ...

        // Return the post-validation value.
        param
    }
}

let cfg = ConfigLoader::<YamcsConfig>::new().load().unwrap().config;

// Register our parameter.
let mut yamcs = ProtoYamcsBridge::new(cfg);
let mut model = MyModel::default();

let (route_from_model, route_to_model, route_to_model_ack) = yamcs
    .register_parameter(
        0.0f64,
        "model/param",
        "A parameter in my model".to_string(),
        "m/s".to_string(),
    ).unwrap();

// Connect our model.
let yamcs_mbox = Mailbox::new();
let model_mbox = Mailbox::new();

model.param_out.map_connect(
    route_from_model,
    YamcsBridge::to_yamcs,
    yamcs_mbox.address(),
);
yamcs.from_yamcs.filter_map_connect(
    route_to_model,
    route_to_model_ack,
    MyModel::param_in,
    model_mbox.address(),
);
```

As earlier, simulation timestamps are automatically appended to any parameter
sent to Yamcs, including when acknowledging a parameter modification request.

## Updates from Yamcs

Parameter updates from Yamcs are serviced by the model at each simulation tick.
It is therefore necessary for the simulation to run with a
`nexosim::time::Ticker` to ensure that parameter updates are processed
regularly.

## Examples

### Multiplexing

A complete example bench with a simulation script is provided under
`examples/multiplexing.rs` for the below configuration. This example illustrates
the use of routing functions to multiplex several read-only and read-write
parameter exchanges between Yamcs and an arbitrary number of models.

```text
┌─────────┐ param_a_out           [set param]►   to_yamcs ┌──────────────┐
│         │►──────────────────┬──────────────────────────►│              │
│         │ param_b_out       │                           │              │
│ Model 1 │►──────────────────┤                           │ Yamcs bridge │
│         │ set_param_b       │  ◄[set param]  from_yamcs │              │
│         │◄►───────────┬─────│─────────────────────────►◄│              │
└─────────┘             │     │   [param ack]►            └──────────────┘
                        │     │
┌─────────┐ param_a_out │     │
│         │►────────────│─────┤
│         │ param_b_out │     │
│ Model 2 │►────────────│─────┘
│         │ set_param_b │
│         │◄►───────────┘
└─────────┘
```
