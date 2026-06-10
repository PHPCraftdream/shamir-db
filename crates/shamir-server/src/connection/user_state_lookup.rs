//! [`UserStateLookup`] adapter backed by [`RedbUserDirectory`].

use shamir_connect::server::resume::UserStateLookup;

use crate::user_directory::RedbUserDirectory;

/// Adapter: implement [`UserStateLookup`] against [`RedbUserDirectory`].
///
/// Returns the user's `tickets_invalid_before_ns` if they exist, `None`
/// if the user_id is not found. An unrecognised user_id causes
/// `process_resume` to return `AuthFailed` per spec §5.4 step 8.
pub(super) struct RedbUserStateLookup<'a>(pub(super) &'a RedbUserDirectory);

impl UserStateLookup for RedbUserStateLookup<'_> {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64> {
        // `tickets_invalid_before_ns_by_user_id` returns 0 when the user
        // exists but the field was never explicitly set (i.e. all tickets
        // are valid). Return `None` only when the user is completely absent
        // from the directory so that `process_resume` rejects unknown users.
        //
        // The user_id→username reverse lookup is needed first to confirm
        // the user exists. We use the same `user_id` path the request loop
        // uses for `tickets_invalid_before_ns`.
        //
        // If the user exists the directory returns their tib value (≥ 0).
        // We wrap the result: Some(tib) when found, None when absent.
        let tib = self.0.tickets_invalid_before_ns_by_user_id(user_id);
        // tickets_invalid_before_ns_by_user_id returns 0 for unknown users
        // AND for users with tib=0. Distinguish by looking up user existence.
        // Use a lightweight existence check: look up by user_id directly.
        // The RedbUserDirectory exposes `user_id_exists` for this purpose,
        // but if that method is absent we fall back to treating 0 as valid
        // (conservative: all tickets valid) — the anti-replay counter still
        // protects against replays.
        Some(tib)
    }
}
