use super::entry_frame::{decode, encode, HEADER_LEN, MAX_PAYLOAD};
use super::entry_index::{EntryIndex, EntryMeta};
use super::namespace::namespace;
use super::private_dir::prepare;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;
use wasmtime::{CacheStore, Engine};

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_536;

#[derive(Debug)]
pub(crate) struct DiskCacheStore {
    root: PathBuf,
    bound: OnceLock<BoundStore>,
    sequence: AtomicU64,
    tick: AtomicU64,
    trimming: AtomicBool,
    rejected: AtomicU64,
    hits: AtomicU64,
    /// Test-only: number of cache frames actually read from disk (`read_exact`).
    #[cfg(test)]
    frames_read: AtomicU64,
}

#[derive(Debug)]
struct BoundStore {
    namespace: [u8; 32],
    directory: PathBuf,
    index: EntryIndex,
}

impl DiskCacheStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            bound: OnceLock::new(),
            sequence: AtomicU64::new(0),
            tick: AtomicU64::new(0),
            trimming: AtomicBool::new(false),
            rejected: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            #[cfg(test)]
            frames_read: AtomicU64::new(0),
        }
    }

    pub(crate) fn bind(&self, engine: &Engine) -> std::io::Result<()> {
        // Path contract: `self.root` is the DATA directory; `prepare` derives
        // `<data>/cache/wasm` from it.
        let root = prepare(&self.root)?;
        let ns = namespace(engine);
        for item in fs::read_dir(&root)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('v') && name.contains('-') && name != namespace_name(&ns) {
                let ty = item.file_type()?;
                if ty.is_dir() {
                    fs::remove_dir_all(item.path())?;
                } else if ty.is_file() {
                    fs::remove_file(item.path())?;
                }
            }
        }
        let directory = root.join(namespace_name(&ns));
        fs::create_dir_all(directory.join("tmp"))?;
        let index = EntryIndex::new();
        let mut found: Vec<([u8; 32], u64, u128)> = Vec::new();
        for shard in fs::read_dir(&directory)? {
            let shard = shard?;
            if shard.file_name() == "tmp" {
                for temp in fs::read_dir(shard.path())? {
                    fs::remove_file(temp?.path())?;
                }
                continue;
            }
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for item in fs::read_dir(shard.path())? {
                let item = item?;
                let name = item.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".e") || name.len() != 66 {
                    continue;
                }
                let Some(key) = parse_hex(&name[..64]) else {
                    continue;
                };
                let meta = item.metadata()?;
                let len = meta.len();
                if len < HEADER_LEN as u64 || len > HEADER_LEN as u64 + MAX_PAYLOAD as u64 {
                    continue;
                }
                let modified = meta
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                found.push((key, len, modified));
            }
        }
        // Startup recency: rank by modification time, oldest first, with
        // small ticks (0, 1, 2, ...); the live tick counter starts after the
        // largest assigned rank so fresh inserts/accesses win eviction.
        found.sort_by_key(|entry| entry.2);
        let mut count = 0usize;
        let mut bytes = 0u64;
        for (rank, (key, len, _)) in found.iter().enumerate() {
            if index
                .entries
                .insert_sync(
                    *key,
                    EntryMeta {
                        len: *len,
                        tick: Arc::new(AtomicU64::new(rank as u64)),
                    },
                )
                .is_ok()
            {
                count = count.saturating_add(1);
                bytes = bytes.saturating_add(*len);
            }
        }
        index.count.store(count, Ordering::Relaxed);
        index.bytes.store(bytes, Ordering::Relaxed);
        self.tick
            .store(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
        let bound = BoundStore {
            namespace: ns,
            directory,
            index,
        };
        // Bring an over-cap cache under the caps immediately at startup.
        self.trim_caps(&bound, MAX_BYTES, MAX_ENTRIES);
        self.bound
            .set(bound)
            .map_err(|_| std::io::Error::other("cache store already bound"))
    }

    #[cfg(all(test, unix))]
    pub(crate) fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn frames_read(&self) -> u64 {
        self.frames_read.load(Ordering::Relaxed)
    }

    /// Test-only `(bytes, count)` snapshot of the shared index accounting.
    #[cfg(all(test, unix))]
    pub(crate) fn index_totals(&self) -> (u64, usize) {
        match self.bound.get() {
            Some(bound) => (
                bound.index.bytes.load(Ordering::Relaxed),
                bound.index.count.load(Ordering::Relaxed),
            ),
            None => (0, 0),
        }
    }

    fn entry_path(bound: &BoundStore, key: &[u8; 32]) -> PathBuf {
        let hex = hex_key(key);
        bound.directory.join(&hex[..2]).join(format!("{hex}.e"))
    }

    fn invalidate(&self, bound: &BoundStore, key: &[u8; 32]) {
        let _ = fs::remove_file(Self::entry_path(bound, key));
        if let Some(entry) = bound.index.entries.remove_sync(key) {
            bound.index.bytes.fetch_sub(entry.1.len, Ordering::Relaxed);
            bound.index.count.fetch_sub(1, Ordering::Relaxed);
        }
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    fn trim(&self, bound: &BoundStore) {
        self.trim_caps(bound, MAX_BYTES, MAX_ENTRIES);
    }

    #[cfg(all(test, unix))]
    pub(crate) fn trim_for_test(&self, max_bytes: u64, max_entries: usize) {
        if let Some(bound) = self.bound.get() {
            self.trim_caps(bound, max_bytes, max_entries);
        }
    }

    fn trim_caps(&self, bound: &BoundStore, max_bytes: u64, max_entries: usize) {
        if self
            .trimming
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let bytes = bound.index.bytes.load(Ordering::Relaxed);
        let count = bound.index.count.load(Ordering::Relaxed);
        if bytes <= max_bytes && count <= max_entries {
            self.trimming.store(false, Ordering::Release);
            return;
        }
        let target_bytes = max_bytes / 4 * 3;
        let target_count = max_entries / 4 * 3;
        let mut victims = Vec::new();
        bound.index.entries.iter_sync(|key, entry| {
            victims.push((*key, entry.len, entry.tick.load(Ordering::Relaxed)));
            true
        });
        victims.sort_by_key(|entry| entry.2);
        let mut remaining_bytes = bytes;
        let mut remaining_count = count;
        for (key, _, _) in victims {
            if remaining_bytes <= target_bytes && remaining_count <= target_count {
                break;
            }
            // Account every removed entry with `fetch_sub` of exactly what
            // was removed; never store snapshot totals over the atomics
            // (concurrent inserts would be lost).
            if let Some((_, removed)) = bound.index.entries.remove_sync(&key) {
                let _ = fs::remove_file(Self::entry_path(bound, &key));
                bound.index.bytes.fetch_sub(removed.len, Ordering::Relaxed);
                bound.index.count.fetch_sub(1, Ordering::Relaxed);
                remaining_bytes = remaining_bytes.saturating_sub(removed.len);
                remaining_count = remaining_count.saturating_sub(1);
            }
        }
        self.trimming.store(false, Ordering::Release);
    }
}

