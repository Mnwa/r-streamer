use actix::prelude::*;
use std::{collections::HashMap, net::SocketAddr};

pub type GroupsStorage = HashMap<usize, SocketAddr>;

#[derive(Default)]
pub struct Group {
    groups_storage: GroupsStorage,
}

impl Group {
    pub fn insert_or_get_sender(&mut self, group_id: usize, addr: SocketAddr) -> SocketAddr {
        *self.groups_storage.entry(group_id).or_insert(addr)
    }

    pub fn remove_sender(&mut self, addr: SocketAddr) {
        self.groups_storage
            .retain(|_, sender_addr| addr != *sender_addr);
    }
}

pub struct GroupId(pub usize, pub SocketAddr);

impl Message for GroupId {
    type Result = ();
}
