mod client;
mod dtls;
mod rtp;
mod sdp;
mod server;
mod stun;

use crate::{
    sdp::generate_streamer_response,
    server::udp::{create_udp, UdpRecv},
};
use actix::Addr;
use actix_files::NamedFile;
use actix_web::{
    get, post,
    web::{Bytes, Data, Path},
    App, HttpRequest, HttpResponse, HttpServer, Result,
};
use log::info;
use std::net::SocketAddr;

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let public_udp_addr = "127.0.0.1:3336"
        .parse()
        .expect("could not parse advertised public WebRTC data address/port");

    let session_listen_addr: SocketAddr = "127.0.0.1:3333"
        .parse()
        .expect("could not parse HTTP address/port");

    let (recv, _send) = create_udp(public_udp_addr).await;

    HttpServer::new(move || {
        App::new()
            .data(recv.clone())
            .service(index)
            .service(parse_sdp)
    })
    .bind(session_listen_addr)?
    .run()
    .await
}

#[get("/")]
async fn index(req: HttpRequest) -> Result<NamedFile> {
    info!("serving example index HTML to {:?}", req.peer_addr());
    Ok(NamedFile::open("public/index.html")?)
}

#[post("/parse_sdp/{group_id}/")]
async fn parse_sdp(
    body: Bytes,
    path_info: Path<(usize,)>,
    recv: Data<Addr<UdpRecv>>,
) -> Result<HttpResponse> {
    let group_id = path_info.0;
    let body = String::from_utf8(body.to_vec()).map_err(|_| HttpResponse::BadRequest().finish())?;

    let sdp = generate_streamer_response(&body, recv.into_inner(), group_id)
        .await
        .map_err(|e| HttpResponse::BadRequest().body(e.to_string()))?;

    Ok(sdp.to_string().replace("\r\n\r\n", "\r\n").into())
}
