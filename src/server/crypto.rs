use openssl::ssl::SslSessionCacheMode;
use openssl::{
    error::ErrorStack,
    hash::MessageDigest,
    pkey::{PKey, Private},
    ssl::{SslAcceptor, SslMethod, SslVerifyMode},
    x509::X509,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Crypto {
    pub key: PKey<Private>,
    pub x509: X509,
    pub digest: Vec<u8>,
    pub ssl_acceptor: Arc<SslAcceptor>,
}

impl Crypto {
    pub fn init() -> Result<Crypto, ErrorStack> {
        let x509 = X509::from_pem(include_bytes!("../../cert/cert.pem"))?;
        let key = PKey::private_key_from_pem(include_bytes!("../../cert/key.pem"))?;

        let digest = x509.digest(MessageDigest::sha256())?.to_vec();

        let mut ssl_acceptor_builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::dtls())?;

        ssl_acceptor_builder.set_verify(SslVerifyMode::NONE);

        ssl_acceptor_builder.set_tlsext_use_srtp("SRTP_AES128_CM_SHA1_80")?;

        ssl_acceptor_builder.set_read_ahead(true);

        ssl_acceptor_builder.set_session_cache_mode(SslSessionCacheMode::OFF);

        ssl_acceptor_builder.set_private_key(&key)?;
        ssl_acceptor_builder.set_certificate(&x509)?;

        ssl_acceptor_builder.check_private_key()?;

        let ssl_acceptor = Arc::new(ssl_acceptor_builder.build());

        Ok(Crypto {
            key,
            x509,
            digest,
            ssl_acceptor,
        })
    }
}
