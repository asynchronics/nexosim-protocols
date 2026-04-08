#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![allow(deprecated)]

mod codegen;
mod server;
mod yamcs_value;

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use schematic::Config;

use tokio::sync::mpsc;

use nexosim::model::{BuildContext, Context, Model, ProtoModel, schedulable};
use nexosim::ports::Requestor;
use nexosim::time::MonotonicTime;

#[cfg(feature = "derive")]
pub use nexosim_yamcs_derive::YamcsValue;

use codegen::ygw;
pub use yamcs_value::{
    BasicYamcsValue, DecodeError, EncodedYamcsValue, YamcsValue, break_aggregate, break_enumerated,
    make_aggregate, make_enumerated,
};

/// A parameter update sent to Yamcs.
///
/// This is an opaque type  meant to be consumed by routing functions.
#[derive(Debug)]
pub struct MsgToYamcs {
    value: EncodedYamcsValue,
    id: u32,
}

/// A parameter update from Yamcs.
///
/// This is an opaque type  meant to be consumed by routing functions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsgFromYamcs {
    value: EncodedYamcsValue,
    id: u32,
}

/// A timestamped parameter update sent to Yamcs.
#[derive(Debug)]
pub(crate) struct StampedMsgToYamcs {
    value: EncodedYamcsValue,
    id: u32,
    timestamp: MonotonicTime,
}

/// Configuration for `ProtoYamcsBridge`.
#[derive(Config, Debug)]
pub struct YamcsConfig {
    /// Server port.
    #[setting(default = 7897)]
    pub port: u16,
}

/// A builder type for the [`YamcsBridge`] model.
///
/// This model builder allows other models to register Yamcs parameters and
/// specify their properties (path, unit, initial value, mutability from Yamcs,
/// etc). Registering a parameter returns the routing function(s) to be used for
/// connections.
///
/// Note that `ProtoYamcsBridge` contains the same `from_yamcs` requestor port
/// as `YamcsBridge`, which makes it possible to connect all replier ports
/// before the final Yamcs bridge is built.
#[derive(Debug, Default)]
pub struct ProtoYamcsBridge {
    /// A requestor port forwarding parameter modification requests from Yamcs.
    ///
    /// The request should be acknowledged by replying with the value that was
    /// actually set, which may be different from the requested value.
    pub from_yamcs: Requestor<MsgFromYamcs, MsgToYamcs>,

    param_values: Vec<EncodedYamcsValue>,
    param_definitions: Vec<ygw::ParameterDefinition>,
    param_paths: HashSet<String>,
    config: YamcsConfig,
}

impl ProtoYamcsBridge {
    /// Constructs a new builder for a [`YamcsBridge`] model,
    ///
    /// The server will use the default Yamcs Gateway port (7897);
    pub fn new(config: YamcsConfig) -> Self {
        Self {
            from_yamcs: Requestor::new(),
            param_values: Vec::new(),
            param_definitions: Vec::new(),
            param_paths: HashSet::new(),
            config,
        }
    }

    /// Registers a parameter that may be modified by Yamcs.
    ///
    /// The initial value needs to be specified, but should be hopefully
    /// irrelevant since models connected to the Yamcs bridge are encouraged to
    /// send their actual initial parameter values when `Model::init` is invoked
    /// at the beginning of the simulation.
    ///
    /// The path argument is a user-specified slash-separated path that
    /// specifies the parameter name and its scope in Yamcs, e.g.
    /// `my_model/my_function/param_a`.
    ///
    /// The description argument is an optional, user-friendly explanation of
    /// the parameter's purpose, displayed in the Yamcs interface.
    ///
    /// The unit argument is an optional string representing the physical unit
    /// of the parameter, displayed in the Yamcs interface.
    ///
    /// An error is returned if a parameter with the same path was already
    /// registered.
    ///
    /// # Routing functions
    ///
    /// The Yamcs bridge takes advantage of the `Output::map_connect` and
    /// `Requestor::filter_map_connect` methods to route messages from/to
    /// several models using only one input port (`to_yamcs`) and one requestor
    /// port (`from_yamcs`).
    ///
    /// This method accordingly returns 3 routing functions:
    /// - one function to be used as argument of `Output::map_connect` to route
    ///   the parameter from the model to the
    ///   [`to_yamcs`](YamcsBridge::to_yamcs) port of the Yamcs bridge,
    /// - one function to be used as argument of `Requestor::filter_map_connect`
    ///   to route the parameter from the [`from_yamcs`](Self::from_yamcs)
    ///   requestor port to the relevant model parameter setter,
    /// - another function to be used as argument of
    ///   `Requestor::filter_map_connect` to route back the acknowledgement of
    ///   the Yamcs parameter setting request.
    ///
    /// See also: [`register_custom_parameter`](Self::register_custom_parameter),
    /// [`register_read_only_parameter`](Self::register_read_only_parameter) and
    /// [`register_custom_read_only_parameter`](Self::register_custom_read_only_parameter).
    #[allow(clippy::type_complexity)]
    pub fn register_parameter<T: BasicYamcsValue + Clone>(
        &mut self,
        init_value: T,
        path: impl Into<String>,
        description: impl Into<Option<String>>,
        unit: impl Into<Option<String>>,
    ) -> Result<
        (
            impl Fn(&T) -> MsgToYamcs + 'static,
            impl Fn(&MsgFromYamcs) -> Option<T> + 'static,
            impl Fn(T) -> MsgToYamcs + 'static,
        ),
        RegistrationError,
    > {
        let ptype = <T as BasicYamcsValue>::ptype().to_owned();

        self.register_custom_parameter(init_value, path, description, unit, ptype)
    }

