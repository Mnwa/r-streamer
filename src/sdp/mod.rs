use crate::client::sessions::{Session, SessionMessage};
use crate::server::udp::{ServerDataRequest, UdpRecv};
use actix::prelude::Request;
use actix::{Addr, MailboxError};
use rand::prelude::ThreadRng;
use rand::Rng;
use std::net::SocketAddr;
use std::sync::Arc;
use webrtc_sdp::address::{Address, ExplicitlyTypedAddress};
use webrtc_sdp::attribute_type::SdpAttribute::{
    Candidate, EndOfCandidates, Fingerprint, Group, IceLite, MsidSemantic, Rtcp,
    Sendrecv as SendrecvAttr, Setup,
};
use webrtc_sdp::attribute_type::SdpAttributeFingerprintHashType::Sha256;
use webrtc_sdp::attribute_type::SdpAttributeGroupSemantic::Bundle;
use webrtc_sdp::attribute_type::SdpAttributeSetup::Passive;
use webrtc_sdp::attribute_type::SdpAttributeType::{IceUfrag, Msid, Sendrecv, Ssrc, SsrcGroup};
use webrtc_sdp::attribute_type::{
    SdpAttribute, SdpAttributeCandidate, SdpAttributeCandidateTransport, SdpAttributeCandidateType,
    SdpAttributeFingerprint, SdpAttributeGroup, SdpAttributeMsidSemantic, SdpAttributeRtcp,
};
use webrtc_sdp::error::SdpParserInternalError;
use webrtc_sdp::media_type::SdpMedia;
use webrtc_sdp::{parse_sdp, SdpConnection, SdpSession, SdpTiming};

pub async fn generate_response(sdp: &str, recv: Arc<Addr<UdpRecv>>) -> Option<SdpSession> {
    let req = parse_sdp(sdp, true).unwrap();

    let server_data = recv.send(ServerDataRequest).await.unwrap();

    let version = req.version;
    let session = req.session.clone()?;
    let mut origin = req.get_origin().clone();

    let server_user = server_data.meta.user.clone();
    let server_passwd = server_data.meta.password.clone();

    let sessions: Vec<Request<UdpRecv, SessionMessage>> = req
        .media
        .iter()
        .filter_map(|m| {
            Some(
                m.get_attribute(IceUfrag)?
                    .to_string()
                    .replace("ice-ufrag:", ""),
            )
        })
        .map(|client_user| {
            let session = Session::new(server_user.clone(), client_user);
            let session_message = SessionMessage(session, 1);
            recv.send(session_message)
        })
        .collect();

    let _inserted = futures::future::join_all(sessions)
        .await
        .into_iter()
        .collect::<Result<Vec<bool>, MailboxError>>();

    let mut rng = rand::thread_rng();
    origin.session_id = rng.gen();
    origin.unicast_addr = ExplicitlyTypedAddress::from(server_data.addr.ip());

    let media: Vec<SdpMedia> = req
        .media
        .into_iter()
        .map(|mut m| {
            m.set_port(server_data.addr.port() as u32);

            remove_useless_attributes(&mut m);
            set_attributes(
                &mut m,
                server_user.clone(),
                server_passwd.clone(),
                server_data.crypto.digest.clone(),
                server_data.addr,
                &mut rng,
            )
            .unwrap();
            replace_connection(m.get_connection(), server_data.addr);
            m
        })
        .collect();

    let mut res = SdpSession::new(version, origin, session);
    res.add_attribute(MsidSemantic(SdpAttributeMsidSemantic {
        semantic: String::from("WMS"),
        msids: Vec::new(),
    }))
    .unwrap();
    res.add_attribute(Group(SdpAttributeGroup {
        semantics: Bundle,
        tags: vec![String::from("0"), String::from("1")],
    }))
    .unwrap();

    res.add_attribute(IceLite).unwrap();

    res.set_timing(SdpTiming { start: 0, stop: 0 });

    res.media = media;
    Some(res)
}

fn replace_connection(connection: &Option<SdpConnection>, addr: SocketAddr) {
    #[allow(mutable_transmutes)]
    #[allow(clippy::transmute_ptr_to_ptr)]
    let connection = unsafe {
        std::mem::transmute::<&Option<SdpConnection>, &mut Option<SdpConnection>>(connection)
    };
    *connection = Some(SdpConnection {
        address: ExplicitlyTypedAddress::from(addr.ip()),
        ttl: None,
        amount: None,
    });
}

fn remove_useless_attributes(m: &mut SdpMedia) {
    m.remove_attribute(Msid);
    m.remove_attribute(Sendrecv);
    m.remove_attribute(SsrcGroup);
    m.remove_attribute(Ssrc);
}

fn set_attributes(
    m: &mut SdpMedia,
    server_user: String,
    server_passwd: String,
    fingerprint: Vec<u8>,
    addr: SocketAddr,
    rng: &mut ThreadRng,
) -> Result<(), SdpParserInternalError> {
    m.set_attribute(SendrecvAttr)?;
    m.set_attribute(SdpAttribute::IcePwd(server_passwd))?;
    m.set_attribute(SdpAttribute::IceUfrag(server_user))?;
    m.set_attribute(Fingerprint(SdpAttributeFingerprint {
        hash_algorithm: Sha256,
        fingerprint,
    }))?;
    m.set_attribute(Setup(Passive))?;
    m.set_attribute(EndOfCandidates)?;
    m.set_attribute(Rtcp(SdpAttributeRtcp {
        port: addr.port(),
        unicast_addr: Some(ExplicitlyTypedAddress::from(addr.ip())),
    }))?;
    m.set_attribute(Candidate(SdpAttributeCandidate {
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

    Ok(())
}
