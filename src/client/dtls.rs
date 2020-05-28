use crate::client::clients::{Client, ClientError, ClientState};
use crate::client::stream::IncomingWriter;
use futures::prelude::*;
use tokio::prelude::*;

pub async fn push_dtls(
    incoming_writer: &mut IncomingWriter,
    buf: Vec<u8>,
) -> Result<(), ClientError> {
    incoming_writer.send(buf).await.map_err(|e| e.into())
}

pub async fn extract_dtls(client: &mut Client, buf: &mut [u8]) -> Result<usize, ClientError> {
    if let ClientState::Connected(ssl_stream, _) = &mut client.state {
        return ssl_stream.read(buf).await.map_err(|e| e.into());
    }
    Err(ClientError::NotConnected)
}

#[allow(dead_code)]
pub async fn pop_dtls(client: &mut Client) -> Option<Vec<u8>> {
    let mut outgoing_reader = client.channels.outgoing_reader.lock().await;
    outgoing_reader.next().await
}

#[allow(dead_code)]
pub async fn write_message(client: &mut Client, buf: &mut [u8]) -> Result<usize, ClientError> {
    if let ClientState::Connected(ssl_stream, _) = &mut client.state {
        return ssl_stream.write(buf).await.map_err(|e| e.into());
    }
    Err(ClientError::NotConnected)
}
