use actix::MailboxError;
use bytes::BytesMut;
use openssl::{error::ErrorStack, ssl::SslRef};
use srtp::{CryptoPolicy, Error as ErrorSrtp, Srtp, SsrcType};
use std::net::SocketAddr;
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

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
            Some(profile) => {
                return Err(ErrorParse::UnsupportedProfile(profile.name().to_string()))
            }
            None => return Err(ErrorParse::UnsupportedProfile("empty".to_string())),
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

    pub fn protect(&mut self, buf: &mut BytesMut) -> Result<(), ErrorParse> {
        self.server.protect(buf).map_err(|e| e.into())
    }

    pub fn protect_rtcp(&mut self, buf: &mut BytesMut) -> Result<(), ErrorParse> {
        self.server.protect_rtcp(buf).map_err(|e| e.into())
    }

    pub fn unprotect(&mut self, buf: &mut BytesMut) -> Result<(), ErrorParse> {
        self.client.unprotect(buf).map_err(|e| e.into())
    }

    pub fn unprotect_rctp(&mut self, buf: &mut BytesMut) -> Result<(), ErrorParse> {
        self.client.unprotect_rtcp(buf).map_err(|e| e.into())
    }
}

#[derive(Debug)]
pub enum ErrorParse {
    Openssl(ErrorStack),
    Srtp(ErrorSrtp),
    UnsupportedProfile(String),
    UnsupportedFormat,
    ClientNotReady(SocketAddr),
    ActorDead(MailboxError),
}

impl ErrorParse {
    pub fn should_ignored(&self) -> bool {
        matches!(self, ErrorParse::UnsupportedFormat)
    }
}

impl Display for ErrorParse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorParse::Openssl(e) => write!(f, "{}", e),
            ErrorParse::Srtp(e) => write!(f, "{:?}", e),
            ErrorParse::UnsupportedProfile(e) => write!(f, "Unsupported profile: {}", e),
            ErrorParse::UnsupportedFormat => write!(f, "Unsupported format: its ok"),
            ErrorParse::ClientNotReady(addr) => write!(f, "Client not ready: {}", addr),
            ErrorParse::ActorDead(e) => write!(f, "Actor is dead: {:?}", e),
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

impl From<MailboxError> for ErrorParse {
    fn from(e: MailboxError) -> Self {
        ErrorParse::ActorDead(e)
    }
}
