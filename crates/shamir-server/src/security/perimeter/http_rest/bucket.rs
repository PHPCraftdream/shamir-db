use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use shamir_connect::common::crypto::try_random_array;
use shamir_connect::server::origin::SessionOrigin;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::{TokenId, Verified};

const SESSION_BUCKET_CAP: usize = 4096;
type SessionBuckets = DashMap<TokenId, Arc<Session>, shamir_collections::THasher>;

/// Bounded per-token post-auth session-bucket reuse for stateless REST requests.
pub(crate) struct BucketCache {
    entries: AtomicUsize,
    buckets: SessionBuckets,
}
impl Default for BucketCache {
    fn default() -> Self {
        Self {
            entries: AtomicUsize::new(0),
            buckets: DashMap::with_hasher(shamir_collections::THasher::default()),
        }
    }
}
impl BucketCache {
    pub(crate) fn session_for(
        &self,
        grant: &Verified,
        username: String,
        now_ns: u64,
    ) -> Result<([u8; 32], Session), u32> {
        let id = TokenId(grant.state.record().id);
        self.buckets.retain(|token, session| {
            let live = token == &id
                || matches!(&session.origin, SessionOrigin::Token(origin) if origin.is_live(now_ns));
            if !live {
                self.entries.fetch_sub(1, Ordering::AcqRel);
            }
            live
        });
        let cached = if let Some(existing) = self.buckets.get(&id) {
            Arc::clone(existing.value())
        } else {
            self.entries
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    n.checked_add(1).filter(|next| *next <= SESSION_BUCKET_CAP)
                })
                .map_err(|_| 1)?;
            let candidate = Arc::new(Session::for_token(grant, username, now_ns, [1; 32]));
            match self.buckets.entry(id) {
                dashmap::mapref::entry::Entry::Occupied(entry) => {
                    self.entries.fetch_sub(1, Ordering::AcqRel);
                    Arc::clone(entry.get())
                }
                dashmap::mapref::entry::Entry::Vacant(entry) => {
                    let inserted = entry.insert(candidate);
                    Arc::clone(inserted.value())
                }
            }
        };
        if let Some(retry) = cached.check_post_auth_rate_limit(now_ns) {
            return Err(retry);
        }
        let sid = try_random_array::<32>().map_err(|_| 1)?;
        if sid.iter().all(|byte| *byte == 0) {
            return Err(1);
        }
        let session = Session::for_token(grant, cached.username.clone(), now_ns, sid);
        Ok((sid, session))
    }
}
