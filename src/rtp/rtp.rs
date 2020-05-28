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
    }
    unimplemented!()
}
