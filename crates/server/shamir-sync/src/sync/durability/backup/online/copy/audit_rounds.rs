//! Fail-closed guard of the audit capture rounds.

use std::collections::BTreeSet;

use super::super::copy_types::CopiedFile;

/// The archive that an EARLIER round of the same data-directory pass captured
/// and the accepted round no longer lists — `None` when the accepted round
/// covers every archive an earlier round copied.
///
/// Both anchors are captured once, in phase 1, BEFORE any audit round, so
/// every archive an earlier round copied belongs to the byte range those
/// anchors authenticate. When a rotation advances the live anchors and a
/// retention sweep then prunes such an archive, the accepted round starts
/// after it: the snapshot would carry anchors over a prefix it does not hold,
/// and boot refuses it with `UnanchoredSuffix`. Reconciliation must not make
/// that shape look clean by unlinking the copy, so the pass refuses here. The
/// case in which the prefix was already gone before ANY round listed it is the
/// documented pre-existing limit (see the guide).
pub(super) fn discarded_captured_archive<'a>(
    earlier: &'a BTreeSet<String>,
    accepted: &[CopiedFile],
) -> Option<&'a str> {
    earlier
        .iter()
        .find(|name| !accepted.iter().any(|file| &file.rel_path == *name))
        .map(String::as_str)
}
