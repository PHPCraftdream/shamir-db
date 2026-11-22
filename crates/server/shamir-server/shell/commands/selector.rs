//! The rights path selector used by `rights show` and the three rights
//! mutations (`design admin-shell.md` §4.3). It is parsed LOCALLY before any
//! dial or secret read, and the path is never sent as a `ResourceRef`: the
//! access tree is fetched and the addressed node is selected on the client.
//!
//! Accepted forms:
//! - `root` or `/` — the synthetic resource root (never mutable);
//! - `db:NAME` — ALWAYS the database `NAME`, which is how a database that is
//!   itself called `root` is addressed (the bare `root` keeps meaning the
//!   synthetic root);
//! - `function:NAME` — a stored function by exact name. `NAME` may contain
//!   `/` because a stored function name is a folder path (`math/abs` is ONE
//!   function, not a folder entity);
//! - `DATABASE[/STORE[/TABLE]]` — one to three resource components.
//!
//! The name rules are MIRRORS of engine rules the shell cannot call without
//! a new dependency, so each one names its source:
//! - database and store components, and every non-final function-name
//!   segment, mirror `validate_name_component`
//!   (`crates/engine/shamir-db/src/shamir_db/execute/core/helpers.rs`), which
//!   the engine applies to database/repository names and to function-folder
//!   segments (`execute/admin_meta/admin_function.rs`, `CreateFunctionFolder`);
//! - the table component mirrors the PHYSICAL store-name length rule
//!   (`crates/engine/shamir-engine-table/src/support/model/names.rs`,
//!   `validate_table_store_name`).
//!
//! The server stays the authority for everything the shell does not mirror.

use super::super::output::ShellError;

/// The resource kinds of the three supported resource-path depths.
pub(in crate::shell) const RESOURCE_KINDS: [&str; 3] = ["database", "store", "table"];

/// Longest accepted stored FUNCTION name, in bytes.
///
/// The engine imposes NO length cap and no charset cap on a function name: it
/// persists the registered name as given (`create_function_with_opts_as`,
/// `shamir_db/db/function/management.rs`), and only the folder segments it
/// creates implicitly pass [`valid_folder_segment`]. 255 bytes is therefore a
/// shell-side bound (the fjall/OS name limit the engine's own generated store
/// names obey), not a mirrored engine rule.
pub(in crate::shell) const MAX_FUNCTION_NAME_BYTES: usize = 255;

/// Longest accepted TABLE component, in bytes.
///
/// Derived from the physical store names the engine generates for a created
/// table (`crates/engine/shamir-engine-table/src/support/model/names.rs`,
/// `table_store_names`, checked by `validate_table_store_name` against
/// fjall's 255-byte store limit before a table is created):
/// `255 - "__history__" (11) - "--" (2) - 22` = **220**, where 22 is the
/// longest base58btc encoding of a 16-byte `RecordId` (the `--{id}` suffix;
/// `log2(256^16) / log2(58) = 21.85`, so at most 22 digits). `__history__` is
/// the longest of the three prefixes. A longer table component can never be
/// created by the server, so the shell refuses it instead of signing a
/// request for an impossible target.
pub(in crate::shell) const MAX_TABLE_NAME_BYTES: usize = 220;

/// The target a `rights show` / mutation path selector resolved to.
pub(in crate::shell) enum Selector {
    /// The resource root (`root` or `/`).
    Root,
    /// The database `NAME` of the explicit `db:NAME` form.
    Database(String),
    /// One entry of the `functions` list, selected by exact name.
    Function(String),
    /// One `database` / `store` / `table` path of one, two or three segments.
    Resource(Vec<String>),
}

