pub mod media;

use crate::sdp::media::MediaUserMessage;
use crate::{
    client::sessions::{Session, SessionMessage},
    server::udp::{ServerDataRequest, UdpRecv},
};
use actix::prelude::*;
use futures::{future::ready, stream::iter, StreamExt, TryStreamExt};
use rand::{prelude::ThreadRng, Rng};
use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
    net::SocketAddr,
    sync::Arc,
};

use crate::client::sessions::SessionStorageItem;
use smol_str::SmolStr;
use std::time::SystemTime;
use webrtc_sdp::attribute_type::SdpAttribute::{IceOptions, RtcpMux};
use webrtc_sdp::attribute_type::SdpAttributeType::{Recvonly, Sendonly};
use webrtc_sdp::{
    address::{Address, ExplicitlyTypedAddress},
    attribute_type::SdpAttribute::Rtpmap,
    attribute_type::{
        SdpAttribute,
        SdpAttribute::{
            Candidate, EndOfCandidates, Fingerprint, Group, IceLite, MsidSemantic,
            Recvonly as RecvonlyAttr, Rtcp, Sendonly as SendonlyAttr, Setup,
        },
        SdpAttributeCandidate, SdpAttributeCandidateTransport, SdpAttributeCandidateType,
        SdpAttributeFingerprint,
        SdpAttributeFingerprintHashType::Sha256,
        SdpAttributeGroup,
        SdpAttributeGroupSemantic::Bundle,
        SdpAttributeMsidSemantic, SdpAttributeRtcp,
        SdpAttributeSetup::Passive,
        SdpAttributeType::{Group as GroupType, IceUfrag, Msid, Ssrc, SsrcGroup},
    },
    error::{SdpParserError, SdpParserInternalError},
    media_type::SdpMedia,
    parse_sdp, SdpConnection, SdpSession, SdpTiming,
};

pub async fn generate_streamer_response(
    sdp: &str,
    recv: Arc<Addr<UdpRecv>>,
    group_id: usize,
    sdp_addr: SocketAddr,
) -> Result<SdpSession, SdpResponseGeneratorError> {
    let req = parse_sdp(sdp, true)?;

    let server_data = recv.send(ServerDataRequest).await?;

    let version = req.version;
    let session = req
        .session
        .clone()
        .ok_or_else(|| SdpResponseGeneratorError::from("Session is empty"))?;
    let mut origin = req.get_origin().clone();

    let server_user = server_data.meta.user.clone();
    let server_passwd = server_data.meta.password.clone();

    let group = req.get_attribute(GroupType).cloned().unwrap_or_else(|| {
        Group(SdpAttributeGroup {
            semantics: Bundle,
            tags: req
                .media
                .iter()
                .enumerate()
                .map(|(k, _v)| k.to_string())
                .collect(),
        })
    });

    let _inserted = iter(&req.media)
        .filter_map(|m| ready(m.get_attribute(IceUfrag)))
        .map(|m| m.to_string().replace("ice-ufrag:", ""))
        .map(|client_user| Session::new(server_user.clone(), client_user.into()))
        .map(|session| {
            (
                MediaUserMessage(session.get_client(), req.media.clone().into()),
                SessionMessage(
                    session,
                    SessionStorageItem {
                        group_id,
                        ttl: SystemTime::now(),
                        is_sender: req.get_attribute(Sendonly).is_some(),
                    },
                ),
            )
        })
        .then(|(media_message, session_message)| {
            futures::future::join(recv.send(session_message), recv.send(media_message))
        })
        .map(|res| res.0.and_then(|v| Ok((v, res.1?))))
        .try_collect::<Vec<_>>()
        .await
        .expect("session sending error");

    let mut rng = rand::thread_rng();
    origin.session_id = rng.gen::<u16>() as u64;
    origin.unicast_addr = ExplicitlyTypedAddress::from(sdp_addr.ip());

    let media: Vec<SdpMedia> = req
        .media
        .into_iter()
        .map(|mut m| {
            m.set_port(sdp_addr.port() as u32);

            remove_useless_attributes(&mut m)?;
            set_attributes(
                &mut m,
                server_user.clone(),
                server_passwd.clone(),
                server_data.crypto.digest.clone(),
                sdp_addr,
                &mut rng,
            )?;
            m.set_connection(SdpConnection {
                address: ExplicitlyTypedAddress::from(sdp_addr.ip()),
                ttl: None,
                amount: None,
            });
            Ok(m)
        })
        .collect::<Result<Vec<SdpMedia>, SdpResponseGeneratorError>>()?;

    /*
    отправить в сессион стор медиа стримера, чтобы потом возвращать его клиентам
    */

    origin.username = String::from("-");

    let mut res = SdpSession::new(version, origin, session);
    res.add_attribute(MsidSemantic(SdpAttributeMsidSemantic {
        semantic: String::from("WMS"),
        msids: Vec::new(),
    }))?;

    res.add_attribute(group)?;

    res.add_attribute(IceLite)?;
    res.add_attribute(IceOptions(vec!["trickle".into()]))?;

    res.set_timing(SdpTiming { start: 0, stop: 0 });

    res.media = media;
    Ok(res)
}

