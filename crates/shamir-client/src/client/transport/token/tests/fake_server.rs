use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::header::HeaderValue;
use shamir_query_types::wire::DbResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::client::transport::token::{TokenEndpoint, TokenTransport};

/// One scripted raw HTTP/1.1 reply, consumed per request in arrival order.
pub struct Scripted {
    bytes: Vec<u8>,
    close_after: bool,
    hang: bool,
}

impl Scripted {
    pub fn fixed(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Self {
        let framing = format!("content-length: {}\r\n", body.len());
        let mut bytes = head(status, headers, &framing);
        bytes.extend_from_slice(body);
        Self::from_bytes(bytes)
    }

    pub fn chunked(status: u16, headers: &[(&str, &str)], chunks: &[&[u8]]) -> Self {
        let mut bytes = head(status, headers, "transfer-encoding: chunked\r\n");
        for chunk in chunks {
            bytes.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            bytes.extend_from_slice(chunk);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
        Self::from_bytes(bytes)
    }

    /// Head only, announcing a body that is never sent.
    pub fn declared(status: u16, headers: &[(&str, &str)], content_length: u64) -> Self {
        let framing = format!("content-length: {content_length}\r\n");
        Self::from_bytes(head(status, headers, &framing))
    }

    pub fn pong() -> Self {
        let body = rmp_serde::encode::to_vec_named(&DbResponse::Pong).unwrap();
        Self::fixed(200, &[("content-type", "application/msgpack")], &body)
    }

    pub fn pong_closing() -> Self {
        let body = rmp_serde::encode::to_vec_named(&DbResponse::Pong).unwrap();
        let headers = [
            ("content-type", "application/msgpack"),
            ("connection", "close"),
        ];
        Self::fixed(200, &headers, &body).closing()
    }

    /// Never answers; the connection stays open.
    pub fn hang() -> Self {
        Self {
            bytes: Vec::new(),
            close_after: false,
            hang: true,
        }
    }

    /// Closes the connection after writing this reply.
    pub fn closing(mut self) -> Self {
        self.close_after = true;
        self
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            close_after: false,
            hang: false,
        }
    }
}

fn head(status: u16, headers: &[(&str, &str)], framing: &str) -> Vec<u8> {
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    format!("HTTP/1.1 {status} Test\r\n{framing}{extra}\r\n").into_bytes()
}

/// A recorded request head with lowercased header names.
pub struct Head {
    pub request_line: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn parse(raw: &[u8]) -> Self {
        let text = String::from_utf8_lossy(raw).into_owned();
        let mut lines = text.lines();
        let request_line = lines.next().unwrap_or_default().to_owned();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        Self {
            request_line,
            headers,
        }
    }

    pub fn values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        names
    }
}

struct Shared {
    accepts: AtomicUsize,
    script: Mutex<VecDeque<Scripted>>,
    heads: Mutex<Vec<Vec<u8>>>,
}

pub struct FakeServer {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeServer {
    pub async fn start(script: Vec<Scripted>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            accepts: AtomicUsize::new(0),
            script: Mutex::new(script.into()),
            heads: Mutex::new(Vec::new()),
        });
        let accept_shared = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accept_shared.accepts.fetch_add(1, Ordering::AcqRel);
                tokio::spawn(serve(stream, Arc::clone(&accept_shared)));
            }
        });
        Self { addr, shared, task }
    }

    pub fn accepts(&self) -> usize {
        self.shared.accepts.load(Ordering::Acquire)
    }

    pub fn heads(&self) -> Vec<Head> {
        self.shared
            .heads
            .lock()
            .unwrap()
            .iter()
            .map(|raw| Head::parse(raw))
            .collect()
    }

    pub fn endpoint(&self) -> TokenEndpoint {
        TokenEndpoint::plain_loopback(self.addr).unwrap()
    }

    pub async fn transport(
        &self,
        connect_timeout: Option<Duration>,
        request_timeout: Option<Duration>,
    ) -> TokenTransport {
        TokenTransport::connect(
            &self.endpoint(),
            auth_value(),
            connect_timeout,
            request_timeout,
        )
        .await
        .unwrap()
    }
}

pub fn auth_value() -> HeaderValue {
    let mut value = HeaderValue::from_static("Bearer shk1.test");
    value.set_sensitive(true);
    value
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let Some(head_end) = read_head(&mut stream, &mut buf).await else {
            return;
        };
        let raw_head = buf[..head_end].to_vec();
        let total = head_end + content_length(&raw_head);
        while buf.len() < total {
            if !fill(&mut stream, &mut buf).await {
                return;
            }
        }
        buf.drain(..total);
        shared.heads.lock().unwrap().push(raw_head);
        let next = shared.script.lock().unwrap().pop_front();
        let Some(reply) = next else {
            return;
        };
        if reply.hang {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return;
        }
        if stream.write_all(&reply.bytes).await.is_err() || reply.close_after {
            return;
        }
    }
}

async fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}

async fn read_head(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<usize> {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(end + 4);
        }
        if !fill(stream, buf).await {
            return None;
        }
    }
}

fn content_length(raw_head: &[u8]) -> usize {
    String::from_utf8_lossy(raw_head)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
}
