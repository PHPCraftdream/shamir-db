use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

#[cfg(test)]
#[path = "tests/bucket_tests.rs"]
mod tests;

use dashmap::DashMap;
use shamir_connect::common::crypto::try_random_array;
use shamir_connect::server::origin::SessionOrigin;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::{TokenId, Verified};

const SESSION_BUCKET_CAP: usize = 4096;
const BUCKET_IDLE_NS: u64 = 1_000_000_000;
type SessionBuckets = DashMap<TokenId, Arc<Session>, shamir_collections::THasher>;

pub(crate) enum BucketError {
    RateLimited(u32),
    Unavailable,
}

/// Bounded per-token post-auth session-bucket reuse for stateless REST requests.
pub(crate) struct BucketCache {
    entries: AtomicUsize,
    buckets: SessionBuckets,
    cap: usize,
    next_sweep_ns: AtomicU64,
    #[cfg(test)]
    sweeps: AtomicUsize,
    #[cfg(test)]
    sid_generator: fn() -> Result<[u8; 32], ()>,
}
impl Default for BucketCache {
    fn default() -> Self {
        Self::with_capacity(SESSION_BUCKET_CAP)
    }
}
impl BucketCache {
    fn with_capacity(cap: usize) -> Self {
        Self {
            entries: AtomicUsize::new(0),
            buckets: DashMap::with_hasher(shamir_collections::THasher::default()),
            cap,
            next_sweep_ns: AtomicU64::new(0),
            #[cfg(test)]
            sweeps: AtomicUsize::new(0),
            #[cfg(test)]
            sid_generator: || try_random_array::<32>().map_err(|_| ()),
        }
    }
    pub(crate) fn session_for(
        &self,
        grant: &Verified,
        username: String,
        now_ns: u64,
    ) -> Result<([u8; 32], Session), BucketError> {
        let id = TokenId(grant.state.record().id);
        if let Some(existing) = self.buckets.get(&id) {
            let cached = Arc::clone(existing.value());
            cached.last_activity_ns.fetch_max(now_ns, Ordering::Relaxed);
            drop(existing);
            return self.session_from_cached(grant, &cached, now_ns);
        }
        if self.entries.load(Ordering::Acquire) >= self.cap {
            self.sweep_if_due(now_ns);
            if self.entries.load(Ordering::Acquire) >= self.cap {
                if let Some(existing) = self.buckets.get(&id) {
                    let cached = Arc::clone(existing.value());
                    cached.last_activity_ns.fetch_max(now_ns, Ordering::Relaxed);
                    drop(existing);
                    return self.session_from_cached(grant, &cached, now_ns);
                }
                return Err(BucketError::Unavailable);
            }
        }
        let candidate = Arc::new(Session::for_token(grant, username, now_ns, [1; 32]));
        match self.buckets.entry(id) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                let cached = Arc::clone(entry.get());
                cached.last_activity_ns.fetch_max(now_ns, Ordering::Relaxed);
                drop(entry);
                self.session_from_cached(grant, &cached, now_ns)
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                if self
                    .entries
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        count.checked_add(1).filter(|next| *next <= self.cap)
                    })
                    .is_err()
                {
                    drop(entry);
                    if let Some(existing) = self.buckets.get(&id) {
                        let cached = Arc::clone(existing.value());
                        cached.last_activity_ns.fetch_max(now_ns, Ordering::Relaxed);
                        drop(existing);
                        return self.session_from_cached(grant, &cached, now_ns);
                    }
                    return Err(BucketError::Unavailable);
                }
                let inserted = entry.insert(candidate);
                inserted
                    .value()
                    .last_activity_ns
                    .fetch_max(now_ns, Ordering::Relaxed);
                let cached = Arc::clone(inserted.value());
                drop(inserted);
                self.session_from_cached(grant, &cached, now_ns)
            }
        }
    }
    fn session_from_cached(
        &self,
        grant: &Verified,
        cached: &Session,
        now_ns: u64,
    ) -> Result<([u8; 32], Session), BucketError> {
        if let Some(retry) = cached.check_post_auth_rate_limit(now_ns) {
            return Err(BucketError::RateLimited(retry));
        }
        #[cfg(test)]
        let sid = (self.sid_generator)().map_err(|_| BucketError::Unavailable)?;
        #[cfg(not(test))]
        let sid = try_random_array::<32>().map_err(|_| BucketError::Unavailable)?;
        if sid.iter().all(|byte| *byte == 0) {
            return Err(BucketError::Unavailable);
        }
        Ok((
            sid,
            Session::for_token(grant, cached.username.clone(), now_ns, sid),
        ))
    }
    fn sweep_if_due(&self, now_ns: u64) {
        let next = self.next_sweep_ns.load(Ordering::Acquire);
        // A wall-clock jump back beyond one interval must not stall sweeps.
        if now_ns < next && next - now_ns <= BUCKET_IDLE_NS {
            return;
        }
        if self
            .next_sweep_ns
            .compare_exchange(
                next,
                now_ns.saturating_add(BUCKET_IDLE_NS),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.sweep(now_ns);
        }
    }
    pub(crate) fn sweep(&self, now_ns: u64) {
        #[cfg(test)]
        self.sweeps.fetch_add(1, Ordering::Relaxed);
        let mut removed = 0usize;
        self.buckets.retain(|_, session| {
            let idle = now_ns.saturating_sub(session.last_activity_ns.load(Ordering::Relaxed))
                >= BUCKET_IDLE_NS;
            let dead =
                !matches!(&session.origin, SessionOrigin::Token(origin) if origin.is_live(now_ns));
            if idle || dead {
                removed += 1;
                false
            } else {
                true
            }
        });
        self.entries.fetch_sub(removed, Ordering::AcqRel);
    }
    #[cfg(test)]
    fn with_cap(cap: usize) -> Self {
        Self::with_capacity(cap)
    }
    #[cfg(test)]
    fn set_sid_generator(&mut self, generator: fn() -> Result<[u8; 32], ()>) {
        self.sid_generator = generator;
    }
}
