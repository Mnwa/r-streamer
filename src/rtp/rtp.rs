use crate::rtp::srtp::{ErrorParse, SrtpTransport};
use crate::server::udp::WebRtcRequest;
use rtp_rs::RtpReader;

pub fn parse_rtp(buf: &[u8]) -> Option<RtpReader> {
    RtpReader::new(buf).ok()
}

pub fn rtp_handler(
    request: WebRtcRequest,
    transport: Option<&mut SrtpTransport>,
) -> Result<(), ErrorParse> {
    if let WebRtcRequest::Rtc(mut message, _addr) = request {
        if let Some(transport) = transport {
            message = transport.unprotect(&message)?;
        }
        println!("{:?}", message);
        return Ok(());
    }
    unimplemented!()
}

pub fn rctp_handler(
    request: WebRtcRequest,
    transport: Option<&mut SrtpTransport>,
) -> Result<(), ErrorParse> {
    if let WebRtcRequest::Rtc(mut message, _addr) = request {
        if let Some(transport) = transport {
            message = transport.unprotect_rctp(&message)?;
        }
        println!("{:?}", message);
        return Ok(());
    }
    unimplemented!()
}

pub fn is_rtcp(buf: &[u8]) -> bool {
    if buf.len() < 4 {
        return false;
    }

    if (buf[0] >> 6) != 2 {
        return false;
    }
    if buf[1] < 200 || buf[1] > 206 {
        return false;
    }

    true
}
