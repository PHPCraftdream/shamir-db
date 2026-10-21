//! Node-local compiled-module cache, pure Rust.
//!
//! Wasmtime's own disk cache (its `cache` feature) compresses with zstd, a C
//! library; this cache stores `Module::serialize` output as it is. An entry is
//! a node-local derivative of the sanitized `.wasm` bytes: never replicated,
//! safe to delete at any time. Anything unreadable, corrupt or incompatible is
//! dropped and recompiled, so the cache can only cost time.
//!
//! Trust boundary (the same as Wasmtime's own cache): entries run as native
//! code, so the directory must be private to the service user. [`open`]
//! creates it with mode 0700 and refuses a group- or world-writable one
//! (unix); every entry also carries a blake3 digest that catches truncation
//! and bit-rot before the bytes reach `Module::deserialize`.
//!
//! [`open`]: CompileCache::open

use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use wasmtime::{Engine, Module};

/// File magic plus format version.
const MAGIC: &[u8; 8] = b"SHMWASM1";
const HEADER_LEN: usize = MAGIC.len() + blake3::OUT_LEN;
const ENTRY_EXT: &str = "cwasm";
const TMP_EXT: &str = "tmp";
/// Default soft cap on the total size of the entries; the oldest go first.
pub(crate) const DEFAULT_SOFT_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
/// A temp file older than this is a crashed writer's leftover.
const STALE_TMP_AGE: Duration = Duration::from_secs(3600);

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Whether a lookup was served from the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Hit,
    Miss,
}

/// A directory of compiled modules keyed by the sanitized wasm bytes and the
/// engine's compile-compatibility hash.
pub(crate) struct CompileCache {
    dir: PathBuf,
}

impl CompileCache {
    /// Opens `dir` (created if missing) and trims it to `soft_limit` bytes.
    pub(crate) fn open(dir: PathBuf, soft_limit: u64) -> io::Result<Self> {
        create_private_dir(&dir)?;
        let cache = Self { dir };
        cache.trim(soft_limit);
        Ok(cache)
    }

    /// Per-user default location: `SHAMIR_WASM_CACHE_DIR`, else the OS cache
    /// directory of the current user. `None` when neither can be resolved.
    pub(crate) fn default_dir() -> Option<PathBuf> {
        let var = |name: &str| {
            env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        if let Some(dir) = var("SHAMIR_WASM_CACHE_DIR") {
            return Some(dir);
        }
        let base = if cfg!(windows) {
            var("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            var("HOME").map(|home| home.join("Library").join("Caches"))
        } else {
            var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
        };
        base.map(|base| base.join("shamir-db").join("wasm-cache"))
    }

    /// Compiles `wasm`, or loads the compiled module when an intact entry for
    /// this engine configuration exists.
    pub(crate) fn module(
        &self,
        engine: &Engine,
        wasm: &[u8],
    ) -> Result<(Module, Outcome), wasmtime::Error> {
        let path = self.entry_path(engine, wasm);
        if let Some(module) = Self::load(engine, &path) {
            return Ok((module, Outcome::Hit));
        }
        let module = Module::from_binary(engine, wasm)?;
        self.store(&path, &module);
        Ok((module, Outcome::Miss))
    }

    fn entry_path(&self, engine: &Engine, wasm: &[u8]) -> PathBuf {
        // The compatibility hash covers the Wasmtime version, the target and
        // the compiler settings; Wasmtime re-validates it on deserialize.
        let mut hasher = Blake3Hasher(blake3::Hasher::new());
        engine.precompile_compatibility_hash().hash(&mut hasher);
        let mut hasher = hasher.0;
        hasher.update(&(wasm.len() as u64).to_le_bytes());
        hasher.update(wasm);
        self.dir
            .join(format!("{}.{ENTRY_EXT}", hasher.finalize().to_hex()))
    }

    fn load(engine: &Engine, path: &Path) -> Option<Module> {
        let bytes = fs::read(path).ok()?;
        let Some(payload) = intact_payload(&bytes) else {
            log::debug!("Wasm compile cache: dropping corrupt {}", path.display());
            discard(path);
            return None;
        };
        // SAFETY: `payload` is `Module::serialize` output of an engine with
        // this entry's compatibility hash, written by this crate into a
        // directory `open` verified to be private to the service user, and its
        // blake3 digest matches the one recorded at write time. Wasmtime also
        // rejects other versions and configurations. Tampering by a principal
        // who can write to the private directory is outside the trust
        // boundary, as for Wasmtime's own disk cache.
        match unsafe { Module::deserialize(engine, payload) } {
            Ok(module) => Some(module),
            Err(e) => {
                log::debug!("Wasm compile cache: dropping {}: {e}", path.display());
                discard(path);
                None
            }
        }
    }

    fn store(&self, path: &Path, module: &Module) {
        if let Err(e) = self.try_store(path, module) {
            log::debug!("Wasm compile cache: cannot store {}: {e}", path.display());
        }
    }

    fn try_store(&self, path: &Path, module: &Module) -> io::Result<()> {
        let payload = module
            .serialize()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut entry = Vec::with_capacity(HEADER_LEN + payload.len());
        entry.extend_from_slice(MAGIC);
        entry.extend_from_slice(blake3::hash(&payload).as_bytes());
        entry.extend_from_slice(&payload);
        // Write beside the entry and rename: readers never see a partial file.
        let tmp = self.dir.join(format!(
            "{}-{}.{TMP_EXT}",
            process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, &entry)?;
        fs::rename(&tmp, path).inspect_err(|_| discard(&tmp))
    }

    /// Removes stale temp files and, past `soft_limit` total bytes, the oldest
    /// entries. Best effort: every error is ignored.
    fn trim(&self, soft_limit: u64) {
        let Ok(listing) = fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut total = 0u64;
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for item in listing.flatten() {
            let path = item.path();
            let Ok(meta) = item.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or(now);
            match path.extension().and_then(|ext| ext.to_str()) {
                Some(TMP_EXT) => {
                    if now
                        .duration_since(modified)
                        .is_ok_and(|age| age > STALE_TMP_AGE)
                    {
                        discard(&path);
                    }
                }
                Some(ENTRY_EXT) => {
                    total += meta.len();
                    entries.push((modified, meta.len(), path));
                }
                _ => {}
            }
        }
        if total <= soft_limit {
            return;
        }
        entries.sort_by_key(|(modified, _, _)| *modified);
        for (_, len, path) in entries {
            if total <= soft_limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

/// The module payload of an entry whose magic and digest check out.
fn intact_payload(bytes: &[u8]) -> Option<&[u8]> {
    let (header, payload) = bytes.split_at_checked(HEADER_LEN)?;
    let (magic, digest) = header.split_at(MAGIC.len());
    (magic == MAGIC && digest == blake3::hash(payload).as_bytes()).then_some(payload)
}

fn discard(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Creates `dir` private to the current user and rejects a group- or
/// world-writable one (unix; Windows relies on the per-user profile ACL).
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        if fs::metadata(dir)?.permissions().mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cache directory is group- or world-writable",
            ));
        }
    }
    Ok(())
}

/// [`Hasher`] that feeds a blake3 stream; only the byte stream matters, so
/// `finish` is unused.
struct Blake3Hasher(blake3::Hasher);

impl Hasher for Blake3Hasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}
