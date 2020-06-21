use crate::rtp::srtp::ErrorParse::{UnsupportedFormat, UnsupportedRequest};
use crate::{
    rtp::srtp::{ErrorParse, SrtpTransport},
    server::udp::WebRtcRequest,
};
use bitreader::BitReader;
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

        let mut reader = BitReader::new(&message);

        reader.skip(2).unwrap();
        reader.skip(1).unwrap();
        reader.skip(1).unwrap();
        reader.skip(4).unwrap();
        reader.skip(1).unwrap();

        // filtering audio messages
        if reader.read_u8(7).unwrap() == 111 {
            return Err(UnsupportedFormat);
        }

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
