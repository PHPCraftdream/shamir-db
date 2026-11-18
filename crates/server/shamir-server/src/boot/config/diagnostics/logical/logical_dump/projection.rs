use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::{
    AclItem, IdentityItem, IdentityKind, LogicalValue, ResourceRef,
};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

/// Allowlisted projections only. Never serialize catalogue rows or credentials.
pub(super) struct Projection {
    directory: Option<Arc<FjallUserDirectory>>,
    pub(super) identities: BTreeMap<String, IdentityItem>,
    pub(super) acls: BTreeMap<Vec<String>, AclItem>,
}

impl Projection {
    pub(super) fn new(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("users");
        let directory = if path.is_dir() {
            Some(Arc::new(FjallUserDirectory::open(path)?))
        } else {
            None
        };
        Ok(Self::from_directory(directory))
    }

    /// Reuse an already-opened user directory (e.g. the live writer's
    /// handle) instead of reopening Fjall underneath it.
    pub(super) fn from_directory(directory: Option<Arc<FjallUserDirectory>>) -> Self {
        Self {
            directory,
            identities: BTreeMap::new(),
            acls: BTreeMap::new(),
        }
    }

    pub(super) fn meta(
        &mut self,
        row: &QueryValue,
        resource: ResourceRef,
    ) -> Result<(Option<String>, Option<String>, u16)> {
        let owner = match row.get("owner") {
            None => None, // legacy open metadata has System ownership
            Some(QueryValue::Int(0)) => None,
            Some(QueryValue::Int(id)) if *id > 0 => {
                let user = self
                    .directory
                    .as_ref()
                    .and_then(|dir| dir.resolve_by_principal64(u64::try_from(*id).ok()?))
                    .ok_or_else(|| {
                        anyhow!("logical-export unresolved ACL owner; explicit mapping required")
                    })?;
                if self.identities.len() >= 4096 {
                    bail!("logical-export identity inventory exceeds scoped 4096 limit");
                }
                let name = user.username;
                if name.len() > 1024 {
                    bail!("logical-export principal name exceeds scoped 1024-byte limit");
                }
                self.identities.insert(
                    name.clone(),
                    IdentityItem {
                        logical_name: name.clone(),
                        kind: IdentityKind::User,
                        disabled_on_import: true,
                    },
                );
                Some(name)
            }
            _ => bail!("logical-export malformed ACL owner"),
        };
        if !matches!(row.get("group"), None | Some(QueryValue::Null)) {
            bail!("logical-export group ACLs and group membership are not implemented");
        }
        let mode = match row.get("mode") {
            None => 0o777,
            Some(QueryValue::Int(mode)) if (0..=0o777).contains(mode) => u16::try_from(*mode)?,
            _ => bail!("logical-export malformed ACL mode"),
        };
        let key = vec![
            resource.database.clone(),
            resource.repository.clone().unwrap_or_default(),
            resource.table.clone().unwrap_or_default(),
            resource.object.clone().unwrap_or_default(),
        ];
        if self.acls.len() >= 4096 {
            bail!("logical-export ACL inventory exceeds scoped 4096 limit");
        }
        if self
            .acls
            .insert(
                key,
                AclItem {
                    resource,
                    owner: owner.clone(),
                    group: None,
                    mode,
                    grants: vec![],
                },
            )
            .is_some()
        {
            bail!("logical-export duplicate ACL resource");
        }
        Ok((owner, None, mode))
    }
}

pub(super) fn fields(row: &QueryValue, allowed: &[&str], family: &str) -> Result<()> {
    let map = row
        .as_object()
        .ok_or_else(|| anyhow!("logical-export malformed {family}"))?;
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("logical-export unsupported {family} field '{key}'");
        }
    }
    Ok(())
}

