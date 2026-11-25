use super::{
    mint_activity_handle, new_activity_projection, projection_snapshot, projection_update,
    ActivityProjection, Session, SessionIdHasher,
};
use crate::common::types::limits;
use crate::server::state::admission::{Admission, AdmitReject, Sid, ACTIVE_WINDOW_NS};
use dashmap::DashMap;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Per-user membership slot list: up to the spec §7.4 cap of 16
/// (sid, session) pairs, stack-allocated via `SmallVec`.
type UserSlot = SmallVec<[(Sid, Arc<Session>); 16]>;
/// Contention-model map: one shard guard per user hash (see `SessionStore`).
type UserSlotMap = DashMap<[u8; 16], UserSlot, SessionIdHasher>;

#[derive(Debug)]
/// Active sessions indexed by session ID (routing) plus a per-user
/// membership index (cap accounting).
pub struct SessionStore {
    /// Bearer sid -> session. Short-lived sid-hash shard guards never span
    /// an `.await` or coexist with another routing guard.
    by_sid: DashMap<Sid, Arc<Session>, SessionIdHasher>,
    /// User -> [(sid, session)]. Cap decisions and slot mutations hold one
    /// user-hash shard guard and cost O(cap), not O(all sessions).
    /// Lock order: `by_user` before a single `by_sid` guard (see `admit`).
    by_user: UserSlotMap,
    /// Replacement rollback restores the predecessor instead of removing —
    /// also no counter change. See `admit`/`remove`/`gc_expired`.
    activity_len: AtomicUsize,
    post_auth_rejections: AtomicU64,
    /// Lock-free `by_sid` activity projection, sharded by sid hash. Rebuilds
    /// publish under the routing write guard; removals follow guard release
    /// and use pointer identity (see [`projection_update`]).
    activity_projection: ActivityProjection,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    /// Empty store.
    pub fn new() -> Self {
        Self {
            by_sid: DashMap::with_hasher(SessionIdHasher::default()),
            by_user: DashMap::with_hasher(SessionIdHasher::default()),
            activity_len: AtomicUsize::new(0),
            post_auth_rejections: AtomicU64::new(0),
            activity_projection: new_activity_projection(),
        }
    }