/// Whether one DATABASE or STORE name component passes the engine's own name
/// rule. MIRRORS `validate_name_component`
/// (`crates/engine/shamir-db/src/shamir_db/execute/core/helpers.rs`), which is
/// `pub(super)` inside `shamir-db` and therefore not reachable from this
/// crate: only `[A-Za-z0-9_-]`, never empty, never `.`/`..`. That rule has NO
/// length cap, so neither does this mirror — a length ceiling here would hide
/// databases the engine accepts.
fn valid_name_component(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether one FUNCTION-FOLDER segment passes the engine's rule: the engine
/// validates every `CreateFunctionFolder` segment with `validate_name_component`
/// ("folder segment", `execute/admin_meta/admin_function.rs`).
fn valid_folder_segment(text: &str) -> bool {
    valid_name_component(text)
}

/// Whether the FINAL segment of a `function:NAME` is addressable. The engine
/// stores the function name as given and validates no charset for it, so the
/// shell only refuses what it cannot represent: an empty name, a `:` (the
/// form's own separator) or a control byte.
fn valid_function_leaf(text: &str) -> bool {
    !text.is_empty() && !text.contains(':') && !text.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
}

/// Whether one TABLE component can exist as a physical store name
/// ([`MAX_TABLE_NAME_BYTES`]).
fn valid_table_component(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TABLE_NAME_BYTES
}

/// Parse a `function:NAME` payload: slash-separated segments, every non-final
/// segment a function-folder segment and the final one a stored function
/// name, the whole at most [`MAX_FUNCTION_NAME_BYTES`] bytes.
fn parse_function_name(name: &str) -> Result<(), ShellError> {
    if name.is_empty() || name.len() > MAX_FUNCTION_NAME_BYTES {
        return Err(ShellError::Usage);
    }
    let segments: Vec<&str> = name.split('/').collect();
    let (leaf, folders) = segments.split_last().ok_or(ShellError::Usage)?;
    if folders.iter().any(|segment| !valid_folder_segment(segment)) || !valid_function_leaf(leaf) {
        return Err(ShellError::Usage);
    }
    Ok(())
}

/// Parse the `rights show` / mutation path selector. `root` and `/` name the
/// resource root; `db:NAME` names a database explicitly; `function:NAME` names
/// a stored function by exact name; otherwise the path is split on `/` into
/// exactly one, two or three non-empty segments selecting a `database`,
/// `store` or `table` node. A component the mirrored engine rule rejects — or
/// a segment count the selector does not support — is `ShellError::Usage`
/// before any request.
pub(in crate::shell) fn parse_selector(path: &str) -> Result<Selector, ShellError> {
    if path == "root" || path == "/" {
        return Ok(Selector::Root);
    }
    if let Some(name) = path.strip_prefix("db:") {
        if !valid_name_component(name) {
            return Err(ShellError::Usage);
        }
        return Ok(Selector::Database(name.to_string()));
    }
    if let Some(name) = path.strip_prefix("function:") {
        parse_function_name(name)?;
        return Ok(Selector::Function(name.to_string()));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.is_empty() || segments.len() > RESOURCE_KINDS.len() {
        return Err(ShellError::Usage);
    }
    for (depth, segment) in segments.iter().enumerate() {
        let accepted = match depth {
            // The database and store components must satisfy the engine's own
            // name rule before they can become a wire target.
            0 | 1 => valid_name_component(segment),
            // The table component must fit the physical store name the engine
            // generates; the server stays the authority for its content.
            _ => valid_table_component(segment),
        };
        if !accepted {
            return Err(ShellError::Usage);
        }
    }
    Ok(Selector::Resource(
        segments.into_iter().map(str::to_string).collect(),
    ))
}

/// The normalized `path` output of a selector: the form the operator used for
/// the two explicit prefixes, and the slash-joined components otherwise.
pub(in crate::shell) fn normalized_path(selector: &Selector) -> String {
    match selector {
        Selector::Root => "/".to_string(),
        Selector::Database(name) => format!("db:{name}"),
        Selector::Function(name) => format!("function:{name}"),
        Selector::Resource(segments) => segments.join("/"),
    }
}

#[cfg(test)]
#[path = "tests/selector_tests.rs"]
mod selector_tests;
