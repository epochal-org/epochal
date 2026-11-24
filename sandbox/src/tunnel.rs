//! Reverse tunnels: a port in the box whose connections are served here. The box's `relay` app
//! listens on the port and is the HTTP/2 client of one connection carried over its own stdin and
//! stdout; this end is the server. Each connection to the port is one `POST /` stream: its request
//! body carries what the box sent and its response body what goes back, and either side's end of
//! stream is that direction's half-close.

use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use axum::Router;
use bytes::Bytes;
use h2::server::{self, Connection};
use h2::{Ping, RecvStream, SendStream};
use hyper::server::conn::http1;
use hyper::{Method, Response, StatusCode};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf, duplex, split,
};
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinHandle;
use tokio::time::interval;

use epochal_api::time_portal::sandbox::{ExecFrame, Stream};

use crate::exec::{Exec, PIPE};
use crate::sandbox::Sandbox;
use crate::{Error, invalid};

/// The app [`SandboxBuilder::with_relay`](crate::SandboxBuilder::with_relay) adds.
pub(crate) const RELAY_APP: &str = "relay";
const RELAY: &str = "/opt/epochal/bin/epochal-relay";

// The wire's settings, as the relay sets them.
const PATH: &str = "/";
const CHUNK: usize = 64 << 10;
const MAX_CONNECTIONS: u32 = 128;
const STREAM_WINDOW: u32 = 256 << 10;
const CONNECTION_WINDOW: u32 = MAX_CONNECTIONS * STREAM_WINDOW;
const MAX_HEADERS: u32 = 16 << 10;
/// The relay exits after 45 s without a byte from here; pings keep an idle tunnel alive.
const PING_EVERY: Duration = Duration::from_secs(10);

/// A served port, from [`Sandbox::serve_http`]. Dropping it kills the relay without waiting;
/// [`stop`](Self::stop) waits.
#[derive(Debug)]
pub struct Tunnel {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), Error>>,
}

impl Tunnel {
    /// Stops serving and waits until the relay is gone, so its port is free.
    pub async fn stop(self) -> Result<(), Error> {
        let _ = self.stop.send(());
        self.task
            .await
            .unwrap_or_else(|e| Err(Error::Tunnel(format!("the tunnel's task failed: {e}"))))
    }
}

impl Sandbox {
    /// Serves `127.0.0.1:<port>` in the box with `router`: each connection a box process opens
    /// there is served as HTTP/1.1 here, with no `Date` header, so the box isn't told today's
    /// date. The box must have been built [`with_relay`](crate::SandboxBuilder::with_relay). Returns once the relay listens.
    pub async fn serve_http(&self, port: u16, router: Router) -> Result<Tunnel, Error> {
        if !self.apps().any(|app| app == RELAY_APP) {
            return Err(invalid("serve_http needs a box built with_relay".into()));
        }
        let exec = self
            .command([RELAY.to_owned(), port.to_string()])
            .with_timeout(self.timeout())
            .with_open_stdin()
            .start()
            .await?;
        let exec = Arc::new(exec);
        let first = match listening(&exec).await {
            Ok(first) => first,
            Err(e) => {
                let _ = exec.kill().await;
                return Err(e);
            }
        };
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (ours, theirs) = duplex(PIPE);
            tokio::select! {
                _ = stopped => {}
                () = pump(&exec, first, theirs) => {}
                () = serve(ours, router) => {}
            }
            exec.kill().await
        });
        Ok(Tunnel { stop, task })
    }
}

/// The relay's first stdout bytes, which it writes once it listens.
async fn listening(exec: &Exec) -> Result<ExecFrame, Error> {
    let mut stderr = Vec::new();
    while let Some(frame) = exec.next().await? {
        match frame.stream {
            Stream::Stdout if frame.offset == 0 => return Ok(frame),
            Stream::Stdout => {
                return Err(Error::Tunnel("the relay's first output was lost".into()));
            }
            _ => stderr.extend(frame.data),
        }
    }
    Err(Error::Tunnel(format!(
        "the relay ended before it listened ({:?}): {}",
        exec.exit(),
        String::from_utf8_lossy(&stderr).trim()
    )))
}