    /// Record one native dispatch refusal (not a shared bucket decision).
    pub(crate) fn record_post_auth_rejection(&self) {
        self.post_auth_rejections.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot of native post-auth rate refusals since store creation.
    pub fn post_auth_rejected_total(&self) -> u64 {
        self.post_auth_rejections.load(Ordering::Relaxed)
    }

    /// Test-fixture insertion ONLY: [`SessionStore::admit`] with the cap
    /// disabled (`cap = 0`). Production code paths (login AND resume) must
    /// use [`SessionStore::admit`] so the per-user cap is always enforced.
    pub fn insert(&self, session_id: Sid, session: Session) -> Arc<Session> {
        match self.admit(session_id, session, 0, 0) {
            Admission::Admitted { session, .. } => session,
            // Unreachable: cap 0 disables the cap and fixture sids are unique.
            Admission::Rejected(_) => unreachable!("admit with cap 0 never rejects"),
        }
    }

    /// The ONLY way a session enters the store (spec §7.4 overflow policy;
    /// one function for login AND resume — RV-CONNECT-8AD8B1C8 /
    /// RV-CONNECT-0A42F42C).
    ///
    /// Under ONE per-user shard guard: (1) idempotent pruning, (2) decide
    /// with no side effects, (3) claim the sid in `by_sid`, (4) evict idle
    /// LRU victims, (5) update the slot. All work is O(cap), synchronous,
    /// never across `.await`. Lock order: the `by_user` guard may be held
    /// while taking a single `by_sid` guard — never the reverse, never two
    /// `by_sid` guards at once.
    ///
    /// Invariants: I2 exact index when nothing is in flight; I3 an admit
    /// never pushes the user above `max(cap, previous length)`; I4 a
    /// `Rejected` result changes nothing apart from idempotent pruning; I5
    /// active iff `now - last < ACTIVE_WINDOW_NS` (saturating), only idle
    /// sessions evicted in (last_activity, sid) order.
    pub fn admit(
        &self,
        session_id: Sid,
        mut session: Session,
        now_ns: u64,
        cap: usize,
    ) -> Admission {
        session.session_id = session_id;
        let user_id = session.user_id;
        let mut arc = Arc::new(session);

        // by_user guard: all cap accounting for this user happens under it.
        let mut slot = self.by_user.entry(user_id).or_default();

        // 1. Idempotent pruning: drop slot entries whose by_sid binding was
        //    removed or replaced (ptr_eq). One by_sid guard at a time.
        slot.retain(|(k, v)| {
            self.by_sid
                .get(k)
                .is_some_and(|b| Arc::ptr_eq(b.value(), v))
        });

        // 2. Decide with NO side effects. A replacement (same sid already in
        //    this user's slot) consumes no extra capacity. cap 0 disables
        //    the cap entirely.
        let replacing = slot.iter().any(|(k, _)| *k == session_id);
        let need = if cap == 0 {
            0
        } else if replacing {
            slot.len().saturating_sub(cap)
        } else {
            (slot.len() + 1).saturating_sub(cap)
        };
        // Victims: the first `need` entries by (last_activity, sid),
        // excluding the sid being replaced.
        let mut order: Vec<(u64, Sid, Arc<Session>)> = slot
            .iter()
            .filter(|(k, _)| *k != session_id)
            .map(|(k, v)| {
                (
                    v.last_activity_ns.load(Ordering::Relaxed),
                    *k,
                    Arc::clone(v),
                )
            })
            .collect();
        order.sort_by_key(|e| (e.0, e.1));
        let victims: SmallVec<[(Sid, Arc<Session>); 1]> = order
            .into_iter()
            .take(need)
            .map(|(_, k, v)| (k, v))
            .collect();
        // I5 classification (no side effects): active iff now - last <
        // ACTIVE_WINDOW_NS (saturating). Any active victim => reject the whole
        // admission before anything is mutated.
        if victims.iter().any(|(_, v)| {
            now_ns.saturating_sub(v.last_activity_ns.load(Ordering::Relaxed)) < ACTIVE_WINDOW_NS
        }) {
            return Admission::Rejected(AdmitReject::AllActive);
        }

        // 3. Claim the sid atomically in by_sid (a foreign user's sid is never
        //    stolen). `prev` lets a later rejection restore the old binding.
        //    Handle stamping (D3) happens HERE, under the shard write guard,
        //    BEFORE publication, so no reader can observe an un-stamped
        //    generation: vacant => mint a fresh monotonic handle; occupied
        //    same-user => inherit the predecessor's handle (same bearer =
        //    same session; see `Session::activity_handle` for the resume
        //    justification). Mirror accounting: only a NEW sid (+1) changes
        //    the count; an in-place swap does not. The projection slab is
        //    rebuilt under this same guard (push / old->new swap).
        let prev = match self.by_sid.entry(session_id) {
            dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                if occ.get().user_id != user_id {
                    return Admission::Rejected(AdmitReject::SidConflict);
                }
                let inherited = occ.get().session_handle.load(Ordering::Relaxed);
                arc.stamp_handle(inherited);
                // Inherit the predecessor's shared in-flight counter: `arc`
                // is exclusive (count 1); swap BEFORE publication.
                if let Some(s) = Arc::get_mut(&mut arc) {
                    s.in_flight_requests = Arc::clone(&occ.get().in_flight_requests);
                }
                let old = occ.insert(Arc::clone(&arc));
                projection_update(
                    &self.activity_projection,
                    &session_id,
                    Some(&old),
                    Some(&arc),
                );
                Some(old)
            }
            dashmap::mapref::entry::Entry::Vacant(vac) => {
                let minted = mint_activity_handle();
                arc.stamp_handle(minted);
                self.activity_len.fetch_add(1, Ordering::Relaxed);
                projection_update(&self.activity_projection, &session_id, None, Some(&arc));
                vac.insert(Arc::clone(&arc));
                None
            }
        };

        // 4. Evict the victims. The idle check is repeated INSIDE `remove_if`
        //    (shard write lock) and `lookup` touches under the shard read
        //    lock, so a concurrent touch is either seen here (victim stays,
        //    admission rolled back) or finds the session already gone.
        //    `ptr_eq` guarantees a replacement generation is never removed.
        //    With a single victim (the only case outside a cap shrink) a
        //    rollback mutates nothing; with several, victims removed before a
        //    racing touch was observed were idle (>= 60 s) and stay removed.
        let mut evicted: SmallVec<[Sid; 1]> = SmallVec::new();
        for (vk, va) in &victims {
            let removed = self.by_sid.remove_if(vk, |_, v| {
                Arc::ptr_eq(v, va)
                    && now_ns.saturating_sub(v.last_activity_ns.load(Ordering::Relaxed))
                        >= ACTIVE_WINDOW_NS
            });
            if removed.is_some() {
                // Successful removal of a live sid: mirror -1, projection out.
                self.activity_len.fetch_sub(1, Ordering::Relaxed);
                projection_update(&self.activity_projection, vk, Some(va), None);
                slot.retain(|(k, _)| k != vk);
                evicted.push(*vk);
            } else if self
                .by_sid
                .get(vk)
                .is_some_and(|b| Arc::ptr_eq(b.value(), va))
            {
                // Still bound => it turned active: undo the claim and reject.
                match prev {
                    Some(old) => {
                        // Guarded restore: only if the primary still holds
                        // exactly OUR generation (`arc`, ptr_eq) do we swap
                        // in `old` under this entry guard and rebuild the
                        // slab. A racing remove (vacant now) or a foreign
                        // re-admission (occupied, different generation)
                        // leaves BOTH registries untouched. The update drops
                        // exactly `arc` and upserts `old` (presence-checked).
                        match self.by_sid.entry(session_id) {
                            dashmap::mapref::entry::Entry::Occupied(mut occ)
                                if Arc::ptr_eq(occ.get(), &arc) =>
                            {
                                occ.insert(Arc::clone(&old));
                                projection_update(
                                    &self.activity_projection,
                                    &session_id,
                                    Some(&arc),
                                    Some(&old),
                                );
                            }
                            _ => {}
                        }
                    }
                    None => {
                        // Rollback of a fresh claim: undo its mirror +1 and
                        // projection entry iff the removal actually happened.
                        // Between the claim and this rollback no third party
                        // could have re-inserted this CSPRNG sid for another
                        // user AND had it observed by our ptr_eq check, so
                        // `Some` here is exactly our own generation; a stale
                        // concurrent remove already dropped the projection
                        // entry via its own ptr_eq step.
                        if self
                            .by_sid
                            .remove_if(&session_id, |_, v| Arc::ptr_eq(v, &arc))
                            .is_some()
                        {
                            self.activity_len.fetch_sub(1, Ordering::Relaxed);
                            projection_update(
                                &self.activity_projection,
                                &session_id,
                                Some(&arc),
                                None,
                            );
                        }
                    }
                }
                return Admission::Rejected(AdmitReject::AllActive);
            } else {
                // Removed concurrently: its capacity is already free.
                slot.retain(|(k, _)| k != vk);
            }
        }

        // 5. Update the per-user slot (victims already dropped): replace or push.
        if replacing {
            if let Some(e) = slot.iter_mut().find(|(k, _)| *k == session_id) {
                e.1 = Arc::clone(&arc);
            }
        } else {
            slot.push((session_id, Arc::clone(&arc)));
        }

        Admission::Admitted {
            session: arc,
            evicted,
            replaced: replacing,
        }
    }

