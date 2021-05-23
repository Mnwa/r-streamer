use crate::rtp::srtp::ErrorParse;
use byteorder::ByteOrder;

#[inline]
pub fn is_rtcp(buf: &[u8]) -> bool {
    if !RtpHeader::is_rtp_header(buf) {
        return false;
    }
    if buf[1] < 200 || buf[1] > 206 {
        return false;
    }

    true
}

#[derive(Copy, Clone, Debug)]
pub struct RtpHeader {
    pub marker: bool,
    pub payload: u8,
    pub sequence: u16,
    pub timestamp: u32,
}

impl RtpHeader {
    #[inline]
    pub fn is_rtp_header(buf: &[u8]) -> bool {
        if buf.len() <= 12 {
            return false;
        }

        let version = buf[0] >> 6;

        if version != 2 {
            return false;
        }

        true
    }
    pub fn from_buf(buf: &[u8]) -> Result<RtpHeader, ErrorParse> {
        if !RtpHeader::is_rtp_header(buf) {
            return Err(ErrorParse::UnsupportedFormat);
        }

        let sequence = byteorder::NetworkEndian::read_u16(&buf[2..4]);
        let timestamp = byteorder::NetworkEndian::read_u32(&buf[4..8]);

        Ok(RtpHeader {
            marker: (buf[1] >> 7) == 1,
            payload: buf[1] & 0b01111111,
            sequence,
            timestamp,
        })
    }
}
