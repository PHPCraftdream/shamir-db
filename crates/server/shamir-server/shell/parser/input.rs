//! The interactive line tokenizer (design §4.3): one bounded physical line
//! from stdin, with the CR / control-byte / over-bound / UTF-8 policy of the
//! old monolith, byte for byte.

use std::io::Read;

/// Hard byte bound for one interactive input line.
const LINE_MAX_BYTES: usize = 8 * 1024;

/// One outcome of the bounded interactive line reader.
pub(super) enum ReadLine {
    /// A complete, valid line (terminator stripped).
    Line(String),
    /// Clean EOF at the start of a line.
    Eof,
    /// stdin read error: terminal for the loop (never retried).
    Terminal,
    /// Invalid physical line — control byte, lone CR, invalid UTF-8 or over
    /// [`LINE_MAX_BYTES`]; the line was drained, so no second command surfaces.
    Invalid,
}

/// Reads one physical line with a hard byte bound. A CR terminates only as
/// CRLF; a lone CR is an invalid, drained line.
pub(super) fn read_line_bounded(reader: &mut dyn Read) -> ReadLine {
    let mut buffer: Vec<u8> = Vec::new();
    let mut byte_buf = [0u8; 1];
    let mut over_bound = false;
    loop {
        let byte = match reader.read(&mut byte_buf) {
            Ok(0) => {
                return if over_bound {
                    ReadLine::Invalid
                } else if buffer.is_empty() {
                    ReadLine::Eof
                } else {
                    finish_line(&buffer)
                };
            }
            Ok(n) => {
                debug_assert!(n == 1);
                byte_buf[0]
            }
            Err(_) => return ReadLine::Terminal,
        };
        if byte == b'\n' {
            return if over_bound {
                ReadLine::Invalid
            } else {
                finish_line(&buffer)
            };
        }
        if byte == b'\r' {
            // CR terminates only when immediately followed by LF; CR at EOF
            // ends the partial line; a lone CR is refused and drained.
            match reader.read(&mut byte_buf) {
                Ok(n) if n == 0 || byte_buf[0] == b'\n' => {
                    return if over_bound {
                        ReadLine::Invalid
                    } else {
                        finish_line(&buffer)
                    };
                }
                Ok(_) => {
                    drain_to_lf(reader, Some(byte_buf[0]));
                    return ReadLine::Invalid;
                }
                Err(_) => return ReadLine::Terminal,
            }
        }
        // Other control bytes are refused: drain and report invalid.
        if byte < 0x20 || byte == 0x7f {
            drain_to_lf(reader, None);
            return ReadLine::Invalid;
        }
        if buffer.len() >= LINE_MAX_BYTES {
            over_bound = true;
            continue;
        }
        buffer.push(byte);
    }
}

/// Consumes bytes up to and including the next LF (or EOF/error), so an
/// invalid physical line leaves nothing behind; no unbounded allocation.
fn drain_to_lf(reader: &mut dyn Read, first: Option<u8>) {
    if first == Some(b'\n') {
        return;
    }
    let mut byte_buf = [0u8; 1];
    loop {
        match reader.read(&mut byte_buf) {
            Ok(0) | Err(_) => return,
            Ok(_) if byte_buf[0] == b'\n' => return,
            Ok(_) => {}
        }
    }
}

/// Decodes one bounded line; invalid UTF-8 is a refusal, never lossy text.
fn finish_line(buffer: &[u8]) -> ReadLine {
    match String::from_utf8(buffer.to_vec()) {
        Ok(line) => ReadLine::Line(line),
        Err(_) => ReadLine::Invalid,
    }
}
