use std::fmt;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::http_login::admission::{BudgetPin, LoginClock, PrefixKey, Source};

const CAPACITY: usize = 1_024;
const PAYLOAD_LIMIT: usize = 1_536;
const TTL: Duration = Duration::from_secs(60);
const LIVE: u8 = 0;
const CLAIMED: u8 = 1;
const RELEASED: u8 = 2;

type Selector = Arc<dyn Fn(u64) -> Option<u64> + Send + Sync>;

/// Opaque random handshake identifier; its bytes are never formatted.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PendingId(pub [u8; 32]);

impl fmt::Debug for PendingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PendingId(<redacted>)")
    }
}

/// Retained handshake material; transcript is stored exactly once.
pub struct PendingEntry {
    /// Validated handshake transcript, zeroized when dropped.
    pub transcript: Zeroizing<Vec<u8>>,
    /// Associated user identifier, when authenticated.
    pub user_id: Option<[u8; 16]>,
    /// Stored authentication key, zeroized when dropped.
    pub stored_key: Zeroizing<[u8; 32]>,
    /// Server handshake key, zeroized when dropped.
    pub server_key: Zeroizing<[u8; 32]>,
    /// Earliest ticket invalidation time in nanoseconds.
    pub tickets_invalid_before_ns: u64,
    /// Handshake begin time in nanoseconds.
    pub begin_time_ns: u64,
    /// Whether this entry is eligible for completion.
    pub eligible: bool,
    /// Listener that accepted the handshake.
    pub listener_id: [u8; 16],
    /// Security class captured for the handshake.
    pub security_class: u8,
    /// Source address captured for the handshake.
    pub source: Source,
}

impl PendingEntry {
    /// Validates fixed transcript offsets and the retained-payload bound.
    pub fn validate(&self) -> Result<(), PendingError> {
        let len = self.transcript.len();
        if !(173..=933).contains(&len) {
            return Err(PendingError::Invalid);
        }
        if self.transcript.get(..b"SHAMIR-HTTP-LOGIN-v1".len())
            != Some(b"SHAMIR-HTTP-LOGIN-v1".as_slice())
            || self.transcript.get(20) != Some(&1)
            || self.transcript.get(21) != Some(&3)
            || self.transcript.get(22) != Some(&self.security_class)
            || self.security_class > 2
            || self.transcript.get(23..39) != Some(self.listener_id.as_slice())
        {
            return Err(PendingError::Invalid);
        }
        let user_len = usize::from(u16::from_be_bytes([
            *self.transcript.get(71).ok_or(PendingError::Invalid)?,
            *self.transcript.get(72).ok_or(PendingError::Invalid)?,
        ]));
        let scope_end = 166usize
            .checked_add(user_len)
            .ok_or(PendingError::Invalid)?;
        if !(1..=255).contains(&user_len)
            || self.transcript.get(39..71).is_none()
            || scope_end > len
            || !(6..=512).contains(&len.saturating_sub(scope_end))
            || len.checked_add(80).ok_or(PendingError::Invalid)? > PAYLOAD_LIMIT
        {
            return Err(PendingError::Invalid);
        }
        Ok(())
    }
}

/// Stable pending-store failure classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingError {
    /// Entry payload or identifier is malformed.
    Invalid,
    /// No eligible capacity is available.
    Full,
    /// Identifier is not present.
    NotFound,
    /// Source, listener, or security class does not match.
    WrongScope,
    /// Entry expired before it could be claimed.
    Expired,
    /// Entry has already been claimed.
    AlreadyClaimed,
    /// Bounded random selection could not complete.
    Entropy,
    /// Identifier collided with a live entry; the caller may generate another.
    Collision,
}

struct Cell {
    id: PendingId,
    expires: Instant,
    source: Source,
    listener_id: [u8; 16],
    security_class: u8,
    entry: Option<PendingEntry>,
    marker: Arc<AtomicU8>,
    pins: Option<(BudgetPin, BudgetPin)>,
}
struct State {
    cells: Vec<Option<Cell>>,
    markers: Box<[Arc<AtomicU8>]>,
    expirations: std::collections::BinaryHeap<std::cmp::Reverse<(Instant, usize)>>,
    released: tokio::sync::mpsc::Receiver<usize>,
    #[cfg(test)]
    reclaim_visits: std::sync::atomic::AtomicUsize,
}

