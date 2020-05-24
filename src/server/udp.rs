use crate::server::crypto::Crypto;
use crate::server::meta::ServerMeta;
use crate::server::tls::TlsActor;
use crate::stun::{parse_stun_binding_request, write_stun_success_response, StunBindingRequest};
use actix::prelude::*;
use futures::StreamExt;
use log::{info, warn};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::udp::{RecvHalf, SendHalf};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

pub struct UdpRecv {
    send: Arc<Addr<UdpSend>>,
    dtls: Arc<Addr<TlsActor>>,
    data: Arc<ServerData>,
    sessions: HashSet<Session>,
}

impl Actor for UdpRecv {
    type Context = Context<Self>;
}

impl UdpRecv {
    pub fn new(
        recv: RecvHalf,
        send: Arc<Addr<UdpSend>>,
        dtls: Arc<Addr<TlsActor>>,
        data: Arc<ServerData>,
    ) -> Addr<UdpRecv> {
        UdpRecv::create(|ctx| {
            let stream = futures::stream::unfold(recv, |mut server| async move {
                let mut message_buf: Vec<u8> = vec![0; 0x10000];

                match server.recv_from(&mut message_buf).await {
                    Ok((n, addr_from)) => {
                        message_buf.truncate(n);
                        Some(((message_buf, addr_from), server))
                    }
                    Err(err) => {
                        warn!("could not receive UDP message: {}", err);
                        None
                    }
                }
            })
            .map(
                |(message, addr)| match parse_stun_binding_request(&message) {
                    Some(request) => WebRtcRequest::Stun(request, addr),
                    None => WebRtcRequest::Dtls(message, addr),
                },
            );

            ctx.add_stream(stream);

            UdpRecv {
                send,
                dtls,
                data,
                sessions: HashSet::new(),
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

impl StreamHandler<WebRtcRequest> for UdpRecv {
    fn handle(&mut self, item: WebRtcRequest, ctx: &mut Context<Self>) {
        match item {
            WebRtcRequest::Stun(req, addr) => {
                let session = Session {
                    server_user: self.data.meta.user.clone(),
                    client_user: req.remote_user.clone(),
                };
                if self.sessions.contains(&session) {
                    let udp_send = Arc::clone(&self.send);
                    ctx.spawn(
                        async move {
                            if let Err(e) = udp_send.send(WebRtcRequest::Stun(req, addr)).await {
                                warn!("udp recv to udp send: {:#?}", e)
                            }
                        }
                        .into_actor(self),
                    );
                }
            }
            WebRtcRequest::Dtls(message, addr) => {
                let dtls = Arc::clone(&self.dtls);
                ctx.spawn(
                    async move {
                        if let Err(e) = dtls.send(WebRtcRequest::Dtls(message, addr)).await {
                            warn!("udp recv to dtls: {:#?}", e)
                        }
                    }
                    .into_actor(self),
                );
            }
        }
    }
}

impl Handler<Session> for UdpRecv {
    type Result = bool;

    fn handle(&mut self, session: Session, _ctx: &mut Context<Self>) -> Self::Result {
        self.sessions.insert(session)
    }
}

pub struct UdpSend {
    send: Arc<Mutex<SendHalf>>,
    data: Arc<ServerData>,
}

impl UdpSend {
    pub fn new(send: SendHalf, data: Arc<ServerData>) -> Addr<Self> {
        Self::create(|_| Self {
            send: Arc::new(Mutex::new(send)),
            data,
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
        let sender = Arc::clone(&self.send);

        match msg {
            WebRtcRequest::Stun(req, addr) => {
                let mut message_buf: Vec<u8> = vec![0; 0x10000];
                let n = write_stun_success_response(
                    req.transaction_id,
                    addr,
                    self.data.meta.password.as_bytes(),
                    &mut message_buf,
                )
                .unwrap();
                message_buf.truncate(n);
                ctx.spawn(
                    async move {
                        let result = sender.lock().await.send_to(&message_buf, &addr).await;
                        if let Err(e) = result {
                            if e.kind() != std::io::ErrorKind::AddrNotAvailable {
                                warn!("err {:?}", e)
                            }
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Dtls(message, addr) => {
                ctx.spawn(
                    async move {
                        let result = sender.lock().await.send_to(&message, &addr).await;
                        if let Err(e) = result {
                            warn!("err dtls {:?}", e)
                        }
                    }
                    .into_actor(self),
                );
            }
        }
    }
}

pub async fn create_udp(addr: SocketAddr) -> (Arc<Addr<UdpRecv>>, Arc<Addr<UdpSend>>) {
    let server = UdpSocket::bind(addr).await.expect("udp must be up");
    let meta = ServerMeta::new();
    let crypto = Crypto::init().expect("WebRTC server could not initialize OpenSSL primitives");
    let data = Arc::new(ServerData { meta, crypto, addr });

    let (recv, send) = server.split();
    let udp_send = Arc::new(UdpSend::new(send, Arc::clone(&data)));
    let dtls = Arc::new(TlsActor::new(Arc::clone(&data), Arc::clone(&udp_send)));
    let udp_recv = UdpRecv::new(recv, Arc::clone(&udp_send), dtls, data);

    (Arc::new(udp_recv), Arc::clone(&udp_send))
}

#[derive(Debug)]
pub enum WebRtcRequest {
    Stun(StunBindingRequest, SocketAddr),
    Dtls(Vec<u8>, SocketAddr),
}

impl Message for WebRtcRequest {
    type Result = ();
}

#[derive(MessageResponse, Clone)]
pub struct ServerData {
    pub crypto: Crypto,
    pub meta: ServerMeta,
    pub addr: SocketAddr,
}

#[derive(Message)]
#[rtype(result = "ServerData")]
pub struct ServerDataRequest;

#[derive(Hash, Eq, PartialEq)]
pub struct Session {
    pub server_user: String,
    pub client_user: String,
}

impl Message for Session {
    type Result = bool;
}
