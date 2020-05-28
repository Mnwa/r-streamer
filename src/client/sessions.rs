use actix::Message;
use std::collections::HashSet;

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

impl Message for Session {
    type Result = bool;
}

pub type SessionsStorage = HashSet<Session>;