    /// Registers a parameter with a user-defined type that may be modified by
    /// Yamcs.
    ///
    /// The only difference with
    /// [`register_parameter`](Self::register_parameter) is that the type needs
    /// not be one of the basic types known to Yamcs, but may be any type that
    /// derives [`YamcsValue`]. In this case, the path to the corresponding type
    /// in the mission database must be supplied as the `ptype` argument.
    ///
    /// See also: [`register_parameter`](Self::register_parameter),
    /// [`register_read_only_parameter`](Self::register_read_only_parameter) and
    /// [`register_custom_read_only_parameter`](Self::register_custom_read_only_parameter).
    #[allow(clippy::type_complexity)]
    pub fn register_custom_parameter<T: YamcsValue + Clone>(
        &mut self,
        init_value: T,
        path: impl Into<String>,
        description: impl Into<Option<String>>,
        unit: impl Into<Option<String>>,
        ptype: impl Into<String>,
    ) -> Result<
        (
            impl Fn(&T) -> MsgToYamcs + 'static,
            impl Fn(&MsgFromYamcs) -> Option<T> + 'static,
            impl Fn(T) -> MsgToYamcs + 'static,
        ),
        RegistrationError,
    > {
        let path: String = path.into();

        if !self.param_paths.insert(path.clone()) {
            return Err(RegistrationError);
        }

        let id: u32 = self
            .param_definitions
            .len()
            .try_into()
            .expect("Too many Yamcs parameters have been registered");

        let definition = ygw::ParameterDefinition {
            relative_name: path,
            description: description.into(),
            unit: unit.into(),
            ptype: ptype.into(),
            writable: Some(true),
            id,
        };

        self.param_definitions.push(definition);
        self.param_values.push(init_value.encode());

        Ok((
            move |v: &T| MsgToYamcs {
                value: v.clone().encode(),
                id,
            },
            move |msg: &MsgFromYamcs| {
                if msg.id == id {
                    // If the type is not the expected one, just ignore.
                    if let Ok(v) = <T as YamcsValue>::decode(msg.value.clone()) {
                        return Some(v);
                    }
                }

                None
            },
            move |v: T| MsgToYamcs {
                value: v.encode(),
                id,
            },
        ))
    }

    /// Registers a parameter that may not be modified by Yamcs.
    ///
    /// This is similar to [`register_parameter`](Self::register_parameter) but
    /// without support for parameter modification by Yamcs. It accordingly
    /// returns a single routing function to route the parameter from the model
    /// to the Yamcs bridge.
    ///
    /// See also: [`register_parameter`](Self::register_parameter),
    /// [`register_custom_parameter`](Self::register_custom_parameter) and
    /// [`register_custom_read_only_parameter`](Self::register_custom_read_only_parameter).
    pub fn register_read_only_parameter<T: BasicYamcsValue + Clone>(
        &mut self,
        init_value: T,
        path: impl Into<String>,
        description: impl Into<Option<String>>,
        unit: impl Into<Option<String>>,
    ) -> Result<impl Fn(&T) -> MsgToYamcs + 'static, RegistrationError> {
        let ptype = <T as BasicYamcsValue>::ptype().to_owned();

