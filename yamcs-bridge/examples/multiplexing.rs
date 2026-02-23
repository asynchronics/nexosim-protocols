//! This example demonstrates the typical use of the Yamcs bridge.
//!
//! The Yamcs bridge takes advantage of the `map_connect` and
//! `filter_map_connect` functions to route messages to/from several models
//! using only one input port (`to_yamcs`) and one requestor port
//! (`from_yamcs`).
//!
//! Another specificity of the Yamcs bridge is that it is actually built using a
//! "builder" model, called `ProtoYamcsBridge`. This model builder allows other
//! models to register parameters and specify their properties (path, unit,
//! initial value, etc). Registering a parameter returns the routing function(s)
//! to be used for the connections. The builder model also contains the same
//! `from_yamcs` requestor port as `YamcsBridge` so that all replier ports can
//! be already connected before the final Yamcs bridge is built.
//!
//! For parameters that are read-only from the viewpoint of Yamcs, there is only
//! one routing function to route the parameter to the `to_yamcs` port of the
//! Yamcs bridge,
//!
//! For parameters that can be modified by Yamcs, there are 3 routing functions:
//! - one function to route the parameter from the model to the `to_yamcs` port
//!   of the Yamcs bridge,
//! - one function to route the parameter from the `from_yamcs` port to the
//!   relevant model parameter setter,
//! - and finally one function to route back the acknowledgement of the Yamcs
//!   parameter setting request.
//!
//!
//! ```text
//! ┌─────────┐ param_a_out           [set param]►   to_yamcs ┌──────────────┐
//! │         │►──────────────────┬──────────────────────────►│              │
//! │         │ param_b_out       │                           │              │
//! │ Model 1 │►──────────────────┤                           │ Yamcs bridge │
//! │         │ set_param_b       │  ◄[set param]  from_yamcs │              │
//! │         │◄►───────────┬─────│─────────────────────────►◄│              │
//! └─────────┘             │     │   [param ack]►            └──────────────┘
//!                         │     │
//! ┌─────────┐ param_a_out │     │
//! │         │►────────────│─────┤
//! │         │ param_b_out │     │
//! │ Model 2 │►────────────│─────┘
//! │         │ set_param_b │
//! │         │◄►───────────┘
//! └─────────┘
//! ```

use std::time::Duration;

use schematic::ConfigLoader;

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::{EventSource, Output};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime, PeriodicTicker};

use nexosim_yamcs_bridge::{ProtoYamcsBridge, YamcsBridge, YamcsConfig};

/// A very simple model with one parameter (`a`) to which Yamcs has only read
/// access, and one parameter (`b`) which Yamcs can modify.
///
/// Two inputs allow the simulator to increment `a` and `b`, respectively, while
/// a replier port allows Yamcs to request a modification of `b` and send back
/// the value that was actually set (which may differ from the requested value).
#[derive(Serialize, Deserialize)]
struct ExampleModel {
    pub param_a_out: Output<i32>,
    pub param_b_out: Output<f64>,

    param_a: i32,
    param_b: f64,
}

#[Model]
impl ExampleModel {
    /// Initializes example model.
    #[nexosim(init)]
    async fn init(&mut self) {
        self.param_a_out.send(self.param_a).await;
        self.param_b_out.send(self.param_b).await;
    }

    /// Constructs a new model with both parameters set to zero.
    pub fn new(a: i32, b: f64) -> Self {
        Self {
            param_a_out: Output::new(),
            param_b_out: Output::new(),

            param_a: a,
            param_b: b,
        }
    }

    /// Increment parameter `a` by the provided value -- input port.
    pub async fn increment_a(&mut self, delta: i32) {
        self.param_a += delta;
        self.param_a = self.param_a.max(0);
        self.param_a_out.send(self.param_a).await;
    }

    /// Increment parameter `b` by the provided value -- input port.
    pub async fn increment_b(&mut self, delta: f64) {
        self.param_b += delta;
        self.param_b_out.send(self.param_b).await;
    }

    /// Set parameter `b` -- replier port.
    ///
    /// The Yamcs timestamp (if any) is ignored.
    ///
    /// As an illustration of the acknowledgement mechanism, this parameter is
    /// constrained to positive values. Even if a Yamcs user tries to set a
    /// negative value, the corrected value (0) will be displayed instead.
    pub async fn set_param_b(&mut self, value: f64) -> f64 {
        // Saturate to 0 if a negative value is given.
        self.param_b = if value >= 0.0 { value } else { 0.0 };

        // Send back the value that was actually set.
        self.param_b
    }
}

