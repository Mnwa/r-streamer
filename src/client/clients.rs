use crate::client::stream::{ClientSslPackets, ClientSslPacketsChannels};
use crate::dtls::message::DtlsMessage;
use crate::rtp::srtp::SrtpTransport;
use futures::channel::mpsc::SendError;
use futures::lock::Mutex;
use futures::prelude::*;
use futures::stream::FusedStream;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_openssl::SslStream;

#[derive(Debug)]
pub struct Client {
    pub(crate) state: ClientState,
    pub(crate) channels: ClientSslPacketsChannels,
}

impl Default for Client {
    fn default() -> Self {
        let (stream, channels) = ClientSslPackets::new();
        Client {
            state: ClientState::New(stream),
            channels,
        }
    }
}

#[derive(Debug)]
pub enum ClientState {
    New(ClientSslPackets),
    Connected(SslStream<ClientSslPackets>, SrtpTransport),
    Shutdown,
}

#[derive(Debug)]
pub enum ClientError {
    Receive(SendError),
    NotConnected,
    Read(std::io::Error),
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Receive(e) => write!(f, "Receive: {}", e),
            ClientError::NotConnected => write!(f, "Client not connected"),
            ClientError::Read(e) => write!(f, "Read: {}", e),
        }
    }
}

impl Error for ClientError {}

impl From<SendError> for ClientError {
    fn from(e: SendError) -> Self {
        ClientError::Receive(e)
    }
}
impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Read(e)
    }
}
impl From<std::io::ErrorKind> for ClientError {
    fn from(e: std::io::ErrorKind) -> Self {
        ClientError::Read(e.into())
    }
}

pub type ClientsRefStorage = HashMap<SocketAddr, ClientRef>;
pub type ClientsStorage = HashMap<SocketAddr, Arc<Mutex<Client>>>;

pub struct ClientRef(Arc<Mutex<Client>>, ClientSslPacketsChannels);
impl ClientRef {
    pub fn get_client(&self) -> Arc<Mutex<Client>> {
        Arc::clone(&self.0)
    }
}

impl From<Client> for ClientRef {
    fn from(c: Client) -> Self {
        let channels = c.channels.clone();
        ClientRef(Arc::new(Mutex::new(c)), channels)
    }
}

impl Default for ClientRef {
    fn default() -> Self {
        Client::default().into()
    }
}

impl ClientRef {
    pub fn outgoing_stream(&self, addr: SocketAddr) -> impl Stream<Item = DtlsMessage> {
        let outgoing_reader = Arc::clone(&self.1.outgoing_reader);
        futures::stream::unfold(
            (outgoing_reader, addr),
            |(outgoing_reader, addr)| async move {
                let mut reader = outgoing_reader.lock().await;
                if reader.is_terminated() {
                    return None;
                }
                let message = reader.next().await?;
                drop(reader);
                Some((
                    DtlsMessage::create_outgoing(message, addr),
                    (outgoing_reader, addr),
                ))
            },
        )
    }

    pub fn get_channels(&self) -> &ClientSslPacketsChannels {
        &self.1
    }
}
