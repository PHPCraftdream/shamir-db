use super::{args::CreateArgs, secret_output};
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
    audit_chain::{AuditChain, AuditChainWriter},
    token::{TokenChannels, TokenMode},
    TokenSessionStore,
};
use std::sync::Arc;
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
    let output = secret_output::prepare(a.out_file.as_deref(), io.is_terminal)?;
    let setup = async {
        let (g, m, d, w, app) = open(c)?;
        let owner = resolve(&d, &a.user)?;
        let channel = parse_channels(&a.channels)?;
        let svc = service(c, &m, d.clone(), w.clone()).await?;
        anyhow::Ok((g, m, d, w, app, owner, channel, svc))
    }
    .await;
    let (g, m, d, w, app, owner, channel, svc) = match setup {
        Ok(v) => v,
        Err(e) => {
            remove_out_file(a.out_file.as_deref());
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
    drop(svc);
    drop(d);
    drop(m);
    drop(app);
    drop(w);
    drop(g);
    let (secret, text, token_id) = match (prepared, cp) {
        (Ok(v), Ok(())) => v,
        (Ok((_, _, id)), Err(_)) => {
            return Err(anyhow::anyhow!(
                "token {id} was created but the audit checkpoint failed; revoke it with `token revoke {id}`"
            ))
        }
        (Err(e), Ok(())) => {
            remove_out_file(a.out_file.as_deref());
            return Err(e);
        }
        (Err(e), Err(_)) => return Err(e.context("the audit checkpoint also failed")),
    };
    secret_output::deliver(output, &secret, io.stdout, io.stderr)
        .map_err(|_| anyhow::anyhow!("token {token_id} was created but secret delivery failed"))?;
    std::io::Write::write_all(io.stdout, text.as_bytes())
        .map_err(|_| anyhow::anyhow!("token {token_id} was created; metadata output failed"))?;
    Ok(())
}
fn remove_out_file(path: Option<&std::path::Path>) {
    if let Some(path) = path {
        let _ = std::fs::remove_file(path);
    }
}
pub(super) fn parse_channels(s: &str) -> anyhow::Result<TokenChannels> {
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
pub(super) fn open(c: &Config) -> anyhow::Result<Opened> {
    std::fs::create_dir_all(&c.data_dir)?;
    let g = crate::maintenance_lock::acquire(&c.data_dir)
        .map_err(|_| anyhow::anyhow!("offline token command requires a stopped server"))?;
    let m = ServerMetaStore::open_or_init(c.data_dir.join("server_meta"))?;
    let key = m.audit_chain_key()?;
    let log = FjallAuditAppender::read_log_for_verify(&c.data_dir)?;
    // Rotation, retention and online restarts can leave a log that does not verify from
    // genesis; the operator command then resumes from the log tail instead of refusing.
    if AuditChain::verify_chain(&key, &log).is_ok() {
        let ac = FjallAuditAppender::load_checkpoint(&c.data_dir)?;
        let mc = m.audit_checkpoint();
        for cp in [ac, mc].into_iter().flatten() {
            AuditChain::verify_against_checkpoint(&log, cp.0, &cp.1)?;
        }
    } else {
        tracing::warn!("audit log does not verify from genesis; resuming from its tail");
    }
    let tail = log
        .last()
        .map(|e| (e.seq.saturating_add(1), e.hmac))
        .unwrap_or((1, [0; 32]));
    let app = FjallAuditAppender::open_strict(&c.data_dir)?;
    let w = Arc::new(AuditChainWriter::new_with_shared(
        Arc::new(AuditChain::from_checkpoint(key, tail.0, tail.1)),
        app.clone(),
    ));
    let d = Arc::new(FjallUserDirectory::open(c.data_dir.join("users"))?);
    Ok((g, m, d, w, app))
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
