use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::{io, thread};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use prost::Message as _;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpSocket, tcp};
use tokio::runtime::Builder;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};
use tracing::{Instrument, info, info_span, warn};

use crate::codegen::ygw;
use crate::yamcs_value::{EncodedYamcsValue, monotonic_time_to_timestamp};
use crate::{MsgFromYamcs, StampedMsgToYamcs};

const YGW_VERSION: u8 = 0;
const YGW_SIMULATOR_NODE_ID: u32 = 0;
const YGW_SIMULATOR_NODE_NAME: &str = "Simulator";
const YGW_SIMULATOR_NODE_DESCRIPTION: &str = "Asynchronix simulator Yamcs node";
const YGW_SIMULATOR_GROUP_NAME: &str = "Simulator";

// This is the maximum capacity of the buffer for messages sent to Yamcs
// instances. Any connected instance lagging by more than
// `BROADCAST_BUFFER_SIZE` messages will be disconnected.
const BROADCAST_BUFFER_SIZE: usize = 128;

/// The single source of truth for the current values of parameters and their
/// associated timestamps.
#[derive(Clone)]
struct Registry {
    inner: Arc<Mutex<RegistryInner>>,
}
struct RegistryInner {
    param_data: ygw::ParameterData,
    generation_count: u64,
    seq_num: u32,
}
impl Registry {
    // Creates a new registry of parameters with the initial values provided by
    // the Yamcs model.
    fn new(params: Vec<EncodedYamcsValue>) -> Self {
        let parameters: Vec<_> = params
            .into_iter()
            .enumerate()
            .map(|(id, v)| {
                let value = ygw::Value { v: Some(v.0) };

                ygw::ParameterValue {
                    id: id.try_into().unwrap(),
                    raw_value: None,
                    eng_value: Some(value),
                    acquisition_time: None,
                    generation_time: None,
                    expire_millis: None,
                }
            })
            .collect();

        let param_data = ygw::ParameterData {
            parameters,
            group: String::from(YGW_SIMULATOR_GROUP_NAME),
            seq_num: 0,
            generation_time: None,
            acquisition_time: None,
        };

        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                param_data,
                generation_count: 0,
                seq_num: 0,
            })),
        }
    }

    /// Updates the value of the parameter with the given ID in the registry.
    ///
    /// If successful, this will increment and return the generation counter.
    fn update_and_serialize_parameter(
        &self,
        msg: StampedMsgToYamcs,
    ) -> Result<(Bytes, u64), InvalidId> {
        let param_value = ygw::ParameterValue {
            id: msg.id,
            raw_value: None,
            eng_value: Some(ygw::Value {
                v: Some(msg.value.0),
            }),
            generation_time: Some(monotonic_time_to_timestamp(&msg.timestamp)),
            acquisition_time: None,
            expire_millis: None,
        };

        let mut registry = self.inner.lock().unwrap();
        let id: usize = msg.id.try_into().unwrap();
        if id >= registry.param_data.parameters.len() {
            return Err(InvalidId {});
        }

        // Increment the generation count and the sequence count.
        registry.generation_count = registry.generation_count.checked_add(1).unwrap();
        registry.seq_num = registry.seq_num.wrapping_add(1);

        let mut param_data = ygw::ParameterData {
            parameters: vec![param_value],
            group: String::from(YGW_SIMULATOR_GROUP_NAME),
            seq_num: registry.seq_num,
            generation_time: None,
            acquisition_time: None,
        };

        let recording_number = registry.generation_count;
        let serialized_param = serialize_message(
            recording_number,
            ygw::MessageType::ParameterData,
            &param_data,
        );

        // Update the registry
        let param_value = param_data.parameters.pop().unwrap();
        registry.param_data.parameters[id] = param_value;

        Ok((serialized_param, registry.generation_count))
    }

    /// Serializes all parameters and returns the generation counter
    /// corresponding to the last update.
    fn serialize_all_parameters(&self) -> (Bytes, u64) {
        let registry = self.inner.lock().unwrap();

        let recording_number = 0;
        let serialized_params = serialize_message(
            recording_number,
            ygw::MessageType::ParameterData,
            &registry.param_data,
        );

        (serialized_params, registry.generation_count)
    }
}

