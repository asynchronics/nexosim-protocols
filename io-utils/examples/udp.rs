//! An example demonstrating an I/O thread used for communication via UDP
//! protocol.

use std::error::Error;
use std::net::UdpSocket as StdUdpSocket;
use std::sync::mpsc::channel;
use std::thread::{self, sleep};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use thread_guard::ThreadGuard;

use nexosim_io_utils::port::{IoThread, TryRecvError};
use nexosim_io_utils::udp::{Data, Udp};

/// Client address.
const IO_THREAD_ADDR: &str = "127.0.0.1:34254";

/// Echo server address.
const ECHO_THREAD_ADDR: &str = "127.0.0.1:34255";

/// Buffer size.
const BUF_SIZE: usize = 65536;

/// Uses I/O thread to send data to echo UDP server.
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // UDP I/O port.
    let udp = Udp::new(IO_THREAD_ADDR.parse()?, BUF_SIZE);

    // I/O thread handling I/O port operations.
    let mut io_thread = IoThread::new(udp);

    // Channel used for client notification.
    let (tx, rx) = channel();

    // Echo UDP server.
    let echo_thread = ThreadGuard::new(thread::spawn(
        move || -> Result<Bytes, Box<dyn Error + Send + Sync>> {
            let socket = StdUdpSocket::bind(ECHO_THREAD_ADDR)?;
            // Notify client.
            tx.send(())?;
            let mut buf = [0; BUF_SIZE];
            let (len, addr) = socket.recv_from(&mut buf)?;
            println!(
                "Echo server has seen the data {:?} from the address {}",
                &buf[..len],
                addr
            );
            sleep(Duration::from_secs(2));
            socket.send_to(&buf[..len], addr)?;
            Ok(BytesMut::from(&buf[..len]).into())
        },
    ));

    // Data to be sent.
    let data = Data {
        addr: ECHO_THREAD_ADDR.parse().unwrap(),
        bytes: BytesMut::from([1_u8, 2, 3].as_slice()).into(),
    };

    // Wait for server to bind.
    rx.recv()?;

    // Send data via UDP.
    io_thread.send(data.clone())?;

    // It is not possible to return value from a for loop, so we are using a
    // counter.
    let mut counter = 5;
    // Try to receive data echoed by the server.
    let echoed = loop {
        if counter <= 0 {
            break Err(TryRecvError::Empty);
        }
        match io_thread.try_recv() {
            Ok(data) => break Ok(data),
            Err(TryRecvError::Empty) => {}
            Err(error) => break Err(error),
        }
        counter -= 1;
        sleep(Duration::from_secs(1));
    }?;

    assert_eq!(data, echoed);
    assert_eq!(data.bytes, echo_thread.join().unwrap()?);
    Ok(())
}
