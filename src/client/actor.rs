use crate::client::clients::ClientStateStatus;
use crate::sdp::media::MediaAddrMessage;
use crate::{
    client::{
        clients::ClientsRefStorage,
        dtls::{extract_dtls, push_dtls},
        group::{Group, GroupId},
    },
    dtls::{
        connector::connect,
        message::{DtlsMessage, MessageType},
    },
    server::udp::{UdpSend, WebRtcRequest},
};
use actix::prelude::*;
use log::{info, warn};
use openssl::ssl::SslAcceptor;
use std::{net::SocketAddr, sync::Arc};
use tokio::time::{timeout, Duration};

pub struct ClientActor {
    client_storage: Arc<ClientsRefStorage>,
    groups: Group,
    ssl_acceptor: Arc<SslAcceptor>,
    udp_send: Addr<UdpSend>,
}

impl ClientActor {
    pub fn new(
        ssl_acceptor: Arc<SslAcceptor>,
        udp_send: Addr<UdpSend>,
        client_storage: Arc<ClientsRefStorage>,
    ) -> Addr<ClientActor> {
        ClientActor::create(|ctx| {
            ctx.set_mailbox_capacity(1024);
            ClientActor {
                ssl_acceptor,
                udp_send,
                client_storage,
                groups: Group::default(),
            }
        })
    }
}

impl Actor for ClientActor {
    type Context = Context<Self>;

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        warn!("client actor died")
    }
}

impl Handler<WebRtcRequest> for ClientActor {
    type Result = ();

    fn handle(&mut self, msg: WebRtcRequest, ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            WebRtcRequest::Dtls(message, addr) => {
                let client_ref = Arc::clone(self.client_storage.write().entry(addr).or_default());
                let acceptor = Arc::clone(&self.ssl_acceptor);

                let self_addr = ctx.address();

                let incoming_writer = Arc::clone(&client_ref.get_channels().incoming_writer);

                if !client_ref.dtls_connected() {
                    ctx.add_message_stream(client_ref.get_channels().outgoing_stream(addr));
                    client_ref.make_connected()
                }

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

                        let state = client_ref.get_state().lock().await;
                        let status = state.get_status();
                        drop(state);

                        match status {
                            ClientStateStatus::New => {
                                match connect(client_ref.clone(), acceptor).await {
                                    Err(e) => {
                                        warn!("{} connect err: {}", addr, e);
                                        match self_addr.send(DeleteMessage(addr)).await {
                                            Err(e) => warn!("delete err: {}", e),
                                            Ok(is_deleted) => info!("deleted {}", is_deleted),
                                        }
                                    }
                                    Ok(srtp_transport) => {
                                        *client_ref.get_srtp().lock() = Some(srtp_transport);
                                        info!("client {} connected", addr)
                                    }
                                }
                            }
                            ClientStateStatus::Connected => {
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
                            ClientStateStatus::Shutdown => {}
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Rtc(_, _) => {
                warn!("stun could not be accepted in client actor")
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
        let client = self.client_storage.write().remove(&addr);

        if let Some(client) = client {
            self.groups.remove_sender(addr);
            client.delete();
            let mut receivers = client.get_receivers().write();
            *receivers = receivers
                .clone()
                .into_iter()
                .filter(|(_, recv)| !recv.is_deleted())
                .collect();
        }
        true
    }
}

impl Handler<GroupId> for ClientActor {
    type Result = ();

    fn handle(
        &mut self,
        GroupId(group_id, addr, is_sender): GroupId,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let last_sender_addr = self.groups.insert_or_get_sender(group_id, addr, is_sender);
        if last_sender_addr == addr && !is_sender {
            return;
        }

        let client_storage = &*self.client_storage.read();

        let receiver_ref = client_storage.get(&addr);
        let sender_ref = client_storage.get(&last_sender_addr);

        if let (Some(receiver_ref), Some(sender_ref)) = (receiver_ref, sender_ref) {
            let mut receivers = sender_ref.get_receivers().write();
            if last_sender_addr != addr && is_sender {
                *receivers = sender_ref
                    .get_receivers()
                    .read()
                    .clone()
                    .into_iter()
                    .filter(|(recv_addr, _)| *recv_addr != addr)
                    .map(|(recv_addr, recv_client)| {
                        *recv_client.get_sender_addr().write() = Some(addr);
                        (recv_addr, recv_client)
                    })
                    .collect();
            } else {
                *receiver_ref.get_sender_addr().write() = Some(last_sender_addr);
                receivers.insert(addr, Arc::clone(receiver_ref));
            }
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
        if let Some(c) = self.client_storage.read().get(&addr) {
            let mut rtp_runtime_storage = c.get_rtp_runtime_storage().lock();
            media.get_frequencies().into_iter().for_each(|f| {
                rtp_runtime_storage.entry(f).or_default();
            });
            let mut c_media = c.get_media().write();
            *c_media = Some(media);
        }
    }
}

struct DeleteMessage(SocketAddr);

impl Message for DeleteMessage {
    type Result = bool;
}
