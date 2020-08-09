use crate::rtp::core::RtpHeader;
use crate::rtp::processor::{ProcessRtpPacket, ProcessorActor, ProtectRtpPacket};
use crate::rtp::srtp::ErrorParse;
use crate::sdp::media::MediaAddrMessage;
use crate::{
    client::{
        clients::{ClientState, ClientsRefStorage},
        dtls::{extract_dtls, push_dtls},
        group::{Group, GroupId},
    },
    dtls::{
        connector::connect,
        message::{DtlsMessage, MessageType},
    },
    rtp::core::is_rtcp,
    server::udp::{UdpSend, WebRtcRequest},
};
use actix::prelude::*;
use futures::stream::{iter, StreamExt, TryStreamExt};
use futures::{FutureExt, TryFutureExt};
use log::{info, warn};
use openssl::ssl::SslAcceptor;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use std::{net::SocketAddr, sync::Arc};
use tokio::time::{timeout, Duration};

pub struct ClientActor {
    client_storage: ClientsRefStorage,
    groups: Group,
    ssl_acceptor: Arc<SslAcceptor>,
    udp_send: Addr<UdpSend>,
    processor: Addr<ProcessorActor>,
}

impl ClientActor {
    pub fn new(ssl_acceptor: Arc<SslAcceptor>, udp_send: Addr<UdpSend>) -> Addr<ClientActor> {
        ClientActor::create(|_| ClientActor {
            ssl_acceptor,
            udp_send,
            client_storage: ClientsRefStorage::new(),
            groups: Group::default(),
            processor: ProcessorActor::new(),
        })
    }
}

impl Actor for ClientActor {
    type Context = Context<Self>;
}

impl Handler<WebRtcRequest> for ClientActor {
    type Result = ();

