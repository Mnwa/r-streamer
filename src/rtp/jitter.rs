use crate::rtp::core::is_rtcp;
use crate::server::udp::DataPacket;
use actix::prelude::*;
use log::{info, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const JITTER_BUFFER_DELAY_INCREASE_STEP: Duration = Duration::from_millis(10);
pub const JITTER_BUFFER_START_DELAY: Duration = Duration::from_millis(50);
pub const JITTER_BUFFER_VOLATILE: u32 = 1000;
pub const JITTER_RECHECK_TIME: Duration = Duration::from_secs(10);

pub type Buffer = Vec<DataPacket>;

#[derive(Debug, Clone)]
pub struct JitterBuffer {
    pub buffer: Buffer,
    pub time: Instant,
    pub delay: Duration,
}

impl JitterBuffer {
    pub fn new(delay: Duration) -> Self {
        Self {
            buffer: Default::default(),
            time: Instant::now(),
            delay,
        }
    }
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new(JITTER_BUFFER_START_DELAY)
    }
}

pub type JitterBufferStorage = HashMap<SocketAddr, JitterBuffer>;

#[derive(Debug, Default, Clone)]
pub struct JitterControlActor {
    jitter_storage: JitterBufferStorage,
}

impl JitterControlActor {
    pub fn new() -> Addr<Self> {
        Self::create(|_| Self::default())
    }
}

impl Actor for JitterControlActor {
    type Context = Context<Self>;

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        warn!("jitter actor died")
    }
}

impl Handler<IncreaseJitterBufferDelay> for JitterControlActor {
    type Result = ();

    fn handle(
        &mut self,
        IncreaseJitterBufferDelay { addr }: IncreaseJitterBufferDelay,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let jitter = self.jitter_storage.entry(addr).or_default();
        jitter.delay += JITTER_BUFFER_DELAY_INCREASE_STEP;
        info!(
            "jitter delay is increased for {} to {} ms",
            addr,
            jitter.delay.as_millis()
        );
    }
}

impl Handler<DecreaseJitterBufferDelay> for JitterControlActor {
    type Result = ();

    fn handle(
        &mut self,
        DecreaseJitterBufferDelay { addr }: DecreaseJitterBufferDelay,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        if let Some(jitter) = self.jitter_storage.get_mut(&addr) {
            if jitter.delay <= JITTER_BUFFER_START_DELAY {
                return;
            }
            jitter.delay -= JITTER_BUFFER_DELAY_INCREASE_STEP;
            info!(
                "jitter delay is decreased for {} to {} ms",
                addr,
                jitter.delay.as_millis()
            );
        }
    }
}

impl Handler<GetOrStoreJitterBuffer> for JitterControlActor {
    type Result = GetOrStoreJitterBufferResponse;

    fn handle(
        &mut self,
        GetOrStoreJitterBuffer { data, addr }: GetOrStoreJitterBuffer,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        if is_rtcp(&data) {
            return GetOrStoreJitterBufferResponse::Packet(data);
        }

        let jitter_buffer = self.jitter_storage.entry(addr).or_default();
        GetOrStoreJitterBufferResponse::Buffer({
            jitter_buffer.buffer.push(data);

            if jitter_buffer.time.elapsed() >= jitter_buffer.delay {
                jitter_buffer.time = Instant::now();
                let buffer = std::mem::take(&mut jitter_buffer.buffer);
                Some(buffer)
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Copy, Clone)]
pub struct IncreaseJitterBufferDelay {
    pub addr: SocketAddr,
}

impl Message for IncreaseJitterBufferDelay {
    type Result = ();
}

#[derive(Debug, Copy, Clone)]
pub struct DecreaseJitterBufferDelay {
    pub addr: SocketAddr,
}

impl Message for DecreaseJitterBufferDelay {
    type Result = ();
}

#[derive(Debug, Clone)]
pub struct GetOrStoreJitterBuffer {
    pub addr: SocketAddr,
    pub data: DataPacket,
}

impl Message for GetOrStoreJitterBuffer {
    type Result = GetOrStoreJitterBufferResponse;
}

#[derive(Debug, Clone, MessageResponse)]
pub enum GetOrStoreJitterBufferResponse {
    Buffer(Option<Buffer>),
    Packet(DataPacket),
}
