//! Startup verification under the caller's data-directory lock. The log is
//! untrusted; the audit key and checkpoint stores are trusted.

use super::{
    AppenderError, AuditRecoveryError, FjallAuditAppender, Position, CHECKPOINT_KEY,
    MAX_RECORD_BYTES,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use fjall::{Database, Keyspace};
use shamir_connect::common::crypto::constant_time_eq;
use shamir_connect::server::audit_chain::{compute_entry_hmac, AuditEntry};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Only names of the exact shape `audit.log.` + 20 ASCII decimal digits are
/// rotation archives; every other name in the reserved namespace is ignored.
const ROTATED_PREFIX: &[u8] = b"audit.log.";
const GENESIS: Position = (1, [0; 32]);

/// Verified log state recovered from an untrusted on-disk history: the
/// reopened active file at its durable tail, the open checkpoint store, the
/// verified resume position, and the active file's post-recovery size.
pub(super) struct RecoveredLog {
    pub(super) log_path: PathBuf,
    pub(super) log_file: File,
    pub(super) checkpoint: (Arc<Database>, Keyspace),
    pub(super) position: Position,
    pub(super) initial_size: u64,
}

pub(super) fn recover(
    dir: &Path,
    audit_chain_key: &[u8; 32],
    meta_checkpoint: Option<Position>,
) -> Result<RecoveredLog, AuditRecoveryError> {
    recover_with_dir_sync(
        dir,
        audit_chain_key,
        meta_checkpoint,
        super::rotation::sync_parent,
    )
}

/// [`recover`] with the final parent-directory sync injectable: production
/// passes [`super::rotation::sync_parent`], recovery-order tests substitute a
/// closure. No other behavior differs.
pub(super) fn recover_with_dir_sync(
    dir: &Path,
    audit_chain_key: &[u8; 32],
    meta_checkpoint: Option<Position>,
    dir_sync: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<RecoveredLog, AuditRecoveryError> {
    fs::create_dir_all(dir).map_err(|source| io_at(dir, source))?;
    let (log_path, db_path) = FjallAuditAppender::paths(dir);
    let checkpoint = FjallAuditAppender::open_db(&db_path).map_err(|source| {
        AuditRecoveryError::CheckpointStore {
            path: db_path.clone(),
            source,
        }
    })?;
    let stored = read_checkpoint(&db_path, &checkpoint.1)?;
    let mut scan = ChainScan::new(audit_chain_key, [stored, meta_checkpoint])?;
    let mut record = Vec::new();
    for path in ordered_archives(dir)? {
        require_regular(&path)?;
        let file = File::open(&path).map_err(|source| io_at(&path, source))?;
        scan_file(file, &path, &mut record, &mut scan, false)?;
    }
    let active = match require_regular(&log_path) {
        Ok(()) => {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&log_path)
                .map_err(|source| io_at(&log_path, source))?;
            let truncate_to = scan_file(&file, &log_path, &mut record, &mut scan, true)?;
            Some((file, truncate_to))
        }
        Err(AuditRecoveryError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            None
        }
        Err(error) => return Err(error),
    };
    let position = scan.finish()?;
    if let Some((file, Some(offset))) = active {
        file.set_len(offset)
            .map_err(|source| io_at(&log_path, source))?;
    }
    // Reopen through the writer's append/create path and sync that writable
    // handle once, so a verified truncate — and any still-unsynced durable
    // tail this recovery is about to anchor — is durable. An unchanged file
    // is synced here exactly once, not twice.
    let log_file =
        FjallAuditAppender::open_log(&log_path).map_err(|source| io_at(&log_path, source))?;
    log_file
        .sync_all()
        .map_err(|source| io_at(&log_path, source))?;
    // A pre-restart crash may have completed a rename/reopen without the
    // directory sync, and this verified state is about to be anchored: make
    // the containing directory durable before returning. This runs AFTER
    // the verified truncation above and AFTER the reopen/create through
    // `open_log` — the seam exists so tests can observe exactly that order.
    dir_sync(&log_path).map_err(|source| io_at(&log_path, source))?;
    let initial_size = log_file
        .metadata()
        .map_err(|source| io_at(&log_path, source))?
        .len();
    Ok(RecoveredLog {
        log_path,
        log_file,
        checkpoint,
        position,
        initial_size,
    })
}

pub(super) fn read_log_for_verify(dir: &Path) -> Result<Vec<AuditEntry>, AppenderError> {
    let (log_path, _db_path) = FjallAuditAppender::paths(dir);
    let invalid = |error: AuditRecoveryError| {
        AppenderError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            error.to_string(),
        ))
    };

    // Same archive set and content order as boot recovery; this diagnostic
    // reader still does not verify the chain.
    let rotated = if dir.exists() {
        ordered_archives(dir).map_err(invalid)?
    } else {
        Vec::new()
    };

    let mut all_paths = rotated;
    if log_path.exists() {
        all_paths.push(log_path);
    }

    let mut out = Vec::new();
    for path in all_paths {
        let f = File::open(&path)?;
        let r = BufReader::new(f);
        for line in r.lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            out.push(line_to_entry(&line)?);
        }
    }
    Ok(out)
}

