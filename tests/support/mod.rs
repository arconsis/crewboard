//! A fake GitHub for unit tests.
//!
//! An in-process HTTP server on 127.0.0.1 with an ephemeral port. It uses only `std`. It
//! records every request, so a test can assert call counts. A handler closure decides the
//! answer. Point the CLI at it with `CREWBOARD_API_BASE` (see `FakeGithub::base`).

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub mod authority;
pub mod board;
pub mod cli;
pub mod headless;
pub mod late;
#[cfg(unix)]
pub mod lock;

/// One recorded request.
#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub body: String,
    /// The token of the `Authorization: Bearer` header, if any.
    pub bearer: Option<String>,
}

/// One answer.
pub struct Resp {
    pub status: u16,
    pub body: String,
    /// Extra response headers (for example `x-oauth-scopes`).
    pub headers: Vec<(String, String)>,
}

impl Resp {
    pub fn json(status: u16, body: &str) -> Self {
        Resp {
            status,
            body: body.to_string(),
            headers: Vec::new(),
        }
    }
}

type Handler = dyn Fn(&Req) -> Resp + Send + Sync;

pub struct FakeGithub {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGithub {
    pub fn start(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> Self {
        Self::start_with(|_| handler)
    }

    /// Like [`FakeGithub::start`], but the handler is made from the base URL of the server,
    /// so its answers can carry loopback links (`html_url`, `issue_url`).
    pub fn start_with<H>(make: impl FnOnce(&str) -> H) -> Self
    where
        H: Fn(&Req) -> Resp + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake github");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let addr = listener.local_addr().unwrap();
        let handler = make(&format!("http://{addr}"));
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let (l2, s2) = (log.clone(), stop.clone());
        // The listener polls, so `Drop` ends the thread without a connection of its own (a
        // connect can fail on a host that is out of ports, and then a blocking accept never
        // returns).
        let thread = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        // Some platforms hand the listener's mode down to the connection.
                        let _ = conn.set_nonblocking(false);
                        let (h, l) = (handler.clone(), l2.clone());
                        std::thread::spawn(move || serve(conn, &*h, &l));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(_) => {}
                }
            }
        });
        FakeGithub {
            addr,
            log,
            stop,
            thread: Some(thread),
        }
    }

    /// The value for `CREWBOARD_API_BASE`.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FakeGithub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A base URL on which nothing listens (connection refused).
pub fn closed_port_base() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

/// One connection: request after request until the client closes it or asks for `Connection:
/// close` (HTTP/1.1 keep-alive). A connection per request ran Windows out of ephemeral ports
/// (WSAEADDRINUSE) at about 700 component tests; a CLI process now uses one or two.
fn serve(conn: TcpStream, handler: &Handler, log: &Mutex<Vec<Req>>) {
    let mut r = BufReader::new(conn.try_clone().unwrap());
    let mut conn = conn;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let (mut len, mut bearer, mut close) = (0usize, None, false);
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let v = v.trim();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.parse().unwrap_or(0),
                    "authorization" => bearer = v.strip_prefix("Bearer ").map(str::to_string),
                    "connection" => close = v.eq_ignore_ascii_case("close"),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let req = Req {
            method,
            path,
            body: String::from_utf8_lossy(&body).to_string(),
            bearer,
        };
        log.lock().unwrap().push(req.clone());
        let resp = handler(&req);
        let extra: String = resp
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        let out = format!(
            "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{}",
            resp.status,
            resp.body.len(),
            resp.body
        );
        if conn.write_all(out.as_bytes()).is_err() || close {
            return;
        }
    }
}
