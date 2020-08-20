use crate::rtp::core::RtpHeader;
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
use std::ops::DerefMut;
use std::time::Instant;
use std::{net::SocketAddr, sync::Arc};
use tokio::time::{timeout, Duration};

pub struct ClientActor {
    client_storage: ClientsRefStorage,
    groups: Group,
    ssl_acceptor: Arc<SslAcceptor>,
    udp_send: Addr<UdpSend>,
}

impl ClientActor {
    pub fn new(ssl_acceptor: Arc<SslAcceptor>, udp_send: Addr<UdpSend>) -> Addr<ClientActor> {
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
                let client_ref = Arc::clone(self.client_storage.entry(addr).or_default());
                let acceptor = Arc::clone(&self.ssl_acceptor);

                let self_addr = ctx.address();

                let incoming_writer = Arc::clone(&client_ref.get_channels().incoming_writer);

                ctx.add_message_stream(client_ref.get_channels().outgoing_stream(addr));

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

                        let mut state = client_ref.get_state().lock().await;

                        match state.deref_mut() {
                            ClientState::New(_) => {
                                drop(state);
                                if let Err(e) = connect(client_ref.clone(), acceptor).await {
                                    warn!("connect err: {}", e);
                                    match self_addr.send(DeleteMessage(addr)).await {
                                        Err(e) => warn!("delete err: {}", e),
                                        Ok(is_deleted) => info!("deleted {}", is_deleted),
                                    }
                                }
                                println!("connected");
                            }
                            ClientState::Connected(_, _) => {
                                drop(state);
                                let mut buf = vec![0; 512];
                                let result = timeout(
                                    Duration::from_millis(10),
                                    extract_dtls(client_ref.clone(), &mut buf),
                                )
                                .await
                                .map_err(|_| std::io::ErrorKind::TimedOut.into())
                                .and_then(|r| r);

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
            WebRtcRequest::Rtc(mut message, addr) => {
                let start = Instant::now();
                let udp_send = self.udp_send.clone();
                let client_ref = Arc::clone(self.client_storage.entry(addr).or_default());
                let is_rtcp = is_rtcp(&message);

                ctx.spawn(
                    async move {
                        let rtp_header = RtpHeader::from_buf(&message)?;

                        let mut state = client_ref.get_state().lock().await;
                        let media = client_ref.get_media().lock().await;

                        let codec = if let ClientState::Connected(_, srtp) = state.deref_mut() {
                            if is_rtcp {
                                srtp.unprotect_rctp(&mut message)?;
                                None
                            } else {
                                srtp.unprotect(&mut message)?;

                                let rtp_header = RtpHeader::from_buf(&message)?;

                                if rtp_header.payload == 111 {
                                    return Err(ErrorParse::UnsupportedFormat);
                                }
                                media
                                    .as_ref()
                                    .and_then(|media| media.get_name(&rtp_header.payload))
                            }
                        } else {
                            return Err(ErrorParse::ClientNotReady(addr));
                        };

                        drop(state);

                        iter(client_ref.get_receivers().lock().await.iter())
                            .map(|(r_addr, recv)| (*r_addr, recv.clone()))
                            .then(|(r_addr, recv)| {
                                let mut message = message.clone();

                                async move {
                                    let mut state = recv.get_state().lock().await;

                                    if let ClientState::Connected(_, srtp) = state.deref_mut() {
                                        if is_rtcp {
                                            srtp.protect_rtcp(&mut message)?;
                                        } else {
                                            srtp.protect(&mut message)?;

                                            let media = recv.get_media().lock().await;

                                            let payload = codec.and_then(|codec| {
                                                media
                                                    .as_ref()
                                                    .and_then(|media| media.get_id(codec))
                                                    .copied()
                                            });

                                            if let Some(payload) = payload {
                                                message[1] =
                                                    calculate_payload(rtp_header.marker, payload);
                                            }
                                        }

                                        Ok((message, r_addr))
                                    } else {
                                        Err(ErrorParse::ClientNotReady(addr))
                                    }
                                }
                            })
                            .try_for_each_concurrent(None, move |(message, addr)| {
                                udp_send
                                    .send(WebRtcRequest::Rtc(message, addr))
                                    .map_err(ErrorParse::from)
                            })
                            .await?;

                        Ok(())
                    }
                    .inspect(move |_| println!("{:?}", start.elapsed()))
                    .inspect_err(move |e| {
                        if !e.should_ignored() {
                            warn!("processor err: {:?} is_rtcp: {}", e, is_rtcp)
                        }
                    })
                    .map(|_| ())
                    .into_actor(self),
                );

                // if !addresses.is_empty() {
                //     ctx.spawn(
                //         client
                //             .lock_owned()
                //             .map(move |mut client| {
                //                 if let ClientState::Connected(_, srtp) = &mut client.state {
                //                     if is_rtcp {
                //                         srtp.unprotect_rctp(&mut message)?;
                //                     } else {
                //                         srtp.unprotect(&mut message)?;
                //
                //                         // let rtp_header = RtpHeader::from_buf(&message)?;
                //                         //
                //                         // if rtp_header.payload == 111 {
                //                         //     return Err(ErrorParse::UnsupportedFormat);
                //                         // }
                //                     }
                //                     Ok(message)
                //                 } else {
                //                     Err(ErrorParse::ClientNotReady(addr))
                //                 }
                //             })
                //             .and_then(move |message| {
                //                 iter(addresses)
                //                     .then(move |(addr, (client, payload))| {
                //                         client
                //                             .lock_owned()
                //                             .map(move |client| (addr, (client, payload)))
                //                     })
                //                     .map(move |(addr, (mut client, payload))| {
                //                         let mut message = message.clone();
                //                         if let ClientState::Connected(_, srtp) = &mut client.state {
                //                             if is_rtcp {
                //                                 srtp.protect_rtcp(&mut message)?;
                //                             } else {
                //                                 srtp.protect(&mut message)?;
                //                                 message[1] = payload;
                //                             }
                //                             Ok((message, addr))
                //                         } else {
                //                             Err(ErrorParse::ClientNotReady(addr))
                //                         }
                //                     })
                //                     .try_for_each_concurrent(None, move |(message, addr)| {
                //                         udp_send
                //                             .send(WebRtcRequest::Rtc(message, addr))
                //                             .map_err(ErrorParse::from)
                //                     })
                //                     .map_err(ErrorParse::from)
                //             })
                //             .inspect_err(move |e| {
                //                 if !e.should_ignored() {
                //                     warn!("processor err: {:?} is_rtcp: {}", e, is_rtcp)
                //                 }
                //             })
                //             .map(move |_| println!("{:?}", start.elapsed()))
                //             .into_actor(self),
                //     );
                // }
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
        ctx: &mut Context<Self>,
    ) -> Self::Result {
        let client = self.client_storage.remove(&addr);

        if let Some(client) = client {
            self.groups.remove_sender(addr);
            ctx.spawn(
                async move {
                    client.delete();
                    let mut receivers = client.get_receivers().lock().await;
                    *receivers = iter(receivers.clone())
                        .filter(|(_, recv)| futures::future::ready(!recv.is_deleted()))
                        .collect()
                        .await;
                }
                .into_actor(self),
            );
        }
        true
    }
}

impl Handler<GroupId> for ClientActor {
    type Result = ();

    fn handle(
        &mut self,
        GroupId(group_id, addr): GroupId,
        ctx: &mut Context<Self>,
    ) -> Self::Result {
        let sender_addr = self.groups.insert_or_get_sender(group_id, addr);
        let result = self
            .client_storage
            .get(&addr)
            .cloned()
            .map(|client_ref| (client_ref, sender_addr))
            .filter(|(_, sender_addr)| *sender_addr != addr)
            .and_then(|(client_ref, sender_addr)| {
                self.client_storage
                    .get_mut(&sender_addr)
                    .cloned()
                    .map(|sender_ref| (client_ref, sender_ref))
            });

        if let Some((client_ref, sender_ref)) = result {
            ctx.spawn(
                async move {
                    let mut receivers = sender_ref.get_receivers().lock().await;
                    receivers.insert(addr, client_ref);
                }
                .into_actor(self),
            );
        }
    }
}

impl Handler<MediaAddrMessage> for ClientActor {
    type Result = ();

    fn handle(
        &mut self,
        MediaAddrMessage(addr, media): MediaAddrMessage,
        ctx: &mut Context<Self>,
    ) -> Self::Result {
        if let Some(c) = self.client_storage.get_mut(&addr).cloned() {
            ctx.spawn(
                async move {
                    let mut c_media = c.get_media().lock().await;
                    *c_media = Some(media);
                }
                .into_actor(self),
            );
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
