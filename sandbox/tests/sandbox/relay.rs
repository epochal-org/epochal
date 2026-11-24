//! The box's end of the tunnel, as the `relay` app speaks it: listens on `127.0.0.1:<port>` and,
//! as the HTTP/2 client of the owner on its stdin and stdout, opens one `POST /` stream per
//! connection, the request body carrying what the box sent and the response body what came back.

use std::future::poll_fn;

use axum::http::{Method, Request};
use bytes::Bytes;
use h2::client::{self, SendRequest};
use h2::{RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

const STREAM_WINDOW: u32 = 256 << 10;
const MAX_CONNECTIONS: u32 = 128;

/// Relays until the owner's end hangs up.
pub async fn run(port: u16, owner: DuplexStream) {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let Ok((sender, connection)) = client::Builder::new()
        .initial_window_size(STREAM_WINDOW)
        .initial_connection_window_size(MAX_CONNECTIONS * STREAM_WINDOW)
        .max_concurrent_streams(MAX_CONNECTIONS)
        .handshake::<_, Bytes>(owner)
        .await
    else {
        return;
    };
    tokio::select! {
        _ = connection => {}
        () = accept(listener, sender) => {}
    }
}

async fn accept(listener: TcpListener, sender: SendRequest<Bytes>) {
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let sender = sender.clone();
        tokio::spawn(splice(socket, sender));
    }
}

async fn splice(socket: TcpStream, sender: SendRequest<Bytes>) {
    let Ok(mut ready) = sender.ready().await else {
        return;
    };
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://tunnel/")
        .body(())
        .unwrap();
    let Ok((response, send)) = ready.send_request(request, false) else {
        return;
    };
    let (from_box, to_box) = socket.into_split();
    let down = async {
        let Ok(response) = response.await else { return };
        assert!(response.status().is_success(), "{}", response.status());
        down(response.into_body(), to_box).await;
    };
    tokio::join!(up(from_box, send), down);
}

async fn up(mut from_box: OwnedReadHalf, mut send: SendStream<Bytes>) {
    let mut buf = vec![0; 64 << 10];
    loop {
        let n = match from_box.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut sent = 0;
        while sent < n {
            send.reserve_capacity(n - sent);
            let Some(Ok(granted)) = poll_fn(|cx| send.poll_capacity(cx)).await else {
                return;
            };
            let end = (sent + granted).min(n);
            let chunk = Bytes::copy_from_slice(&buf[sent..end]);
            if send.send_data(chunk, false).is_err() {
                return;
            }
            sent = end;
        }
    }
    let _ = send.send_data(Bytes::new(), true);
}

async fn down(mut body: RecvStream, mut to_box: OwnedWriteHalf) {
    while let Some(Ok(data)) = body.data().await {
        let _ = body.flow_control().release_capacity(data.len());
        if to_box.write_all(&data).await.is_err() {
            return;
        }
    }
    let _ = to_box.shutdown().await;
}
