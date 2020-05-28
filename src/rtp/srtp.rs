use bytes::BytesMut;
use openssl::error::ErrorStack;
use openssl::ssl::SslRef;
use srtp::{CryptoPolicy, Error as ErrorSrtp, Srtp, SsrcType};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub struct SrtpTransport {
    client: Srtp,
    server: Srtp,
}

impl SrtpTransport {
    pub fn new(ssl: &SslRef) -> Result<SrtpTransport, ErrorParse> {
        let (rtp_policy, rtcp_policy) = match ssl.selected_srtp_profile() {
            Some(profile) if profile.name() == "SRTP_AES128_CM_SHA1_80" => (
                CryptoPolicy::AesCm128HmacSha1Bit80,
                CryptoPolicy::AesCm128HmacSha1Bit80,
            ),
            Some(profile) => return Err(ErrorParse::UnsupportedProfile(profile.name())),
            None => return Err(ErrorParse::UnsupportedProfile("empty")),
        };

        let mut dtls_buf = vec![0; rtp_policy.master_len() * 2];
        ssl.export_keying_material(dtls_buf.as_mut_slice(), "EXTRACTOR-dtls_srtp", None)?;

        let pair = rtp_policy.extract_keying_material(dtls_buf.as_mut_slice());

        let srtp_incoming = Srtp::new(SsrcType::AnyInbound, rtp_policy, rtcp_policy, pair.client)?;
        let srtp_outcoming =
            Srtp::new(SsrcType::AnyOutbound, rtp_policy, rtcp_policy, pair.server)?;

        Ok(SrtpTransport {
            client: srtp_incoming,
            server: srtp_outcoming,
        })
    }

    pub fn protect(&mut self, buf: &[u8]) -> Result<(), ErrorParse> {
        let mut buf = BytesMut::from(buf);
        self.server.protect(&mut buf).map_err(|e| e.into())
    }

    pub fn unprotect(&mut self, buf: &[u8]) -> Result<Vec<u8>, ErrorParse> {
        let mut buf = BytesMut::from(buf);
        self.client.unprotect(&mut buf)?;
        Ok(buf.to_vec())
    }
}

#[derive(Debug)]
pub enum ErrorParse {
    Openssl(ErrorStack),
    Srtp(ErrorSrtp),
    UnsupportedProfile(&'static str),
}

impl Display for ErrorParse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorParse::Openssl(e) => write!(f, "{}", e),
            ErrorParse::Srtp(e) => write!(f, "{:?}", e),
            ErrorParse::UnsupportedProfile(e) => write!(f, "Unsupported profile: {}", e),
        }
    }
}

impl Error for ErrorParse {}

impl From<ErrorStack> for ErrorParse {
    fn from(e: ErrorStack) -> Self {
        ErrorParse::Openssl(e)
    }
}

impl From<ErrorSrtp> for ErrorParse {
    fn from(e: ErrorSrtp) -> Self {
        ErrorParse::Srtp(e)
    }
}