/// Starts the server.
///
/// This function is non-blocking and runs tokio in its own thread.
pub(crate) fn start(
    param_definitions: Vec<ygw::ParameterDefinition>,
    param_values: Vec<EncodedYamcsValue>,
    gateway_tx: mpsc::UnboundedSender<MsgFromYamcs>,
    gateway_rx: mpsc::UnboundedReceiver<StampedMsgToYamcs>,
    port: u16,
) -> io::Result<()> {
    let registry = Registry::new(param_values);

    // We could use the simpler `TcpListener::bind()` method, but it requires
    // tokio to be already started, which would make it impossible to report any
    // potential `bind` error on this thread without blocking.
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let socket = TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;

    let _th = thread::spawn(move || {
        let rt = Builder::new_current_thread().enable_io().build().unwrap();

        rt.block_on(async move {
            let connection_tx = broadcast::Sender::new(BROADCAST_BUFFER_SIZE);

            let listener = socket
                .listen(1024)
                .expect("Could not listen on localhost socket");

            tokio::spawn(
                accept_connection(
                    param_definitions,
                    registry.clone(),
                    connection_tx.clone(),
                    listener,
                    gateway_tx,
                )
                .instrument(info_span!("Yamcs bridge connection listener")),
            );

            // The broadcast task will return automatically when the model is
            // dropped since it will see that the gateway channel has been
            // closed.
            broadcast_to_yamcs(registry, connection_tx, gateway_rx).await;

            // All spawned tasks are aborted here.
        });
    });

    Ok(())
}

/// Set up a new connection to a Yamcs instance.
async fn accept_connection(
    param_definitions: Vec<ygw::ParameterDefinition>,
    registry: Registry,
    connection_tx: broadcast::Sender<(Bytes, u64)>,
    listener: TcpListener,
    gateway_tx: mpsc::UnboundedSender<MsgFromYamcs>,
) -> Result<(), io::Error> {
    // Parameter definitions are immutable so they are serialized ahead of time.
    let serialized_param_definition_list = {
        let param_definition_list = ygw::ParameterDefinitionList {
            definitions: param_definitions,
        };
        let recording_number = 0;

        serialize_message(
            recording_number,
            ygw::MessageType::ParameterDefinitions,
            &param_definition_list,
        )
    };

    loop {
        let socket = listener.accept().await?.0;
        let (read_socket, mut write_socket) = socket.into_split();

        // Send node information.
        {
            let node_list = ygw::NodeList {
                nodes: vec![ygw::Node {
                    id: YGW_SIMULATOR_NODE_ID,
                    name: YGW_SIMULATOR_NODE_NAME.into(),
                    description: Some(YGW_SIMULATOR_NODE_DESCRIPTION.into()),
                    tc: None,
                    tm: None,
                    links: Vec::new(),
                }],
            };
            let serialized_node_list = serialize_node_list(&node_list);
            if let Err(err_msg) = write_socket.write_all(&serialized_node_list).await {
                info!("Dropping connection with Yamcs: {}", err_msg);

                continue;
            }
        }

        // Send parameter definitions.
        if let Err(err_msg) = write_socket
            .write_all(&serialized_param_definition_list)
            .await
        {
            info!("Dropping connection with Yamcs: {}", err_msg);

            continue;
        }

        // Subscribe to the broadcast channel _before_ synchronizing the set of
        // parameters. Otherwise, we may miss some parameter updates between the
        // moment the parameter set is synchronized and the moment the channel
        // is subscribed to. Doing it too early is not a problem since the
        // writer discards updates that predate the synchronization.
        let connection_rx = connection_tx.subscribe();

        // Synchronize the set of parameters with the new Yamcs instance.
        //
        // This block is scoped to eagerly free allocated memory.
        let init_generation_count = {
            let (serialized_parameters, generation_count) = registry.serialize_all_parameters();
            if let Err(err_msg) = write_socket.write_all(&serialized_parameters).await {
                info!("Dropping connection with Yamcs: {}", err_msg);

                continue;
            }

            generation_count
        };

        // Send the initial link status.
        //
        // This block is scoped to eagerly free allocated memory.
        {
            let link_status = ygw::LinkStatus {
                state: ygw::LinkState::Ok as i32,
                data_in_count: 0,
                data_out_count: 0,
                data_in_size: 0,
                data_out_size: 0,
                err: None,
            };
            let serialized_link_status =
                serialize_message(0, ygw::MessageType::LinkStatus, &link_status);
            if let Err(err_msg) = write_socket.write_all(&serialized_link_status).await {
                info!("Dropping connection with Yamcs: {}", err_msg);

                continue;
            }
        }

        // Start regular I/O.
        let (data_in_reader, data_in_writer) = data_counter_handles();
        tokio::spawn(
            socket_reader(read_socket, gateway_tx.clone(), data_in_writer)
                .instrument(info_span!("Yamcs bridge socket reader")),
        );
        tokio::spawn(
            socket_writer(
                init_generation_count,
                write_socket,
                connection_rx,
                data_in_reader,
            )
            .instrument(info_span!("Yamcs bridge socket writer")),
        );
    }
}

