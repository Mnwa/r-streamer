use crate::client::clients::ClientSafeRef;
use crate::server::udp::DataPacket;
use crate::{
    client::clients::{ClientError, ClientState},
    client::stream::IncomingWriter,
};
use futures::prelude::*;
use std::ops::DerefMut;
use tokio::io::AsyncReadExt;

pub async fn push_dtls(
    incoming_writer: &mut IncomingWriter,
    buf: DataPacket,
) -> Result<(), ClientError> {
    incoming_writer.send(buf).await.map_err(|e| e.into())
}

pub async fn extract_dtls(client: ClientSafeRef, buf: &mut [u8]) -> Result<usize, ClientError> {
    if let ClientState::Connected(ssl_stream) = client.get_state().lock().await.deref_mut() {
        return ssl_stream.read(buf).await.map_err(|e| e.into());
    }
    Err(ClientError::NotConnected)
}
