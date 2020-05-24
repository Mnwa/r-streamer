use bytes::BytesMut;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::io::Error;
use futures::lock::Mutex;
use futures::task::{Context, Poll};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use log::warn;
use openssl::error::ErrorStack;
use openssl::ssl::{SslAcceptor, SslRef};
use srtp::{CryptoPolicy, Srtp, SsrcType};
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::macros::support::Pin;
use tokio::time::{timeout, Duration};
use tokio_openssl::accept;

#[derive(Debug)]
pub struct Client {
    pub addr: SocketAddr,
    pub ssl_state: ClientSslState,
    pub channels: ClientSslPacketsChannels,
}

impl Client {
    pub fn new(addr: SocketAddr, handshake: Vec<u8>) -> Client {
        let (ssl_state, channels) = ClientSslPackets::new();
        let ssl_state = ClientSslState::Empty(ssl_state, handshake);

        Client {
            addr,
            channels,
            ssl_state,
        }
    }
}

pub async fn connect(
    ssl_state: ClientSslPackets,
    handshake: Vec<u8>,
    mut incoming_writer: IncomingWriter,
    ssl_acceptor: Arc<SslAcceptor>,
) -> std::io::Result<impl Stream<Item = Vec<u8>>> {
    match incoming_writer.send(handshake).await {
        Ok(()) => {}
        Err(err) => warn!("{:?}", err),
    }

    let ssl_stream = timeout(Duration::from_secs(10), accept(&ssl_acceptor, ssl_state)).await?;

    let ssl_stream = match ssl_stream {
        Ok(s) => s,
        Err(e) => {
            warn!("handshake error: {:?}", e);
            return Err(std::io::ErrorKind::ConnectionAborted.into());
        }
    };

    warn!("end of handshake");

    Ok(futures::stream::unfold(
        ssl_stream,
        |mut ssl_stream| async move {
            let mut buf = vec![0; 0x10000];

            let (mut reader, _) = get_srtp(ssl_stream.ssl()).unwrap();

            match ssl_stream.get_mut().read(&mut buf).await {
                Ok(n) => {
                    if n == 0 {
                        return None;
                    }
                    buf.truncate(n);

                    let mut bufm = BytesMut::from(buf.as_slice());

                    println!("is err {}", reader.unprotect(&mut bufm).is_err());

                    Some((buf, ssl_stream))
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    warn!("long message");
                    None
                }
                Err(_) => None,
            }
        },
    ))
}

#[derive(Debug)]
pub enum ClientSslState {
    Connected,
    Shutdown,
    Empty(ClientSslPackets, Vec<u8>),
}

pub struct ClientSslPackets {
    incoming_reader: IncomingReader, // read here to decrypt request
    outgoing_writer: OutgoingWriter, // write here to send encrypted request
}

impl Debug for ClientSslPackets {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            r#"ClientSslPackets
            incoming_reader {:?}
            outgoing_writer {:?}"#,
            self.incoming_reader, self.outgoing_writer
        )
    }
}

#[derive(Debug)]
pub struct ClientSslPacketsChannels {
    pub incoming_writer: IncomingWriter,
    pub outgoing_reader: Arc<Mutex<OutgoingReader>>,
}

pub type IncomingWriter = UnboundedSender<Vec<u8>>;
pub type IncomingReader = UnboundedReceiver<Vec<u8>>;

pub type OutgoingReader = UnboundedReceiver<Vec<u8>>;
pub type OutgoingWriter = UnboundedSender<Vec<u8>>;

impl ClientSslPackets {
    fn new() -> (ClientSslPackets, ClientSslPacketsChannels) {
        let (incoming_writer, incoming_reader): (IncomingWriter, IncomingReader) = unbounded();
        let (outgoing_writer, outgoing_reader): (OutgoingWriter, OutgoingReader) = unbounded();

        let ssl_stream = ClientSslPackets {
            incoming_reader,
            outgoing_writer,
        };

        let outgoing_reader = Arc::new(Mutex::new(outgoing_reader));
        let ssl_channel = ClientSslPacketsChannels {
            incoming_writer,
            outgoing_reader,
        };

        (ssl_stream, ssl_channel)
    }
}

impl AsyncRead for ClientSslPackets {
    fn poll_read<'a>(
        mut self: Pin<&'a mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.incoming_reader.poll_next_unpin(cx) {
            Poll::Ready(Some(message)) => {
                if buf.len() < message.len() {
                    return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
                }
                buf[0..message.len()].copy_from_slice(&message);
                Poll::Ready(Ok(message.len()))
            }
            Poll::Ready(None) => Poll::Ready(Err(std::io::ErrorKind::ConnectionAborted.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for ClientSslPackets {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self
            .get_mut()
            .outgoing_writer
            .send(buf.to_vec())
            .poll_unpin(cx)
        {
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(buf.len())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(std::io::ErrorKind::WriteZero.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut().outgoing_writer.flush().poll_unpin(cx) {
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match self.get_mut().outgoing_writer.close().poll_unpin(cx) {
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn get_srtp(ssl: &SslRef) -> Result<(Srtp, Srtp), ErrorStack> {
    const SRTP_MASTER_KEY_KEY_LEN: usize = 16;
    const SRTP_MASTER_KEY_SALT_LEN: usize = 14;

    let mut client_write_key = vec![0; SRTP_MASTER_KEY_KEY_LEN + SRTP_MASTER_KEY_SALT_LEN];
    let mut server_write_key = vec![0; SRTP_MASTER_KEY_KEY_LEN + SRTP_MASTER_KEY_SALT_LEN];
    let mut dtls_buf = vec![0; SRTP_MASTER_KEY_KEY_LEN * 2 + SRTP_MASTER_KEY_SALT_LEN * 2];
    ssl.export_keying_material(&mut dtls_buf, "EXTRACTOR-dtls_srtp", None)?;

    let (client_master_key, dtls_buf) = dtls_buf.split_at(SRTP_MASTER_KEY_KEY_LEN);
    let (server_master_key, dtls_buf) = dtls_buf.split_at(SRTP_MASTER_KEY_KEY_LEN);

    let (client_salt_key, server_salt_key) = dtls_buf.split_at(SRTP_MASTER_KEY_SALT_LEN);

    client_write_key[..SRTP_MASTER_KEY_KEY_LEN].copy_from_slice(client_master_key);
    client_write_key[SRTP_MASTER_KEY_KEY_LEN..].copy_from_slice(client_salt_key);

    server_write_key[..SRTP_MASTER_KEY_KEY_LEN].copy_from_slice(server_master_key);
    server_write_key[SRTP_MASTER_KEY_KEY_LEN..].copy_from_slice(server_salt_key);

    println!("{:?}", server_write_key.len());
    println!("{:?}", client_write_key.len());

    let rtp_policy = CryptoPolicy::default();
    let rtcp_policy = CryptoPolicy::default();

    let srtp_incoming = Srtp::new(
        SsrcType::AnyInbound,
        rtp_policy,
        rtcp_policy,
        &server_write_key,
    )
    .unwrap();
    let srtp_outcoming = Srtp::new(
        SsrcType::AnyOutbound,
        rtp_policy,
        rtcp_policy,
        &client_write_key,
    )
    .unwrap();

    Ok((srtp_incoming, srtp_outcoming))
}
