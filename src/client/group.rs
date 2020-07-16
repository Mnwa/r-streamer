use actix::prelude::*;
use std::{collections::HashMap, net::SocketAddr};

pub type GroupsStorage = HashMap<SocketAddr, usize>;
pub type GroupsAddrStorage = HashMap<usize, Vec<SocketAddr>>;

#[derive(Default)]
pub struct Group {
    groups_storage: GroupsStorage,
    groups_addr_storage: GroupsAddrStorage,
}

impl Group {
    pub fn get_addressess(&self, addr: SocketAddr) -> Option<Vec<SocketAddr>> {
        let group_id = self.groups_storage.get(&addr)?;
        let addresses = self.groups_addr_storage.get(group_id)?;
        Some(
            addresses
                .iter()
                .filter(|g_addr| **g_addr != addr)
                .cloned()
                .collect(),
        )
    }

    pub fn insert_client(&mut self, group_id: usize, addr: SocketAddr) {
        let groups_addr_storage = self.groups_addr_storage.entry(group_id).or_default();
        groups_addr_storage.push(addr);
        self.groups_storage.insert(addr, group_id);
    }

    pub fn remove_client(&mut self, addr: SocketAddr) -> bool {
        self.groups_storage
            .remove(&addr)
            .and_then(|group_id| self.groups_addr_storage.get_mut(&group_id))
            .and_then(|group_addrs| {
                let pos = group_addrs.iter().position(|x| *x == addr)?;
                Some(group_addrs.remove(pos))
            })
            .is_some()
    }
}

pub struct GroupId(pub usize, pub SocketAddr);

impl Message for GroupId {
    type Result = ();
}