fn main() -> Result<(), SimulationError> {
    let cfg = ConfigLoader::<YamcsConfig>::new().load().unwrap().config;

    // Models and model builders.
    let mut yamcs = ProtoYamcsBridge::new(cfg);
    let mut model1 = ExampleModel::new(100, 1000.0);
    let mut model2 = ExampleModel::new(500, 5000.0);

    // Mailboxes and addresses.
    let yamcs_mbox = Mailbox::new();
    let model1_mbox = Mailbox::new();
    let model2_mbox = Mailbox::new();

    // Register read-only and read-write parameters to be exchanged with Yamcs.
    // The registration methods return opaque connection mapping/filtering
    // functions that either (i) route parameters to the Yamcs with their
    // registered parameter path, or (ii) route parameters from the Yamcs to the
    // model that registered them.
    let route_a_from_model1 = yamcs
        .register_read_only_parameter(
            0i32,
            "model1/param_a",
            "Parameter 'a' in model 1".to_string(),
            None, // No unit
        )
        .unwrap();
    let (route_b_from_model1, route_b_to_model1, route_b_to_model1_ack) = yamcs
        .register_parameter(
            0.0f64,
            "model1/param_b",
            "Parameter 'b' in model 1".to_string(),
            "m/s".to_string(),
        )
        .unwrap();
    let route_a_from_model2 = yamcs
        .register_read_only_parameter(
            0i32,
            "model2/param_a",
            "Parameter 'a' in model 2".to_string(),
            None, // No unit
        )
        .unwrap();
    let (route_b_from_model2, route_b_to_model2, route_b_to_model2_ack) = yamcs
        .register_parameter(
            0.0f64,
            "model2/param_b",
            "Parameter 'b' in model 2".to_string(),
            "m/s".to_string(),
        )
        .unwrap();

    // Connect the models to the Yamcs bridge so that any parameter updated by a
    // model is forwarded to Yamcs.
    model1.param_a_out.map_connect(
        route_a_from_model1,
        YamcsBridge::to_yamcs,
        yamcs_mbox.address(),
    );
    model1.param_b_out.map_connect(
        route_b_from_model1,
        YamcsBridge::to_yamcs,
        yamcs_mbox.address(),
    );
    model2.param_a_out.map_connect(
        route_a_from_model2,
        YamcsBridge::to_yamcs,
        yamcs_mbox.address(),
    );
    model2.param_b_out.map_connect(
        route_b_from_model2,
        YamcsBridge::to_yamcs,
        yamcs_mbox.address(),
    );

    // Connect the Yamcs bridge to the models to allow writable parameters to be
    // updated by the Yamcs. Note that each modification must be acknowledged by
    // the model, which may choose to set a value different from the requested
    // one.
    yamcs.from_yamcs.filter_map_connect(
        route_b_to_model1,
        route_b_to_model1_ack,
        ExampleModel::set_param_b,
        model1_mbox.address(),
    );
    yamcs.from_yamcs.filter_map_connect(
        route_b_to_model2,
        route_b_to_model2_ack,
        ExampleModel::set_param_b,
        model2_mbox.address(),
    );

    // Create the simulation, using a real-time clock..
    let mut bench = SimInit::new();

    let inc_1_a = EventSource::new()
        .connect(ExampleModel::increment_a, &model1_mbox)
        .register(&mut bench);
    let inc_2_a = EventSource::new()
        .connect(ExampleModel::increment_a, &model2_mbox)
        .register(&mut bench);
    let inc_1_b = EventSource::new()
        .connect(ExampleModel::increment_b, &model1_mbox)
        .register(&mut bench);
    let inc_2_b = EventSource::new()
        .connect(ExampleModel::increment_b, &model2_mbox)
        .register(&mut bench);

    let mut sim = bench
        .with_clock(
            AutoSystemClock::new(),
            PeriodicTicker::new(Duration::from_millis(10)),
        )
        .add_model(yamcs, yamcs_mbox, "Yamcs model")
        .add_model(model1, model1_mbox, "Model 1")
        .add_model(model2, model2_mbox, "Model 1")
        .init(MonotonicTime::EPOCH)?;

    let scheduler = sim.scheduler();

    // Increment `model1::a` every 1s.
    scheduler.schedule_periodic_event(
        Duration::from_secs(1),
        Duration::from_secs(1),
        &inc_1_a,
        1,
    )?;

    // Increment `model2::a` every 2s.
    scheduler.schedule_periodic_event(
        Duration::from_secs(2),
        Duration::from_secs(2),
        &inc_2_a,
        1,
    )?;

    // Increment `model1::b` every 5s.
    scheduler.schedule_periodic_event(
        Duration::from_secs(5),
        Duration::from_secs(5),
        &inc_1_b,
        1.0,
    )?;

    // Increment `model2::b` every 10s.
    scheduler.schedule_periodic_event(
        Duration::from_secs(10),
        Duration::from_secs(10),
        &inc_2_b,
        1.0,
    )?;

    // Run the simulation for 5 min.
    sim.step_until(Duration::from_secs(300))?;
    Ok(())
}