    /// Look up a session by id, touching `last_activity_ns` if found.
    ///
    /// Calls `UnixNanos::now()` internally LAZILY (only on hit). On miss
    /// returns immediately without consulting the clock. Hot-path callers
    /// should still prefer [`SessionStore::lookup_at`] to share one
    /// timestamp across multiple session touches.
    ///
    /// Legacy wrapper; prefer [`SessionStore::lookup_at`].
    ///
    /// # Panics
    /// Panics when the system clock is invalid (on hit only).
    pub fn lookup(&self, sid: &[u8; limits::SESSION_ID_BYTES]) -> Option<Arc<Session>> {
        // Touch under the shard guard: admission's idle recheck (remove_if)
        // takes the shard write lock, so it observes this touch.
        self.by_sid.get(sid).map(|r| {
            r.touch();
            r.clone()
        })
    }

    /// **Optim #5:** lookup with a caller-supplied `now_ns`, avoiding a
    /// `UnixNanos::now()` call inside the hot path.
    pub fn lookup_at(
        &self,
        sid: &[u8; limits::SESSION_ID_BYTES],
        now_ns: u64,
    ) -> Option<Arc<Session>> {
        self.by_sid.get(sid).map(|r| {
            r.touch_at(now_ns);
            r.clone()
        })
    }

    /// Remove a session by id. Returns the previous session if any.
    ///
    /// Takes the `by_sid` guard FIRST and drops it, then updates the
    /// per-user slot with a ptr_eq check — a stale removal racing with an
    /// admit that replaced the sid cannot drop the replacement generation.
    pub fn remove(&self, sid: &Sid) -> Option<Arc<Session>> {
        let (_, v) = self.by_sid.remove(sid)?;
        // Successful removal: mirror -1, projection out (ptr_eq against the
        // generation actually removed — a stale removal cannot delete a
        // replacement's projection entry).
        self.activity_len.fetch_sub(1, Ordering::Relaxed);
        projection_update(&self.activity_projection, sid, Some(&v), None);
        let user = v.user_id;
        self.remove_from_user_slot(user, sid, &v);
        Some(v)
    }