/// Decode a free-text field that was produced by [`super::escape_field`].
fn unescape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse one fixed-format log line back into an [`AuditEntry`].
fn line_to_entry(line: &str) -> Result<AuditEntry, AppenderError> {
    let invalid = |msg: &'static str| {
        AppenderError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
    };

    let mut cols = line.splitn(11, '\t');
    let seq: u64 = cols
        .next()
        .ok_or_else(|| invalid("missing seq"))?
        .parse()
        .map_err(|_| invalid("seq not u64"))?;
    let ts_ns: u64 = cols
        .next()
        .ok_or_else(|| invalid("missing ts_ns"))?
        .parse()
        .map_err(|_| invalid("ts_ns not u64"))?;
    let event = unescape_field(cols.next().ok_or_else(|| invalid("missing event"))?);
    let transport = unescape_field(cols.next().ok_or_else(|| invalid("missing transport"))?);
    let user = unescape_field(cols.next().ok_or_else(|| invalid("missing user"))?);
    let ip_subnet = unescape_field(cols.next().ok_or_else(|| invalid("missing ip_subnet"))?);

    let session_id_prefix_v = URL_SAFE_NO_PAD
        .decode(
            cols.next()
                .ok_or_else(|| invalid("missing session_id_prefix"))?,
        )
        .map_err(|_| invalid("session_id_prefix: invalid base64url"))?;
    if session_id_prefix_v.len() != 8 {
        return Err(invalid("session_id_prefix not 8 bytes"));
    }
    let mut session_id_prefix = [0u8; 8];
    session_id_prefix.copy_from_slice(&session_id_prefix_v);

    let result = unescape_field(cols.next().ok_or_else(|| invalid("missing result"))?);

    let details = URL_SAFE_NO_PAD
        .decode(
            cols.next()
                .ok_or_else(|| invalid("missing details_canonical_msgpack"))?,
        )
        .map_err(|_| invalid("details_canonical_msgpack: invalid base64url"))?;

    let prev_hmac_v = URL_SAFE_NO_PAD
        .decode(cols.next().ok_or_else(|| invalid("missing prev_hmac"))?)
        .map_err(|_| invalid("prev_hmac: invalid base64url"))?;
    if prev_hmac_v.len() != 32 {
        return Err(invalid("prev_hmac not 32 bytes"));
    }
    let mut prev_hmac = [0u8; 32];
    prev_hmac.copy_from_slice(&prev_hmac_v);

    let hmac_v = URL_SAFE_NO_PAD
        .decode(cols.next().ok_or_else(|| invalid("missing hmac"))?)
        .map_err(|_| invalid("hmac: invalid base64url"))?;
    if hmac_v.len() != 32 {
        return Err(invalid("hmac not 32 bytes"));
    }
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&hmac_v);

    Ok(AuditEntry {
        seq,
        ts_ns,
        event,
        transport,
        user,
        ip_subnet,
        session_id_prefix,
        result,
        details_canonical_msgpack: details,
        prev_hmac,
        hmac,
    })
}

