use crate::sdp::media::MediaList;
use crate::{
    client::stream::{ClientSslPackets, ClientSslPacketsChannels},
    rtp::srtp::{ErrorParse, SrtpTransport},
};
use futures::channel::mpsc::SendError;
use openssl::error::ErrorStack;
use openssl::ssl::Error as SslError;
use parking_lot::{Mutex, RwLock};
use rand::Rng;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex as MutexAsync;
use tokio_openssl::SslStream;

#[derive(Debug)]
pub enum ClientState {
    New(ClientSslPackets),
    Connected(SslStream<ClientSslPackets>),
    Shutdown,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ClientStateStatus {
    New,
    Connected,
    Shutdown,
}

impl ClientState {
    pub fn get_status(&self) -> ClientStateStatus {
        match self {
            ClientState::New(_) => ClientStateStatus::New,
            ClientState::Connected(_) => ClientStateStatus::Connected,
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
    SslErrorStack(ErrorStack),
    SslError(SslError),
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Receive(e) => write!(f, "Receive: {}", e),
            ClientError::NotConnected => write!(f, "Client not connected"),
            ClientError::AlreadyConnected => write!(f, "Client already connected"),
            ClientError::Read(e) => write!(f, "Read: {}", e),
            ClientError::SrtpParseError(e) => write!(f, "Srtp parsing error: {}", e),
            ClientError::SslErrorStack(e) => write!(f, "Ssl stack error: {}", e),
            ClientError::SslError(e) => write!(f, "Ssl error: {}", e),
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
impl From<ErrorStack> for ClientError {
    fn from(e: ErrorStack) -> Self {
        ClientError::SslErrorStack(e)
    }
}
impl From<SslError> for ClientError {
    fn from(e: SslError) -> Self {
        ClientError::SslError(e)
    }
}

pub type ClientsRefStorage = Arc<RwLock<HashMap<SocketAddr, ClientSafeRef>>>;

pub type ClientSafeRef = Arc<Client>;

#[derive(Debug, Copy, Clone)]
pub struct ClientRtpRuntime {
    pub client_ts: Option<NonZeroU32>,
    pub server_ts: u32,
    pub client_sequence: Option<NonZeroU16>,
    pub server_sequence: u16,
}

impl Default for ClientRtpRuntime {
    fn default() -> Self {
        let mut rng = rand::thread_rng();
        Self {
            server_ts: rng.gen_range(1..100000),
            server_sequence: rng.gen_range(1..10000),
            client_ts: Default::default(),
            client_sequence: Default::default(),
        }
    }
}

pub struct Client {
    state: MutexAsync<ClientState>,
    channels: ClientSslPacketsChannels,
    media: RwLock<Option<MediaList>>,
    srtp: Mutex<Option<SrtpTransport>>,
    rtp_runtime: Mutex<ClientRtpRuntime>,
    receivers: ClientsRefStorage,
    sender_addr: RwLock<Option<SocketAddr>>,
    is_deleted: AtomicBool,
    dtls_connected: AtomicBool,
}
impl Client {
    pub fn get_state(&self) -> &MutexAsync<ClientState> {
        &self.state
    }
    pub fn get_srtp(&self) -> &Mutex<Option<SrtpTransport>> {
        &self.srtp
    }
    pub fn get_media(&self) -> &RwLock<Option<MediaList>> {
        &self.media
    }
    pub fn get_receivers(&self) -> &ClientsRefStorage {
        &self.receivers
    }
    pub fn get_rtp_runtime(&self) -> &Mutex<ClientRtpRuntime> {
        &self.rtp_runtime
    }

    pub fn get_sender_addr(&self) -> &RwLock<Option<SocketAddr>> {
        &self.sender_addr
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
            state: MutexAsync::new(ClientState::New(stream)),
            channels,
            media: Default::default(),
            srtp: Default::default(),
            receivers: Default::default(),
            sender_addr: Default::default(),
            is_deleted: Default::default(),
            dtls_connected: Default::default(),
            rtp_runtime: Default::default(),
        }
    }
}
