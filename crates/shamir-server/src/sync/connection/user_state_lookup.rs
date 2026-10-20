//! [`UserStateLookup`] adapter backed by [`FjallUserDirectory`].

use shamir_connect::server::resume::{ResumeUserState, UserStateLookup};

use crate::user_directory::FjallUserDirectory;

/// Adapter: implement [`UserStateLookup`] against [`FjallUserDirectory`].
///
/// Returns the user's CURRENT authoritative state `(username, roles,
/// superuser, tickets_invalid_before_ns)` if they exist, `None` if the
/// `user_id` is not found. An unrecognised user_id causes `process_resume`
/// to return `AuthFailed` per spec §5.4 step 8, and — since task #558 — the
/// resumed session is built from this snapshot, NOT from any (now-removed)
/// authorization field baked into the ticket (design doc §5).
///
/// `pub(crate)` so the fail-closed behaviour (unknown → `None`) can be
/// exercised directly by an in-crate test (`src/tests/user_state_lookup_tests`).
pub(crate) struct RedbUserStateLookup<'a>(pub(crate) &'a FjallUserDirectory);

impl UserStateLookup for RedbUserStateLookup<'_> {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<ResumeUserState> {
        // Distinguishes "unknown user" (None — resume rejects per spec §5.4
        // step 8) from "known user with tib = 0" (Some(state) — all tickets
        // valid). `state_by_user_id` resolves the user_id through the
        // durable reverse index, so a removed/never-existed account yields
        // `None` rather than collapsing to the fail-open `Some(0)` the old
        // implementation returned for every lookup.
        //
        // Task #558: map the FULL `UserDirectoryState` across the crate
        // boundary into `ResumeUserState` so resume can build the session
        // from the directory's current `(username, roles, superuser)`, not
        // from a ticket snapshot.
        self.0.state_by_user_id(user_id).map(|s| ResumeUserState {
            username: s.username,
            roles: s.roles,
            superuser: s.superuser,
            replicator: s.replicator,
            tickets_invalid_before_ns: s.tickets_invalid_before_ns,
        })
    }
}
