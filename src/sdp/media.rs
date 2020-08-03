use actix::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use webrtc_sdp::media_type::SdpMedia;

pub type MediaUserStorage = HashMap<String, MediaList>;
pub type MediaAddrStorage = HashMap<SocketAddr, MediaList>;

pub type MediaList = Arc<Vec<SdpMedia>>;

pub struct MediaUserMessage(pub String, pub MediaList);
pub struct MediaAddrMessage(pub SocketAddr, pub MediaList);

impl Message for MediaUserMessage {
    type Result = ();
}

impl Message for MediaAddrMessage {
    type Result = ();
}