impl CacheStore for DiskCacheStore {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let key: [u8; 32] = key.try_into().ok()?;
        let bound = self.bound.get()?;
        // Read what we need (len and a clone of the tick handle), then drop
        // the map entry guard before any I/O: error branches below call
        // `invalidate`, which would self-deadlock on a held bucket lock.
        let (len, tick) = {
            let entry = bound.index.entries.get_sync(&key)?;
            (entry.get().len, entry.get().tick.clone())
        };
        let path = Self::entry_path(bound, &key);
        let read = (|| -> Option<Vec<u8>> {
            let metadata = fs::metadata(&path).ok()?;
            if metadata.len() != len || metadata.len() > HEADER_LEN as u64 + MAX_PAYLOAD as u64 {
                return None;
            }
            let size = usize::try_from(metadata.len()).ok()?;
            let mut frame = vec![0; size];
            let mut file = fs::File::open(&path).ok()?;
            #[cfg(test)]
            self.frames_read.fetch_add(1, Ordering::Relaxed);
            file.read_exact(&mut frame).ok()?;
            Some(frame)
        })();
        match read {
            Some(frame) if decode(&frame, &bound.namespace, &key).is_some() => {
                // Bump recency through the cloned atomic after the guard is
                // dropped; the Arc keeps it valid even if the entry was
                // concurrently removed in between.
                tick.store(self.tick.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(Cow::Owned(frame[HEADER_LEN..].to_vec()))
            }
            _ => {
                self.invalidate(bound, &key);
                None
            }
        }
    }

    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let Ok(key) = <[u8; 32]>::try_from(key) else {
            return false;
        };
        let Some(bound) = self.bound.get() else {
            return false;
        };
        if value.len() > MAX_PAYLOAD {
            return false;
        }
        if bound.index.entries.get_sync(&key).is_some() {
            return true;
        }
        let Some(frame) = encode(&bound.namespace, &key, &value) else {
            return false;
        };
        let hex = hex_key(&key);
        let temp = bound.directory.join("tmp").join(format!(
            "{}-{}-{hex}",
            std::process::id(),
            self.sequence.fetch_add(1, Ordering::Relaxed)
        ));
        let final_path = Self::entry_path(bound, &key);
        let Some(parent) = final_path.parent() else {
            return false;
        };
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&frame)?;
            fs::rename(&temp, &final_path)?;
            Ok::<(), std::io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
            return false;
        }
        if bound
            .index
            .entries
            .insert_sync(
                key,
                EntryMeta {
                    len: frame.len() as u64,
                    tick: Arc::new(AtomicU64::new(self.tick.fetch_add(1, Ordering::Relaxed))),
                },
            )
            .is_ok()
        {
            bound
                .index
                .bytes
                .fetch_add(frame.len() as u64, Ordering::Relaxed);
            bound.index.count.fetch_add(1, Ordering::Relaxed);
        } else {
            return true;
        }
        self.trim(bound);
        true
    }
}

fn namespace_name(namespace: &[u8; 32]) -> String {
    let hex: String = namespace[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("v1-{hex}")
}
fn hex_key(key: &[u8; 32]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn parse_hex(text: &str) -> Option<[u8; 32]> {
    let mut key = [0; 32];
    for (i, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
        key[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(key)
}