fn io_at(path: &Path, source: io::Error) -> AuditRecoveryError {
    AuditRecoveryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn read_checkpoint(
    path: &Path,
    keyspace: &Keyspace,
) -> Result<Option<Position>, AuditRecoveryError> {
    let value =
        keyspace
            .get(CHECKPOINT_KEY)
            .map_err(|source| AuditRecoveryError::CheckpointStore {
                path: path.to_path_buf(),
                source: source.into(),
            })?;
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: &[u8] = value.as_ref();
    decode_checkpoint(bytes)
        .map(Some)
        .ok_or_else(|| AuditRecoveryError::MalformedCheckpoint {
            path: path.to_path_buf(),
            length: bytes.len(),
        })
}

pub(super) fn decode_checkpoint(bytes: &[u8]) -> Option<Position> {
    let (seq, hmac) = bytes.split_first_chunk::<8>()?;
    Some((u64::from_be_bytes(*seq), hmac.try_into().ok()?))
}

fn require_regular(path: &Path) -> Result<(), AuditRecoveryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_at(path, source))?;
    if !metadata.is_file() {
        return Err(AuditRecoveryError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Classify a directory entry in the reserved `audit.log.*` namespace.
/// Only the exact rotation shape — the prefix followed by exactly 20 ASCII
/// decimal digits that fit a `u64` — is an archive (`Some`); any other
/// prefixed name is ignored with a warning (`None`); a 20-digit name whose
/// value overflows `u64` is a refusal.
pub(super) fn archive_name(path: &Path) -> Result<Option<u64>, AuditRecoveryError> {
    let Some(name) = path.file_name() else {
        return Ok(None);
    };
    let Some(suffix) = name.as_encoded_bytes().strip_prefix(ROTATED_PREFIX) else {
        return Ok(None);
    };
    if suffix.len() != 20 || !suffix.iter().all(u8::is_ascii_digit) {
        tracing::warn!(
            ?path,
            "ignoring a non-rotation file in the reserved audit log namespace"
        );
        return Ok(None);
    }
    // 20 ASCII digits, so UTF-8 cannot fail; only overflow can.
    let digits = std::str::from_utf8(suffix).unwrap_or_default();
    digits
        .parse::<u64>()
        .map(Some)
        .map_err(|_| AuditRecoveryError::InvalidRotationName {
            path: path.to_path_buf(),
        })
}

/// Rotation archives in the directory, ordered by the sequence of each
/// archive's first record (filename order is only a tiebreak). The chain
/// sequence/link/HMAC checks remain the authority; this ordering only makes
/// wall-clock-rolled-back archive names recoverable. Memory is bounded by one
/// record's working buffer and the path list; at most one archive is open at
/// a time and no record rows are retained.
pub(super) fn ordered_archives(dir: &Path) -> Result<Vec<PathBuf>, AuditRecoveryError> {
    let mut archives = Vec::new();
    for entry in fs::read_dir(dir).map_err(|source| io_at(dir, source))? {
        let entry = entry.map_err(|source| io_at(dir, source))?;
        if archive_name(&entry.path())?.is_some() {
            archives.push(entry.path());
        }
    }
    let mut ordered: Vec<(u64, PathBuf)> = Vec::with_capacity(archives.len());
    for path in archives {
        require_regular(&path)?;
        ordered.push((first_record_seq(&path)?, path));
    }
    ordered.sort_unstable();
    Ok(ordered.into_iter().map(|(_, path)| path).collect())
}

/// Sequence of an archive's first record, read without retaining the row.
/// An exact-pattern archive that is empty, or whose first line is not a
/// terminated record of the fixed format, is malformed.
fn first_record_seq(path: &Path) -> Result<u64, AuditRecoveryError> {
    let file = File::open(path).map_err(|source| io_at(path, source))?;
    let mut record = Vec::new();
    let size = (&mut BufReader::new(file))
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', &mut record)
        .map_err(|source| io_at(path, source))?;
    if size == 0 {
        return Err(AuditRecoveryError::MalformedRecord {
            path: path.to_path_buf(),
            offset: 0,
            source: AppenderError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty audit archive has no records",
            )),
        });
    }
    if size > MAX_RECORD_BYTES {
        return Err(AuditRecoveryError::RecordTooLarge {
            path: path.to_path_buf(),
            offset: 0,
        });
    }
    if !record.ends_with(b"\n") {
        return Err(AuditRecoveryError::ArchivedFragment {
            path: path.to_path_buf(),
            offset: 0,
        });
    }
    record.pop();
    let entry = parse_record(record.strip_suffix(b"\r").unwrap_or(&record)).map_err(|source| {
        AuditRecoveryError::MalformedRecord {
            path: path.to_path_buf(),
            offset: 0,
            source,
        }
    })?;
    Ok(entry.seq)
}

/// Return the truncation point only for an unterminated active record.
fn scan_file(
    file: impl Read,
    path: &Path,
    record: &mut Vec<u8>,
    scan: &mut ChainScan<'_>,
    active: bool,
) -> Result<Option<u64>, AuditRecoveryError> {
    let mut reader = BufReader::new(file);
    let mut offset = 0u64;
    loop {
        record.clear();
        let size = (&mut reader)
            .take((MAX_RECORD_BYTES + 1) as u64)
            .read_until(b'\n', record)
            .map_err(|source| io_at(path, source))?;
        if size == 0 {
            return Ok(None);
        }
        if size > MAX_RECORD_BYTES {
            return Err(AuditRecoveryError::RecordTooLarge {
                path: path.to_path_buf(),
                offset,
            });
        }
        let terminated = record.ends_with(b"\n");
        if !terminated {
            if !active {
                return Err(AuditRecoveryError::ArchivedFragment {
                    path: path.to_path_buf(),
                    offset,
                });
            }
            if parse_record(record.strip_suffix(b"\r").unwrap_or(record)).is_ok() {
                return Err(AuditRecoveryError::UnterminatedActiveRecord {
                    path: path.to_path_buf(),
                    offset,
                });
            }
            return Ok(Some(offset));
        }
        record.pop();
        let entry =
            parse_record(record.strip_suffix(b"\r").unwrap_or(record)).map_err(|source| {
                AuditRecoveryError::MalformedRecord {
                    path: path.to_path_buf(),
                    offset,
                    source,
                }
            })?;
        scan.verify(&entry, path, offset)?;
        offset += size as u64;
    }
}

