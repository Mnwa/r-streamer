use openssl::hash::{hash, MessageDigest};
use std::collections::HashSet;

#[derive(Hash, Eq, PartialEq)]
pub struct Session {
    user: String,
    password: Password,
}

impl Session {
    fn new(user: String, password: String) -> Self {
        Session {
            user,
            password: password.into(),
        }
    }
}

#[derive(Hash, Eq, PartialEq)]
struct Password(Vec<u8>);

impl From<String> for Password {
    fn from(password: String) -> Self {
        let hash = hash(MessageDigest::sha256(), password.as_bytes()).unwrap();
        Self(hash.to_vec())
    }
}

pub type SessionsStorage = HashSet<Session>;
