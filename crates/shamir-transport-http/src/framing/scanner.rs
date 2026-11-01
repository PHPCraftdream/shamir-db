//! Request-head framing validator for the HTTP/1.1 transport.

/// Incremental result of validating a request head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// More bytes are required to complete the head.
    NeedMore,
    /// The complete head is valid.
    Complete,
    /// The head contains invalid framing or syntax.
    BadRequest,
    /// The configured request-head byte limit was exceeded.
    TooLarge,
    /// The configured request-header count was exceeded.
    TooManyHeaders,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Framing {
    Fixed(u64),
    Chunked,
}

/// Incrementally validates request-head bytes independently of read chunking.
#[derive(Debug)]
pub struct Scanner {
    bytes: Vec<u8>,
    done: bool,
    verdict: Verdict,
    framing: Option<Framing>,
}
impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}
impl Scanner {
    /// Creates a scanner with no input.
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            done: false,
            verdict: Verdict::NeedMore,
            framing: None,
        }
    }
    /// Adds input and reports the current validation verdict.
    pub fn feed(&mut self, input: &[u8], max_bytes: usize, max_headers: usize) -> Verdict {
        if self.done {
            return self.verdict;
        }
        for byte in input {
            self.bytes.push(*byte);
            if self.bytes.len() > max_bytes {
                return self.finish(Verdict::TooLarge);
            }
            if *byte == b'\n' && (self.bytes.len() < 2 || self.bytes[self.bytes.len() - 2] != b'\r')
            {
                return self.finish(Verdict::BadRequest);
            }
            if self.bytes.ends_with(b"\r\n\r\n") {
                let (verdict, framing) = validate_head(&self.bytes, max_headers);
                self.framing = framing;
                return self.finish(verdict);
            }
        }
        Verdict::NeedMore
    }
    fn finish(&mut self, verdict: Verdict) -> Verdict {
        self.done = true;
        self.verdict = verdict;
        verdict
    }
    /// Returns all bytes fed to the scanner so far.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn framing(&self) -> Option<Framing> {
        self.framing
    }
}

fn valid_request_line(line: &[u8]) -> bool {
    let mut parts = line.split(|byte| *byte == b' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    !method.is_empty()
        && method
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
        && !target.is_empty()
        && !target.iter().any(|byte| *byte <= 0x20 || *byte == 0x7f)
        && version == b"HTTP/1.1"
        && parts.next().is_none()
}

fn valid_field_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
}

fn validate_head(head: &[u8], max_headers: usize) -> (Verdict, Option<Framing>) {
    let Some(lines) = head.strip_suffix(b"\r\n\r\n") else {
        return (Verdict::BadRequest, None);
    };
    let lines: Vec<&[u8]> = lines
        .split(|b| *b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .collect();
    if lines.is_empty() || !valid_request_line(lines[0]) {
        return (Verdict::BadRequest, None);
    }
    if lines.iter().any(|line| {
        line.iter()
            .any(|b| (*b < 0x20 && *b != b'\t') || *b == 0x7f)
    }) || lines.iter().skip(1).any(|line| line.contains(&b'\r'))
    {
        return (Verdict::BadRequest, None);
    }
    if lines.len().saturating_sub(1) > max_headers {
        return (Verdict::TooManyHeaders, None);
    }
    let mut length = None;
    let mut lengths = 0usize;
    let mut transfers = 0usize;
    for line in lines.iter().skip(1) {
        if matches!(line.first(), Some(b' ' | b'\t')) {
            return (Verdict::BadRequest, None);
        }
        let Some(colon) = line.iter().position(|b| *b == b':') else {
            return (Verdict::BadRequest, None);
        };
        let name = &line[..colon];
        if !valid_field_name(name) {
            return (Verdict::BadRequest, None);
        }
        let value = trim_ows(&line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"content-length") {
            lengths += 1;
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return (Verdict::BadRequest, None);
            }
            let Some(parsed) = value.iter().try_fold(0u64, |n, digit| {
                n.checked_mul(10)?.checked_add(u64::from(*digit - b'0'))
            }) else {
                return (Verdict::BadRequest, None);
            };
            length = Some(parsed);
        }
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            transfers += 1;
            if !value.eq_ignore_ascii_case(b"chunked") {
                return (Verdict::BadRequest, None);
            }
        }
    }
    if lengths > 1 || transfers > 1 || (lengths > 0 && transfers > 0) {
        return (Verdict::BadRequest, None);
    }
    let framing = if transfers == 1 {
        Framing::Chunked
    } else {
        Framing::Fixed(length.unwrap_or(0))
    };
    (Verdict::Complete, Some(framing))
}
fn trim_ows(mut value: &[u8]) -> &[u8] {
    while matches!(value.first(), Some(b' ' | b'\t')) {
        value = &value[1..];
    }
    while matches!(value.last(), Some(b' ' | b'\t')) {
        value = &value[..value.len() - 1];
    }
    value
}
