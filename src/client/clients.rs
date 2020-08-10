use crate::sdp::media::MediaList;
use crate::{
    client::stream::{ClientSslPackets, ClientSslPacketsChannels},
    dtls::message::DtlsMessage,
    rtp::srtp::{ErrorParse, SrtpTransport},
};
use futures::{channel::mpsc::SendError, prelude::*, stream::FusedStream};
use std::cell::RefCell;
use std::rc::Rc;
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

pub type ClientsRefStorage = HashMap<SocketAddr, ClientSafeRef>;

pub type ClientSafeRef = Rc<RefCell<ClientRef>>;

pub struct ClientRef {
    client: Arc<Mutex<Client>>,
    channels: ClientSslPacketsChannels,
    media: Option<MediaList>,
    receivers: HashMap<SocketAddr, ClientSafeRef>,
    is_deleted: bool,
}
impl ClientRef {
    pub fn get_client(&self) -> Arc<Mutex<Client>> {
        Arc::clone(&self.client)
    }
    pub fn get_media(&self) -> &Option<MediaList> {
        &self.media
    }
    pub fn set_media(&mut self, media: MediaList) {
        self.media = Some(media)
    }
    pub fn add_receiver(&mut self, addr: SocketAddr, client: ClientSafeRef) {
        self.receivers.insert(addr, client);
    }

    pub fn get_receivers(&self) -> &HashMap<SocketAddr, ClientSafeRef> {
        &self.receivers
    }

    pub fn clear(&mut self) {
        self.receivers
            .retain(|_, client_ref| !client_ref.as_ref().borrow().is_deleted());
    }

    pub fn delete(&mut self) {
        self.is_deleted = true
    }

    pub fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    pub fn outgoing_stream(&self, addr: SocketAddr) -> impl Stream<Item = DtlsMessage> {
        let outgoing_reader = Arc::clone(&self.channels.outgoing_reader);
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
        &self.channels
    }
}

impl From<Client> for ClientRef {
    fn from(c: Client) -> Self {
        let channels = c.channels.clone();
        ClientRef {
            client: Arc::new(Mutex::new(c)),
            channels,
            media: None,
            receivers: HashMap::new(),
            is_deleted: false,
        }
    }
}

impl Default for ClientRef {
    fn default() -> Self {
        Client::default().into()
    }
}
