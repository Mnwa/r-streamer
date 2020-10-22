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
use tokio::sync::{Mutex, RwLock};
use tokio_openssl::SslStream;

#[derive(Debug)]
pub enum ClientState {
    New(ClientSslPackets),
    Connected(SslStream<ClientSslPackets>, SrtpTransport),
    Shutdown,
}

#[derive(Debug, Copy, Clone)]
pub enum ClientStateStatus {
    New,
    Connected,
    Shutdown,
}

impl ClientState {
    pub fn get_status(&self) -> ClientStateStatus {
        match self {
            ClientState::New(_) => ClientStateStatus::New,
            ClientState::Connected(_, _) => ClientStateStatus::Connected,
            ClientState::Shutdown => ClientStateStatus::Shutdown,
        }
    }
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
    state: Mutex<ClientState>,
    channels: ClientSslPacketsChannels,
    media: RwLock<Option<MediaList>>,
    receivers: RwLock<ClientsRefStorage>,
    is_deleted: AtomicBool,
    dtls_connected: AtomicBool,
}
impl Client {
    pub fn get_state(&self) -> &Mutex<ClientState> {
        &self.state
    }
    pub fn get_media(&self) -> &RwLock<Option<MediaList>> {
        &self.media
    }
    pub fn get_receivers(&self) -> &RwLock<ClientsRefStorage> {
        &self.receivers
    }

    pub fn delete(&self) {
        self.is_deleted.store(true, Ordering::Release)
    }

    pub fn is_deleted(&self) -> bool {
        self.is_deleted.load(Ordering::Acquire)
    }

    pub fn dtls_connected(&self) -> bool {
        self.dtls_connected.load(Ordering::Acquire)
    }

    pub fn make_connected(&self) {
        self.dtls_connected.store(true, Ordering::Release);
    }

    pub fn get_channels(&self) -> &ClientSslPacketsChannels {
        &self.channels
    }
}

unsafe impl Send for ClientState {}
unsafe impl Send for Client {}

impl Default for Client {
    fn default() -> Self {
        let (stream, channels) = ClientSslPackets::new();
        Client {
            state: Mutex::new(ClientState::New(stream)),
            channels,
            media: Default::default(),
            receivers: Default::default(),
            is_deleted: Default::default(),
            dtls_connected: Default::default(),
        }
    }
}
