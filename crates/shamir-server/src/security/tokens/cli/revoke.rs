use super::create::{finish, open, resolve, service, CliIo};
use crate::config::Config;
use crate::security::tokens::TokenStore;
use base64::Engine;
use shamir_connect::server::token::TokenId;
pub enum Action {
    One(String),
    User(String),
    All,
}
pub fn parse_id(s: &str) -> Result<String, String> {
    if s.len() != 22 {
        return Err("token id must be 22 base64url characters".into());
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| "token id must be base64url".to_owned())?;
    if raw.len() != 16 {
        return Err("token id must decode to 16 bytes".into());
    }
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw) != s {
        return Err("token id must use canonical base64url".into());
    }
    Ok(s.to_owned())
}
fn id(s: &str) -> anyhow::Result<TokenId> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parse_id(s).map_err(anyhow::Error::msg)?)?;
    Ok(TokenId(
        raw.try_into()
            .map_err(|_| anyhow::anyhow!("invalid token id"))?,
    ))
}
pub async fn run_revoke(c: &Config, a: Action, io: &mut CliIo<'_>) -> anyhow::Result<()> {
    let (g, m, d, w, app) = open(c)?;
    let all_ids = if matches!(a, Action::All) {
        let store = TokenStore::open(&c.data_dir).await?;
        let rows = store.scan().await?;
        drop(store);
        Some(
            rows.into_iter()
                .map(|r| r.owner_user_id)
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let svc = service(c, &m, d.clone(), w.clone()).await?;
    let result: anyhow::Result<String> = async {
        let now = shamir_connect::common::time::UnixNanos::try_now()?.as_u64();
        match a {
            Action::One(x) => Ok(format!(
                "{}\n",
                if svc.revoke(id(&x)?, now).await? {
                    "revoked=1"
                } else {
                    "already_revoked=1"
                }
            )),
            Action::User(n) => {
                let u = resolve(&d, &n)?.user_id;
                Ok(format!("revoked={}\n", svc.revoke_user(u, now).await?))
            }
            Action::All => {
                let mut owners = all_ids.unwrap_or_default();
                owners.sort_unstable();
                owners.dedup();
                let mut count = 0usize;
                for u in owners {
                    count += svc.revoke_user(u, now).await?;
                }
                Ok(format!("revoked={count}\n"))
            }
        }
    }
    .await;
    let cp = finish(&w, &app, &m).await;
    drop(svc);
    drop(d);
    drop(m);
    drop(app);
    drop(w);
    drop(g);
    let out = match (result, cp) {
        (Ok(s), Ok(())) => s,
        (Err(e), Err(_)) => return Err(e.context("the audit checkpoint also failed")),
        (Err(e), _) => return Err(e),
        (_, Err(_)) => {
            return Err(anyhow::anyhow!(
                "mutation may have happened; audit checkpoint failed"
            ))
        }
    };
    std::io::Write::write_all(io.stdout, out.as_bytes())?;
    Ok(())
}
