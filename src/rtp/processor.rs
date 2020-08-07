use crate::{
    client::clients::Client, rtp::core::rtcp_processor, rtp::core::rtp_processor,
    rtp::srtp::ErrorParse, server::udp::WebRtcRequest,
};
use actix::prelude::*;

use crate::client::clients::ClientState;
use std::net::SocketAddr;
use tokio::sync::OwnedMutexGuard;

pub struct ProcessorActor;

impl ProcessorActor {
    pub fn new() -> Addr<Self> {
        SyncArbiter::start(4, || ProcessorActor)
    }
}

impl Actor for ProcessorActor {
    type Context = SyncContext<Self>;
}

impl Handler<ProcessRtpPacket> for ProcessorActor {
    type Result = Result<Vec<u8>, ErrorParse>;

    fn handle(&mut self, mut msg: ProcessRtpPacket, _ctx: &mut SyncContext<Self>) -> Self::Result {
        if let ClientState::Connected(_, srtp) = &mut msg.client.state {
            if msg.is_rtcp {
                rtcp_processor(WebRtcRequest::Rtc(msg.message, msg.addr), Some(srtp))
            } else {
                rtp_processor(WebRtcRequest::Rtc(msg.message, msg.addr), Some(srtp))
            }
        } else {
            Err(ErrorParse::ClientNotReady(msg.addr))
        }
    }
}

impl Handler<ProtectRtpPacket> for ProcessorActor {
    type Result = Result<Vec<u8>, ErrorParse>;

    fn handle(&mut self, mut msg: ProtectRtpPacket, _ctx: &mut SyncContext<Self>) -> Self::Result {
        if let ClientState::Connected(_, srtp) = &mut msg.client.state {
            if msg.is_rtcp {
                srtp.protect_rtcp(&msg.message)
            } else {
                srtp.protect(&msg.message).map(|mut m| {
                    m[1] = msg.payload;
                    m
                })
            }
        } else {
            Err(ErrorParse::ClientNotReady(msg.addr))
        }
    }
}

pub struct ProcessRtpPacket {
    pub is_rtcp: bool,
    pub client: OwnedMutexGuard<Client>,
    pub message: Vec<u8>,
    pub addr: SocketAddr,
}

impl Message for ProcessRtpPacket {
    type Result = Result<Vec<u8>, ErrorParse>;
}

pub struct ProtectRtpPacket {
    pub is_rtcp: bool,
    pub client: OwnedMutexGuard<Client>,
    pub message: Vec<u8>,
    pub addr: SocketAddr,
    pub payload: u8,
}

impl Message for ProtectRtpPacket {
    type Result = Result<Vec<u8>, ErrorParse>;
}