/// Broadcasts messages from the model to all `socket_writer` tasks.
async fn broadcast_to_yamcs(
    registry: Registry,
    connection_tx: broadcast::Sender<(Bytes, u64)>,
    mut gateway_rx: mpsc::UnboundedReceiver<StampedMsgToYamcs>,
) {
    while let Some(msg) = gateway_rx.recv().await {
        if let Ok(payload) = registry.update_and_serialize_parameter(msg) {
            let _ = connection_tx.send(payload);
        }
    }

    // Nothing else to do: all `socket_writer` and `socket_reader` tasks will
    // either exit cleanly due to their channels being closed, or will be
    // cancelled when tokio shuts down.
}

/// Receives serialized messages from a Yamcs instance.
///
/// Valid messages are deserialized and forwarded to the model. At the moment,
/// only parameter updates are supported. Other messages types and invalid
/// messages are ignored.
///
/// The value is expected in the "engineering value" field. The timestamp can be
/// optionally provided in the "generation time" field and will be forwarded
/// too.
async fn socket_reader(
    read_socket: tcp::OwnedReadHalf,
    gateway_tx: mpsc::UnboundedSender<MsgFromYamcs>,
    mut data_in_writer: DataCounterWriter,
) {
    let mut stream = FramedRead::new(read_socket, LengthDelimitedCodec::new());

    loop {
        match stream.next().await {
            // A message frame was received.
            Some(Ok(buf)) => {
                data_in_writer.increment(buf.len() as u64);

                match deserialize_parameter_updates(buf) {
                    // A list of parameter updates was received.
                    Ok(updates) => {
                        for update in updates {
                            let value = match update.eng_value {
                                // An engineering value was sent.
                                Some(ygw::Value { v: Some(value) }) => {
                                    // Convert the time stamp (if any) to a
                                    // `MonotonicTime`.
                                    let id = update.id;

                                    MsgFromYamcs {
                                        value: EncodedYamcsValue(value),
                                        id,
                                    }
                                }
                                // No engineering value.
                                _ => {
                                    info!(
                                        "Ignored parameter update from Yamcs without engineering value"
                                    );

                                    continue;
                                }
                            };

                            if gateway_tx.send(value).is_err() {
                                info!("The Yamcs bridge is no longer reachable");

                                return;
                            }
                        }
                    }

                    // The message was not a list of parameter updates or was
                    // otherwise invalid.
                    Err(err_msg) => {
                        warn!("Invalid message from Yamcs: {}", err_msg);
                    }
                }
            }

            // Irrecoverable error, e.g. wrong frame length or connection closed
            // in the middle of a message.
            Some(Err(err_msg)) => {
                warn!("Dropping connection with Yamcs: {}", err_msg);

                return;
            }

            // Connection cleanly closed.
            None => {
                info!("The connection was closed");

                return;
            }
        }
    }
}

