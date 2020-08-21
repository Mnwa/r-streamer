use actix::Message;
use bimap::BiMap;
use std::collections::HashMap;
use std::iter::FromIterator;
use std::net::SocketAddr;
use webrtc_sdp::attribute_type::SdpAttribute;
use webrtc_sdp::attribute_type::SdpAttribute::Rtpmap;
use webrtc_sdp::media_type::SdpMedia;

pub type MediaUserStorage = HashMap<String, MediaList>;

#[derive(Debug, Clone, Default)]
pub struct MediaList(BiMap<String, u8>);

impl MediaList {
    pub fn get_name(&self, id: &u8) -> Option<&String> {
        self.0.get_by_right(id)
    }
    #[allow(clippy::ptr_arg)]
    pub fn get_id(&self, name: &String) -> Option<&u8> {
        self.0.get_by_left(name)
    }

    pub fn insert(&mut self, id: u8, name: String) {
        self.0.insert(name, id);
    }
}

impl FromIterator<SdpAttribute> for MediaList {
    fn from_iter<T: IntoIterator<Item = SdpAttribute>>(iter: T) -> Self {
        let mut list = MediaList::default();
        iter.into_iter()
            .filter_map(|attr| match attr {
                Rtpmap(s) => Some((s.payload_type, format!("{}/{}", s.codec_name, s.frequency))),
                _ => None,
            })
            .for_each(|attr| list.insert(attr.0, attr.1));
        list
    }
}

impl From<Vec<SdpMedia>> for MediaList {
    fn from(media_vec: Vec<SdpMedia>) -> Self {
        media_vec
            .into_iter()
            .flat_map(|media| media.get_attributes().clone())
            .collect::<MediaList>()
    }
}

pub struct MediaUserMessage(pub String, pub MediaList);
pub struct MediaAddrMessage(pub SocketAddr, pub MediaList);

impl Message for MediaUserMessage {
    type Result = ();
}

impl Message for MediaAddrMessage {
    type Result = ();
}
