use crate::client::clients::ClientsRefStorage;
use crate::rtp::core::{is_rtcp, RtpHeader};
use crate::rtp::srtp::ErrorParse;
use crate::server::udp::{DataPacket, UdpSend, WebRtcRequest};
use actix::prelude::*;
use log::warn;
use rayon::prelude::*;
use std::sync::Arc;

pub struct RtcActor {
    client_storage: Arc<ClientsRefStorage>,
    udp_send: Addr<UdpSend>,
}

impl RtcActor {
    pub fn new(udp_send: Addr<UdpSend>, client_storage: Arc<ClientsRefStorage>) -> Addr<Self> {
        SyncArbiter::start(num_cpus::get(), move || Self {
            udp_send: udp_send.clone(),
            client_storage: client_storage.clone(),
        })
    }
}

impl Actor for RtcActor {
    type Context = SyncContext<Self>;
}

impl Handler<WebRtcRequest> for RtcActor {
    type Result = ();

    fn handle(&mut self, msg: WebRtcRequest, _ctx: &mut SyncContext<Self>) -> Self::Result {
        let (mut message, addr) = match msg {
            WebRtcRequest::Rtc(m, a) => (m, a),
            _ => return,
        };
        let client_ref = match self.client_storage.read().get(&addr) {
            Some(cf) => Arc::clone(cf),
            None => return,
        };

        let udp_send = self.udp_send.clone();
        let is_rtcp = is_rtcp(&message);

        let res: Result<(), ErrorParse> = RtpHeader::from_buf(&message).and_then(|rtp_header| {
            let mut state = client_ref.get_srtp().lock();

            let codec = if let Some(srtp) = &mut *state {
                if is_rtcp {
                    srtp.unprotect_rtcp(&mut message)?;
                    None
                } else {
                    srtp.unprotect(&mut message)?;

                    if rtp_header.payload == 111 {
                        return Err(ErrorParse::UnsupportedFormat);
                    }

                    let media = client_ref.get_media().read();
                    media
                        .as_ref()
                        .and_then(|media| media.get_name(&rtp_header.payload))
                        .cloned()
                }
            } else {
                return Err(ErrorParse::ClientNotReady(addr));
            };

            drop(state);

            let receivers = client_ref.get_receivers().read();

            if receivers.is_empty() {
                println!("empty receivers, {} {}", is_rtcp, addr)
            }

            receivers
                .par_iter()
                .filter(|(_, recv)| !recv.is_deleted())
                .try_for_each(|(r_addr, recv)| {
                    let mut message = message.clone();
                    let mut state = recv.get_srtp().lock();

                    let message = if let Some(srtp) = &mut *state {
                        if is_rtcp {
                            srtp.protect_rtcp(&mut message)?;
                        } else {
                            srtp.protect(&mut message)?;

                            if let Some(codec) = codec.as_ref() {
                                let media = recv.get_media().read();
                                if let Some(payload) = media
                                    .as_ref()
                                    .and_then(|media| media.get_id(codec))
                                    .copied()
                                {
                                    message[1] = calculate_payload(rtp_header.marker, payload);
                                }
                            }
                        }

                        message
                    } else {
                        return Err(ErrorParse::ClientNotReady(addr));
                    };

                    udp_send.do_send(WebRtcRequest::Rtc(
                        DataPacket::from(message.as_slice()),
                        *r_addr,
                    ));

                    Ok(())
                })?;

            Ok(())
        });

        if let Err(e) = res {
            if !e.should_ignored() {
                warn!("processor err: {:?} is_rtcp: {}", e, is_rtcp)
            }
        }
    }
}

#[inline]
const fn calculate_payload(marker: bool, payload: u8) -> u8 {
    payload | ((marker as u8) << 7)
}
