use crate::client::clients::ClientsRefStorage;
use crate::rtp::core::{is_rtcp, RtpHeader};
use crate::rtp::srtp::ErrorParse;
use crate::server::udp::{DataPacket, UdpSend, WebRtcRequest};
use actix::prelude::*;
use byteorder::ByteOrder;
use log::warn;
use rayon::prelude::*;
use std::num::{NonZeroU16, NonZeroU32};
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

            let codec_n_frequency = if let Some(srtp) = &mut *state {
                if is_rtcp {
                    srtp.unprotect_rtcp(&mut message)?;
                    None
                } else {
                    srtp.unprotect(&mut message)?;

                    if rtp_header.payload == 111 {
                        return Err(ErrorParse::UnsupportedFormat);
                    }

                    let media = client_ref.get_media().read();
                    media.as_ref().and_then(|media| {
                        Some((
                            media.get_name(&rtp_header.payload).cloned()?,
                            media.get_frequency(&rtp_header.payload).copied()?,
                        ))
                    })
                }
            } else {
                return Err(ErrorParse::ClientNotReady(addr));
            };

            drop(state);

            if !is_rtcp {
                let mut rtp_runtime = client_ref.get_rtp_runtime().lock();

                let client_diff = match rtp_runtime.client_ts {
                    Some(client_ts) => rtp_header.timestamp.saturating_sub(client_ts.get()),
                    None => 0,
                };
                rtp_runtime.client_sequence = NonZeroU16::new(rtp_header.sequence);
                rtp_runtime.client_ts = NonZeroU32::new(rtp_header.timestamp);
                rtp_runtime.server_sequence += 1;
                rtp_runtime.server_ts += client_diff;

                replace_sequence_n_timestamp(
                    &mut message,
                    rtp_runtime.server_sequence,
                    rtp_runtime.server_ts,
                );
            }

            let receivers = client_ref.get_receivers().read();

            if is_rtcp && receivers.is_empty() {
                let result = client_ref.get_sender_addr().read().and_then(|sender_addr| {
                    let sender = Arc::clone(self.client_storage.read().get(&sender_addr)?);
                    let mut state = sender.get_srtp().lock();
                    let srtp = state.as_mut()?;
                    let mut message = message.clone();
                    Some(
                        srtp.protect_rtcp(&mut message)
                            .map(|_| (sender_addr, message)),
                    )
                });

                match result {
                    Some(Ok((sender_addr, message))) => {
                        udp_send.do_send(WebRtcRequest::Rtc(message, sender_addr));
                    }
                    Some(Err(e)) => return Err(e),
                    None => return Ok(()),
                }
            } else {
                receivers
                    .par_iter()
                    .filter(|(_, recv)| !recv.is_deleted())
                    .try_for_each(|(r_addr, recv)| {
                        let mut message = message.clone();
                        let mut state = recv.get_srtp().lock();

                        let message = if let Some(srtp) = state.as_mut() {
                            if is_rtcp {
                                srtp.protect_rtcp(&mut message)?;
                            } else {
                                srtp.protect(&mut message)?;

                                if let Some((codec, _frequency)) = codec_n_frequency.as_ref() {
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
            }

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

fn replace_sequence_n_timestamp(buf: &mut [u8], sequence: u16, timestamp: u32) {
    byteorder::NetworkEndian::write_u16(&mut buf[2..4], sequence);
    byteorder::NetworkEndian::write_u32(&mut buf[4..8], timestamp);
}
