use crate::client::clients::ClientSafeRef;
use crate::{
    client::clients::{ClientError, ClientState},
    rtp::srtp::SrtpTransport,
};
use log::warn;
use openssl::ssl::SslAcceptor;
use std::ops::DerefMut;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tokio_openssl::accept;

pub async fn connect(
    client: ClientSafeRef,
    ssl_acceptor: Arc<SslAcceptor>,
) -> Result<(), ClientError> {
    let mut state = client.get_state().lock().await;

    let ssl_stream = match std::mem::replace(state.deref_mut(), ClientState::Shutdown) {
        ClientState::New(stream) => timeout(Duration::from_secs(10), accept(&ssl_acceptor, stream))
            .await
            .map_err(|_| std::io::ErrorKind::TimedOut)?,
        ClientState::Connected(_, _) => return Err(ClientError::AlreadyConnected),
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

    *state = ClientState::Connected(ssl_stream, srtp_transport);
    Ok(())
}
