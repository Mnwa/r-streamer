use crate::client::clients::ClientsRefStorage;
use crate::client::rtc_actor::RtcActor;
use crate::client::sessions::SessionStorageItem;
use crate::rtp::core::RtpHeader;
use crate::sdp::media::{MediaAddrMessage, MediaList, MediaUserMessage, MediaUserStorage};
use crate::{
    client::{
        actor::ClientActor,
        group::GroupId,
        sessions::{Session, SessionMessage, SessionsStorage},
    },
    dtls::is_dtls,
    server::{crypto::Crypto, meta::ServerMeta},
    stun::{parse_stun_binding_request, write_stun_success_response, StunBindingRequest},
};
use actix::prelude::*;
use futures::{FutureExt, TryFutureExt};
use log::{info, warn};
use smallvec::SmallVec;
use std::{collections::HashMap, future::Future, net::SocketAddr, pin::Pin, sync::Arc, task::Poll};
use tokio::{net::UdpSocket, time::Duration};

pub struct UdpRecv {
    send: Addr<UdpSend>,
    dtls: Addr<ClientActor>,
    rtc: Addr<RtcActor>,
    data: Arc<ServerData>,
    sessions: SessionsStorage,
    media_sessions: MediaUserStorage,
}

impl Actor for UdpRecv {
    type Context = Context<Self>;
}

impl UdpRecv {
    pub fn new(
        recv: Arc<UdpSocket>,
        send: Addr<UdpSend>,
        dtls: Addr<ClientActor>,
        rtc: Addr<RtcActor>,
        data: Arc<ServerData>,
    ) -> Addr<UdpRecv> {
        UdpRecv::create(|ctx| {
            ctx.set_mailbox_capacity(1024);

            ctx.add_message_stream(futures::stream::unfold(recv, |recv| async move {
                let mut message = DataPacket::with_capacity(1200);
                unsafe { message.set_len(1200) }

                match recv.recv_from(&mut message).await {
                    Ok((n, addr)) => {
                        message.truncate(n);
                        let message = WebRtcRequest::from((message, addr));

                        Some((message, recv))
                    }
                    Err(err) => {
                        warn!("could not receive UDP message: {}", err);
                        None
                    }
                }
            }));

            ctx.add_stream(futures::stream::unfold(ClearData, |message| async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Some((message, message))
            }));

            UdpRecv {
                send,
                dtls,
                data,
                rtc,
                sessions: HashMap::new(),
                media_sessions: HashMap::new(),
            }
        })
    }
}

impl Handler<ServerDataRequest> for UdpRecv {
    type Result = ServerData;

    fn handle(&mut self, _: ServerDataRequest, _ctx: &mut Context<Self>) -> Self::Result {
        ServerData::clone(&self.data)
    }
}

impl Handler<WebRtcRequest> for UdpRecv {
    type Result = ();

    fn handle(&mut self, item: WebRtcRequest, ctx: &mut Context<Self>) {
        match item {
            WebRtcRequest::Stun(req, addr) => {
                let session = Session::new(req.server_user.clone(), req.remote_user.clone());

                if let Some(SessionStorageItem {
                    group_id,
                    is_sender,
                    ..
                }) = self.sessions.get_mut(&session).copied()
                {
                    let media = self
                        .media_sessions
                        .get(&req.remote_user)
                        .map(MediaList::clone);

                    if let Some(media) = media {
                        ctx.spawn(
                            self.dtls
                                .send(MediaAddrMessage(addr, media))
                                .inspect_err(|e| warn!("dtls send err: {:?}", e))
                                .map(|_| ())
                                .into_actor(self),
                        );
                    }

                    ctx.spawn(
                        futures::future::try_join(
                            self.send.send(WebRtcRequest::Stun(req, addr)),
                            self.dtls.send(GroupId(group_id, addr, is_sender)),
                        )
                        .inspect_err(|e| warn!("dtls or udp send err: {:?}", e))
                        .map(|_| ())
                        .into_actor(self),
                    );
                }
            }
            WebRtcRequest::Dtls(message, addr) => {
                ctx.spawn(
                    self.dtls
                        .send(WebRtcRequest::Dtls(message, addr))
                        .inspect_err(|e| warn!("dtls send err: {:?}", e))
                        .map(|_| ())
                        .into_actor(self),
                );
            }
            WebRtcRequest::Rtc(message, addr) => {
                self.rtc.do_send(WebRtcRequest::Rtc(message, addr));
            }
            WebRtcRequest::Unknown => warn!("recv unknown request"),
        }
    }
}

impl StreamHandler<ClearData> for UdpRecv {
    fn handle(&mut self, _: ClearData, _ctx: &mut Context<Self>) {
        let sessions_to_remove: Vec<Session> = self
            .sessions
            .iter()
            .filter(|(_, SessionStorageItem { ttl, .. })| match ttl.elapsed() {
                Ok(d) => d > Duration::from_secs(60),
                Err(_e) => false,
            })
            .map(|(s, _)| s.clone())
            .collect();

        sessions_to_remove.into_iter().for_each(|s| {
            self.sessions.remove(&s);
            self.media_sessions.remove(&s.get_client());
        });
    }
}

