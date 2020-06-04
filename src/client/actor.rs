use crate::client::clients::{ClientState, ClientsStorage};
use crate::client::dtls::{extract_dtls, push_dtls};
use crate::client::group::{GroupId, GroupsAddrStorage, GroupsStorage};
use crate::dtls::connector::connect;
use crate::dtls::message::{DtlsMessage, MessageType};
use crate::rtp::rtp::{is_rtcp, rtcp_processor, rtp_processor};
use crate::server::udp::{UdpSend, WebRtcRequest};
use actix::prelude::*;
use log::warn;
use openssl::ssl::SslAcceptor;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{timeout, Duration};

pub struct ClientActor {
    client_storage: ClientsStorage,
    group_storage: GroupsStorage,
    group_addr_storage: GroupsAddrStorage,
    ssl_acceptor: Arc<SslAcceptor>,
    udp_send: Arc<Addr<UdpSend>>,
}

impl ClientActor {
    pub fn new(ssl_acceptor: Arc<SslAcceptor>, udp_send: Arc<Addr<UdpSend>>) -> Addr<ClientActor> {
        ClientActor::create(|_| ClientActor {
            ssl_acceptor,
            udp_send,
            client_storage: ClientsStorage::new(),
            group_storage: GroupsStorage::new(),
            group_addr_storage: GroupsAddrStorage::new(),
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
                                println!("{:?}", result);
                            }
                            ClientState::Shutdown => {}
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Rtc(message, addr) => {
                let udp_send = Arc::clone(&self.udp_send);
                let client = self.client_storage.entry(addr).or_default().get_client();
                let addresses = match self.group_storage.get(&addr) {
                    Some(group_id) => match self.group_addr_storage.get(group_id) {
                        Some(addresses) => Some(
                            addresses
                                .clone()
                                .into_iter()
                                .filter(|g_addr| *g_addr != addr)
                                .collect::<Vec<SocketAddr>>(),
                        ),
                        None => None,
                    },
                    None => None,
                };

                ctx.spawn(
                    async move {
                        let mut client_unlocked = client.lock().await;
                        if let ClientState::Connected(_, srtp) = &mut client_unlocked.state {
                            let message_processed = if is_rtcp(&message) {
                                match rtcp_processor(WebRtcRequest::Rtc(message, addr), Some(srtp))
                                {
                                    Ok(message) => Some(message),
                                    Err(e) => {
                                        warn!("rtp err: {}", e);
                                        None
                                    }
                                }
                            } else {
                                match rtp_processor(WebRtcRequest::Rtc(message, addr), Some(srtp)) {
                                    Ok(message) => Some(message),
                                    Err(e) => {
                                        warn!("rtp err: {}", e);
                                        None
                                    }
                                }
                            };

                            if let Some(message) = message_processed {
                                if let Some(addresses) = addresses {
                                    let web_rtc_requests: Vec<Request<UdpSend, WebRtcRequest>> =
                                        addresses
                                            .into_iter()
                                            .map(|addr| {
                                                udp_send
                                                    .send(WebRtcRequest::Rtc(message.clone(), addr))
                                            })
                                            .collect();

                                    let is_sent = futures::future::join_all(web_rtc_requests)
                                        .await
                                        .into_iter()
                                        .collect::<Result<Vec<()>, MailboxError>>();

                                    if let Err(e) = is_sent {
                                        warn!("udp send err: {}", e)
                                    }
                                }
                            }
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Stun(_, _) => unimplemented!(),
        }
    }
}

impl Handler<DtlsMessage> for ClientActor {
    type Result = ();

    fn handle(&mut self, item: DtlsMessage, ctx: &mut Context<Self>) {
        match item.get_type() {
            MessageType::Incoming => unimplemented!(),
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
        self.client_storage.remove(&addr).is_some()
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
            let group_addr_storage = self.group_addr_storage.entry(group_id).or_default();
            group_addr_storage.push(addr);
            self.group_storage.insert(addr, group_id);
        }
    }
}

struct DeleteMessage(SocketAddr);

impl Message for DeleteMessage {
    type Result = bool;
}
