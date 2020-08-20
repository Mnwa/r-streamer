use crate::sdp::media::MediaList;
use crate::{
    client::stream::{ClientSslPackets, ClientSslPacketsChannels},
    rtp::srtp::{ErrorParse, SrtpTransport},
};
use futures::channel::mpsc::SendError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex;
use tokio_openssl::SslStream;

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

pub type ClientsRefStorage = HashMap<SocketAddr, ClientSafeRef>;

pub type ClientSafeRef = Arc<Client>;

pub struct Client {
    state: Arc<Mutex<ClientState>>,
    channels: ClientSslPacketsChannels,
    media: Arc<Mutex<Option<MediaList>>>,
    receivers: Arc<Mutex<ClientsRefStorage>>,
    is_deleted: AtomicBool,
}
impl Client {
    pub fn get_state(&self) -> Arc<Mutex<ClientState>> {
        Arc::clone(&self.state)
    }
    pub fn get_media(&self) -> Arc<Mutex<Option<MediaList>>> {
        Arc::clone(&self.media)
    }
    pub fn get_receivers(&self) -> Arc<Mutex<ClientsRefStorage>> {
        Arc::clone(&self.receivers)
    }

    pub fn delete(&self) {
        self.is_deleted.store(true, Ordering::Relaxed)
    }

    pub fn is_deleted(&self) -> bool {
        self.is_deleted.load(Ordering::Relaxed)
    }

    pub fn get_channels(&self) -> &ClientSslPacketsChannels {
        &self.channels
    }
}

impl Default for Client {
    fn default() -> Self {
        let (stream, channels) = ClientSslPackets::new();
        Client {
            state: Arc::new(Mutex::new(ClientState::New(stream))),
            channels,
            media: Default::default(),
            receivers: Default::default(),
            is_deleted: AtomicBool::new(false),
        }
    }
}
