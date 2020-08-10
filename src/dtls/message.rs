use crate::server::udp::WebRtcRequest;
use actix::Message;
use bytes::BytesMut;
use std::net::SocketAddr;

#[derive(Debug)]
pub struct DtlsMessage(MessageType, BytesMut, SocketAddr);
impl DtlsMessage {
    #[allow(dead_code)]
    pub fn create_incoming(message: BytesMut, addr: SocketAddr) -> Self {
        DtlsMessage(MessageType::Incoming, message, addr)
    }
    pub fn create_outgoing(message: BytesMut, addr: SocketAddr) -> Self {
        DtlsMessage(MessageType::Outgoing, message, addr)
    }

    pub fn get_type(&self) -> &MessageType {
        &self.0
    }
    pub fn into_webrtc(self) -> WebRtcRequest {
        WebRtcRequest::Dtls(self.1, self.2)
    }
}

impl Message for DtlsMessage {
    type Result = ();
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum MessageType {
    Incoming,
    Outgoing,
}
