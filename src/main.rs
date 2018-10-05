extern crate bytes;
extern crate futures;
extern crate tokio;

use std::net::SocketAddr;
use std::ops::Deref;

use bytes::BytesMut;
use futures::sync::mpsc::{channel, Receiver, Sender};
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

/// Chat command.
enum Cmd {
    Register(
        SocketAddr,
        Box<Sink<SinkItem = String, SinkError = Error> + 'static + Send + Sync>,
    ),
    Broadcast(SocketAddr, String),
}

/// Controls chat messages.
struct MessageController {
    rx: Receiver<Cmd>,
    txs: Vec<Box<Sink<SinkItem = String, SinkError = Error> + 'static + Send + Sync>>,
}

impl MessageController {
    fn new(rx: Receiver<Cmd>) -> Self {
        Self {
            rx: rx,
            txs: vec![],
        }
    }
}

impl Stream for MessageController {
    type Item = ();
    type Error = Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.rx.poll() {
            Ok(Async::Ready(Some(cmd))) => {
                match cmd {
                    Cmd::Register(addr, tx) => {
                        println!("A new peer registered: {:?}", addr);
                        self.txs.push(tx);
                        Ok(Async::Ready(Some(())))
                    }
                    Cmd::Broadcast(sender, msg) => {
                        for tx in self.txs.iter_mut() {
                            match tx.start_send(format!("{}\r\n", msg.clone())) {
                                Ok(AsyncSink::Ready) => {
                                    // バッファーにのせれたよ
                                }
                                Ok(AsyncSink::NotReady(msg)) => {
                                    // クライアントがつまっててバッファにのせれなかった
                                    // TODO リトライしないとこの人にメッセージをおくれない
                                }
                                Err(_) => {
                                    // TODO txsから削除とかする
                                    return Ok(Async::Ready(Some(())));
                                }
                            }

                            match tx.poll_complete() {
                                Ok(Async::Ready(())) => {
                                    // おくれたよ
                                }
                                Ok(Async::NotReady) => {
                                    // まだフラッシュできてないよ
                                }
                                Err(_) => {
                                    // TODO txsから削除とかする
                                    return Err(Error::IoError(format!("Message stream error.")));
                                }
                            }
                        }

                        Ok(Async::Ready(Some(())))
                    }
                }
            }
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Err(Error::IoError(format!("Message stream error."))),
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
fn process(executor: TaskExecutor, socket: TcpStream, handle: Sender<Cmd>) {
    let addr = socket.peer_addr().unwrap();
    let (tx, rx) = LineCodec::new().framed(socket).split();

    let f = handle
        .clone()
        .send(Cmd::Register(addr, Box::new(tx)))
        .map(|_| ())
        .map_err(|e| println!("Peer register error: {:?}", e));

    executor.spawn(f);

    let task = rx
        .map_err(|e| println!("Error: {:?}", e))
        .for_each(move |s| {
            println!("Got: {}", s);
            handle
                .clone()
                .send(Cmd::Broadcast(addr, s))
                .map(|_| ())
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
    let (tx, rx) = channel(0);
    let ctrl = MessageController::new(rx);
    ex.spawn(
        ctrl.for_each(|_| Ok(()))
            .map_err(|e| println!("Message stream error: {:?}", e)),
    );

    let server = listener
        .incoming()
        .for_each(move |socket| {
            println!("A new connection accepted.");
            process(ex.clone(), socket, tx.clone());
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
