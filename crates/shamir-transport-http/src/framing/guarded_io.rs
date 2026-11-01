use super::scanner::{Framing, Scanner, Verdict};
use std::future::Future;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Head,
    Fixed(u64),
    ChunkSize,
    ChunkData(u64),
    ChunkDataCrlf,
    Trailers,
    Closed,
}

/// Validates a whole request head before exposing it, then limits reads to its body envelope.
pub(crate) struct GuardedIo<T> {
    inner: T,
    head: Vec<u8>,
    pending: Vec<u8>,
    scanner: Scanner,
    state: State,
    max_bytes: usize,
    max_headers: usize,
    line: Vec<u8>,
    trailer_bytes: usize,
    trailer_headers: usize,
    canned: Option<Vec<u8>>,
    canned_offset: usize,
    tls: bool,
    hsts: bool,
    response_head: Vec<u8>,
    response_started: bool,
    response_remaining: Option<u64>,
    response_interim: bool,
    response_active: bool,
    reader: Option<std::task::Waker>,
    holds: Option<tokio::sync::mpsc::Receiver<Box<dyn Send + Sync>>>,
    active_hold: Option<Box<dyn Send + Sync>>,
    head_timeout: Option<std::time::Duration>,
    request_timeout: Option<std::time::Duration>,
    write_timeout: Option<std::time::Duration>,
    head_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    request_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    write_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    canned_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    response_bytes_started: bool,
    request_active: bool,
}
impl<T: AsyncRead + AsyncWrite + Unpin> GuardedIo<T> {
    pub(crate) fn new(
        inner: T,
        max_bytes: usize,
        max_headers: usize,
        tls: bool,
        hsts: bool,
        holds: tokio::sync::mpsc::Receiver<Box<dyn Send + Sync>>,
    ) -> Self {
        Self {
            inner,
            head: Vec::new(),
            pending: Vec::new(),
            scanner: Scanner::new(),
            state: State::Head,
            max_bytes,
            max_headers,
            line: Vec::new(),
            trailer_bytes: 0,
            trailer_headers: 0,
            canned: None,
            canned_offset: 0,
            tls,
            hsts,
            response_head: Vec::new(),
            response_started: false,
            response_remaining: None,
            response_interim: false,
            response_active: false,
            reader: None,
            holds: Some(holds),
            active_hold: None,
            head_timeout: None,
            request_timeout: None,
            write_timeout: None,
            head_deadline: None,
            request_deadline: None,
            write_deadline: None,
            canned_deadline: None,
            response_bytes_started: false,
            request_active: false,
        }
    }
    pub(crate) fn with_timeouts(
        mut self,
        head: std::time::Duration,
        request: std::time::Duration,
        write: std::time::Duration,
    ) -> Self {
        self.head_timeout = Some(head);
        self.request_timeout = Some(request);
        self.write_timeout = Some(write);
        self
    }
    fn poll_deadlines(
        &mut self,
        cx: &mut std::task::Context<'_>,
        reading_head: bool,
    ) -> io::Result<()> {
        if self.request_active && self.request_deadline.is_none() {
            self.request_deadline = self
                .request_timeout
                .map(|duration| Box::pin(tokio::time::sleep(duration)));
        }
        if reading_head && !self.response_active && self.head_deadline.is_none() {
            self.head_deadline = self
                .head_timeout
                .map(|duration| Box::pin(tokio::time::sleep(duration)));
        }
        if self.request_deadline.is_none()
            && (!self.head.is_empty() || !self.scanner.bytes().is_empty() || self.request_active)
        {
            self.request_deadline = self
                .request_timeout
                .map(|duration| Box::pin(tokio::time::sleep(duration)));
        }
        let mut expired = false;
        for timer in [
            &mut self.head_deadline,
            &mut self.request_deadline,
            &mut self.write_deadline,
        ] {
            if let Some(timer) = timer {
                if timer.as_mut().poll(cx).is_ready() {
                    expired = true;
                }
            }
        }
        if expired {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP deadline expired",
            ));
        }
        Ok(())
    }
    fn reject(&mut self, status: u16) {
        self.request_deadline = None;
        self.request_active = false;
        self.head_deadline = None;
        self.write_deadline = None;
        self.canned_deadline = self
            .write_timeout
            .map(|duration| Box::pin(tokio::time::sleep(duration)));
        if self.canned.is_some() {
            return;
        }
        let (reason, phrase) = match status {
            431 => ("431", "Request Header Fields Too Large"),
            408 => ("408", "Request Timeout"),
            _ => ("400", "Bad Request"),
        };
        let hsts = if self.tls && self.hsts {
            "Strict-Transport-Security: max-age=31536000\r\n"
        } else {
            ""
        };
        self.canned=Some(format!("HTTP/1.1 {reason} {phrase}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\n{hsts}Connection: close\r\nContent-Length: 0\r\n\r\n").into_bytes());
        self.canned_offset = 0;
        self.state = State::Closed;
        self.head.clear();
        self.pending.clear();
    }
    pub(crate) async fn finish_rejection(&mut self, timed_out: bool) -> io::Result<()> {
        if timed_out
            && self.canned.is_none()
            && self.state == State::Head
            && !self.response_active
            && !self.response_bytes_started
        {
            self.reject(408);
        }
        std::future::poll_fn(|cx| self.finish_canned(cx)).await
    }
    fn finish_canned(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        loop {
            if let Some(timer) = self.canned_deadline.as_mut() {
                if timer.as_mut().poll(cx).is_ready() {
                    self.canned = None;
                    self.state = State::Closed;
                    return std::task::Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "canned response write deadline expired",
                    )));
                }
            }
            if let Some(response) = self.canned.as_ref() {
                let offset = self.canned_offset;
                if offset < response.len() {
                    let rest = response[offset..].to_vec();
                    match std::pin::Pin::new(&mut self.inner).poll_write(cx, &rest) {
                        std::task::Poll::Pending => return std::task::Poll::Pending,
                        std::task::Poll::Ready(Err(error)) => {
                            return std::task::Poll::Ready(Err(error))
                        }
                        std::task::Poll::Ready(Ok(0)) => {
                            return std::task::Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "canned response write returned zero",
                            )))
                        }
                        std::task::Poll::Ready(Ok(n)) => self.canned_offset += n,
                    }
                    continue;
                }
                return match std::pin::Pin::new(&mut self.inner).poll_flush(cx) {
                    std::task::Poll::Ready(Ok(())) => {
                        self.canned = None;
                        self.canned_deadline = None;
                        std::task::Poll::Ready(Ok(()))
                    }
                    other => other,
                };
            }
            return std::task::Poll::Ready(Ok(()));
        }
    }
    fn body_envelope(&mut self, bytes: &[u8]) -> bool {
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Fixed(left) => {
                    let n = (bytes.len() - i).min(left.min(usize::MAX as u64) as usize);
                    i += n;
                    self.state = if n as u64 == left {
                        State::Head
                    } else {
                        State::Fixed(left - n as u64)
                    };
                    if self.state == State::Head {
                        self.scanner = Scanner::new();
                        self.response_active = true;
                    }
                }
                State::ChunkData(left) => {
                    let n = (bytes.len() - i).min(left.min(usize::MAX as u64) as usize);
                    i += n;
                    self.state = if n as u64 == left {
                        State::ChunkDataCrlf
                    } else {
                        State::ChunkData(left - n as u64)
                    };
                }
                State::ChunkSize => {
                    self.line.push(bytes[i]);
                    i += 1;
                    if self.line.len() > 128 {
                        self.reject(400);
                        return false;
                    }
                    if self.line.ends_with(b"\r\n") {
                        let digits = &self.line[..self.line.len() - 2];
                        let token = digits.split(|b| *b == b';').next().unwrap_or_default();
                        let size = match std::str::from_utf8(token)
                            .ok()
                            .and_then(|s| u64::from_str_radix(s, 16).ok())
                        {
                            Some(v) => v,
                            None => {
                                self.reject(400);
                                return false;
                            }
                        };
                        self.line.clear();
                        self.state = if size == 0 {
                            State::Trailers
                        } else {
                            State::ChunkData(size)
                        };
                    }
                }
                State::ChunkDataCrlf => {
                    let n = (2 - self.line.len()).min(bytes.len() - i);
                    self.line.extend_from_slice(&bytes[i..i + n]);
                    i += n;
                    if self.line.len() == 2 {
                        if self.line != b"\r\n" {
                            self.reject(400);
                            return false;
                        }
                        self.line.clear();
                        self.state = State::ChunkSize;
                    }
                }
                State::Trailers => {
                    self.line.push(bytes[i]);
                    i += 1;
                    self.trailer_bytes += 1;
                    if self.trailer_bytes > self.max_bytes {
                        self.reject(431);
                        return false;
                    }
                    if self.line.ends_with(b"\n") && !self.line.ends_with(b"\r\n") {
                        self.reject(400);
                        return false;
                    }
                    if self.line.ends_with(b"\r\n") {
                        if self.line == b"\r\n" {
                            self.line.clear();
                            self.trailer_bytes = 0;
                            self.trailer_headers = 0;
                            self.scanner = Scanner::new();
                            self.state = State::Head;
                            self.response_active = true;
                        } else {
                            let field = &self.line[..self.line.len() - 2];
                            self.trailer_headers += 1;
                            if self.trailer_headers > self.max_headers
                                || field.first().is_some_and(|b| *b == b' ' || *b == b'\t')
                                || !field.contains(&b':')
                                || field.iter().any(|b| *b < 0x20 && *b != b'\t' || *b == 0x7f)
                            {
                                self.reject(400);
                                return false;
                            }
                            self.line.clear();
                        }
                    }
                }
                State::Head | State::Closed => return false,
            }
        }
        true
    }
    fn observe_write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() && !self.response_bytes_started {
            self.response_bytes_started = true;
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            if let Some(left) = self.response_remaining {
                let n = rest.len().min(left.min(usize::MAX as u64) as usize);
                rest = &rest[n..];
                self.response_remaining = Some(left - n as u64);
                if self.response_remaining == Some(0) {
                    self.response_remaining = None;
                }
            } else {
                self.response_head.push(rest[0]);
                rest = &rest[1..];
                if self.response_head.ends_with(b"\r\n\r\n") {
                    let h = String::from_utf8_lossy(&self.response_head).into_owned();
                    self.response_interim = h
                        .lines()
                        .next()
                        .is_some_and(|line| line.starts_with("HTTP/1.1 1"));
                    let len = h
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<u64>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    self.response_head.clear();
                    self.response_started = true;
                    self.response_bytes_started = true;
                    if len > 0 {
                        self.response_remaining = Some(len);
                    }
                }
                if self.response_head.len() > self.max_bytes.saturating_add(1024) {
                    self.response_head.clear();
                }
            }
        }
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for GuardedIo<T> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        out: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut reading_head = self.state == State::Head && !self.response_active;
        if self.state == State::Closed {
            if self.canned.is_some() {
                return self.finish_canned(cx);
            }
            return std::task::Poll::Ready(Ok(()));
        }
        if self.poll_deadlines(cx, reading_head).is_err() {
            if self.response_bytes_started {
                self.canned = None;
                self.state = State::Closed;
                return std::task::Poll::Ready(Err(deadline_error(
                    "response write deadline expired",
                )));
            }
            self.reject(408);
            return self.finish_canned(cx);
        }
        loop {
            if self.state == State::Closed {
                return self.finish_canned(cx);
            }
            reading_head = self.state == State::Head && !self.response_active;
            if self.poll_deadlines(cx, reading_head).is_err() {
                if self.response_bytes_started {
                    self.canned = None;
                    self.state = State::Closed;
                    return std::task::Poll::Ready(Err(deadline_error(
                        "response write deadline expired",
                    )));
                }
                self.reject(408);
                return self.finish_canned(cx);
            }
            if !self.pending.is_empty() {
                let n = out.remaining().min(self.pending.len());
                out.put_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return std::task::Poll::Ready(Ok(()));
            }
            if self.state == State::Closed {
                return self.finish_canned(cx);
            }
            if self.state == State::Head && self.response_active {
                if let Some(receiver) = self.holds.as_mut() {
                    match receiver.poll_recv(cx) {
                        std::task::Poll::Ready(Some(hold)) => self.active_hold = Some(hold),
                        std::task::Poll::Ready(None) => self.holds = None,
                        std::task::Poll::Pending => {}
                    }
                }
                self.reader = Some(cx.waker().clone());
                return std::task::Poll::Pending;
            }
            let is_head = self.state == State::Head;
            let mut temp = [0u8; 1024];
            let cap = match self.state {
                State::Head | State::ChunkSize | State::ChunkDataCrlf | State::Trailers => 1,
                State::Fixed(n) | State::ChunkData(n) => n.min(temp.len() as u64) as usize,
                State::Closed => 1,
            }
            .max(1);
            let mut buf = tokio::io::ReadBuf::new(&mut temp[..cap]);
            match std::pin::Pin::new(&mut self.inner).poll_read(cx, &mut buf) {
                std::task::Poll::Pending => return std::task::Poll::Pending,
                std::task::Poll::Ready(Err(error)) => return std::task::Poll::Ready(Err(error)),
                std::task::Poll::Ready(Ok(())) => {
                    let n = buf.filled().len();
                    if n == 0 {
                        if is_head && (!self.head.is_empty() || !self.scanner.bytes().is_empty()) {
                            self.reject(400);
                            continue;
                        }
                        return std::task::Poll::Ready(Ok(()));
                    }
                    if is_head {
                        if !self.request_active {
                            self.request_active = true;
                            self.request_deadline = self
                                .request_timeout
                                .map(|duration| Box::pin(tokio::time::sleep(duration)));
                        }
                        self.head.extend_from_slice(&temp[..n]);
                        let max_bytes = self.max_bytes;
                        let max_headers = self.max_headers;
                        let verdict = self.scanner.feed(&temp[..n], max_bytes, max_headers);
                        match verdict {
                            Verdict::NeedMore => continue,
                            Verdict::BadRequest => {
                                self.reject(400);
                                continue;
                            }
                            Verdict::TooLarge | Verdict::TooManyHeaders => {
                                self.reject(431);
                                continue;
                            }
                            Verdict::Complete => {
                                let framing = self.scanner.framing().unwrap_or(Framing::Fixed(0));
                                self.head_deadline = None;
                                self.request_active = true;
                                if self.request_deadline.is_none() {
                                    self.request_deadline = self
                                        .request_timeout
                                        .map(|duration| Box::pin(tokio::time::sleep(duration)));
                                }
                                self.pending = std::mem::take(&mut self.head);
                                self.response_active = true;
                                self.response_started = false;
                                self.response_interim = false;
                                self.response_head.clear();
                                self.response_bytes_started = false;
                                self.response_remaining = None;
                                self.state = match framing {
                                    Framing::Chunked => State::ChunkSize,
                                    Framing::Fixed(length) => State::Fixed(length),
                                };
                                if self.state == State::Fixed(0) {
                                    self.state = State::Head;
                                    self.scanner = Scanner::new();
                                }
                                let count = out.remaining().min(self.pending.len());
                                out.put_slice(&self.pending[..count]);
                                self.pending.drain(..count);
                                return std::task::Poll::Ready(Ok(()));
                            }
                        }
                    } else {
                        if !self.body_envelope(&temp[..n]) {
                            continue;
                        }
                        self.pending.extend_from_slice(&temp[..n]);
                        continue;
                    }
                }
            }
        }
    }
}
fn deadline_error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, message)
}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for GuardedIo<T> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        if self.state == State::Closed {
            if self.canned.is_some() {
                return match self.finish_canned(cx) {
                    std::task::Poll::Ready(Ok(())) => std::task::Poll::Ready(Err(deadline_error(
                        "connection closed after rejection",
                    ))),
                    std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
                    std::task::Poll::Pending => std::task::Poll::Pending,
                };
            }
            return std::task::Poll::Ready(Err(deadline_error("connection closed after timeout")));
        }
        if self.write_deadline.is_none() {
            self.write_deadline = self
                .write_timeout
                .map(|duration| Box::pin(tokio::time::sleep(duration)));
        }
        if self.poll_deadlines(cx, false).is_err() {
            if self.response_bytes_started {
                self.canned = None;
                self.state = State::Closed;
                return std::task::Poll::Ready(Err(deadline_error(
                    "response write deadline expired",
                )));
            }
            self.reject(408);
            return match self.finish_canned(cx) {
                std::task::Poll::Ready(Ok(())) => std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "request timed out before response write",
                ))),
                std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
                std::task::Poll::Pending => std::task::Poll::Pending,
            };
        }
        if bytes.is_empty() {
            return std::task::Poll::Ready(Ok(0));
        }
        if self.active_hold.is_none() {
            if let Some(receiver) = self.holds.as_mut() {
                if let Ok(hold) = receiver.try_recv() {
                    self.active_hold = Some(hold);
                }
            }
        }
        let result = std::pin::Pin::new(&mut self.inner).poll_write(cx, bytes);
        if let std::task::Poll::Ready(Ok(n)) = result {
            self.observe_write(&bytes[..n]);
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if self.state == State::Closed {
            return match self.finish_canned(cx) {
                std::task::Poll::Ready(Ok(())) if self.response_started => {
                    std::task::Poll::Ready(Err(deadline_error("connection closed after timeout")))
                }
                other => other,
            };
        }
        if self.response_started && self.write_deadline.is_none() {
            self.write_deadline = self
                .write_timeout
                .map(|duration| Box::pin(tokio::time::sleep(duration)));
        }
        let reading_head = self.state == State::Head && !self.response_active;
        if self.poll_deadlines(cx, reading_head).is_err() {
            if self.response_bytes_started {
                self.canned = None;
                self.state = State::Closed;
                return std::task::Poll::Ready(Err(deadline_error(
                    "response write deadline expired",
                )));
            }
            self.reject(408);
            return self.finish_canned(cx);
        }
        match std::pin::Pin::new(&mut self.inner).poll_flush(cx) {
            std::task::Poll::Ready(Ok(())) => {
                if self.response_started
                    && self.response_remaining.is_none()
                    && self.response_head.is_empty()
                {
                    self.response_bytes_started = false;
                    if self.response_interim {
                        self.response_interim = false;
                        self.response_started = false;
                        self.write_deadline = None;
                    } else {
                        self.response_active = false;
                        self.response_started = false;
                        self.write_deadline = None;
                        self.request_deadline = None;
                        self.request_active = false;
                        self.active_hold = None;
                        if let Some(waker) = self.reader.take() {
                            waker.wake();
                        }
                    }
                }
                std::task::Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