    fn handle(&mut self, msg: WebRtcRequest, ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            WebRtcRequest::Dtls(message, addr) => {
                let client_ref = self.client_storage.entry(addr).or_default();
                let acceptor = Arc::clone(&self.ssl_acceptor);

                ctx.add_message_stream(client_ref.borrow().outgoing_stream(addr));

                let incoming_writer =
                    Arc::clone(&client_ref.borrow().get_channels().incoming_writer);
                let client = client_ref.borrow().get_client();

                let self_addr = ctx.address();

                ctx.spawn(
                    async move {
                        let mut incoming_writer = incoming_writer.lock().await;
                        if let Err(e) = push_dtls(&mut incoming_writer, message).await {
                            warn!("push dtls err: {}", e);
                            if let Err(e) = self_addr.send(DeleteMessage(addr)).await {
                                warn!("delete err: {}", e)
                            }
                        }
                        drop(incoming_writer);

                        let mut client_unlocked = client.lock().await;

                        match client_unlocked.state {
                            ClientState::New(_) => {
                                if let Err(e) = connect(&mut client_unlocked, acceptor).await {
                                    warn!("connect err: {}", e);
                                    match self_addr.send(DeleteMessage(addr)).await {
                                        Err(e) => warn!("delete err: {}", e),
                                        Ok(is_deleted) => info!("deleted {}", is_deleted),
                                    }
                                }
                            }
                            ClientState::Connected(_, _) => {
                                let mut buf = vec![0; 512];
                                let result = timeout(
                                    Duration::from_millis(10),
                                    extract_dtls(&mut client_unlocked, &mut buf),
                                )
                                .await
                                .map_err(|_| std::io::ErrorKind::TimedOut.into())
                                .and_then(|r| r);

                                drop(client_unlocked);

                                if matches!(result, Ok(0)) {
                                    match self_addr.send(DeleteMessage(addr)).await {
                                        Err(e) => warn!("delete err: {}", e),
                                        Ok(d) if d => info!("success deleted"),
                                        Ok(_) => info!("fail deleting"),
                                    }
                                }
                            }
                            ClientState::Shutdown => {}
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Rtc(message, addr) => {
                let start = Instant::now();
                let udp_send = self.udp_send.clone();
                let client_ref = self.client_storage.entry(addr).or_default();
                let client = client_ref.borrow().get_client();

                let is_rtcp = is_rtcp(&message);

                let addresses = if is_rtcp {
                    client_ref
                        .borrow()
                        .get_receivers()
                        .iter()
                        .filter(|(_, client)| !client.borrow().is_deleted())
                        .map(|(g_addr, client)| (*g_addr, (client.borrow().get_client(), 0)))
                        .collect::<HashMap<_, _>>()
                } else {
                    let codec = client_ref
                        .borrow()
                        .get_media()
                        .and_then(|mref| Some((mref, RtpHeader::from_buf(&message).ok()?)))
                        .and_then(|(m, r)| Some((r.marker, m.get_name(&r.payload).cloned()?)));

                    if let Some((marker, codec_format)) = codec {
                        client_ref
                            .borrow()
                            .get_receivers()
                            .iter()
                            .filter(|(_, client)| !client.borrow().is_deleted())
                            .filter_map(|(g_addr, client_ref)| {
                                let media = client_ref
                                    .borrow()
                                    .get_media()?
                                    .get_id(&codec_format)
                                    .copied()?;
                                Some((
                                    *g_addr,
                                    (
                                        client_ref.borrow().get_client(),
                                        calculate_payload(marker, media),
                                    ),
                                ))
                            })
                            .collect::<HashMap<_, _>>()
                    } else {
                        client_ref
                            .borrow()
                            .get_receivers()
                            .iter()
                            .filter(|(_, client)| !client.borrow().is_deleted())
                            .map(|(g_addr, client_ref)| {
                                (*g_addr, (client_ref.borrow().get_client(), 0))
                            })
                            .collect::<HashMap<_, _>>()
                    }
                };

                let processor = self.processor.clone();
                let processor_two = self.processor.clone();

                if !addresses.is_empty() {
                    ctx.spawn(
                        client
                            .lock_owned()
                            .then(move |client| {
                                processor
                                    .send(ProcessRtpPacket {
                                        message,
                                        addr,
                                        client,
                                        is_rtcp,
                                    })
                                    .map_err(ErrorParse::from)
                                    .map(|message_result| {
                                        message_result
                                            .and_then(|message_processed| message_processed)
                                    })
                            })
                            .and_then(move |message| {
                                iter(addresses)
                                    .then(move |(addr, (client, payload))| {
                                        client
                                            .lock_owned()
                                            .map(move |client| (addr, (client, payload)))
                                    })
                                    .then(move |(addr, (client, payload))| {
                                        processor_two
                                            .send(ProtectRtpPacket {
                                                message: message.clone(),
                                                addr,
                                                client,
                                                is_rtcp,
                                                payload,
                                            })
                                            .map_err(ErrorParse::from)
                                            .map(move |message_result| {
                                                message_result
                                                    .and_then(|message_processed| message_processed)
                                                    .map(|message| (message, addr))
                                            })
                                    })
                                    .and_then(move |(message, addr)| {
                                        udp_send
                                            .send(WebRtcRequest::Rtc(message, addr))
                                            .map_err(ErrorParse::from)
                                    })
                                    .map_err(ErrorParse::from)
                                    .try_collect::<Vec<_>>()
                            })
                            .inspect_err(|e| {
                                if !e.should_ignored() {
                                    warn!("processor err: {:?}", e)
                                }
                            })
                            .map(move |_| println!("{:?}", start.elapsed()))
                            .into_actor(self),
                    );
                }
            }
            WebRtcRequest::Stun(_, _) => warn!("stun could not be accepted in client actor"),
            WebRtcRequest::Unknown => warn!("client actor unknown request"),
        }
    }
}

impl Handler<DtlsMessage> for ClientActor {
    type Result = ();

    fn handle(&mut self, item: DtlsMessage, ctx: &mut Context<Self>) {
        match item.get_type() {
            MessageType::Incoming => warn!("accepted incoming dtls message in the ClientActor"),
            MessageType::Outgoing => {
                let udp_send = self.udp_send.clone();
                ctx.spawn(
                    async move {
                        if let Err(e) = udp_send.send(item.into_webrtc()).await {
                            warn!("udp sender: {}", e)
                        }
                    }
                    .into_actor(self),
                );
            }
        }
    }
}

impl Handler<DeleteMessage> for ClientActor {
    type Result = bool;

    fn handle(
        &mut self,
        DeleteMessage(addr): DeleteMessage,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.client_storage
            .remove(&addr)
            .map(|client| client.borrow_mut().delete())
            .map(|_| self.groups.remove_sender(addr))
            .is_some()
    }
}

impl Handler<GroupId> for ClientActor {
    type Result = ();

    fn handle(
        &mut self,
        GroupId(group_id, addr): GroupId,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let sender_addr = self.groups.insert_or_get_sender(group_id, addr);
        let result = self
            .client_storage
            .get(&addr)
            .map(|client_ref| (Rc::clone(client_ref), sender_addr))
            .filter(|(_, sender_addr)| *sender_addr != addr)
            .and_then(|(client_ref, sender_addr)| {
                self.client_storage
                    .get_mut(&sender_addr)
                    .map(|sender_ref| (client_ref, sender_ref))
            });

        if let Some((client_ref, sender_ref)) = result {
            sender_ref.borrow_mut().add_receiver(addr, client_ref);
        }
    }
}

impl Handler<MediaAddrMessage> for ClientActor {
    type Result = ();

    fn handle(
        &mut self,
        MediaAddrMessage(addr, media): MediaAddrMessage,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        if let Some(c) = self.client_storage.get_mut(&addr) {
            c.borrow_mut().set_media(media)
        }
    }
}

struct DeleteMessage(SocketAddr);

impl Message for DeleteMessage {
    type Result = bool;
}

#[inline]
const fn calculate_payload(marker: bool, payload: u8) -> u8 {
    payload | ((marker as u8) << 7)
}
