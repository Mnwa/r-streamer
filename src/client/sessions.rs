use actix::Message;
use std::collections::HashMap;

#[derive(Hash, Eq, PartialEq)]
pub struct Session {
    server_user: String,
    client_user: String,
}

impl Session {
    pub fn new(server_user: String, client_user: String) -> Self {
        Session {
            server_user,
            client_user,
        }
    }
}

pub struct SessionMessage(pub Session, pub usize);

impl Message for SessionMessage {
    type Result = bool;
}

pub type SessionsStorage = HashMap<Session, usize>;
