pub mod connector;
pub mod message;

pub fn is_dtls(buf: &[u8]) -> bool {
    buf[0] >= 20 && buf[0] <= 64
}
