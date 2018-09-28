//! A chat server that broadcasts a message to all connections.
//!
//! This example is explicitly more verbose than it has to be. This is to
//! illustrate more concepts.
//!
//! A chat server for telnet clients. After a telnet client connects, the first
//! line should contain the client's name. After that, all lines sent by a
//! client are broadcasted to all other connected clients.
//!
//! Because the client is telnet, lines are delimited by "\r\n".
//!
//! You can test this out by running:
//!
//!     cargo run --example chat
//!
//! And then in another terminal run:
//!
//!     telnet localhost 6142
//!
//! You can run the `telnet` command in any number of additional windows.
//!
//! You can run the second command in multiple windows and then chat between the
//! two, seeing the messages from the other client as they're received. For all
//! connected clients they'll all join the same room and see everyone else's
//! messages.

extern crate tokio;
#[macro_use]
extern crate futures;
extern crate bytes;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use bytes::{BufMut, Bytes, BytesMut};
use futures::future::{self, Either};
use futures::sync::mpsc;
use tokio::codec::{Decoder, Encoder};
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

type Tx = mpsc::UnboundedSender<Bytes>;

type Rx = mpsc::UnboundedReceiver<Bytes>;

struct Shared {
    peers: HashMap<SocketAddr, Peer>,
}

struct Peer {
    tx: Tx,
    rx: Rx,
}

impl Peer {
    fn new() -> Self {
        let (tx, rx) = futures::sync::mpsc::unbounded();
        Self { tx: tx, rx: rx }
    }
}

#[derive(Debug)]
enum Error {
    IoError(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::IoError(e.to_string())
    }
}

#[derive(Debug, Clone)]
struct LineCodec {}

impl LineCodec {
    pub fn new() -> Self {
        Self {}
    }
}

impl Decoder for LineCodec {
    type Item = String;
    type Error = Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let pos = buf.windows(2).position(|bytes| bytes == b"\r\n");

        if let Some(pos) = pos {
            let mut msg = buf.split_to(pos + 2);
            msg.split_off(pos);
            Ok(Some(String::from_utf8_lossy(msg.deref()).to_string()))
        } else {
            Ok(None)
        }
    }
}

impl Encoder for LineCodec {
    type Item = String;
    type Error = Error;

    fn encode(&mut self, data: String, buf: &mut BytesMut) -> Result<(), Self::Error> {
        buf.extend_from_slice(data.as_bytes());
        Ok(())
    }
}

impl Shared {
    fn new() -> Self {
        Shared {
            peers: HashMap::new(),
        }
    }
}

fn broadcast(shared: Arc<Mutex<Shared>>) -> impl Future<Item = (), Error = ()> {
    futures::future::ok(())
}

fn process(socket: TcpStream, shared: Arc<Mutex<Shared>>) {
    let addr = socket.peer_addr().unwrap();
    let peer = Peer::new();
    shared.lock().unwrap().peers.insert(addr, peer);

    let (tx, rx) = LineCodec::new().framed(socket).split();

    let task = rx
        .for_each(move |s| {
            println!("Got: {}", s);
            broadcast(shared.clone())
            //broadcast(shared.clone()).map_err(|e| {
            //    println!("Error: {:?}", e);
            //    ()
            //})
        })
        .map_err(|e| {
            println!("Error: {:?}", e);
            ()
        });
    tokio::spawn(task);
}

pub fn main() {
    let shared = Arc::new(Mutex::new(Shared::new()));
    let addr = "127.0.0.1:6142".parse().unwrap();

    let listener = TcpListener::bind(&addr).unwrap();

    let server = listener
        .incoming()
        .for_each(move |socket| {
            process(socket, shared.clone());
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    println!("server running on localhost:6142");

    tokio::run(server);
}
