use crate::{
    client::stream::{ClientSslPackets, ClientSslPacketsChannels},
    dtls::message::DtlsMessage,
    rtp::srtp::{ErrorParse, SrtpTransport},
};
use futures::{channel::mpsc::SendError, lock::Mutex, prelude::*, stream::FusedStream};
use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};
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
    AlreadyConnected,
    Read(std::io::Error),
    SrtpParseError(ErrorParse),
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Receive(e) => write!(f, "Receive: {}", e),
            ClientError::NotConnected => write!(f, "Client not connected"),
            ClientError::AlreadyConnected => write!(f, "Client already connected"),
            ClientError::Read(e) => write!(f, "Read: {}", e),
            ClientError::SrtpParseError(e) => write!(f, "Srtp parsing error: {}", e),
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
impl From<ErrorParse> for ClientError {
    fn from(e: ErrorParse) -> Self {
        ClientError::SrtpParseError(e)
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
