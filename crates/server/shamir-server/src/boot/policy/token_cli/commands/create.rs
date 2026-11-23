use super::{args::CreateArgs, lock, secret_output};
use crate::{
    audit_appender::FjallAuditAppender,
    config::Config,
    engine::server_meta::ServerMetaStore,
    security::tokens::{
        DirectoryOwnerFloors, NewToken, TokenAuditContext, TokenService, TokenServiceOptions,
    },
    user_directory::{FjallUserDirectory, UserDirectoryState},
};
use base64::Engine;
use shamir_connect::server::{
    audit_chain::{AuditChain, AuditChainWriter, AuditError},
    token::{TokenChannels, TokenMode},
    TokenSessionStore,
};
use shamir_sync::format_stamp::{check_data_dir, EpochPolicy, IgnoreSet, StampMode};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use zeroize::Zeroizing;
/// I/O seam for tests and the process entry point.
pub struct CliIo<'a> {
    pub stdout: &'a mut dyn std::io::Write,
    pub stderr: &'a mut dyn std::io::Write,
    pub is_terminal: bool,
}
pub async fn run_create(c: &Config, a: CreateArgs, io: &mut CliIo<'_>) -> anyhow::Result<()> {
    if a.out_file.is_none() && !io.is_terminal {
        anyhow::bail!("create requires terminal or --out-file; no token was created");
    }
    // Lock FIRST: a live server is refused before anything is created.
    // Held to the end of this function: the lock must outlive every store
    // handle and cover delivery and cleanup.
    let _guard = prepare_dir_and_lock(c)?;
    // Validate the destination while the lock is held, BEFORE recovery can
    // recreate a missing active log and BEFORE any output file exists; the
    // returned path is the validated destination used for creation AND
    // cleanup, so no later link resolution can differ.
    let validated_out = match a.out_file.as_deref() {
        Some(path) => {
            Some(secret_output::validate_out_file(path, &c.data_dir).map_err(anyhow::Error::new)?)
        }
        None => None,
    };
    let output = secret_output::prepare(validated_out.as_deref(), io.is_terminal)?;
    // Identity of the file THIS process just created, captured while no other
    // process can have replaced it yet (the exclusive create just succeeded).
    // Cleanup below removes the path only while it still names this file; a
    // `None` identity removes nothing, which is the safe direction.
    let identity = validated_out.as_deref().and_then(file_identity);
    let setup = async {
        let (m, d, w, app) = open_locked(c)?;
        let owner = resolve(&d, &a.user)?;
        let channel = parse_channels(&a.channels)?;
        let svc = service(c, &m, d.clone(), w.clone()).await?;
        anyhow::Ok((m, d, w, app, owner, channel, svc))
    }
    .await;
    let (m, d, w, app, owner, channel, svc) = match setup {
        Ok(v) => v,
        Err(e) => {
            drop(output);
            remove_out_file(validated_out.as_deref(), identity);
            return Err(e);
        }
    };
    let result = svc
        .create_with_context(
            NewToken {
                owner_user_id: owner.user_id,
                owner_roles: owner.roles,
                label: a.label.clone(),
                ttl_secs: a.ttl,
                mode: if a.read_write {
                    TokenMode::ReadWrite
                } else {
                    TokenMode::ReadOnly
                },
                channels: channel,
                databases: if a.databases.is_empty() {
                    None
                } else {
                    Some(a.databases)
                },
            },
            TokenAuditContext {
                actor_username: String::new(),
                transport: "cli".to_owned(),
                session_id_prefix: [0; 8],
            },
        )
        .await;
    let prepared = result.map(|created| {
        let id = id_text(created.record_view.id);
        let databases = match created.record_view.databases.as_deref() {
            Some(values) => values.iter().map(|v| escape(v)).collect::<Vec<_>>().join(","),
            None => "*".to_owned(),
        };
        let text = format!(
            "id={}\tlabel={}\towner={}\tmode={}\tchannels={}\tdatabases={}\tcreated_at_ns={}\texpires_at_ns={}\n",
            id, escape(&created.record_view.label), escape(&a.user),
            mode_text(created.record_view.mode), channel_text(created.record_view.channels), databases,
            created.record_view.created_at_ns, created.record_view.expires_at_ns
        );
        (created.token, text, id)
    }).map_err(anyhow::Error::from);
    let cp = finish(&w, &app, &m).await;
    // Drop every store handle BEFORE delivery or cleanup; the lock file is
    // released only when this function returns.
    drop(svc);
    drop(d);
    drop(m);
    drop(app);
    drop(w);
    let (secret, text, token_id) = match (prepared, cp) {
        (Ok(v), Ok(())) => v,
        (Ok((_, _, id)), Err(e)) => {
            // Nothing was delivered: this file still holds no secret, so the
            // created-but-unusable output must not be left behind.
            drop(output);
            remove_out_file(validated_out.as_deref(), identity);
            // `finish` reports a latched sequence exhaustion distinctly: the
            // token exists but its audit event was never written, so this is
            // NOT the generic checkpoint failure below.
            let unaudited = e.chain().any(|cause| {
                matches!(
                    cause.downcast_ref::<AuditError>(),
                    Some(AuditError::SequenceExhaustedUnaudited)
                )
            });
            if unaudited {
                return Err(anyhow::anyhow!(
                    "token {id} WAS created but an audit event was NOT written: the \
                     audit sequence space is exhausted, so the audit trail for this \
                     token does not exist; revoke it with `token revoke {id}`"
                ));
            }
            return Err(anyhow::anyhow!(
                "token {id} was created but the audit checkpoint failed; revoke it with `token revoke {id}`"
            ));
        }
        (Err(e), Ok(())) => {
            drop(output);
            remove_out_file(validated_out.as_deref(), identity);
            return Err(e);
        }
        (Err(e), Err(_)) => {
            // Token creation failed too, so the file holds no secret either.
            drop(output);
            remove_out_file(validated_out.as_deref(), identity);
            return Err(e.context("the audit checkpoint also failed"));
        }
    };
    // No removal from here on: the file then holds the legitimately delivered
    // secret and the operator needs it to revoke.
    secret_output::deliver(output, &secret, io.stdout, io.stderr)
        .map_err(|_| anyhow::anyhow!("token {token_id} was created but secret delivery failed"))?;
    std::io::Write::write_all(io.stdout, text.as_bytes())
        .map_err(|_| anyhow::anyhow!("token {token_id} was created; metadata output failed"))?;
    Ok(())
}
/// Identity of the directory entry at `path`, taken WITHOUT following a
/// final link, so cleanup can tell the file this process created from a
/// replacement that reused the same name.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}
/// Identity of the directory entry at `path`, taken WITHOUT following a
/// final reparse point (`FILE_FLAG_OPEN_REPARSE_POINT`), so cleanup can tell
/// the file this process created from a replacement that reused the name.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    index: [u8; 16],
}
/// `(device, inode)` of the entry itself — `symlink_metadata`, never
/// `metadata`, so a final symlink reports the link and not its target.
#[cfg(unix)]
fn file_identity(path: &Path) -> Option<FileIdentity> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(FileIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}
/// `(volume serial, 128-bit file id)` of the entry itself. The path is opened
/// with `FILE_FLAG_OPEN_REPARSE_POINT`, so a final reparse point reports the
/// link rather than its target — the same object `symlink_metadata` reports on
/// Unix — and the handle is closed on every path.
#[cfg(windows)]
fn file_identity(path: &Path) -> Option<FileIdentity> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return None;
    }
    wide.push(0);
    // SAFETY: `wide` is a valid null-terminated UTF-16 path; no security
    // attributes and no template handle are passed. The returned handle is
    // either `INVALID_HANDLE_VALUE`/null (rejected below) or owned here and
    // closed below on every path. The share mode admits the exclusive-create
    // handle `prepare` still holds open.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `handle` is valid here, and `info` is a correctly sized and
    // aligned out-buffer for `FileIdInfo`; the call only writes it.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut info as *mut FILE_ID_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    // SAFETY: `handle` came from the `CreateFileW` above and is closed exactly
    // once, on both the success and the failure path.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(FileIdentity {
        volume: info.VolumeSerialNumber,
        index: info.FileId.Identifier,
    })
}
/// Remove the output file created by [`secret_output::prepare`] — but only if
/// the path still names the SAME file: a replacement that reused the name is
/// left alone, and an unknown identity removes nothing.
fn remove_out_file(path: Option<&Path>, expected: Option<FileIdentity>) {
    let (Some(path), Some(expected)) = (path, expected) else {
        return;
    };
    if file_identity(path) == Some(expected) {
        let _ = std::fs::remove_file(path);
    }
}
fn parse_channels(s: &str) -> anyhow::Result<TokenChannels> {
    match s {
        "http" => Ok(TokenChannels::Rest),
        "ws" => Ok(TokenChannels::Ws),
        "both" => Ok(TokenChannels::Both),
        _ => anyhow::bail!("invalid channels"),
    }
}
pub(super) fn parse_ttl(s: &str) -> Result<u64, String> {
    let i = s.find(|c: char| !c.is_ascii_digit()).ok_or("invalid ttl")?;
    let n: u64 = s[..i].parse().map_err(|_| "invalid ttl")?;
    let m = match &s[i..] {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err("invalid ttl".into()),
    };
    n.checked_mul(m)
        .filter(|v| *v > 0)
        .ok_or_else(|| "invalid ttl".into())
}
pub(super) fn resolve(d: &FjallUserDirectory, n: &str) -> anyhow::Result<UserDirectoryState> {
    d.list_all()?
        .into_iter()
        .map(|(_, s)| s)
        .find(|s| s.username == n)
        .ok_or_else(|| anyhow::anyhow!("unknown user"))
}
pub(super) fn id_text(id: [u8; 16]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id)
}
pub(super) fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
fn mode_text(v: TokenMode) -> &'static str {
    match v {
        TokenMode::ReadOnly => "read_only",
        TokenMode::ReadWrite => "read_write",
    }
}
fn channel_text(v: TokenChannels) -> &'static str {
    match v {
        TokenChannels::Rest => "http",
        TokenChannels::Ws => "ws",
        TokenChannels::Both => "both",
    }
}
/// Exclusive lock, server meta, user directory, shared audit writer and its appender.
pub(super) type Opened = (
    std::fs::File,
    ServerMetaStore,
    Arc<FjallUserDirectory>,
    Arc<AuditChainWriter>,
    Arc<FjallAuditAppender>,
);
/// Create the data directory if needed (matching the old lock-last layout
/// on a fresh directory), then take the classified offline maintenance lock.
fn prepare_dir_and_lock(c: &Config) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(&c.data_dir)?;
    lock::acquire(&c.data_dir).map_err(anyhow::Error::new)
}
/// Server meta, user directory, shared audit writer and its appender.
type Stores = (
    ServerMetaStore,
    Arc<FjallUserDirectory>,
    Arc<AuditChainWriter>,
    Arc<FjallAuditAppender>,
);
pub(super) fn open(c: &Config) -> anyhow::Result<Opened> {
    let guard = prepare_dir_and_lock(c)?;
    let (m, d, w, app) = open_locked(c)?;
    Ok((guard, m, d, w, app))
}
/// Everything after the maintenance lock is held: the CALLER must hold the
/// shared offline maintenance lock, and its guard file must stay alive
/// longer than every returned store handle.
fn open_locked(c: &Config) -> anyhow::Result<Stores> {
    // Format-stamp gate (design 3.3: Writer — this command creates
    // server_meta in an empty directory).
    check_data_dir(
        &c.data_dir,
        StampMode::Writer,
        &IgnoreSet::none(),
        &EpochPolicy::compiled(),
    )?;
    let m = ServerMetaStore::open_or_init(c.data_dir.join("server_meta"))?;
    // Load the key once; the strict helper authenticates with the same copy.
    let key = Zeroizing::new(m.audit_chain_key()?);
    // Shared strict recovery (finding 2): the SAME routine boot uses, against
    // BOTH trusted anchors; on failure refuse — never resume from an
    // unverified tail. The typed error is preserved as anyhow's source (the
    // entry point prints `{error:#}`).
    let (app, (tail_seq, tail_hmac)) =
        m.open_audit_recovered_strict(&c.data_dir, &key)
            .map_err(|e| {
                anyhow::Error::new(e).context(
                    "the audit history in this data directory failed verification; no token \
                 command ran. Keep the server STOPPED, verify the audit history, and restore \
                 an audit-consistent backup before retrying",
                )
            })?;
    let w = Arc::new(AuditChainWriter::new_with_shared(
        Arc::new(AuditChain::from_checkpoint(*key, tail_seq, tail_hmac)),
        app.clone(),
    ));
    let d = Arc::new(FjallUserDirectory::open(c.data_dir.join("users"))?);
    Ok((m, d, w, app))
}
pub(super) async fn service(
    c: &Config,
    m: &ServerMetaStore,
    d: Arc<FjallUserDirectory>,
    w: Arc<AuditChainWriter>,
) -> anyhow::Result<TokenService> {
    let k = m.token_key()?;
    TokenService::open(TokenServiceOptions {
        data_dir: c.data_dir.clone(),
        token_key: Zeroizing::new(*k.as_bytes()),
        owners: Arc::new(DirectoryOwnerFloors::new(d)),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: w,
        config: c.tokens.clone(),
        read_only_node: false, // Offline operator mutation on a stopped node, including a read-only replica.
        clock: Arc::new(|| {
            shamir_connect::common::time::UnixNanos::try_now()
                .ok()
                .map(|n| n.as_u64())
        }),
    })
    .await
    .map_err(Into::into)
}
pub(super) async fn finish(
    w: &AuditChainWriter,
    a: &FjallAuditAppender,
    m: &ServerMetaStore,
) -> anyhow::Result<()> {
    w.checkpoint_now()?;
    let cp = w.chain().checkpoint();
    a.flush_now().await?;
    m.store_audit_checkpoint(cp.0, cp.1)?;
    Ok(())
}
