//! Container-level refusals: corrupt frame checksums, truncation, trailing
//! bytes, over-limit frames, unsupported version/flags, non-canonical value
//! encoding, and cancellation — each proves the typed variant and that the
//! attempt leaves the target absent with no lock or stage behind.

use shamir_server::logical_import::LogicalImportError;
use tokio_util::sync::CancellationToken;

use crate::logical_import::common::assert_no_leftovers;
use crate::logical_import::common::build_fixture;
use crate::logical_import::common::export_args;
use crate::logical_import::common::export_now;
use crate::logical_import::common::handcrafted_bytes;
use crate::logical_import::common::import_args;
use crate::logical_import::common::import_now;
use crate::logical_import::common::replace_frame_payload;
use crate::logical_import::common::runtime;
use crate::logical_import::common::walk_frames;
use crate::logical_import::common::write_bytes;
use crate::logical_import::common::ALICE_FIXSTR;
use crate::logical_import::common::FRAME_LIMIT;

/// A flipped payload byte fails the frame checksum the verifier computes;
/// the refusal is the typed Artifact variant with checksum context.
#[test]
fn corrupt_frame_checksum_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut bytes = handcrafted_bytes(None, None);
    let (_, frames) = walk_frames(&bytes);
    let payload_at = frames[0].start + 4;
    bytes[payload_at] ^= 0x01;
    let artifact = write_bytes(&root.join("corrupt.mpack"), &bytes);
    let target = root.join("target");
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error.to_string().contains("frame checksum mismatch"));
    assert_no_leftovers(&root, &target);
}

/// A file cut off inside a frame is refused as a truncated seed stream.
#[test]
fn truncated_artifact_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut bytes = handcrafted_bytes(None, None);
    let (_, frames) = walk_frames(&bytes);
    bytes.truncate(frames[1].start + 10);
    let artifact = write_bytes(&root.join("cut.mpack"), &bytes);
    let target = root.join("target");
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error.to_string().contains("seed I/O"));
    assert_no_leftovers(&root, &target);
}

/// Any byte after the trailer is refused; nothing partial is published.
#[test]
fn trailing_bytes_after_trailer_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut bytes = handcrafted_bytes(None, None);
    bytes.push(0);
    let artifact = write_bytes(&root.join("trailing.mpack"), &bytes);
    let target = root.join("target");
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error.to_string().contains("trailing bytes"));
    assert_no_leftovers(&root, &target);
}

/// A frame whose declared length exceeds the 16 MiB budget is rejected
/// before any allocation (the corruption-suite technique reused here for
/// over-limit values).
#[test]
fn over_limit_frame_is_refused_before_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut bytes = handcrafted_bytes(None, None);
    let (_, frames) = walk_frames(&bytes);
    bytes[frames[0].start..frames[0].start + 4]
        .copy_from_slice(&((FRAME_LIMIT + 1) as u32).to_be_bytes());
    bytes.truncate(frames[0].start + 8);
    let artifact = write_bytes(&root.join("over.mpack"), &bytes);
    let target = root.join("target");
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error.to_string().contains("frame length is outside limits"));
    assert_no_leftovers(&root, &target);
}

/// Unsupported container version bits and unsupported flags bits are both
/// refused with their typed Artifact variant and specific messages.
#[test]
fn unsupported_version_and_flags_bits_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("target");

    let mut version = handcrafted_bytes(None, None);
    version[8] ^= 0xff;
    let artifact = write_bytes(&root.join("version.mpack"), &version);
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error
        .to_string()
        .contains("unsupported logical-dump version"));
    assert_no_leftovers(&root, &target);

    let mut flags = handcrafted_bytes(None, None);
    flags[10] = 1;
    let artifact = write_bytes(&root.join("flags.mpack"), &flags);
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    assert!(error.to_string().contains("unsupported logical-dump flags"));
    assert_no_leftovers(&root, &target);
}

/// A value encoded in non-shortest MessagePack form (str8 where the
/// canonical encoding is fixstr) decodes but fails the canonical
/// re-encoding comparison; the import refuses with the typed variant.
#[test]
fn non_canonical_value_encoding_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut bytes = handcrafted_bytes(None, None);
    let (_, frames) = walk_frames(&bytes);
    let needle: &[u8] = &ALICE_FIXSTR;
    let at = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("identity fixstr present");
    let mut frame_index = None;
    for (index, frame) in frames.iter().enumerate() {
        if frame.start <= at && at < frame.start + frame.wire_len {
            frame_index = Some(index);
            break;
        }
    }
    let frame_index = frame_index.expect("edited frame found");
    let frame_start = frames[frame_index].start;
    // Patch the frame PAYLOAD, not the wire: extract the payload with its
    // original length first, apply fixstr(5) -> str8(5) inside it, then
    // rebuild the frame (new length + recomputed checksum over the frame
    // sequence and the patched payload) so the verifier passes checksum and
    // decoding and reaches the canonical-form comparison.
    let payload_length =
        u32::from_be_bytes(bytes[frame_start..frame_start + 4].try_into().unwrap()) as usize;
    let mut payload = bytes[frame_start + 4..frame_start + 4 + payload_length].to_vec();
    let at = payload
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("identity fixstr present in the frame payload");
    // fixstr(5) -> str8(5): marker + explicit length + the same text.
    let mut replacement = Vec::with_capacity(needle.len() + 1);
    replacement.push(0xd9);
    replacement.push(5);
    replacement.extend_from_slice(&needle[1..]);
    payload.splice(at..at + needle.len(), replacement);
    replace_frame_payload(&mut bytes, frame_start, frame_index as u64, &payload);
    let artifact = write_bytes(&root.join("noncanon.mpack"), &bytes);
    let target = root.join("target");
    let error = import_now(&import_args(&artifact, &target), CancellationToken::new()).unwrap_err();
    assert!(matches!(error, LogicalImportError::Artifact(_)));
    let message = error.to_string();
    assert!(
        message.contains("noncanonical") || message.contains("canonically encoded"),
        "unexpected refusal: {message}"
    );
    assert_no_leftovers(&root, &target);
}

/// Cancellation observed at the artifact streaming checkpoint (the
/// cancellation-checked reader wrapper) aborts the import with the typed
/// Cancelled variant and discards the stage.
#[test]
fn cancelled_import_aborts_with_no_stage() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fixture = runtime().block_on(build_fixture(&root));
    drop(fixture.home);
    let artifact = root.join("dump.mpack");
    export_now(&root, &export_args("app", &artifact, false));
    let target = root.join("target");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = import_now(&import_args(&artifact, &target), cancel).unwrap_err();
    assert!(matches!(error, LogicalImportError::Cancelled));
    assert!(error.to_string().contains("cancelled"));
    assert_no_leftovers(&root, &target);
}
