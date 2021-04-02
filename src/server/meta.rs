#[derive(Clone)]
pub struct ServerMeta {
    pub user: String,
    pub password: String,
}

impl ServerMeta {
    pub fn new() -> ServerMeta {
        let mut rng = rand::thread_rng();
        let user = rand_string(&mut rng, 12);
        let password = rand_string(&mut rng, 24);

        ServerMeta { user, password }
    }
}

fn rand_string<R: rand::Rng>(rng: &mut R, size: usize) -> String {
    const RAND_CHAR_TABLE: &[u8; 62] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    let mut s = String::new();
    s.reserve(size);
    for _ in 0..size {
        s.push(RAND_CHAR_TABLE[rng.gen_range(0..62)] as char);
    }
    s
}
