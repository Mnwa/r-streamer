use openssl::{
    error::ErrorStack,
    hash::MessageDigest,
    pkey::{PKey, Private},
    ssl::{SslAcceptor, SslMethod, SslVerifyMode},
    x509::X509,
};

#[derive(Clone)]
pub struct Crypto {
    pub key: PKey<Private>,
    pub x509: X509,
    pub digest: Vec<u8>,
    pub ssl_acceptor: SslAcceptor,
}

impl Crypto {
    pub fn init() -> Result<Crypto, ErrorStack> {
        let x509 = X509::from_pem(include_bytes!("../../cert/cert.pem"))?;
        let key = PKey::private_key_from_pem(include_bytes!("../../cert/key.pem"))?;

        let digest = x509.digest(MessageDigest::sha256())?.to_vec();

        let mut ssl_acceptor_builder = SslAcceptor::mozilla_intermediate(SslMethod::dtls())?;

        ssl_acceptor_builder.set_verify(SslVerifyMode::NONE);

        ssl_acceptor_builder.set_private_key(&key)?;
        ssl_acceptor_builder.set_certificate(&x509)?;
        let ssl_acceptor = ssl_acceptor_builder.build();

        Ok(Crypto {
            key,
            x509,
            digest,
            ssl_acceptor,
        })
    }
}
