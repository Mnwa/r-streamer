use crate::server::client::{connect, Client, ClientSslState};
use crate::server::udp::{ServerData, UdpSend, WebRtcRequest};
use actix::prelude::*;
use actix_rt::spawn;
use futures::{FutureExt, SinkExt, StreamExt};
use log::{info, warn};
use rtp_rs::RtpReader;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct TlsActor {
    clients: HashMap<SocketAddr, Client>,
    data: Arc<ServerData>,
    send: Arc<Addr<UdpSend>>,
}

impl TlsActor {
    pub fn new(data: Arc<ServerData>, send: Arc<Addr<UdpSend>>) -> Addr<Self> {
        Self::create(|_| TlsActor {
            data,
            send,
            clients: HashMap::new(),
        })
    }
}

impl Actor for TlsActor {
    type Context = Context<Self>;
}

impl Handler<WebRtcRequest> for TlsActor {
    type Result = ();

    fn handle(&mut self, msg: WebRtcRequest, ctx: &mut Context<Self>) -> Self::Result {
        if let WebRtcRequest::Dtls(message, addr) = msg {
            let ssl_acceptor = Arc::clone(&self.data.crypto.ssl_acceptor);

            let client = self
                .clients
                .entry(addr)
                .or_insert_with(|| Client::new(addr, message.clone()));

            let mut incoming_writer = client.channels.incoming_writer.clone();
            match std::mem::replace(&mut client.ssl_state, ClientSslState::Shutdown) {
                ClientSslState::Empty(ssl_stream, handshake) => {
                    let stream = connect(ssl_stream, handshake, incoming_writer, ssl_acceptor)
                        .into_stream()
                        .filter_map(|res| async {
                            match res {
                                Ok(stream) => Some(stream),
                                Err(_) => {
                                    info!("handshake timeout");
                                    None
                                }
                            }
                        })
                        .flatten()
                        .map(IncomingWritings);

                    client.ssl_state = ClientSslState::Connected;
                    ctx.add_message_stream(stream);

                    let outgoing_reader = Arc::clone(&client.channels.outgoing_reader);
                    let addr = client.addr;
                    ctx.add_message_stream(futures::stream::unfold(
                        (outgoing_reader, addr),
                        |(outgoing_reader, addr)| async move {
                            let message = outgoing_reader.lock().await.next().await?;
                            Some((OutgoingReaders(message, addr), (outgoing_reader, addr)))
                        },
                    ));
                }
                ClientSslState::Connected => {
                    spawn(async move {
                        match incoming_writer.send(message).await {
                            Ok(()) => {}
                            Err(err) => warn!("{:?}", err),
                        }
                    });
                    client.ssl_state = ClientSslState::Connected;
                }
                ssl_state => {
                    client.ssl_state = ssl_state;
                }
            }
        }
    }
}

impl Handler<IncomingWritings> for TlsActor {
    type Result = ();

    fn handle(&mut self, IncomingWritings(item): IncomingWritings, _ctx: &mut Context<Self>) {
        match RtpReader::new(&item) {
            Ok(reader) => warn!("version {:?}", reader.timestamp()),
            Err(err) => warn!("{:?}", err),
        }
    }
}

impl Handler<OutgoingReaders> for TlsActor {
    type Result = ();

    fn handle(&mut self, OutgoingReaders(message, addr): OutgoingReaders, ctx: &mut Context<Self>) {
        let send = Arc::clone(&self.send);
        ctx.spawn(
            async move {
                if let Err(e) = send.send(WebRtcRequest::Dtls(message, addr)).await {
                    warn!("dtls to udp send: {:#?}", e)
                }
            }
            .into_actor(self),
        );
    }
}

struct IncomingWritings(Vec<u8>);
struct OutgoingReaders(Vec<u8>, SocketAddr);

impl Message for IncomingWritings {
    type Result = ();
}
impl Message for OutgoingReaders {
    type Result = ();
}
