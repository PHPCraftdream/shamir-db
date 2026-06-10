//! `access-tree` CLI command — render the Shomer access-control tree.
//!
//! Two modes, one binary:
//!
//! * **offline** (default) — open `data_dir` directly via [`ShamirDb::init`]
//!   and assemble the tree as the `System` actor. Works when the server is
//!   stopped; requires exclusive access to the redb files (single-writer),
//!   so the server must not be running.
//! * **online** (`--connect host:port`) — authenticate to a running server
//!   over TLS+SCRAM as an admin and request the tree via the `access_tree`
//!   DDL op. The server gates it on `Manage` of the root, so a non-admin
//!   user is denied.
//!
//! Both modes obtain the same `access_tree` JSON; output is either the raw
//! JSON (`--json`) or a rendered ASCII tree.

use std::net::SocketAddr;

use anyhow::{anyhow, Context};
use serde_json::Value;
use zeroize::Zeroizing;

use shamir_client::{Client, ConnectOptions};
use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;

use crate::config::Config;

/// Parsed `access-tree` arguments (mirrors the clap subcommand in `main`).
#[derive(Debug, Clone, Default)]
pub struct AccessTreeArgs {
    /// Resource-depth cap: 0=root, 1=db, 2=store, 3=table. `None` = full.
    pub depth: Option<u32>,
    /// Restrict the resource tree to a single database.
    pub db: Option<String>,
    /// Emit raw JSON instead of the rendered ASCII tree.
    pub json: bool,
    /// Online mode: connect to a running server at `host:port`.
    pub connect: Option<String>,
    /// SNI hostname for TLS in online mode (matches the server cert).
    pub server_name: String,
    /// Username for online mode (must be an admin).
    pub user: Option<String>,
    /// Password for online mode; falls back to `$SHAMIR_PASSWORD`.
    pub password: Option<String>,
}

/// Fetch the access tree as JSON — offline (open `data_dir`) or online
/// (`--connect`). The rendering/printing is left to the caller so this is
/// directly testable.
pub async fn fetch_tree(config: &Config, args: &AccessTreeArgs) -> anyhow::Result<Value> {
    match &args.connect {
        Some(addr) => fetch_online(args, addr).await,
        None => fetch_offline(config, args).await,
    }
}

/// Run the command: fetch the tree (offline or online) and print it.
pub async fn run(config: &Config, args: &AccessTreeArgs) -> anyhow::Result<()> {
    let tree = fetch_tree(config, args).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&tree)?);
    } else {
        println!("{}", render(&tree));
    }
    Ok(())
}

