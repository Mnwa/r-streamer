# *RStreamer* Media Server written with Rust
Status: *In Progress*

RStreamer Media Server supports only WebRTC protocol.

## Running
### Configuration
RStreamer has two env variables for manage start settings
```env
TURN_ADDR=127.0.0.1:3336 //ip address of you turn server
WEB_ADDR=127.0.0.1:3333 //ip address of you web server
```

### Running RStreamer in the dev env.
```bash
TURN_ADDR=127.0.0.1:3336 WEB_ADDR=127.0.0.1:3333 cargo run
```

### Running RStreamer in the prod env.
***Attention*** replace certs in the `cert` dir for security reasons!
```bash
cargo build --release
TURN_ADDR=127.0.0.1:3336 WEB_ADDR=127.0.0.1:3333 ./target/release/r-streamer
```