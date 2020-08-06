use crate::rtp::srtp::ErrorParse::UnsupportedRequest;
use crate::{
    rtp::srtp::{ErrorParse, SrtpTransport},
    server::udp::WebRtcRequest,
};
use rtp_rs::RtpReader;

pub fn parse_rtp(buf: &[u8]) -> Option<RtpReader> {
    RtpReader::new(buf).ok()
}

pub fn rtp_processor(
    request: WebRtcRequest,
    transport: Option<&mut SrtpTransport>,
) -> Result<Vec<u8>, ErrorParse> {
    if let WebRtcRequest::Rtc(mut message, _addr) = request {
        if let Some(transport) = transport {
            message = transport.unprotect(&message)?;
        }

        // let rtp_header = RtpHeader::from_buf(&message)?;

        // if rtp_header.payload == 111 {
        //     return Err(UnsupportedFormat);
        // }

        return Ok(message);
    }
    Err(UnsupportedRequest(format!(
        "unsupported request {}, when waiting rtc",
        request.get_type()
    )))
}

pub fn rtcp_processor(
    request: WebRtcRequest,
    transport: Option<&mut SrtpTransport>,
) -> Result<Vec<u8>, ErrorParse> {
    if let WebRtcRequest::Rtc(mut message, _addr) = request {
        if let Some(transport) = transport {
            message = transport.unprotect_rctp(&message)?;
        }
        return Ok(message);
    }
    Err(UnsupportedRequest(format!(
        "unsupported request {}, when waiting rtc",
        request.get_type()
    )))
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

pub struct RtpHeader {
    pub marker: bool,
    pub payload: u8,
}

impl RtpHeader {
    pub fn from_buf(buf: &[u8]) -> Result<RtpHeader, ErrorParse> {
        if buf.len() <= 2 {
            return Err(ErrorParse::UnsupportedFormat);
        }

        let version = buf[0] >> 6;

        if version != 2 {
            return Err(ErrorParse::UnsupportedFormat);
        }

        Ok(RtpHeader {
            marker: (buf[1] >> 7) == 1,
            payload: buf[1] & 127, // 127 - 01111111 in binary format
        })
    }
}