/// Carries the relay's stdout into `pipe` and `pipe` into its stdin until either ends or the
/// output has a gap, which HTTP/2 can't resume past.
async fn pump(exec: &Exec, first: ExecFrame, pipe: DuplexStream) {
    let (mut from_owner, mut to_owner) = split(pipe);
    let down = async {
        let mut at = 0;
        let mut frame = first;
        loop {
            if frame.stream == Stream::Stdout {
                if frame.offset != at || to_owner.write_all(&frame.data).await.is_err() {
                    return;
                }
                at += frame.data.len() as u64;
            }
            frame = match exec.next().await {
                Ok(Some(frame)) => frame,
                _ => return,
            };
        }
    };
    let up = async {
        let mut buf = vec![0; PIPE];
        while let Ok(n @ 1..) = from_owner.read(&mut buf).await {
            if exec.write_stdin(&buf[..n]).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        () = down => {}
        () = up => {}
    }
}

/// Serves the relay's connection on `relay` until it hangs up or fails.
async fn serve(relay: DuplexStream, router: Router) {
    let handshake = server::Builder::new()
        .initial_window_size(STREAM_WINDOW)
        .initial_connection_window_size(CONNECTION_WINDOW)
        .max_concurrent_streams(MAX_CONNECTIONS)
        .max_header_list_size(MAX_HEADERS)
        .handshake::<_, Bytes>(relay)
        .await;
    if let Ok(connection) = handshake {
        accept(connection, router).await;
    }
}

async fn accept(mut connection: Connection<DuplexStream, Bytes>, router: Router) {
    // Held for a handler's whole life: a reset stream frees its h2 slot, not its handler.
    let handlers = Arc::new(Semaphore::new(MAX_CONNECTIONS as usize));
    let Some(mut ping) = connection.ping_pong() else {
        return;
    };
    let mut every = interval(PING_EVERY);
    loop {
        let accepted = tokio::select! {
            accepted = connection.accept() => accepted,
            _ = every.tick() => {
                // One ping in flight at a time; a refused one waits for the next tick.
                let _ = ping.send_ping(Ping::opaque());
                let _ = poll_fn(|cx| Poll::Ready(ping.poll_pong(cx))).await;
                continue;
            }
        };
        let Some(Ok((request, mut respond))) = accepted else {
            return;
        };
        let wanted = request.method() == Method::POST && request.uri().path() == PATH;
        let permit = wanted
            .then(|| handlers.clone().try_acquire_owned().ok())
            .flatten();
        let Some(permit) = permit else {
            let status = match wanted {
                true => StatusCode::SERVICE_UNAVAILABLE,
                false => StatusCode::NOT_FOUND,
            };
            let refusal = Response::builder().status(status).body(());
            if let Ok(refusal) = refusal {
                let _ = respond.send_response(refusal, true);
            }
            continue;
        };
        let Ok(send) = respond.send_response(Response::new(()), false) else {
            continue;
        };
        let conn = Conn {
            send,
            recv: request.into_body(),
            pending: Bytes::new(),
            eof: false,
            shut: false,
        };
        let router = router.clone();
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .auto_date_header(false)
                .half_close(true)
                .serve_connection(TokioIo::new(conn), TowerToHyperService::new(router))
                .with_upgrades()
                .await;
            drop(permit);
        });
    }
}

/// One tunnelled connection as a byte stream.
struct Conn {
    send: SendStream<Bytes>,
    recv: RecvStream,
    /// Received bytes not yet read.
    pending: Bytes,
    eof: bool,
    shut: bool,
}

fn h2_error(e: h2::Error) -> io::Error {
    if e.is_io() {
        return e
            .into_io()
            .unwrap_or_else(|| io::Error::other("h2 io error"));
    }
    io::Error::new(io::ErrorKind::ConnectionReset, e)
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        while this.pending.is_empty() {
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(this.recv.poll_data(cx)) {
                Some(Ok(data)) => this.pending = data,
                Some(Err(e)) => return Poll::Ready(Err(h2_error(e))),
                None => this.eof = true,
            }
        }
        let n = this.pending.len().min(buf.remaining());
        buf.put_slice(&this.pending.split_to(n));
        this.recv
            .flow_control()
            .release_capacity(n)
            .map_err(h2_error)?;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let send = &mut self.send;
        send.reserve_capacity(buf.len().min(CHUNK));
        let granted = loop {
            match ready!(send.poll_capacity(cx)) {
                Some(Ok(0)) => continue,
                Some(Ok(n)) => break n.min(buf.len()),
                Some(Err(e)) => return Poll::Ready(Err(h2_error(e))),
                None => return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
            }
        };
        send.send_data(Bytes::copy_from_slice(&buf[..granted]), false)
            .map_err(h2_error)?;
        Poll::Ready(Ok(granted))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.shut {
            self.shut = true;
            self.send.send_data(Bytes::new(), true).map_err(h2_error)?;
        }
        Poll::Ready(Ok(()))
    }
}