/// Forward serialized messages from the broadcaster to the socket.
async fn socket_writer(
    init_generation_count: u64,
    mut write_socket: tcp::OwnedWriteHalf,
    mut connection_rx: broadcast::Receiver<(Bytes, u64)>,
    data_in_reader: DataCounterReader,
) {
    let mut link_status = ygw::LinkStatus {
        state: ygw::LinkState::Ok as i32,
        data_in_count: 0,
        data_out_count: 0,
        data_in_size: 0,
        data_out_size: 0,
        err: None,
    };

    loop {
        match connection_rx.recv().await {
            // Only send the message if it was created after the initial
            // synchronization.
            Ok((bytes, generation_count)) if generation_count > init_generation_count => {
                // Send the message.
                if let Err(e) = write_socket.write_all(&bytes).await {
                    info!("Dropping connection with Yamcs: {}", e);

                    break;
                }

                // Update and send the link status.
                link_status.data_out_count = link_status.data_out_count.wrapping_add(1);
                link_status.data_out_size =
                    link_status.data_out_size.wrapping_add(bytes.len() as u64);
                link_status.data_in_count = data_in_reader.count();
                link_status.data_in_size = data_in_reader.size();
                let serialized_link_status =
                    serialize_message(0, ygw::MessageType::LinkStatus, &link_status);
                if let Err(e) = write_socket.write_all(&serialized_link_status).await {
                    info!("Dropping connection with Yamcs: {}", e);

                    break;
                }
            }
            // Otherwise just ignore the parameter update.
            Ok(_) => {}
            // Close the connection and return if some updates were missed (i.e.
            // if we get `RecvError::Lagged`) or if the channel was closed.
            Err(_) => {
                warn!("Dropping connection with Yamcs: too many failed parameter updates");

                break;
            }
        }
    }
}

/// Deserializes a message from a Yamcs instance, assuming the message is a list
/// of parameter updates.
///
///  A data frame should contain:
/// - 1 byte: the YGW version, expected to be YGW_VERSION,
/// - 1 byte: the message type,
/// - 4 bytes: the node id, expected to be YGW_NODE_ID or unspecified (0xffffffff),
/// - 4 bytes: the link id, expected to be 0,
/// - n bytes: the message content.
fn deserialize_parameter_updates(mut buf: BytesMut) -> Result<Vec<ygw::ParameterValue>, String> {
    // Check the message length.
    if buf.len() < 10 {
        return Err(format!(
            "message too short: expected at least 10 bytes, got {}",
            buf.len()
        ));
    }

    // Check the version.
    let version = buf.get_u8();
    if version != YGW_VERSION {
        return Err(format!(
            "invalid message version: expected {}, got {})",
            YGW_VERSION, version,
        ));
    }

    // Return an error if this is not a list of parameter updates.
    let msg_type = buf.get_u8() as i32;
    if msg_type != ygw::MessageType::ParameterUpdates as i32 {
        return Err(format!("unexpected message type: {}", msg_type));
    }

    // Returns an error if the node ID is invalid.
    let node_id = buf.get_u32();
    if node_id != YGW_SIMULATOR_NODE_ID && node_id != u32::MAX {
        return Err(format!("unexpected node ID: {}", node_id));
    }

    // Returns an error if the link ID is non-null.
    let link_id = buf.get_u32();
    if link_id != 0 {
        return Err(format!("unexpected link ID: {}", link_id));
    }

    match ygw::ParameterUpdates::decode(buf) {
        Ok(param_data) => Ok(param_data.parameters),
        Err(e) => Err(e.to_string()),
    }
}