/// A successfully renamed table keeps the committed physical provenance in
/// its catalogue row (`rename_from`, optional `rename_from_store_id`, written
/// by the rename persist path). Boot validates these fields the same way
/// (`table_catalogue_config.rs`); the export validates them here and never
/// emits them: the portable table item carries only the current name. A
/// rename still in doubt is refused earlier by the repository fence, not
/// here. Presence is coherent: the store id may appear only together with
/// the source name, both types match the persisted string shapes, and the
/// store id parses as a `RecordId`.
pub(super) fn rename_provenance(row: &QueryValue) -> Result<()> {
    match row.get("rename_from") {
        None => {
            if row.get("rename_from_store_id").is_some() {
                bail!("logical-export table rename_from_store_id without rename_from");
            }
        }
        Some(QueryValue::Str(from)) => {
            if from.is_empty() || from.len() > 1024 {
                bail!("logical-export invalid table rename_from");
            }
            match row.get("rename_from_store_id") {
                None => {}
                Some(QueryValue::Str(id)) => {
                    id.parse::<RecordId>().map_err(|error| {
                        anyhow!("logical-export invalid table rename_from_store_id: {error}")
                    })?;
                }
                Some(_) => bail!("logical-export invalid table rename_from_store_id type"),
            }
        }
        Some(_) => bail!("logical-export invalid table rename_from type"),
    }
    Ok(())
}

pub(super) fn text<'a>(row: &'a QueryValue, key: &str) -> Result<&'a str> {
    let value = row
        .get(key)
        .and_then(QueryValue::as_str)
        .ok_or_else(|| anyhow!("logical-export missing/invalid catalogue field '{key}'"))?;
    if value.len() > 1024 {
        bail!("logical-export logical name exceeds scoped 1024-byte limit");
    }
    Ok(value)
}

pub(super) fn resource(
    db: &str,
    repo: Option<&str>,
    table: Option<&str>,
    object: Option<&str>,
) -> ResourceRef {
    ResourceRef {
        database: db.into(),
        repository: repo.map(Into::into),
        table: table.map(Into::into),
        object: object.map(Into::into),
    }
}

/// Stored-shape fidelity. Persisted Dec/Big are flattened to strings and
/// persisted Set to lists, so the export emits what is stored: a stored string
/// as `str`, a stored list as `list`, recursively inside nested maps and lists.
/// The artifact round-trips the stored bytes exactly; the import accepts every
/// envelope tag, so a future exporter tagging dec/big/set needs no import
/// change. Map order, binary, integer and IEEE-754 bits remain exact.
pub(super) fn record_value(value: &QueryValue) -> Result<LogicalValue> {
    let mut nodes = 0;
    let mut bytes = 0;
    record_budget(value, 0, &mut nodes, &mut bytes)?;
    convert_record(value)
}

fn record_budget(
    value: &QueryValue,
    depth: usize,
    nodes: &mut usize,
    bytes: &mut usize,
) -> Result<()> {
    *nodes += 1;
    if depth >= 64 || *nodes > 2048 || *bytes > 512 * 1024 {
        bail!("logical-export record exceeds scoped expansion budget (2048 nodes / 512 KiB / depth 64)");
    }
    match value {
        QueryValue::Map(map) => {
            for (key, child) in map {
                *bytes = bytes.saturating_add(key.len());
                record_budget(child, depth + 1, nodes, bytes)?;
            }
        }
        QueryValue::Bin(value) => {
            *bytes = bytes.saturating_add(value.len());
        }
        QueryValue::List(list) => {
            for child in list {
                record_budget(child, depth + 1, nodes, bytes)?;
            }
        }
        _ => {}
    }
    if *bytes > 512 * 1024 {
        bail!("logical-export record exceeds scoped 512 KiB expansion budget");
    }
    Ok(())
}

fn convert_record(value: &QueryValue) -> Result<LogicalValue> {
    Ok(match value {
        QueryValue::Null => LogicalValue::Null,
        QueryValue::Bool(v) => LogicalValue::Bool(*v),
        QueryValue::Int(v) => LogicalValue::Int(*v),
        QueryValue::F64(v) => LogicalValue::F64(v.to_bits()),
        QueryValue::Bin(v) => LogicalValue::Bin(v.clone()),
        QueryValue::Map(map) => LogicalValue::Map(map.iter().map(|(k, v)| Ok((k.clone(), convert_record(v)?))).collect::<Result<_>>()?),
        QueryValue::Str(v) => LogicalValue::Str(v.clone()),
        QueryValue::List(values) => LogicalValue::List(
            values
                .iter()
                .map(convert_record)
                .collect::<Result<Vec<_>>>()?,
        ),
        QueryValue::Dec(_) | QueryValue::Big(_) => bail!(
            "logical-export cannot prove Dec/Big fidelity from persisted storage; they are flattened to strings"
        ),
        QueryValue::Set(_) => bail!(
            "logical-export cannot prove Set fidelity from persisted storage; it is flattened to a list"
        ),
    })
}
