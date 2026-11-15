//! `ws-pipe [addr]`: serve wire.ws-door's `/wire` (and `/wire?at=<peer>`) on `addr` (default 127.0.0.1:7794), piped
//! byte for byte to this node's ce-wire door or to a peer door listed in this node's peers file. The pipe adds no
//! authority: every cell's own ce-cap gate decides. It exists only until the node's wire.ws-door is updated.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7794".into());
    let port = std::env::var("CE_DOOR_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7700u16);
    let l = tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("ws-pipe: {addr}: {e}");
        std::process::exit(1)
    });
    eprintln!("ws-pipe: /wire on {addr} -> door 127.0.0.1:{port} (or ?at=<peer>)");
    if let Err(e) = wire_ws_door::serve_ws(l, std::net::SocketAddr::from(([127, 0, 0, 1], port))).await {
        eprintln!("ws-pipe: {e}");
        std::process::exit(1);
    }
}
