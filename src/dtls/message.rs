use crate::server::udp::{DataPacket, WebRtcRequest};
use actix::Message;
use std::net::SocketAddr;

#[derive(Debug)]
pub struct DtlsMessage(MessageType, DataPacket, SocketAddr);
impl DtlsMessage {
    pub fn create_outgoing(message: DataPacket, addr: SocketAddr) -> Self {
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
