use crate::client::clients::ClientSafeRef;
use crate::{
    client::clients::{ClientError, ClientState},
    rtp::srtp::SrtpTransport,
};
use log::warn;
use openssl::ssl::{Ssl, SslAcceptor};
use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tokio_openssl::SslStream;

pub async fn connect(
    client: ClientSafeRef,
    ssl_acceptor: Arc<SslAcceptor>,
) -> Result<SrtpTransport, ClientError> {
    let mut state = client.get_state().lock().await;

    let ssl = Ssl::new(ssl_acceptor.context())?;

    let ssl_stream: Result<_, ClientError> =
        match std::mem::replace(state.deref_mut(), ClientState::Shutdown) {
            ClientState::New(stream) => {
                let mut async_stream = SslStream::new(ssl, stream)?;
                timeout(
                    Duration::from_secs(10),
                    Pin::new(&mut async_stream).accept(),
                )
                .await
                .map_err(|_| std::io::ErrorKind::TimedOut)??;
                Ok(async_stream)
            }
            ClientState::Connected(_) => return Err(ClientError::AlreadyConnected),
            ClientState::Shutdown => return Err(std::io::ErrorKind::WouldBlock.into()),
        };

    let ssl_stream = match ssl_stream {
        Ok(s) => s,
        Err(e) => {
            warn!("handshake error: {:?}", e);
            return Err(std::io::ErrorKind::ConnectionAborted.into());
        }
    };

    let srtp_transport = SrtpTransport::new(ssl_stream.ssl())?;

    *state = ClientState::Connected(ssl_stream);
    Ok(srtp_transport)
}