/// Offline: open the durable system store and assemble the tree directly.
async fn fetch_offline(config: &Config, args: &AccessTreeArgs) -> anyhow::Result<Value> {
    let meta_path = config.data_dir.join("shamir_db_meta.redb");

    // redb is single-writer: a fresh open fails while the file lock is still
    // held. In normal use the server is stopped (separate process exited) and
    // the lock is free; but the OS can lag releasing a just-dropped handle
    // (e.g. right after a same-host stop), so retry briefly before giving up.
    // A genuinely-running server holds the lock past this window → we surface
    // the clear "is the server stopped?" error.
    let shamir = {
        let mut last_err = None;
        let mut opened = None;
        for _ in 0..20 {
            match ShamirDb::init(SystemStoreConfig::Redb(meta_path.clone())).await {
                Ok(db) => {
                    opened = Some(db);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
        match opened {
            Some(db) => db,
            None => {
                return Err(anyhow!(
                    "open data_dir (is the server stopped?): {}",
                    last_err.expect("at least one attempt failed")
                ))
            }
        }
    };

    shamir
        .access_tree(args.depth, args.db.as_deref())
        .await
        .map_err(|e| anyhow!("assemble access tree: {e}"))
}

/// Online: SCRAM-authenticate to a running server and request the tree.
async fn fetch_online(args: &AccessTreeArgs, addr: &str) -> anyhow::Result<Value> {
    let user = args
        .user
        .as_deref()
        .ok_or_else(|| anyhow!("--user is required with --connect"))?;
    let password = args
        .password
        .clone()
        .or_else(|| std::env::var("SHAMIR_PASSWORD").ok())
        .ok_or_else(|| anyhow!("--password or $SHAMIR_PASSWORD is required with --connect"))?;
    let sockaddr: SocketAddr = addr
        .parse()
        .with_context(|| format!("invalid --connect address '{addr}'"))?;

    let client = Client::connect(ConnectOptions {
        addr: sockaddr,
        server_name: args.server_name.clone(),
        username: user.to_string(),
        password: Zeroizing::new(password.into_bytes()),
        accept_new_host: true,
        trusted_pin: None,
    })
    .await
    .map_err(|e| anyhow!("connect/authenticate: {e}"))?;

    let mut ddl = shamir_query_builder::ddl::access_tree();
    if let Some(d) = args.depth {
        ddl = ddl.depth(d);
    }
    if let Some(db) = &args.db {
        ddl = ddl.db(db);
    }
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id(1).access_tree("tree", ddl);
    let batch = batch.build();

    let resp = client
        .execute("default", batch)
        .await
        .map_err(|e| anyhow!("execute access_tree: {e}"))?;

    let qr = resp
        .results
        .get("tree")
        .ok_or_else(|| anyhow!("server returned no 'tree' result"))?;
    let rec = qr
        .records
        .first()
        .ok_or_else(|| anyhow!("empty access_tree result"))?;
    Ok(rec.get("access_tree").cloned().unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the access-tree JSON as a human-readable ASCII tree.
pub fn render(tree: &Value) -> String {
    let mut out = String::new();

    // ── resources ──
    let mut rows: Vec<(String, String, String)> = Vec::new();
    if let Some(root) = tree.get("resources") {
        collect_rows(root, "", true, true, &mut rows);
    }
    push_aligned(&mut out, &rows);

    // ── functions ──
    if let Some(fns) = tree.get("functions").and_then(|v| v.as_array()) {
        if !fns.is_empty() {
            out.push_str("\nfunctions\n");
            let mut frows: Vec<(String, String, String)> = Vec::new();
            for (i, f) in fns.iter().enumerate() {
                let last = i + 1 == fns.len();
                let connector = if last { "└─ " } else { "├─ " };
                let mut label = format!("{connector}{}", f["name"].as_str().unwrap_or("?"));
                if f["builtin"].as_bool().unwrap_or(false) {
                    label.push_str(" (builtin)");
                } else if f["setuid"].as_bool().unwrap_or(false) {
                    label.push_str(" (setuid)");
                }
                frows.push((label, owner_group(f), mode_str(mode_of(f))));
            }
            push_aligned(&mut out, &frows);
        }
    }

    // ── principals ──
    if let Some(p) = tree.get("principals") {
        out.push_str("\nprincipals\n");
        let users = p["users"].as_array().cloned().unwrap_or_default();
        let user_strs: Vec<String> = users
            .iter()
            .map(|u| {
                format!(
                    "{}({})",
                    u["name"].as_str().unwrap_or("?"),
                    u["id"].as_u64().unwrap_or(0)
                )
            })
            .collect();
        out.push_str(&format!("├─ users:  {}\n", user_strs.join(" ")));

        let groups = p["groups"].as_array().cloned().unwrap_or_default();
        let group_strs: Vec<String> = groups
            .iter()
            .map(|g| {
                let members: Vec<String> = g["members"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|m| m["name"].as_str().unwrap_or("?").to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                format!(
                    "{}({})=[{}]",
                    g["name"].as_str().unwrap_or("?"),
                    g["id"].as_u64().unwrap_or(0),
                    members.join(",")
                )
            })
            .collect();
        out.push_str(&format!("└─ groups: {}\n", group_strs.join(" ")));
    }

    out
}

/// Walk a resource node, appending `(tree_line, owner:group, mode)` rows.
fn collect_rows(
    node: &Value,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    out: &mut Vec<(String, String, String)>,
) {
    let connector = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    let line = format!("{prefix}{connector}{}", node_label(node));
    out.push((line, owner_group(node), mode_str(mode_of(node))));

    let children = match node.get("children").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => return,
    };
    let child_prefix = if is_root {
        String::new()
    } else {
        format!("{prefix}{}", if is_last { "   " } else { "│  " })
    };
    for (i, child) in children.iter().enumerate() {
        collect_rows(child, &child_prefix, i + 1 == children.len(), false, out);
    }
}

/// Human label for a resource node, by kind.
fn node_label(node: &Value) -> String {
    let name = node["name"].as_str().unwrap_or("?");
    match node["kind"].as_str().unwrap_or("") {
        "root" => "/".to_string(),
        "database" => format!("db {name}"),
        "store" => format!("store {name}"),
        "table" => format!("table {name}"),
        other => format!("{other} {name}"),
    }
}

/// `owner:group` label, preferring resolved names over numeric ids.
fn owner_group(node: &Value) -> String {
    let owner = node["owner_name"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| node["owner"].as_u64().unwrap_or(0).to_string());
    let group = node["group_name"]
        .as_str()
        .map(str::to_string)
        .or_else(|| node["group"].as_u64().map(|g| g.to_string()))
        .unwrap_or_else(|| "-".to_string());
    format!("{owner}:{group}")
}

fn mode_of(node: &Value) -> u16 {
    node["mode"].as_u64().unwrap_or(0) as u16
}

/// POSIX `rwx` rendering of a mode, with the setuid bit folding into the
/// owner-execute slot (`s`/`S` like `ls -l`).
pub fn mode_str(mode: u16) -> String {
    let bit = |b: u16, c: char| if mode & b != 0 { c } else { '-' };
    let setuid = mode & 0o4000 != 0;
    let owner_x = mode & 0o100 != 0;
    let owner_x_char = match (setuid, owner_x) {
        (true, true) => 's',
        (true, false) => 'S',
        (false, true) => 'x',
        (false, false) => '-',
    };
    let mut s = String::with_capacity(9);
    s.push(bit(0o400, 'r'));
    s.push(bit(0o200, 'w'));
    s.push(owner_x_char);
    s.push(bit(0o040, 'r'));
    s.push(bit(0o020, 'w'));
    s.push(bit(0o010, 'x'));
    s.push(bit(0o004, 'r'));
    s.push(bit(0o002, 'w'));
    s.push(bit(0o001, 'x'));
    s
}

/// Append rows, aligning the `owner:group` and `mode` columns.
fn push_aligned(out: &mut String, rows: &[(String, String, String)]) {
    let tree_w = rows
        .iter()
        .map(|(l, _, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let og_w = rows
        .iter()
        .map(|(_, og, _)| og.chars().count())
        .max()
        .unwrap_or(0);
    for (line, og, mode) in rows {
        out.push_str(&format!(
            "{line:<tree_w$}   {og:<og_w$}   {mode}\n",
            tree_w = tree_w,
            og_w = og_w,
        ));
    }
}