    /// Drop the per-user slot entry for ONE removed generation (ptr_eq),
    /// detaching the user cell if it empties. Shared by [`SessionStore::remove`]
    /// and [`SessionStore::gc_expired`] so a stale cleanup can never drop a
    /// freshly-admitted replacement generation bound to the same sid.
    fn remove_from_user_slot(&self, user: [u8; 16], sid: &Sid, gen: &Arc<Session>) {
        if let Some(mut slot) = self.by_user.get_mut(&user) {
            slot.retain(|(k, s)| !(k == sid && Arc::ptr_eq(s, gen)));
            if slot.is_empty() {
                drop(slot);
                // Detach the empty user cell only if still empty.
                self.by_user.remove_if(&user, |_, s| s.is_empty());
            }
        }
    }

    /// Number of currently-live sessions.
    ///
    /// O(1): reads the mirror atomics maintained at every `by_sid`
    /// mutation (see [`Self::activity_len`] field doc for the accounting —
    /// increment under the vacant shard guard before visibility, decrement
    /// on every successful removal, no change on replacement or restore),
    /// instead of `DashMap::len`'s per-shard guard sweep. Mirrors the gauge
    /// source required by the R4b activity view.
    pub fn len(&self) -> usize {
        self.activity_len.load(Ordering::Relaxed)
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.by_sid.is_empty()
    }

    /// Background GC: remove sessions where `is_expired(now, max_age, idle_ttl)`.
    /// Returns the number of evicted sessions (only generations ACTUALLY
    /// removed are counted).
    ///
    /// Generation-safe (stale-cleanup): each victim is captured as a
    /// `(sid, Arc<Session>)` PAIR and removal re-checks BOTH `is_expired`
    /// AND `Arc::ptr_eq` via `remove_if` — a GC pass that captured an
    /// expired generation while a replacement was being admitted can never
    /// later delete the freshly-admitted REPLACEMENT with the same sid. The
    /// per-user slot is updated through the same ptr_eq helper as
    /// [`SessionStore::remove`], keeping the index exact (I2).
    pub fn gc_expired(&self, now_ns: u64, max_age_ns: u64, idle_ttl_ns: u64) -> usize {
        let victims: Vec<(Sid, Arc<Session>)> = self
            .by_sid
            .iter()
            .filter(|e| e.value().is_expired(now_ns, max_age_ns, idle_ttl_ns))
            .map(|e| (*e.key(), Arc::clone(e.value())))
            .collect();
        let mut removed = 0;
        for (sid, gen) in victims {
            let gone = self
                .by_sid
                .remove_if(&sid, |_, v| {
                    Arc::ptr_eq(v, &gen) && v.is_expired(now_ns, max_age_ns, idle_ttl_ns)
                })
                .is_some();
            if gone {
                removed += 1;
                self.activity_len.fetch_sub(1, Ordering::Relaxed);
                projection_update(&self.activity_projection, &sid, Some(&gen), None);
                let user = gen.user_id;
                self.remove_from_user_slot(user, &sid, &gen);
            }
        }
        removed
    }

    /// Snapshot all sessions belonging to a given `user_id` (for admin
    /// `kickSession` / `updateUser` snapshot+kill semantics, spec §12.4 / §12.6).
    /// Reads the per-user index — O(cap), never an O(all-sessions) scan.
    pub fn snapshot_by_user(&self, user_id: &[u8; 16]) -> Vec<Sid> {
        self.by_user
            .get(user_id)
            .map(|slot| slot.iter().map(|(k, _)| *k).collect())
            .unwrap_or_default()
    }

    /// Lock-free snapshot of live generations and their minted handles:
    /// atomic projection loads and `Arc` clones, no `by_sid` guard.
    /// May be torn across shards; after quiescence its length equals `len()`
    /// (see [`projection_update`] for pointer-identity guarded removal).
    pub fn activity_snapshot(&self) -> Vec<Arc<Session>> {
        projection_snapshot(&self.activity_projection)
    }

    /// Check the stored generation's handle without touching its TTL.
    #[doc(hidden)]
    pub fn matches_activity_handle(&self, sid: &Sid, handle: u64) -> bool {
        let stored = self.by_sid.get(sid);
        handle != 0 && stored.is_some_and(|s| s.activity_handle() == handle)
    }

    /// Number of sessions currently held for a user. Reads the per-user
    /// index — O(cap).
    pub fn count_for_user(&self, user_id: &[u8; 16]) -> usize {
        self.by_user.get(user_id).map_or(0, |slot| slot.len())
    }

    /// Iterate over all (sid, session) pairs — for monitoring/`listSessions`.
    /// Clones nothing; the callback runs without any store guard held by the
    /// iterator beyond the per-shard read guard of `by_sid` itself.
    pub fn for_each(&self, mut f: impl FnMut(&Sid, &Session)) {
        for entry in self.by_sid.iter() {
            f(entry.key(), entry.value());
        }
    }
}
