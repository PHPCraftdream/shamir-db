//! Privileged replication wire-types (REPLICATION §5, PR5).
//!
//! The replication protocol rides as the single [`crate::DbRequest::Repl`]
//! variant so it can version independently of the client query protocol —
//! R0 implements only `Hello` + `Pull` (§5.3); `Stream`, `InternerSync`
//! and `Status` are later phases and are intentionally absent here.

use serde::{Deserialize, Serialize};

/// Highest replication-protocol version this build speaks/accepts.
///
/// Followers advertise their `proto_ver` in [`ReplRequest::Hello`]; a leader
/// rejects any `proto_ver` strictly greater than this constant (an
/// unrecognized, newer protocol) but accepts anything lower or equal
/// (forward-compat with an older follower). See
/// `repl_handler.rs::handle_repl`'s `Hello` arm for the enforcement point.
pub const CURRENT_REPL_PROTO_VER: u32 = 1;

/// Capability advertised by a follower that runs the universal identity
/// fence (S5c-X PR7): hello + per-pull identity verification. A leader whose
/// repos run `replication_serve = completed_reseed` refuses followers that
/// do NOT advertise this capability (`legacy_follower_refused`) — fail
/// closed against legacy peers that cannot hold a completed-reseed lineage.
pub const REPL_CAP_IDENTITY_FENCE: &str = "identity_fence";

/// Privileged replication request (leader-facing). Carried as the single
/// `DbRequest::Repl` variant so the replication protocol versions
/// independently of the client query protocol (REPLICATION §5, PR5).
/// R0 implements only Hello + Pull (§5.3); Stream/InternerSync/Status
/// are later phases and are intentionally absent here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "repl_op", rename_all = "snake_case")]
pub enum ReplRequest {
    /// Handshake: advertise protocol version + node identity, learn the
    /// leader's epoch and replicable repos.
    Hello {
        /// Replication-protocol version the follower speaks.
        proto_ver: u32,
        /// Stable follower node identity.
        node_id: String,
        /// Optional capability list (e.g. [`REPL_CAP_IDENTITY_FENCE`]).
        /// Additive: a request from an older peer (or one serialized before
        /// this field existed) decodes as `None` — the leader then treats
        /// the follower as legacy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
    /// Pull a batch of changelog events for one repo from `from_version`.
    Pull {
        /// Target database name.
        db: String,
        /// Target repo name.
        repo: String,
        /// Inclusive lower bound of the requested changelog range.
        from_version: u64,
        /// Max events the leader should return in this call.
        limit: u32,
        /// Long-poll budget in ms. `None`/0 = return immediately even if
        /// no events are available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u32>,
        /// Optional capability list, mirroring [`ReplRequest::Hello`] — the
        /// leader re-checks it on every pull (`completed_reseed` repos must
        /// never be served to a legacy peer).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
}

/// Per-repo advertisement in a [`ReplResponse::Hello`] reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplRepoInfo {
    /// Database name.
    pub db: String,
    /// Repo name.
    pub repo: String,
    /// Highest committed version currently in this repo's journal.
    pub current_version: u64,
    /// Lowest version still retained in the journal (G4). R0: 0 (no
    /// retention yet) — follower with bookmark+1 < floor needs reseed.
    pub journal_floor: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_lineage_id: Option<[u8; 16]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_first: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_through: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_digest: Option<String>,
    /// Settled source domain, distinct from eager journal position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_version: Option<u64>,
    /// Named publications visible in full to this caller. Missing on older peers.
    #[serde(default)]
    pub publications: Vec<crate::admin::CreatePublicationOp>,
}

/// Privileged replication reply. Every variant carries `leader_epoch`
/// (VR-style fencing, §5.2): the follower tracks the max epoch seen and
/// drops a connection whose epoch regresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "repl_kind", rename_all = "snake_case")]
pub enum ReplResponse {
    /// Reply to [`ReplRequest::Hello`]: the leader's epoch + the set of
    /// repos it can replicate.
    Hello {
        /// Current leader epoch — followers fence on regressions.
        leader_epoch: u64,
        /// Replicable repos this leader advertises.
        repos: Vec<ReplRepoInfo>,
    },
    /// Reply to [`ReplRequest::Pull`]: a batch of changelog events.
    Pull {
        /// Current leader epoch — followers fence on regressions.
        leader_epoch: u64,
        /// msgpack-encoded `Vec<ChangelogEvent>` — raw events, opaque at
        /// the wire layer (decoded by the follower apply-engine in R1).
        #[serde(with = "serde_bytes")]
        events: Vec<u8>,
        /// Set if a gap was detected (requested `from_version` precedes
        /// `journal_floor`): the follower must reseed from this version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap_at: Option<u64>,
        /// Highest version in the repo at reply time (lag computation).
        current_version: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_lineage_id: Option<[u8; 16]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history_epoch: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap_first: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap_through: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema_digest: Option<String>,
    },
    /// Replication-layer error (bad role, denied repo, unknown repo, stale
    /// epoch). Carries the epoch so the follower can still fence.
    Error {
        /// Current leader epoch — followers fence on regressions.
        leader_epoch: u64,
        /// Coarse classification (`bad_role`, `denied_repo`, `unknown_repo`,
        /// `stale_epoch`, …).
        code: String,
        /// Human-readable detail.
        message: String,
    },
}
