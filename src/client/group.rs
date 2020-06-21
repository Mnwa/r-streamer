use actix::prelude::*;
use std::{collections::HashMap, net::SocketAddr};

pub type GroupsStorage = HashMap<SocketAddr, usize>;
pub type GroupsAddrStorage = HashMap<usize, Vec<SocketAddr>>;

pub struct GroupId(pub usize, pub SocketAddr);

impl Message for GroupId {
    type Result = ();
}
