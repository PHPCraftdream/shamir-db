use super::args::ListArgs;
use super::create::{finish, open, resolve, CliIo};
use crate::config::Config;
use crate::security::tokens::{TokenStore, TokenView};
use shamir_connect::server::token::{TokenChannels, TokenMode};
pub(super) async fn run_list(c: &Config, a: ListArgs, io: &mut CliIo<'_>) -> anyhow::Result<()> {
    let (g, m, d, w, app) = open(c)?;
    let owner = a
        .user
        .as_deref()
        .map(|u| resolve(&d, u).map(|x| x.user_id))
        .transpose()?;
    let store = TokenStore::open(&c.data_dir).await?;
    let rows = store.scan().await;
    drop(store);
    let output = rows.map(|r| {
        render_list(
            &r.iter().map(TokenView::from).collect::<Vec<_>>(),
            &d,
            owner,
        )
    });
    let cp = finish(&w, &app, &m).await;
    drop(d);
    drop(m);
    drop(app);
    drop(w);
    drop(g);
    let text = match (output, cp) {
        (Ok(s), Ok(())) => s,
        (Err(e), Err(_)) => {
            return Err(anyhow::Error::from(e).context("the audit checkpoint also failed"))
        }
        (Err(e), _) => return Err(anyhow::Error::from(e)),
        (_, Err(_)) => return Err(anyhow::anyhow!("audit checkpoint failed")),
    };
    std::io::Write::write_all(io.stdout, text.as_bytes())?;
    Ok(())
}
pub fn render_list(
    rows: &[TokenView],
    dir: &crate::user_directory::FjallUserDirectory,
    owner: Option<[u8; 16]>,
) -> String {
    let mut ordered = rows
        .iter()
        .filter(|r| owner.is_none_or(|id| id == r.owner_user_id))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|r| r.id);
    let mut out = String::new();
    for r in ordered {
        let name = dir
            .state_by_user_id(&r.owner_user_id)
            .map(|u| u.username)
            .unwrap_or_else(|| "<unknown>".into());
        let db = match r.databases.as_deref() {
            Some(v) => v
                .iter()
                .map(|x| super::create::escape(x))
                .collect::<Vec<_>>()
                .join(","),
            None => "*".into(),
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            super::create::id_text(r.id),
            super::create::escape(&r.label),
            super::create::escape(&name),
            match r.mode {
                TokenMode::ReadOnly => "read_only",
                TokenMode::ReadWrite => "read_write",
            },
            match r.channels {
                TokenChannels::Rest => "http",
                TokenChannels::Ws => "ws",
                TokenChannels::Both => "both",
            },
            db,
            r.created_at_ns,
            r.expires_at_ns,
            r.revoked_at_ns.map(|x| x.to_string()).unwrap_or_default()
        ));
    }
    out
}
