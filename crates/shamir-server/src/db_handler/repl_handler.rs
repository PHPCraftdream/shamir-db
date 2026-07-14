//! Leader-side handler for the privileged replication pull-API
//! (REPLICATION §5.1/§5.2/§5.4/§5.6).
//!
//! Dispatched from `handler.rs` as `DbRequest::Repl(repl_req) =>
//! DbResponse::Repl(self.handle_repl(session, repl_req).await)`.
//!
//! # Authorisation model
//!
//! Replication is a **privileged** operation: the calling session must hold
//! the `replicator` capability flag (task #621; or be a superuser). On top
//! of that, every repo
//! advertised in `Hello` / served by `Pull` is individually authorised via
//! `authorize_access(actor, store(db,repo), Read)` — repos the caller cannot
//! read are silently omitted from `Hello` (no existence leak) and rejected
//! with `denied_repo` on direct `Pull`.
//!
//! # Long-poll (§5.1 / §5.6)
//!
//! `Pull.wait_ms` requests a polite long-poll: if no events are immediately
//! available the handler re-reads the journal in a short-step loop
//! (`POLL_STEP`) until the deadline expires or events appear. This is **not**
//! a subscription — the leader holds no per-follower state, takes no locks,
//! and never blocks writers (§5.6). The loop is deadline-bounded so an empty
//! tail always returns within `~wait_ms`.

use std::time::{Duration, Instant};

use shamir_connect::server::session::Session;
use shamir_db::access::{Action, ResourcePath};
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplRequest, ReplResponse};

use super::handler::{session_actor, ShamirDbHandler};

/// Long-poll re-read step. Short enough that the handler stays responsive
/// (new events appear within ~this interval), long enough to avoid a busy
/// loop when the tail is empty. See §5.1.
const POLL_STEP: Duration = Duration::from_millis(50);

impl ShamirDbHandler {
    /// Entry point for `DbRequest::Repl`. Enforces the capability gate, then
    /// dispatches to [`Self::handle_hello`] / [`Self::handle_pull`].
    pub(super) async fn handle_repl(&self, session: &Session, req: ReplRequest) -> ReplResponse {
        // Capability gate — deny-by-default. Superuser bypasses (§5.4).
        //
        // Task #621: `replicator` is now an authoritative
        // `SessionPermissions::is_replicator` flag (mirrors `is_superuser`),
        // NOT a role string — the literal `"replicator"` string is reserved
        // at the directory write boundary (`FjallUserDirectory::update_roles`),
        // so `has_role("replicator")` would never match a real account.
        if !(session.permissions.is_superuser || session.permissions.is_replicator) {
            return ReplResponse::Error {
                leader_epoch: self.leader_epoch,
                code: "bad_role".into(),
                message: "replication requires the `replicator` role".into(),
            };
        }

        match req {
            ReplRequest::Hello {
                proto_ver: _,
                node_id: _,
            } => {
                // TODO(R1): negotiate proto_ver — for R0 we accept any.
                self.handle_hello(session).await
            }
            ReplRequest::Pull {
                db,
                repo,
                from_version,
                limit,
                wait_ms,
            } => {
                self.handle_pull(session, &db, &repo, from_version, limit, wait_ms)
                    .await
            }
        }
    }

    /// Gather the set of repos this leader can replicate to the caller.
    /// Repos the caller cannot read are omitted (no existence leak).
    async fn handle_hello(&self, session: &Session) -> ReplResponse {
        let actor = session_actor(session);
        let mut repos = Vec::new();

        for db_name in self.db.list_dbs() {
            let Some(db_instance) = self.db.get_db(&db_name) else {
                continue;
            };
            for repo_name in db_instance.list_repos() {
                let path = ResourcePath::store(&db_name, &repo_name);
                // Skip repos the caller cannot read — do NOT leak existence.
                if self
                    .db
                    .authorize_access(&actor, &path, Action::Read)
                    .await
                    .is_err()
                {
                    continue;
                }
                let current_version = self
                    .db
                    .current_commit_version(&db_name, &repo_name)
                    .await
                    .unwrap_or(0);
                repos.push(ReplRepoInfo {
                    db: db_name.clone(),
                    repo: repo_name,
                    current_version,
                    // R0: no retention — the journal floor is 0 (G4).
                    journal_floor: 0,
                });
            }
        }

        ReplResponse::Hello {
            leader_epoch: self.leader_epoch,
            repos,
        }
    }

    /// Pull a batch of changelog events for one repo, with optional
    /// deadline-bounded long-poll.
    async fn handle_pull(
        &self,
        session: &Session,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> ReplResponse {
        let actor = session_actor(session);

        // Per-repo authorisation.
        let path = ResourcePath::store(db, repo);
        if let Err(e) = self.db.authorize_access(&actor, &path, Action::Read).await {
            return ReplResponse::Error {
                leader_epoch: self.leader_epoch,
                code: "denied_repo".into(),
                message: e.to_string(),
            };
        }

        // Clamp limit to a non-zero floor so the journal read always has
        // room to return at least one event. A `limit == 0` request is
        // treated as "give me what you can" — we use 1 as the effective
        // floor. (The wire layer allows 0 as "no events requested"; we
        // interpret it as the minimum useful batch.)
        let effective_limit = limit.max(1) as usize;

        // First read — if we get events or the caller didn't ask to wait,
        // return immediately.
        let mut jr = match self
            .db
            .read_changelog_from_journal(db, repo, from_version, effective_limit)
            .await
        {
            Some(jr) => jr,
            None => {
                // db/repo does not exist (shouldn't happen after the auth
                // gate above, but the journal may not be initialised yet).
                return ReplResponse::Error {
                    leader_epoch: self.leader_epoch,
                    code: "unknown_repo".into(),
                    message: format!("repository '{db}/{repo}' not found or journal unavailable"),
                };
            }
        };

        // Long-poll loop (§5.1): only when the first read was empty AND the
        // caller supplied a positive wait budget. Deadline-bounded so the
        // handler never hangs.
        if jr.events.is_empty() {
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    let deadline = Instant::now() + Duration::from_millis(u64::from(ms));
                    while jr.events.is_empty() {
                        // Sleep first, then re-read: the initial read was
                        // already empty, and this avoids a redundant
                        // immediate re-read of the same empty tail.
                        let now = Instant::now();
                        if now >= deadline {
                            break;
                        }
                        // Never sleep past the deadline.
                        let remaining = deadline - now;
                        tokio::time::sleep(remaining.min(POLL_STEP)).await;

                        match self
                            .db
                            .read_changelog_from_journal(db, repo, from_version, effective_limit)
                            .await
                        {
                            Some(fresh) => jr = fresh,
                            None => break,
                        }
                    }
                }
            }
        }

        // Encode the events. `to_vec_named` produces a canonical map-style
        // msgpack so the follower can decode field-by-field.
        let events_bytes = match rmp_serde::to_vec_named(&jr.events) {
            Ok(b) => b,
            Err(e) => {
                return ReplResponse::Error {
                    leader_epoch: self.leader_epoch,
                    code: "encode_error".into(),
                    message: format!("failed to encode changelog events: {e}"),
                };
            }
        };

        let current_version = self
            .db
            .current_commit_version(db, repo)
            .await
            .unwrap_or(from_version);

        ReplResponse::Pull {
            leader_epoch: self.leader_epoch,
            events: events_bytes,
            gap_at: jr.gap_at,
            current_version,
        }
    }
}