fn parse_record(record: &[u8]) -> Result<AuditEntry, AppenderError> {
    let line = std::str::from_utf8(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    line_to_entry(line)
}

struct ChainScan<'a> {
    key: &'a [u8; 32],
    anchors: [Option<Position>; 2],
    first_seq: Option<u64>,
    tail: Position,
    anchored: bool,
}

impl<'a> ChainScan<'a> {
    fn new(key: &'a [u8; 32], anchors: [Option<Position>; 2]) -> Result<Self, AuditRecoveryError> {
        for (next_seq, digest) in anchors.iter().flatten() {
            if *next_seq == 0 || (*next_seq == GENESIS.0 && !constant_time_eq(digest, &GENESIS.1)) {
                return Err(AuditRecoveryError::InvalidAnchor {
                    next_seq: *next_seq,
                });
            }
        }
        match &anchors {
            [Some((a_seq, a_mac)), Some((b_seq, b_mac))]
                if a_seq == b_seq && !constant_time_eq(a_mac, b_mac) =>
            {
                return Err(AuditRecoveryError::ConflictingAnchors { next_seq: *a_seq });
            }
            _ => {}
        }
        Ok(Self {
            key,
            anchors,
            first_seq: None,
            tail: GENESIS,
            anchored: false,
        })
    }

    fn check_anchors(&mut self, (next_seq, digest): Position) -> Result<(), AuditRecoveryError> {
        for (seq, mac) in self.anchors.iter().flatten() {
            if *seq == next_seq {
                if !constant_time_eq(mac, &digest) {
                    return Err(AuditRecoveryError::AnchorMismatch { next_seq });
                }
                self.anchored = true;
            }
        }
        Ok(())
    }

    fn verify(
        &mut self,
        entry: &AuditEntry,
        path: &Path,
        offset: u64,
    ) -> Result<(), AuditRecoveryError> {
        if self.first_seq.is_none() {
            self.first_seq = Some(entry.seq);
            if entry.seq > GENESIS.0 {
                self.tail = (entry.seq, entry.prev_hmac);
            }
        }
        if entry.seq != self.tail.0 {
            return Err(AuditRecoveryError::SequenceGap {
                path: path.to_path_buf(),
                offset,
                expected: self.tail.0,
                found: entry.seq,
            });
        }
        if !constant_time_eq(&entry.prev_hmac, &self.tail.1) {
            return Err(AuditRecoveryError::LinkMismatch {
                path: path.to_path_buf(),
                offset,
            });
        }
        let computed = compute_entry_hmac(self.key, entry).map_err(|source| {
            AuditRecoveryError::InvalidCanonicalRecord {
                path: path.to_path_buf(),
                offset,
                source,
            }
        })?;
        if !constant_time_eq(&computed, &entry.hmac) {
            return Err(AuditRecoveryError::HmacMismatch {
                path: path.to_path_buf(),
                offset,
            });
        }
        // The first predecessor is eligible only after authenticating the
        // first retained record. All later positions are checked on arrival.
        if self.first_seq == Some(entry.seq) {
            self.check_anchors(self.tail)?;
        }
        // The existing writer saturates at u64::MAX. Do not resume at a
        // position where its very next append could no longer advance it.
        let next_seq = entry
            .seq
            .checked_add(1)
            .filter(|seq| *seq < u64::MAX)
            .ok_or_else(|| AuditRecoveryError::SequenceExhausted {
                path: path.to_path_buf(),
                offset,
            })?;
        self.tail = (next_seq, computed);
        self.check_anchors(self.tail)
    }

    fn finish(self) -> Result<Position, AuditRecoveryError> {
        for (next_seq, _) in self.anchors.into_iter().flatten() {
            if next_seq > self.tail.0 {
                return Err(AuditRecoveryError::AnchorPastTail {
                    next_seq,
                    verified_next_seq: self.tail.0,
                });
            }
        }
        // Historical anchors before first_seq are superseded only if an
        // in-range anchor matched. At genesis (including empty history),
        // all in-range anchors have already matched or are valid genesis.
        if let Some(first_seq) = self
            .first_seq
            .filter(|seq| *seq > GENESIS.0 && !self.anchored)
        {
            return Err(AuditRecoveryError::UnanchoredSuffix { first_seq });
        }
        Ok(self.tail)
    }
}
