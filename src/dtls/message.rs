use crate::server::udp::WebRtcRequest;
use actix::Message;
use std::net::SocketAddr;

#[derive(Debug)]
pub struct DtlsMessage(MessageType, Vec<u8>, SocketAddr);
impl DtlsMessage {
    pub fn create_incoming(message: Vec<u8>, addr: SocketAddr) -> Self {
        DtlsMessage(MessageType::Incoming, message, addr)
    }
    pub fn create_outgoing(message: Vec<u8>, addr: SocketAddr) -> Self {
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

#[derive(Debug)]
pub enum MessageType {
    Incoming,
    Outgoing,
}
