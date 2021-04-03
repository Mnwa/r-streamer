use actix::Message;
use smol_str::SmolStr;
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct Session {
    server_user: SmolStr,
    client_user: SmolStr,
}

impl Session {
    pub fn new(server_user: SmolStr, client_user: SmolStr) -> Self {
        Session {
            server_user,
            client_user,
        }
    }

    pub fn get_client(&self) -> SmolStr {
        self.client_user.clone()
    }
}

pub struct SessionMessage(pub Session, pub SessionStorageItem);

impl Message for SessionMessage {
    type Result = bool;
}

pub type SessionsStorage = HashMap<Session, SessionStorageItem>;

#[derive(Debug, Clone, Copy)]
pub struct SessionStorageItem {
    pub group_id: usize,
    pub ttl: SystemTime,
    pub is_sender: bool,
}
