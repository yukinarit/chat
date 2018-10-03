extern crate bytes;
extern crate futures;
extern crate tokio;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::Deref;

use bytes::BytesMut;
use futures::sync::mpsc::{channel, Sender};
use tokio::codec::{Decoder, Encoder};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;
use tokio::runtime::{Runtime, TaskExecutor};

use self::Error::IoError;

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
struct Peer {
    addr: SocketAddr,
    tx: Sender<String>,
}

impl Peer {
    fn new(
        addr: SocketAddr,
        tx: impl Sink<SinkItem = String, SinkError = Error> + Send + 'static,
    ) -> Self {
        let (ctx, crx) = channel(0);

        let f = crx
            .map_err(|e| IoError(format!("Peer stream error {:?}", e)))
            .forward(tx)
            .map(|_| ())
            .map_err(|e| println!("Forwarding aborted: {:?}", e));
        tokio::spawn(f);

        Peer {
            addr: addr,
            tx: ctx,
        }
    }
}

/// Chat command.
#[derive(Debug)]
enum Cmd {
    Register(Peer),
    Broadcast(SocketAddr, String),
}

/// Controls chat messages.
#[derive(Debug, Clone)]
struct MessageController {
    executor: TaskExecutor,
    tx: Option<Sender<Cmd>>,
}

impl MessageController {
    fn new(ex: TaskExecutor) -> Self {
        Self {
            executor: ex,
            tx: None,
        }
    }

    /// Run MessageController.
    fn run(&mut self) -> impl Future<Item = (), Error = ()> {
        let (tx, rx) = channel(0);
        self.tx = Some(tx);
        let mut peers: HashMap<SocketAddr, Peer> = HashMap::new();

        let ex = self.executor.clone();

        rx.map_err(|e| IoError(format!("MessageController stream error {:?}", e)))
            .for_each(move |cmd| {
                let f = match cmd {
                    Cmd::Register(peer) => {
                        println!("A new peer registered: {:?}", peer);
                        peers.insert(peer.addr, peer);
                        Ok(())
                    }
                    Cmd::Broadcast(sender, msg) => {
                        for (addr, peer) in peers.iter() {
                            if sender != *addr {
                                println!("Broadcasting '{}' to {:?}", msg, addr);
                                let f = peer
                                    .tx
                                    .clone()
                                    .send(format!("{}\r\n", msg.clone()))
                                    .map(|_| ())
                                    .map_err(|e| println!("Broadcast error: {:?}", e));
                                ex.spawn(f)
                            }
                        }
                        Ok(())
                    }
                };
                f
            })
            .map_err(|e| println!("Forwarding aborted: {:?}", e))
    }

    /// Get a handle.
    fn handle(&self) -> Option<MessageHandle> {
        match self.tx {
            Some(ref tx) => Some(MessageHandle { tx: tx.clone() }),
            None => None,
        }
    }
}

struct MessageHandle {
    tx: Sender<Cmd>,
}

impl MessageHandle {
    /// Push a new chat command to the queue.
    fn push_cmd(&self, cmd: Cmd) -> impl Future<Item = (), Error = Error> {
        self.tx
            .clone()
            .send(cmd)
            .map(|_| ())
            .map_err(|e| IoError(e.to_string()))
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

/// Process client connection.
fn process(executor: TaskExecutor, socket: TcpStream, handle: MessageHandle) {
    let addr = socket.peer_addr().unwrap();
    let (tx, rx) = LineCodec::new().framed(socket).split();

    let f = handle
        .push_cmd(Cmd::Register(Peer::new(addr, tx)))
        .map_err(|e| println!("Peer register error: {:?}", e));

    executor.spawn(f);

    let task = rx
        .map_err(|e| println!("Error: {:?}", e))
        .for_each(move |s| {
            println!("Got: {}", s);
            handle
                .push_cmd(Cmd::Broadcast(addr, s))
                .map_err(|e| println!("MessageController error: {:?}", e))
        })
        .map_err(|e| println!("Error: {:?}", e));

    executor.spawn(task);
}

pub fn main() {
    let mut runtime = Runtime::new().unwrap();
    let ex = runtime.executor().clone();
    let addr = "0.0.0.0:6142".parse().unwrap();

    let listener = TcpListener::bind(&addr).unwrap();
    let mut ctrl = MessageController::new(ex.clone());
    ex.spawn(ctrl.run());

    let server = listener
        .incoming()
        .for_each(move |socket| {
            println!("A new connection accepted.");
            process(ex.clone(), socket, ctrl.handle().unwrap());
            Ok(())
        })
        .map_err(|err| {
            println!("accept error = {:?}", err);
        });

    println!("server running on localhost:6142");

    match runtime.block_on(server) {
        Ok(_) => {}
        Err(e) => println!("Runtime error: {:?}", e),
    };
}