/// Fixed-capacity, bounded pending table with injected monotonic time.
pub struct PendingStore {
    clock: Arc<dyn LoginClock>,
    selector: Selector,
    released: tokio::sync::mpsc::Sender<usize>,
    inner: Mutex<State>,
}

impl PendingStore {
    /// Creates an empty bounded store with operating-system randomness.
    pub fn new(clock: Arc<dyn LoginClock>) -> Self {
        Self::with_selector(clock, random_sample)
    }

    /// Creates a store with a bounded random-choice test seam.
    pub fn with_selector<F>(clock: Arc<dyn LoginClock>, selector: F) -> Self
    where
        F: Fn(u64) -> Option<u64> + Send + Sync + 'static,
    {
        let markers = (0..CAPACITY)
            .map(|_| Arc::new(AtomicU8::new(RELEASED)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let (released_tx, released_rx) = tokio::sync::mpsc::channel(CAPACITY);
        // Claimed slots cannot be evicted or reused before their one Drop notification is drained.
        // Thus no more than CAPACITY notifications can be outstanding and try_send cannot overflow.
        Self {
            clock,
            selector: Arc::new(selector),
            released: released_tx,
            inner: Mutex::new(State {
                cells: (0..CAPACITY).map(|_| None).collect(),
                markers,
                expirations: std::collections::BinaryHeap::with_capacity(CAPACITY),
                released: released_rx,
                #[cfg(test)]
                reclaim_visits: std::sync::atomic::AtomicUsize::new(0),
            }),
        }
    }

    /// Inserts an entry; IDs are supplied by the caller's CSPRNG seam.
    pub async fn insert(&self, id: PendingId, entry: PendingEntry) -> Result<(), PendingError> {
        self.insert_inner(id, entry, None).await
    }

    /// Inserts an entry while retaining its admission-budget pins.
    pub async fn insert_pinned(
        &self,
        id: PendingId,
        entry: PendingEntry,
        pins: (BudgetPin, BudgetPin),
    ) -> Result<(), PendingError> {
        self.insert_inner(id, entry, Some(pins)).await
    }

    async fn insert_inner(
        &self,
        id: PendingId,
        entry: PendingEntry,
        pins: Option<(BudgetPin, BudgetPin)>,
    ) -> Result<(), PendingError> {
        entry.validate()?;
        if entry.transcript.get(39..71) != Some(id.0.as_slice()) {
            return Err(PendingError::Invalid);
        }
        // This mutex protects only a fixed-capacity table; its critical section does no awaits.
        let mut state = self.inner.lock().await;
        let now = self.clock.now();
        reclaim_released(&mut state);
        purge_expired(&mut state, now);
        let collision = state.cells.iter().flatten().fold(false, |found, cell| {
            let equal = shamir_connect::common::crypto::constant_time_eq(&cell.id.0, &id.0);
            found | (equal && cell.marker.load(Ordering::Acquire) != RELEASED)
        });
        if collision {
            return Err(PendingError::Collision);
        }
        enforce_prefix_caps(&state, entry.source)?;
        let empty = state.cells.iter().position(Option::is_none);
        let index = match empty {
            Some(index) => index,
            None => {
                let index = choose_eviction(&state, &self.selector)?;
                super::super::metrics::record_eviction("fair_share");
                state
                    .expirations
                    .retain(|std::cmp::Reverse((_, queued_index))| *queued_index != index);
                index
            }
        };
        let source = entry.source;
        let listener_id = entry.listener_id;
        let security_class = entry.security_class;
        let expires = now + TTL;
        let marker = Arc::clone(&state.markers[index]);
        marker.store(LIVE, Ordering::Release);
        if empty.is_some() {
            super::super::metrics::update_pending(1);
        }
        state.cells[index] = Some(Cell {
            id,
            expires,
            source,
            listener_id,
            security_class,
            entry: Some(entry),
            marker,
            pins,
        });
        state.expirations.push(std::cmp::Reverse((expires, index)));
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn reset_reclaim_visits(&self) {
        let state = self.inner.lock().await;
        state.reclaim_visits.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) async fn reclaim_visits(&self) -> usize {
        let state = self.inner.lock().await;
        state.reclaim_visits.load(Ordering::Relaxed)
    }

    pub async fn claim(
        &self,
        id: PendingId,
        source: Source,
        listener_id: [u8; 16],
        class: u8,
    ) -> Result<PendingClaim, PendingError> {
        let mut state = self.inner.lock().await;
        let now = self.clock.now();
        reclaim_released(&mut state);
        purge_expired(&mut state, now);
        let mut match_index = None;
        for (index, cell) in state.cells.iter().enumerate() {
            if let Some(cell) = cell {
                let equal = shamir_connect::common::crypto::constant_time_eq(&cell.id.0, &id.0);
                if equal && cell.marker.load(Ordering::Acquire) != RELEASED {
                    match_index = Some(index);
                }
            }
        }
        let match_index = match_index.ok_or(PendingError::NotFound)?;
        let claim = {
            let cell = state
                .cells
                .get_mut(match_index)
                .and_then(Option::as_mut)
                .ok_or(PendingError::NotFound)?;
            match cell.marker.load(Ordering::Acquire) {
                CLAIMED => return Err(PendingError::AlreadyClaimed),
                RELEASED => return Err(PendingError::NotFound),
                _ => {}
            }
            if cell.source.tiers()[0] != source.tiers()[0]
                || cell.listener_id != listener_id
                || cell.security_class != class
            {
                return Err(PendingError::WrongScope);
            }
            cell.marker.store(CLAIMED, Ordering::Release);
            let payload = cell.entry.take().ok_or(PendingError::AlreadyClaimed)?;
            let pins = cell.pins.take();
            PendingClaim {
                payload: Some(payload),
                marker: Arc::clone(&cell.marker),
                expires: cell.expires,
                clock: Arc::clone(&self.clock),
                _pins: pins,
                released: self.released.clone(),
                slot: match_index,
            }
        };
        state
            .expirations
            .retain(|std::cmp::Reverse((_, queued_index))| *queued_index != match_index);
        Ok(claim)
    }

    /// Returns whether an identifier is stored, comparing every live identifier.
    pub async fn contains_id(&self, id: PendingId) -> bool {
        let mut state = self.inner.lock().await;
        purge_expired(&mut state, self.clock.now());
        let mut found = false;
        for cell in state.cells.iter().flatten() {
            let equal = shamir_connect::common::crypto::constant_time_eq(&cell.id.0, &id.0);
            found |= equal && cell.marker.load(Ordering::Acquire) != RELEASED;
        }
        found
    }

    /// Returns active entry count, excluding released cells and purging expiry.
    pub async fn count(&self) -> usize {
        let mut state = self.inner.lock().await;
        purge_expired(&mut state, self.clock.now());
        state
            .cells
            .iter()
            .flatten()
            .filter(|cell| cell.marker.load(Ordering::Acquire) != RELEASED)
            .count()
    }

    /// Copies active source prefixes for the admission layer to pin.
    pub async fn active_prefixes(&self) -> Vec<PrefixKey> {
        let mut prefixes = Vec::with_capacity(CAPACITY);
        let mut state = self.inner.lock().await;
        purge_expired(&mut state, self.clock.now());
        for cell in state.cells.iter().flatten() {
            if cell.marker.load(Ordering::Acquire) != RELEASED {
                prefixes.push(cell.source.tiers()[0]);
            }
        }
        prefixes
    }
}

impl Drop for PendingStore {
    fn drop(&mut self) {
        let state = self.inner.get_mut();
        let live = state
            .cells
            .iter()
            .flatten()
            .filter(|cell| cell.marker.load(Ordering::Acquire) == LIVE)
            .count();
        if live > 0 {
            super::super::metrics::update_pending(-(live as isize));
        }
    }
}

/// Claimed payload; its capacity remains charged until this guard drops.
pub struct PendingClaim {
    payload: Option<PendingEntry>,
    marker: Arc<AtomicU8>,
    expires: Instant,
    clock: Arc<dyn LoginClock>,
    _pins: Option<(BudgetPin, BudgetPin)>,
    released: tokio::sync::mpsc::Sender<usize>,
    slot: usize,
}
impl PendingClaim {
    /// Moves the sensitive payload to the FINISH task.
    pub fn take(&mut self) -> Option<PendingEntry> {
        self.payload.take()
    }
    /// Whether this claim remains within its pending-entry lifetime.
    pub fn is_live(&self) -> bool {
        self.clock.now() < self.expires
    }
}
impl Drop for PendingClaim {
    fn drop(&mut self) {
        self.marker.store(RELEASED, Ordering::Release);
        let _ = self.released.try_send(self.slot);
        super::super::metrics::update_pending(-1);
    }
}

fn reclaim_released(state: &mut State) {
    while let Ok(index) = state.released.try_recv() {
        #[cfg(test)]
        state.reclaim_visits.fetch_add(1, Ordering::Relaxed);
        if state
            .cells
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|cell| cell.marker.load(Ordering::Acquire) == RELEASED)
        {
            state.cells[index] = None;
        }
    }
}

fn purge_expired(state: &mut State, now: Instant) {
    while state
        .expirations
        .peek()
        .is_some_and(|std::cmp::Reverse((expires, _))| *expires <= now)
    {
        let Some(std::cmp::Reverse((_expires, index))) = state.expirations.pop() else {
            break;
        };
        let Some(cell) = state.cells.get_mut(index).and_then(Option::take) else {
            continue;
        };
        if cell.expires > now {
            let expires = cell.expires;
            state.cells[index] = Some(cell);
            state.expirations.push(std::cmp::Reverse((expires, index)));
            continue;
        }
        let marker = cell.marker.load(Ordering::Acquire);
        if marker == CLAIMED || marker == RELEASED {
            state.cells[index] = Some(cell);
            continue;
        }
        if marker != RELEASED {
            super::super::metrics::update_pending(-1);
            super::super::metrics::record_eviction("expired");
        }
    }
}

fn enforce_prefix_caps(state: &State, source: Source) -> Result<(), PendingError> {
    let keys = source.tiers();
    let caps = if keys[0].family() == 4 {
        [16, 256, 256]
    } else {
        [16, 64, 256]
    };
    for (tier, cap) in keys.into_iter().zip(caps) {
        let count = state
            .cells
            .iter()
            .flatten()
            .filter(|cell| {
                cell.marker.load(Ordering::Acquire) != RELEASED
                    && cell.source.tiers().contains(&tier)
            })
            .count();
        if count >= cap {
            return Err(PendingError::Full);
        }
    }
    Ok(())
}

fn choose_eviction(state: &State, selector: &Selector) -> Result<usize, PendingError> {
    let mut principals: [Option<PrefixKey>; CAPACITY] = [None; CAPACITY];
    let mut principal_counts = [0usize; CAPACITY];
    let mut active = 0usize;
    for cell in state.cells.iter().flatten() {
        if cell.marker.load(Ordering::Acquire) == RELEASED {
            continue;
        }
        let key = cell.source.principal();
        let slot = principals[..active]
            .iter()
            .position(|item| *item == Some(key))
            .unwrap_or_else(|| {
                principals[active] = Some(key);
                active += 1;
                active - 1
            });
        principal_counts[slot] += 1;
    }
    if active == 0 {
        return Err(PendingError::Full);
    }
    let threshold = CAPACITY.div_ceil(active);
    let mut candidate_count = 0usize;
    for cell in state.cells.iter().flatten() {
        if cell.marker.load(Ordering::Acquire) != LIVE {
            continue;
        }
        let slot = principals[..active]
            .iter()
            .position(|item| *item == Some(cell.source.principal()))
            .ok_or(PendingError::Full)?;
        if principal_counts[slot] >= threshold {
            candidate_count += 1;
        }
    }
    if candidate_count == 0 {
        return Err(PendingError::Full);
    }
    let mut selected = None;
    for _ in 0..3 {
        let raw = selector(candidate_count as u64).ok_or(PendingError::Entropy)?;
        let zone = u64::MAX - u64::MAX % candidate_count as u64;
        if raw < zone {
            selected = Some((raw % candidate_count as u64) as usize);
            break;
        }
    }
    let mut rank = selected.ok_or(PendingError::Entropy)?;
    for (index, cell) in state
        .cells
        .iter()
        .enumerate()
        .filter_map(|(index, cell)| cell.as_ref().map(|cell| (index, cell)))
    {
        if cell.marker.load(Ordering::Acquire) != LIVE {
            continue;
        }
        let slot = principals[..active]
            .iter()
            .position(|item| *item == Some(cell.source.principal()))
            .ok_or(PendingError::Full)?;
        if principal_counts[slot] >= threshold {
            if rank == 0 {
                return Ok(index);
            }
            rank -= 1;
        }
    }
    Err(PendingError::Full)
}

fn random_sample(_bound: u64) -> Option<u64> {
    let bytes = shamir_connect::common::crypto::try_random_array::<8>().ok()?;
    Some(u64::from_be_bytes(bytes))
}
