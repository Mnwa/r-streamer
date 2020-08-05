use crate::rtp::core::RtpHeader;
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
    rtp::core::{is_rtcp, rtcp_processor, rtp_processor},
    server::udp::{UdpSend, WebRtcRequest},
};
use actix::prelude::*;
use futures::stream::{iter, StreamExt, TryStreamExt};
use log::warn;
use openssl::ssl::SslAcceptor;
use std::collections::HashMap;
use std::{net::SocketAddr, sync::Arc};
use tokio::time::{timeout, Duration};

pub struct ClientActor {
    client_storage: ClientsRefStorage,
    groups: Group,
    ssl_acceptor: Arc<SslAcceptor>,
    udp_send: Arc<Addr<UdpSend>>,
}

impl ClientActor {
    pub fn new(ssl_acceptor: Arc<SslAcceptor>, udp_send: Arc<Addr<UdpSend>>) -> Addr<ClientActor> {
        ClientActor::create(|_| ClientActor {
            ssl_acceptor,
            udp_send,
            client_storage: ClientsRefStorage::new(),
            groups: Group::default(),
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

                ctx.add_message_stream(client_ref.outgoing_stream(addr));

                let incoming_writer = Arc::clone(&client_ref.get_channels().incoming_writer);
                let client = client_ref.get_client();

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
                                        Ok(is_deleted) => println!("deleted {}", is_deleted),
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
                                        Ok(d) if d => println!("success deleted"),
                                        Ok(_) => println!("fail deleting"),
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
                let udp_send = Arc::clone(&self.udp_send);
                let client_ref = self.client_storage.entry(addr).or_default();
                let client = client_ref.get_client();

                let is_rtcp = is_rtcp(&message);

                let addresses = if is_rtcp {
                    self.groups.get_addressess(addr).map(|addresses| {
                        addresses
                            .into_iter()
                            .filter_map(|g_addr| {
                                self.client_storage
                                    .get(&g_addr)
                                    .map(|client_ref| client_ref.get_client())
                                    .map(|client| (g_addr, (client, 0)))
                            })
                            .collect::<HashMap<_, _>>()
                    })
                } else {
                    let codec = client_ref
                        .get_media()
                        .and_then(|mref| Some((mref, RtpHeader::from_buf(&message).ok()?)))
                        .and_then(|(m, r)| Some((r.marker, m.get_name(&r.payload).cloned()?)));

                    self.groups.get_addressess(addr).map(|addresses| {
                        addresses
                            .into_iter()
                            .filter_map(|g_addr| {
                                self.client_storage
                                    .get(&g_addr)
                                    .and_then(|client_ref| {
                                        Some((client_ref.get_media()?, client_ref.get_client()))
                                    })
                                    .and_then(|(media, client)| {
                                        let (marker, payload) = codec.as_ref()?;
                                        let new_payload = media.get_id(payload).copied()?;
                                        Some((
                                            g_addr,
                                            (client, calculate_payload(*marker, new_payload)),
                                        ))
                                    })
                            })
                            .collect::<HashMap<_, _>>()
                    })
                };

                ctx.spawn(
                    async move {
                        let mut client_unlocked = client.lock().await;
                        if let ClientState::Connected(_, srtp) = &mut client_unlocked.state {
                            let message_processed = if is_rtcp {
                                rtcp_processor(WebRtcRequest::Rtc(message, addr), Some(srtp))
                                    .map_err(|e| {
                                        if !e.should_ignored() {
                                            warn!("rtp err: {}", e);
                                        }
                                        e
                                    })
                                    .ok()
                            } else {
                                rtp_processor(WebRtcRequest::Rtc(message, addr), Some(srtp))
                                    .map_err(|e| {
                                        if !e.should_ignored() {
                                            warn!("rtp err: {}", e);
                                        }
                                        e
                                    })
                                    .ok()
                            };

                            drop(client_unlocked);

                            let addresses_processed = addresses
                                .filter(|addresses| !addresses.is_empty())
                                .and_then(|addresses| Some((addresses, message_processed?)));

                            if let Some((addresses, message)) = addresses_processed {
                                let is_sent = iter(addresses)
                                    .then(|(addr, (client, payload))| {
                                        let mut message = message.clone();
                                        let udp_send = Arc::clone(&udp_send);
                                        async move {
                                            let mut client = client.lock().await;
                                            if let ClientState::Connected(_, srtp) =
                                                &mut client.state
                                            {
                                                if is_rtcp {
                                                    match srtp.protect_rtcp(&message) {
                                                        Ok(m) => message = m,
                                                        Err(e) => warn!("protect rtcp err: {}", e),
                                                    }
                                                } else {
                                                    match srtp.protect(&message) {
                                                        Ok(m) => message = m,
                                                        Err(e) => warn!("protect rtp err: {}", e),
                                                    }

                                                    message[1] = payload;
                                                }
                                            }
                                            drop(client);
                                            udp_send.send(WebRtcRequest::Rtc(message, addr)).await
                                        }
                                    })
                                    .try_collect::<Vec<_>>()
                                    .await;

                                if let Err(e) = is_sent {
                                    warn!("udp send err: {}", e)
                                }
                            }
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Stun(_, _) => warn!("stun could not be accepted in client actor"),
            WebRtcRequest::Unknown => warn!("unknown request"),
        }
    }
}

impl Handler<DtlsMessage> for ClientActor {
    type Result = ();

    fn handle(&mut self, item: DtlsMessage, ctx: &mut Context<Self>) {
        match item.get_type() {
            MessageType::Incoming => warn!("accepted incoming dtls message in the ClientActor"),
            MessageType::Outgoing => {
                let udp_send = Arc::clone(&self.udp_send);
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
            .and_then(|_| {
                if self.groups.remove_client(addr) {
                    Some(())
                } else {
                    None
                }
            })
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
        if self.client_storage.contains_key(&addr) {
            self.groups.insert_client(group_id, addr)
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
            c.set_media(media)
        }
    }
}

struct DeleteMessage(SocketAddr);

impl Message for DeleteMessage {
    type Result = bool;
}

#[inline]
fn calculate_payload(marker: bool, payload: u8) -> u8 {
    if !marker {
        return payload;
    }

    let mut res = 0;

    res = (res << 1 as u8) | 1;

    for i in 9..16 {
        let shift = 7 - (i % 8);
        let bit = payload >> shift & 1;

        res = (res << 1) | bit;
    }

    res
}
