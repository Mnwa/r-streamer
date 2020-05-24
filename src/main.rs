mod sdp;
mod server;
mod stun;

use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
    Body, Error, Method, Request, Response, Server, StatusCode,
};
use log::info;

use crate::sdp::generate_response;
use crate::server::udp::create_udp;
use actix::System;
use futures::StreamExt;
use hyper::body::Buf;
use std::sync::Arc;
use webrtc_unreliable::Server as RtcServer;

fn main() {
    let mut system = System::new("g-streamer");
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let webrtc_listen_addr = "127.0.0.1:3335"
        .parse()
        .expect("could not parse WebRTC data address/port");

    let public_webrtc_addr = "127.0.0.1:3335"
        .parse()
        .expect("could not parse advertised public WebRTC data address/port");

    let public_udp_addr = "127.0.0.1:3336"
        .parse()
        .expect("could not parse advertised public WebRTC data address/port");

    let session_listen_addr = "127.0.0.1:3333"
        .parse()
        .expect("could not parse HTTP address/port");

    let _ = RtcServer::new(webrtc_listen_addr, public_webrtc_addr);

    let (recv, _send) = system.block_on(create_udp(public_udp_addr));

    let make_svc = make_service_fn(move |addr_stream: &AddrStream| {
        let remote_addr = addr_stream.remote_addr();
        let recv = Arc::clone(&recv);
        async move {
            let recv = Arc::clone(&recv);
            Ok::<_, Error>(service_fn(move |req: Request<Body>| {
                let recv = Arc::clone(&recv);
                async move {
                    if req.uri().path() == "/"
                        || req.uri().path() == "/index.html" && req.method() == Method::GET
                    {
                        info!("serving example index HTML to {}", remote_addr);
                        Response::builder().body(Body::from(include_str!("./index.html")))
                    } else if req.uri().path() == "/parse_sdp" {
                        let sdp_req: String = req
                            .into_body()
                            .map(|c| c.unwrap())
                            .map(|c| Vec::from(c.bytes()))
                            .map(|c| String::from_utf8(c).unwrap())
                            .collect()
                            .await;

                        let sdp = generate_response(&sdp_req, recv).await.unwrap();
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Body::from(sdp.to_string().replace("\r\n\r\n", "\r\n")))
                    } else {
                        Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::from("not found"))
                    }
                }
            }))
        }
    });

    actix_rt::spawn(async move {
        Server::bind(&session_listen_addr)
            .serve(make_svc)
            .await
            .expect("HTTP session server has died");
    });

    system.run().unwrap();

    // let mut message_buf = vec![0; 0x10000];
    // loop {
    //     match rtc_server.recv(&mut message_buf).await {
    //         Ok(received) => {
    //             println!("received");
    //             println!(
    //                 "{}",
    //                 String::from_utf8_lossy(&message_buf[0..received.message_len])
    //             );
    //             if let Err(err) = rtc_server
    //                 .send(
    //                     &message_buf[0..received.message_len],
    //                     received.message_type,
    //                     &received.remote_addr,
    //                 )
    //                 .await
    //             {
    //                 warn!(
    //                     "could not send message to {}: {}",
    //                     received.remote_addr, err
    //                 )
    //             }
    //         }
    //         Err(err) => warn!("could not receive RTC message: {}", err),
    //     }
    // }
}