impl Handler<SessionMessage> for UdpRecv {
    type Result = bool;

    fn handle(
        &mut self,
        SessionMessage(session, item): SessionMessage,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.sessions.insert(session, item);
        true
    }
}

impl Handler<MediaUserMessage> for UdpRecv {
    type Result = ();

    fn handle(
        &mut self,
        MediaUserMessage(user_name, media): MediaUserMessage,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.media_sessions.insert(user_name, media);
    }
}

pub struct UdpSend {
    send: Arc<UdpSocket>,
    data: Arc<ServerData>,
}

impl UdpSend {
    pub fn new(send: Arc<UdpSocket>, data: Arc<ServerData>) -> Addr<Self> {
        Self::create(|ctx| {
            ctx.set_mailbox_capacity(1024);
            Self { send, data }
        })
    }
}

impl Actor for UdpSend {
    type Context = Context<Self>;

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!("stopping")
    }
}

impl Handler<WebRtcRequest> for UdpSend {
    type Result = ();

    fn handle(&mut self, msg: WebRtcRequest, ctx: &mut Context<Self>) -> Self::Result {
        let send = self.send.clone();
        match msg {
            WebRtcRequest::Stun(req, addr) => {
                let mut message_buf: DataPacket = DataPacket::with_capacity(1200);
                unsafe { message_buf.set_len(1200) }

                let n = write_stun_success_response(
                    req.transaction_id,
                    addr,
                    self.data.meta.password.as_bytes(),
                    &mut message_buf,
                );

                let n = match n {
                    Ok(n) => n,
                    Err(e) => {
                        warn!("error on writing stun response: {}", e);
                        return;
                    }
                };

                message_buf.truncate(n);

                ctx.spawn(
                    UdpMassSender(send, addr, message_buf)
                        .inspect_err(|e| {
                            if e.kind() != std::io::ErrorKind::AddrNotAvailable {
                                warn!("err stun: {:?}", e)
                            }
                        })
                        .map(|_| ())
                        .into_actor(self),
                );
            }
            WebRtcRequest::Dtls(message, addr) => {
                ctx.spawn(
                    UdpMassSender(send, addr, message)
                        .inspect_err(|e| warn!("err dtls send: {:?}", e))
                        .map(|_| ())
                        .into_actor(self),
                );
            }
            WebRtcRequest::Rtc(message, addr) => {
                ctx.spawn(
                    UdpMassSender(send, addr, message)
                        .inspect_err(|e| warn!("err rtc send: {:?}", e))
                        .map(|_| ())
                        .into_actor(self),
                );
            }
            WebRtcRequest::Unknown => warn!("send unknown request"),
        }
    }
}

pub async fn create_udp(addr: SocketAddr) -> (Addr<UdpRecv>, Addr<UdpSend>) {
    let server = Arc::new(UdpSocket::bind(addr).await.expect("udp must be up"));
    let meta = ServerMeta::new();
    let crypto = Crypto::init().expect("WebRTC server could not initialize OpenSSL primitives");
    let data = Arc::new(ServerData { meta, crypto });

    let udp_send = UdpSend::new(server.clone(), Arc::clone(&data));
    let client_storage = Arc::new(ClientsRefStorage::default());
    let dtls = ClientActor::new(
        Arc::clone(&data.crypto.ssl_acceptor),
        udp_send.clone(),
        Arc::clone(&client_storage),
    );
    let rtc = RtcActor::new(udp_send.clone(), Arc::clone(&client_storage));
    let udp_recv = UdpRecv::new(server, udp_send.clone(), dtls, rtc, data);

    (udp_recv, udp_send)
}

#[derive(Debug, Clone)]
pub enum WebRtcRequest {
    Stun(StunBindingRequest, SocketAddr),
    Dtls(DataPacket, SocketAddr),
    Rtc(DataPacket, SocketAddr),
    Unknown,
}

impl From<(DataPacket, SocketAddr)> for WebRtcRequest {
    fn from((buf, addr): (DataPacket, SocketAddr)) -> Self {
        if RtpHeader::is_rtp_header(&buf) {
            return WebRtcRequest::Rtc(buf, addr);
        }
        if let Some(stun) = parse_stun_binding_request(&buf) {
            return WebRtcRequest::Stun(stun, addr);
        }
        if is_dtls(&buf) {
            return WebRtcRequest::Dtls(buf, addr);
        }

        WebRtcRequest::Unknown
    }
}

impl Message for WebRtcRequest {
    type Result = ();
}

#[derive(MessageResponse, Clone)]
pub struct ServerData {
    pub crypto: Crypto,
    pub meta: ServerMeta,
}

#[derive(Message)]
#[rtype(result = "ServerData")]
pub struct ServerDataRequest;

#[derive(Copy, Clone, Debug)]
struct ClearData;

impl Message for ClearData {
    type Result = ();
}

struct UdpMassSender(Arc<UdpSocket>, SocketAddr, DataPacket);

impl Future for UdpMassSender {
    type Output = std::io::Result<usize>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.0.as_ref().poll_send_to(cx, &self.2, self.1)
    }
}

pub type DataPacket = SmallVec<[u8; 2048]>;