/// Serializes an arbitrary ProtoBuf message.
///
///  A data frame contains:
/// - 4 bytes: the total length, excluding this field,
/// - 1 byte: the YGW version,
/// - 8 bytes: the recording number,
///   but its function is not fully clear at the moment.
/// - 1 byte: the message type,
/// - 4 bytes: the node id (always YGW_NODE_ID),
/// - 4 bytes: the link id (always 0),
/// - n bytes: the message content.
fn serialize_message<T: prost::Message>(msg_rn: u64, msg_type: ygw::MessageType, msg: &T) -> Bytes {
    let len = msg.encoded_len() + 18;
    let mut buf = BytesMut::with_capacity(msg.encoded_len() + 18);

    buf.put_u32(len as u32);
    buf.put_u8(YGW_VERSION);
    buf.put_u64(msg_rn);
    buf.put_u8(msg_type as u8);
    buf.put_u32(YGW_SIMULATOR_NODE_ID);
    buf.put_u32(0);
    msg.encode_raw(&mut buf);

    buf.freeze()
}

/// Serializes the list of nodes.
///
///  A data frame contains:
/// - 4 bytes: the total length, excluding this field
/// - 1 byte: the YGW version,
/// - 8 bytes: the recording number (always 0),
/// - 1 byte: the message type,
/// - n bytes: the message content.
pub(crate) fn serialize_node_list(node_list: &ygw::NodeList) -> Bytes {
    let len = 10 + node_list.encoded_len();
    let mut buf = BytesMut::with_capacity(len);

    buf.put_u32(len as u32);
    buf.put_u8(YGW_VERSION);
    buf.put_u64(0);
    buf.put_u8(ygw::MessageType::NodeInfo as u8);
    node_list.encode_raw(&mut buf);

    buf.freeze()
}

/// Error returned when a parameter ID is not valid.
struct InvalidId {}

/// A data counter reader.
#[derive(Clone)]
struct DataCounterReader {
    inner: Arc<DataCounterInner>,
}

impl DataCounterReader {
    /// Count of transmitted data.
    fn count(&self) -> u64 {
        self.inner.count.load(Ordering::Relaxed)
    }
    /// Cumulated size of transmitted bytes.
    fn size(&self) -> u64 {
        self.inner.size.load(Ordering::Relaxed)
    }
}

/// A data counter single writer.
struct DataCounterWriter {
    inner: Arc<DataCounterInner>,
}

impl DataCounterWriter {
    /// Increment the data count by one and the transmitted number of bytes by
    /// the provided size.
    fn increment(&mut self, size: u64) {
        // These atomics are only modified from this single writer so there is
        // no need for RMW atomic operations: plain loads and stores are enough.
        //
        // We could ensure that `data_in_count` and `data_in_size` are
        // synchronized by using a mutex instead, but this is probably overkill
        // since these counters are displayed to the user for informational
        // purpose only.
        self.inner.count.store(
            self.inner.count.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
        self.inner.size.store(
            self.inner.size.load(Ordering::Relaxed).wrapping_add(size),
            Ordering::Relaxed,
        );
    }
}

/// Content of the data counter.
struct DataCounterInner {
    count: AtomicU64,
    size: AtomicU64,
}

/// Returns a reader-writer pair for the data counter.
fn data_counter_handles() -> (DataCounterReader, DataCounterWriter) {
    let inner = Arc::new(DataCounterInner {
        count: AtomicU64::new(0),
        size: AtomicU64::new(0),
    });
    let reader = DataCounterReader {
        inner: inner.clone(),
    };
    let writer = DataCounterWriter { inner };

    (reader, writer)
}