fn remove_useless_attributes(m: &mut SdpMedia) -> Result<(), SdpParserInternalError> {
    m.remove_attribute(Msid);
    if m.get_attribute(Recvonly).is_some() {
        m.remove_attribute(Recvonly);
        m.set_attribute(SendonlyAttr)?;
    } else if m.get_attribute(Sendonly).is_some() {
        m.remove_attribute(Sendonly);
        m.set_attribute(RecvonlyAttr)?;
    }
    m.remove_attribute(SsrcGroup);
    m.remove_attribute(Ssrc);
    Ok(())
}

fn set_attributes(
    m: &mut SdpMedia,
    server_user: SmolStr,
    server_passwd: SmolStr,
    fingerprint: Vec<u8>,
    addr: SocketAddr,
    rng: &mut ThreadRng,
) -> Result<(), SdpParserInternalError> {
    let good_codecs: Vec<_> = m
        .get_attributes()
        .iter()
        .filter_map(|attr| match attr {
            Rtpmap(s) => {
                if is_supported_format(&format!("{}/{}", s.codec_name, s.frequency)) {
                    Some(s)
                } else {
                    None
                }
            }
            _ => None,
        })
        .cloned()
        .collect();

    m.remove_codecs();
    good_codecs
        .into_iter()
        .try_for_each(|attr| m.add_codec(attr))?;

    m.set_attribute(SdpAttribute::IcePwd(server_passwd.to_string()))?;
    m.set_attribute(SdpAttribute::IceUfrag(server_user.to_string()))?;
    m.set_attribute(Fingerprint(SdpAttributeFingerprint {
        hash_algorithm: Sha256,
        fingerprint,
    }))?;
    m.set_attribute(Setup(Passive))?;
    m.set_attribute(Rtcp(SdpAttributeRtcp {
        port: addr.port(),
        unicast_addr: Some(ExplicitlyTypedAddress::from(addr.ip())),
    }))?;

    m.set_attribute(RtcpMux)?;

    m.add_attribute(Candidate(SdpAttributeCandidate {
        foundation: "0".to_string(),
        priority: rng.gen::<u32>() as u64,
        address: Address::Ip(addr.ip()),
        port: addr.port() as u32,
        c_type: SdpAttributeCandidateType::Host,
        raddr: None,
        rport: None,
        tcp_type: None,
        generation: None,
        ufrag: None,
        networkcost: None,
        transport: SdpAttributeCandidateTransport::Udp,
        component: 1,
        unknown_extensions: vec![],
    }))?;

    m.set_attribute(EndOfCandidates)?;

    Ok(())
}

#[derive(Debug)]
pub enum SdpResponseGeneratorError {
    SdpParserError(SdpParserError),
    SdpParserInternalError(SdpParserInternalError),
    MailBoxError(MailboxError),
    CustomError(SmolStr),
}

impl Display for SdpResponseGeneratorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SdpResponseGeneratorError::SdpParserError(e) => std::fmt::Display::fmt(&e, f),
            SdpResponseGeneratorError::SdpParserInternalError(e) => std::fmt::Display::fmt(&e, f),
            SdpResponseGeneratorError::MailBoxError(_) => write!(f, "Udp receiver is broken"),
            SdpResponseGeneratorError::CustomError(m) => write!(f, "{}", m),
        }
    }
}

impl Error for SdpResponseGeneratorError {}

impl From<SdpParserError> for SdpResponseGeneratorError {
    fn from(e: SdpParserError) -> Self {
        SdpResponseGeneratorError::SdpParserError(e)
    }
}

impl From<SdpParserInternalError> for SdpResponseGeneratorError {
    fn from(e: SdpParserInternalError) -> Self {
        SdpResponseGeneratorError::SdpParserInternalError(e)
    }
}

impl From<MailboxError> for SdpResponseGeneratorError {
    fn from(e: MailboxError) -> Self {
        SdpResponseGeneratorError::MailBoxError(e)
    }
}

impl From<&str> for SdpResponseGeneratorError {
    fn from(e: &str) -> Self {
        SdpResponseGeneratorError::CustomError(e.into())
    }
}

#[inline]
fn is_supported_format(format: &str) -> bool {
    matches!(
        format,
        "PCMU/8000"
            | "PCMA/8000"
            | "G722/8000"
            | "H264/90000"
            | "telephone-event/8000"
            | "opus/48000"
            | "VP8/90000"
            | "VP9/90000"
    )
}
