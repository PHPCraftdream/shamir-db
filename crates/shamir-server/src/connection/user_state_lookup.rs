//! [`UserStateLookup`] adapter backed by [`FjallUserDirectory`].

use shamir_connect::server::resume::UserStateLookup;

use crate::user_directory::FjallUserDirectory;

/// Adapter: implement [`UserStateLookup`] against [`FjallUserDirectory`].
///
/// Returns the user's `tickets_invalid_before_ns` if they exist, `None`
/// if the `user_id` is not found. An unrecognised user_id causes
/// `process_resume` to return `AuthFailed` per spec §5.4 step 8.
///
/// `pub(crate)` so the fail-closed behaviour (unknown → `None`) can be
/// exercised directly by an in-crate test (`src/tests/user_state_lookup_tests`).
pub(crate) struct RedbUserStateLookup<'a>(pub(crate) &'a FjallUserDirectory);

impl UserStateLookup for RedbUserStateLookup<'_> {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64> {
        // Distinguishes "unknown user" (None — resume rejects per spec §5.4
        // step 8) from "known user with tib = 0" (Some(0) — all tickets
        // valid). `state_by_user_id` resolves the user_id through the
        // durable reverse index, so a removed/never-existed account yields
        // `None` rather than collapsing to the fail-open `Some(0)` the old
        // implementation returned for every lookup.
        self.0
            .state_by_user_id(user_id)
            .map(|s| s.tickets_invalid_before_ns)
    }
}
