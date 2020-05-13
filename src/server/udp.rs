use crate::server::crypto::Crypto;
use crate::server::meta::ServerMeta;
use crate::stun::{parse_stun_binding_request, write_stun_success_response, StunBindingRequest};
use actix::prelude::*;
use futures::StreamExt;
use log::{info, warn};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::udp::{RecvHalf, SendHalf};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

pub struct UdpRecv {
    send: Arc<Addr<UdpSend>>,
    data: Arc<ServerData>,
}

impl Actor for UdpRecv {
    type Context = Context<Self>;
}

impl UdpRecv {
    pub fn new(recv: RecvHalf, send: Arc<Addr<UdpSend>>, data: Arc<ServerData>) -> Addr<UdpRecv> {
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
                    None => WebRtcRequest::Raw(message, addr),
                },
            );

            ctx.add_stream(stream);

            UdpRecv { send, data }
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
                let udp_send = Arc::clone(&self.send);
                ctx.spawn(
                    async move {
                        match udp_send.send(WebRtcRequest::Stun(req, addr)).await {
                            Ok(_) => {}
                            Err(e) => warn!("mailbox error: {:#?}", e),
                        }
                    }
                    .into_actor(self),
                );
            }
            WebRtcRequest::Raw(message, addr) => {
                let udp_send = Arc::clone(&self.send);
                ctx.spawn(
                    async move {
                        match udp_send.send(WebRtcRequest::Raw(message, addr)).await {
                            Ok(_) => {}
                            Err(e) => warn!("mailbox error: {:#?}", e),
                        }
                    }
                    .into_actor(self),
                );
            }
        }
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
        if let WebRtcRequest::Stun(req, addr) = msg {
            let sender = Arc::clone(&self.send);
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
                    if let Ok(n) = result {
                        info!("success sent {} bytes", n)
                    }
                }
                .into_actor(self),
            );
        } else {
            if let WebRtcRequest::Raw(message, _addr) = msg {
                info!("message: {:?}", message)
            }
            warn!("Unhandled request. Ignoring...")
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
    let udp_recv = UdpRecv::new(recv, Arc::clone(&udp_send), data);

    (Arc::new(udp_recv), Arc::clone(&udp_send))
}

#[derive(Debug)]
pub enum WebRtcRequest {
    Stun(StunBindingRequest, SocketAddr),
    Raw(Vec<u8>, SocketAddr),
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
