extern crate bytes;
extern crate futures;
extern crate tokio;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use bytes::BytesMut;
use futures::sync::mpsc::{channel, Sender};
use tokio::codec::{Decoder, Encoder};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

#[derive(Debug)]
enum Error {
    IoError(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::IoError(e.to_string())
    }
}

struct Shared {
    peers: HashMap<SocketAddr, Peer>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            peers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct Peer {
    tx: Sender<String>,
}

impl Peer {
    fn new(tx: impl Sink<SinkItem = String, SinkError = Error> + Send + 'static) -> Self {
        let (ctx, crx) = channel(0);

        let f = crx
            .map_err(|e| Error::IoError(format!("Peer stream error {:?}", e)))
            .forward(tx)
            .map(|_| ())
            .map_err(|e| println!("Forwarding aborted: {:?}", e));
        tokio::spawn(f);

        Peer { tx: ctx }
    }
}

/// Chat message codec.
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
        println!("Decode {:?}", buf);
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

/// Broadcast messages to all peers.
fn broadcast(
    sender: SocketAddr,
    msg: String,
    shared: Arc<Mutex<Shared>>,
) -> impl Future<Item = (), Error = ()> {
    println!("Broadcasting message: {}", msg);

    for (addr, peer) in shared.lock().unwrap().peers.iter_mut() {
        if sender != *addr {
            tokio::spawn(
                peer.clone()
                    .tx
                    .send(format!("{}\r\n", msg))
                    .map(|_| ())
                    .map_err(|e| println!("Broadcast error: {:?}", e)),
            );
        }
    }

    futures::future::ok(())
}

/// Process client connection.
fn process(socket: TcpStream, shared: Arc<Mutex<Shared>>) {
    let addr = socket.peer_addr().unwrap();
    let (tx, rx) = LineCodec::new().framed(socket).split();
    shared.lock().unwrap().peers.insert(addr, Peer::new(tx));

    let task = rx
        .map_err(|e| println!("Error: {:?}", e))
        .for_each(move |s| {
            println!("Got: {}", s);
            broadcast(addr, s, shared.clone())
        })
        .map_err(|e| println!("Error: {:?}", e));
    tokio::spawn(task);
}

pub fn main() {
    let shared = Arc::new(Mutex::new(Shared::new()));
    let addr = "0.0.0.0:6142".parse().unwrap();

    let listener = TcpListener::bind(&addr).unwrap();

    let server = listener
        .incoming()
        .for_each(move |socket| {
            println!("A new connection accepted.");
            process(socket, shared.clone());
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    println!("server running on localhost:6142");

    tokio::run(server);
}