        self.register_custom_read_only_parameter(init_value, path, description, unit, ptype)
    }

    /// Registers a parameter with a user-defined type that may not be modified
    /// by Yamcs.
    ///
    /// This is similar to
    /// [`register_custom_parameter`](Self::register_custom_parameter) but
    /// without support for parameter modification by Yamcs. It accordingly
    /// returns a single routing function to route the parameter from the model
    /// to the Yamcs bridge.
    ///
    /// See also: [`register_parameter`](Self::register_parameter),
    /// [`register_custom_parameter`](Self::register_custom_parameter) and
    /// [`register_read_only_parameter`](Self::register_read_only_parameter).
    pub fn register_custom_read_only_parameter<T: YamcsValue + Clone>(
        &mut self,
        init_value: T,
        path: impl Into<String>,
        description: impl Into<Option<String>>,
        unit: impl Into<Option<String>>,
        ptype: impl Into<String>,
    ) -> Result<impl Fn(&T) -> MsgToYamcs + 'static, RegistrationError> {
        let path: String = path.into();

        if !self.param_paths.insert(path.clone()) {
            return Err(RegistrationError);
        }

        let id: u32 = self
            .param_definitions
            .len()
            .try_into()
            .expect("Too many Yamcs parameters have been registered");

        let definition = ygw::ParameterDefinition {
            relative_name: path,
            description: description.into(),
            unit: unit.into(),
            ptype: ptype.into(),
            writable: Some(false),
            id,
        };

        self.param_definitions.push(definition);
        self.param_values.push(init_value.encode());

        Ok(move |v: &T| MsgToYamcs {
            value: v.clone().encode(),
            id,
        })
    }
}

impl ProtoModel for ProtoYamcsBridge {
    type Model = YamcsBridge;

    /// Builds the final [`YamcsBridge`] model and start the Yamcs gateway server.
    ///
    /// The [`from_yamcs`](Self::from_yamcs) requestor port is moved from this
    /// builder to the model.
    ///
    /// # Panics
    ///
    /// Panics if the Yamcs gateway server could not be started.
    fn build(self, cx: &mut BuildContext<Self>) -> (YamcsBridge, YamcsBridgeEnv) {
        let (tx, gateway_rx) = mpsc::unbounded_channel();

        server::start(
            self.param_definitions,
            self.param_values,
            cx.injector(),
            *schedulable!(YamcsBridge::msg_from_yamcs),
            gateway_rx,
            self.config.port,
        )
        .unwrap();

        (
            YamcsBridge {
                from_yamcs: self.from_yamcs,
            },
            YamcsBridgeEnv { tx },
        )
    }
}

/// Yamcs bridge model environment.
#[derive(Debug)]
pub struct YamcsBridgeEnv {
    tx: mpsc::UnboundedSender<StampedMsgToYamcs>,
}

/// A model acting as a proxy for connected Yamcs instances.
///
/// This model takes advantage of the `Output::map_connect` and
/// `Requestor::filter_map_connect` methods to route messages from/to several
/// models using only one input port ([`to_yamcs`](Self::to_yamcs)) and one
/// requestor port ([`from_yamcs`](Self::from_yamcs)).
///
// FIXME: remove `Clone` bound once
// https://github.com/asynchronics/nexosim/pull/165 is merged into main.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YamcsBridge {
    /// A requestor port forwarding parameter modification requests from Yamcs.
    ///
    /// The request should be acknowledged by replying with the value that was
    /// actually set, which may be different from the requested value.
    pub from_yamcs: Requestor<MsgFromYamcs, MsgToYamcs>,
}

#[Model(type Env = YamcsBridgeEnv)]
impl YamcsBridge {
    /// Forwards the value of a registered parameter to Yamcs -- input port.
    pub async fn to_yamcs(
        &mut self,
        msg: MsgToYamcs,
        cx: &Context<Self>,
        env: &mut YamcsBridgeEnv,
    ) {
        let timestamp = cx.time();
        let msg = StampedMsgToYamcs {
            value: msg.value,
            id: msg.id,
            timestamp,
        };
        // the receiver should never be dropped while the model is alive.
        env.tx.send(msg).unwrap();
    }

    /// Receives a message from Yamcs.
    ///
    /// This port is called by the background server's injector.
    #[nexosim(schedulable)]
    async fn msg_from_yamcs(
        &mut self,
        msg: MsgFromYamcs,
        cx: &Context<Self>,
        env: &mut YamcsBridgeEnv,
    ) {
        let id = msg.id;

        let timestamp = cx.time();

        let mut replies = self.from_yamcs.send(msg).await;
        match replies.next() {
            None => {} // just ignore if no model is connected
            Some(reply) => {
                let validated_msg = StampedMsgToYamcs {
                    value: reply.value,
                    id,
                    timestamp,
                };
                env.tx.send(validated_msg).unwrap();

                assert!(
                    replies.next().is_none(),
                    "Unexpectedly received more than one reply when sending a Yamcs parameter update to models"
                );
            }
        }
    }
}

/// An error returned when attempting to register a parameter with the same path
/// as an already registered parameter.
#[derive(Debug)]
pub struct RegistrationError;

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a parameter with the same path was already registered")
    }
}

impl Error for RegistrationError {}
