use crate::framing::scanner::{Framing, Scanner, Verdict};

#[test]
fn each_split_point_preserves_scanner_verdict() {
    let cases: &[(&[u8], Verdict)] = &[
        (b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", Verdict::Complete),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked \r\n\r\n",
            Verdict::Complete,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\t\r\n\r\n",
            Verdict::Complete,
        ),
        (b"GET / HTTP/2.0\r\nHost: x\r\n\r\n", Verdict::BadRequest),
        (b"GET\t/ HTTP/1.1\r\nHost: x\r\n\r\n", Verdict::BadRequest),
        (
            b"GET /\tpath HTTP/1.1\r\nHost: x\r\n\r\n",
            Verdict::BadRequest,
        ),
        (b"GET / HTTP/1.1\t\r\nHost: x\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.0\r\nHost: x\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.2\r\nHost: x\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\nX: \x00\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\nX: \x01\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\nX: \x01\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\nX: \x7f\r\n\r\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\nX: \x08\r\n\r\n", Verdict::BadRequest),
        (
            b"GET / HTTP/1.1\r\nX: bare\rvalue\r\n\r\n",
            Verdict::BadRequest,
        ),
        (b"GET / HTTP/1.1\n\n", Verdict::BadRequest),
        (b"GET / HTTP/1.1\r\n folded: x\r\n\r\n", Verdict::BadRequest),
        (
            b"POST / HTTP/1.1\r\nContent-Length: +1\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: \r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: 1x\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: 18446744073709551616\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"POST / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked, gzip\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked;foo\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
            Verdict::BadRequest,
        ),
        (
            b"GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            Verdict::BadRequest,
        ),
        (b"GET / HTTP/1.1\r\nX : y\r\n\r\n", Verdict::BadRequest),
        (
            b"GET / HTTP/1.1\r\nX: bare\rvalue\r\n\r\n",
            Verdict::BadRequest,
        ),
    ];
    for (input, expected) in cases {
        for split in 0..=input.len() {
            let mut scanner = Scanner::new();
            let first = scanner.feed(&input[..split], 8192, 64);
            let mut verdict = first;
            if verdict == Verdict::NeedMore {
                verdict = scanner.feed(&input[split..], 8192, 64);
            }
            assert_eq!(verdict, *expected, "split at {split} for {input:?}");
            if verdict != Verdict::NeedMore {
                assert_eq!(
                    scanner.feed(b"ignored", 8192, 64),
                    verdict,
                    "terminal verdict must stick"
                );
            }
        }
    }
}

#[test]
fn scanner_exposes_verified_framing_metadata() {
    for (head, expected) in [
        (
            b"POST / HTTP/1.1\r\nContent-Length: 42\r\n\r\n".as_slice(),
            Framing::Fixed(42),
        ),
        (
            b"POST / HTTP/1.1\r\nTransfer-Encoding: \tchunked \r\n\r\n".as_slice(),
            Framing::Chunked,
        ),
    ] {
        let mut scanner = Scanner::new();
        assert_eq!(scanner.feed(head, 8192, 64), Verdict::Complete);
        assert_eq!(scanner.framing(), Some(expected));
    }
}

#[test]
fn scanner_enforces_head_and_header_limits() {
    let mut scanner = Scanner::new();
    assert_eq!(
        scanner.feed(b"GET / HTTP/1.1\r\n\r\n", 5, 64),
        Verdict::TooLarge
    );
    let mut scanner = Scanner::new();
    assert_eq!(
        scanner.feed(b"GET / HTTP/1.1\r\nA: b\r\nB: c\r\n\r\n", 8192, 1),
        Verdict::TooManyHeaders
    );
}

#[test]
fn scanner_limits_and_need_more_are_split_invariant() {
    let cases: &[(&[u8], usize, usize, Verdict)] = &[
        (b"GET / HTTP/1.1\r\n\r\n", 8, 64, Verdict::TooLarge),
        (
            b"GET / HTTP/1.1\r\nA: b\r\nB: c\r\n\r\n",
            8192,
            1,
            Verdict::TooManyHeaders,
        ),
    ];
    for (input, max_bytes, max_headers, expected) in cases {
        for split in 0..=input.len() {
            let mut scanner = Scanner::new();
            let verdict = scanner.feed(&input[..split], *max_bytes, *max_headers);
            let verdict = if verdict == Verdict::NeedMore {
                scanner.feed(&input[split..], *max_bytes, *max_headers)
            } else {
                verdict
            };
            assert_eq!(verdict, *expected, "{input:?} split {split}");
        }
    }
    let input = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
    for end in 0..input.len() {
        let mut scanner = Scanner::new();
        assert_eq!(
            scanner.feed(&input[..end], 8192, 64),
            Verdict::NeedMore,
            "prefix {end}"
        );
    }
}

#[test]
fn scanner_rejects_transfer_encodings_except_chunked() {
    for value in [b"gzip".as_slice(), b"chunked, gzip"] {
        let mut scanner = Scanner::new();
        let mut input = b"POST / HTTP/1.1\r\nTransfer-Encoding: ".to_vec();
        input.extend_from_slice(value);
        input.extend_from_slice(b"\r\n\r\n");
        assert_eq!(scanner.feed(&input, 8192, 64), Verdict::BadRequest);
    }
}
